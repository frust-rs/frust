//! The shell side of the in-app devtools service: a [`DevtoolsBackend`]
//! implementation over each shell's own UI thread, plus the process-wide
//! start/pump/publish seam the three shells drive.
//!
//! Compiled **only** under this crate's `devtools` cargo feature. Without it
//! nothing here exists — no listener, no protocol symbols, no discovery-line
//! bytes (see the feature's comment in `Cargo.toml`). The gate is the feature,
//! never a `debug_assertions` runtime check.
//!
//! # Shape
//!
//! ```text
//!  service thread ─► backend thread ─► [request queue] ─► UI thread (pump)
//!  (frust-devtools)   (ShellBackend)    ▲   reply channel   ▲
//!                                       └──── one hop ──────┘
//! ```
//!
//! `frust-devtools` calls [`DevtoolsBackend`]'s plain sync methods on its own
//! backend thread. Everything that needs live app state — the widget tree,
//! one widget's props, an injected event — is answered by *hopping to the UI
//! thread*: the backend pushes a [`UiRequest`] onto a process-wide queue,
//! nudges the shell awake if that shell needs nudging, and blocks on a reply
//! channel. Each shell calls [`pump`] once per frame (desktop: on the woken
//! event-loop turn) with a [`DevtoolsUi`] view of itself, which drains the
//! queue, answers each request, and — for an injection — dispatches the
//! synthetic event through **the same `event` path real input uses**, so
//! nothing bypasses hit-testing, capture, focus routing, or the reactive
//! root-owner wrap.
//!
//! # Timeouts
//!
//! The service already caps every backend call at
//! `ServiceConfig::backend_timeout` (1s) and answers the waiting client with an
//! error when it expires, so a frozen UI thread costs a client one error and
//! nothing more. This module deliberately adds **no competing client-facing
//! timeout**: [`UI_HOP_DEADLINE`] is five times that budget and exists only as
//! a liveness backstop so a permanently dead UI thread cannot wedge the single
//! backend thread for the rest of the process — the client has long since been
//! answered by the service's own timeout in that case.
//!
//! # Metrics
//!
//! [`ShellBackend::metrics_snapshot`] needs no hop: uptime comes from an
//! [`Instant`] captured at [`start`], and RSS is read from `/proc/self/status`
//! (Linux/Android; `None` everywhere else — the protocol models it optional
//! precisely because it is platform-dependent).
//!
//! # Screenshot
//!
//! Not supported in v1 — the trait's default (`NOT_SUPPORTED` on the wire),
//! and the default `handshake_info` capability set that pairs with it, are both
//! left untouched.
//!
//! # Hot patching (`hotpatch` feature)
//!
//! [`ShellBackend`] offers `Capability::HotPatch` and answers its three calls;
//! `frust-devtools` keeps the capability only while every code-execution
//! precondition holds (debug build, OS-CSPRNG token with `require_token` on,
//! not Windows), and owns the per-connection chunk reassembly. Here:
//!
//! - `patch_chunk` decodes one chunk's base64 payload;
//! - `patch_file` (unix; `hotpatch_info` advertises `patch_file_hand_off`)
//!   reads a patch the loopback host already wrote, named on `apply_patch` by
//!   path and SHA-256, and refuses it unless all five checks hold: opened
//!   `O_NOFOLLOW` and a regular file by `fstat`, owned by this process's
//!   effective uid, `mode & 0o077 == 0`, size equal to `len`, matching
//!   SHA-256. The bytes then go through `apply_patch` exactly as reassembled
//!   chunks do; the path is never logged or echoed;
//! - `apply_patch` checks `pid` and `anchor_runtime` against this process (an
//!   unset anchor fails closed), writes the bytes it was handed — never loading
//!   a path from the wire — to `<cache dir>/frust-hotpatch/patch-<pid>-<id>.<ext>`
//!   (directory `0700`, file `0600`, created fresh, never through an existing
//!   entry), applies it through `frust_hotpatch::apply_from_devtools`, removes
//!   the file, and answers **after the following frame**: it parks on the frame
//!   gate, asks the UI thread for a frame ([`DevtoolsUi::request_frame`]), and
//!   the shell's [`frame_submitted`] releases it, so the outcome's seam hits,
//!   missed keys and layout mismatches are those of a frame built after the
//!   apply. No error or outcome names the file;
//! - `hotpatch_info` reports this process's anchor, pid, target triple,
//!   counters, and the layout-mismatch records not yet reported (marking them
//!   reported).
//!
//! The desktop and Android shells call [`frame_submitted`]; elsewhere an apply
//! answers once the frame wait's [`UI_HOP_DEADLINE`] lapses.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use frust_core::InspectNode;
use frust_core::event::{
    InputEvent, Key, KeyEvent, Modifiers, PointerButton, PointerEvent, PointerPhase, ScrollDelta,
};
use frust_devtools::{AppInfo, BackendError, DevtoolsBackend, Service, ServiceHandle};
#[cfg(all(feature = "hotpatch", unix))]
use frust_devtools_protocol::PatchFile;
#[cfg(feature = "hotpatch")]
use frust_devtools_protocol::{
    ApplyPatchParams, Capability, HandshakeInfo, HotpatchInfo, PROTOCOL_VERSION, PatchChunkParams,
    PatchOutcome,
};
use frust_devtools_protocol::{
    FrameStats, InputScrollParams, InputTapParams, MetricsSnapshot, RectPx, WidgetNode,
    WidgetProps, WidgetTreeDump,
};
use kurbo::Point;

use crate::perf::FramePasses;

/// Runtime kill switch: set `FRUST_DEVTOOLS=0` to skip starting the service in
/// a build that compiled it in. Parsed exactly like `FRUST_TRACE`/the
/// `FRUST_NO_*` frame-pipeline switches (see `crate::perf`): only the literal
/// string `"0"` disables — anything else, including unset, leaves devtools on.
pub const DEVTOOLS_VAR: &str = "FRUST_DEVTOOLS";

/// Liveness backstop for one UI-thread hop — **not** a client-facing timeout
/// (see the module docs). Deliberately far longer than the service's own 1s
/// `backend_timeout`, so the client always learns "the UI thread is busy" from
/// the service, and this only ever fires to release a backend thread parked
/// behind a UI thread that is never coming back.
const UI_HOP_DEADLINE: Duration = Duration::from_secs(5);

/// Cap on how many requests may queue for the UI thread. A client that floods
/// the service is already bounded by `ServiceConfig::backend_queue_depth` (one
/// call runs at a time), so this only guards against a UI thread that stopped
/// pumping entirely (backgrounded app) while calls keep timing out and
/// abandoning their queue slots.
const MAX_QUEUED_REQUESTS: usize = 64;

// ---------------------------------------------------------------------
// The UI-thread hop
// ---------------------------------------------------------------------

/// One question the backend thread needs the UI thread to answer. Each variant
/// carries its own reply channel; the sender is dropped without a value if the
/// pump cannot answer, which the waiting side reads as "no answer".
enum UiRequest {
    /// Snapshot the retained tree.
    Tree(std::sync::mpsc::Sender<Vec<InspectNode>>),
    /// Deliver synthetic input events, in order, through the real event path.
    Events(Vec<InputEvent>, std::sync::mpsc::Sender<()>),
    /// Render a frame soon: a hot-patch answer is parked on the frame gate
    /// until the shell's next [`frame_submitted`].
    #[cfg(feature = "hotpatch")]
    RequestFrame,
}

