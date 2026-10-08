//! Line → [`Method`] → backend call → [`Response`]: the protocol half of the
//! service, with no I/O in it.
//!
//! Split from `crate::server` (which owns sockets and tasks) so every wire
//! decision — which method exists, which error code an unknown one earns, when
//! a malformed line is answerable and when it can only be logged — is decided
//! in one place, and can be tested without a socket.

use std::sync::Arc;

use frust_devtools_protocol::{
    AckResult, FrameStats, HandshakeInfo, HandshakeParams, Incoming, InputScrollParams,
    InputTapParams, InputTextParams, Method, Notification, Request, Response, RpcError,
    WidgetPropsParams, decode_line, serde_json, serde_json::Value,
};

use crate::frame_stats::FrameStatsBus;
use crate::hop::{BackendClient, Call, CallOutcome};
use crate::service::HotPatchUnavailable;
use crate::token;

/// Everything a connection needs to answer a request. One instance, shared by
/// every connection task.
pub(crate) struct SessionCtx {
    /// Captured once at startup — see [`crate::DevtoolsBackend::handshake_info`].
    pub(crate) handshake: HandshakeInfo,
    pub(crate) backend: BackendClient,
    pub(crate) bus: Arc<FrameStatsBus>,
    /// This process's handshake token, or `None` when the service was started
    /// with auth switched off (`ServiceConfig::require_token`). Never leaves
    /// the process: it is compared against what a client presents, and is
    /// written to no response, notification, or error payload.
    pub(crate) token: Option<String>,
    /// Whether the hot-patch methods are dispatched, decided once at startup by
    /// `crate::service::hot_patch_gate`. `Enabled` exactly when the cached
    /// [`HandshakeInfo`] carries `Capability::HotPatch`.
    pub(crate) hot_patch: HotPatch,
}

/// The hot-patch state of a service.
pub(crate) enum HotPatch {
    /// The capability is absent: `patch_chunk`/`apply_patch` answer
    /// `METHOD_NOT_FOUND`, and `hotpatch_info` answers `NOT_SUPPORTED` naming
    /// the failed precondition.
    Unavailable(HotPatchUnavailable),
    /// Every precondition held: the methods run on the hot-patch lane.
    #[cfg(feature = "hotpatch")]
    Enabled(HotpatchLane),
}

/// Per-connection protocol state. Separate from [`SessionCtx`] because it is
/// exactly the state that must **not** be shared: one client authenticating
/// must not authenticate any other connection, and one connection's patch
/// chunks must never complete a patch another connection applies.
#[derive(Debug)]
pub(crate) struct ConnState {
    authenticated: bool,
    #[cfg(feature = "hotpatch")]
    patches: PatchAssembly,
}

impl ConnState {
    /// A fresh connection: unauthenticated when the service requires a token,
    /// and pre-authenticated when it does not (so an auth-off service behaves
    /// exactly as it did before auth existed).
    pub(crate) fn new(ctx: &SessionCtx) -> Self {
        Self {
            authenticated: ctx.token.is_none(),
            #[cfg(feature = "hotpatch")]
            patches: PatchAssembly::default(),
        }
    }
}

/// What a received line turned out to be.
#[derive(Debug, PartialEq)]
pub(crate) enum Decoded {
    /// A well-formed request to answer.
    Request(Box<Request>),
    /// A well-formed line this server has nothing to do with (a client that
    /// sent a response or a notification). Carries the reason, for logging.
    Ignore(&'static str),
    /// A line that could not be decoded, but carried a recoverable `id` — so
    /// the client gets a `PARSE_ERROR` reply it can correlate.
    Malformed { id: u64, detail: String },
    /// A line that could not be decoded and carried no usable `id`. There is
    /// nothing to correlate a reply to, so the only correct move is to log and
    /// skip (JSON-RPC 2.0 §5: a response is routed by its id; a reply the
    /// client cannot correlate is worse than silence).
    Undeliverable(String),
}

/// Classifies one received line. Pure — no backend, no socket.
pub(crate) fn decode(line: &str) -> Decoded {
    match decode_line(line) {
        Ok(Incoming::Request(req)) => Decoded::Request(Box::new(req)),
        Ok(Incoming::Response(_)) => Decoded::Ignore("clients do not send responses"),
        Ok(Incoming::Notification(_)) => {
            Decoded::Ignore("this server subscribes to no client push")
        }
        Err(e) => match recover_id(line) {
            Some(id) => Decoded::Malformed {
                id,
                detail: e.to_string(),
            },
            None => Decoded::Undeliverable(e.to_string()),
        },
    }
}

/// Best-effort `id` salvage from a line that failed to decode: a valid JSON
/// object with an integer `id` is enough to correlate a `PARSE_ERROR` reply,
/// even when the rest of the object is unusable.
fn recover_id(line: &str) -> Option<u64> {
    let value: Value = serde_json::from_str(line.trim_end_matches('\n')).ok()?;
    value.get("id")?.as_u64()
}

/// The `PARSE_ERROR` reply to a [`Decoded::Malformed`] line.
pub(crate) fn parse_error_response(id: u64, detail: &str) -> Response {
    Response::error(
        id,
        RpcError::new(
            RpcError::PARSE_ERROR,
            format!("could not decode request: {detail}"),
        ),
    )
}

/// The effect a handled request has on the connection beyond its reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SideEffect {
    None,
    /// Start forwarding `frame_stats` notifications to this client.
    SubscribeFrameStats,
}

