//! End-to-end wire round-trips: `encode_line`/`decode_line` a full
//! `Request`/`Response`/`Notification` envelope carrying each v1 method's
//! typed params/result/notification-payload struct, confirming the whole
//! path (envelope → NDJSON line → envelope) preserves every field.

use frust_devtools_protocol::{
    AckResult, Capability, DecodeError, FrameStats, HandshakeInfo, Incoming, InputScrollParams,
    InputTapParams, InputTextParams, Method, MetricsSnapshot, Notification, PROTOCOL_VERSION,
    RectPx, Request, Response, ResponseOutcome, RpcError, ScreenshotResult, WidgetNode,
    WidgetProps, WidgetPropsParams, WidgetTreeDump, decode_line, encode_line, parse_discovery_line,
};

fn round_trip_request<T: serde::Serialize>(method: Method, params: &T) -> Request {
    let req = Request::new(1, method.as_str(), serde_json::to_value(params).unwrap());
    let line = encode_line(&req);
    match decode_line(&line).unwrap() {
        Incoming::Request(r) => {
            assert_eq!(r.method, method.as_str());
            r
        }
        other => panic!("expected Request for {method}, got {other:?}"),
    }
}

fn round_trip_success<
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
>(
    result: T,
) {
    let resp = Response::success(1, serde_json::to_value(&result).unwrap());
    let line = encode_line(&resp);
    match decode_line(&line).unwrap() {
        Incoming::Response(r) => match r.outcome {
            ResponseOutcome::Success { result: v } => {
                let back: T = serde_json::from_value(v).unwrap();
                assert_eq!(back, result);
            }
            ResponseOutcome::Error { .. } => panic!("expected a success outcome"),
        },
        other => panic!("expected Response, got {other:?}"),
    }
}

#[test]
fn handshake_round_trips() {
    round_trip_request(Method::Handshake, &serde_json::Value::Null);
    round_trip_success(HandshakeInfo {
        app_name: "huddle".to_string(),
        frust_version: "0.1.0".to_string(),
        protocol_version: PROTOCOL_VERSION,
        capabilities: vec![
            Capability::WidgetTree,
            Capability::FrameStats,
            Capability::Input,
            Capability::Metrics,
            Capability::Screenshot,
        ],
    });
}

#[test]
fn widget_tree_round_trips() {
    round_trip_request(Method::WidgetTree, &serde_json::Value::Null);
    round_trip_success(WidgetTreeDump {
        roots: vec![WidgetNode {
            id: 1,
            type_name: "Flex".to_string(),
            debug_label: Some("root".to_string()),
            bounds: Some(RectPx {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 800.0,
            }),
            children: vec![WidgetNode {
                id: 2,
                type_name: "Text".to_string(),
                debug_label: None,
                bounds: None,
                children: vec![],
            }],
        }],
    });
}

#[test]
fn widget_props_round_trips() {
    round_trip_request(Method::WidgetProps, &WidgetPropsParams { id: 42 });
    round_trip_success(WidgetProps {
        id: 42,
        entries: vec![
            ("text".to_string(), "hello".to_string()),
            ("size".to_string(), "32.0".to_string()),
        ],
    });
}

#[test]
fn frame_stats_subscribe_acks_and_frame_stats_pushes_as_notification() {
    round_trip_request(Method::FrameStatsSubscribe, &serde_json::Value::Null);
    round_trip_success(AckResult { ok: true });

    let stats = FrameStats {
        n: 100,
        total_us: 8_000,
        rebuild_us: 500,
        layout_us: 700,
        paint_us: 1_200,
        encode_us: 2_000,
        acquire_us: 300,
        submit_us: 3_300,
        skipped: false,
    };
    let notif = Notification::new(
        Method::FrameStats.as_str(),
        serde_json::to_value(stats).unwrap(),
    );
    let line = encode_line(&notif);
    match decode_line(&line).unwrap() {
        Incoming::Notification(n) => {
            assert_eq!(n.method, "frame_stats");
            let back: FrameStats = serde_json::from_value(n.params).unwrap();
            assert_eq!(back, stats);
        }
        other => panic!("expected Notification, got {other:?}"),
    }
}

