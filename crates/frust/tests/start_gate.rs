//! `StartGate` shows a splash until its start-up task resolves, then the ready view.
#![cfg(feature = "test-support")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use frust::authoring::Size;
use frust::testing::{Tester, TesterOptions, find};
use frust::{StartGate, View, text};

const SIZE: Size = Size::new(400.0, 300.0);
const WAIT: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Boom;

impl std::fmt::Display for Boom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("boom")
    }
}

impl std::error::Error for Boom {}

/// Shared knobs: how many attempts ran, how many were allowed to fail, ready-builder calls.
#[derive(Clone, Default)]
struct Probe {
    attempts: Arc<AtomicUsize>,
    failures: Arc<AtomicUsize>,
    ready_calls: Arc<AtomicUsize>,
    release: Arc<AtomicBool>,
}

fn gate_tester(probe: &Probe) -> Tester<(), impl View<()>> {
    let probe = probe.clone();
    Tester::new_with(SIZE, &TesterOptions::default(), move || {
        ((), move |_: &mut ()| {
            let start = probe.clone();
            let ready = probe.clone();
            StartGate::new(move || {
                let probe = start.clone();
                async move {
                    let attempt = probe.attempts.fetch_add(1, Ordering::SeqCst);
                    while !probe.release.load(Ordering::SeqCst) {
                        frust::time::sleep(Duration::from_millis(5)).await;
                    }
                    if attempt < probe.failures.load(Ordering::SeqCst) {
                        Err(Boom)
                    } else {
                        Ok(7_u32)
                    }
                }
            })
            .splash(text("splash"))
            .ready(move |value: &u32| {
                ready.ready_calls.fetch_add(1, Ordering::SeqCst);
                text(format!("ready {value}"))
            })
        })
    })
}

fn shows<S: 'static, V: View<S>>(tester: &Tester<S, V>, s: &str) -> bool {
    !tester.find(&find::text(s)).is_empty()
}

#[test]
fn splash_shows_then_ready_replaces_it() {
    let probe = Probe::default();
    let mut tester = gate_tester(&probe);
    tester.pump(Duration::from_millis(16));
    assert!(shows(&tester, "splash"));
    probe.release.store(true, Ordering::SeqCst);
    tester.pump_until("ready view", WAIT, |t| shows(t, "ready 7"));
    assert!(!shows(&tester, "splash"));
    assert_eq!(probe.ready_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn error_shows_the_error_view_and_retry_recovers() {
    let probe = Probe::default();
    probe.failures.store(1, Ordering::SeqCst);
    probe.release.store(true, Ordering::SeqCst);
    let mut tester = gate_tester(&probe);
    tester.pump_until("error view", WAIT, |t| shows(t, "boom"));
    assert!(shows(&tester, "Retry"));
    assert_eq!(probe.ready_calls.load(Ordering::SeqCst), 0);
    tester.tap(find::text("Retry"));
    tester.pump_until("ready after retry", WAIT, |t| shows(t, "ready 7"));
    assert_eq!(probe.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(probe.ready_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn ready_builder_is_not_rerun_by_further_pumps() {
    let probe = Probe::default();
    probe.release.store(true, Ordering::SeqCst);
    let mut tester = gate_tester(&probe);
    tester.pump_until("ready view", WAIT, |t| shows(t, "ready 7"));
    for _ in 0..5 {
        tester.pump(Duration::from_millis(16));
    }
    assert_eq!(probe.ready_calls.load(Ordering::SeqCst), 1);
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn dropping_the_owner_aborts_a_pending_task() {
    let dropped = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    let tester = {
        let dropped = dropped.clone();
        let seen = started.clone();
        let mut tester = Tester::new_with(SIZE, &TesterOptions::default(), move || {
            ((), move |_: &mut ()| {
                let dropped = dropped.clone();
                let started = seen.clone();
                StartGate::new(move || {
                    let flag = DropFlag(dropped.clone());
                    let started = started.clone();
                    async move {
                        let _flag = flag;
                        started.store(true, Ordering::SeqCst);
                        std::future::pending::<Result<u32, Boom>>().await
                    }
                })
                .splash(text("splash"))
                .ready(|_: &u32| text("ready"))
            })
        });
        tester.pump_until("task started", WAIT, |_| started.load(Ordering::SeqCst));
        tester
    };
    assert!(!dropped.load(Ordering::SeqCst));
    drop(tester);
    let deadline = std::time::Instant::now() + WAIT;
    while !dropped.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        dropped.load(Ordering::SeqCst),
        "pending task was not aborted"
    );
}
