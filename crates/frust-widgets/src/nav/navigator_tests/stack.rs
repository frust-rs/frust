//! Stack/ops: push/pop/replace, controller depth, `on_result` delivery,
//! opaque-page paint culling, and the side effects a push has on the outgoing
//! top page (captured drag cancelled, focused-field IME session cleared).

use super::super::*;
use super::support::*;
use crate::nav::transition::TransitionSpec;
use crate::test_support::RecordingScene;
use crate::{Column, FlexView};
use frust_core::{FrameTime, PointerPhase, RenderRoot, any};
use kurbo::Rect;
use std::cell::Cell;

// --- Retained per-page widget state across push → pop. ---

#[test]
fn push_pop_preserves_page_widget_state() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let observed = Rc::new(Cell::new(0u32));

    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        let obs = observed.clone();
        move |_: &mut ()| {
            navigator(&ctrl, {
                let obs = obs.clone();
                move || counter_page(&obs)
            })
        }
    };
    let mut state = ();

    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    assert_eq!(observed.get(), 0, "fresh counter starts at zero");

    // Tap page A → its retained counter increments to 1.
    root.event(&mut state, &down(5.0, 5.0));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    assert_eq!(observed.get(), 1);

    // Push B (opaque): A is culled — not painted, count untouched.
    controller.push(|| sized_page(20.0, 20.0));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    assert_eq!(observed.get(), 1, "covered page A is not repainted");

    // Pop B → A is revealed and repainted; its retained count is still 1
    // (proving the pod survived rather than being rebuilt from scratch).
    controller.pop();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    assert_eq!(observed.get(), 1, "page A's widget state survived push→pop");
}

// --- Controller swap on rebuild: rebuilding a `NavigatorView` slot
//     against a *different* `NavigatorController` must not split published
//     state from the ops actually applied, and must not leave the old
//     controller's mounted count stuck above zero forever. `AnyView::rebuild`
//     only matches on concrete view type, never controller identity, so this
//     reaches `NavigatorView::rebuild` — not `build` — exactly like an
//     ordinary same-controller rebuild. ---

#[test]
fn controller_swap_on_rebuild_rebinds_liveness_and_state() {
    let controller_a: NavigatorController<()> = NavigatorController::new();
    let controller_b: NavigatorController<()> = NavigatorController::new();

    let view_a = navigator(&controller_a, || sized_page(10.0, 10.0));
    let mut next_id = 0u64;
    let mut ctx = BuildCtx::new(&mut next_id);
    let mut widget = view_a.build(&mut ctx);

    assert!(
        controller_a.is_mounted(),
        "build mounts the controller it was given"
    );
    assert!(
        !controller_b.is_mounted(),
        "B was never attached to anything yet"
    );
    assert_eq!(controller_a.depth(), 1);

    // Queue an op on B *before* the swap, proving it lands on B once B is
    // the controller actually driving this widget (not dropped, not
    // misapplied to A).
    controller_b.push(|| sized_page(20.0, 20.0));

    // Rebuild the same widget against a view driven by a *different*
    // controller — the misuse this fix makes safe.
    let view_b = navigator(&controller_b, || sized_page(10.0, 10.0));
    view_b.rebuild(&view_a, &mut widget, &mut ctx);

    assert!(
        !controller_a.is_mounted(),
        "the OLD controller must be unmounted in the same rebuild that swaps away from it"
    );
    assert!(
        controller_b.is_mounted(),
        "the NEW controller must be mounted once it drives a live widget"
    );
    assert_eq!(
        controller_b.depth(),
        2,
        "the op queued on B (the controller actually in use) applied"
    );
    assert_eq!(
        controller_a.depth(),
        1,
        "A's published state is frozen where the swap left it, not further updated"
    );

    // Tear down through the view currently bound (B) — the decrement must
    // hit B's cell (via the widget's own captured `mounted`), not re-derive
    // it from `self.controller` by coincidence.
    view_b.teardown(&mut widget, &mut ctx);
    assert!(
        !controller_b.is_mounted(),
        "teardown unmounts whichever controller the widget is currently bound to"
    );
    assert!(!controller_a.is_mounted(), "A stays unmounted");
}

