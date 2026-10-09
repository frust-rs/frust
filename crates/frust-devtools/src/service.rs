//! [`Service::start`] and [`ServiceHandle`] — what a shell actually touches.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use frust_devtools_protocol::{Capability, FrameStats, format_discovery_line};
use tokio::net::TcpListener;
use tokio::runtime::Builder;
use tokio::sync::watch;

use crate::backend::{AppInfo, DevtoolsBackend};
use crate::dispatch::{HotPatch, SessionCtx};
use crate::frame_stats::{self, FrameStatsBus};
use crate::hop::spawn_backend_thread;
use crate::server::accept_loop;
use crate::token::{self, TokenSource};

/// Tuning knobs. [`ServiceConfig::default`] is what [`Service::start`] uses;
/// [`Service::start_with_config`] exists for tests and for a shell with an
/// unusual budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceConfig {
    /// How long a single backend call may take before the waiting client is
    /// answered with an error instead. This is the promise that a wedged UI
    /// thread costs a client an error, never a hang — see `crate::hop`'s
    /// blocking model.
    pub backend_timeout: Duration,
    /// Per-client frame-stats queue depth (drop-oldest beyond it).
    pub frame_stats_capacity: usize,
    /// Simultaneous clients; further connections are closed immediately.
    pub max_clients: usize,
    /// How many backend calls may queue ahead of the one in flight.
    pub backend_queue_depth: usize,
    /// Whether a client must present this process's token at `handshake`
    /// before any other method is dispatched. **Default `true`, and a shell
    /// should leave it that way**: loopback is not a trust boundary on a
    /// device, where any co-resident app can reach the port (see
    /// `crate::token`'s module doc). Switching it off is for an in-process
    /// test or a host with a stronger boundary of its own, never for a shipped
    /// build. With it off the `HotPatch` capability is never offered.
    pub require_token: bool,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            // Comfortably longer than a slow frame, far shorter than a human's
            // patience: a client learns "the app is wedged" in about a second.
            backend_timeout: Duration::from_millis(1_000),
            frame_stats_capacity: frame_stats::DEFAULT_CAPACITY,
            // A devtools client, a CI driver, and room for a stale connection
            // the OS has not reaped yet.
            max_clients: 4,
            backend_queue_depth: 16,
            require_token: true,
        }
    }
}

/// Starts the in-app devtools service. A namespace, not a value — the running
/// service is owned through its [`ServiceHandle`].
pub struct Service;

impl Service {
    /// Binds an ephemeral loopback port and starts serving, with
    /// [`ServiceConfig::default`].
    ///
    /// # Errors
    ///
    /// The `io::Error` from binding `127.0.0.1:0` or from building the
    /// internal runtime. A shell should treat a failure here as "no devtools
    /// this run" and carry on — the service is never load-bearing for the app.
    pub fn start<B: DevtoolsBackend>(backend: B, app: AppInfo) -> io::Result<ServiceHandle> {
        Service::start_with_config(backend, app, ServiceConfig::default())
    }

