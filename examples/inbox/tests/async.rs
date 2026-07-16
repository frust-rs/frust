//! Headless proof that a `clean_signals` controller drives a ForgeKit
//! [`Component`] through real pumped frames (phase-5.5 exit gate).
//!
//! There is no GPU window here: the test drives the framework's [`RenderRoot`]
//! directly (the desktop shell's own rebuild/layout/paint seam) and pumps the
//! reactive runtime's UI-thread local task queue
//! ([`ReactiveRuntime::pump_local`]) frame by frame — exactly what the desktop
//! shell does each loop turn. Text is shaped through a real `TextContext`
//! (CPU-only) and paint output is recorded, so every assertion is deterministic.
//!
//! Coverage:
//! - **Loading → Data through pumped frames, including the retry path.** The
//!   fake repo fails its first attempt (retryable) and succeeds on the second;
//!   the painted scene must transition from the single loading line to one row
//!   per message, and the controller's attempt counter must read `2`.
//! - **Disposal on teardown, exactly once.** Removing the component from a keyed
//!   list disposes its embedded `ControllerCore` a single time (an `on_dispose`
//!   probe), wired via the component's `on_cleanup` — the seam this phase
//!   proves. Extra frames/pumps never dispose again.

use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use forgekit::{Axis, ComponentView, FlexView, component, keyed};
use forgekit_core::{PaintScene, RenderRoot, View};
use forgekit_reactive::{FrameWaker, ReactiveRuntime};
use forgekit_scene::GlyphRun;
use forgekit_text::TextContext;
use inbox::{AsyncState, ControllerSpy, Inbox, InboxController, MESSAGE_COUNT};
use kurbo::{Point, Rect, Size};
use peniko::Color;
use reactive_graph::owner::Owner;
use reactive_graph::traits::GetUntracked;

const W: f64 = 800.0;
const H: f64 = 600.0;

/// A GPU-free paint target that counts glyph runs (the loading line vs. the
/// message rows) so the test can observe the Loading→Data transition in the
/// painted scene rather than only in the signal.
#[derive(Default)]
struct RecScene {
    glyph_runs: usize,
}

impl PaintScene for RecScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
    fn fill_rounded_rect(&mut self, _origin: Point, _size: Size, _radius: f64, _color: Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn draw_glyph_run(&mut self, _run: GlyphRun) {
        self.glyph_runs += 1;
    }
    fn draw_image(&mut self, _data: &peniko::ImageData, _dest: Rect) {}
}

/// One frame: rebuild the tree from `state`, lay it out at window size shaping
/// real text, and record the paint output.
fn frame<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
) -> RecScene {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, H), tcx_any);
    let mut scene = RecScene::default();
    root.paint(&mut scene);
    scene
}

/// A no-op-but-recording frame waker (see `ReactiveRuntime::init`). The global
/// waker is swapped by whichever test inits last, so the count is not asserted
/// on — installing a recording waker just exercises the real init path.
fn recording_waker() -> (FrameWaker, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let seen = counter.clone();
    let waker: FrameWaker = Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    (waker, seen)
}

#[test]
fn loading_transitions_to_data_through_pumped_frames_with_retry() {
    let (waker, _seen) = recording_waker();
    let runtime = ReactiveRuntime::init(waker);

    // Set an ambient reactive owner so each component's owner (created as its
    // child) is valid — mirrors the desktop shell's per-frame `with_owner`.
    let ambient = Owner::new();
    ambient.set();

    let spy: ControllerSpy = Arc::new(Mutex::new(None));
    let spy_for_logic = spy.clone();
    let mut logic = move |_: &mut ()| component(Inbox::with_spy(spy_for_logic.clone()));
    let mut root: RenderRoot<(), ComponentView<Inbox>> = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    // First frame: `init` runs (spawning the load), the signal is still Loading.
    let first = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let controller = spy
        .lock()
        .unwrap()
        .clone()
        .expect("init published the controller to the spy");
    assert!(
        matches!(controller.messages.get_untracked(), AsyncState::Loading),
        "the signal starts Loading before any frame is pumped"
    );
    assert_eq!(
        first.glyph_runs, 1,
        "the loading state paints a single spinner line"
    );

    // Drive frames — pump the local task queue, then rebuild/layout/paint — until
    // the load lands. This inherently exercises the retry (the first attempt
    // fails, so Data only appears after the second attempt succeeds).
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut scene = first;
    while !matches!(controller.messages.get_untracked(), AsyncState::Data(_)) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for Loading→Data (attempts so far: {})",
            controller.attempts()
        );
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(1));
        scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    }

    // The retry path ran: attempt 1 failed (retryable), attempt 2 succeeded.
    assert_eq!(
        controller.attempts(),
        2,
        "the first attempt failed and the RetryPolicy retried once"
    );
    // The painted scene now reflects the loaded messages — one glyph run per row.
    assert_eq!(
        scene.glyph_runs, MESSAGE_COUNT,
        "the data state paints one row per message"
    );
    match controller.messages.get_untracked() {
        AsyncState::Data(messages) => assert_eq!(messages.len(), MESSAGE_COUNT),
        other => panic!("expected Data, got {other:?}"),
    }
}

/// Test harness state for the teardown test: whether the inbox is mounted.
struct Harness {
    show: bool,
}

#[test]
fn teardown_disposes_controller_exactly_once() {
    let (waker, _seen) = recording_waker();
    let runtime = ReactiveRuntime::init(waker);

    let ambient = Owner::new();
    ambient.set();

    let spy: ControllerSpy = Arc::new(Mutex::new(None));
    let spy_for_logic = spy.clone();
    // The inbox lives in a keyed list; removing it drives the keyed reconciler's
    // teardown path (structural remove), which disposes the component's owner.
    let mut logic = move |h: &mut Harness| -> FlexView<Harness> {
        let mut children = Vec::new();
        if h.show {
            children.push(keyed(
                1u64,
                component(Inbox::with_spy(spy_for_logic.clone())),
            ));
        }
        FlexView::new(Axis::Vertical, children)
    };
    let mut root: RenderRoot<Harness, FlexView<Harness>> = RenderRoot::new();
    let mut state = Harness { show: true };
    let mut tcx = TextContext::new();

    // Mount the component and let its load get in flight.
    frame(&mut root, &mut logic, &mut state, &mut tcx);
    let controller: Arc<InboxController> = spy
        .lock()
        .unwrap()
        .clone()
        .expect("the mounted component published its controller");
    assert!(!controller.is_disposed(), "live while mounted");
    for _ in 0..3 {
        runtime.pump_local();
        frame(&mut root, &mut logic, &mut state, &mut tcx);
    }

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

    // Further frames and pumps must not dispose the controller again.
    for _ in 0..3 {
        runtime.pump_local();
        frame(&mut root, &mut logic, &mut state, &mut tcx);
    }
    assert_eq!(
        controller.dispose_calls(),
        1,
        "still disposed exactly once after extra frames"
    );
}