#[test]
fn ops_apply_only_against_the_controller_currently_in_use() {
    let controller_a: NavigatorController<()> = NavigatorController::new();
    let controller_b: NavigatorController<()> = NavigatorController::new();

    let view_a = navigator(&controller_a, || sized_page(10.0, 10.0));
    let mut next_id = 0u64;
    let mut ctx = BuildCtx::new(&mut next_id);
    let mut widget = view_a.build(&mut ctx);

    // Swap to B with no queued ops — a plain rebind.
    let view_b = navigator(&controller_b, || sized_page(10.0, 10.0));
    view_b.rebuild(&view_a, &mut widget, &mut ctx);
    assert_eq!(controller_b.depth(), 1);

    // An op queued on the now-orphaned A must never reach this widget —
    // there is nothing left driving it through A.
    controller_a.push(|| sized_page(30.0, 30.0));
    // An op queued on B, the controller actually in use, must apply.
    controller_b.push(|| sized_page(20.0, 20.0));

    view_b.rebuild(&view_b, &mut widget, &mut ctx);

    assert_eq!(
        controller_b.depth(),
        2,
        "only the op queued on the controller in use (B) applied"
    );
}

// --- The controller's depth slot tracks the stack through rebuilds, and
//     `can_pop` mirrors it. ---

#[test]
fn controller_depth_and_can_pop_track_the_stack() {
    let controller: NavigatorController<()> = NavigatorController::new();

    // Before any navigator attaches, depth is 0 and can_pop is false (an
    // app with no navigator must let back exit).
    assert_eq!(controller.depth(), 0, "no navigator attached yet");
    assert!(!controller.can_pop(), "can_pop is false with no navigator");

    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
    };
    let mut state = ();

    // First rebuild seeds the root page: depth 1, still can't pop the root.
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 1, "root page published on build");
    assert!(!controller.can_pop(), "a single (root) page cannot pop");

    // Push B: depth 2, can_pop true (published at rebuild, when apply_ops runs).
    controller.push(|| sized_page(20.0, 20.0));
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 2, "push published through rebuild");
    assert!(controller.can_pop(), "a two-page stack can pop");

    // Push C: depth 3.
    controller.push(|| sized_page(30.0, 30.0));
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 3);
    assert!(controller.can_pop());

    // Pop back down to the root: depth returns to 1, can_pop false again.
    controller.pop();
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 2);
    assert!(controller.can_pop());

    controller.pop();
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 1, "back at the root");
    assert!(!controller.can_pop(), "root again: pop is a no-op");

    // A pop at the root is a safe no-op — depth stays 1 (the widget's
    // len > 1 guard is authoritative, can_pop is advisory).
    controller.pop();
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        controller.depth(),
        1,
        "pop-at-root does not remove the root"
    );
    assert!(!controller.can_pop());
}

// --- A pop result reaches the on_result callback with state. ---

#[test]
fn pop_result_reaches_callback_with_state() {
    let controller: NavigatorController<ResultState> = NavigatorController::new();

    let mut root: RenderRoot<ResultState, NavigatorView<ResultState>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ResultState| {
            navigator(&ctrl, || {
                any(SizedLeaf {
                    size: Size::new(10.0, 10.0),
                })
            })
        }
    };
    let mut state = ResultState::default();

    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Push B, registering a result callback that records into app state.
    controller.push_for_result(
        || {
            any(SizedLeaf {
                size: Size::new(10.0, 10.0),
            })
        },
        |state: &mut ResultState, result: PopResult| {
            state.received = result.take::<i32>();
        },
    );
    root.rebuild(&mut app, &mut state);

    // Pop B with a payload. The structural pop applies at rebuild, queues the
    // callback, and the same rebuild flushes it through its own
    // `InputEvent::Housekeeping` broadcast. No event pass, no input.
    controller.pop_with_result(PopResult::of(42i32));
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        state.received,
        Some(42),
        "an eager pop delivers its result within the rebuild that applied it — \
         no input event required"
    );
}

// --- push_transparent_for_result: the modal+result combination carries its
//     callback through the pop, same as push_for_result's opaque case, while
//     also keeping the page below visible (transparent) the whole time. ---

