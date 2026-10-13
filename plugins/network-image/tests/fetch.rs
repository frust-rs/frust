//! `ReqwestFetcher` against the loopback test server.
#![cfg(feature = "reqwest")]

mod support;

use std::time::Duration;

use frust_network_image::{FetchError, FetchRequest, ImageFetcher, ReqwestFetcher};
use support::{Route, spawn};

fn req(url: String, max_bytes: usize) -> FetchRequest {
    FetchRequest {
        url,
        headers: Vec::new(),
        max_bytes,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn body_round_trips_with_content_type() {
    let server = spawn(vec![(
        "/a.png",
        Route::Ok {
            content_type: "image/png",
            body: b"hello".to_vec(),
        },
    )])
    .await;
    let fetcher = ReqwestFetcher::new().unwrap();
    let got = fetcher
        .fetch(req(server.url("/a.png"), 1024))
        .await
        .unwrap();
    assert_eq!(got.bytes, b"hello");
    assert_eq!(got.content_type.as_deref(), Some("image/png"));
    let log = server.requests.lock().unwrap();
    assert_eq!(log[0].method, "GET");
    assert!(
        log[0]
            .header("user-agent")
            .unwrap()
            .starts_with("frust-network-image/")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_header_reaches_server() {
    let server = spawn(vec![(
        "/p",
        Route::Ok {
            content_type: "image/png",
            body: vec![1],
        },
    )])
    .await;
    let fetcher = ReqwestFetcher::new().unwrap();
    let mut r = req(server.url("/p"), 1024);
    r.headers.push(("Authorization".into(), "Bearer x".into()));
    fetcher.fetch(r).await.unwrap();
    let log = server.requests.lock().unwrap();
    assert_eq!(log[0].header("authorization"), Some("Bearer x"));
}

#[tokio::test(flavor = "multi_thread")]
async fn not_found_is_status() {
    let server = spawn(vec![]).await;
    let fetcher = ReqwestFetcher::new().unwrap();
    let err = fetcher
        .fetch(req(server.url("/missing"), 1024))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, FetchError::Status(404)), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_body_is_too_large() {
    let three_mib = 3 * 1024 * 1024;
    let server = spawn(vec![
        (
            "/announced",
            Route::Ok {
                content_type: "image/png",
                body: vec![0; three_mib],
            },
        ),
        ("/streamed", Route::Oversized { len: three_mib }),
    ])
    .await;
    let fetcher = ReqwestFetcher::new().unwrap();
    for path in ["/announced", "/streamed"] {
        let err = fetcher
            .fetch(req(server.url(path), 1024 * 1024))
            .await
            .err()
            .unwrap();
        assert!(
            matches!(err, FetchError::TooLarge { limit } if limit == 1024 * 1024),
            "{path}: {err:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn silent_server_times_out() {
    let server = spawn(vec![("/hang", Route::Hang)]).await;
    let fetcher = ReqwestFetcher::with_timeout(Duration::from_secs(1)).unwrap();
    let err = fetcher
        .fetch(req(server.url("/hang"), 1024))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, FetchError::Timeout), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn https_to_plain_server_is_network_error() {
    let server = spawn(vec![(
        "/a",
        Route::Ok {
            content_type: "image/png",
            body: vec![1],
        },
    )])
    .await;
    let fetcher = ReqwestFetcher::new().unwrap();
    let err = fetcher
        .fetch(req(server.https_url("/a"), 1024))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, FetchError::Network(_)), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_scheme_fails_before_any_socket() {
    let server = spawn(vec![]).await;
    let fetcher = ReqwestFetcher::new().unwrap();
    let err = fetcher
        .fetch(req("javascript:alert(1)".into(), 1024))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, FetchError::InvalidUrl));
    assert_eq!(server.request_count(), 0);
}