/// Answers one request, hopping to the backend where the method needs it.
///
/// `conn` carries this connection's authentication state: `handshake` is the
/// only method dispatched before it is established, and a successful one is
/// what establishes it.
pub(crate) async fn handle_request(
    ctx: &SessionCtx,
    conn: &mut ConnState,
    req: &Request,
) -> (Response, SideEffect) {
    let id = req.id;
    let Some(method) = Method::from_str(&req.method) else {
        return (method_not_found(id, &req.method), SideEffect::None);
    };

    let no_effect = |response| (response, SideEffect::None);

    // The auth gate, ahead of every dispatch arm below: an unauthenticated
    // connection may call `handshake` and nothing else. Deliberately *after*
    // the unknown-method check only because an unknown name has no meaning to
    // gate — it reaches no backend and reveals nothing about the app.
    if method != Method::Handshake && !conn.authenticated {
        return no_effect(Response::error(
            id,
            RpcError::unauthorized(format!(
                "`{method}` requires an authenticated connection — call `handshake` with the \
                 token from this app's devtools discovery line first"
            )),
        ));
    }

    match method {
        // Answered from the startup cache, never from the backend: this is
        // what keeps `handshake` available while the UI thread is wedged —
        // including the token check, which touches no app state either.
        //
        // The token is checked once per connection, not once per call:
        // `handshake` doubles as "identify this app", and a client that already
        // proved itself re-asking for the app's identity gains nothing by
        // presenting the secret again (it would only put the token on the wire
        // more often). Authentication is never revoked on a live connection.
        Method::Handshake if conn.authenticated => {
            no_effect(result_response(id, serde_json::to_value(&ctx.handshake)))
        }
        Method::Handshake => match authenticate(ctx, req) {
            Ok(()) => {
                conn.authenticated = true;
                no_effect(result_response(id, serde_json::to_value(&ctx.handshake)))
            }
            Err(error) => no_effect(Response::error(id, error)),
        },

        // A pure connection-state change — nothing to ask the backend.
        Method::FrameStatsSubscribe => (
            result_response(id, serde_json::to_value(AckResult::default())),
            SideEffect::SubscribeFrameStats,
        ),

        // Server→client only. It decodes, and it is a known name, so
        // METHOD_NOT_FOUND would be a lie; the request itself is what is
        // invalid.
        Method::FrameStats => no_effect(Response::error(
            id,
            RpcError::new(
                RpcError::INVALID_REQUEST,
                "`frame_stats` is a server-to-client notification, not a request — call \
                 `frame_stats_subscribe` instead",
            ),
        )),

        Method::WidgetTree => no_effect(backend_response(ctx, id, Call::WidgetTree).await),
        Method::MetricsSnapshot => {
            no_effect(backend_response(ctx, id, Call::MetricsSnapshot).await)
        }
        Method::Screenshot => no_effect(backend_response(ctx, id, Call::Screenshot).await),

        Method::WidgetProps => {
            match serde_json::from_value::<WidgetPropsParams>(req.params.clone()) {
                Ok(p) => no_effect(backend_response(ctx, id, Call::WidgetProps(p.id)).await),
                Err(e) => no_effect(Response::error(id, invalid_params(req, &e))),
            }
        }
        Method::InputTap => match serde_json::from_value::<InputTapParams>(req.params.clone()) {
            Ok(p) => no_effect(backend_response(ctx, id, Call::InputTap(p)).await),
            Err(e) => no_effect(Response::error(id, invalid_params(req, &e))),
        },
        Method::InputScroll => {
            match serde_json::from_value::<InputScrollParams>(req.params.clone()) {
                Ok(p) => no_effect(backend_response(ctx, id, Call::InputScroll(p)).await),
                Err(e) => no_effect(Response::error(id, invalid_params(req, &e))),
            }
        }
        Method::InputText => match serde_json::from_value::<InputTextParams>(req.params.clone()) {
            Ok(p) => no_effect(backend_response(ctx, id, Call::InputText(p.text)).await),
            Err(e) => no_effect(Response::error(id, invalid_params(req, &e))),
        },

        // Capability-gated (`Capability::HotPatch`): see `hot_patch_request`.
        Method::HotpatchInfo | Method::PatchChunk | Method::ApplyPatch => {
            no_effect(hot_patch_request(ctx, conn, method, req).await)
        }
    }
}

/// The reply to a method this server does not dispatch. Shared by an unknown
/// name and by a hot-patch method while the capability is absent, so the two
/// are indistinguishable on the wire.
fn method_not_found(id: u64, name: &str) -> Response {
    Response::error(
        id,
        RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            format!("unknown method `{name}`"),
        ),
    )
}

/// `hotpatch_info` while the capability is absent: `NOT_SUPPORTED`, naming the
/// precondition that failed so the CLI/TUI can say why the session is
/// restart-only.
fn hot_patch_unavailable(id: u64, why: HotPatchUnavailable) -> Response {
    Response::error(
        id,
        RpcError::not_supported(format!("hot patching unavailable: {why}")),
    )
}

/// A hot-patch method on a service built without this crate's `hotpatch`
/// feature: the capability can never be present.
#[cfg(not(feature = "hotpatch"))]
async fn hot_patch_request(
    ctx: &SessionCtx,
    _conn: &mut ConnState,
    method: Method,
    req: &Request,
) -> Response {
    let HotPatch::Unavailable(why) = ctx.hot_patch;
    match method {
        Method::HotpatchInfo => hot_patch_unavailable(req.id, why),
        _ => method_not_found(req.id, &req.method),
    }
}

/// A hot-patch method: answered as an unknown method (or `NOT_SUPPORTED` for
/// `hotpatch_info`) while the capability is absent, else dispatched to the
/// hot-patch lane.
///
/// `patch_chunk` appends to this connection's reassembly, and `apply_patch`
/// consumes it: a `patch_id` whose chunks arrived on another connection is
/// unknown here, and the reassembled length must equal `len`.
#[cfg(feature = "hotpatch")]
async fn hot_patch_request(
    ctx: &SessionCtx,
    conn: &mut ConnState,
    method: Method,
    req: &Request,
) -> Response {
    let id = req.id;
    let lane = match &ctx.hot_patch {
        HotPatch::Enabled(lane) => lane,
        HotPatch::Unavailable(why) => {
            return match method {
                Method::HotpatchInfo => hot_patch_unavailable(id, *why),
                _ => method_not_found(id, &req.method),
            };
        }
    };

    match method {
        Method::HotpatchInfo => match lane.info().await {
            Ok(info) => result_response(id, serde_json::to_value(info)),
            Err(e) => Response::error(id, e),
        },
        Method::PatchChunk => {
            let chunk = match serde_json::from_value::<frust_devtools_protocol::PatchChunkParams>(
                req.params.clone(),
            ) {
                Ok(chunk) => chunk,
                Err(e) => return Response::error(id, invalid_params(req, &e)),
            };
            // Shape checks before the backend decodes anything: an over-cap
            // chunk is refused without being decoded or buffered.
            if let Err(e) = conn.patches.admit(&chunk) {
                return Response::error(id, e);
            }
            let bytes = match lane.chunk(chunk.clone()).await {
                Ok(bytes) => bytes,
                Err(e) => return Response::error(id, e),
            };
            match conn.patches.append(&chunk, bytes) {
                Ok(()) => result_response(id, serde_json::to_value(AckResult::default())),
                Err(e) => Response::error(id, e),
            }
        }
        Method::ApplyPatch => {
            // `deny_unknown_fields`: a `lib`/`path` field fails here.
            let params = match serde_json::from_value::<frust_devtools_protocol::ApplyPatchParams>(
                req.params.clone(),
            ) {
                Ok(params) => params,
                Err(e) => return Response::error(id, invalid_params(req, &e)),
            };
            let bytes = match conn.patches.take_complete(params.patch_id, params.len) {
                Ok(bytes) => bytes,
                Err(e) => return Response::error(id, e),
            };
            match lane.apply(bytes, params).await {
                Ok(outcome) => result_response(id, serde_json::to_value(outcome)),
                Err(e) => Response::error(id, e),
            }
        }
        // Only the three hot-patch methods are routed here.
        _ => method_not_found(id, &req.method),
    }
}

