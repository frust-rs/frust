//! Interactive edge-swipe back gesture: arm/steal/drive/settle, gesture
//! policy (`BackPolicy` arm gate, per-route/platform `pop_swipe` resolution),
//! and R-B3-inner (innermost-first swipe claiming).

use super::super::*;
use super::support::*;
use crate::nav::edge_swipe::EDGE_SWIPE_COMMIT_PROGRESS;
use crate::nav::transition::{PageTransition, Timing, TransitionSpec};
use crate::test_support::RecordingScene;
use frust_core::Curve;
use frust_core::{FrameTime, PointerPhase, RenderRoot, any};
use frust_theme::Theme;
use std::any::Any;
use std::cell::Cell;
use std::time::Duration;

// ---------------------------------------------------------------------
// Interactive edge-swipe back gesture.
// ---------------------------------------------------------------------

/// A page leaf that captures the pointer on `Down`, counts the `Move`s it
/// receives, and records whether it got a synthetic `Cancel` — so a test can
/// tell a steal (child gets Cancel, no more moves) from a yield (child keeps
/// receiving moves). Its `Cancel` arm touches no application state (the `()`
/// tripwire contract).
struct DragProbe {
    moves: Rc<Cell<u32>>,
    cancelled: Rc<Cell<bool>>,
}
struct DragProbeWidget {
    moves: Rc<Cell<u32>>,
    cancelled: Rc<Cell<bool>>,
}
impl View<()> for DragProbe {
    type Element = DragProbeWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> DragProbeWidget {
        DragProbeWidget {
            moves: self.moves.clone(),
            cancelled: self.cancelled.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut DragProbeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.moves = self.moves.clone();
        element.cancelled = self.cancelled.clone();
        ChangeFlags::NONE
    }
}
impl Widget for DragProbeWidget {
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
                PointerPhase::Move => {
                    self.moves.set(self.moves.get() + 1);
                    return EventResult::Handled;
                }
                PointerPhase::Cancel => {
                    self.cancelled.set(true);
                    return EventResult::Handled;
                }
                PointerPhase::Up => return EventResult::Handled,
            }
        }
        EventResult::Ignored
    }
}

// erasure: keep borrow escapes the fn (callers pass it through a 'static page-builder closure)
fn drag_probe_page(moves: &Rc<Cell<u32>>, cancelled: &Rc<Cell<bool>>) -> AnyView<()> {
    any(DragProbe {
        moves: moves.clone(),
        cancelled: cancelled.clone(),
    })
}

// --- An edge drag past slop steals from a capturing child. ---

#[test]
fn edge_drag_steals_from_capturing_child() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let moves = Rc::new(Cell::new(0u32));
    let cancelled = Rc::new(Cell::new(false));
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();

    // Push a capturing page B on top (instant — default transition is NONE).
    {
        let m = moves.clone();
        let c = cancelled.clone();
        controller.push(move || drag_probe_page(&m, &c));
    }
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    // Down at the left edge: B captures (mid-press); nothing cancelled yet.
    root.event(&mut state, &down(5.0, 50.0));
    assert!(!cancelled.get(), "no cancel before the steal");

    // A rightward drag past the slop steals: B receives a synthetic Cancel and
    // no further moves; the navigator now drives an interactive pop.
    root.paint(&mut scene, ft(16));
    root.event(&mut state, &move_to(40.0, 50.0));
    assert!(
        cancelled.get(),
        "the mid-press child got a synthetic Cancel"
    );
    let moves_at_steal = moves.get();
    assert!(
        nav_widget(&root).transition.is_some(),
        "an interactive pop began"
    );
    assert!(
        nav_widget(&root).edge.active,
        "the navigator drives the swipe"
    );

    // Subsequent drag moves drive the pop and never reach B.
    root.paint(&mut scene, ft(32));
    root.event(&mut state, &move_to(60.0, 50.0));
    assert_eq!(
        moves.get(),
        moves_at_steal,
        "B receives no moves after the steal"
    );
}

// --- Drag moves pages, origins tracking progress. ---

#[test]
fn edge_drag_moves_pages_tracking_progress() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    controller.push(|| sized_page(100.0, 60.0)); // B, instant
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    // Down at the edge, then drag to x=40 (progress 0.35): B (leaving, iOS-pop)
    // sits at dx = progress * width.
    root.event(&mut state, &down(5.0, 50.0));
    root.paint(&mut scene, ft(16));
    root.event(&mut state, &move_to(40.0, 50.0)); // steal, progress (40-5)/100
    let (f1, _) = full_frame(&mut root, &mut app, &mut state, ft(24));
    let b1 = fill_h(&f1, 60.0).0.x;
    assert!(
        (b1 - 35.0).abs() < 1e-6,
        "B tracks drag progress (x was {b1})"
    );

    // Drag further right → B's origin advances with progress.
    root.event(&mut state, &move_to(70.0, 50.0)); // progress (70-5)/100 = 0.65
    let (f2, _) = full_frame(&mut root, &mut app, &mut state, ft(40));
    let b2 = fill_h(&f2, 60.0).0.x;
    assert!(
        (b2 - 65.0).abs() < 1e-6,
        "B follows the finger (x was {b2})"
    );
    assert!(
        b2 > b1,
        "the popped page moves right as the drag progresses"
    );
}