/// The process-wide hop queue plus each shell's optional wake mechanism.
struct Bridge {
    queue: Mutex<VecDeque<UiRequest>>,
    /// How to nudge the UI thread when a request is queued. `Some` for a shell
    /// whose loop idles until something wakes it (desktop's winit
    /// `ControlFlow::Wait`, woken through its `EventLoopProxy`); `None` for the
    /// two mobile shells, whose Choreographer/`CADisplayLink` loop ticks
    /// continuously while resumed and drains the queue at the top of every
    /// frame — a hop there costs at most one frame interval.
    wake: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Bridge {
    /// Queue `request` and wake the UI thread. Returns `false` when the queue
    /// is at [`MAX_QUEUED_REQUESTS`] (the request is dropped, the caller
    /// reports the failure).
    fn submit(&self, request: UiRequest) -> bool {
        {
            let Ok(mut queue) = self.queue.lock() else {
                return false;
            };
            if queue.len() >= MAX_QUEUED_REQUESTS {
                return false;
            }
            queue.push_back(request);
        }
        if let Some(wake) = self.wake.as_ref() {
            wake();
        }
        true
    }
}

/// Installed once by [`start`]; read by both the backend thread ([`Bridge::submit`])
/// and the UI thread ([`pump`]).
static BRIDGE: OnceLock<Arc<Bridge>> = OnceLock::new();

/// The running service. Held for the process lifetime deliberately: devtools
/// should be available for as long as the app is, and there is no shutdown
/// point a shell could hand it (`frust_destroy`/window close are also
/// process-teardown on every shipped shell).
static SERVICE: OnceLock<ServiceHandle> = OnceLock::new();

/// Monotonic frame counter for the published [`FrameStats::n`] — independent of
/// `perf::FrameStats`'s own counter, which only advances when perf tracing is
/// enabled (devtools frame stats must flow regardless of `FRUST_TRACE`).
static FRAME_N: AtomicU64 = AtomicU64::new(0);

/// What a shell exposes to the devtools pump: read the retained tree, and
/// deliver one synthetic event through the shell's ordinary input path.
///
/// Deliberately tiny — everything else a backend answers (metrics, handshake)
/// needs no app state at all. Implemented by each shell over whatever it holds:
/// the desktop `ShellHandler` over its own `RenderRoot`, the mobile handles
/// over their `Box<dyn AppTree>`.
pub trait DevtoolsUi {
    /// A read-only snapshot of the retained widget tree
    /// (`RenderRoot::inspect`).
    fn inspect(&self) -> Vec<InspectNode>;

    /// Deliver one synthetic input event.
    ///
    /// **Contract:** this must route through exactly the same path a real
    /// platform event takes — the shell's own `event`/`dispatch` helper, under
    /// the reactive root owner, setting whatever frame-gate latch real input
    /// sets. An implementation that reaches into widget code directly would
    /// bypass hit-testing, capture and focus routing, which is the whole reason
    /// injection is expressed as an event rather than a call.
    fn dispatch(&mut self, event: InputEvent);

    /// Schedule a frame, for an answer that waits on the next one (a hot-patch
    /// outcome, released by [`frame_submitted`]). The default does nothing,
    /// right for a shell whose loop ticks continuously while resumed; a shell
    /// that idles until woken (desktop) requests a redraw here.
    fn request_frame(&mut self) {}
}

/// Drain every queued devtools request, answering each against `ui`.
///
/// Called by each shell once per frame (or, on desktop, on the event-loop turn
/// the wake produced). A no-op — one uncontended `Mutex` check — when nothing
/// is queued, which is every frame of a session with no devtools client
/// attached.
///
/// **Never blocks or parks the UI thread** (`docs/REVIEW_FOCUS.md` rates that
/// critical): it only ever *sends* on a reply channel, never receives, and the
/// one lock it takes is held for a `pop_front` — released before any app code
/// runs. All the waiting in this design happens on the backend thread, which
/// is exactly what that thread exists for.
pub fn pump(ui: &mut dyn DevtoolsUi) {
    let Some(bridge) = BRIDGE.get() else {
        return;
    };
    loop {
        let request = {
            let Ok(mut queue) = bridge.queue.lock() else {
                return;
            };
            // The lock is released before answering: `dispatch` re-enters app
            // code, which must never run while holding the queue lock.
            match queue.pop_front() {
                Some(request) => request,
                None => return,
            }
        };
        match request {
            UiRequest::Tree(reply) => {
                // A dropped receiver (the client timed out) is expected, not an
                // error — see `frust_devtools::hop`'s blocking model.
                let _ = reply.send(ui.inspect());
            }
            UiRequest::Events(events, reply) => {
                for event in events {
                    ui.dispatch(event);
                }
                let _ = reply.send(());
            }
            #[cfg(feature = "hotpatch")]
            UiRequest::RequestFrame => ui.request_frame(),
        }
    }
}

// ---------------------------------------------------------------------
// The frame gate (hot-patch answers)
// ---------------------------------------------------------------------

/// Answers parked until the shell hands off its next frame.
#[cfg(feature = "hotpatch")]
struct FrameGate {
    /// Fast path for [`frame_submitted`]: anything parked at all.
    waiting: std::sync::atomic::AtomicBool,
    parked: Mutex<Vec<std::sync::mpsc::Sender<()>>>,
}

#[cfg(feature = "hotpatch")]
static FRAME_GATE: FrameGate = FrameGate {
    waiting: std::sync::atomic::AtomicBool::new(false),
    parked: Mutex::new(Vec::new()),
};

/// Park on the frame gate: the receiver gets `()` from the first
/// [`frame_submitted`] that runs after this call.
#[cfg(feature = "hotpatch")]
fn park_until_next_frame() -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut parked = FRAME_GATE
        .parked
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    parked.push(tx);
    FRAME_GATE.waiting.store(true, Ordering::Release);
    rx
}

/// Tell devtools that this shell just handed a finished frame off (`hotpatch`
/// feature): releases every answer parked on the frame gate — a hot-patch
/// outcome waits for the frame after its apply, so its seam hits and layout
/// records are those of a rebuild that ran the patched code.
///
/// Call it once per frame, right after the frame leaves the UI thread. With
/// nothing parked (every frame of a session not applying a patch) it is one
/// atomic load; it never blocks beyond a briefly held lock, and only sends.
#[cfg(feature = "hotpatch")]
pub fn frame_submitted() {
    if !FRAME_GATE.waiting.load(Ordering::Acquire) {
        return;
    }
    let parked = {
        let mut parked = FRAME_GATE
            .parked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        FRAME_GATE.waiting.store(false, Ordering::Release);
        std::mem::take(&mut *parked)
    };
    for answer in parked {
        // A gone receiver (the waiter's deadline lapsed) is expected.
        let _ = answer.send(());
    }
}

// ---------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------

/// Start the devtools service for this process, once.
///
/// `app_name` identifies the app at handshake; `wake` is the shell's
/// UI-thread nudge (see [`Bridge::wake`] — `None` for a continuously ticking
/// mobile loop). Call this **after** the shell's logger is installed: the
/// service logs its discovery line (`frust-devtools listening on <port> token
/// <token>`) through the `log` facade, and tooling recovers both the port and
/// the handshake token by grepping that line out of stderr/logcat. The token
/// is the service's own secret — this module only passes the log line along
/// (and offers [`token`] to the in-process caller that already owns it).
///
/// Never load-bearing: a bind failure, a repeat call, or the `FRUST_DEVTOOLS=0`
/// kill switch all leave the app running exactly as it would without devtools.
pub fn start(app_name: impl Into<String>, wake: Option<Box<dyn Fn() + Send + Sync>>) {
    if kill_switch_engaged(std::env::var(DEVTOOLS_VAR).ok().as_deref()) {
        log::info!("frust-devtools: disabled by {DEVTOOLS_VAR}=0");
        return;
    }
    if SERVICE.get().is_some() {
        return;
    }

    let bridge = Arc::new(Bridge {
        queue: Mutex::new(VecDeque::new()),
        wake,
    });
    // A second `start` in one process (an Android activity recreated against a
    // live process) must not install a second bridge behind the first service.
    if BRIDGE.set(Arc::clone(&bridge)).is_err() {
        return;
    }

    let backend = ShellBackend {
        bridge,
        started: Instant::now(),
        #[cfg(feature = "hotpatch")]
        hot: HotState::default(),
    };
    let info = AppInfo::new(app_name, env!("CARGO_PKG_VERSION"));
    match Service::start(backend, info) {
        Ok(handle) => {
            let _ = SERVICE.set(handle);
        }
        Err(err) => log::warn!(
            "{}",
            frust_devtools_protocol::format_failure_line(&err.to_string())
        ),
    }
}

/// The loopback port the service bound, or `None` when it never started
/// (kill switch, bind failure, or a shell that does not start it).
///
/// The in-process equivalent of grepping the discovery line out of a log
/// stream — what a test, or an app that wants to surface the port in its own
/// UI, reads.
pub fn port() -> Option<u16> {
    SERVICE.get().map(ServiceHandle::port)
}

/// The handshake token this process's service requires, or `None` when no
/// service is running (see [`port`]) or it was started with auth off.
///
/// The token is otherwise **service-internal**: this module neither generates
/// nor checks it, and nothing in the shell needs it — the accessor exists for
/// the in-process caller that is already inside the trust boundary (a test
/// driving a loopback client, or an app choosing to surface it next to the
/// port). Never send it anywhere the discovery line does not already go.
pub fn token() -> Option<String> {
    SERVICE
        .get()
        .and_then(ServiceHandle::token)
        .map(str::to_string)
}

