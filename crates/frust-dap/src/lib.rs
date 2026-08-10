//! Debug Adapter Protocol (DAP) server for Frust.
//!
//! **Print-free, always.** In stdio mode `stdout` *is* the DAP wire — a
//! `Content-Length`-framed JSON message stream to the client. A stray
//! `println!`/`print!`/`eprintln!`/`eprint!` anywhere in this crate corrupts
//! that stream (or, on stderr, a client's captured diagnostics). Every
//! diagnostic goes through `log` instead.
//!
//! ## Layers
//!
//! - [`protocol`] — wire types and the Content-Length codec.
//! - [`transport`] — which pipe the server speaks over ([`TransportMode`]).
//! - [`server`] — the accept/serve entry points and the per-connection
//!   session state machine that owns the DAP lifecycle.
//!
//! The session hands everything past `initialize` to a [`DapAdapter`], built
//! per connection from an [`EventSender`]. That is the whole seam: the session
//! knows the protocol, the adapter knows what a Frust app is.

pub mod protocol;
pub mod server;
pub mod transport;

pub use protocol::codec::{self, CodecError, Result};
pub use protocol::types::{
    Capabilities, DapEvent, DapMessage, DapRequest, DapResponse, ExitedEventBody,
    InitializeRequestArguments, LaunchArguments, OutputEventBody, Thread, ThreadsResponseBody,
};
pub use server::{
    AdapterResponse, DapAdapter, EventSender, ServerError, run_session, serve, serve_tcp,
};
pub use transport::TransportMode;
pub use transport::stdio::run_stdio_session;