// --- Release past the halfway point completes the pop. ---

#[test]
fn release_past_half_completes_pop() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    controller.push(|| sized_page(100.0, 60.0)); // B, instant
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    // Slow drag (100 ms apart → low velocity) past the halfway commit point.
    root.event(&mut state, &down(5.0, 50.0));
    root.paint(&mut scene, ft(100));
    root.event(&mut state, &move_to(70.0, 50.0)); // progress 0.65, velocity ~650 px/s
    root.paint(&mut scene, ft(200));
    root.event(&mut state, &up(70.0, 50.0)); // >0.5 → complete

    // After settle the pop completed: only page A (height 100) remains.
    let fills = run_until_settled(&mut root, &mut app, &mut state, 300);
    assert_eq!(
        fills.len(),
        1,
        "the pop completed — stack shrank to one page"
    );
    assert!(
        (fills[0].1.height - 100.0).abs() < 1e-9,
        "the surviving page is A"
    );
    assert_eq!(
        nav_widget(&root).pages.len(),
        1,
        "the retained stack shrank"
    );
    assert!(
        nav_widget(&root).transition.is_none(),
        "the transition finalized"
    );
}

// --- An interactive edge-swipe pop drives the popped page's own
//     `Custom` preset RAW, bypassing `resolve_spec`'s `reduce_motion`
//     collapse — unlike a programmatic push/pop. This lives here (not in
//     `transition.rs`'s test module) because driving an interactive pop
//     needs this module's private test harness (`NavigatorController`,
//     `down`/`move_to`, `full_frame`, `nav_widget`) — the interactive-pop
//     path is not reachable from `transition.rs` at all. See
//     `PageTransition::Custom`'s doc comment and
//     `transition.rs`'s `custom_preset_collapses_under_reduce_motion_on_the_resolve_spec_path`
//     for the contrasting programmatic-path case. ---

#[test]
fn interactive_pop_calls_custom_fn_under_reduce_motion() {
    // A plain `fn` pointer can't capture, so invocation is recorded
    // through a process-static flag — fine here since this exact
    // function is only ever installed/read by this one test.
    static CALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    fn recording_custom(_p: f64, _is_pop: bool, _size: Size) -> (Layer, Layer) {
        CALLED.store(true, std::sync::atomic::Ordering::SeqCst);
        (Layer::IDENTITY, Layer::IDENTITY)
    }

    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut reduced = Theme::neutral();
    reduced.motion.reduce_motion = true;
    root.set_theme(Box::new(reduced));
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Push B with a `Custom` transition (explicit duration, so the
    // collapse below is attributable to `reduce_motion` alone). This
    // push's own animation still resolves through `resolve_spec` at its
    // first paint (the `pending_spec` seam `paint_transition` drains)
    // and collapses to `ReducedCrossfade` as documented — the caller's
    // fn must NOT run for the push itself.
    controller.push_with(
        || sized_page(100.0, 80.0),
        TransitionSpec::new(
            PageTransition::Custom(recording_custom),
            Timing::Duration(Duration::from_millis(80), Curve::Linear),
        ),
    );
    run_until_settled(&mut root, &mut app, &mut state, 0);
    assert!(
        !CALLED.load(std::sync::atomic::Ordering::SeqCst),
        "the programmatic push collapsed to ReducedCrossfade — Custom's fn must not run"
    );
    assert!(
        nav_widget(&root).transition.is_none(),
        "the push transition finalized before the swipe begins"
    );

    // Drive an interactive edge-swipe pop of B. `begin_interactive_pop`
    // takes `spec.preset` (`Custom`) RAW and sets `pending_spec: None`,
    // so `resolve_spec`'s `reduce_motion` collapse never runs on this
    // path — `paint_transition` calls `resolve_layers` with the
    // unresolved `Custom` preset directly.
    let mut scene = TransitionScene::default();
    root.event(&mut state, &down(5.0, 50.0));
    root.paint(&mut scene, ft(1000));
    root.event(&mut state, &move_to(40.0, 50.0)); // past slop -> steals
    assert!(
        nav_widget(&root).transition.is_some(),
        "an interactive pop began"
    );

    full_frame(&mut root, &mut app, &mut state, ft(1016));
    assert!(
        CALLED.load(std::sync::atomic::Ordering::SeqCst),
        "an interactive edge-swipe pop drives the popped page's own Custom \
         preset directly under reduce_motion — the caller's fn IS called"
    );
}

// --- A completed swipe delivers the pop result. ---

#[test]
fn swipe_complete_delivers_result_to_callback() {
    let controller: NavigatorController<SwipeResultState> = NavigatorController::new();
    let mut root: RenderRoot<SwipeResultState, NavigatorView<SwipeResultState>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut SwipeResultState| {
            navigator(&ctrl, || SizedLeaf {
                size: Size::new(100.0, 100.0),
            })
            .pop_swipe(true)
        }
    };
    let mut state = SwipeResultState::default();

    // Push B (instant) registering a result callback fired on its pop.
    controller.push_for_result(
        || SizedLeaf {
            size: Size::new(100.0, 60.0),
        },
        |state: &mut SwipeResultState, _result: PopResult| {
            state.popped = true;
        },
    );
    let mut sink = RecordingScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut sink, FrameTime::ZERO);

    // Swipe across and release past the commit point → complete.
    root.event(&mut state, &down(5.0, 50.0));
    root.paint(&mut sink, ft(100));
    root.event(&mut state, &move_to(80.0, 50.0));
    root.paint(&mut sink, ft(200));
    root.event(&mut state, &up(80.0, 50.0));

    // Drive to settle/finalize. The callback is queued at finalize (a
    // `BuildCtx` pass) and flushed by that same rebuild's housekeeping
    // broadcast, so it lands on the settle frame with no further input.
    assert!(!state.popped, "not delivered before the swipe settles");
    for t in [300u64, 316, 332, 348, 400, 500, 800, 1200, 2000] {
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut sink, ft(t));
    }
    assert!(
        state.popped,
        "the completed swipe delivered its pop result on the settle frame's \
         rebuild, with no input event"
    );
}