/// The kill-switch decision, pure so it is testable without touching the
/// process environment: only the literal `"0"` disables (mirroring
/// `perf::trace_switch`'s "not the string zero" convention, inverted — this
/// switch is default-ON once compiled in).
fn kill_switch_engaged(value: Option<&str>) -> bool {
    value == Some("0")
}

/// A best-effort app name for the handshake, derived from the running process:
/// `/proc/self/cmdline`'s first entry on Linux/Android (on Android that is the
/// package name), else `argv[0]`, reduced to its file name. Falls back to
/// `"frust-app"` when neither is readable.
///
/// A shell that knows a better name should pass its own — nothing here is
/// authoritative, it is a label a developer reads in a client.
pub fn app_name_from_process() -> String {
    let raw = std::fs::read_to_string("/proc/self/cmdline")
        .ok()
        .and_then(|cmdline| {
            cmdline
                .split('\0')
                .next()
                .filter(|first| !first.is_empty())
                .map(str::to_string)
        })
        .or_else(|| std::env::args().next());

    raw.as_deref()
        .and_then(|path| path.rsplit(['/', '\\']).next())
        .filter(|name| !name.is_empty())
        .unwrap_or("frust-app")
        .to_string()
}

// ---------------------------------------------------------------------
// Frame stats
// ---------------------------------------------------------------------

/// Publish one frame's timings to every subscribed devtools client.
///
/// Called from `perf::FrameStats::record` — the single site every shell's
/// frame pipeline already funnels through (desktop/Android/iOS, inline and
/// render-thread-split alike), so no shell has its own copy to keep in sync.
/// It sits **before** that method's `perf::enabled()` early return on purpose:
/// devtools frame stats must flow whether or not `FRUST_TRACE` is set, since a
/// client subscribing to them is its own opt-in.
///
/// Non-blocking by construction (`ServiceHandle::publish_frame_stats` writes
/// into a bounded drop-oldest ring), and with no service running it is one
/// relaxed atomic load.
pub(crate) fn publish_frame(passes: &FramePasses) {
    let Some(service) = SERVICE.get() else {
        return;
    };
    let n = FRAME_N.fetch_add(1, Ordering::Relaxed) + 1;
    service.publish_frame_stats(FrameStats {
        n,
        total_us: passes.total().as_micros() as u64,
        rebuild_us: passes.rebuild.as_micros() as u64,
        layout_us: passes.layout.as_micros() as u64,
        paint_us: passes.paint.as_micros() as u64,
        encode_us: passes.encode.as_micros() as u64,
        acquire_us: passes.acquire.as_micros() as u64,
        submit_us: passes.submit.as_micros() as u64,
        skipped: passes.skipped,
    });
}

// ---------------------------------------------------------------------
// The backend
// ---------------------------------------------------------------------

/// The [`DevtoolsBackend`] every shell shares: a hop to the UI thread for
/// anything that touches app state, answered locally for anything that does
/// not.
struct ShellBackend {
    bridge: Arc<Bridge>,
    /// The epoch `metrics_snapshot`'s uptime is measured from — process start
    /// as far as devtools is concerned (the shell starts the service during
    /// its own init).
    started: Instant,
    /// Hot-patch counters and the patch directory override.
    #[cfg(feature = "hotpatch")]
    hot: HotState,
}

impl ShellBackend {
    /// Run one UI-thread hop: queue `make_request`'s value, block on the reply
    /// channel up to [`UI_HOP_DEADLINE`], and report what came back.
    fn hop<T>(
        &self,
        make_request: impl FnOnce(std::sync::mpsc::Sender<T>) -> UiRequest,
    ) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        if !self.bridge.submit(make_request(tx)) {
            log::warn!("frust-devtools: the UI-thread request queue is full");
            return None;
        }
        rx.recv_timeout(UI_HOP_DEADLINE).ok()
    }
}

impl DevtoolsBackend for ShellBackend {
    /// The default capability set plus [`Capability::HotPatch`]; the service
    /// strips it again unless every precondition holds.
    #[cfg(feature = "hotpatch")]
    fn handshake_info(&self, app: &AppInfo) -> HandshakeInfo {
        HandshakeInfo {
            app_name: app.app_name.clone(),
            frust_version: app.frust_version.clone(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec![
                Capability::WidgetTree,
                Capability::FrameStats,
                Capability::Input,
                Capability::Metrics,
                Capability::HotPatch,
            ],
        }
    }

    #[cfg(feature = "hotpatch")]
    fn hotpatch_info(&self) -> Result<HotpatchInfo, BackendError> {
        let pending = frust_hotpatch::pending_layout_mismatches();
        // Carried by this answer, so reported (only these: one recorded after
        // the read stays pending).
        frust_hotpatch::mark_layout_mismatches_reported(&pending);
        let counters = self.hot.lock();
        Ok(HotpatchInfo {
            anchor_runtime: frust_hotpatch::aslr_reference() as u64,
            pid: std::process::id(),
            triple: target_triple(),
            patches_applied: counters.patches_applied,
            patch_bytes_loaded: counters.patch_bytes_loaded,
            pending_layout_mismatches: pending.iter().map(ToString::to_string).collect(),
            patch_file_hand_off: PATCH_FILE_HAND_OFF,
        })
    }

    #[cfg(feature = "hotpatch")]
    fn patch_chunk(&self, chunk: &PatchChunkParams) -> Result<Vec<u8>, BackendError> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(&chunk.data_base64)
            .map_err(|e| BackendError::invalid_request(format!("patch chunk is not base64: {e}")))
    }

    /// The loopback hand-off: see the module doc's *Hot patching*.
    #[cfg(all(feature = "hotpatch", unix))]
    fn patch_file(&self, file: &PatchFile, len: u64) -> Result<Vec<u8>, BackendError> {
        let dir = self.hot.patch_dir().ok_or_else(|| {
            BackendError::unavailable("no cache directory to check the patch file's owner against")
        })?;
        let own_uid = effective_uid(&dir).map_err(|e| {
            BackendError::internal(format!("could not determine this process's user: {e}"))
        })?;
        let bytes = read_handed_off_patch(file, len, own_uid)?;
        log::debug!("frust-devtools: patch file handed off ({len} bytes, checks passed)");
        Ok(bytes)
    }

    /// See the module doc's *Hot patching*. Every answer — applied, refused,
    /// failed — is sent after the frame that follows the attempt, so a reply
    /// always means the app has rendered since.
    #[cfg(feature = "hotpatch")]
    fn apply_patch(
        &self,
        bytes: Vec<u8>,
        params: ApplyPatchParams,
    ) -> Result<PatchOutcome, BackendError> {
        // Held across the attempt and the frame wait: one patch at a time.
        let mut counters = self.hot.lock();
        let attempt = self.hot.attempt(&mut counters, bytes, params);

        let frame = park_until_next_frame();
        if !self.bridge.submit(UiRequest::RequestFrame) {
            log::warn!("frust-devtools: could not ask the UI thread for a frame");
        }
        if frame.recv_timeout(UI_HOP_DEADLINE).is_err() {
            log::warn!(
                "frust-devtools: no frame followed the patch within {}s; answering anyway",
                UI_HOP_DEADLINE.as_secs()
            );
        }

        let Attempt {
            applied,
            records: mut refusal,
        } = attempt?;
        // Records the attempt refused over, plus any the following frame met.
        for record in frust_hotpatch::pending_layout_mismatches() {
            if !refusal.contains(&record) {
                refusal.push(record);
            }
        }
        frust_hotpatch::mark_layout_mismatches_reported(&refusal);
        let (seam_hits, seam_fall_throughs) = if applied {
            let missed = frust_hotpatch::missed_keys()
                .into_iter()
                .map(|key| frust_devtools_protocol::MissedKey {
                    image: key.image,
                    link_address: key.link_address,
                })
                .collect();
            (frust_hotpatch::seam_hits(), missed)
        } else {
            // The counters still describe the previous patch: not this answer's.
            (0, Vec::new())
        };
        Ok(PatchOutcome {
            applied,
            seam_hits,
            seam_fall_throughs,
            layout_mismatches: refusal.iter().map(ToString::to_string).collect(),
            patches_applied: counters.patches_applied,
            patch_bytes_loaded: counters.patch_bytes_loaded,
        })
    }

    fn widget_tree(&self) -> WidgetTreeDump {
        // Infallible by contract: an unanswered hop reports an empty tree, the
        // same thing a not-yet-built app reports.
        match self.hop(UiRequest::Tree) {
            Some(nodes) => tree_dump(&nodes),
            None => WidgetTreeDump { roots: Vec::new() },
        }
    }

    fn widget_props(&self, id: u64) -> Option<WidgetProps> {
        let nodes = self.hop(UiRequest::Tree)?;
        props_for(&nodes, id)
    }