#[test]
fn push_transparent_for_result_carries_callback_through_pop() {
    let controller: NavigatorController<ResultState> = NavigatorController::new();
    let mut root: RenderRoot<ResultState, NavigatorView<ResultState>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ResultState| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ResultState::default();

    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Push a transparent "dialog" page registering a result callback.
    controller.push_transparent_for_result(
        || sized_page(20.0, 20.0),
        TransitionSpec::NONE,
        |state: &mut ResultState, result: PopResult| {
            state.received = result.take::<i32>();
        },
    );
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    assert_eq!(
        scene.rects,
        vec![
            (Point::ZERO, Size::new(100.0, 100.0)),
            (Point::ZERO, Size::new(20.0, 20.0)),
        ],
        "the page below the transparent dialog stays visible"
    );

    // Pop the dialog with a payload; the callback is queued and flushed
    // inside the same rebuild, exactly like the opaque push_for_result path.
    controller.pop_with_result(PopResult::of(7i32));
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        state.received,
        Some(7),
        "the transparent modal delivers its result on the pop's own rebuild"
    );
}

// --- Same-frame result delivery: a queued pop result is delivered by the
//     rebuild that queued it, driven by the `InputEvent::Housekeeping`
//     broadcast `RenderRoot::rebuild` dispatches — never by waiting for user
//     input. ---

/// A leaf that counts the hit-tested pointer presses it fires on, so a test
/// can prove the housekeeping broadcast fires **no** ordinary handler. Shaped
/// like every interactive widget in the crate: it fires on `Up`-inside, and
/// its `Down` claims the pointer.
struct TapProbe {
    taps: Rc<Cell<u32>>,
}
struct TapProbeWidget {
    taps: Rc<Cell<u32>>,
}
impl<S: 'static> View<S> for TapProbe {
    type Element = TapProbeWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TapProbeWidget {
        TapProbeWidget {
            taps: self.taps.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut TapProbeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.taps = self.taps.clone();
        ChangeFlags::NONE
    }
}
impl Widget for TapProbeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.max()
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event {
            match p.phase {
                PointerPhase::Down => {
                    ctx.capture_pointer();
                    return EventResult::Handled;
                }
                PointerPhase::Up => {
                    self.taps.set(self.taps.get() + 1);
                    return EventResult::Handled;
                }
                _ => {}
            }
        }
        EventResult::Ignored
    }
}

#[test]
fn enqueue_marks_pending_flush_so_a_gated_frame_still_runs() {
    // Regression pin for the device-proven gap: before this fix, `enqueue`
    // only appended to the plain `Rc<RefCell<Vec<NavOp>>>` queue — nothing
    // told the mobile frame gate a frame was owed, so a page pushed from
    // outside any input/signal path (a `spawn_local` continuation, e.g.)
    // could mount and paint nothing until whatever touch happened to arrive
    // next. `has_pending_result_flush` is the exact peek
    // `FrameInputs::deferred_callbacks_pending` reads to force a `Run`, so
    // it is the reachable proxy here for "the gate would have run this
    // frame" — there is no shell/gate type in scope from this crate.
    let _ = frust_core::take_pending_result_flush();

    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    assert!(
        !frust_core::has_pending_result_flush(),
        "a settled navigator with no queued op owes nothing"
    );

    // No input event, no signal write — exactly the async-continuation
    // shape the device bug reproduced.
    controller.push(|| sized_page(20.0, 20.0));
    assert!(
        frust_core::has_pending_result_flush(),
        "a queued push must mark the flag with no rebuild involved yet"
    );

    // A queued op with nothing else dirty still gets drained on the next
    // rebuild — the gated frame this mark exists to force.
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        nav_widget(&root).pages.len(),
        2,
        "the queued push was applied on the one rebuild that ran, with no \
         input event of any kind"
    );
    assert!(
        !frust_core::has_pending_result_flush(),
        "the rebuild drained the mark it was forced to observe"
    );
}