// --- Release below threshold cancels; page restored exactly. ---

#[test]
fn release_below_threshold_cancels_and_restores() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    controller.push(|| sized_page(100.0, 60.0)); // B, instant
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    // Small, slow drag (well under half, low velocity) then release → cancel.
    root.event(&mut state, &down(5.0, 50.0));
    root.paint(&mut scene, ft(200));
    root.event(&mut state, &move_to(30.0, 50.0)); // progress 0.25 (< 0.5)
    root.paint(&mut scene, ft(400));
    root.event(&mut state, &up(30.0, 50.0)); // cancel

    // After settle the pop was cancelled: page B (height 60) is restored on top
    // at exact resting geometry (origin ZERO), the stack is unchanged (depth 2).
    let fills = run_until_settled(&mut root, &mut app, &mut state, 500);
    assert_eq!(
        fills.len(),
        1,
        "cancelled pop: only the opaque top page paints"
    );
    assert!(
        (fills[0].1.height - 60.0).abs() < 1e-9,
        "page B was restored on top"
    );
    assert_eq!(
        fills[0].0,
        Point::ZERO,
        "restored page sits at exact resting origin"
    );
    assert_eq!(nav_widget(&root).pages.len(), 2, "the stack is unchanged");
    assert!(
        nav_widget(&root).transition.is_none(),
        "the transition finalized"
    );
}

// --- A low-progress high-velocity release completes the pop. ---

#[test]
fn low_progress_high_velocity_release_completes() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    controller.push(|| sized_page(100.0, 60.0)); // B, instant
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    // Fast flick: x 5→25 in 10 ms ≈ 2000 px/s, but progress only 0.20 (< 0.5).
    root.event(&mut state, &down(5.0, 50.0));
    root.paint(&mut scene, ft(10));
    root.event(&mut state, &move_to(25.0, 50.0)); // steal, progress 0.20
    assert!(
        nav_widget(&root)
            .transition
            .as_ref()
            .map(|t| t.driver.value() < EDGE_SWIPE_COMMIT_PROGRESS)
            .unwrap_or(false),
        "progress is below the commit threshold at release"
    );
    root.paint(&mut scene, ft(20));
    root.event(&mut state, &up(25.0, 50.0)); // low progress, high velocity → complete

    let fills = run_until_settled(&mut root, &mut app, &mut state, 100);
    assert_eq!(fills.len(), 1, "the fast flick completed the pop");
    assert!(
        (fills[0].1.height - 100.0).abs() < 1e-9,
        "the surviving page is A"
    );
    assert_eq!(
        nav_widget(&root).pages.len(),
        1,
        "the stack shrank to one page"
    );
}

