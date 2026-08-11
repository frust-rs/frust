//! Log-subscription tests for [`frust_mcp::SessionEngine`]: the push seam a
//! consumer streams a session's output through instead of re-polling
//! `app_logs`.
//!
//! Five contracts are pinned here — a gapless seed→live seam, redaction,
//! bounded overflow with an in-band marker, replace-on-resubscribe, and the
//! channel closing when the session does — plus the seed's own truncation
//! marker.
//!
//! **No real process** (every launch is a scripted `FakeProcessRunner`
//! stream) and **no sleeps in the test**: every wait is on something the
//! engine produces — a session state/log-count transition, or a line arriving
//! on the subscription itself. The scripted per-line delays a few tests use
//! are the *fixture's* pacing, not the test's: they are what makes a
//! subscription land mid-stream, with lines still to come, rather than after
//! a script that already replayed in full.

use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::time::Duration;

mod common;

use common::{
    DEADLINE, DEBUG_DESKTOP_INVOCATION, TEST_PROJECT_ROOT, await_snapshot,
    engine_with_hanging_desktop_stream,
};
use frust_devtools_protocol::{format_discovery_line, redact_discovery_token};
use frust_drive::build_info::BuildMode;
use frust_drive::process::FakeProcessRunner;
use frust_mcp::SessionEngine;
use frust_mcp::engine::{LOG_SUBSCRIPTION_CAP, LogSubscription, RunTarget, SessionState};

/// The prefix every in-band loss marker carries.
const MARKER_PREFIX: &str = "[frust] ";

/// An engine whose desktop launch replays `lines` at their paced delays and
/// then exits — the fixture shape a test needs when it must subscribe while
/// output is still coming.
fn engine_with_paced_desktop_stream(lines: Vec<(String, Duration)>) -> SessionEngine {
    SessionEngine::with_runner(
        TEST_PROJECT_ROOT,
        Arc::new(FakeProcessRunner::new().with_stream_delayed(
            DEBUG_DESKTOP_INVOCATION,
            lines,
            true,
        )),
    )
}

/// Everything already buffered on the subscription, taken without blocking —
/// immediately after subscribing, that is exactly the seed.
fn drain(subscription: &LogSubscription) -> Vec<String> {
    let mut lines = Vec::new();
    loop {
        match subscription.try_recv() {
            Ok(line) => lines.push(line),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return lines,
        }
    }
}

/// The seed and the live feed meet exactly: a subscriber that arrives mid
/// stream sees every line the session ever ingested, once each, in order.
///
/// The script's paced tail is what makes this a seam test rather than a seed
/// test — the subscription is opened while the fixture is still emitting, so
/// the assertion below spans both halves.
#[tokio::test]
async fn a_subscription_seeds_the_backlog_then_continues_live_with_no_gap() {
    const PACED_TAIL_DELAY: Duration = Duration::from_millis(25);
    const IMMEDIATE: usize = 4;
    const TOTAL: usize = 24;

    let expected: Vec<String> = (0..TOTAL).map(|i| format!("line {i}")).collect();
    let script: Vec<(String, Duration)> = expected
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let delay = if i < IMMEDIATE {
                Duration::ZERO
            } else {
                PACED_TAIL_DELAY
            };
            (line.clone(), delay)
        })
        .collect();
    let engine = engine_with_paced_desktop_stream(script);

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    // The unpaced prefix is in the ring, so the seed below is non-empty.
    await_snapshot(&engine, id, |s| s.log_lines >= IMMEDIATE).await;

    let subscription = engine
        .subscribe_logs(id)
        .expect("a live session is subscribable");
    let seeded = drain(&subscription);
    assert!(
        seeded.len() >= IMMEDIATE,
        "the seed must carry the lines already retained, got {seeded:?}"
    );
    assert!(
        seeded.len() < TOTAL,
        "the whole script was already in the ring at subscribe time — this run proved nothing \
         about the live half of the seam"
    );

    // The rest arrive live, and the feed closes when the scripted stream ends.
    let mut seen = seeded;
    loop {
        match subscription.recv_timeout(DEADLINE) {
            Ok(line) => seen.push(line),
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                panic!(
                    "the subscription stalled at {} of {TOTAL} lines",
                    seen.len()
                )
            }
        }
    }

    // No gap, no duplicate, no reordering across the subscribe boundary — and
    // no loss marker, since nothing was ever dropped.
    assert_eq!(seen, expected);
}