    fn metrics_snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            rss_bytes: rss_bytes(),
            uptime_ms: self.started.elapsed().as_millis() as u64,
        }
    }

    fn inject_tap(&self, params: InputTapParams) -> Result<(), BackendError> {
        let position = logical_point(params.x, params.y)?;
        // A tap is a full gesture, not a lone `Down`: every interactive widget
        // in `frust-widgets` fires on **up-inside** (docs/CODE_STANDARDS.md's
        // Interaction Semantics), so a `Down` alone would only paint a pressed
        // state and never activate anything.
        self.inject(vec![
            InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position,
                button: PointerButton::Primary,
            }),
            InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Up,
                position,
                button: PointerButton::Primary,
            }),
        ])
    }

    fn inject_scroll(&self, params: InputScrollParams) -> Result<(), BackendError> {
        let position = logical_point(params.x, params.y)?;
        if !params.dx.is_finite() || !params.dy.is_finite() {
            return Err(BackendError::invalid_request(
                "scroll delta must be finite logical px",
            ));
        }
        // Pixels, not Lines: the wire delta is documented as logical px, which
        // is what a trackpad/touch scroll already reports.
        self.inject(vec![InputEvent::Scroll {
            position,
            delta: ScrollDelta::Pixels(params.dx, params.dy),
        }])
    }

    fn inject_text(&self, text: &str) -> Result<(), BackendError> {
        if text.is_empty() {
            return Err(BackendError::invalid_request("text must not be empty"));
        }
        // `Key::Character` carries the already-resolved text a key produced —
        // the same shape the desktop shell builds from winit's `KeyEvent.text`
        // — and is focus-routed, so an app with nothing focused simply ignores
        // it (not an error the client has to distinguish).
        self.inject(vec![InputEvent::Key(KeyEvent {
            key: Key::Character(text.to_string()),
            modifiers: Modifiers::default(),
            repeat: false,
        })])
    }
}

impl ShellBackend {
    /// Hop a batch of synthetic events onto the UI thread and wait for them to
    /// have been dispatched, so the ack a client receives means "delivered",
    /// not "queued".
    fn inject(&self, events: Vec<InputEvent>) -> Result<(), BackendError> {
        match self.hop(|reply| UiRequest::Events(events, reply)) {
            Some(()) => Ok(()),
            None => Err(BackendError::unavailable(
                "the app's UI thread did not process the injected event",
            )),
        }
    }
}

// ---------------------------------------------------------------------
// Hot-patch apply
// ---------------------------------------------------------------------

/// The patch library's file extension on this target.
#[cfg(feature = "hotpatch")]
const PATCH_EXT: &str = if cfg!(any(target_os = "macos", target_os = "ios")) {
    "dylib"
} else if cfg!(windows) {
    "dll"
} else {
    "so"
};

/// Whether `hotpatch_info` advertises the loopback patch-file hand-off: exactly
/// when `ShellBackend::patch_file`'s checked reader is compiled in (unix) and
/// the host can share a filesystem with the app. Never on Android: host and
/// device are different machines, so a host path means nothing there.
#[cfg(feature = "hotpatch")]
const PATCH_FILE_HAND_OFF: bool = cfg!(all(unix, not(target_os = "android")));

/// What [`ShellBackend`] tracks across patches.
#[cfg(feature = "hotpatch")]
#[derive(Default)]
struct HotState {
    /// Replaces `frust_paths::cache_dir()` as the patch file's base (tests).
    cache_root: Option<std::path::PathBuf>,
    counters: Mutex<HotCounters>,
}

#[cfg(feature = "hotpatch")]
#[derive(Default)]
struct HotCounters {
    patches_applied: u32,
    patch_bytes_loaded: u64,
    /// Every `patch_id` that reached the file stage. A loader keys loaded
    /// images by path, so a reused id would get the earlier library back.
    used_ids: HashSet<u64>,
}

/// One attempt's result short of the frame wait: applied, or refused over
/// these unreported layout-mismatch records.
#[cfg(feature = "hotpatch")]
struct Attempt {
    applied: bool,
    records: Vec<frust_hotpatch::LayoutMismatch>,
}

#[cfg(feature = "hotpatch")]
impl HotState {
    fn lock(&self) -> std::sync::MutexGuard<'_, HotCounters> {
        self.counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `<cache dir>/frust-hotpatch`, or `None` when the platform has no cache
    /// dir (yet — Android installs it from its first surface).
    fn patch_dir(&self) -> Option<std::path::PathBuf> {
        self.cache_root
            .clone()
            .or_else(frust_paths::cache_dir)
            .map(|root| root.join("frust-hotpatch"))
    }

    /// Check, write, apply, remove: everything `apply_patch` does before its
    /// frame wait. Refusals load nothing.
    fn attempt(
        &self,
        counters: &mut HotCounters,
        bytes: Vec<u8>,
        params: ApplyPatchParams,
    ) -> Result<Attempt, BackendError> {
        check_target(
            (params.pid, params.anchor_runtime),
            (std::process::id(), frust_hotpatch::aslr_reference() as u64),
        )?;
        let len = bytes.len() as u64;
        if len != params.len {
            return Err(BackendError::invalid_request(format!(
                "patch is {len} bytes, apply_patch says {}",
                params.len
            )));
        }
        if counters.used_ids.contains(&params.patch_id) {
            return Err(BackendError::invalid_request(format!(
                "patch_id {} was already used in this process; send a fresh id",
                params.patch_id
            )));
        }
        let dir = self.patch_dir().ok_or_else(|| {
            BackendError::unavailable("no cache directory to write the patch into")
        })?;
        counters.used_ids.insert(params.patch_id);
        let name = format!(
            "patch-{}-{}.{PATCH_EXT}",
            std::process::id(),
            params.patch_id
        );
        let path = write_patch_file(&dir, &name, &bytes).map_err(|e| {
            BackendError::internal(redact(
                &format!("could not write the patch: {e}"),
                &dir,
                &name,
            ))
        })?;
        log::debug!(
            "frust-devtools: patch {} written to {}",
            params.patch_id,
            path.display()
        );

        let table = frust_hotpatch::JumpTable {
            lib: path.clone(),
            map: params.table.map.into_iter().collect(),
            aslr_reference: params.table.aslr_reference,
            new_base_address: params.table.new_base_address,
            ifunc_count: params.table.ifunc_count,
        };
        log::debug!(
            "frust-devtools: applying patch {} ({len} bytes, {} expected seams)",
            params.patch_id,
            params.expected_seams
        );
        let report = frust_hotpatch::apply_from_devtools(&path, table);
        // A loaded library stays mapped; the file has done its job either way.
        if let Err(e) = std::fs::remove_file(&path) {
            log::debug!("frust-devtools: could not remove {}: {e}", path.display());
        }

        if let Some(error) = report.error {
            return Err(patch_error(&error, &dir, &name));
        }
        if report.applied {
            counters.patches_applied = counters.patches_applied.saturating_add(1);
            counters.patch_bytes_loaded = counters.patch_bytes_loaded.saturating_add(len);
        }
        Ok(Attempt {
            applied: report.applied,
            records: report.layout_mismatches,
        })
    }
}

/// `apply_patch`'s `(pid, anchor_runtime)` against this process's. An unset
/// anchor (0) fails closed: nothing could rebase the patch.
#[cfg(feature = "hotpatch")]
fn check_target(requested: (u32, u64), own: (u32, u64)) -> Result<(), BackendError> {
    let ((pid, anchor), (own_pid, own_anchor)) = (requested, own);
    if pid != own_pid {
        return Err(BackendError::invalid_request(format!(
            "patch targets pid {pid}, this process is {own_pid}"
        )));
    }
    if own_anchor == 0 {
        return Err(BackendError::unavailable(
            "no hot-patch anchor is set in this process; a patch cannot be rebased",
        ));
    }
    if anchor != own_anchor {
        return Err(BackendError::invalid_request(format!(
            "patch targets anchor {anchor:#x}, this process's is {own_anchor:#x}"
        )));
    }
    Ok(())
}

/// Writes `bytes` to `dir/name`: `dir` created `0700`, the file created fresh
/// with mode `0600`. An existing entry (a stale file, a planted symlink) is
/// removed first and the file opened `create_new`, so nothing is ever written
/// through a pre-existing path.
#[cfg(feature = "hotpatch")]
fn write_patch_file(
    dir: &std::path::Path,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write as _;
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = dir.join(name);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    // Exact, whatever the umask took away.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    Ok(path)
}