// --- A system Cancel mid-drag takes the cancel path (page restored). ---

#[test]
fn system_cancel_mid_drag_cancels_pop() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    controller.push(|| sized_page(100.0, 60.0)); // B, instant
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    // Steal an interactive pop, then a system Cancel arrives mid-drag: the pop
    // cancels and the page is restored (no application-state access — a Cancel
    // arm must never touch state).
    root.event(&mut state, &down(5.0, 50.0));
    root.paint(&mut scene, ft(100));
    root.event(&mut state, &move_to(70.0, 50.0)); // steal, progress 0.65
    assert!(nav_widget(&root).edge.active, "the swipe is driving");
    root.paint(&mut scene, ft(200));
    root.event(&mut state, &cancel_ev(70.0, 50.0)); // system gesture steal
    assert!(
        !nav_widget(&root).edge.active,
        "the cancel released the drive"
    );

    // After settle the pop was cancelled: page B is restored on top (depth 2).
    let fills = run_until_settled(&mut root, &mut app, &mut state, 300);
    assert_eq!(
        fills.len(),
        1,
        "cancelled pop leaves the opaque top painting"
    );
    assert!(
        (fills[0].1.height - 60.0).abs() < 1e-9,
        "page B was restored"
    );
    assert_eq!(nav_widget(&root).pages.len(), 2, "the stack is unchanged");
}

// --- A non-edge drag never arms the gesture. ---

#[test]
fn non_edge_down_never_arms() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    controller.push(|| sized_page(100.0, 60.0));
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    // A Down well inside the page (x=50, past the ~20px edge zone) never arms.
    root.event(&mut state, &down(50.0, 50.0));
    assert!(
        !nav_widget(&root).edge.armed,
        "a non-edge Down does not arm"
    );
    root.paint(&mut scene, ft(16));
    root.event(&mut state, &move_to(90.0, 50.0)); // large rightward drag
    assert!(
        !nav_widget(&root).edge.active,
        "no steal from a non-edge drag"
    );
    assert!(
        nav_widget(&root).transition.is_none(),
        "no interactive pop began"
    );
}

// --- A vertical drag starting in the edge zone stays with the
//     page (a ScrollView child scrolls normally). ---

#[test]
fn vertical_drag_in_edge_zone_stays_with_page() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let moves = Rc::new(Cell::new(0u32));
    let cancelled = Rc::new(Cell::new(false));
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    {
        let m = moves.clone();
        let c = cancelled.clone();
        controller.push(move || drag_probe_page(&m, &c));
    }
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    // Down in the edge zone (arms), then a vertical drag: the arm releases and
    // the page keeps the gesture — the child keeps receiving moves, no Cancel.
    root.event(&mut state, &down(5.0, 30.0));
    assert!(nav_widget(&root).edge.armed, "edge-zone Down arms");
    root.paint(&mut scene, ft(16));
    root.event(&mut state, &move_to(5.0, 70.0)); // vertical → disarm, yield
    root.paint(&mut scene, ft(32));
    root.event(&mut state, &move_to(5.0, 100.0)); // stays with the page

    assert!(
        !cancelled.get(),
        "a vertical drag never steals from the page"
    );
    assert!(moves.get() >= 1, "the page keeps receiving the drag moves");
    assert!(
        !nav_widget(&root).edge.active,
        "no interactive pop for a vertical drag"
    );
    assert!(nav_widget(&root).transition.is_none());
}

// --- A depth-1 stack disables the gesture. ---

#[test]
fn depth_one_stack_disables_gesture() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    // A single-page stack: an edge Down never arms (nothing to pop back to).
    root.event(&mut state, &down(5.0, 50.0));
    assert!(!nav_widget(&root).edge.armed, "depth-1 stack: no arm");
    root.paint(&mut scene, ft(16));
    root.event(&mut state, &move_to(60.0, 50.0));
    assert!(
        nav_widget(&root).transition.is_none(),
        "no interactive pop at depth 1"
    );
}

// --- Regression: a programmatic instant pop between an edge-swipe arm
//     and its steal must not empty the page stack. The arm is captured at
//     depth 2; a default (non-animated) pop applied at the next rebuild
//     shrinks the stack to the root page and never runs a transition (so the
//     `finalize_transition` arm-clearing never fires); a decisive rightward
//     Move must then NOT steal an interactive pop against the now-depth-1
//     stack — which, in a release build (where `begin_interactive_pop`'s only
//     depth guard is a compiled-out `debug_assert!`), would pop the root page
//     and leave zero pages. ---