#[test]
fn every_controller_mutator_funnels_through_the_same_enqueue_mark() {
    // `enqueue` is the single choke point behind every `NavigatorController`
    // mutator (see the type's doc) — this pins that the mark travels with
    // whichever op funnels through it, not just `push`, so a sibling
    // programmatic mutation (`replace`, `pop`) can't reopen the gap `push`
    // alone would leave closed.
    let _ = frust_core::take_pending_result_flush();
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    controller.replace(|| sized_page(30.0, 30.0));
    assert!(
        frust_core::has_pending_result_flush(),
        "replace must mark it too"
    );
    root.rebuild(&mut app, &mut state);
    assert!(!frust_core::has_pending_result_flush());

    controller.push(|| sized_page(20.0, 20.0));
    root.rebuild(&mut app, &mut state);
    assert!(!frust_core::has_pending_result_flush());

    controller.pop();
    assert!(
        frust_core::has_pending_result_flush(),
        "pop must mark it too"
    );
    root.rebuild(&mut app, &mut state);
    assert!(!frust_core::has_pending_result_flush());
}

#[test]
fn eager_pop_result_is_delivered_without_any_input_event() {
    let controller: NavigatorController<ResultState> = NavigatorController::new();
    let mut root: RenderRoot<ResultState, NavigatorView<ResultState>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ResultState| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ResultState::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    controller.push_for_result(
        || sized_page(50.0, 50.0),
        |state: &mut ResultState, result: PopResult| {
            state.received = result.take::<i32>();
        },
    );
    root.rebuild(&mut app, &mut state);

    // The whole point: ONE rebuild, zero `root.event` calls, result present.
    controller.pop_with_result(PopResult::of(5i32));
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        state.received,
        Some(5),
        "the rebuild that applied the pop also flushed its result callback"
    );
    assert!(
        !frust_core::take_pending_result_flush(),
        "the rebuild drained its own flush mark — nothing is left owed to a \
         later frame"
    );
}

#[test]
fn swipe_settle_delivers_its_result_on_the_settle_frames_rebuild() {
    let controller: NavigatorController<SwipeResultState> = NavigatorController::new();
    let mut root: RenderRoot<SwipeResultState, NavigatorView<SwipeResultState>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut SwipeResultState| {
            navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        }
    };
    let mut state = SwipeResultState::default();

    controller.push_for_result(
        || sized_page(100.0, 60.0),
        |state: &mut SwipeResultState, _result: PopResult| {
            state.popped = true;
        },
    );
    let mut sink = RecordingScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut sink, FrameTime::ZERO);

    // Swipe across and release past the commit point → the pop completes.
    // Unlike the eager path, the interactive path queues its callback at
    // `finalize_transition`, so the delivery frame is the *settle* frame.
    root.event(&mut state, &down(5.0, 50.0));
    root.paint(&mut sink, ft(100));
    root.event(&mut state, &move_to(80.0, 50.0));
    root.paint(&mut sink, ft(200));
    root.event(&mut state, &up(80.0, 50.0));
    assert!(
        !state.popped,
        "not delivered while the spring is still running"
    );

    // From here on: rebuild/layout/paint only. No input of any kind.
    let mut delivered_on_finalize = false;
    for t in [300u64, 316, 332, 348, 400, 500, 800, 1200, 2000] {
        let was_running = nav_widget(&root).transition.is_some();
        root.rebuild(&mut app, &mut state);
        let now_settled = nav_widget(&root).transition.is_none();
        if state.popped && !delivered_on_finalize {
            delivered_on_finalize = true;
            assert!(
                was_running && now_settled,
                "delivery must land on the rebuild that finalized the \
                 transition, not a later one"
            );
        }
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut sink, ft(t));
    }
    assert!(
        delivered_on_finalize,
        "the settled swipe delivered its result with no input event"
    );
}

/// App state for the chained-result convergence test.
#[derive(Default)]
struct ChainState {
    hops: Vec<u32>,
}