    /// [`Service::start`] with explicit tuning.
    ///
    /// # Errors
    ///
    /// See [`Service::start`].
    pub fn start_with_config<B: DevtoolsBackend>(
        backend: B,
        app: AppInfo,
        config: ServiceConfig,
    ) -> io::Result<ServiceHandle> {
        // Captured here, on the caller's thread (the shell's, at setup time),
        // so `handshake` is answerable later without touching the backend at
        // all — including while the UI thread is wedged.
        let mut handshake = backend.handshake_info(&app);

        // One current-thread runtime: this service is entirely IO-bound, and a
        // worker pool inside an app process would be a cost the shell never
        // asked for. `enable_io` needs tokio's `net` feature; `enable_time`
        // backs the backend timeout and the shutdown grace window.
        let runtime = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .thread_name("frust-devtools")
            .build()?;

        // Loopback only, ephemeral port. Never `0.0.0.0`: this port answers
        // `input_*` and dumps the widget tree, so exposing it on a network
        // interface would hand any peer on the LAN control of the app (see the
        // crate doc's trust model).
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let listener = runtime.block_on(TcpListener::bind(addr))?;
        let port = listener.local_addr()?.port();

        // One token per process, minted here and never regenerated: a client
        // that read the discovery line once can reconnect for the app's whole
        // lifetime (see `crate::token` for the entropy source and its limits).
        let minted = config.require_token.then(token::generate);
        let token_source = minted.as_ref().map(|t| t.source);
        let token = minted.map(|t| t.value);

        // The one discovery contract, formatted by the protocol crate itself so
        // the formatter and `parse_discovery_line` cannot drift. `log` (not
        // `println!`) because that is the only sink that reaches logcat/oslog on
        // a device, which is where tooling greps for it — and, with auth on, the
        // only place the token appears at all.
        log::info!("{}", format_discovery_line(port, token.as_deref()));

        // `HotPatch` survives in the cached handshake only when the backend
        // offers it AND every code-execution precondition holds; otherwise it
        // is stripped, and its methods answer as on a build without them.
        let offered = handshake.capabilities.contains(&Capability::HotPatch);
        let gate = hot_patch_gate(HotPatchFacts::of_this_build(offered, token_source));
        if let Err(why) = gate {
            handshake
                .capabilities
                .retain(|c| *c != Capability::HotPatch);
            if offered {
                log::warn!("frust-devtools: hot patching unavailable: {why}");
            }
        }

        let bus = Arc::new(FrameStatsBus::new(config.frame_stats_capacity));
        #[cfg(feature = "hotpatch")]
        let (backend, hot_patch) = {
            let shared = SharedBackend::new(backend);
            let hot_patch = match gate {
                Ok(()) => HotPatch::Enabled(crate::dispatch::HotpatchLane::new(
                    Arc::new(shared.clone()),
                    config.backend_timeout,
                )),
                Err(why) => HotPatch::Unavailable(why),
            };
            (shared, hot_patch)
        };
        #[cfg(not(feature = "hotpatch"))]
        let hot_patch =
            HotPatch::Unavailable(gate.err().unwrap_or(HotPatchUnavailable::FeatureOff));
        let backend_client =
            spawn_backend_thread(backend, config.backend_queue_depth, config.backend_timeout);
        let ctx = Arc::new(SessionCtx {
            handshake,
            backend: backend_client,
            bus: Arc::clone(&bus),
            token: token.clone(),
            hot_patch,
        });

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let max_clients = config.max_clients;
        let driver = std::thread::Builder::new()
            .name("frust-devtools".to_string())
            .spawn(move || {
                runtime.block_on(accept_loop(listener, ctx, shutdown_rx, max_clients));
                // Dropping the runtime here (rather than leaking it) drops the
                // last `SessionCtx`, which closes the backend channel and lets
                // the backend thread finish.
            })?;

        Ok(ServiceHandle {
            port,
            token,
            bus,
            shutdown_tx,
            driver: Some(driver),
        })
    }
}

// ---------------------------------------------------------------------
// The hot-patch gate
// ---------------------------------------------------------------------

/// Why this service does not offer `HotPatch`. Each variant names exactly one
/// failed precondition; its `Display` text is what `hotpatch_info` answers
/// (`NOT_SUPPORTED`) and what the service logs, so the CLI/TUI can say which
/// precondition failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HotPatchUnavailable {
    /// `frust-devtools` was built without its `hotpatch` feature.
    FeatureOff,
    /// The backend does not declare [`Capability::HotPatch`].
    NotOffered,
    /// Not a `debug_assertions` build.
    ReleaseBuild,
    /// `ServiceConfig::require_token` is off, so there is no token at all.
    TokenNotRequired,
    /// The token came from the non-CSPRNG fallback: a unix sandbox that
    /// cannot read `/dev/urandom`, or a failed `BCryptGenRandom` on Windows.
    FallbackToken,
}