#[test]
fn metrics_snapshot_round_trips() {
    round_trip_request(Method::MetricsSnapshot, &serde_json::Value::Null);
    round_trip_success(MetricsSnapshot {
        rss_bytes: Some(123_456_789),
        uptime_ms: 42_000,
    });
    // rss_bytes is best-effort/platform-dependent — None must round-trip too.
    round_trip_success(MetricsSnapshot {
        rss_bytes: None,
        uptime_ms: 42_000,
    });
}

#[test]
fn input_tap_round_trips() {
    round_trip_request(Method::InputTap, &InputTapParams { x: 10.5, y: 20.25 });
    round_trip_success(AckResult { ok: true });
}

#[test]
fn input_scroll_round_trips() {
    round_trip_request(
        Method::InputScroll,
        &InputScrollParams {
            x: 1.0,
            y: 2.0,
            dx: 0.0,
            dy: -15.0,
        },
    );
    round_trip_success(AckResult { ok: true });
}

#[test]
fn input_text_round_trips() {
    round_trip_request(
        Method::InputText,
        &InputTextParams {
            text: "hello, frust".to_string(),
        },
    );
    round_trip_success(AckResult { ok: true });
}

#[test]
fn screenshot_round_trips_success_and_not_supported_error() {
    round_trip_request(Method::Screenshot, &serde_json::Value::Null);
    round_trip_success(ScreenshotResult {
        png_base64: "iVBORw0KGgo=".to_string(),
    });

    // Capability-gated rejection path.
    let resp = Response::error(1, RpcError::not_supported("no Screenshot capability"));
    let line = encode_line(&resp);
    match decode_line(&line).unwrap() {
        Incoming::Response(r) => match r.outcome {
            ResponseOutcome::Error { error } => {
                assert_eq!(error.code, RpcError::NOT_SUPPORTED);
            }
            ResponseOutcome::Success { .. } => panic!("expected an error outcome"),
        },
        other => panic!("expected Response, got {other:?}"),
    }
}

#[test]
fn incoming_discriminates_all_three_shapes_from_the_same_decoder() {
    let request_line = encode_line(&Request::new(1, "handshake", serde_json::Value::Null));
    let response_line = encode_line(&Response::success(1, serde_json::json!({"ok": true})));
    let notification_line = encode_line(&Notification::new(
        "frame_stats",
        serde_json::json!({"n": 1}),
    ));

    assert!(matches!(
        decode_line(&request_line).unwrap(),
        Incoming::Request(_)
    ));
    assert!(matches!(
        decode_line(&response_line).unwrap(),
        Incoming::Response(_)
    ));
    assert!(matches!(
        decode_line(&notification_line).unwrap(),
        Incoming::Notification(_)
    ));
}

#[test]
fn unknown_method_and_unknown_fields_are_tolerated_end_to_end() {
    // A method name outside the v1 Method set still decodes as a Request —
    // forward compatibility for a protocol version bump on one side only.
    let line = r#"{"jsonrpc":"2.0","id":1,"method":"future_method","params":{"a":1},"unexpected":"field"}"#;
    match decode_line(line).unwrap() {
        Incoming::Request(r) => {
            assert_eq!(r.method, "future_method");
            assert_eq!(Method::from_str(&r.method), None);
        }
        other => panic!("expected Request, got {other:?}"),
    }
}

#[test]
fn decode_line_rejects_malformed_json_with_a_useful_error() {
    let err = decode_line("{not json").unwrap_err();
    assert!(matches!(err, DecodeError::InvalidJson(_)));
    assert!(err.to_string().contains("invalid JSON"));
}

#[test]
fn discovery_line_parses_across_prefix_variants() {
    assert_eq!(
        parse_discovery_line("frust-devtools listening on 54321"),
        Some(54321)
    );
    assert_eq!(
        parse_discovery_line(
            "08-10 12:00:00.123  1234  5678 I Frust   : frust-devtools listening on 8123"
        ),
        Some(8123)
    );
    assert_eq!(
        parse_discovery_line("[2026-08-10T12:00:00Z] app: frust-devtools listening on 65000\n"),
        Some(65000)
    );
    assert_eq!(parse_discovery_line("no marker here"), None);
}