#[test]
fn a_result_callback_that_navigates_converges_within_one_rebuild() {
    // Two chained hops, both under `MAX_PENDING_RESULT_FLUSH_PASSES`: popping
    // B fires B's callback, which pushes C *and* pops it again; that pop fires
    // C's callback. Both land inside the single `rebuild` below, and the stack
    // it leaves behind is the post-chain one — proving the flush/re-diff cycle
    // really re-runs the build closure rather than shipping a stale view.
    let controller: NavigatorController<ChainState> = NavigatorController::new();
    let mut root: RenderRoot<ChainState, NavigatorView<ChainState>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ChainState| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ChainState::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    controller.push_for_result(|| sized_page(80.0, 80.0), {
        let ctrl = controller.clone();
        move |state: &mut ChainState, _result: PopResult| {
            state.hops.push(1);
            // Calling back into the controller from a result callback is
            // supported (ops are recorded, applied at the next rebuild — here,
            // the re-diff this very flush triggers).
            ctrl.push_for_result(
                || sized_page(60.0, 60.0),
                |state: &mut ChainState, _| {
                    state.hops.push(2);
                },
            );
            ctrl.pop();
        }
    });
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 2, "B pushed");

    controller.pop();
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        state.hops,
        vec![1, 2],
        "both chained result callbacks ran inside the one rebuild"
    );
    assert_eq!(
        controller.depth(),
        1,
        "the ops those callbacks queued were applied by the same rebuild's \
         re-diff, so the stack is already back at the root"
    );
    assert!(
        !frust_core::take_pending_result_flush(),
        "the chain converged under the cap — nothing deferred to a later frame"
    );
}

#[test]
fn the_housekeeping_broadcast_never_fires_a_hit_tested_handler() {
    // `InputEvent::Housekeeping` reports `Point::ZERO` for its position, so a
    // container that hit-tested it instead of branching on `is_broadcast()`
    // would deliver a phantom press to whatever sits at the origin. The page
    // content here is exactly such a widget, filling the whole page.
    let taps = Rc::new(Cell::new(0u32));
    let controller: NavigatorController<ResultState> = NavigatorController::new();
    let mut root: RenderRoot<ResultState, NavigatorView<ResultState>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        let taps = taps.clone();
        move |_: &mut ResultState| {
            navigator(&ctrl, {
                let taps = taps.clone();
                move || any(TapProbe { taps: taps.clone() })
            })
        }
    };
    let mut state = ResultState::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    controller.push_for_result(
        || sized_page(50.0, 50.0),
        |state: &mut ResultState, result: PopResult| {
            state.received = result.take::<i32>();
        },
    );
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // The pop's rebuild broadcasts through the whole tree, including the
    // revealed root page's probe.
    controller.pop_with_result(PopResult::of(1i32));
    root.rebuild(&mut app, &mut state);
    assert_eq!(state.received, Some(1), "the result still arrived");
    assert_eq!(
        taps.get(),
        0,
        "the housekeeping broadcast must fire no press handler — it is not \
         user input"
    );

    // Sanity: the probe *does* fire on a real press, so the assertion above
    // is not passing because the fixture is inert.
    root.event(&mut state, &down(10.0, 10.0));
    root.event(&mut state, &up(10.0, 10.0));
    assert_eq!(taps.get(), 1, "a real tap still activates the same widget");
}

// --- Opaque-page paint culling. ---

fn drive_paint(root: &mut RenderRoot<(), NavigatorView<()>>) -> Vec<(Point, Size)> {
    root.layout(Size::new(100.0, 100.0));
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    scene.rects
}

#[test]
fn opaque_top_culls_pages_below() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);

    // Only the root page paints.
    assert_eq!(
        drive_paint(&mut root),
        vec![(Point::ZERO, Size::new(10.0, 10.0))]
    );

    // Push an opaque page B (20x20): only B paints, A is culled.
    controller.push(|| sized_page(20.0, 20.0));
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        drive_paint(&mut root),
        vec![(Point::ZERO, Size::new(20.0, 20.0))]
    );
}

#[test]
fn transparent_top_paints_page_below() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);

    // A transparent top page B (20x20) over opaque A (10x10): both paint,
    // bottom-to-top (A then B).
    controller.push_transparent(|| sized_page(20.0, 20.0));
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        drive_paint(&mut root),
        vec![
            (Point::ZERO, Size::new(10.0, 10.0)),
            (Point::ZERO, Size::new(20.0, 20.0)),
        ]
    );
}

// --- A captured drag on the top page is cancelled on push. ---

