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
    /// The `table_chunk` stream: a patch's encoded jump table, reassembled
    /// apart from its bytes under the same rules.
    #[cfg(feature = "hotpatch")]
    tables: PatchAssembly,
}

impl ConnState {
    /// A fresh connection: unauthenticated when the service requires a token,
    /// and pre-authenticated when it does not (so an auth-off service behaves
    /// exactly as it did before auth existed).
    pub(crate) fn new(ctx: &SessionCtx) -> Self {
        Self {
            authenticated: ctx.token.is_none(),
            #[cfg(feature = "hotpatch")]
            patches: PatchAssembly::patches(),
            #[cfg(feature = "hotpatch")]
            tables: PatchAssembly::tables(),
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

/// The reply to a request line longer than the line cap (`cap` bytes), sent
/// just before the connection closes so the client learns why instead of
/// seeing a bare close. `id` is the one [`salvage_id`] recovered from the
/// line's start; without one the reply goes to id `0`, which no client call
/// uses, so a client can still read the reason off it.
pub(crate) fn oversized_line_response(id: Option<u64>, cap: usize) -> Response {
    Response::error(
        id.unwrap_or(0),
        RpcError::new(
            RpcError::INVALID_REQUEST,
            format!(
                "request line exceeds the devtools cap of {cap} bytes; the connection is closed \
                 (large payloads travel as chunks)"
            ),
        ),
    )
}

/// The top-level integer `id` of a JSON object of which `prefix` is only the
/// start (a line cut at the cap): the key `"id"` at nesting depth 1, followed
/// by an unsigned integer. A key of that name inside a nested value is not
/// it; anything that is not that shape answers `None`.
pub(crate) fn salvage_id(prefix: &[u8]) -> Option<u64> {
    let skip_ws = |bytes: &[u8], mut i: usize| {
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        i
    };
    let mut i = skip_ws(prefix, 0);
    if prefix.get(i) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    while i < prefix.len() {
        match prefix[i] {
            b'"' => {
                let start = i + 1;
                i = start;
                while i < prefix.len() && prefix[i] != b'"' {
                    i += if prefix[i] == b'\\' { 2 } else { 1 };
                }
                if i >= prefix.len() {
                    return None;
                }
                let text = &prefix[start..i];
                i = skip_ws(prefix, i + 1);
                if depth == 1 && text == b"id" && prefix.get(i) == Some(&b':') {
                    let from = skip_ws(prefix, i + 1);
                    let to = from
                        + prefix[from..]
                            .iter()
                            .take_while(|b| b.is_ascii_digit())
                            .count();
                    // A fraction, exponent or cut-off number is no integer id;
                    // any JSON whitespace may sit between it and its `,`/`}`.
                    if to == from || !matches!(prefix.get(skip_ws(prefix, to)), Some(b',' | b'}')) {
                        return None;
                    }
                    return std::str::from_utf8(&prefix[from..to]).ok()?.parse().ok();
                }
                continue;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    None
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
        Method::HotpatchInfo | Method::PatchChunk | Method::TableChunk | Method::ApplyPatch => {
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
/// unknown here, and the reassembled length must equal `len`. An
/// `apply_patch` naming a file (the loopback hand-off) instead gets its bytes
/// from the backend's checked read (`DevtoolsBackend::patch_file`); a
/// `patch_id` both uploaded and named by file is refused.
///
/// `table_chunk` fills the connection's second reassembly, the jump table's
/// encoded map, on either path. `apply_patch` checks it is complete and
/// `table_len` bytes long before taking the patch bytes, then decodes it
/// (bounded by [`MAX_TABLE_BYTES`]) into the params the backend receives, so
/// the backend sees the whole table exactly as a single line once carried it.
/// With `table_len` 0 the params' own map (empty, or a pre-chunk host's
/// inline one) is applied as decoded.
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
        Method::PatchChunk | Method::TableChunk => {
            let handling = std::time::Instant::now();
            let chunk = match serde_json::from_value::<frust_devtools_protocol::PatchChunkParams>(
                req.params.clone(),
            ) {
                Ok(chunk) => chunk,
                Err(e) => return Response::error(id, invalid_params(req, &e)),
            };
            let assembly = if method == Method::TableChunk {
                &mut conn.tables
            } else {
                &mut conn.patches
            };
            // Shape checks before the backend decodes anything: an over-cap
            // chunk is refused without being decoded or buffered.
            if let Err(e) = assembly.admit(&chunk) {
                return Response::error(id, e);
            }
            let bytes = match lane.chunk(chunk.clone()).await {
                Ok(bytes) => bytes,
                Err(e) => return Response::error(id, e),
            };
            match assembly.append(&chunk, bytes) {
                Ok(()) => {
                    assembly.handled(handling);
                    result_response(id, serde_json::to_value(AckResult::default()))
                }
                Err(e) => Response::error(id, e),
            }
        }
        Method::ApplyPatch => {
            // `deny_unknown_fields`: a `lib`/`path` field fails here.
            let mut params = match serde_json::from_value::<frust_devtools_protocol::ApplyPatchParams>(
                req.params.clone(),
            ) {
                Ok(params) => params,
                Err(e) => return Response::error(id, invalid_params(req, &e)),
            };
            if let Err(e) = check_table_len(params.table_len) {
                return Response::error(id, e);
            }
            // A pre-chunk host's inline map stands in for a stream only when
            // no stream is declared.
            if params.table_len > 0 && !params.table.map.is_empty() {
                return Response::error(
                    id,
                    invalid(format!(
                        "patch {} declares a streamed jump table and an inline map",
                        params.patch_id
                    )),
                );
            }
            // The table must be complete before any patch byte is taken or
            // read, so a missing table leaves an uploaded patch in place.
            if params.table_len > 0
                && let Err(e) = conn.tables.ready(params.patch_id, params.table_len)
            {
                return Response::error(id, e);
            }
            let patch_transfer = conn.patches.transfer(params.patch_id);
            let table_transfer = conn.tables.transfer(params.patch_id);
            let bytes = match params.file.clone() {
                None => match conn.patches.take_complete(params.patch_id, params.len) {
                    Ok(bytes) => bytes,
                    Err(e) => return Response::error(id, e),
                },
                Some(file) => {
                    if conn.patches.holds(params.patch_id) {
                        return Response::error(
                            id,
                            RpcError::new(
                                RpcError::INVALID_REQUEST,
                                format!(
                                    "patch {} was both uploaded and named by file",
                                    params.patch_id
                                ),
                            ),
                        );
                    }
                    if params.len == 0 || params.len > MAX_PATCH_BYTES {
                        return Response::error(
                            id,
                            invalid(format!("patch len must be 1..={MAX_PATCH_BYTES} bytes")),
                        );
                    }
                    match lane.file(file, params.len).await {
                        Ok(bytes) if bytes.len() as u64 == params.len => bytes,
                        Ok(bytes) => {
                            return Response::error(
                                id,
                                invalid(format!(
                                    "patch {} is {} bytes, apply_patch says {}",
                                    params.patch_id,
                                    bytes.len(),
                                    params.len
                                )),
                            );
                        }
                        Err(e) => return Response::error(id, e),
                    }
                }
            };
            let table_decoding = std::time::Instant::now();
            if params.table_len == 0 {
                // No stream: the map is empty, or a pre-chunk host's inline one.
                conn.tables.discard(params.patch_id);
            } else {
                let encoded = match conn.tables.take_complete(params.patch_id, params.table_len) {
                    Ok(encoded) => encoded,
                    Err(e) => return Response::error(id, e),
                };
                params.table.map =
                    match frust_devtools_protocol::JumpTableWire::decode_map(&encoded) {
                        Ok(map) => map,
                        Err(e) => return Response::error(id, invalid(e)),
                    };
            }
            log::info!(
                "frust-hotpatch: transfer timings: patch={} table={} table_decode={}ms",
                Transfer::describe(patch_transfer),
                Transfer::describe(table_transfer),
                table_decoding.elapsed().as_millis()
            );
            match lane.apply(bytes, params).await {
                Ok(outcome) => result_response(id, serde_json::to_value(outcome)),
                Err(e) => Response::error(id, e),
            }
        }
        // Only the four hot-patch methods are routed here.
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

/// Largest encoded jump table a connection may assemble: the patch cap again.
/// At [`frust_devtools_protocol::JumpTableWire::ENTRY_BYTES`] per mapping that
/// is about four million symbols, several times the largest app measured, and
/// it bounds what one client can make this process buffer and decode.
#[cfg(feature = "hotpatch")]
pub(crate) const MAX_TABLE_BYTES: u64 = MAX_PATCH_BYTES;

/// `apply_patch`'s `table_len` shape: whole entries, within
/// [`MAX_TABLE_BYTES`].
#[cfg(feature = "hotpatch")]
fn check_table_len(table_len: u64) -> Result<(), RpcError> {
    let entry = frust_devtools_protocol::JumpTableWire::ENTRY_BYTES as u64;
    if table_len > MAX_TABLE_BYTES {
        return Err(invalid(format!(
            "jump table of {table_len} bytes exceeds the {MAX_TABLE_BYTES}-byte cap"
        )));
    }
    if !table_len.is_multiple_of(entry) {
        return Err(invalid(format!(
            "jump table of {table_len} bytes is not a whole number of {entry}-byte entries"
        )));
    }
    Ok(())
}

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
    /// When its first chunk began to be handled, when its latest chunk
    /// finished, and how many were appended.
    started: std::time::Instant,
    last: std::time::Instant,
    chunks: u32,
    /// Time spent handling its chunks here (params, decode, append), as
    /// against the wall time the transfer took.
    handled: std::time::Duration,
}

/// How one stream's transfer went: the instrument behind the app's permanent
/// `frust-hotpatch: transfer timings` line, logged at `info` once per
/// `apply_patch` (`patch=<wall>ms/<chunks> chunks/<handled>ms handled`, the
/// same for the table, then the table's decode). The wall time from the
/// first chunk to the last, against the time spent handling chunks here,
/// tells a slow link or line reader from slow per-chunk work.
#[cfg(feature = "hotpatch")]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Transfer {
    pub(crate) wall: std::time::Duration,
    pub(crate) chunks: u32,
    pub(crate) handled: std::time::Duration,
}

#[cfg(feature = "hotpatch")]
impl Transfer {
    /// `<wall>ms/<n> chunks/<handled>ms handled`, or `none` with no chunk.
    fn describe(transfer: Option<Self>) -> String {
        match transfer {
            None => "none".to_string(),
            Some(t) => format!(
                "{}ms/{} chunks/{}ms handled",
                t.wall.as_millis(),
                t.chunks,
                t.handled.as_millis()
            ),
        }
    }
}

/// A connection's chunk reassembly of one stream (a patch's bytes, or its
/// encoded jump table): **one** transfer at a time, chunks in order and
/// contiguous (`offset` equal to the bytes received so far), every chunk
/// agreeing on `total_len`. A chunk at offset 0 for a new `patch_id` discards
/// any unfinished one, so an abandoned transfer never wedges the connection
/// and the buffer never holds more than one transfer.
#[cfg(feature = "hotpatch")]
pub(crate) struct PatchAssembly {
    /// What this stream is, for messages: `patch` or `table of patch`.
    what: &'static str,
    /// Largest `total_len` this stream accepts.
    cap: u64,
    pending: Option<PendingPatch>,
}

#[cfg(feature = "hotpatch")]
impl std::fmt::Debug for PatchAssembly {
    /// Lengths only, never the bytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("PatchAssembly");
        s.field("what", &self.what);
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
    /// The patch-bytes stream (`patch_chunk`), capped at [`MAX_PATCH_BYTES`].
    pub(crate) fn patches() -> Self {
        Self {
            what: "patch",
            cap: MAX_PATCH_BYTES,
            pending: None,
        }
    }

    /// The jump-table stream (`table_chunk`), capped at [`MAX_TABLE_BYTES`].
    pub(crate) fn tables() -> Self {
        Self {
            what: "table of patch",
            cap: MAX_TABLE_BYTES,
            pending: None,
        }
    }

    /// The checks that need no decoding: size caps, `total_len` agreement, and
    /// that `offset` continues this connection's transfer of `patch_id`.
    pub(crate) fn admit(
        &self,
        chunk: &frust_devtools_protocol::PatchChunkParams,
    ) -> Result<(), RpcError> {
        let (what, cap) = (self.what, self.cap);
        if chunk.data_base64.len() > MAX_CHUNK_BASE64 {
            return Err(invalid(format!(
                "{what} chunk exceeds {MAX_CHUNK_BYTES} raw bytes"
            )));
        }
        if chunk.data_base64.is_empty() {
            return Err(invalid(format!(
                "a {what} chunk must carry at least one byte"
            )));
        }
        if chunk.total_len == 0 || chunk.total_len > cap {
            return Err(invalid(format!("{what} total_len must be 1..={cap} bytes")));
        }
        match &self.pending {
            Some(p) if p.patch_id == chunk.patch_id => {
                if p.total_len != chunk.total_len {
                    return Err(invalid(format!(
                        "{what} {} changed total_len from {} to {}",
                        chunk.patch_id, p.total_len, chunk.total_len
                    )));
                }
                if chunk.offset != p.bytes.len() as u64 {
                    return Err(invalid(format!(
                        "{what} {} chunk at offset {}, expected {}",
                        chunk.patch_id,
                        chunk.offset,
                        p.bytes.len()
                    )));
                }
            }
            // A new transfer starts at offset 0 (and replaces any unfinished one).
            _ if chunk.offset != 0 => {
                return Err(invalid(format!(
                    "{what} {} has no chunks on this connection; its first chunk must be at offset 0",
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
        let what = self.what;
        if data.is_empty() || data.len() > MAX_CHUNK_BYTES {
            return Err(invalid(format!(
                "a decoded {what} chunk must be 1..={MAX_CHUNK_BYTES} bytes"
            )));
        }
        if chunk.offset + data.len() as u64 > chunk.total_len {
            return Err(invalid(format!(
                "{what} {} chunk runs past total_len {}",
                chunk.patch_id, chunk.total_len
            )));
        }
        let continuing = matches!(&self.pending, Some(p) if p.patch_id == chunk.patch_id);
        if !continuing {
            if let Some(dropped) = self.pending.take() {
                log::debug!(
                    "frust-devtools: {what} {} superseded by {what} {} before it completed",
                    dropped.patch_id,
                    chunk.patch_id
                );
            }
            self.pending = Some(PendingPatch {
                patch_id: chunk.patch_id,
                total_len: chunk.total_len,
                bytes: Vec::new(),
                started: std::time::Instant::now(),
                last: std::time::Instant::now(),
                chunks: 0,
                handled: std::time::Duration::ZERO,
            });
        }
        if let Some(p) = self.pending.as_mut() {
            p.bytes.extend_from_slice(&data);
            p.chunks += 1;
        }
        Ok(())
    }

    /// Adds the time since `since`, when the chunk just appended began to
    /// be handled, to the pending transfer's handling time ([`Transfer`]);
    /// the transfer's wall clock starts there for its first chunk.
    pub(crate) fn handled(&mut self, since: std::time::Instant) {
        if let Some(p) = self.pending.as_mut() {
            p.handled += since.elapsed();
            if p.chunks == 1 {
                p.started = since;
            }
            p.last = std::time::Instant::now();
        }
    }

    /// How `patch_id`'s transfer on this connection went so far, when it has
    /// one.
    pub(crate) fn transfer(&self, patch_id: u64) -> Option<Transfer> {
        self.pending
            .as_ref()
            .filter(|p| p.patch_id == patch_id)
            .map(|p| Transfer {
                // First chunk to last: idle time before `apply_patch` (or a
                // table upload after the patch) is not transfer time.
                wall: p.last.saturating_duration_since(p.started),
                chunks: p.chunks,
                handled: p.handled,
            })
    }

    /// Whether this connection holds chunks (complete or not) for `patch_id`.
    pub(crate) fn holds(&self, patch_id: u64) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| p.patch_id == patch_id)
    }

    /// Whether [`Self::take_complete`] would hand over `patch_id`'s bytes
    /// (assembled here, complete, exactly `len` bytes), without taking them;
    /// the error is the one `take_complete` would answer.
    pub(crate) fn ready(&self, patch_id: u64, len: u64) -> Result<(), RpcError> {
        let what = self.what;
        let Some(p) = self.pending.as_ref().filter(|p| p.patch_id == patch_id) else {
            return Err(invalid(format!(
                "{what} {patch_id} has no chunks on this connection"
            )));
        };
        if (p.bytes.len() as u64) < p.total_len {
            return Err(invalid(format!(
                "{what} {patch_id} is incomplete: {} of {} bytes received",
                p.bytes.len(),
                p.total_len
            )));
        }
        if p.bytes.len() as u64 != len {
            return Err(invalid(format!(
                "{what} {patch_id} is {} bytes, apply_patch says {len}",
                p.bytes.len()
            )));
        }
        Ok(())
    }

    /// Hands over `patch_id`'s bytes for `apply_patch`: it must have been
    /// assembled on **this** connection, be complete, and be exactly `len`
    /// bytes. A complete transfer is consumed whatever the answer (a length
    /// mismatch means its bytes cannot be trusted); an incomplete one stays.
    pub(crate) fn take_complete(&mut self, patch_id: u64, len: u64) -> Result<Vec<u8>, RpcError> {
        let verdict = self.ready(patch_id, len);
        let complete = self
            .pending
            .as_ref()
            .is_some_and(|p| p.patch_id == patch_id && p.bytes.len() as u64 >= p.total_len);
        let taken = if complete { self.pending.take() } else { None };
        match (verdict, taken) {
            (Ok(()), Some(p)) => Ok(p.bytes),
            (Err(e), _) => Err(e),
            // `ready` passing means a complete transfer was there to take.
            (Ok(()), None) => Err(invalid(format!(
                "{} {patch_id} has no chunks on this connection",
                self.what
            ))),
        }
    }

    /// Drops whatever this connection holds for `patch_id`: an `apply_patch`
    /// that declares no table leaves no table stream behind it.
    pub(crate) fn discard(&mut self, patch_id: u64) {
        self.pending.take_if(|p| p.patch_id == patch_id);
    }
}

/// The hot-patch lane: where the hot-patch backend calls run.
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

    /// Reads a patch named by file (the loopback hand-off) through the
    /// backend's checked [`crate::DevtoolsBackend::patch_file`].
    async fn file(
        &self,
        file: frust_devtools_protocol::PatchFile,
        len: u64,
    ) -> Result<Vec<u8>, RpcError> {
        self.run("apply_patch (file)", self.timeout, None, move |b| {
            b.patch_file(&file, len)
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
            Method::TableChunk,
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
    fn salvage_id_reads_only_the_top_level_integer_id() {
        assert_eq!(
            salvage_id(br#"{"jsonrpc":"2.0","id":42,"method":"apply_patch","params":{"#),
            Some(42)
        );
        assert_eq!(salvage_id(br#"  { "id" : 7 , "params": "xxx"#), Some(7));
        // An `id` inside a nested value is not the envelope's.
        assert_eq!(
            salvage_id(br#"{"method":"x","params":{"id":9,"text":"aaaa"#),
            None
        );
        assert_eq!(salvage_id(br#"{"params":["id",1],"id":3}"#), Some(3));
        // A key-looking string inside a string value is skipped whole.
        assert_eq!(salvage_id(br#"{"params":"\"id\":5","id":6,"#), Some(6));
        // JSON whitespace between the id and its delimiter.
        assert_eq!(salvage_id(b"{\"id\":7\n,\"method\":\"x\""), Some(7));
        assert_eq!(salvage_id(b"{\"id\":8\t}"), Some(8));
        // Not an unsigned integer, cut off, or not an object at all.
        for prefix in [
            &br#"{"id":"abc","#[..],
            br#"{"id":-1,"#,
            br#"{"id":1.5,"#,
            br#"{"id":12"#,
            br#"[{"id":1}]"#,
            b"xxxxxxxx",
        ] {
            assert_eq!(
                salvage_id(prefix),
                None,
                "{}",
                String::from_utf8_lossy(prefix)
            );
        }
    }

    #[test]
    fn an_oversized_line_reply_names_the_cap_and_falls_back_to_id_zero() {
        let reply = oversized_line_response(Some(5), 1 << 20);
        assert_eq!(reply.id, 5);
        assert_eq!(error_code(&reply), RpcError::INVALID_REQUEST);
        assert!(error_message(&reply).contains("1048576"), "{reply:?}");
        assert_eq!(oversized_line_response(None, 1 << 20).id, 0);
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
        for method in [Method::PatchChunk, Method::TableChunk, Method::ApplyPatch] {
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
            ApplyPatchParams, HotpatchInfo, JumpTableWire, PatchChunkParams, PatchFile,
            PatchOutcome,
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
            /// The jump-table map each `apply_patch` arrived with.
            tables: Mutex<Vec<std::collections::HashMap<u64, u64>>>,
            /// When set, `apply_patch` parks until a value arrives.
            hold: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
            /// What `patch_file` answers with; `None` refuses like a backend
            /// that does not accept files.
            file_bytes: Mutex<Option<Vec<u8>>>,
            /// Every `(path, len)` `patch_file` was asked for.
            files_read: Mutex<Vec<(String, u64)>>,
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
                    patch_file_hand_off: false,
                })
            }
            fn patch_chunk(
                &self,
                chunk: &PatchChunkParams,
            ) -> Result<Vec<u8>, crate::BackendError> {
                self.chunks_decoded.fetch_add(1, Ordering::SeqCst);
                Ok(chunk.data_base64.as_bytes().to_vec())
            }
            fn patch_file(
                &self,
                file: &PatchFile,
                len: u64,
            ) -> Result<Vec<u8>, crate::BackendError> {
                self.files_read
                    .lock()
                    .expect("files_read")
                    .push((file.path.clone(), len));
                self.file_bytes
                    .lock()
                    .expect("file_bytes")
                    .clone()
                    .ok_or_else(|| crate::BackendError::invalid_request("no file here"))
            }
            fn apply_patch(
                &self,
                bytes: Vec<u8>,
                params: ApplyPatchParams,
            ) -> Result<PatchOutcome, crate::BackendError> {
                self.tables.lock().expect("tables").push(params.table.map);
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
            enabled_ctx_with(Arc::clone(stub))
        }

        fn enabled_ctx_with(backend: impl crate::DevtoolsBackend) -> SessionCtx {
            let mut ctx = ctx_with_token(Some("s3cret"));
            let shared = crate::service::SharedBackend::new(backend);
            ctx.hot_patch = HotPatch::Enabled(HotpatchLane::new(
                Arc::new(shared),
                std::time::Duration::from_secs(2),
            ));
            ctx
        }

        /// An `apply_patch` naming `/tmp/patch-<id>.so` as the hand-off file.
        fn apply_file(patch_id: u64, len: u64) -> Request {
            let mut req = apply(patch_id, len);
            req.params["file"] = serde_json::to_value(PatchFile {
                path: format!("/tmp/patch-{patch_id}.so"),
                sha256: "0".repeat(64),
            })
            .expect("file");
            req
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
            apply_with_table(patch_id, len, 0)
        }

        /// An `apply_patch` whose table is `table_len` bytes of `table_chunk`s.
        fn apply_with_table(patch_id: u64, len: u64, table_len: u64) -> Request {
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
                    table_len,
                    expected_seams: 0,
                    file: None,
                })
                .expect("apply params"),
            )
        }

        /// A `table_chunk` whose identity-encoded text is `data`.
        fn table_chunk(patch_id: u64, offset: u64, total_len: u64, data: &str) -> Request {
            let mut req = chunk(patch_id, offset, total_len, data);
            req.method = Method::TableChunk.as_str().to_string();
            req
        }

        /// A map whose encoding is plain ASCII, so [`HotStub`]'s identity
        /// transfer encoding carries it, with that encoding as text.
        fn ascii_table() -> (std::collections::HashMap<u64, u64>, String) {
            let map: std::collections::HashMap<u64, u64> =
                [(0x10, 0x20), (0x30, 0x40)].into_iter().collect();
            let wire = JumpTableWire {
                map: map.clone(),
                aslr_reference: 0,
                new_base_address: 0,
                ifunc_count: 0,
            };
            let text = String::from_utf8(wire.encode_map()).expect("ascii encoding");
            (map, text)
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
        fn a_transfer_counts_its_chunks_and_handling_until_it_is_applied() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);
            assert_eq!(conn.patches.transfer(7), None);

            assert!(is_success(&send(&ctx, &mut conn, &chunk(7, 0, 6, "abc"))));
            assert!(is_success(&send(&ctx, &mut conn, &chunk(7, 3, 6, "def"))));
            let transfer = conn
                .patches
                .transfer(7)
                .expect("patch 7 is being assembled");
            assert_eq!(transfer.chunks, 2);
            assert!(transfer.wall >= transfer.handled, "{transfer:?}");
            assert_eq!(conn.patches.transfer(8), None, "another id has none");
            assert_eq!(conn.tables.transfer(7), None, "the table stream is apart");
            assert_eq!(Transfer::describe(None), "none");

            assert!(is_success(&send(&ctx, &mut conn, &apply(7, 6))));
            assert_eq!(conn.patches.transfer(7), None, "consumed by the apply");
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
            // The chunk path never asks the backend for a file.
            assert!(stub.files_read.lock().expect("files_read").is_empty());
        }

        #[test]
        fn a_file_hand_off_applies_exactly_the_bytes_the_backend_read() {
            let stub = Arc::new(HotStub::default());
            *stub.file_bytes.lock().expect("file_bytes") = Some(b"filed!".to_vec());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);

            let outcome = send(&ctx, &mut conn, &apply_file(12, 6));
            assert!(is_success(&outcome), "{outcome:?}");
            assert_eq!(
                *stub.files_read.lock().expect("files_read"),
                vec![("/tmp/patch-12.so".to_string(), 6)]
            );
            assert_eq!(
                *stub.applied.lock().expect("applied"),
                vec![b"filed!".to_vec()]
            );
            assert_eq!(stub.chunks_decoded.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn a_file_read_of_the_wrong_length_is_refused_before_the_apply() {
            let stub = Arc::new(HotStub::default());
            *stub.file_bytes.lock().expect("file_bytes") = Some(b"short".to_vec());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);
            let refused = send(&ctx, &mut conn, &apply_file(14, 6));
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
            assert!(stub.applied.lock().expect("applied").is_empty());
        }

        #[test]
        fn a_patch_both_uploaded_and_named_by_file_is_refused() {
            let stub = Arc::new(HotStub::default());
            *stub.file_bytes.lock().expect("file_bytes") = Some(b"abc".to_vec());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);

            assert!(is_success(&send(&ctx, &mut conn, &chunk(13, 0, 3, "abc"))));
            let refused = send(&ctx, &mut conn, &apply_file(13, 3));
            assert_eq!(error_code(&refused), RpcError::INVALID_REQUEST);
            assert!(
                error_message(&refused).contains("both uploaded and named by file"),
                "{refused:?}"
            );
            assert!(stub.files_read.lock().expect("files_read").is_empty());
            assert!(stub.applied.lock().expect("applied").is_empty());
        }

        #[test]
        fn a_file_hand_off_to_a_refusing_backend_is_answered_as_its_error() {
            // `StubBackend` keeps the trait's default `patch_file`.
            let ctx = enabled_ctx_with(StubBackend);
            let mut conn = authed(&ctx);
            let refused = send(&ctx, &mut conn, &apply_file(15, 3));
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
            assert!(
                error_message(&refused).contains("does not accept patch files"),
                "{refused:?}"
            );
        }

        #[test]
        fn a_file_hand_off_outside_the_patch_size_cap_is_refused_unread() {
            let stub = Arc::new(HotStub::default());
            *stub.file_bytes.lock().expect("file_bytes") = Some(Vec::new());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);
            for len in [0, MAX_PATCH_BYTES + 1] {
                let refused = send(&ctx, &mut conn, &apply_file(16, len));
                assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS, "{len}");
            }
            assert!(stub.files_read.lock().expect("files_read").is_empty());
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
        fn a_table_streamed_in_chunks_reaches_the_backend_whole() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);
            let (map, text) = ascii_table();

            assert!(is_success(&send(&ctx, &mut conn, &chunk(7, 0, 3, "abc"))));
            assert!(is_success(&send(
                &ctx,
                &mut conn,
                &table_chunk(7, 0, 32, &text[..16])
            )));
            assert!(is_success(&send(
                &ctx,
                &mut conn,
                &table_chunk(7, 16, 32, &text[16..])
            )));
            let outcome = send(&ctx, &mut conn, &apply_with_table(7, 3, 32));
            assert!(is_success(&outcome), "{outcome:?}");
            assert_eq!(
                *stub.applied.lock().expect("applied"),
                vec![b"abc".to_vec()]
            );
            assert_eq!(*stub.tables.lock().expect("tables"), vec![map]);
        }

        #[test]
        fn an_apply_missing_its_table_is_refused_and_keeps_the_patch() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);
            let (map, text) = ascii_table();

            assert!(is_success(&send(&ctx, &mut conn, &chunk(8, 0, 3, "abc"))));
            let refused = send(&ctx, &mut conn, &apply_with_table(8, 3, 32));
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
            assert!(
                error_message(&refused).contains("table of patch 8 has no chunks"),
                "{refused:?}"
            );
            // An incomplete table is refused the same way, and kept.
            assert!(is_success(&send(
                &ctx,
                &mut conn,
                &table_chunk(8, 0, 32, &text[..16])
            )));
            let early = send(&ctx, &mut conn, &apply_with_table(8, 3, 32));
            assert!(error_message(&early).contains("incomplete"), "{early:?}");
            assert!(stub.applied.lock().expect("applied").is_empty());

            assert!(is_success(&send(
                &ctx,
                &mut conn,
                &table_chunk(8, 16, 32, &text[16..])
            )));
            let outcome = send(&ctx, &mut conn, &apply_with_table(8, 3, 32));
            assert!(is_success(&outcome), "{outcome:?}");
            assert_eq!(*stub.tables.lock().expect("tables"), vec![map]);
        }

        #[test]
        fn a_file_hand_off_takes_its_table_from_table_chunks() {
            let stub = Arc::new(HotStub::default());
            *stub.file_bytes.lock().expect("file_bytes") = Some(b"filed!".to_vec());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);
            let (map, text) = ascii_table();

            assert!(is_success(&send(
                &ctx,
                &mut conn,
                &table_chunk(12, 0, 32, &text)
            )));
            let mut req = apply_file(12, 6);
            req.params["table_len"] = serde_json::json!(32);
            let outcome = send(&ctx, &mut conn, &req);
            assert!(is_success(&outcome), "{outcome:?}");
            assert_eq!(
                *stub.applied.lock().expect("applied"),
                vec![b"filed!".to_vec()]
            );
            assert_eq!(*stub.tables.lock().expect("tables"), vec![map]);
        }

        #[test]
        fn a_table_len_off_the_entry_grid_or_over_the_cap_is_refused() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);
            assert!(is_success(&send(&ctx, &mut conn, &chunk(9, 0, 3, "abc"))));
            for table_len in [17, MAX_TABLE_BYTES + 16] {
                let refused = send(&ctx, &mut conn, &apply_with_table(9, 3, table_len));
                assert_eq!(
                    error_code(&refused),
                    RpcError::INVALID_PARAMS,
                    "{table_len}"
                );
            }
            // ...and a table stream claiming more than the cap is refused at
            // its first chunk.
            let refused = send(
                &ctx,
                &mut conn,
                &table_chunk(9, 0, MAX_TABLE_BYTES + 16, "abcdefghijklmnop"),
            );
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
            assert!(stub.applied.lock().expect("applied").is_empty());
        }

        #[test]
        fn a_table_that_does_not_decode_is_refused_before_the_apply() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);
            let (_, text) = ascii_table();
            // The same entry twice: not strictly ascending.
            let repeated = format!("{}{}", &text[..16], &text[..16]);
            assert!(is_success(&send(&ctx, &mut conn, &chunk(10, 0, 3, "abc"))));
            assert!(is_success(&send(
                &ctx,
                &mut conn,
                &table_chunk(10, 0, 32, &repeated)
            )));
            let refused = send(&ctx, &mut conn, &apply_with_table(10, 3, 32));
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
            assert!(error_message(&refused).contains("not above"), "{refused:?}");
            assert!(stub.applied.lock().expect("applied").is_empty());
        }

        #[test]
        fn a_pre_chunk_inline_map_applies_as_sent_but_never_beside_a_stream() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);
            let inline = || {
                let mut req = apply(14, 3);
                req.params["table"]["map"] = serde_json::json!({ "16": 32 });
                req
            };

            // The shape an older host sends: the map inline, no `table_len`.
            assert!(is_success(&send(&ctx, &mut conn, &chunk(14, 0, 3, "abc"))));
            let mut legacy = inline();
            legacy
                .params
                .as_object_mut()
                .expect("params")
                .remove("table_len");
            let outcome = send(&ctx, &mut conn, &legacy);
            assert!(is_success(&outcome), "{outcome:?}");
            assert_eq!(
                *stub.tables.lock().expect("tables"),
                vec![[(16, 32)].into_iter().collect()]
            );

            // Both at once is refused.
            assert!(is_success(&send(&ctx, &mut conn, &chunk(15, 0, 3, "abc"))));
            let mut both = inline();
            both.params["patch_id"] = serde_json::json!(15);
            both.params["table_len"] = serde_json::json!(16);
            let refused = send(&ctx, &mut conn, &both);
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
            assert!(error_message(&refused).contains("inline"), "{refused:?}");
        }

        #[test]
        fn table_chunks_never_complete_a_patch() {
            let stub = Arc::new(HotStub::default());
            let ctx = enabled_ctx(&stub);
            let mut conn = authed(&ctx);
            assert!(is_success(&send(
                &ctx,
                &mut conn,
                &table_chunk(11, 0, 3, "abc")
            )));
            let refused = send(&ctx, &mut conn, &apply(11, 3));
            assert_eq!(error_code(&refused), RpcError::INVALID_PARAMS);
            assert!(
                error_message(&refused).starts_with("patch 11 has no chunks"),
                "{refused:?}"
            );
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

        /// What the real-socket backend received: each apply's bytes and map.
        type Received = Arc<Mutex<Vec<(Vec<u8>, std::collections::HashMap<u64, u64>)>>>;

        /// A backend offering `HotPatch` whose transfer encoding is lowercase
        /// hex (the service never decodes chunks itself), recording what each
        /// `apply_patch` received.
        struct SocketBackend(Received);

        impl crate::DevtoolsBackend for SocketBackend {
            fn handshake_info(&self, app: &crate::AppInfo) -> HandshakeInfo {
                HandshakeInfo {
                    app_name: app.app_name.clone(),
                    frust_version: app.frust_version.clone(),
                    protocol_version: frust_devtools_protocol::PROTOCOL_VERSION,
                    capabilities: vec![frust_devtools_protocol::Capability::HotPatch],
                }
            }
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
            fn patch_chunk(
                &self,
                chunk: &PatchChunkParams,
            ) -> Result<Vec<u8>, crate::BackendError> {
                let text = chunk.data_base64.as_bytes();
                if !text.len().is_multiple_of(2) {
                    return Err(crate::BackendError::invalid_request("odd hex"));
                }
                text.chunks(2)
                    .map(|pair| {
                        std::str::from_utf8(pair)
                            .ok()
                            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                            .ok_or_else(|| crate::BackendError::invalid_request("not hex"))
                    })
                    .collect()
            }
            fn apply_patch(
                &self,
                bytes: Vec<u8>,
                params: ApplyPatchParams,
            ) -> Result<PatchOutcome, crate::BackendError> {
                let len = bytes.len() as u64;
                self.0
                    .lock()
                    .expect("received")
                    .push((bytes, params.table.map));
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

        /// One blocking NDJSON connection that refuses to write a line over
        /// the server's cap, so the test proves no request needed one.
        struct LineClient {
            writer: std::net::TcpStream,
            reader: std::io::BufReader<std::net::TcpStream>,
            next_id: u64,
            longest_line: usize,
        }

        impl LineClient {
            fn call(&mut self, method: Method, params: Value) -> Response {
                use std::io::{BufRead as _, Write as _};
                self.next_id += 1;
                let line = encode_line(&Request::new(self.next_id, method.as_str(), params));
                assert!(
                    line.len() <= 1 << 20,
                    "a {} byte `{method}` line",
                    line.len()
                );
                self.longest_line = self.longest_line.max(line.len());
                self.writer.write_all(line.as_bytes()).expect("write");
                self.writer.write_all(b"\n").expect("write");
                let mut reply = String::new();
                self.reader.read_line(&mut reply).expect("read");
                let response: Response = serde_json::from_str(reply.trim_end()).expect("reply");
                assert_eq!(response.id, self.next_id);
                response
            }

            /// Sends `bytes` as `method` chunks of `raw` bytes, hex-encoded.
            fn upload(&mut self, method: Method, patch_id: u64, bytes: &[u8], raw: usize) {
                for (index, data) in bytes.chunks(raw).enumerate() {
                    let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
                    let params = serde_json::to_value(PatchChunkParams {
                        patch_id,
                        offset: (index * raw) as u64,
                        total_len: bytes.len() as u64,
                        data_base64: hex,
                    })
                    .expect("chunk");
                    let response = self.call(method, params);
                    assert!(is_success(&response), "{response:?}");
                }
            }
        }

        #[test]
        fn a_two_hundred_thousand_entry_table_crosses_a_real_socket_and_applies_whole() {
            const ENTRIES: u64 = 200_000;
            let received = Received::default();
            let service = crate::Service::start_with_config(
                SocketBackend(Arc::clone(&received)),
                crate::AppInfo::new("large-table", "0.0.0"),
                crate::ServiceConfig {
                    backend_timeout: std::time::Duration::from_secs(10),
                    ..crate::ServiceConfig::default()
                },
            )
            .expect("service starts");
            let stream =
                std::net::TcpStream::connect(("127.0.0.1", service.port())).expect("connect");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(30)))
                .expect("timeout");
            let mut client = LineClient {
                writer: stream.try_clone().expect("clone"),
                reader: std::io::BufReader::new(stream),
                next_id: 0,
                longest_line: 0,
            };
            let token = service.token().expect("a token").to_string();
            let hello = client.call(
                Method::Handshake,
                serde_json::to_value(HandshakeParams { token: Some(token) }).expect("params"),
            );
            assert!(is_success(&hello), "{hello:?}");

            let started = std::time::Instant::now();
            let table = JumpTableWire {
                map: (0..ENTRIES)
                    .map(|i| (0x1000 + i * 8, 0x9000_0000 + i * 4))
                    .collect(),
                aslr_reference: 0x100,
                new_base_address: 0x200,
                ifunc_count: 0,
            };
            let encoded = table.encode_map();
            assert_eq!(encoded.len() as u64, ENTRIES * 16);
            let patch: Vec<u8> = (0..1_000_003u32).map(|i| (i % 251) as u8).collect();
            // 256 KiB raw is 512 KiB of hex: inside the chunk text bound.
            client.upload(Method::PatchChunk, 3, &patch, 256 * 1024);
            client.upload(Method::TableChunk, 3, &encoded, 256 * 1024);
            let params = ApplyPatchParams {
                patch_id: 3,
                len: patch.len() as u64,
                pid: 2,
                anchor_runtime: 1,
                table: table.clone(),
                table_len: encoded.len() as u64,
                expected_seams: 1,
                file: None,
            };
            let outcome = client.call(
                Method::ApplyPatch,
                serde_json::to_value(&params).expect("params"),
            );
            assert!(is_success(&outcome), "{outcome:?}");
            let elapsed = started.elapsed();
            eprintln!(
                "real-socket apply: {ENTRIES} entries ({} table bytes) + {} patch bytes in {} ms, \
                 longest request line {} bytes",
                encoded.len(),
                patch.len(),
                elapsed.as_millis(),
                client.longest_line
            );

            let received = received.lock().expect("received");
            assert_eq!(received.len(), 1);
            assert_eq!(received[0].0, patch);
            assert_eq!(received[0].1.len() as u64, ENTRIES);
            assert_eq!(received[0].1, table.map);
            assert!(
                elapsed < std::time::Duration::from_secs(60),
                "the transfer took {elapsed:?}"
            );
            drop(received);
            service.shutdown();
        }
    }
}