impl std::fmt::Display for HotPatchUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            HotPatchUnavailable::FeatureOff => "this app was built without the `hotpatch` feature",
            HotPatchUnavailable::NotOffered => "this app's devtools backend offers no hot patching",
            HotPatchUnavailable::ReleaseBuild => {
                "this is not a debug build (hot patching needs debug_assertions)"
            }
            HotPatchUnavailable::TokenNotRequired => {
                "the devtools service runs with require_token off"
            }
            HotPatchUnavailable::FallbackToken => {
                "the devtools token came from the non-CSPRNG fallback, not the OS"
            }
        })
    }
}

/// Everything the hot-patch gate decides on, gathered so the decision itself
/// is a pure function a test can drive through every combination (a test
/// binary is never a release build).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HotPatchFacts {
    /// `frust-devtools`'s `hotpatch` feature is compiled in.
    pub(crate) feature: bool,
    /// The backend declared [`Capability::HotPatch`].
    pub(crate) offered: bool,
    /// `cfg(debug_assertions)`.
    pub(crate) debug_assertions: bool,
    /// The token's source, `None` with `require_token` off.
    pub(crate) token: Option<TokenSource>,
}

impl HotPatchFacts {
    /// The facts of the running build.
    pub(crate) fn of_this_build(offered: bool, token: Option<TokenSource>) -> Self {
        Self {
            feature: cfg!(feature = "hotpatch"),
            offered,
            debug_assertions: cfg!(debug_assertions),
            token,
        }
    }
}

/// `Ok` only when every code-execution precondition holds: the feature, a
/// backend that offers it, `debug_assertions`, and an OS-sourced token
/// (`/dev/urandom` on unix, `BCryptGenRandom` on Windows) with `require_token`
/// on. The host OS is not a precondition: every host answers the same gate.
/// The listener's loopback-only bind is unconditional (see
/// [`Service::start_with_config`]), so it is not a fact here. The first failed
/// precondition, in that order, is the reason reported.
pub(crate) fn hot_patch_gate(facts: HotPatchFacts) -> Result<(), HotPatchUnavailable> {
    if !facts.feature {
        return Err(HotPatchUnavailable::FeatureOff);
    }
    if !facts.offered {
        return Err(HotPatchUnavailable::NotOffered);
    }
    if !facts.debug_assertions {
        return Err(HotPatchUnavailable::ReleaseBuild);
    }
    match facts.token {
        None => Err(HotPatchUnavailable::TokenNotRequired),
        Some(TokenSource::Fallback) => Err(HotPatchUnavailable::FallbackToken),
        Some(TokenSource::Os) => Ok(()),
    }
}

/// The backend shared between the backend thread and the hot-patch worker
/// (`crate::dispatch::HotpatchLane`), behind one mutex: the two never run a
/// backend call at the same time, so the backend still sees one call at a time
/// (its threading contract), and a patch is applied by one call only.
///
/// A panic inside a call poisons the mutex; the next call recovers the guard
/// rather than failing forever, as the backend thread itself would have died
/// with the panic either way.
#[cfg(feature = "hotpatch")]
pub(crate) struct SharedBackend<B>(Arc<std::sync::Mutex<B>>);

#[cfg(feature = "hotpatch")]
impl<B> Clone for SharedBackend<B> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

