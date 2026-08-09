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
}

/// Per-connection protocol state. Separate from [`SessionCtx`] because it is
/// exactly the state that must **not** be shared: one client authenticating
/// must not authenticate any other connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConnState {
    authenticated: bool,
}

impl ConnState {
    /// A fresh connection: unauthenticated when the service requires a token,
    /// and pre-authenticated when it does not (so an auth-off service behaves
    /// exactly as it did before auth existed).
    pub(crate) fn new(ctx: &SessionCtx) -> Self {
        Self {
            authenticated: ctx.token.is_none(),
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
        return (
            Response::error(
                id,
                RpcError::new(
                    RpcError::METHOD_NOT_FOUND,
                    format!("unknown method `{}`", req.method),
                ),
            ),
            SideEffect::None,
        );
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
}