#[test]
fn programmatic_pop_between_arm_and_steal_does_not_empty_stack() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    controller.push(|| sized_page(100.0, 60.0)); // B, instant → depth 2
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    // Arm the gesture on an edge Down at depth 2.
    root.event(&mut state, &down(5.0, 50.0));
    assert!(
        nav_widget(&root).edge.armed,
        "an edge-zone Down arms at depth 2"
    );

    // A programmatic instant (default non-animated) pop lands at the next
    // rebuild, shrinking the stack to the root page — and disarming the stale
    // edge gesture as a structural mutation, with no transition to clear it.
    controller.pop();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(16));
    assert_eq!(
        nav_widget(&root).pages.len(),
        1,
        "the pop shrank the stack to the root page"
    );
    assert!(
        !nav_widget(&root).edge.armed,
        "the structural pop disarmed the stale edge gesture"
    );

    // A decisive rightward Move past the slop must NOT steal an interactive
    // pop on the now-depth-1 stack (which would empty it).
    root.event(&mut state, &move_to(60.0, 50.0));
    assert!(
        nav_widget(&root).transition.is_none(),
        "no interactive pop was stolen on the depth-1 stack"
    );
    assert_eq!(
        nav_widget(&root).pages.len(),
        1,
        "the root page is intact — the stack was never emptied"
    );
    assert!(
        !nav_widget(&root).edge.armed,
        "the stale arm did not survive"
    );
}

// --- Pop-swipe defaults on for the iOS-push preset. ---

#[test]
fn pop_swipe_defaults_on_for_ios_preset() {
    // Default transition IosPush → gesture enabled without an explicit flag.
    let ios = NavigatorController::<()>::new();
    let mut root_ios: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app_ios = {
        let ctrl = ios.clone();
        move |_: &mut ()| {
            navigator(&ctrl, || sized_page(10.0, 10.0))
                .transition(TransitionSpec::duration(PageTransition::IosPush))
        }
    };
    let mut s = ();
    root_ios.rebuild(&mut app_ios, &mut s);
    assert!(
        nav_widget(&root_ios).pop_swipe_enabled,
        "iOS-push default enables the pop-swipe"
    );

    // Default transition NONE → gesture off unless explicitly enabled.
    let plain = NavigatorController::<()>::new();
    let mut root_plain: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app_plain = {
        let ctrl = plain.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
    };
    let mut s2 = ();
    root_plain.rebuild(&mut app_plain, &mut s2);
    assert!(
        !nav_widget(&root_plain).pop_swipe_enabled,
        "the default (instant) preset leaves the pop-swipe off"
    );
}

// ---------------------------------------------------------------------
// Gesture policy: BackPolicy arm gate, per-route/platform pop_swipe
// resolution, and R-B3-inner (innermost-first swipe claiming).
// ---------------------------------------------------------------------

/// Downcast a page's own `ChildPod` widget to `&NavigatorWidget<S>` — the
/// nested-navigator analog of [`nav_widget`], for R-B3-inner tests that
/// must inspect an INNER navigator's private edge state directly (its
/// `NavigatorController` exposes `depth`/`transition`/`back_interest`,
/// none of which distinguish "armed" from "never tried"). `AnyView`'s
/// erasure keeps the concrete widget type reachable through
/// `ChildPod::widget` with no extra unwrap layer (see `AnyView::Element`'s
/// doc), so this holds whenever a page's view is a bare
/// `navigator(...)`/`overlay_host(...)` call with no wrapping container.
/// Panics otherwise.
///
/// `authoring::build_child`'s `ChildPod` stores an `AnyView`'s element
/// **double-boxed** (its own doc comment: "so a later `rebuild_child` can
/// recover it as `&mut Box<dyn Widget>`"), so `pod.widget()`'s concrete
/// runtime type is `Box<dyn Widget>` wrapping the real widget, not the
/// real widget directly — one extra downcast layer versus `nav_widget`'s
/// root pod, which the tree's own `insert_root` stores single-boxed.
fn inner_nav_widget<S: 'static>(outer: &NavigatorWidget<S>) -> &NavigatorWidget<S> {
    let top = outer.pages.last().expect("outer has a top page");
    let boxed = (top.pod.widget() as &dyn Any)
        .downcast_ref::<Box<dyn Widget>>()
        .expect("AnyView-erased page: double-boxed ChildPod element");
    (boxed.as_ref() as &dyn Any)
        .downcast_ref::<NavigatorWidget<S>>()
        .expect("outer's top page hosts a nested NavigatorWidget directly")
}

// --- §4.1: BackPolicy gates the arm — Pop arms, DismissAnimated/Veto
//     refuse (arm-refusal, not a dismiss-signal bump). ---