struct CaptureLeaf {
    cancelled: Rc<Cell<bool>>,
}
struct CaptureLeafWidget {
    cancelled: Rc<Cell<bool>>,
}
impl View<()> for CaptureLeaf {
    type Element = CaptureLeafWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CaptureLeafWidget {
        CaptureLeafWidget {
            cancelled: self.cancelled.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut CaptureLeafWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.cancelled = self.cancelled.clone();
        ChangeFlags::NONE
    }
}
impl Widget for CaptureLeafWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.max()
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event {
            match p.phase {
                PointerPhase::Down => {
                    ctx.capture_pointer();
                    return EventResult::Handled;
                }
                // A `Cancel` arm must never touch application state (the `()`
                // tripwire): only record that it fired.
                PointerPhase::Cancel => {
                    self.cancelled.set(true);
                    return EventResult::Handled;
                }
                _ => return EventResult::Handled,
            }
        }
        EventResult::Ignored
    }
}

#[test]
fn push_cancels_captured_drag_on_outgoing_top() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let cancelled = Rc::new(Cell::new(false));
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        let flag = cancelled.clone();
        move |_: &mut ()| {
            let flag = flag.clone();
            navigator(&ctrl, move || {
                any(CaptureLeaf {
                    cancelled: flag.clone(),
                })
            })
        }
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Capture a drag on page A.
    root.event(&mut state, &down(5.0, 5.0));
    assert!(root.is_pointer_captured());
    assert!(!cancelled.get());

    // Push B → A's in-flight capture is cancelled (synthetic Cancel, no state
    // access) as it becomes the covered page.
    controller.push(|| {
        any(SizedLeaf {
            size: Size::new(10.0, 10.0),
        })
    });
    root.rebuild(&mut app, &mut state);
    assert!(
        cancelled.get(),
        "the covered page received a synthetic Cancel"
    );
}

// --- A focused field's IME surface is cleared on push. ---

struct EditableLeaf;
struct EditableLeafWidget;
impl View<()> for EditableLeaf {
    type Element = EditableLeafWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> EditableLeafWidget {
        EditableLeafWidget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut EditableLeafWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}
impl Widget for EditableLeafWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.max()
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Down
        {
            ctx.request_focus();
            ctx.publish_ime_state(ImeState {
                active: true,
                editing: EditingState {
                    text: "abc".to_string(),
                    selection_base: 3,
                    selection_extent: 3,
                    composing_base: -1,
                    composing_extent: -1,
                },
                caret: Some(Rect::new(0.0, 0.0, 1.0, 12.0)),
                content_type: Default::default(),
                suppress_soft_keyboard: false,
            });
            return EventResult::Handled;
        }
        EventResult::Ignored
    }
}

#[test]
fn push_clears_focused_field_ime_surface() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || any(EditableLeaf))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Focus the editable on page A: it publishes an active IME surface.
    root.event(&mut state, &down(5.0, 5.0));
    assert!(root.is_focus_active());
    let ime = root
        .ime_state()
        .expect("focused field published an IME surface");
    assert!(ime.active);
    assert_eq!(ime.editing.text, "abc");

    // Push B programmatically (no blurring tap): the navigator's own switch
    // handling must clear the stale IME surface deterministically at paint.
    controller.push(|| {
        any(SizedLeaf {
            size: Size::new(10.0, 10.0),
        })
    });
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

    // The stale ACTIVE surface is gone — and the inactive surface the
    // navigator published is read as a session release, so the whole session
    // is torn down rather than parked at `Some(inactive)` with `focus_active`
    // left standing. `None` is what the shells serialise to the same inactive
    // JSON the old `Some(inactive)` produced (see `RenderRoot::ime_state`), so
    // the keyboard still drops; what changes is that the root no longer lies.
    assert!(
        root.ime_state().is_none(),
        "the stale active IME surface was released, not parked as Some(inactive)"
    );
    assert!(
        !root.is_focus_active(),
        "the focus session dies with the page that held it"
    );
}

