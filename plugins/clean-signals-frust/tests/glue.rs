//! Headless integration tests for clean-signals-frust's controller hooks and
//! failure listener, driven through real pumped Frust frames.
//!
//! There is no GPU window: each test drives the framework's [`RenderRoot`]
//! directly (the desktop shell's own rebuild/layout/paint seam) under an ambient
//! reactive [`Owner`], initialising the [`ReactiveRuntime`] exactly as the shell
//! does. Lifecycle is exercised by mounting/removing components in a keyed list —
//! keyed removal drives the reconciler's structural teardown, which disposes the
//! component's owner and runs its `on_cleanup`s (the seam these hooks rely on).
//!
//! Coverage (the task's acceptance criteria):
//! - `use_controller`: the controller is built exactly once across frames, is
//!   live while mounted, and is disposed exactly once on keyed removal.
//! - `provide_controller` / `expect_controller`: an app-scoped controller
//!   round-trips from a provider component to a nested consumer component;
//!   `expect_controller` with nothing provided panics with the documented
//!   message.
//! - `use_failure_listener`: the handler receives an emitted failure while
//!   mounted, and stops receiving after the component is removed.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use clean_signals::failure::fixtures::NetworkFailure;
use clean_signals::{ControllerCore, FailureSink};
use clean_signals_frust::{
    expect_controller, provide_controller, use_controller, use_failure_listener,
};
use frust::{AnyView, Axis, Component, FlexView, any, component, keyed, text};
use frust_core::RenderRoot;
use frust_text::TextContext;

mod support;
use support::{frame, setup};

// ---------------------------------------------------------------------------
// Harness: shared with the other integration test files via `tests/support`
// (see that module) — the inbox recipe (Frust
// `examples/inbox/tests/async.rs`). These lifecycle tests only need `frame`/
// `setup` (no async use case runs, so no pump-poll loop here).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// A minimal test controller: embeds a `ControllerCore` by composition (the
// clean-signals idiom) and exposes a disposal probe so tests can prove disposal
// runs exactly once. Uses the shared `NetworkFailure` fixture as its failure
// type — no hand-rolled failure duplicates (CODE_STANDARDS).
// ---------------------------------------------------------------------------

struct GlueController {
    core: ControllerCore<NetworkFailure>,
    dispose_calls: Arc<AtomicU32>,
}

impl GlueController {
    fn new() -> Self {
        let core = ControllerCore::new();
        let dispose_calls = Arc::new(AtomicU32::new(0));
        let probe = Arc::clone(&dispose_calls);
        core.on_dispose(move || {
            probe.fetch_add(1, Ordering::SeqCst);
        });
        Self {
            core,
            dispose_calls,
        }
    }

    fn is_disposed(&self) -> bool {
        self.core.is_disposed()
    }

    fn dispose_calls(&self) -> u32 {
        self.dispose_calls.load(Ordering::SeqCst)
    }
}

impl AsRef<ControllerCore<NetworkFailure>> for GlueController {
    fn as_ref(&self) -> &ControllerCore<NetworkFailure> {
        &self.core
    }
}

/// A test spy: a component's `init` publishes its freshly built controller here
/// so a test can observe its lifecycle across frames.
type ControllerSpy = Arc<Mutex<Option<Arc<GlueController>>>>;

fn spy_set(spy: &ControllerSpy, controller: Arc<GlueController>) {
    *spy.lock().unwrap_or_else(|e| e.into_inner()) = Some(controller);
}