#[test]
fn pop_policy_top_arms_the_gesture() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    controller.push(|| sized_page(100.0, 60.0)); // default BackPolicy::Pop
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    root.event(&mut state, &down(5.0, 50.0));
    assert!(
        nav_widget(&root).edge.armed,
        "the ordinary Pop policy still arms"
    );
}

#[test]
fn dismiss_animated_top_never_arms_the_gesture() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    let dismiss_signal = Rc::new(Cell::new(0u64));
    {
        let signal = dismiss_signal.clone();
        controller.push_with_options(
            || sized_page(100.0, 60.0),
            PushOptions::opaque()
                .back(BackPolicy::DismissAnimated)
                .dismiss_signal(signal),
        );
    }
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    // A left-edge Down over a poppable stack whose top is DismissAnimated
    // must not arm — a swipe scrubs a real pop in reverse, and this page
    // does not pop on back.
    root.event(&mut state, &down(5.0, 50.0));
    assert!(
        !nav_widget(&root).edge.armed,
        "a DismissAnimated top refuses to arm"
    );
    root.paint(&mut scene, ft(16));
    root.event(&mut state, &move_to(60.0, 50.0));
    assert!(
        nav_widget(&root).transition.is_none(),
        "no interactive pop began"
    );
    assert_eq!(
        dismiss_signal.get(),
        0,
        "arm-refusal, not a dismiss-signal bump — refusing to arm leaves the \
         whole pointer stream with the page instead of stealing and staging \
         a lying scrub"
    );
}

#[test]
fn veto_top_never_arms_the_gesture() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    controller.push_with_options(
        || sized_page(100.0, 60.0),
        PushOptions::opaque().back(BackPolicy::Veto),
    );
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    root.event(&mut state, &down(5.0, 50.0));
    assert!(!nav_widget(&root).edge.armed, "a Veto top refuses to arm");
}

// --- Steal-site re-check (§4.1): mirrors the existing depth-recheck
//     regression above — a DismissAnimated push landing between the arm's
//     Down and the decisive Move must retract the arm end-to-end. ---

#[test]
fn dismiss_animated_push_between_arm_and_move_disarms_at_the_steal_site() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    controller.push(|| sized_page(100.0, 60.0)); // B, Pop — depth 2
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    // Arm against B's Pop policy.
    root.event(&mut state, &down(5.0, 50.0));
    assert!(nav_widget(&root).edge.armed, "arms against B's Pop policy");

    // A DismissAnimated page lands on top before the decisive Move — the
    // stack stays poppable (now depth 3), but the CURRENT top's policy
    // changed underneath the stale arm.
    let dismiss_signal = Rc::new(Cell::new(0u64));
    {
        let signal = dismiss_signal.clone();
        controller.push_with_options(
            || sized_page(100.0, 40.0),
            PushOptions::opaque()
                .back(BackPolicy::DismissAnimated)
                .dismiss_signal(signal),
        );
    }
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    assert_eq!(
        nav_widget(&root).pages.len(),
        3,
        "now poppable to a DismissAnimated top"
    );

    // The decisive rightward Move must NOT steal.
    root.paint(&mut scene, ft(16));
    root.event(&mut state, &move_to(60.0, 50.0));
    assert!(
        nav_widget(&root).transition.is_none(),
        "the stale arm did not steal against the new DismissAnimated top"
    );
    assert_eq!(
        nav_widget(&root).pages.len(),
        3,
        "the stack is untouched — no steal, no pop"
    );
    assert_eq!(dismiss_signal.get(), 0);
}

// --- §4.3: platform_pop_swipe's second slot resolves navigator explicit
//     > platform > preset. ---

#[test]
fn platform_pop_swipe_resolution_order() {
    // Platform slot set, no explicit override → platform beats the
    // preset-derived default.
    let platform_on: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = platform_on.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0)).platform_pop_swipe(true)
    };
    root.rebuild(&mut app, &mut ());
    assert!(
        nav_widget(&root).pop_swipe_enabled,
        "the platform slot beats the (off) preset-derived default"
    );

    // An explicit navigator override beats a contradicting platform slot.
    let explicit_wins: NavigatorController<()> = NavigatorController::new();
    let mut root2: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app2 = {
        let ctrl = explicit_wins.clone();
        move |_: &mut ()| {
            navigator(&ctrl, || sized_page(10.0, 10.0))
                .platform_pop_swipe(true)
                .pop_swipe(false)
        }
    };
    root2.rebuild(&mut app2, &mut ());
    assert!(
        !nav_widget(&root2).pop_swipe_enabled,
        "an explicit navigator override outranks the platform slot"
    );

    // `overlay_host`'s explicit `pop_swipe(false)` outranks the platform
    // slot the same way, unconditionally — an edge swipe must never
    // dismiss an overlay, on any platform.
    let host: NavigatorController<()> = NavigatorController::new();
    let host_view = overlay_host(&host, || sized_page(10.0, 10.0)).platform_pop_swipe(true);
    assert!(
        !host_view.resolve_pop_swipe(),
        "overlay_host still refuses even with the platform slot set"
    );
}