#[test]
fn pop_releases_the_focused_field_session() {
    // The pop twin of `push_clears_focused_field_ime_surface`, and the shape
    // the device report was filed against: a focused field on the page being
    // POPPED. `apply_pop` raises `needs_ime_clear`, paint publishes the
    // inactive surface, and the root releases the session — so an idle screen
    // (no further touch to self-correct on) is left with nothing forcing
    // frames and nothing lying about focus.
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| {
            navigator(&ctrl, || {
                any(SizedLeaf {
                    size: Size::new(10.0, 10.0),
                })
            })
        }
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Push the editable page and focus its field.
    controller.push(|| any(EditableLeaf));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    root.event(&mut state, &down(5.0, 5.0));
    assert!(root.is_focus_active(), "the pushed page's field is focused");
    assert!(root.ime_state().is_some_and(|s| s.active));
    let focused_gen = root.focus_ime_generation();

    // Pop back. Exactly one edge for the release, and the session is gone.
    controller.pop();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    assert!(
        !root.is_focus_active(),
        "the popped page's focus is released"
    );
    assert!(root.ime_state().is_none());
    assert_eq!(
        root.focus_ime_generation(),
        focused_gen.wrapping_add(1),
        "one pop is one focus/IME edge"
    );

    // And it stays released: further idle frames publish nothing, so the
    // shell's `focus_or_ime_changed` edge never fires again.
    for _ in 0..30 {
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    }
    assert_eq!(
        root.focus_ime_generation(),
        focused_gen.wrapping_add(1),
        "an idle popped screen fires no further focus/IME edges"
    );
}

// --- Cross-subtree survival: a navigator whose OWN subtree never held focus
//     must not release somebody else's live session (review-fix-3, FC). ---

/// The IME surface the sibling field owns for the whole of each test below.
fn field_surface() -> ImeState {
    ImeState {
        active: true,
        editing: EditingState {
            text: "query".to_string(),
            selection_base: 5,
            selection_extent: 5,
            composing_base: -1,
            composing_extent: -1,
        },
        caret: Some(Rect::new(0.0, 0.0, 1.0, 12.0)),
        content_type: Default::default(),
        suppress_soft_keyboard: false,
    }
}

/// A **persistent** field-shaped leaf: it claims focus and publishes on
/// `Down` like [`EditableLeaf`], and additionally **republishes the same
/// surface on every paint while it still holds focus** — the behavior a real
/// `TextInput` has, and the reason paint order decides the last write.
///
/// Its size is bounded (unlike `EditableLeaf`'s `bc.max()`) so it can sit as
/// an inflexible child on a `Column`'s unbounded main axis.
struct PersistentField {
    size: Size,
}
struct PersistentFieldWidget {
    size: Size,
}
impl<S: 'static> View<S> for PersistentField {
    type Element = PersistentFieldWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> PersistentFieldWidget {
        PersistentFieldWidget { size: self.size }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut PersistentFieldWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}
impl Widget for PersistentFieldWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.size)
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        if ctx.has_focus() {
            ctx.publish_ime_state(field_surface());
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Down
        {
            ctx.request_focus();
            ctx.publish_ime_state(field_surface());
            return EventResult::Handled;
        }
        EventResult::Ignored
    }
}

/// The search-bar-above-nav shape the defect was reported against: a
/// persistent field as the **earlier** sibling of an unrelated navigator, so
/// the field paints FIRST and any surface the navigator published would win
/// last-write-wins on the way to the root.
///
/// Rows are 40 tall: the field owns `y ∈ [0, 40)`, the navigator `y ∈ [40, 80)`.
fn field_above_navigator_app(
    controller: &NavigatorController<()>,
) -> impl FnMut(&mut ()) -> FlexView<()> + use<> {
    let ctrl = controller.clone();
    move |_: &mut ()| {
        Column(vec![
            any(PersistentField {
                size: Size::new(100.0, 40.0),
            }),
            any(navigator(&ctrl, || sized_page(100.0, 40.0))),
        ])
    }
}

/// The mounted, focused cross-subtree fixture: the live root, its app logic
/// (boxed so the tuple stays a nameable type), and the focus/IME generation
/// the field's session is parked at.
type FixtureAppLogic = Box<dyn FnMut(&mut ()) -> FlexView<()>>;
type FocusedFieldFixture = (RenderRoot<(), FlexView<()>>, FixtureAppLogic, u64);

