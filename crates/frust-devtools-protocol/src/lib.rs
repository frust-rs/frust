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
//! # Discovery and auth
//!
//! A server announces its listening port — and the per-process token a client
//! must present at `handshake` — with one printed line built from
//! [`DISCOVERY_PREFIX`]. [`format_discovery_line`]/[`parse_discovery_line`]
//! are the single formatter/parser pair the service and tooling share, and
//! [`Discovery`] is what a parsed line yields. The token travels back to the
//! server exactly once, in [`HandshakeParams`]; a connection that has not
//! presented it is answered [`RpcError::UNAUTHORIZED`] for every other method.
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

/// Whole-crate valve for `serde_json`, this crate's one public-API dependency.
///
/// [`Request::params`]/[`Response`]'s `result` are `serde_json::Value`, so a
/// peer cannot build or read a message without that crate — and it must be the
/// *same* version this crate speaks. Reaching through this valve
/// (`frust_devtools_protocol::serde_json::to_value(..)`) instead of
/// re-declaring the dependency keeps the pin in exactly one manifest, the same
/// rule `frust::kurbo`/`frust::peniko` follow for app code
/// (`docs/CODE_STANDARDS.md`'s State & Reactivity Conventions).
pub use serde_json;

pub use codec::{DecodeError, decode_line, encode_line};
pub use discovery::{
    DISCOVERY_PREFIX, Discovery, FAILURE_PREFIX, format_discovery_line, format_failure_line,
    parse_discovery_line, parse_failure_line,
};
pub use messages::{
    AckResult, Capability, FrameStats, HandshakeInfo, HandshakeParams, InputScrollParams,
    InputTapParams, InputTextParams, MetricsSnapshot, RectPx, ScreenshotResult, WidgetNode,
    WidgetProps, WidgetPropsParams, WidgetTreeDump,
};
pub use method::Method;
pub use types::{
    Incoming, JSONRPC_VERSION, Notification, Request, Response, ResponseOutcome, RpcError,
};

/// The devtools wire protocol version this crate implements —
/// [`HandshakeInfo::protocol_version`]'s value.
pub const PROTOCOL_VERSION: u32 = 1;