/// This process's effective uid, without `unsafe` (this crate's charter): the
/// owner of a file a process creates is its effective uid (POSIX `open`), so
/// a fresh probe file is created in `dir` (the app's own `0700` patch
/// directory, through [`write_patch_file`]), read back and removed.
#[cfg(all(feature = "hotpatch", unix))]
fn effective_uid(dir: &std::path::Path) -> std::io::Result<u32> {
    use std::os::unix::fs::MetadataExt as _;
    let name = format!(".uid-probe-{}", std::process::id());
    let path = write_patch_file(dir, &name, &[])?;
    let uid = std::fs::symlink_metadata(&path).map(|meta| meta.uid());
    let _ = std::fs::remove_file(&path);
    uid
}

/// Reads a patch handed off by file, refusing it unless all five checks hold:
/// opened with `O_NOFOLLOW` (a symlink fails the open) and `fstat` says a
/// regular file; owned by `own_uid`; `mode & 0o077 == 0`; exactly `len`
/// bytes; SHA-256 equal to `file.sha256`. `O_NONBLOCK` keeps a FIFO from
/// parking the open (`fstat` then refuses it). No error names the path.
#[cfg(all(feature = "hotpatch", unix))]
fn read_handed_off_patch(
    file: &PatchFile,
    len: u64,
    own_uid: u32,
) -> Result<Vec<u8>, BackendError> {
    use sha2::Digest as _;
    use std::io::Read as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};

    let refuse = |why: String| BackendError::invalid_request(format!("patch file refused: {why}"));
    let well_formed = file.sha256.len() == 64
        && file
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !well_formed {
        return Err(refuse(
            "sha256 must be 64 lowercase hex characters".to_string(),
        ));
    }
    let path = std::path::Path::new(&file.path);
    if !path.is_absolute() {
        return Err(refuse("the path is not absolute".to_string()));
    }
    let mut opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| refuse(format!("it cannot be opened without following a link: {e}")))?;
    let meta = opened
        .metadata()
        .map_err(|e| refuse(format!("it cannot be inspected: {e}")))?;
    if !meta.file_type().is_file() {
        return Err(refuse("it is not a regular file".to_string()));
    }
    if meta.uid() != own_uid {
        return Err(refuse(format!(
            "it is owned by uid {}, this process runs as uid {own_uid}",
            meta.uid()
        )));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(refuse(format!(
            "its mode {:o} grants group or other access",
            meta.mode() & 0o777
        )));
    }
    if meta.len() != len {
        return Err(refuse(format!(
            "it is {} bytes, apply_patch says {len}",
            meta.len()
        )));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    (&mut opened)
        .take(len.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| refuse(format!("it cannot be read: {e}")))?;
    if bytes.len() as u64 != len {
        return Err(refuse("its size changed while it was read".to_string()));
    }
    let digest = format!("{:x}", sha2::Sha256::digest(&bytes));
    if digest != file.sha256 {
        return Err(refuse(
            "its SHA-256 does not match apply_patch's".to_string(),
        ));
    }
    Ok(bytes)
}

/// `message` with every spelling of the patch file's location masked: its
/// directory (as given and canonicalised — a loader may report either) and its
/// file name. The path never reaches the wire.
#[cfg(feature = "hotpatch")]
fn redact(message: &str, dir: &std::path::Path, name: &str) -> String {
    let mut dirs = vec![dir.to_path_buf()];
    if let Ok(canonical) = std::fs::canonicalize(dir) {
        dirs.push(canonical);
    }
    let mut out = message.to_string();
    for dir in &dirs {
        let dir = dir.to_string_lossy();
        if !dir.is_empty() {
            out = out.replace(dir.as_ref(), "<patch dir>");
        }
    }
    out.replace(name, "<patch file>")
}

/// A `frust-hotpatch` refusal as the backend error the client receives, the
/// patch file's location redacted.
#[cfg(feature = "hotpatch")]
fn patch_error(
    error: &frust_hotpatch::PatchError,
    dir: &std::path::Path,
    name: &str,
) -> BackendError {
    use frust_hotpatch::PatchError;
    let message = redact(&format!("patch not applied: {error}"), dir, name);
    match error {
        PatchError::AnchorMismatch { .. } => BackendError::invalid_request(message),
        PatchError::AnchorUnresolved => BackendError::unavailable(message),
        PatchError::ReleaseBuild => BackendError::not_supported(message),
        _ => BackendError::internal(message),
    }
}

/// This build's target triple, best effort from `cfg` (no build script): arch
/// plus the vendor/OS/env spelling rustc uses for the shipped targets.
#[cfg(feature = "hotpatch")]
fn target_triple() -> String {
    let arch = std::env::consts::ARCH;
    let rest = if cfg!(target_os = "macos") {
        "apple-darwin"
    } else if cfg!(target_os = "ios") {
        ios_suffix(cfg!(target_abi = "sim"), cfg!(target_arch = "aarch64"))
    } else if cfg!(target_os = "android") {
        if arch == "arm" {
            "linux-androideabi"
        } else {
            "linux-android"
        }
    } else if cfg!(target_os = "linux") {
        if cfg!(target_env = "musl") {
            "unknown-linux-musl"
        } else {
            "unknown-linux-gnu"
        }
    } else if cfg!(windows) {
        if cfg!(target_env = "gnu") {
            "pc-windows-gnu"
        } else {
            "pc-windows-msvc"
        }
    } else {
        std::env::consts::OS
    };
    let arch = if arch == "arm" && cfg!(target_os = "android") {
        "armv7"
    } else {
        arch
    };
    format!("{arch}-{rest}")
}

/// The iOS vendor/OS spelling: only the arm64 simulator carries `-sim`
/// (`aarch64-apple-ios-sim`); the Intel simulator's triple is
/// `x86_64-apple-ios`, the same spelling as a device.
#[cfg(feature = "hotpatch")]
fn ios_suffix(simulator: bool, aarch64: bool) -> &'static str {
    if simulator && aarch64 {
        "apple-ios-sim"
    } else {
        "apple-ios"
    }
}

/// Validate an injected logical-px coordinate. A non-finite coordinate would
/// poison hit-testing rather than miss, so it is rejected as a bad request
/// instead of being clamped.
fn logical_point(x: f64, y: f64) -> Result<Point, BackendError> {
    if !x.is_finite() || !y.is_finite() {
        return Err(BackendError::invalid_request(
            "tap/scroll position must be finite logical px",
        ));
    }
    Ok(Point::new(x, y))
}

/// Best-effort resident-set size in bytes, read from `/proc/self/status`'s
/// `VmRSS` (reported in kB). `None` on any platform without procfs, and on any
/// read/parse failure — the protocol models this optional for exactly that
/// reason.
fn rss_bytes() -> Option<u64> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        parse_vm_rss_kb(&status).map(|kb| kb * 1024)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        None
    }
}

/// Pull `VmRSS`'s kB value out of a `/proc/self/status` body. Separated from
/// the read so the parse is unit-testable against a fixture on any host.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "android", test)),
    allow(dead_code)
)]
fn parse_vm_rss_kb(status: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse::<u64>().ok())
}

// ---------------------------------------------------------------------
// InspectNode -> protocol mapping
// ---------------------------------------------------------------------

/// Fold `frust-core`'s flat, pre-order [`InspectNode`] snapshot into the
/// protocol's nested [`WidgetTreeDump`].
///
/// The two shapes differ on purpose: core reports a flat list with parent/child
/// **ids** (nothing serialization-shaped in `frust-core`), while the wire type
/// nests so a client renders a tree with no reconstruction pass. Roots are the
/// nodes core reported with no parent, in snapshot order.
///
/// A `visited` set guards the descent: the snapshot is a tree by construction,
/// but this walk is driven by ids and must terminate even against a malformed
/// one rather than recursing until the stack dies.
fn tree_dump(nodes: &[InspectNode]) -> WidgetTreeDump {
    let index: HashMap<u64, usize> = nodes
        .iter()
        .enumerate()
        .map(|(slot, node)| (node.id.0, slot))
        .collect();
    let mut visited = HashSet::new();
    let roots = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.parent.is_none())
        .filter_map(|(slot, _)| widget_node(nodes, &index, slot, &mut visited))
        .collect();
    WidgetTreeDump { roots }
}