/// Checks a `handshake` request's token against this process's.
///
/// Malformed params are treated as "no token presented" rather than
/// `INVALID_PARAMS`: an unauthenticated peer learns only that it is
/// unauthorized, never anything about the shape the server expected.
fn authenticate(ctx: &SessionCtx, req: &Request) -> Result<(), RpcError> {
    let Some(expected) = ctx.token.as_deref() else {
        return Ok(()); // Auth switched off for this service.
    };
    let presented = serde_json::from_value::<HandshakeParams>(req.params.clone())
        .ok()
        .and_then(|params| params.token);
    match presented {
        Some(token) if token::matches(expected, &token) => Ok(()),
        Some(_) => {
            log::warn!("frust-devtools: rejected a handshake presenting the wrong token");
            Err(RpcError::unauthorized("invalid devtools token"))
        }
        None => Err(RpcError::unauthorized(
            "this app's devtools service requires the token printed on its discovery line",
        )),
    }
}

async fn backend_response(ctx: &SessionCtx, id: u64, call: Call) -> Response {
    match ctx.backend.call(call).await {
        CallOutcome::WidgetTree(tree) => result_response(id, serde_json::to_value(tree)),
        CallOutcome::WidgetProps(props) => result_response(id, serde_json::to_value(props)),
        CallOutcome::Metrics(metrics) => result_response(id, serde_json::to_value(metrics)),
        CallOutcome::Ack => result_response(id, serde_json::to_value(AckResult::default())),
        CallOutcome::Screenshot(shot) => result_response(id, serde_json::to_value(shot)),
        CallOutcome::Failed(e) => Response::error(id, e),
    }
}

fn invalid_params(req: &Request, e: &serde_json::Error) -> RpcError {
    RpcError::new(
        RpcError::INVALID_PARAMS,
        format!("invalid params for `{}`: {e}", req.method),
    )
}

/// Wraps an already-serialized result into a success [`Response`].
///
/// Serialization of this crate's own wire types cannot fail in practice; if it
/// somehow did, an `INTERNAL_ERROR` reply is still better than a dropped
/// request, so the failure is answered rather than unwrapped.
fn result_response(id: u64, value: Result<Value, serde_json::Error>) -> Response {
    match value {
        Ok(v) => Response::success(id, v),
        Err(e) => Response::error(
            id,
            RpcError::new(
                RpcError::INTERNAL_ERROR,
                format!("failed to serialize result: {e}"),
            ),
        ),
    }
}

/// The `frame_stats` push a subscribed client receives each frame.
pub(crate) fn frame_stats_notification(stats: FrameStats) -> Notification {
    let params = serde_json::to_value(stats).unwrap_or(Value::Null);
    Notification::new(Method::FrameStats.as_str(), params)
}

// ---------------------------------------------------------------------
// Hot patching: per-connection chunk reassembly and the hot-patch lane
// ---------------------------------------------------------------------

/// Most raw bytes one `patch_chunk` may carry (the protocol's 512 KiB, which
/// keeps its base64 text under the 1 MiB line cap).
#[cfg(feature = "hotpatch")]
pub(crate) const MAX_CHUNK_BYTES: usize = 512 * 1024;

/// Longest base64 text a chunk of [`MAX_CHUNK_BYTES`] encodes to (padded), so
/// an over-cap chunk is refused before anything decodes it.
#[cfg(feature = "hotpatch")]
const MAX_CHUNK_BASE64: usize = MAX_CHUNK_BYTES.div_ceil(3) * 4;

/// Largest patch a connection may assemble. A thin patch is a few MB (the
/// spike's 1.5 MB); the cap bounds what one authenticated client can make
/// this process buffer. Buffers grow with the bytes actually received, never
/// pre-sized from a claimed `total_len`.
#[cfg(feature = "hotpatch")]
pub(crate) const MAX_PATCH_BYTES: u64 = 64 * 1024 * 1024;

/// How long an `apply_patch` may take before its client is answered with an
/// error: a library load plus a wait for the following frame, which the shell
/// side bounds itself.
#[cfg(feature = "hotpatch")]
const APPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// One patch being assembled on a connection.
#[cfg(feature = "hotpatch")]
struct PendingPatch {
    patch_id: u64,
    total_len: u64,
    bytes: Vec<u8>,
}

/// A connection's chunk reassembly: **one** patch at a time, chunks in order
/// and contiguous (`offset` equal to the bytes received so far), every chunk
/// agreeing on `total_len`. A chunk at offset 0 for a new `patch_id` discards
/// any unfinished one, so an abandoned transfer never wedges the connection
/// and the buffer never holds more than one patch.
#[cfg(feature = "hotpatch")]
#[derive(Default)]
pub(crate) struct PatchAssembly {
    pending: Option<PendingPatch>,
}

#[cfg(feature = "hotpatch")]
impl std::fmt::Debug for PatchAssembly {
    /// Lengths only, never the bytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("PatchAssembly");
        if let Some(p) = &self.pending {
            s.field("patch_id", &p.patch_id)
                .field("received", &p.bytes.len())
                .field("total_len", &p.total_len);
        }
        s.finish()
    }
}

#[cfg(feature = "hotpatch")]
fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(RpcError::INVALID_PARAMS, message)
}

