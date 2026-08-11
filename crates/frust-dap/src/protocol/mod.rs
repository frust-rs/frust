//! Wire-level types and Content-Length framing codec for the Debug Adapter
//! Protocol (DAP). Every message between a DAP client (VS Code, an editor's
//! DAP client) and a Frust DAP session flows through this module.
//!
//! - [`types`] — the tagged `DapMessage` core plus the orchestration-v1
//!   payload subset (`InitializeRequestArguments`, `Capabilities`,
//!   `LaunchArguments`, `output`/`exited`/`terminated` event bodies,
//!   `Thread`/`ThreadsResponseBody`).
//! - [`codec`] — `Content-Length: N\r\n\r\n` + UTF-8 JSON body framing over
//!   a generic async reader/writer: `read_message`, `write_message`.

pub mod codec;
pub mod types;

pub use codec::{CodecError, MAX_MESSAGE_SIZE, Result, read_message, write_message};
pub use types::{
    Capabilities, DapEvent, DapMessage, DapRequest, DapResponse, ExitedEventBody,
    InitializeRequestArguments, LaunchArguments, OutputEventBody, Thread, ThreadsResponseBody,
};
