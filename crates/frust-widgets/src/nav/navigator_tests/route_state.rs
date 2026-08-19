//! Route-state observable: `NavigatorController::route_stack()`/
//! `route_generation()` and the diffed `NavChange` published from every
//! committed-mutation site, including the uncommitted-interactive-swipe
//! contract.

use super::super::*;
use super::support::*;
use crate::nav::route_state::NavChange;
use frust_core::RenderRoot;

// ---------------------------------------------------------------------
// Route-state observable.
// ---------------------------------------------------------------------

// --- (a) push/push/pop publishes [A]→[A,B]→[A] with Push/Pop. ---

#[test]
fn push_pop_publishes_the_route_stack_with_derived_change() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).root_route(route("/a"))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);

    let initial = controller.route_stack();
    assert_eq!(initial.entries(), &[Some(route("/a"))]);
    assert_eq!(initial.change(), NavChange::Initial);
    let gen0 = initial.generation();

    controller.push_with_options(
        || sized_page(50.0, 50.0),
        PushOptions::opaque().route(route("/b")),
    );
    root.rebuild(&mut app, &mut state);
    let after_push = controller.route_stack();
    assert_eq!(
        after_push.entries(),
        &[Some(route("/a")), Some(route("/b"))]
    );
    assert_eq!(after_push.change(), NavChange::Push);
    assert!(after_push.generation() > gen0);

    controller.pop();
    root.rebuild(&mut app, &mut state);
    let after_pop = controller.route_stack();
    assert_eq!(after_pop.entries(), &[Some(route("/a"))]);
    assert_eq!(after_pop.change(), NavChange::Pop);
    assert!(after_pop.generation() > after_push.generation());
}

// --- (b) a `request_back` pop publishes identically to a programmatic pop
//     — the case the #40 mirror class missed. ---

#[test]
fn request_back_pop_publishes_identically_to_a_programmatic_pop() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).root_route(route("/a"))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    controller.push_with_options(
        || sized_page(50.0, 50.0),
        PushOptions::opaque().route(route("/b")),
    );
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        controller.route_stack().entries(),
        &[Some(route("/a")), Some(route("/b"))]
    );

    controller.request_back();
    root.rebuild(&mut app, &mut state);

    let stack = controller.route_stack();
    assert_eq!(
        stack.entries(),
        &[Some(route("/a"))],
        "request_back's Pop routing publishes the same way apply_pop does"
    );
    assert_eq!(stack.change(), NavChange::Pop);
}

// --- (c) an unchanged stack across 10 rebuilds bumps the generation once. ---

#[test]
fn an_unchanged_stack_across_rebuilds_bumps_the_generation_once() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).root_route(route("/a"))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    let generation_after_first_build = controller.route_generation();

    for _ in 0..10 {
        root.rebuild(&mut app, &mut state);
    }

    assert_eq!(
        controller.route_generation(),
        generation_after_first_build,
        "10 rebuilds with no stack mutation must not re-bump the generation"
    );
}

// --- (d) a builder-pushed overlay publishes a `None` entry; `current_route`
//     skips it — the structural form of muxr's
//     "a transparent overlay never retitles the bar". ---

#[test]
fn a_routeless_overlay_publishes_none_and_current_route_skips_it() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).root_route(route("/a"))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);

    // A bare-builder overlay push (a dialog) — no `.route(...)` call.
    controller.push_with_options(|| sized_page(50.0, 50.0), PushOptions::transparent());
    root.rebuild(&mut app, &mut state);

    let stack = controller.route_stack();
    assert_eq!(
        stack.entries(),
        &[Some(route("/a")), None],
        "an overlay pushed as a bare builder publishes a `None` entry"
    );
    assert_eq!(
        stack.current(),
        None,
        "the raw top is the overlay itself, which carries no route"
    );
    assert_eq!(
        stack.current_route().map(|l| l.path.as_str()),
        Some("/a"),
        "chrome reading current_route sees the routed page underneath, unretitled"
    );
}

// --- (b') the uncommitted-swipe contract, both ways. ---

#[test]
fn interactive_swipe_mid_drag_is_uncommitted_then_publishes_on_settle() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| {
            navigator(&ctrl, || sized_page(100.0, 100.0))
                .root_route(route("/a"))
                .pop_swipe(true)
        }
    };
    let mut state = ();
    controller.push_with_options(
        || sized_page(100.0, 60.0),
        PushOptions::opaque().route(route("/b")),
    );
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    let pre_drag = controller.route_stack();
    assert_eq!(pre_drag.entries(), &[Some(route("/a")), Some(route("/b"))]);
    let pre_drag_generation = pre_drag.generation();

    // Steal the gesture with a drag past slop, then hold mid-drag.
    root.event(&mut state, &down(5.0, 50.0));
    root.paint(&mut scene, ft(100));
    root.event(&mut state, &move_to(80.0, 50.0));
    root.paint(&mut scene, ft(116));

    // Mid-drag: the observable is UNCOMMITTED — still the pre-swipe stack,
    // same generation, no callback fired.
    let mid_drag = controller.route_stack();
    assert_eq!(
        mid_drag.entries(),
        pre_drag.entries(),
        "an in-flight interactive pop must not publish"
    );
    assert_eq!(
        mid_drag.generation(),
        pre_drag_generation,
        "mid-drag: the generation is untouched"
    );

    // Release past the commit point (progress 0.75 > 0.5) -> completes.
    root.event(&mut state, &up(80.0, 50.0));
    run_until_settled(&mut root, &mut app, &mut state, 200);

    let settled = controller.route_stack();
    assert_eq!(
        settled.entries(),
        &[Some(route("/a"))],
        "a completed swipe publishes on the settle frame"
    );
    assert_eq!(settled.change(), NavChange::Pop);
    assert!(
        settled.generation() > pre_drag_generation,
        "the settle-frame publish bumps the generation"
    );
}

#[test]
fn interactive_swipe_cancel_publishes_nothing_and_generation_is_untouched() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| {
            navigator(&ctrl, || sized_page(100.0, 100.0))
                .root_route(route("/a"))
                .pop_swipe(true)
        }
    };
    let mut state = ();
    controller.push_with_options(
        || sized_page(100.0, 60.0),
        PushOptions::opaque().route(route("/b")),
    );
    let mut scene = TransitionScene::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, ft(0));

    let pre_drag = controller.route_stack();
    let pre_drag_generation = pre_drag.generation();

    // Small, slow drag (well under half, low velocity) then release -> cancel.
    root.event(&mut state, &down(5.0, 50.0));
    root.paint(&mut scene, ft(200));
    root.event(&mut state, &move_to(30.0, 50.0)); // progress 0.25 (< 0.5)
    root.paint(&mut scene, ft(400));
    root.event(&mut state, &up(30.0, 50.0)); // cancel

    run_until_settled(&mut root, &mut app, &mut state, 500);

    let after = controller.route_stack();
    assert_eq!(
        after.entries(),
        pre_drag.entries(),
        "a cancelled swipe publishes nothing at all"
    );
    assert_eq!(
        after.generation(),
        pre_drag_generation,
        "generation is untouched end-to-end by a cancelled swipe"
    );
    assert_eq!(after.change(), pre_drag.change());
}
