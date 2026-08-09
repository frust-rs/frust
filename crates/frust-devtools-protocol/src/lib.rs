//! `frust-devtools-protocol` — the wire contract between an in-app Frust
//! debug service and external tooling (`frust-drive`/`frust-tui`).
//!
//! This is the **one sanctioned crossing** of the tooling-isolation charter
//! (`docs/ARCHITECTURE.md`'s Cross-Unit Layer Dependencies: "`frust-cli`,
//! `frust-drive`, and `frust-tui` depend on NO framework crate"). Both sides
//! of the devtools wire depend on this crate, and only this crate, to agree
//! on message shapes — neither side depends on the other, and this crate
//! stays a **leaf**: `serde` (derive) + `serde_json` only, no `tokio`, no
//! framework crate, no other `frust-*` crate. Pulling it into the tooling
//! side must never drag framework or async-runtime code along with it.
//!
//! # Framing
//!
//! One JSON-RPC 2.0 object per `\n`-terminated line — no `Content-Length`
//! headers. [`encode_line`]/[`decode_line`] are the pure (no I/O)
//! serialize/discriminate pair; [`Incoming`] tells the caller whether a
//! decoded line was a [`Request`], [`Response`], or [`Notification`].
//!
//! # Discovery
//!
//! A server announces its listening port with one printed line built from
//! [`DISCOVERY_PREFIX`]; [`parse_discovery_line`] is the single parser both
//! the service (which formats it) and tooling (which greps for it) share.
//!
//! # Methods
//!
//! [`Method`] is the typed v1 method set; [`messages`]-derived re-exports
//! below are each method's typed params/result/notification-payload struct.
//! This crate is pure data + pure functions — no I/O, no async, no runtime
//! state of its own.

mod codec;
mod discovery;
mod messages;
mod method;
mod types;

pub use codec::{DecodeError, decode_line, encode_line};
pub use discovery::{DISCOVERY_PREFIX, parse_discovery_line};
pub use messages::{
    AckResult, Capability, FrameStats, HandshakeInfo, InputScrollParams, InputTapParams,
    InputTextParams, MetricsSnapshot, RectPx, ScreenshotResult, WidgetNode, WidgetProps,
    WidgetPropsParams, WidgetTreeDump,
};
pub use method::Method;
pub use types::{
    Incoming, JSONRPC_VERSION, Notification, Request, Response, ResponseOutcome, RpcError,
};

/// The devtools wire protocol version this crate implements —
/// [`HandshakeInfo::protocol_version`]'s value.
pub const PROTOCOL_VERSION: u32 = 1;
