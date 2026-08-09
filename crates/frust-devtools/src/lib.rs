//! `frust-devtools` — the in-app debug service a Frust app hosts for external
//! tooling.
//!
//! It listens on an ephemeral **loopback** TCP port, speaks the NDJSON
//! JSON-RPC protocol defined by [`frust_devtools_protocol`], and answers every
//! request through one trait, [`DevtoolsBackend`], which a shell implements.
//! The tooling side (`frust-drive`/`frust-tui`) never depends on this crate —
//! the two sides meet at the protocol crate and nowhere else
//! (`docs/ARCHITECTURE.md`'s Tooling isolation rule), which is also why this
//! crate depends on no framework crate: it is a service, not a framework
//! layer, and it learns about the app only through the backend it is handed.
//!
//! ```no_run
//! use frust_devtools::{AppInfo, Service};
//! # use frust_devtools::{BackendError, DevtoolsBackend};
//! # use frust_devtools_protocol::{
//! #     InputScrollParams, InputTapParams, MetricsSnapshot, WidgetProps, WidgetTreeDump,
//! # };
//! # struct ShellBackend;
//! # impl DevtoolsBackend for ShellBackend {
//! #     fn widget_tree(&self) -> WidgetTreeDump { WidgetTreeDump { roots: Vec::new() } }
//! #     fn widget_props(&self, _id: u64) -> Option<WidgetProps> { None }
//! #     fn metrics_snapshot(&self) -> MetricsSnapshot {
//! #         MetricsSnapshot { rss_bytes: None, uptime_ms: 0 }
//! #     }
//! #     fn inject_tap(&self, _p: InputTapParams) -> Result<(), BackendError> { Ok(()) }
//! #     fn inject_scroll(&self, _p: InputScrollParams) -> Result<(), BackendError> { Ok(()) }
//! #     fn inject_text(&self, _t: &str) -> Result<(), BackendError> { Ok(()) }
//! # }
//! let devtools = Service::start(ShellBackend, AppInfo::new("my-app", "0.1.0"))?;
//! // ...each frame, from the frame hook — never blocks:
//! // devtools.publish_frame_stats(stats);
//! devtools.shutdown();
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! # Trust model
//!
//! The listener binds `127.0.0.1:0` and **only** `127.0.0.1` — never
//! `0.0.0.0`, not configurably. There is no authentication in v1: the trust
//! boundary is the loopback interface itself, plus (on a device) the
//! `adb forward` / port-forward a developer sets up deliberately. That is
//! sound exactly as far as "any process on this machine is already inside the
//! app's trust boundary" is true, which is the same assumption a debugger
//! attaching to the process makes. It is not sound on a shared or
//! multi-tenant host, and the protocol carries `input_*` methods that drive
//! the real UI — so a shell must gate starting this service on a debug build,
//! never ship it enabled in a release one. Adding auth (a token on the
//! discovery line, checked at handshake) is the natural v2 step if that
//! boundary ever has to move.
//!
//! # Threading & blocking model
//!
//! Three threads are involved, and only one of them is the app's:
//!
//! | Thread | Owns | Blocking rule |
//! |---|---|---|
//! | the shell's frame/UI thread | calls [`ServiceHandle::publish_frame_stats`] | never blocks: a bounded, drop-oldest, sync send |
//! | the service thread | the internal current-thread tokio runtime, the listener, every connection | blocks only on IO it owns |
//! | the backend thread | the [`DevtoolsBackend`] value | runs one sync trait call at a time, in arrival order |
//!
//! A backend method that needs UI-thread state hops there itself; the service
//! caps every call with [`ServiceConfig::backend_timeout`], so a hop that
//! never returns costs that client one error response and nothing else.
//! `handshake` bypasses the backend entirely (its answer is captured at
//! startup), so a client can always identify even a wedged app. `crate::hop`'s
//! module doc carries the full contract, including what happens to a call that
//! completes after its client gave up.
//!
//! # Discovery
//!
//! On start the service logs one line built from
//! [`frust_devtools_protocol::DISCOVERY_PREFIX`] at `info` level; tooling
//! recovers the port from a log/logcat stream with
//! [`frust_devtools_protocol::parse_discovery_line`]. [`ServiceHandle::port`]
//! is the in-process equivalent.

mod backend;
mod dispatch;
mod frame_stats;
mod hop;
mod server;
mod service;

pub use backend::{AppInfo, BackendError, DevtoolsBackend};
pub use service::{Service, ServiceConfig, ServiceHandle};