#[cfg(feature = "hotpatch")]
impl<B: DevtoolsBackend> SharedBackend<B> {
    pub(crate) fn new(backend: B) -> Self {
        Self(Arc::new(std::sync::Mutex::new(backend)))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, B> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(feature = "hotpatch")]
impl<B: DevtoolsBackend> DevtoolsBackend for SharedBackend<B> {
    fn handshake_info(&self, app: &AppInfo) -> frust_devtools_protocol::HandshakeInfo {
        self.lock().handshake_info(app)
    }
    fn widget_tree(&self) -> frust_devtools_protocol::WidgetTreeDump {
        self.lock().widget_tree()
    }
    fn widget_props(&self, id: u64) -> Option<frust_devtools_protocol::WidgetProps> {
        self.lock().widget_props(id)
    }
    fn metrics_snapshot(&self) -> frust_devtools_protocol::MetricsSnapshot {
        self.lock().metrics_snapshot()
    }
    fn inject_tap(
        &self,
        params: frust_devtools_protocol::InputTapParams,
    ) -> Result<(), crate::BackendError> {
        self.lock().inject_tap(params)
    }
    fn inject_scroll(
        &self,
        params: frust_devtools_protocol::InputScrollParams,
    ) -> Result<(), crate::BackendError> {
        self.lock().inject_scroll(params)
    }
    fn inject_text(&self, text: &str) -> Result<(), crate::BackendError> {
        self.lock().inject_text(text)
    }
    fn screenshot(&self) -> Result<frust_devtools_protocol::ScreenshotResult, crate::BackendError> {
        self.lock().screenshot()
    }
    fn hotpatch_info(&self) -> Result<frust_devtools_protocol::HotpatchInfo, crate::BackendError> {
        self.lock().hotpatch_info()
    }
    fn patch_chunk(
        &self,
        chunk: &frust_devtools_protocol::PatchChunkParams,
    ) -> Result<Vec<u8>, crate::BackendError> {
        self.lock().patch_chunk(chunk)
    }
    fn patch_file(
        &self,
        file: &frust_devtools_protocol::PatchFile,
        len: u64,
    ) -> Result<Vec<u8>, crate::BackendError> {
        self.lock().patch_file(file, len)
    }
    fn apply_patch(
        &self,
        bytes: Vec<u8>,
        params: frust_devtools_protocol::ApplyPatchParams,
    ) -> Result<frust_devtools_protocol::PatchOutcome, crate::BackendError> {
        self.lock().apply_patch(bytes, params)
    }
}

/// The running service. Dropping it shuts the service down, so a shell can
/// simply hold it for as long as devtools should be available.
pub struct ServiceHandle {
    port: u16,
    token: Option<String>,
    bus: Arc<FrameStatsBus>,
    shutdown_tx: watch::Sender<bool>,
    driver: Option<std::thread::JoinHandle<()>>,
}

impl ServiceHandle {
    /// The bound loopback port — always ephemeral, so this is the only way to
    /// know it besides the discovery line.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// This process's handshake token, or `None` when the service was started
    /// with [`ServiceConfig::require_token`] off.
    ///
    /// The **in-process** counterpart of reading it off the discovery line, and
    /// no weaker: the caller is the process that owns the secret. Handing it
    /// anywhere else — a response body, a file, another process — defeats the
    /// whole mechanism, which relies on the token reaching only readers of this
    /// app's log stream.
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Publishes one frame's stats to every subscribed client.
    ///
    /// **Safe to call from the frame thread**: it writes one value into a
    /// preallocated bounded ring and returns — it never waits on a client, on
    /// the network, or on a full queue, and it cannot fail. With no
    /// subscribers it is very nearly free. A client that cannot keep up loses
    /// the oldest queued frames (`crate::frame_stats`), which is the whole
    /// reason this cannot stall the producer — `docs/REVIEW_FOCUS.md` rates a
    /// parking call on the UI thread critical.
    pub fn publish_frame_stats(&self, stats: FrameStats) {
        self.bus.publish(stats);
    }

    /// Stops accepting, closes live connections, and joins the service thread.
    ///
    /// The backend thread is deliberately **not** joined: it may be parked
    /// inside a shell call on a frozen UI thread, and shutdown must not be
    /// hostage to that. It exits on its own once the last connection drops the
    /// channel.
    pub fn shutdown(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        // A receiver-less send is not a failure here — it means the accept
        // loop already exited.
        let _ = self.shutdown_tx.send(true);
        if let Some(driver) = self.driver.take()
            && driver.join().is_err()
        {
            log::error!("frust-devtools: the service thread panicked");
        }
    }
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

impl std::fmt::Debug for ServiceHandle {
    /// Hand-written, and deliberately **not** derived: the token must never
    /// reach a log line a `{:?}` produces (it belongs on the discovery line and
    /// nowhere else), and a derive would leak it the moment a field is added.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceHandle")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;