fn spy_get(spy: &ControllerSpy) -> Option<Arc<GlueController>> {
    spy.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Test harness state for the mount/remove tests: whether the subject component
/// is currently mounted.
struct Harness {
    show: bool,
}

// ===========================================================================
// 1. use_controller: built once, disposed exactly once on keyed removal.
// ===========================================================================

/// A component that builds its controller via [`use_controller`] in `init`,
/// publishing it to a spy and counting how many times `init` (and thus the
/// factory) runs.
struct ControllerScreen {
    spy: ControllerSpy,
    init_count: Arc<AtomicUsize>,
}

impl Component for ControllerScreen {
    type State = Arc<GlueController>;

    fn init(&self) -> Arc<GlueController> {
        self.init_count.fetch_add(1, Ordering::SeqCst);
        // The component's reactive Owner is ambient here, so `use_controller`'s
        // `on_cleanup` binds disposal to *this* component's teardown.
        let controller = use_controller::<GlueController, NetworkFailure>(GlueController::new);
        spy_set(&self.spy, Arc::clone(&controller));
        controller
    }

    fn build(&self, _state: &mut Arc<GlueController>) -> AnyView<Arc<GlueController>> {
        any(text("controller screen"))
    }
}

#[test]
fn use_controller_builds_once_and_disposes_on_keyed_removal() {
    let _ambient = setup();

    let spy: ControllerSpy = Arc::new(Mutex::new(None));
    let init_count = Arc::new(AtomicUsize::new(0));

    let spy_for_logic = spy.clone();
    let count_for_logic = init_count.clone();
    let mut logic = move |h: &mut Harness| -> FlexView<Harness> {
        let mut children = Vec::new();
        if h.show {
            children.push(keyed(
                1u64,
                component(ControllerScreen {
                    spy: spy_for_logic.clone(),
                    init_count: count_for_logic.clone(),
                }),
            ));
        }
        FlexView::new(Axis::Vertical, children)
    };
    let mut root: RenderRoot<Harness, FlexView<Harness>> = RenderRoot::new();
    let mut state = Harness { show: true };
    let mut tcx = TextContext::new();

    // Mount the component and capture its controller.
    frame(&mut root, &mut logic, &mut state, &mut tcx);
    let controller = spy_get(&spy).expect("mounted component published its controller");
    assert!(
        !controller.is_disposed(),
        "controller is live while mounted"
    );

    // Extra frames: `init`/the factory does not re-run, and the controller stays
    // live and identical.
    for _ in 0..3 {
        frame(&mut root, &mut logic, &mut state, &mut tcx);
    }
    assert_eq!(
        init_count.load(Ordering::SeqCst),
        1,
        "the controller is built exactly once across frames"
    );
    let still = spy_get(&spy).expect("spy still holds the controller");
    assert!(Arc::ptr_eq(&controller, &still), "same controller instance");
    assert!(!controller.is_disposed(), "still live while mounted");

    // Remove the keyed child: teardown → owner cleanup → the component's
    // `on_cleanup` → `ControllerCore::dispose`.
    state.show = false;
    frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(
        controller.is_disposed(),
        "removing the component disposes its embedded controller"
    );
    assert_eq!(
        controller.dispose_calls(),
        1,
        "disposal runs exactly once on teardown"
    );

    // Further frames must not dispose the controller again.
    for _ in 0..3 {
        frame(&mut root, &mut logic, &mut state, &mut tcx);
    }
    assert_eq!(
        controller.dispose_calls(),
        1,
        "still disposed exactly once after extra frames"
    );
}

// ===========================================================================
// 2. provide_controller / expect_controller: round-trip + panic contract.
// ===========================================================================

/// The provider: builds an app-scoped controller and puts it in context (and in
/// a spy so the test can compare identities). It does *not* dispose it — the
/// provider owns the lifecycle, so it uses a plain `Arc`, not `use_controller`.
struct ProviderScreen {
    provided_spy: ControllerSpy,
    consumer_spy: ControllerSpy,
}

impl Component for ProviderScreen {
    type State = ();

    fn init(&self) {
        let controller = Arc::new(GlueController::new());
        spy_set(&self.provided_spy, Arc::clone(&controller));
        provide_controller(Arc::clone(&controller));
    }

    fn build(&self, _state: &mut ()) -> AnyView<()> {
        // A nested consumer component: its owner is a child of this provider's
        // owner, so `expect_controller` finds the context by walking up.
        any(component(ConsumerScreen {
            spy: self.consumer_spy.clone(),
        }))
    }
}

/// The nested consumer: reads the app-scoped controller out of context.
struct ConsumerScreen {
    spy: ControllerSpy,
}

impl Component for ConsumerScreen {
    type State = ();

    fn init(&self) {
        let fetched = expect_controller::<Arc<GlueController>>();
        spy_set(&self.spy, fetched);
    }

    fn build(&self, _state: &mut ()) -> AnyView<()> {
        any(text("consumer screen"))
    }
}

#[test]
fn provide_and_expect_controller_round_trip_through_nested_component() {
    let _ambient = setup();

    let provided_spy: ControllerSpy = Arc::new(Mutex::new(None));
    let consumer_spy: ControllerSpy = Arc::new(Mutex::new(None));

    let provided_for_logic = provided_spy.clone();
    let consumer_for_logic = consumer_spy.clone();
    let mut logic = move |_: &mut ()| {
        component(ProviderScreen {
            provided_spy: provided_for_logic.clone(),
            consumer_spy: consumer_for_logic.clone(),
        })
    };
    let mut root = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    frame(&mut root, &mut logic, &mut state, &mut tcx);

    let provided = spy_get(&provided_spy).expect("provider published its controller");
    let fetched = spy_get(&consumer_spy).expect("consumer resolved the controller from context");
    assert!(
        Arc::ptr_eq(&provided, &fetched),
        "the nested consumer receives the very controller the provider supplied"
    );
    assert!(!fetched.is_disposed());
}

/// A component that calls [`expect_controller`] with nothing provided — must
/// panic during `init`.
struct MissingConsumer;

impl Component for MissingConsumer {
    type State = ();

    fn init(&self) {
        // Nothing provided anywhere up the owner chain → panics.
        let _ = expect_controller::<Arc<GlueController>>();
    }

    fn build(&self, _state: &mut ()) -> AnyView<()> {
        any(text("unreachable"))
    }
}

#[test]
#[should_panic(expected = "no controller of the requested type is provided in context")]
fn expect_controller_panics_when_nothing_provided() {
    let _ambient = setup();

    let mut logic = move |_: &mut ()| component(MissingConsumer);
    let mut root = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    // `init` runs during rebuild and panics with the documented message.
    frame(&mut root, &mut logic, &mut state, &mut tcx);
}

// ===========================================================================
// 3. use_failure_listener: fires while mounted, stops after removal.
// ===========================================================================

/// A component that subscribes to `sink` via [`use_failure_listener`] in `init`,
/// incrementing `hits` for every failure it receives.
struct ListenerScreen {
    sink: FailureSink<NetworkFailure>,
    hits: Arc<AtomicU32>,
}

impl Component for ListenerScreen {
    type State = ();

    fn init(&self) {
        let hits = Arc::clone(&self.hits);
        use_failure_listener(&self.sink, move |_f: NetworkFailure| {
            hits.fetch_add(1, Ordering::SeqCst);
        });
    }

    fn build(&self, _state: &mut ()) -> AnyView<()> {
        any(text("listener screen"))
    }
}

#[test]
fn failure_listener_fires_while_mounted_and_stops_after_removal() {
    let _ambient = setup();

    // The sink outlives the component; `use_failure_listener` subscribes a clone
    // (clones share the same subscriber list), so emitting on this handle reaches
    // the component's listener while it is mounted.
    let sink = FailureSink::<NetworkFailure>::new();
    let hits = Arc::new(AtomicU32::new(0));

    let sink_for_logic = sink.clone();
    let hits_for_logic = hits.clone();
    let mut logic = move |h: &mut Harness| -> FlexView<Harness> {
        let mut children = Vec::new();
        if h.show {
            children.push(keyed(
                1u64,
                component(ListenerScreen {
                    sink: sink_for_logic.clone(),
                    hits: hits_for_logic.clone(),
                }),
            ));
        }
        FlexView::new(Axis::Vertical, children)
    };
    let mut root: RenderRoot<Harness, FlexView<Harness>> = RenderRoot::new();
    let mut state = Harness { show: true };
    let mut tcx = TextContext::new();

    // Mount → the component subscribes.
    frame(&mut root, &mut logic, &mut state, &mut tcx);

    sink.emit(&NetworkFailure::new("boom"));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the listener fires while the component is mounted"
    );

    // Remove the component → teardown drops the subscription.
    state.show = false;
    frame(&mut root, &mut logic, &mut state, &mut tcx);

    sink.emit(&NetworkFailure::new("after"));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the listener is unsubscribed once the component is removed"
    );
}