/// Mount [`field_above_navigator_app`], focus the field with a tap in ITS
/// row, and hand back the fixture.
fn field_focused_beside_navigator(controller: &NavigatorController<()>) -> FocusedFieldFixture {
    let mut root: RenderRoot<(), FlexView<()>> = RenderRoot::new();
    let mut app: FixtureAppLogic = Box::new(field_above_navigator_app(controller));
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

    // Tap inside the field's row — never the navigator's, which would blur
    // the field on the way in (blur-on-outside-tap) and hide the defect.
    root.event(&mut state, &down(5.0, 5.0));
    assert!(
        root.is_focus_active(),
        "the sibling field owns the focus session"
    );
    assert_eq!(
        root.ime_state().map(|s| s.editing.text),
        Some("query".to_string()),
        "…and the shell-facing surface is the field's"
    );
    let generation = root.focus_ime_generation();
    (root, app, generation)
}

/// Every navigator case asserts the same thing: the field's session is
/// untouched — still active, same surface, and **no generation edge at all**
/// (an edge is what wakes the shell's IME machinery).
fn assert_field_session_survived(root: &RenderRoot<(), FlexView<()>>, generation: u64, op: &str) {
    assert!(
        root.is_focus_active(),
        "{op} in an unfocused navigator must not release the sibling field's session"
    );
    let ime = root
        .ime_state()
        .unwrap_or_else(|| panic!("{op} dropped the sibling field's IME surface entirely"));
    assert!(ime.active, "{op} left the field's surface inactive");
    assert_eq!(
        ime.editing.text, "query",
        "{op} replaced the field's surface with someone else's"
    );
    assert_eq!(
        root.focus_ime_generation(),
        generation,
        "{op} in an unfocused navigator is not a focus/IME edge"
    );
}

#[test]
fn pop_in_an_unfocused_navigator_leaves_a_sibling_fields_session_alive() {
    let controller: NavigatorController<()> = NavigatorController::new();
    // Depth 2 before the first frame, so the pop below is a real one.
    controller.push(|| sized_page(100.0, 40.0));
    let (mut root, mut app, generation) = field_focused_beside_navigator(&controller);
    let mut state = ();

    controller.pop();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

    assert_field_session_survived(&root, generation, "a pop");
}

#[test]
fn push_in_an_unfocused_navigator_leaves_a_sibling_fields_session_alive() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let (mut root, mut app, generation) = field_focused_beside_navigator(&controller);
    let mut state = ();

    // A push COVERS the navigator's own top page — but that page holds no
    // focus link, so there is no session of its own to end.
    controller.push(|| sized_page(100.0, 40.0));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

    assert_field_session_survived(&root, generation, "a push");
}

#[test]
fn replace_in_an_unfocused_navigator_leaves_a_sibling_fields_session_alive() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let (mut root, mut app, generation) = field_focused_beside_navigator(&controller);
    let mut state = ();

    controller.replace(|| sized_page(100.0, 40.0));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

    assert_field_session_survived(&root, generation, "a replace");
}

// --- An example-style stack driven through the facade
//     API (counter pattern), proving replace swaps the top in place. ---

#[test]
fn replace_swaps_top_in_place() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        drive_paint(&mut root),
        vec![(Point::ZERO, Size::new(10.0, 10.0))]
    );

    // Replace the root page with a 30x30 page: still a single page, new size.
    controller.replace(|| sized_page(30.0, 30.0));
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        drive_paint(&mut root),
        vec![(Point::ZERO, Size::new(30.0, 30.0))]
    );

    // Pop is a no-op on a single-page stack (root is never popped).
    controller.pop();
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        drive_paint(&mut root),
        vec![(Point::ZERO, Size::new(30.0, 30.0))]
    );
}

#[test]
fn pop_result_take_recovers_typed_payload() {
    assert_eq!(PopResult::of(7u8).take::<u8>(), Some(7));
    assert_eq!(PopResult::of(7u8).take::<i64>(), None);
    assert!(PopResult::empty().is_empty());
    assert_eq!(PopResult::empty().take::<u8>(), None);
}
