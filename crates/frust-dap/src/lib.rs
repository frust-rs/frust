//! Debug Adapter Protocol (DAP) server for Frust.
//!
//! **Print-free, always.** In stdio mode `stdout` *is* the DAP wire — a
//! `Content-Length`-framed JSON message stream to the client. A stray
//! `println!`/`print!`/`eprintln!`/`eprint!` anywhere in this crate corrupts
//! that stream (or, on stderr, a client's captured diagnostics). Every
//! diagnostic goes through `log` instead.

pub mod protocol;

pub use protocol::codec::{self, CodecError, Result};
pub use protocol::types::{
    Capabilities, DapEvent, DapMessage, DapRequest, DapResponse, ExitedEventBody,
    InitializeRequestArguments, LaunchArguments, OutputEventBody, Thread, ThreadsResponseBody,
};