    use frust_devtools_protocol::{
        HandshakeInfo, HandshakeParams, Incoming, InputScrollParams, InputTapParams, Method,
        MetricsSnapshot, PROTOCOL_VERSION, Request, ResponseOutcome, WidgetProps, WidgetTreeDump,
        decode_line, encode_line, serde_json,
    };

    /// Every precondition holding; each test below breaks exactly one.
    fn all_hold() -> HotPatchFacts {
        HotPatchFacts {
            feature: true,
            offered: true,
            debug_assertions: true,
            token: Some(TokenSource::Os),
        }
    }

    #[test]
    fn hot_patch_is_offered_only_when_every_precondition_holds() {
        assert_eq!(hot_patch_gate(all_hold()), Ok(()));
    }

    #[test]
    fn hot_patch_is_absent_without_the_feature() {
        let facts = HotPatchFacts {
            feature: false,
            ..all_hold()
        };
        assert_eq!(hot_patch_gate(facts), Err(HotPatchUnavailable::FeatureOff));
    }

    #[test]
    fn hot_patch_is_absent_in_a_non_debug_build() {
        let facts = HotPatchFacts {
            debug_assertions: false,
            ..all_hold()
        };
        assert_eq!(
            hot_patch_gate(facts),
            Err(HotPatchUnavailable::ReleaseBuild)
        );
    }

    #[test]
    fn hot_patch_is_absent_behind_a_fallback_token() {
        let facts = HotPatchFacts {
            token: Some(TokenSource::Fallback),
            ..all_hold()
        };
        assert_eq!(
            hot_patch_gate(facts),
            Err(HotPatchUnavailable::FallbackToken)
        );
    }

    #[test]
    fn hot_patch_is_absent_with_require_token_off() {
        let facts = HotPatchFacts {
            token: None,
            ..all_hold()
        };
        assert_eq!(
            hot_patch_gate(facts),
            Err(HotPatchUnavailable::TokenNotRequired)
        );
    }

    #[test]
    fn hot_patch_is_absent_when_the_backend_does_not_offer_it() {
        let facts = HotPatchFacts {
            offered: false,
            ..all_hold()
        };
        assert_eq!(hot_patch_gate(facts), Err(HotPatchUnavailable::NotOffered));
    }

    #[test]
    fn this_build_reports_its_own_facts() {
        let facts = HotPatchFacts::of_this_build(true, Some(TokenSource::Os));
        assert_eq!(facts.feature, cfg!(feature = "hotpatch"));
        assert_eq!(facts.debug_assertions, cfg!(debug_assertions));
    }

    #[test]
    fn every_reason_reads_as_a_sentence() {
        for why in [
            HotPatchUnavailable::FeatureOff,
            HotPatchUnavailable::NotOffered,
            HotPatchUnavailable::ReleaseBuild,
            HotPatchUnavailable::TokenNotRequired,
            HotPatchUnavailable::FallbackToken,
        ] {
            assert!(!why.to_string().is_empty());
        }
    }

    /// A backend that offers `HotPatch` in its handshake (and nothing else).
    struct OffersHotPatch;

    impl DevtoolsBackend for OffersHotPatch {
        fn handshake_info(&self, app: &AppInfo) -> HandshakeInfo {
            HandshakeInfo {
                app_name: app.app_name.clone(),
                frust_version: app.frust_version.clone(),
                protocol_version: PROTOCOL_VERSION,
                capabilities: vec![Capability::WidgetTree, Capability::HotPatch],
            }
        }
        fn widget_tree(&self) -> WidgetTreeDump {
            WidgetTreeDump { roots: Vec::new() }
        }
        fn widget_props(&self, _id: u64) -> Option<WidgetProps> {
            None
        }
        fn metrics_snapshot(&self) -> MetricsSnapshot {
            MetricsSnapshot {
                rss_bytes: None,
                uptime_ms: 0,
            }
        }
        fn inject_tap(&self, _p: InputTapParams) -> Result<(), crate::BackendError> {
            Ok(())
        }
        fn inject_scroll(&self, _p: InputScrollParams) -> Result<(), crate::BackendError> {
            Ok(())
        }
        fn inject_text(&self, _t: &str) -> Result<(), crate::BackendError> {
            Ok(())
        }
    }

