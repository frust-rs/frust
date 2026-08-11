//! Debug Adapter Protocol (DAP) server for Frust, **embedded in its host**.
//!
//! There is no `frust dap` process: [`serve_embedded`] runs a loopback DAP
//! listener on the host's tokio runtime, over the host's own
//! `frust_mcp::SessionBackend` and project root. An editor attaching to it and
//! the host's own UI therefore drive the *same* app sessions — one world, two
//! front ends — and stopping a debug session stops that session's app, never
//! the host.
//!
//! **Print-free, always.** This crate runs inside a workbench that owns the
//! terminal (raw mode, a full-screen UI): a stray
//! `println!`/`print!`/`eprintln!`/`eprint!` anywhere in it corrupts that
//! host's display, and there is no stdout of its own to write to. Every
//! diagnostic goes through `log` instead, which the host routes where it
//! chooses.
//!
//! ## Layers
//!
//! - [`protocol`] — wire types and the Content-Length codec.
//! - [`clients`] — [`DapClientRegistry`], the connected-client view a host
//!   renders.
//! - [`server`] — [`serve_embedded`] and the loopback accept loop, plus the
//!   per-connection session state machine that owns the DAP lifecycle.
//! - [`adapter`] — [`OrchestrationAdapter`], which turns DAP requests into
//!   `SessionBackend` operations (build → deploy → launch → logs → stop) and
//!   the app's log lines into `output` events.
//! - [`ide_config`] — generating and merging the client-side launch
//!   configuration an editor needs to attach.
//!
//! The session hands everything past `initialize` to a [`DapAdapter`], built
//! per connection from an [`EventSender`]. That is the whole seam: the session
//! knows the protocol, the adapter knows what a Frust app is.

pub mod adapter;
pub mod clients;
pub mod ide_config;
pub mod protocol;
mod sanitize;
pub mod server;

pub use adapter::OrchestrationAdapter;
pub use clients::{DapClientEntry, DapClientRegistry};
pub use frust_mcp::SharedBackend;
pub use protocol::codec::{self, CodecError, Result};
pub use protocol::types::{
    Capabilities, DapEvent, DapMessage, DapRequest, DapResponse, ExitedEventBody,
    InitializeRequestArguments, LaunchArguments, OutputEventBody, Thread, ThreadsResponseBody,
};
pub use server::{
    AdapterResponse, DapAdapter, EventSender, ServerError, run_session, serve_embedded, serve_tcp,
};

/// The loopback port a host binds [`serve_embedded`] to unless told otherwise.
///
/// A **fixed** default rather than `0` (an OS-assigned ephemeral port) on
/// purpose: the whole point of the embed is that an editor's launch
/// configuration can name the port ahead of time and keep working across
/// workbench restarts, which an ephemeral port makes impossible. `0` stays
/// available for a caller that genuinely wants one (the tests do).
///
/// Unregistered with IANA and outside the ephemeral range Linux hands out by
/// default (32768–60999), so a normal desktop session will not have something
/// else sitting on it.
pub const DEFAULT_DAP_PORT: u16 = 4849;
