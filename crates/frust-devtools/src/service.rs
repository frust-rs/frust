//! [`Service::start`] and [`ServiceHandle`] — what a shell actually touches.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use frust_devtools_protocol::{DISCOVERY_PREFIX, FrameStats};
use tokio::net::TcpListener;
use tokio::runtime::Builder;
use tokio::sync::watch;

use crate::backend::{AppInfo, DevtoolsBackend};
use crate::dispatch::SessionCtx;
use crate::frame_stats::{self, FrameStatsBus};
use crate::hop::spawn_backend_thread;
use crate::server::accept_loop;

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
        let handshake = backend.handshake_info(&app);

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

        // The one discovery contract, formatted from the protocol's own
        // constant so the formatter and `parse_discovery_line` cannot drift.
        // `log` (not `println!`) because that is the only sink that reaches
        // logcat/oslog on a device, which is where tooling greps for it.
        log::info!("{DISCOVERY_PREFIX}{port}");

        let bus = Arc::new(FrameStatsBus::new(config.frame_stats_capacity));
        let backend_client =
            spawn_backend_thread(backend, config.backend_queue_depth, config.backend_timeout);
        let ctx = Arc::new(SessionCtx {
            handshake,
            backend: backend_client,
            bus: Arc::clone(&bus),
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
            bus,
            shutdown_tx,
            driver: Some(driver),
        })
    }
}

/// The running service. Dropping it shuts the service down, so a shell can
/// simply hold it for as long as devtools should be available.
pub struct ServiceHandle {
    port: u16,
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceHandle")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}