// --- §4.4: PushOptions::pop_swipe rides the options as a per-route
//     override, outranking everything else. ---

#[test]
fn push_options_pop_swipe_overrides_the_navigators_resolved_default() {
    // Navigator resolves OFF (no platform slot, NONE preset), but the
    // pushed page opts in explicitly — the highest-ranked slot.
    let opt_in: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = opt_in.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    opt_in.push_with_options(
        || sized_page(100.0, 60.0),
        PushOptions::opaque().pop_swipe(true),
    );
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));
    assert!(
        !nav_widget(&root).pop_swipe_enabled,
        "the navigator's own resolved default is off"
    );
    root.event(&mut state, &down(5.0, 50.0));
    assert!(
        nav_widget(&root).edge.armed,
        "the page's own pop_swipe(true) override wins"
    );

    // The opposite: navigator resolves ON, page opts out.
    let opt_out: NavigatorController<()> = NavigatorController::new();
    let mut root2: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app2 = {
        let ctrl = opt_out.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state2 = ();
    opt_out.push_with_options(
        || sized_page(100.0, 60.0),
        PushOptions::opaque().pop_swipe(false),
    );
    let mut scene2 = TransitionScene::default();
    root2.rebuild(&mut app2, &mut state2);
    root2.layout(Size::new(100.0, 100.0));
    root2.paint(&mut scene2, ft(0));
    assert!(
        nav_widget(&root2).pop_swipe_enabled,
        "navigator resolves on"
    );
    root2.event(&mut state2, &down(5.0, 50.0));
    assert!(
        !nav_widget(&root2).edge.armed,
        "the page's own pop_swipe(false) override wins"
    );
}

// --- §4.2, R-B3-inner: innermost-first swipe claiming. ---

#[test]
fn r_b3_inner_single_navigator_is_unaffected_and_still_steals() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    controller.push(|| sized_page(100.0, 60.0));
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    root.event(&mut state, &down(2.0, 50.0));
    assert!(nav_widget(&root).edge.armed, "arms as before R-B3-inner");
    assert!(
        !nav_widget(&root).edge.inner_claimed,
        "no navigator below it, so nothing claims — the degenerate \
         single-navigator case `inner_claimed == false` always holds"
    );

    root.paint(&mut scene, ft(16));
    root.event(&mut state, &move_to(60.0, 50.0));
    assert!(
        nav_widget(&root).transition.is_some(),
        "with no claim, the navigator steals exactly as before R-B3-inner"
    );
}