#[cfg(feature = "hotpatch")]
impl PatchAssembly {
    /// The checks that need no decoding: size caps, `total_len` agreement, and
    /// that `offset` continues this connection's transfer of `patch_id`.
    pub(crate) fn admit(
        &self,
        chunk: &frust_devtools_protocol::PatchChunkParams,
    ) -> Result<(), RpcError> {
        if chunk.data_base64.len() > MAX_CHUNK_BASE64 {
            return Err(invalid(format!(
                "patch chunk exceeds {MAX_CHUNK_BYTES} raw bytes"
            )));
        }
        if chunk.data_base64.is_empty() {
            return Err(invalid("a patch chunk must carry at least one byte"));
        }
        if chunk.total_len == 0 || chunk.total_len > MAX_PATCH_BYTES {
            return Err(invalid(format!(
                "patch total_len must be 1..={MAX_PATCH_BYTES} bytes"
            )));
        }
        match &self.pending {
            Some(p) if p.patch_id == chunk.patch_id => {
                if p.total_len != chunk.total_len {
                    return Err(invalid(format!(
                        "patch {} changed total_len from {} to {}",
                        chunk.patch_id, p.total_len, chunk.total_len
                    )));
                }
                if chunk.offset != p.bytes.len() as u64 {
                    return Err(invalid(format!(
                        "patch {} chunk at offset {}, expected {}",
                        chunk.patch_id,
                        chunk.offset,
                        p.bytes.len()
                    )));
                }
            }
            // A new transfer starts at offset 0 (and replaces any unfinished one).
            _ if chunk.offset != 0 => {
                return Err(invalid(format!(
                    "patch {} has no chunks on this connection; its first chunk must be at offset 0",
                    chunk.patch_id
                )));
            }
            _ => {}
        }
        Ok(())
    }

    /// Appends one decoded chunk after [`Self::admit`] passed for it.
    pub(crate) fn append(
        &mut self,
        chunk: &frust_devtools_protocol::PatchChunkParams,
        data: Vec<u8>,
    ) -> Result<(), RpcError> {
        self.admit(chunk)?;
        if data.is_empty() || data.len() > MAX_CHUNK_BYTES {
            return Err(invalid(format!(
                "a decoded patch chunk must be 1..={MAX_CHUNK_BYTES} bytes"
            )));
        }
        if chunk.offset + data.len() as u64 > chunk.total_len {
            return Err(invalid(format!(
                "patch {} chunk runs past total_len {}",
                chunk.patch_id, chunk.total_len
            )));
        }
        let continuing = matches!(&self.pending, Some(p) if p.patch_id == chunk.patch_id);
        if !continuing {
            if let Some(dropped) = self.pending.take() {
                log::debug!(
                    "frust-devtools: patch {} superseded by patch {} before it completed",
                    dropped.patch_id,
                    chunk.patch_id
                );
            }
            self.pending = Some(PendingPatch {
                patch_id: chunk.patch_id,
                total_len: chunk.total_len,
                bytes: Vec::new(),
            });
        }
        if let Some(p) = self.pending.as_mut() {
            p.bytes.extend_from_slice(&data);
        }
        Ok(())
    }

    /// Hands over `patch_id`'s bytes for `apply_patch`: it must have been
    /// assembled on **this** connection, be complete, and be exactly `len`
    /// bytes. A complete patch is consumed whatever the answer (a length
    /// mismatch means its bytes cannot be trusted); an incomplete one stays.
    pub(crate) fn take_complete(&mut self, patch_id: u64, len: u64) -> Result<Vec<u8>, RpcError> {
        let Some(p) = self.pending.take_if(|p| p.patch_id == patch_id) else {
            return Err(invalid(format!(
                "patch {patch_id} has no chunks on this connection"
            )));
        };
        if (p.bytes.len() as u64) < p.total_len {
            let message = format!(
                "patch {patch_id} is incomplete: {} of {} bytes received",
                p.bytes.len(),
                p.total_len
            );
            self.pending = Some(p);
            return Err(invalid(message));
        }
        if p.bytes.len() as u64 != len {
            return Err(invalid(format!(
                "patch {patch_id} is {} bytes, apply_patch says {len}",
                p.bytes.len()
            )));
        }
        Ok(p.bytes)
    }
}

/// The hot-patch lane: where the three hot-patch backend calls run.
///
/// Not the backend thread (`crate::hop`): each call runs on its own short-lived
/// worker thread over the backend shared with that thread
/// (`crate::service::SharedBackend`, one mutex, so the backend still sees one
/// call at a time). Like the backend thread, a worker is never joined, so a
/// wedged UI thread cannot hold shutdown hostage; and `apply_patch` gets its
/// own, longer [`APPLY_TIMEOUT`], since it waits for the frame after the apply.
///
/// One patch is in flight at a time: a second `apply_patch` while one runs —
/// including one whose client already timed out, which still completes — is
/// refused rather than queued, so a retry can never apply a patch twice.
#[cfg(feature = "hotpatch")]
pub(crate) struct HotpatchLane {
    backend: Arc<dyn crate::DevtoolsBackend + Sync>,
    timeout: std::time::Duration,
    apply_in_flight: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "hotpatch")]
