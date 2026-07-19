//! Headless tests for [`async_view`]: painted-scene assertions for each of
//! the four [`AsyncState`] arms, plus the end-to-end proof that a component
//! wiring [`use_controller`] + [`async_view`] through a real (retrying) async
//! use case transitions Loading→Data across pumped frames — a port of
//! Frust's `examples/inbox` retry test, driven entirely through
//! `clean-signals-frust`'s public API.
//!
//! There is no GPU window: [`support::frame`]/[`support::pump_until`] drive
//! the framework's [`RenderRoot`] directly, exactly like `tests/glue.rs`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use clean_signals::failure::fixtures::NetworkFailure;
use clean_signals::use_case::fixtures::Flaky;
use clean_signals::{
    AsyncState, ControllerCore, Failure, NoParams, RetryPolicy, RunOptions, async_state_signal,
};
use clean_signals_frust::{async_view, use_controller};
use frust::{AnyView, Column, Component, RwSignal, any, component, text};
use frust_core::RenderRoot;
use frust_text::TextContext;
use reactive_graph::traits::{Get, GetUntracked};

mod support;
use support::{frame, pump_until, setup};

// ===========================================================================
// 1. Arm-level mapping: async_view is a pure function of an AsyncState
//    snapshot, so no reactive owner/controller is needed here at all — just
//    feed it a fixed state per test and assert on the painted scene.
// ===========================================================================

type Fixture = AsyncState<Vec<i32>, NetworkFailure>;

/// Renders `state` through `async_view` with fixed, distinguishable arms:
/// - loading → one line
/// - data/reloading → one line per value
/// - error → one line
fn render(state: Fixture) -> AnyView<()> {
    async_view(
        state,
        || any(text("loading")),
        |values: Vec<i32>| {
            any(Column(
                values
                    .into_iter()
                    .map(|v| any(text(v.to_string())))
                    .collect(),
            ))
        },
        |failure: NetworkFailure| any(text(format!("error: {failure}"))),
    )
}

#[test]
fn loading_renders_the_loading_view() {
    let mut logic = |_: &mut ()| render(AsyncState::Loading);
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(scene.glyph_runs, 1, "loading paints exactly one line");
}

#[test]
fn data_renders_one_row_per_value() {
    let mut logic = |_: &mut ()| render(AsyncState::Data(vec![1, 2, 3]));
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(scene.glyph_runs, 3, "data paints one row per value");
}

#[test]
fn error_renders_the_error_view() {
    let mut logic = |_: &mut ()| {
        render(AsyncState::Error {
            failure: NetworkFailure::new("boom"),
            stale: None,
        })
    };
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(scene.glyph_runs, 1, "error paints exactly one line");
}

#[test]
fn reloading_renders_the_data_view_for_the_stale_value() {
    let mut logic = |_: &mut ()| render(AsyncState::Reloading(vec![1, 2]));
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(
        scene.glyph_runs, 2,
        "reloading(v) renders the data view for v, like Data"
    );
}

#[test]
fn error_with_stale_drops_the_stale_value_in_this_v0_shape() {
    // Documented decision: `Error { failure, stale }`'s `stale` payload is not
    // passed to the error closure — only `failure` is. Even with `stale`
    // populated, the error arm still paints just the one error line, not the
    // stale data rows too.
    let mut logic = |_: &mut ()| {
        render(AsyncState::Error {
            failure: NetworkFailure::new("boom"),
            stale: Some(vec![1, 2, 3]),
        })
    };
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(
        scene.glyph_runs, 1,
        "the error arm ignores stale data in this minimal v0 shape"
    );
}

// ===========================================================================
// 2. End-to-end retry test: a component wiring use_controller + async_view to
//    a real, retrying UseCase — ported from Frust's `examples/inbox`
//    (`src/lib.rs`/`tests/async.rs`) THROUGH clean-signals-frust's public
//    API rather than the example's own hand-rolled `use_controller`/
//    `async_view`.
// ===========================================================================

