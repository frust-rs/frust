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
//! `0.0.0.0`, not configurably. Loopback alone is *not* the boundary, though:
//! on a device every co-resident app can reach `127.0.0.1:<port>` too, and the
//! protocol carries `input_*` methods that drive the real UI plus a widget-tree
//! dump that is user data. So the service also mints a random per-process
//! **token**, prints it on its discovery line, and requires it at `handshake`
//! before dispatching any other method — the same shape the Dart VM service's
//! auth code has. That works because only a privileged reader sees the line:
//! another Android app cannot read this app's logcat (`READ_LOGS` is a
//! privileged permission), and a desktop app's stderr reaches only the tooling
//! process that launched it.
//!
//! Two layers still sit above it, and neither is optional: a shell gates
//! starting the service on a debug/profile build via a cargo feature (a release
//! build compiles the listener out entirely), and `ServiceConfig::require_token`
//! — default **on** — is the only way to run without auth, meant for in-process
//! tests, never a shipped build. See `crate::token` for the token's entropy
//! source, stated with its limits.
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
//! On start the service logs one line built by
//! [`frust_devtools_protocol::format_discovery_line`] at `info` level, carrying
//! the port and (with auth on) the token; tooling recovers both from a
//! log/logcat stream with
//! [`frust_devtools_protocol::parse_discovery_line`].
//! [`ServiceHandle::port`]/[`ServiceHandle::token`] are the in-process
//! equivalents.

mod backend;
mod dispatch;
mod frame_stats;
mod hop;
mod server;
mod service;
mod token;

pub use backend::{AppInfo, BackendError, DevtoolsBackend};
pub use service::{Service, ServiceConfig, ServiceHandle};