impl HotpatchLane {
    pub(crate) fn new(
        backend: Arc<dyn crate::DevtoolsBackend + Sync>,
        timeout: std::time::Duration,
    ) -> Self {
        Self {
            backend,
            timeout,
            apply_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    async fn info(&self) -> Result<frust_devtools_protocol::HotpatchInfo, RpcError> {
        self.run("hotpatch_info", self.timeout, None, |b| b.hotpatch_info())
            .await
    }

    async fn chunk(
        &self,
        chunk: frust_devtools_protocol::PatchChunkParams,
    ) -> Result<Vec<u8>, RpcError> {
        self.run("patch_chunk", self.timeout, None, move |b| {
            b.patch_chunk(&chunk)
        })
        .await
    }

    async fn apply(
        &self,
        bytes: Vec<u8>,
        params: frust_devtools_protocol::ApplyPatchParams,
    ) -> Result<frust_devtools_protocol::PatchOutcome, RpcError> {
        use std::sync::atomic::Ordering;
        if self
            .apply_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(RpcError::new(
                RpcError::INVALID_REQUEST,
                "a patch is already being applied; wait for its outcome",
            ));
        }
        let flag = Arc::clone(&self.apply_in_flight);
        let timeout = self.timeout.max(APPLY_TIMEOUT);
        self.run("apply_patch", timeout, Some(flag), move |b| {
            b.apply_patch(bytes, params)
        })
        .await
    }

    /// Runs `call` on a fresh worker thread and awaits it under `timeout`.
    /// `in_flight`, when given, is cleared by the worker once `call` returns
    /// (or panics) — not when the client stops waiting.
    async fn run<T: Send + 'static>(
        &self,
        name: &'static str,
        timeout: std::time::Duration,
        in_flight: Option<Arc<std::sync::atomic::AtomicBool>>,
        call: impl FnOnce(&dyn crate::DevtoolsBackend) -> Result<T, crate::BackendError>
        + Send
        + 'static,
    ) -> Result<T, RpcError> {
        /// Clears the in-flight flag on drop, panic included.
        struct Clear(Option<Arc<std::sync::atomic::AtomicBool>>);
        impl Drop for Clear {
            fn drop(&mut self) {
                if let Some(flag) = self.0.take() {
                    flag.store(false, std::sync::atomic::Ordering::Release);
                }
            }
        }

        let (reply, answer) = tokio::sync::oneshot::channel();
        let backend = Arc::clone(&self.backend);
        let clear = Clear(in_flight);
        let spawned = std::thread::Builder::new()
            .name("frust-devtools-hotpatch".to_string())
            .spawn(move || {
                let _clear = clear;
                let _ = reply.send(call(&*backend).map_err(|e| e.to_rpc_error()));
            });
        if let Err(e) = spawned {
            // The closure (and its `Clear`) was dropped with the failed spawn.
            log::error!("frust-devtools: failed to spawn the hot-patch worker: {e}");
            return Err(RpcError::new(
                RpcError::INTERNAL_ERROR,
                "could not start the hot-patch worker",
            ));
        }

        match tokio::time::timeout(timeout, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(RpcError::new(
                RpcError::INTERNAL_ERROR,
                format!("`{name}` failed: the hot-patch worker stopped without an answer"),
            )),
            Err(_) => {
                let ms = timeout.as_millis();
                log::warn!("frust-devtools: `{name}` timed out after {ms}ms");
                Err(RpcError::new(
                    RpcError::INTERNAL_ERROR,
                    format!(
                        "`{name}` timed out after {ms}ms — the app's UI thread is busy or blocked"
                    ),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_devtools_protocol::encode_line;

    /// The smallest backend that satisfies the trait — the auth gate is
    /// decided before any of these are reachable, so they only need to exist.
    struct StubBackend;

    impl crate::DevtoolsBackend for StubBackend {
        fn widget_tree(&self) -> frust_devtools_protocol::WidgetTreeDump {
            frust_devtools_protocol::WidgetTreeDump { roots: Vec::new() }
        }
        fn widget_props(&self, _id: u64) -> Option<frust_devtools_protocol::WidgetProps> {
            None
        }
        fn metrics_snapshot(&self) -> frust_devtools_protocol::MetricsSnapshot {
            frust_devtools_protocol::MetricsSnapshot {
                rss_bytes: None,
                uptime_ms: 0,
            }
        }
        fn inject_tap(
            &self,
            _p: frust_devtools_protocol::InputTapParams,
        ) -> Result<(), crate::BackendError> {
            Ok(())
        }
        fn inject_scroll(
            &self,
            _p: frust_devtools_protocol::InputScrollParams,
        ) -> Result<(), crate::BackendError> {
            Ok(())
        }
        fn inject_text(&self, _t: &str) -> Result<(), crate::BackendError> {
            Ok(())
        }
    }

    /// A [`SessionCtx`] over [`StubBackend`]: enough to drive `handle_request`
    /// end to end without a socket.
    fn ctx_with_token(token: Option<&str>) -> SessionCtx {
        let backend =
            crate::hop::spawn_backend_thread(StubBackend, 4, std::time::Duration::from_millis(500));
        SessionCtx {
            handshake: HandshakeInfo {
                app_name: "fake-app".to_string(),
                frust_version: "0.0.0".to_string(),
                protocol_version: frust_devtools_protocol::PROTOCOL_VERSION,
                capabilities: Vec::new(),
            },
            backend,
            bus: Arc::new(FrameStatsBus::new(4)),
            token: token.map(str::to_string),
            hot_patch: HotPatch::Unavailable(HotPatchUnavailable::NotOffered),
        }
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("current-thread runtime")
            .block_on(future)
    }

    fn error_code(response: &Response) -> i32 {
        match &response.outcome {
            frust_devtools_protocol::ResponseOutcome::Error { error } => error.code,
            other => panic!("expected an error outcome, got {other:?}"),
        }
    }

    fn is_success(response: &Response) -> bool {
        matches!(
            response.outcome,
            frust_devtools_protocol::ResponseOutcome::Success { .. }
        )
    }

    fn handshake_request(id: u64, token: Option<&str>) -> Request {
        Request::new(
            id,
            Method::Handshake.as_str(),
            serde_json::to_value(HandshakeParams {
                token: token.map(str::to_string),
            })
            .expect("params serialize"),
        )
    }

    #[test]
    fn an_unauthenticated_connection_is_refused_every_method_but_handshake() {
        let ctx = ctx_with_token(Some("s3cret"));
        let mut conn = ConnState::new(&ctx);
        for method in [
            Method::WidgetTree,
            Method::WidgetProps,
            Method::MetricsSnapshot,
            Method::FrameStatsSubscribe,
            Method::InputTap,
            Method::InputScroll,
            Method::InputText,
            Method::Screenshot,
            Method::FrameStats,
            Method::HotpatchInfo,
            Method::PatchChunk,
            Method::ApplyPatch,
        ] {
            let req = Request::new(1, method.as_str(), Value::Null);
            let (response, effect) = block_on(handle_request(&ctx, &mut conn, &req));
            assert_eq!(
                error_code(&response),
                RpcError::UNAUTHORIZED,
                "{method} must not be dispatched before handshake"
            );
            assert_eq!(
                effect,
                SideEffect::None,
                "{method} must have no side effect"
            );
        }
    }

    #[test]
    fn a_wrong_or_absent_token_leaves_the_connection_unauthenticated() {
        let ctx = ctx_with_token(Some("s3cret"));
        let mut conn = ConnState::new(&ctx);

        for params in [
            handshake_request(1, Some("not-the-token")),
            handshake_request(2, None),
            // Params that are not even a params object: still just "no token".
            Request::new(3, Method::Handshake.as_str(), Value::Null),
            Request::new(4, Method::Handshake.as_str(), serde_json::json!("nope")),
        ] {
            let (response, _) = block_on(handle_request(&ctx, &mut conn, &params));
            assert_eq!(error_code(&response), RpcError::UNAUTHORIZED);
        }

        let (after, _) = block_on(handle_request(
            &ctx,
            &mut conn,
            &Request::new(9, Method::MetricsSnapshot.as_str(), Value::Null),
        ));
        assert_eq!(error_code(&after), RpcError::UNAUTHORIZED);
    }

    #[test]
    fn the_right_token_authenticates_the_connection() {
        let ctx = ctx_with_token(Some("s3cret"));
        let mut conn = ConnState::new(&ctx);

        let (response, _) = block_on(handle_request(
            &ctx,
            &mut conn,
            &handshake_request(1, Some("s3cret")),
        ));
        assert!(is_success(&response));

        // The gate is open: a subsequent method now reaches the backend.
        let (next, _) = block_on(handle_request(
            &ctx,
            &mut conn,
            &Request::new(2, Method::MetricsSnapshot.as_str(), Value::Null),
        ));
        assert!(is_success(&next));
    }

    #[test]
    fn a_handshake_response_never_carries_the_token_back() {
        let ctx = ctx_with_token(Some("s3cret"));
        let mut conn = ConnState::new(&ctx);
        let (response, _) = block_on(handle_request(
            &ctx,
            &mut conn,
            &handshake_request(1, Some("s3cret")),
        ));
        assert!(!encode_line(&response).contains("s3cret"));

        // ...and neither does the rejection.
        let mut fresh = ConnState::new(&ctx);
        let (rejected, _) = block_on(handle_request(
            &ctx,
            &mut fresh,
            &handshake_request(2, Some("s3cret-ish")),
        ));
        let line = encode_line(&rejected);
        assert!(!line.contains("s3cret"));
    }

    #[test]
    fn auth_off_serves_every_method_without_a_handshake() {
        let ctx = ctx_with_token(None);
        let mut conn = ConnState::new(&ctx);
        let (response, effect) = block_on(handle_request(
            &ctx,
            &mut conn,
            &Request::new(1, Method::FrameStatsSubscribe.as_str(), Value::Null),
        ));
        assert!(is_success(&response));
        assert_eq!(effect, SideEffect::SubscribeFrameStats);

        // A token presented to an auth-off server is simply ignored, never an
        // error: a client that read one from an older line still connects.
        let (handshake, _) = block_on(handle_request(
            &ctx,
            &mut conn,
            &handshake_request(2, Some("whatever")),
        ));
        assert!(is_success(&handshake));
    }

    #[test]
    fn one_connection_authenticating_does_not_authenticate_another() {
        let ctx = ctx_with_token(Some("s3cret"));
        let mut first = ConnState::new(&ctx);
        let mut second = ConnState::new(&ctx);

        let (ok, _) = block_on(handle_request(
            &ctx,
            &mut first,
            &handshake_request(1, Some("s3cret")),
        ));
        assert!(is_success(&ok));

        let (denied, _) = block_on(handle_request(
            &ctx,
            &mut second,
            &Request::new(1, Method::MetricsSnapshot.as_str(), Value::Null),
        ));
        assert_eq!(error_code(&denied), RpcError::UNAUTHORIZED);
    }

    #[test]
    fn a_request_line_decodes_to_a_request() {
        let line = encode_line(&Request::new(1, "handshake", Value::Null));
        match decode(&line) {
            Decoded::Request(req) => assert_eq!(req.method, "handshake"),
            other => panic!("expected a Request, got {other:?}"),
        }
    }

    #[test]
    fn a_client_sent_response_is_ignored_not_answered() {
        let line = encode_line(&Response::success(1, Value::Null));
        assert!(matches!(decode(&line), Decoded::Ignore(_)));
    }

    #[test]
    fn a_client_sent_notification_is_ignored_not_answered() {
        let line = encode_line(&Notification::new("frame_stats", Value::Null));
        assert!(matches!(decode(&line), Decoded::Ignore(_)));
    }

    #[test]
    fn a_broken_line_with_a_usable_id_is_answerable() {
        // `method` is not a string, so the envelope cannot be built — but the
        // id is right there, so the client can be told.
        match decode(r#"{"id":7,"method":42}"#) {
            Decoded::Malformed { id, .. } => assert_eq!(id, 7),
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn garbage_with_no_id_can_only_be_skipped() {
        assert!(matches!(
            decode("this is not json"),
            Decoded::Undeliverable(_)
        ));
        assert!(matches!(decode("[1,2,3]"), Decoded::Undeliverable(_)));
        assert!(matches!(
            decode(r#"{"jsonrpc":"2.0"}"#),
            Decoded::Undeliverable(_)
        ));
    }

    #[test]
    fn a_non_integer_id_is_not_recoverable() {
        // JSON-RPC allows a string id; this protocol's envelope is u64-only,
        // so there is no id to answer with.
        assert!(matches!(
            decode(r#"{"id":"abc","method":42}"#),
            Decoded::Undeliverable(_)
        ));
    }

    #[test]
    fn parse_error_response_carries_the_standard_code() {
        let resp = parse_error_response(3, "bad");
        match resp.outcome {
            frust_devtools_protocol::ResponseOutcome::Error { error } => {
                assert_eq!(error.code, RpcError::PARSE_ERROR);
                assert_eq!(resp.id, 3);
            }
            other => panic!("expected an error outcome, got {other:?}"),
        }
    }

    #[test]
    fn frame_stats_notification_uses_the_protocol_method_name() {
        let stats = FrameStats {
            n: 3,
            total_us: 1,
            rebuild_us: 1,
            layout_us: 1,
            paint_us: 1,
            encode_us: 1,
            acquire_us: 1,
            submit_us: 1,
            skipped: false,
        };
        let notif = frame_stats_notification(stats);
        assert_eq!(notif.method, Method::FrameStats.as_str());
        assert_eq!(notif.params["n"], 3);
    }

    /// Authenticates `conn` against [`ctx_with_token`]'s `"s3cret"`.
    fn authenticate_conn(ctx: &SessionCtx, conn: &mut ConnState) {
        let (response, _) = block_on(handle_request(
            ctx,
            conn,
            &handshake_request(0, Some("s3cret")),
        ));
        assert!(is_success(&response));
    }

    fn error_message(response: &Response) -> String {
        match &response.outcome {
            frust_devtools_protocol::ResponseOutcome::Error { error } => error.message.clone(),
            other => panic!("expected an error outcome, got {other:?}"),
        }
    }

    #[test]
    fn while_the_capability_is_absent_the_patch_methods_are_unknown_methods() {
        // Every build: `ctx_with_token` carries no HotPatch capability, so the
        // two patch methods answer exactly as an unknown method does, and
        // `hotpatch_info` says which precondition failed.
        let ctx = ctx_with_token(Some("s3cret"));
        let mut conn = ConnState::new(&ctx);
        authenticate_conn(&ctx, &mut conn);

        let unknown = Request::new(1, "no_such_method", Value::Null);
        let (unknown, _) = block_on(handle_request(&ctx, &mut conn, &unknown));
        for method in [Method::PatchChunk, Method::ApplyPatch] {
            let req = Request::new(1, method.as_str(), Value::Null);
            let (response, effect) = block_on(handle_request(&ctx, &mut conn, &req));
            assert_eq!(
                error_code(&response),
                RpcError::METHOD_NOT_FOUND,
                "{method}"
            );
            assert_eq!(
                error_message(&response),
                error_message(&unknown).replace("no_such_method", method.as_str()),
                "{method} must be indistinguishable from an unknown method"
            );
            assert_eq!(effect, SideEffect::None);
        }

        let req = Request::new(2, Method::HotpatchInfo.as_str(), Value::Null);
        let (info, _) = block_on(handle_request(&ctx, &mut conn, &req));
        assert_eq!(error_code(&info), RpcError::NOT_SUPPORTED);
        assert!(
            error_message(&info).contains(&HotPatchUnavailable::NotOffered.to_string()),
            "hotpatch_info names the failed precondition: {}",
            error_message(&info)
        );
    }

    #[cfg(not(feature = "hotpatch"))]
    #[test]
    fn without_the_feature_hotpatch_info_says_so() {
        let mut ctx = ctx_with_token(Some("s3cret"));
        ctx.hot_patch = HotPatch::Unavailable(HotPatchUnavailable::FeatureOff);
        let mut conn = ConnState::new(&ctx);
        authenticate_conn(&ctx, &mut conn);
        let req = Request::new(1, Method::HotpatchInfo.as_str(), Value::Null);
        let (response, _) = block_on(handle_request(&ctx, &mut conn, &req));
        assert_eq!(error_code(&response), RpcError::NOT_SUPPORTED);
        assert!(error_message(&response).contains("`hotpatch` feature"));
    }

    #[cfg(feature = "hotpatch")]
    mod hot {
        use super::*;
        use frust_devtools_protocol::{
            ApplyPatchParams, HotpatchInfo, JumpTableWire, PatchChunkParams, PatchOutcome,
        };
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// Records what reached it. Its "transfer encoding" is the identity:
        /// a chunk's `data_base64` text *is* its bytes, so a test controls
        /// decoded lengths exactly.
        #[derive(Default)]
        struct HotStub {
            chunks_decoded: AtomicUsize,
            applied: Mutex<Vec<Vec<u8>>>,
            /// When set, `apply_patch` parks until a value arrives.
            hold: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
        }

        impl crate::DevtoolsBackend for Arc<HotStub> {
            fn widget_tree(&self) -> frust_devtools_protocol::WidgetTreeDump {
                frust_devtools_protocol::WidgetTreeDump { roots: Vec::new() }
            }
            fn widget_props(&self, _id: u64) -> Option<frust_devtools_protocol::WidgetProps> {
                None
            }
            fn metrics_snapshot(&self) -> frust_devtools_protocol::MetricsSnapshot {
                frust_devtools_protocol::MetricsSnapshot {
                    rss_bytes: None,
                    uptime_ms: 0,
                }
            }
            fn inject_tap(&self, _p: InputTapParams) -> Result<(), crate::BackendError> {
                Ok(())
            }
            fn inject_scroll(&self, _p: InputScrollParams) -> Result<(), crate::BackendError> {
                Ok(())
            }
            fn inject_text(&self, _t: &str) -> Result<(), crate::BackendError> {
                Ok(())
            }
            fn hotpatch_info(&self) -> Result<HotpatchInfo, crate::BackendError> {
                Ok(HotpatchInfo {
                    anchor_runtime: 1,
                    pid: 2,
                    triple: "test".to_string(),
                    patches_applied: 0,
                    patch_bytes_loaded: 0,
                    pending_layout_mismatches: Vec::new(),
                })
            }
            fn patch_chunk(
                &self,
                chunk: &PatchChunkParams,
            ) -> Result<Vec<u8>, crate::BackendError> {
                self.chunks_decoded.fetch_add(1, Ordering::SeqCst);
                Ok(chunk.data_base64.as_bytes().to_vec())
            }
            fn apply_patch(
                &self,
                bytes: Vec<u8>,
                _params: ApplyPatchParams,
            ) -> Result<PatchOutcome, crate::BackendError> {
                let hold = self.hold.lock().expect("hold").take();
                if let Some(hold) = hold {
                    let _ = hold.recv_timeout(std::time::Duration::from_secs(5));
                }
                let len = bytes.len() as u64;
                self.applied.lock().expect("applied").push(bytes);
                Ok(PatchOutcome {
                    applied: true,
                    seam_hits: 1,
                    seam_fall_throughs: Vec::new(),
                    layout_mismatches: Vec::new(),
                    patches_applied: 1,
                    patch_bytes_loaded: len,
                })
            }
        }

        fn enabled_ctx(stub: &Arc<HotStub>) -> SessionCtx {
            let mut ctx = ctx_with_token(Some("s3cret"));
            let shared = crate::service::SharedBackend::new(Arc::clone(stub));
            ctx.hot_patch = HotPatch::Enabled(HotpatchLane::new(
                Arc::new(shared),
                std::time::Duration::from_secs(2),
            ));
            ctx
        }

        fn chunk(patch_id: u64, offset: u64, total_len: u64, data: &str) -> Request {
            Request::new(
                10,
                Method::PatchChunk.as_str(),
                serde_json::to_value(PatchChunkParams {
                    patch_id,
                    offset,
                    total_len,
                    data_base64: data.to_string(),
                })
                .expect("chunk params"),
            )
        }

        fn apply(patch_id: u64, len: u64) -> Request {
            Request::new(
                11,
                Method::ApplyPatch.as_str(),
                serde_json::to_value(ApplyPatchParams {
                    patch_id,
                    len,
                    pid: 1,
                    anchor_runtime: 1,
                    table: JumpTableWire {
                        map: std::collections::HashMap::new(),
                        aslr_reference: 0,
                        new_base_address: 0,
                        ifunc_count: 0,
                    },
                    expected_seams: 0,
                })
                .expect("apply params"),
            )
        }

        fn send(ctx: &SessionCtx, conn: &mut ConnState, req: &Request) -> Response {
            block_on(handle_request(ctx, conn, req)).0
        }

        fn authed(ctx: &SessionCtx) -> ConnState {
            let mut conn = ConnState::new(ctx);
            authenticate_conn(ctx, &mut conn);
            conn
        }

        #[test]
        fn chunks_reassemble_in_order_and_apply_with_exactly_those_bytes() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);

            assert!(is_success(&send(&ctx, &mut conn, &chunk(7, 0, 6, "abc"))));
            assert!(is_success(&send(&ctx, &mut conn, &chunk(7, 3, 6, "def"))));
            let outcome = send(&ctx, &mut conn, &apply(7, 6));
            assert!(is_success(&outcome), "{outcome:?}");
            assert_eq!(
                *stub.applied.lock().expect("applied"),
                vec![b"abcdef".to_vec()]
            );

            // Consumed: a second apply of the same id has nothing to apply.
            let again = send(&ctx, &mut conn, &apply(7, 6));
            assert_eq!(error_code(&again), RpcError::INVALID_PARAMS);
        }

        #[test]
        fn a_patch_id_whose_chunks_arrived_on_another_connection_is_refused() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut sender = authed(&ctx);
            let mut other = authed(&ctx);

            assert!(is_success(&send(&ctx, &mut sender, &chunk(3, 0, 3, "abc"))));
            let refused = send(&ctx, &mut other, &apply(3, 3));
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
            assert!(error_message(&refused).contains("no chunks on this connection"));
            assert!(stub.applied.lock().expect("applied").is_empty());
        }

        #[test]
        fn a_length_mismatch_is_refused_and_discards_the_patch() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);

            assert!(is_success(&send(&ctx, &mut conn, &chunk(4, 0, 3, "abc"))));
            let refused = send(&ctx, &mut conn, &apply(4, 4));
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
            assert!(stub.applied.lock().expect("applied").is_empty());
            // The mismatched bytes are not kept for a retry.
            let retry = send(&ctx, &mut conn, &apply(4, 3));
            assert_eq!(error_code(&retry), RpcError::INVALID_PARAMS);
        }

        #[test]
        fn an_incomplete_patch_is_refused_and_kept() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);

            assert!(is_success(&send(&ctx, &mut conn, &chunk(5, 0, 6, "abc"))));
            let early = send(&ctx, &mut conn, &apply(5, 6));
            assert_eq!(error_code(&early), RpcError::INVALID_PARAMS);
            assert!(is_success(&send(&ctx, &mut conn, &chunk(5, 3, 6, "def"))));
            assert!(is_success(&send(&ctx, &mut conn, &apply(5, 6))));
        }

        #[test]
        fn an_over_cap_chunk_is_refused_before_it_is_decoded() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);

            let too_long = "A".repeat(MAX_CHUNK_BASE64 + 4);
            let refused = send(&ctx, &mut conn, &chunk(6, 0, MAX_PATCH_BYTES, &too_long));
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
            assert_eq!(stub.chunks_decoded.load(Ordering::SeqCst), 0);

            // A decoded chunk past the raw cap is refused too (the stub's
            // identity encoding lets one through `admit`'s text bound).
            let raw = "B".repeat(MAX_CHUNK_BYTES + 1);
            let refused = send(&ctx, &mut conn, &chunk(6, 0, MAX_PATCH_BYTES, &raw));
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);

            // ...and so is a patch claiming more than the per-patch cap.
            let refused = send(&ctx, &mut conn, &chunk(6, 0, MAX_PATCH_BYTES + 1, "abc"));
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
        }