/// The subscription is fed where lines enter the ring — i.e. **after** the
/// devtools handshake token is redacted out. A subscriber must never be able
/// to read the token that authenticates against the app's debug service.
#[tokio::test]
async fn a_subscriber_never_sees_the_devtools_handshake_token() {
    const TOKEN: &str = "s3cret-handshake-token";
    // A port nothing listens on: the connect attempt fails, which is
    // irrelevant here — the line's *retention* is what is under test.
    const CLOSED_PORT: u16 = 1;

    let discovery = format_discovery_line(CLOSED_PORT, Some(TOKEN));
    let engine = engine_with_paced_desktop_stream(vec![
        ("Compiling frust v0.1.0".to_string(), Duration::ZERO),
        (discovery.clone(), Duration::from_millis(150)),
    ]);

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    await_snapshot(&engine, id, |s| s.log_lines >= 1).await;

    // Subscribed *before* the discovery line is ingested, so it arrives on the
    // live half of the feed rather than in the seed.
    let subscription = engine
        .subscribe_logs(id)
        .expect("a live session is subscribable");
    let seeded = drain(&subscription);
    assert!(
        !seeded.iter().any(|line| line.contains(TOKEN)),
        "the seed leaked the handshake token: {seeded:?}"
    );

    let live = subscription
        .recv_timeout(DEADLINE)
        .expect("the discovery line reaches the subscriber");
    assert_eq!(live, redact_discovery_token(&discovery).into_owned());
    assert!(
        live.contains("token <redacted>"),
        "the discovery line reached the subscriber unredacted: {live}"
    );
    assert!(!live.contains(TOKEN), "{live}");

    engine.stop_app(id).expect("stop_app");
}

/// A subscriber that stops reading loses lines rather than stalling the
/// session's own drain thread — and is told exactly how many, in band, on the
/// next line the channel accepts.
#[tokio::test]
async fn a_slow_subscriber_drops_lines_and_reads_a_marker_saying_how_many() {
    /// Comfortably past the channel's capacity, so the overflow is real
    /// rather than an off-by-one.
    const FLOOD: usize = LOG_SUBSCRIPTION_CAP + 500;
    /// The flood does not start until the subscription is open…
    const FLOOD_START_DELAY: Duration = Duration::from_millis(400);
    /// …and the line that carries the marker does not arrive until the
    /// reader below has had the channel to itself.
    const TAIL_DELAY: Duration = Duration::from_millis(400);
    const TAIL: &str = "tail line";

    let mut script = vec![("warmup".to_string(), Duration::ZERO)];
    script.extend((0..FLOOD).map(|i| {
        let delay = if i == 0 {
            FLOOD_START_DELAY
        } else {
            Duration::ZERO
        };
        (format!("flood {i}"), delay)
    }));
    script.push((TAIL.to_string(), TAIL_DELAY));
    let engine = engine_with_paced_desktop_stream(script);

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    await_snapshot(&engine, id, |s| s.log_lines >= 1).await;
    let subscription = engine
        .subscribe_logs(id)
        .expect("a live session is subscribable");

    // Nothing is read while the flood runs: the session thread must fill the
    // channel, drop the rest, and carry on — never block on this subscriber.
    // The warmup line plus every flood line, so the whole flood is ingested.
    await_snapshot(&engine, id, |s| s.log_lines > FLOOD).await;

    let mut seen = Vec::new();
    while seen.last().map(String::as_str) != Some(TAIL) {
        match subscription.recv_timeout(DEADLINE) {
            Ok(line) => seen.push(line),
            Err(err) => panic!("the subscription ended before the marker and tail line: {err}"),
        }
    }

    let marker_at = seen
        .iter()
        .position(|line| line.starts_with(MARKER_PREFIX))
        .expect("overflow must be reported in band, never silently");
    // Everything before the marker is the unbroken head of the stream…
    let mut head = vec!["warmup".to_string()];
    head.extend((0..marker_at - 1).map(|i| format!("flood {i}")));
    assert_eq!(&seen[..marker_at], head.as_slice());
    // …the marker accounts for every flood line that did not fit…
    let delivered = marker_at - 1;
    let dropped = FLOOD - delivered;
    assert!(dropped > 0, "the flood never overflowed the channel");
    assert_eq!(
        seen[marker_at],
        format!("[frust] {dropped} log line(s) dropped (slow consumer)")
    );
    // …and the line that finally fit follows it, with nothing after.
    assert_eq!(seen.len(), marker_at + 2);
    assert_eq!(seen[marker_at + 1], TAIL);
    assert!(
        marker_at <= LOG_SUBSCRIPTION_CAP,
        "the channel buffered {marker_at} lines, past its {LOG_SUBSCRIPTION_CAP} cap"
    );

    engine.stop_app(id).expect("stop_app");
}