#[test]
fn r_b3_inner_both_arm_on_down_and_the_outer_defers_at_move() {
    let outer: NavigatorController<()> = NavigatorController::new();
    let inner: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app: AppLogic = {
        let outer_c = outer.clone();
        Box::new(move |_: &mut ()| navigator(&outer_c, || sized_page(100.0, 100.0)).pop_swipe(true))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // The outer's root is a plain leaf; push a page that hosts the inner
    // navigator DIRECTLY (no wrapping container) — the outer becomes
    // poppable while that page stays current.
    {
        let inner_c = inner.clone();
        outer.push(move || navigator(&inner_c, || sized_page(100.0, 100.0)).pop_swipe(true));
    }
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    assert_eq!(outer.depth(), 2, "outer is now poppable");

    // The inner navigator must ALSO be poppable to legitimately arm.
    inner.push(|| sized_page(100.0, 100.0));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    assert_eq!(inner.depth(), 2, "inner is now poppable too");

    let mut scene = TransitionScene::default();
    root.paint(&mut scene, ft(0));

    // A single left-edge Down: BOTH navigators arm — mechanism, not a bug
    // (the outer forwards it through `route_top`, so the inner's own
    // `event_at` runs underneath and arms independently).
    root.event(&mut state, &down(2.0, 50.0));
    assert!(nav_widget(&root).edge.armed, "outer arms on Down");
    assert!(
        inner_nav_widget(nav_widget(&root)).edge.armed,
        "inner ALSO arms on the same Down"
    );
    assert!(
        nav_widget(&root).edge.inner_claimed,
        "the outer recorded the inner's claim"
    );

    // The decisive Move: the outer's steal branch defers to the claim; the
    // inner's steals instead.
    root.paint(&mut scene, ft(16));
    root.event(&mut state, &move_to(60.0, 50.0));
    assert!(
        nav_widget(&root).transition.is_none(),
        "the outer did NOT steal — it deferred to the inner"
    );
    assert!(
        !nav_widget(&root).edge.armed,
        "the outer's own arm was dropped at the defer"
    );
    assert!(
        inner_nav_widget(nav_widget(&root)).transition.is_some(),
        "the inner DID steal and began its own interactive pop"
    );
}

#[test]
fn r_b3_inner_three_level_chain_defers_to_the_innermost() {
    let outer: NavigatorController<()> = NavigatorController::new();
    let middle: NavigatorController<()> = NavigatorController::new();
    let inner: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app: AppLogic = {
        let outer_c = outer.clone();
        Box::new(move |_: &mut ()| navigator(&outer_c, || sized_page(100.0, 100.0)).pop_swipe(true))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // outer → middle: middle hosted directly on outer's pushed top page.
    {
        let middle_c = middle.clone();
        outer.push(move || navigator(&middle_c, || sized_page(100.0, 100.0)).pop_swipe(true));
    }
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    assert_eq!(outer.depth(), 2);

    // middle → inner: inner hosted directly on middle's pushed top page.
    {
        let inner_c = inner.clone();
        middle.push(move || navigator(&inner_c, || sized_page(100.0, 100.0)).pop_swipe(true));
    }
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    assert_eq!(middle.depth(), 2);

    // inner needs its own depth 2 to legitimately arm.
    inner.push(|| sized_page(100.0, 100.0));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    assert_eq!(inner.depth(), 2);

    let mut scene = TransitionScene::default();
    root.paint(&mut scene, ft(0));

    root.event(&mut state, &down(2.0, 50.0));
    {
        let outer_w = nav_widget(&root);
        assert!(outer_w.edge.armed, "outer arms");
        let middle_w = inner_nav_widget(outer_w);
        assert!(middle_w.edge.armed, "middle arms");
        let inner_w = inner_nav_widget(middle_w);
        assert!(inner_w.edge.armed, "inner arms");
        assert!(
            outer_w.edge.inner_claimed,
            "outer's claim propagated up from below"
        );
        assert!(
            middle_w.edge.inner_claimed,
            "middle's own claim recorded from inner, one level at a time"
        );
    }

    root.paint(&mut scene, ft(16));
    root.event(&mut state, &move_to(60.0, 50.0));
    {
        let outer_w = nav_widget(&root);
        assert!(outer_w.transition.is_none(), "outer deferred");
        let middle_w = inner_nav_widget(outer_w);
        assert!(middle_w.transition.is_none(), "middle deferred too");
        let inner_w = inner_nav_widget(middle_w);
        assert!(inner_w.transition.is_some(), "only the innermost stole");
    }
}

#[test]
fn r_b3_inner_claim_clears_on_cancel_so_a_later_swipe_works() {
    let outer: NavigatorController<()> = NavigatorController::new();
    let inner: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app: AppLogic = {
        let outer_c = outer.clone();
        Box::new(move |_: &mut ()| navigator(&outer_c, || sized_page(100.0, 100.0)).pop_swipe(true))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    {
        let inner_c = inner.clone();
        outer.push(move || navigator(&inner_c, || sized_page(100.0, 100.0)).pop_swipe(true));
    }
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    inner.push(|| sized_page(100.0, 100.0));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let mut scene = TransitionScene::default();
    root.paint(&mut scene, ft(0));

    // Arm both, then a system Cancel arrives before any decisive Move —
    // e.g. a platform gesture stole the whole pointer stream.
    root.event(&mut state, &down(2.0, 50.0));
    assert!(
        nav_widget(&root).edge.inner_claimed,
        "the outer recorded the inner's claim"
    );
    root.event(&mut state, &cancel_ev(2.0, 50.0));
    assert!(
        !nav_widget(&root).edge.armed,
        "Cancel releases the outer's own arm"
    );
    assert!(
        !nav_widget(&root).edge.inner_claimed,
        "and clears the R-B3-inner claim with the rest of the edge state — \
         not left stale for whatever swipe comes next"
    );

    // A LATER, independent swipe — the inner popped back to depth 1, so it
    // no longer arms — must steal normally at the outer.
    inner.pop();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(32));

    root.event(&mut state, &down(2.0, 50.0));
    assert!(nav_widget(&root).edge.armed, "outer re-arms");
    assert!(
        !nav_widget(&root).edge.inner_claimed,
        "the inner (now depth 1) does not arm, so nothing claims this time"
    );
    root.paint(&mut scene, ft(48));
    root.event(&mut state, &move_to(60.0, 50.0));
    assert!(
        nav_widget(&root).transition.is_some(),
        "the later swipe steals normally — not blocked by a stale claim"
    );
}