        #[test]
        fn out_of_order_and_inconsistent_chunks_are_refused() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);

            // A first chunk not at offset 0.
            let refused = send(&ctx, &mut conn, &chunk(8, 3, 6, "abc"));
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
            assert!(is_success(&send(&ctx, &mut conn, &chunk(8, 0, 6, "abc"))));
            // A gap, an overlap, a changed total, a chunk past the end.
            for bad in [
                chunk(8, 4, 6, "def"),
                chunk(8, 2, 6, "def"),
                chunk(8, 3, 7, "def"),
                chunk(8, 3, 6, "defg"),
            ] {
                let refused = send(&ctx, &mut conn, &bad);
                assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS, "{bad:?}");
            }
            assert!(is_success(&send(&ctx, &mut conn, &chunk(8, 3, 6, "def"))));
            assert!(is_success(&send(&ctx, &mut conn, &apply(8, 6))));
        }

        #[test]
        fn a_path_field_on_apply_patch_is_rejected_at_decode() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);
            assert!(is_success(&send(&ctx, &mut conn, &chunk(9, 0, 3, "abc"))));
            let mut req = apply(9, 3);
            req.params["table"]["lib"] = serde_json::json!("/tmp/evil.dylib");
            let refused = send(&ctx, &mut conn, &req);
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
            assert!(stub.applied.lock().expect("applied").is_empty());
        }

        #[test]
        fn hotpatch_info_reaches_the_backend_when_enabled() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);
            let req = Request::new(1, Method::HotpatchInfo.as_str(), Value::Null);
            let response = send(&ctx, &mut conn, &req);
            assert!(is_success(&response), "{response:?}");
        }

        #[test]
        fn a_second_apply_while_one_is_in_flight_is_refused() {
            let stub = Arc::new(HotStub::default());
            let (release, hold) = std::sync::mpsc::channel();
            *stub.hold.lock().expect("hold") = Some(hold);
            let ctx = enabled_ctx(&stub);
            let mut first = authed(&ctx);
            let mut second = authed(&ctx);
            assert!(is_success(&send(&ctx, &mut first, &chunk(1, 0, 3, "abc"))));
            assert!(is_success(&send(&ctx, &mut second, &chunk(2, 0, 3, "xyz"))));

            let first_apply = apply(1, 3);
            let (a, b) = block_on(async {
                let a = handle_request(&ctx, &mut first, &first_apply);
                let b = async {
                    // Let the first apply reach its (parked) worker.
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let response = handle_request(&ctx, &mut second, &apply(2, 3)).await;
                    let _ = release.send(());
                    response
                };
                tokio::join!(a, b)
            });
            assert!(is_success(&a.0), "{:?}", a.0);
            assert_eq!(error_code(&b.0), RpcError::INVALID_REQUEST);
            assert_eq!(
                *stub.applied.lock().expect("applied"),
                vec![b"abc".to_vec()]
            );
        }
    }
}