/// A backlog deeper than the channel cannot be seeded whole: the subscriber
/// gets the newest lines that fit, preceded by one marker naming what it will
/// never see. Same never-silent contract as the overflow above.
#[tokio::test]
async fn a_backlog_deeper_than_the_channel_is_seeded_with_a_marker() {
    const RETAINED: usize = LOG_SUBSCRIPTION_CAP + 500;

    let lines: Vec<String> = (0..RETAINED).map(|i| format!("line {i}")).collect();
    let engine = engine_with_hanging_desktop_stream(lines);

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    await_snapshot(&engine, id, |s| s.log_lines >= RETAINED).await;

    let subscription = engine
        .subscribe_logs(id)
        .expect("a live session is subscribable");
    let seeded = drain(&subscription);

    assert_eq!(
        seeded.len(),
        LOG_SUBSCRIPTION_CAP,
        "the seed fills the channel exactly"
    );
    let kept = LOG_SUBSCRIPTION_CAP - 1;
    let skipped = RETAINED - kept;
    assert_eq!(
        seeded[0],
        format!(
            "[frust] {skipped} log line(s) dropped (backlog older than the subscription buffer)"
        )
    );
    // The newest lines are the ones kept, in arrival order.
    assert_eq!(seeded[1], format!("line {skipped}"));
    assert_eq!(
        seeded.last().expect("a seeded line"),
        &format!("line {}", RETAINED - 1)
    );

    engine.stop_app(id).expect("stop_app");
}

/// One subscriber per session: a second `subscribe_logs` takes over, and the
/// first receiver observes a closed channel rather than a feed that silently
/// went quiet.
#[tokio::test]
async fn a_second_subscription_replaces_the_first() {
    let lines = vec!["line a".to_string(), "line b".to_string()];
    let engine = engine_with_hanging_desktop_stream(lines.clone());

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    let expected = lines.len();
    await_snapshot(&engine, id, move |s| s.log_lines >= expected).await;

    let first = engine.subscribe_logs(id).expect("the first subscription");
    assert_eq!(drain(&first), lines);

    let second = engine.subscribe_logs(id).expect("the second subscription");
    assert!(
        first.recv().is_err(),
        "the replaced subscription must see its channel close"
    );
    // The replacement is fully seeded, exactly as the first one was.
    assert_eq!(drain(&second), lines);

    engine.stop_app(id).expect("stop_app");
}

/// The feed ends with the session: teardown drops the sending half, so a
/// subscriber reads everything still buffered and then a clean disconnect —
/// never a channel that stays open on a session nothing will ingest into
/// again.
#[tokio::test]
async fn stopping_the_session_closes_the_subscription() {
    let engine = engine_with_hanging_desktop_stream(vec!["Compiling frust v0.1.0".to_string()]);
    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    await_snapshot(&engine, id, |s| s.state == SessionState::Running).await;

    let subscription = engine
        .subscribe_logs(id)
        .expect("a live session is subscribable");
    engine.stop_app(id).expect("stop_app");

    let mut seen = Vec::new();
    loop {
        match subscription.recv_timeout(DEADLINE) {
            Ok(line) => seen.push(line),
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                panic!("the subscription outlived the session it was feeding from")
            }
        }
    }
    // Buffered lines are still handed back before the close.
    assert_eq!(seen, vec!["Compiling frust v0.1.0".to_string()]);
}

/// A stale or invented session id is refused cleanly, like every other engine
/// reader.
#[tokio::test]
async fn subscribing_to_an_unknown_session_is_refused() {
    let engine = SessionEngine::new(TEST_PROJECT_ROOT);
    assert!(
        engine
            .subscribe_logs(frust_mcp::engine::SessionId(9_999))
            .is_none()
    );
}