fn widget_node(
    nodes: &[InspectNode],
    index: &HashMap<u64, usize>,
    slot: usize,
    visited: &mut HashSet<u64>,
) -> Option<WidgetNode> {
    let node = nodes.get(slot)?;
    if !visited.insert(node.id.0) {
        return None;
    }
    let children = node
        .children
        .iter()
        .filter_map(|child| index.get(&child.0).copied())
        .filter_map(|child_slot| widget_node(nodes, index, child_slot, visited))
        .collect();
    Some(WidgetNode {
        id: node.id.0,
        type_name: node.type_name.to_string(),
        debug_label: node.debug_label.as_ref().map(|label| label.to_string()),
        // Always reported, never `None`: core's bounds are `Rect::ZERO`-sized
        // until a layout pass has run, and "zero-sized because nothing is laid
        // out yet" is more useful to a client than an absent field.
        bounds: Some(RectPx {
            x: node.bounds.x0,
            y: node.bounds.y0,
            width: node.bounds.width(),
            height: node.bounds.height(),
        }),
        children,
    })
}

/// One node's inspectable properties, or `None` when `id` names no node in this
/// snapshot (a stale id from a tree the app has since rebuilt — the service
/// turns that into `INVALID_PARAMS`).
///
/// The entry set is deliberately flat `(name, rendered value)` pairs: the
/// protocol carries strings, and a widget's real props are behind a `dyn Widget`
/// with no introspection seam, so what is reportable today is exactly what the
/// tree snapshot itself carries.
fn props_for(nodes: &[InspectNode], id: u64) -> Option<WidgetProps> {
    let node = nodes.iter().find(|node| node.id.0 == id)?;
    let mut entries = vec![("type".to_string(), node.type_name.to_string())];
    if let Some(label) = node.debug_label.as_ref() {
        entries.push(("debug_label".to_string(), label.to_string()));
    }
    entries.push(("x".to_string(), format!("{:.1}", node.bounds.x0)));
    entries.push(("y".to_string(), format!("{:.1}", node.bounds.y0)));
    entries.push(("width".to_string(), format!("{:.1}", node.bounds.width())));
    entries.push(("height".to_string(), format!("{:.1}", node.bounds.height())));
    entries.push(("depth".to_string(), node.depth.to_string()));
    entries.push((
        "parent".to_string(),
        node.parent
            .map(|parent| parent.0.to_string())
            .unwrap_or_else(|| "none".to_string()),
    ));
    entries.push(("children".to_string(), node.children.len().to_string()));
    Some(WidgetProps { id, entries })
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::view::WidgetId;
    use kurbo::Rect;

    fn node(id: u64, parent: Option<u64>, children: &[u64], depth: usize) -> InspectNode {
        InspectNode {
            id: WidgetId(id),
            parent: parent.map(WidgetId),
            type_name: "StubWidget",
            debug_label: None,
            bounds: Rect::new(0.0, 0.0, 10.0, 20.0),
            children: children.iter().copied().map(WidgetId).collect(),
            depth,
        }
    }

    /// The fixture shape a real `RenderRoot::inspect()` produces: pre-order,
    /// one root, a child reached through `visit_children`, and a grandchild
    /// below it.
    fn fixture() -> Vec<InspectNode> {
        vec![
            InspectNode {
                bounds: Rect::new(0.0, 0.0, 100.0, 200.0),
                debug_label: Some("root".into()),
                ..node(1, None, &[2], 0)
            },
            node(2, Some(1), &[3], 1),
            node(3, Some(2), &[], 2),
        ]
    }

    #[test]
    fn tree_dump_nests_the_flat_pre_order_snapshot() {
        let dump = tree_dump(&fixture());
        assert_eq!(dump.roots.len(), 1);
        let root = &dump.roots[0];
        assert_eq!(root.id, 1);
        assert_eq!(root.type_name, "StubWidget");
        assert_eq!(root.debug_label.as_deref(), Some("root"));
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].id, 2);
        assert_eq!(root.children[0].children[0].id, 3);
        assert!(root.children[0].children[0].children.is_empty());
    }

    #[test]
    fn tree_dump_carries_absolute_bounds_in_logical_px() {
        let dump = tree_dump(&fixture());
        let bounds = dump.roots[0].bounds.expect("bounds always reported");
        assert_eq!(bounds.x, 0.0);
        assert_eq!(bounds.y, 0.0);
        assert_eq!(bounds.width, 100.0);
        assert_eq!(bounds.height, 200.0);
    }

    #[test]
    fn tree_dump_of_an_empty_snapshot_is_an_empty_dump() {
        assert!(tree_dump(&[]).roots.is_empty());
    }

    #[test]
    fn tree_dump_reports_every_root_of_a_multi_root_snapshot() {
        let nodes = vec![node(1, None, &[], 0), node(2, None, &[], 0)];
        let dump = tree_dump(&nodes);
        assert_eq!(
            dump.roots.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn tree_dump_terminates_on_a_malformed_cyclic_snapshot() {
        // Not producible by `RenderRoot::inspect`, but the walk is id-driven:
        // a cycle must end the descent, not the process.
        let nodes = vec![node(1, None, &[2], 0), node(2, Some(1), &[1], 1)];
        let dump = tree_dump(&nodes);
        assert_eq!(dump.roots.len(), 1);
        assert!(dump.roots[0].children[0].children.is_empty());
    }

    #[test]
    fn tree_dump_skips_a_child_id_with_no_node() {
        let nodes = vec![node(1, None, &[2, 9], 0), node(2, Some(1), &[], 1)];
        let dump = tree_dump(&nodes);
        assert_eq!(dump.roots[0].children.len(), 1);
    }

    #[test]
    fn props_for_reports_geometry_and_lineage() {
        let props = props_for(&fixture(), 2).expect("node 2 is in the snapshot");
        assert_eq!(props.id, 2);
        let entries: HashMap<&str, &str> = props
            .entries
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(entries["type"], "StubWidget");
        assert_eq!(entries["width"], "10.0");
        assert_eq!(entries["height"], "20.0");
        assert_eq!(entries["depth"], "1");
        assert_eq!(entries["parent"], "1");
        assert_eq!(entries["children"], "1");
        assert!(!entries.contains_key("debug_label"));
    }

    #[test]
    fn props_for_reports_a_root_with_no_parent() {
        let props = props_for(&fixture(), 1).expect("node 1 is in the snapshot");
        let parent = props
            .entries
            .iter()
            .find(|(k, _)| k == "parent")
            .map(|(_, v)| v.as_str());
        assert_eq!(parent, Some("none"));
    }

    #[test]
    fn props_for_unknown_id_is_none() {
        assert!(props_for(&fixture(), 999).is_none());
    }

    #[test]
    fn a_non_finite_injection_coordinate_is_rejected() {
        // Never clamped: a NaN/infinite coordinate poisons every hit-test
        // comparison it reaches, so it must fail as a bad request instead.
        assert!(logical_point(f64::NAN, 0.0).is_err());
        assert!(logical_point(0.0, f64::INFINITY).is_err());
        assert_eq!(logical_point(3.0, 4.0).map(|p| (p.x, p.y)), Ok((3.0, 4.0)));
    }

    #[test]
    fn kill_switch_engages_only_on_literal_zero() {
        assert!(kill_switch_engaged(Some("0")));
        assert!(!kill_switch_engaged(None));
        assert!(!kill_switch_engaged(Some("1")));
        assert!(!kill_switch_engaged(Some("")));
    }

    #[test]
    fn vm_rss_is_parsed_in_kilobytes() {
        let status = "Name:\tapp\nVmPeak:\t  100 kB\nVmRSS:\t   4096 kB\nThreads:\t3\n";
        assert_eq!(parse_vm_rss_kb(status), Some(4096));
    }

    #[test]
    fn vm_rss_absent_is_none() {
        assert_eq!(parse_vm_rss_kb("Name:\tapp\nThreads:\t3\n"), None);
    }

    /// A `pump` with no service started must be a no-op rather than a panic —
    /// the state every host unit test and every non-devtools frame is in.
    #[test]
    fn pump_without_a_started_service_is_inert() {
        struct NoUi;
        impl DevtoolsUi for NoUi {
            fn inspect(&self) -> Vec<InspectNode> {
                panic!("pump must not touch the UI with nothing queued");
            }
            fn dispatch(&mut self, _event: InputEvent) {
                panic!("pump must not touch the UI with nothing queued");
            }
        }
        pump(&mut NoUi);
    }

    #[cfg(feature = "hotpatch")]
    mod hot {
        use super::*;
        use frust_devtools_protocol::JumpTableWire;
        use std::path::{Path, PathBuf};

        /// The anchor, layout-mismatch records and frame gate are
        /// process-global: every test touching them holds this.
        static SERIAL: Mutex<()> = Mutex::new(());

        fn serial() -> std::sync::MutexGuard<'static, ()> {
            SERIAL
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        extern "C" fn test_anchor() {}

        fn anchor() -> u64 {
            test_anchor as extern "C" fn() as usize as u64
        }

        /// A fresh, empty scratch directory for one test.
        fn scratch(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "frust-shell-common-hot-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            dir
        }

        /// A backend whose patch files go under `root`, with a bridge no UI
        /// thread drains (the tests tick the frame gate themselves).
        fn backend(root: &Path) -> ShellBackend {
            ShellBackend {
                bridge: Arc::new(Bridge {
                    queue: Mutex::new(VecDeque::new()),
                    wake: None,
                }),
                started: Instant::now(),
                hot: HotState {
                    cache_root: Some(root.to_path_buf()),
                    counters: Mutex::default(),
                },
            }
        }

        fn params(patch_id: u64, len: u64, pid: u32, anchor_runtime: u64) -> ApplyPatchParams {
            ApplyPatchParams {
                patch_id,
                len,
                pid,
                anchor_runtime,
                table: JumpTableWire {
                    map: std::collections::HashMap::new(),
                    // Not this image's anchor address: a base AnchorMismatch.
                    aslr_reference: 0,
                    new_base_address: 0,
                    ifunc_count: 0,
                },
                expected_seams: 1,
                file: None,
            }
        }

        /// Runs `apply_patch` on a worker while this thread plays the shell:
        /// reports whether an answer existed before the fake frame tick, then
        /// ticks and returns the answer.
        fn apply_with_frame_tick(
            backend: ShellBackend,
            bytes: Vec<u8>,
            params: ApplyPatchParams,
        ) -> (bool, Result<PatchOutcome, BackendError>) {
            let (tx, rx) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                let _ = tx.send(backend.apply_patch(bytes, params));
            });
            let early = rx.recv_timeout(Duration::from_millis(300)).is_ok();
            frame_submitted();
            let answer = rx
                .recv_timeout(Duration::from_secs(4))
                .expect("the frame tick releases the answer");
            worker.join().expect("worker");
            (early, answer)
        }

        #[test]
        fn the_frame_gate_releases_only_waiters_parked_before_the_tick() {
            let _serial = serial();
            let before = park_until_next_frame();
            assert!(before.try_recv().is_err(), "nothing before a frame");
            frame_submitted();
            assert!(before.try_recv().is_ok(), "the next frame releases it");

            let after = park_until_next_frame();
            assert!(after.try_recv().is_err(), "an earlier tick does not count");
            frame_submitted();
            assert!(after.try_recv().is_ok());
        }

        #[test]
        fn a_frame_with_nothing_parked_is_inert() {
            let _serial = serial();
            frame_submitted();
            frame_submitted();
        }

        #[test]
        fn the_shell_backend_offers_hot_patch() {
            let info = backend(&scratch("offer")).handshake_info(&AppInfo::new("a", "0"));
            assert!(info.capabilities.contains(&Capability::HotPatch));
            assert!(info.capabilities.contains(&Capability::WidgetTree));
        }

        #[test]
        fn pid_and_anchor_must_match_and_a_missing_anchor_fails_closed() {
            assert!(check_target((7, 0x10), (7, 0x10)).is_ok());
            assert!(matches!(
                check_target((8, 0x10), (7, 0x10)),
                Err(BackendError::InvalidRequest(_))
            ));
            assert!(matches!(
                check_target((7, 0x11), (7, 0x10)),
                Err(BackendError::InvalidRequest(_))
            ));
            // An unset anchor refuses even a request that "matches" it.
            assert!(matches!(
                check_target((7, 0), (7, 0)),
                Err(BackendError::Unavailable(_))
            ));
        }

        #[test]
        fn a_wrong_pid_is_refused_writes_nothing_and_still_answers_after_the_frame() {
            let _serial = serial();
            frust_hotpatch::set_anchor(anchor() as usize);
            let root = scratch("pid");
            let wrong_pid = std::process::id().wrapping_add(1);
            let (early, answer) = apply_with_frame_tick(
                backend(&root),
                vec![1, 2, 3],
                params(1, 3, wrong_pid, anchor()),
            );
            assert!(!early, "the answer waits for the frame");
            assert!(matches!(answer, Err(BackendError::InvalidRequest(_))));
            assert!(!root.join("frust-hotpatch").exists(), "nothing written");
        }

        #[test]
        fn a_wrong_anchor_runtime_is_refused() {
            let _serial = serial();
            frust_hotpatch::set_anchor(anchor() as usize);
            let root = scratch("anchor");
            let (_, answer) = apply_with_frame_tick(
                backend(&root),
                vec![1, 2, 3],
                params(1, 3, std::process::id(), anchor() + 1),
            );
            assert!(matches!(answer, Err(BackendError::InvalidRequest(_))));
            assert!(!root.join("frust-hotpatch").exists());
        }

        #[test]
        fn a_refused_load_names_no_path_and_leaves_no_file() {
            let _serial = serial();
            frust_hotpatch::set_anchor(anchor() as usize);
            let root = scratch("redact");
            let (early, answer) = apply_with_frame_tick(
                backend(&root),
                vec![0xde, 0xad],
                params(42, 2, std::process::id(), anchor()),
            );
            assert!(!early);
            let Err(error) = answer else {
                panic!("a table not anchored on this image must be refused");
            };
            let wire = error.to_rpc_error();
            let line = frust_devtools_protocol::encode_line(
                &frust_devtools_protocol::Response::error(1, wire),
            );
            let root_text = root.to_string_lossy().into_owned();
            assert!(!line.contains(&root_text), "{line}");
            assert!(!line.contains("frust-hotpatch/"), "{line}");
            assert!(!line.contains("patch-"), "{line}");
            let dir = root.join("frust-hotpatch");
            assert_eq!(
                std::fs::read_dir(&dir).expect("patch dir").count(),
                0,
                "the patch file is removed after the attempt"
            );
        }

        #[test]
        fn a_reused_patch_id_is_refused() {
            let _serial = serial();
            frust_hotpatch::set_anchor(anchor() as usize);
            let root = scratch("reuse");
            let backend = Arc::new(backend(&root));
            for expect_reuse_refusal in [false, true] {
                let b = Arc::clone(&backend);
                let worker = std::thread::spawn(move || {
                    b.apply_patch(vec![1], params(5, 1, std::process::id(), anchor()))
                });
                std::thread::sleep(Duration::from_millis(100));
                frame_submitted();
                let answer = worker.join().expect("worker");
                let reused = matches!(&answer, Err(BackendError::InvalidRequest(m)) if m.contains("already used"));
                assert_eq!(reused, expect_reuse_refusal, "{answer:?}");
            }
        }

        #[test]
        fn unreported_layout_records_refuse_with_applied_false_and_are_then_reported() {
            let _serial = serial();
            frust_hotpatch::set_anchor(anchor() as usize);
            frust_hotpatch::report_layout_mismatch("HotTestState", 4, 8);
            let root = scratch("l2");
            let (early, answer) = apply_with_frame_tick(
                backend(&root),
                vec![1, 2, 3, 4],
                params(9, 4, std::process::id(), anchor()),
            );
            assert!(!early);
            let outcome = answer.expect("a refusal is an outcome, not an error");
            assert!(!outcome.applied);
            assert_eq!(
                outcome.layout_mismatches,
                vec!["HotTestState: stored 4, own 8"]
            );
            assert_eq!(outcome.seam_hits, 0);
            assert_eq!(outcome.patches_applied, 0);
            assert!(frust_hotpatch::pending_layout_mismatches().is_empty());
        }

        #[test]
        fn hotpatch_info_reports_this_process_and_carries_pending_records_once() {
            let _serial = serial();
            frust_hotpatch::set_anchor(anchor() as usize);
            frust_hotpatch::report_layout_mismatch("InfoTestState", 1, 2);
            let backend = backend(&scratch("info"));
            let info = backend.hotpatch_info().expect("info");
            assert_eq!(info.pid, std::process::id());
            assert_eq!(info.anchor_runtime, anchor());
            assert!(info.triple.starts_with(std::env::consts::ARCH));
            assert_eq!(info.patches_applied, 0);
            assert_eq!(
                info.pending_layout_mismatches,
                vec!["InfoTestState: stored 1, own 2"]
            );
            let again = backend.hotpatch_info().expect("info");
            assert!(again.pending_layout_mismatches.is_empty(), "reported once");
        }

        #[test]
        fn patch_chunk_decodes_base64_and_rejects_anything_else() {
            let backend = backend(&scratch("chunk"));
            let chunk = |data: &str| PatchChunkParams {
                patch_id: 1,
                offset: 0,
                total_len: 4,
                data_base64: data.to_string(),
            };
            assert_eq!(
                backend.patch_chunk(&chunk("AAECAw==")),
                Ok(vec![0, 1, 2, 3])
            );
            assert!(matches!(
                backend.patch_chunk(&chunk("not base64!")),
                Err(BackendError::InvalidRequest(_))
            ));
        }

        #[cfg(unix)]
        #[test]
        fn the_patch_file_is_0600_in_a_0700_dir() {
            use std::os::unix::fs::PermissionsExt as _;
            let dir = scratch("mode").join("frust-hotpatch");
            let path = write_patch_file(&dir, "patch-1-1.so", b"bytes").expect("write");
            let mode = |p: &Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(std::fs::read(&path).expect("read"), b"bytes");
        }

        #[cfg(unix)]
        #[test]
        fn a_planted_entry_is_replaced_never_written_through() {
            let root = scratch("plant");
            let dir = root.join("frust-hotpatch");
            std::fs::create_dir_all(&dir).expect("dir");
            let victim = root.join("victim");
            std::fs::write(&victim, b"untouched").expect("victim");
            std::os::unix::fs::symlink(&victim, dir.join("patch-1-2.so")).expect("symlink");

            let path = write_patch_file(&dir, "patch-1-2.so", b"patch").expect("write");
            assert_eq!(std::fs::read(&victim).expect("victim"), b"untouched");
            assert!(
                !std::fs::symlink_metadata(&path)
                    .expect("meta")
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(std::fs::read(&path).expect("patch"), b"patch");
        }

        /// Writes `bytes` to `dir/name` with `mode` and returns the hand-off
        /// that names it, digest included.
        #[cfg(unix)]
        fn handed_off(dir: &Path, name: &str, bytes: &[u8], mode: u32) -> PatchFile {
            use sha2::Digest as _;
            use std::os::unix::fs::PermissionsExt as _;
            let path = dir.join(name);
            std::fs::write(&path, bytes).expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("mode");
            PatchFile {
                path: path.to_string_lossy().into_owned(),
                sha256: format!("{:x}", sha2::Sha256::digest(bytes)),
            }
        }

        /// The refusal `patch_file` answers `file` with; its message must not
        /// carry the path.
        #[cfg(unix)]
        fn refusal(backend: &ShellBackend, file: &PatchFile, len: u64) -> String {
            match backend.patch_file(file, len) {
                Err(BackendError::InvalidRequest(message)) => {
                    assert!(!message.contains(&file.path), "{message}");
                    message
                }
                other => panic!("expected an invalid_request refusal, got {other:?}"),
            }
        }

        #[cfg(unix)]
        #[test]
        fn hotpatch_info_advertises_the_patch_file_hand_off() {
            let _serial = serial();
            let info = backend(&scratch("advert")).hotpatch_info().expect("info");
            assert_eq!(
                info.patch_file_hand_off,
                !cfg!(target_os = "android"),
                "advertised on a shared-filesystem host, never on Android"
            );
        }

        #[cfg(unix)]
        #[test]
        fn a_private_regular_file_with_the_right_len_and_digest_is_read() {
            let root = scratch("handoff-ok");
            let backend = backend(&root);
            let file = handed_off(&root, "patch-1.so", b"patch bytes", 0o600);
            assert_eq!(backend.patch_file(&file, 11), Ok(b"patch bytes".to_vec()));
            let probes = std::fs::read_dir(root.join("frust-hotpatch"))
                .expect("patch dir")
                .count();
            assert_eq!(probes, 0, "the uid probe leaves nothing behind");
        }

        #[cfg(unix)]
        #[test]
        fn a_symlink_is_refused_even_to_a_valid_patch() {
            let root = scratch("handoff-link");
            let backend = backend(&root);
            let target = handed_off(&root, "real.so", b"patch", 0o600);
            let link = root.join("link.so");
            std::os::unix::fs::symlink(&target.path, &link).expect("symlink");
            let file = PatchFile {
                path: link.to_string_lossy().into_owned(),
                sha256: target.sha256,
            };
            let message = refusal(&backend, &file, 5);
            assert!(message.contains("following a link"), "{message}");
        }

        #[cfg(unix)]
        #[test]
        fn a_directory_is_refused() {
            let root = scratch("handoff-dir");
            let backend = backend(&root);
            let dir = root.join("patch-dir.so");
            std::fs::create_dir(&dir).expect("dir");
            let file = PatchFile {
                path: dir.to_string_lossy().into_owned(),
                sha256: "0".repeat(64),
            };
            let message = refusal(&backend, &file, 0);
            assert!(message.contains("not a regular file"), "{message}");
        }

        #[cfg(unix)]
        #[test]
        fn a_group_or_world_readable_file_is_refused() {
            let root = scratch("handoff-mode");
            let backend = backend(&root);
            let file = handed_off(&root, "patch-1.so", b"patch", 0o644);
            let message = refusal(&backend, &file, 5);
            assert!(message.contains("mode 644"), "{message}");
        }

        #[cfg(unix)]
        #[test]
        fn a_wrong_len_is_refused() {
            let root = scratch("handoff-len");
            let backend = backend(&root);
            let file = handed_off(&root, "patch-1.so", b"patch", 0o600);
            let message = refusal(&backend, &file, 6);
            assert!(message.contains("5 bytes, apply_patch says 6"), "{message}");
        }

        #[cfg(unix)]
        #[test]
        fn a_wrong_or_malformed_digest_is_refused() {
            let root = scratch("handoff-digest");
            let backend = backend(&root);
            let mut file = handed_off(&root, "patch-1.so", b"patch", 0o600);
            let good = file.sha256.clone();
            file.sha256 = "0".repeat(64);
            let message = refusal(&backend, &file, 5);
            assert!(message.contains("SHA-256 does not match"), "{message}");
            file.sha256 = good.to_uppercase();
            let message = refusal(&backend, &file, 5);
            assert!(message.contains("lowercase hex"), "{message}");
        }

        #[cfg(unix)]
        #[test]
        fn a_file_owned_by_another_uid_is_refused() {
            use std::os::unix::fs::MetadataExt as _;
            let root = scratch("handoff-owner");
            let file = handed_off(&root, "patch-1.so", b"patch", 0o600);
            let own = std::fs::metadata(&file.path).expect("meta").uid();
            let Err(BackendError::InvalidRequest(message)) =
                read_handed_off_patch(&file, 5, own.wrapping_add(1))
            else {
                panic!("a foreign owner must be refused");
            };
            assert!(message.contains("owned by uid"), "{message}");
            assert!(!message.contains(&file.path), "{message}");
            assert_eq!(effective_uid(&root.join("probe")).expect("uid"), own);
        }

        #[cfg(unix)]
        #[test]
        fn a_relative_path_is_refused() {
            let backend = backend(&scratch("handoff-rel"));
            let file = PatchFile {
                path: "patch-1.so".to_string(),
                sha256: "0".repeat(64),
            };
            let message = refusal(&backend, &file, 1);
            assert!(message.contains("not absolute"), "{message}");
        }

        #[test]
        fn redaction_masks_the_dir_in_every_spelling_and_the_file_name() {
            let dir = scratch("mask").join("frust-hotpatch");
            std::fs::create_dir_all(&dir).expect("dir");
            let canonical = std::fs::canonicalize(&dir).expect("canonical");
            let message = format!(
                "dlopen({}/patch-1-3.so) failed; also {}/patch-1-3.so",
                dir.display(),
                canonical.display()
            );
            let masked = redact(&message, &dir, "patch-1-3.so");
            assert!(!masked.contains(&*dir.to_string_lossy()), "{masked}");
            assert!(!masked.contains(&*canonical.to_string_lossy()), "{masked}");
            assert!(!masked.contains("patch-1-3.so"), "{masked}");
        }

        #[test]
        fn the_target_triple_names_this_arch_and_os_family() {
            let triple = target_triple();
            assert!(triple.starts_with(std::env::consts::ARCH), "{triple}");
            if cfg!(target_os = "macos") {
                assert!(triple.ends_with("-apple-darwin"), "{triple}");
            }
            if cfg!(all(target_os = "linux", target_env = "gnu")) {
                assert!(triple.ends_with("-unknown-linux-gnu"), "{triple}");
            }
        }

        #[test]
        fn only_the_arm64_simulator_spells_the_ios_suffix_with_sim() {
            assert_eq!(ios_suffix(true, true), "apple-ios-sim");
            assert_eq!(ios_suffix(false, true), "apple-ios");
            assert_eq!(ios_suffix(true, false), "apple-ios");
            assert_eq!(ios_suffix(false, false), "apple-ios");
            if cfg!(not(target_os = "ios")) {
                assert!(!target_triple().contains("apple-ios"));
            }
        }
    }
}