    /// The capability set a real client reads at handshake from a service over
    /// [`OffersHotPatch`].
    fn handshake_capabilities(config: ServiceConfig) -> Vec<Capability> {
        let handle = Service::start_with_config(OffersHotPatch, AppInfo::new("t", "0"), config)
            .expect("service starts");
        let stream = TcpStream::connect(("127.0.0.1", handle.port())).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut writer = stream.try_clone().expect("clone");
        let params = serde_json::to_value(HandshakeParams {
            token: handle.token().map(str::to_string),
        })
        .expect("params");
        let line = encode_line(&Request::new(1, Method::Handshake.as_str(), params));
        writer
            .write_all(format!("{line}\n").as_bytes())
            .expect("write");
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).expect("read");
        let Ok(Incoming::Response(response)) = decode_line(&reply) else {
            panic!("expected a response, got {reply}");
        };
        let ResponseOutcome::Success { result } = response.outcome else {
            panic!("handshake failed: {reply}");
        };
        serde_json::from_value::<HandshakeInfo>(result)
            .expect("handshake info")
            .capabilities
    }

    #[test]
    fn a_service_strips_an_offered_hot_patch_unless_every_precondition_holds() {
        let caps = handshake_capabilities(ServiceConfig::default());
        // Unix hosts read /dev/urandom, Windows calls BCryptGenRandom; this
        // suite runs in debug.
        let os_token =
            cfg!(windows) || (cfg!(unix) && std::path::Path::new("/dev/urandom").exists());
        let expected = cfg!(all(feature = "hotpatch", debug_assertions)) && os_token;
        assert_eq!(caps.contains(&Capability::HotPatch), expected, "{caps:?}");
        // Everything else the backend declared is untouched.
        assert!(caps.contains(&Capability::WidgetTree));
    }

    /// Windows mints its token from the OS (`BCryptGenRandom`), so it answers
    /// the same gate as every other host: offered with every precondition
    /// held, refused behind the fallback source.
    #[cfg(windows)]
    #[test]
    fn a_windows_build_with_an_os_token_offers_hot_patch() {
        let source = token::generate().source;
        assert_eq!(source, TokenSource::Os);
        let facts = HotPatchFacts {
            feature: true,
            offered: true,
            debug_assertions: true,
            ..HotPatchFacts::of_this_build(true, Some(source))
        };
        assert_eq!(hot_patch_gate(facts), Ok(()));
        let fallback = HotPatchFacts {
            token: Some(TokenSource::Fallback),
            ..facts
        };
        assert_eq!(
            hot_patch_gate(fallback),
            Err(HotPatchUnavailable::FallbackToken)
        );
        assert!(ServiceConfig::default().require_token);
        let caps = handshake_capabilities(ServiceConfig::default());
        // The real service carries the capability whenever this build has the
        // feature (the suite runs in debug).
        assert_eq!(
            caps.contains(&Capability::HotPatch),
            cfg!(all(feature = "hotpatch", debug_assertions)),
            "{caps:?}"
        );
        assert!(caps.contains(&Capability::WidgetTree));
    }

    #[test]
    fn a_service_with_require_token_off_never_offers_hot_patch() {
        let caps = handshake_capabilities(ServiceConfig {
            require_token: false,
            ..ServiceConfig::default()
        });
        assert!(!caps.contains(&Capability::HotPatch), "{caps:?}");
        assert!(caps.contains(&Capability::WidgetTree));
    }
}