/// The controller: embeds a `ControllerCore` by composition (the
/// clean-signals idiom) and drives the shared `Flaky` fixture use case
/// (fails its first `fail_times` attempts, then succeeds) into an
/// `AsyncState` signal via `run_into` + a `RetryPolicy`.
struct RetryController {
    core: ControllerCore<NetworkFailure>,
    value: RwSignal<AsyncState<i32, NetworkFailure>>,
    flaky: Flaky,
}

impl RetryController {
    fn new() -> Self {
        Self {
            core: ControllerCore::new(),
            value: async_state_signal(),
            // Fails exactly once (retryable `NetworkFailure`), succeeds from
            // the second attempt on — exercises the RetryPolicy exactly like
            // the inbox example's `LoadMessages`.
            flaky: Flaky::new(1),
        }
    }

    async fn load(&self) {
        let opts = RunOptions {
            retry: RetryPolicy::new(3, Duration::from_millis(5)),
            ..Default::default()
        };
        let _ = self
            .core
            .run_into(&self.flaky, NoParams, self.value, opts)
            .await;
    }

    fn attempts(&self) -> u32 {
        self.flaky.attempts()
    }
}

impl AsRef<ControllerCore<NetworkFailure>> for RetryController {
    fn as_ref(&self) -> &ControllerCore<NetworkFailure> {
        &self.core
    }
}

/// A test spy: `RetryScreen::init` publishes its freshly built controller
/// here so the test can observe its state/attempts across pumped frames.
type ControllerSpy = Arc<Mutex<Option<Arc<RetryController>>>>;

struct RetryScreen {
    spy: ControllerSpy,
}

impl Component for RetryScreen {
    type State = Arc<RetryController>;

    fn init(&self) -> Arc<RetryController> {
        // Through the crate's public API: use_controller ties disposal to
        // this component's teardown.
        let controller = use_controller::<RetryController, NetworkFailure>(RetryController::new);
        *self.spy.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&controller));

        // Kick off the load on the UI-thread local task queue, exactly like
        // the inbox example's `Inbox::init`.
        let handle = Arc::clone(&controller);
        frust::spawn_local(async move {
            handle.load().await;
        });

        controller
    }

    fn build(&self, state: &mut Arc<RetryController>) -> AnyView<Arc<RetryController>> {
        // Tracked read: the shell's frame-tracking scope re-renders when
        // `run_into` writes the next state.
        let snapshot = state.value.get();
        async_view(
            snapshot,
            || any(text("Loading…")),
            |v: i32| any(text(format!("value: {v}"))),
            |failure: NetworkFailure| any(text(format!("error: {}", failure.user_message()))),
        )
    }
}

#[test]
fn loading_transitions_to_data_through_pumped_frames_with_retry() {
    let _ambient = setup();

    let spy: ControllerSpy = Arc::new(Mutex::new(None));
    let spy_for_logic = spy.clone();
    let mut logic = move |_: &mut ()| {
        component(RetryScreen {
            spy: spy_for_logic.clone(),
        })
    };
    let mut root = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    // First frame: `init` runs (spawning the load), the signal is still
    // Loading.
    let first = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let controller = spy
        .lock()
        .unwrap()
        .clone()
        .expect("init published the controller to the spy");
    assert!(
        matches!(controller.value.get_untracked(), AsyncState::Loading),
        "the signal starts Loading before any frame is pumped"
    );
    assert_eq!(
        first.glyph_runs, 1,
        "the loading state paints a single line"
    );

    // Pump frames until the load lands — inherently exercises the retry (the
    // first attempt fails, so Data only appears after the second succeeds).
    let scene = pump_until(&mut root, &mut logic, &mut state, &mut tcx, first, || {
        matches!(controller.value.get_untracked(), AsyncState::Data(_))
    });

    assert_eq!(
        controller.attempts(),
        2,
        "the first attempt failed (retryable) and the RetryPolicy retried once"
    );
    assert_eq!(
        scene.glyph_runs, 1,
        "the data state paints a single value line"
    );
    match controller.value.get_untracked() {
        AsyncState::Data(value) => assert_eq!(value, 5, "Flaky resolves to its fixed Ok(5)"),
        other => panic!("expected Data, got {other:?}"),
    }
}
