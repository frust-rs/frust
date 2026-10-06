//! Back-request routing, per-page dismiss policy, `back_interest`/reach
//! composition through nested navigators, and the root overlay host's
//! (`overlay_host`) input-reach behavior.

use super::super::*;
use super::support::*;
use crate::nav::transition::{PageTransition, Timing, TransitionSpec};
use crate::test_support::RecordingScene;
use frust_core::Curve;
use frust_core::{FrameTime, RenderRoot, any};
use std::cell::Cell;
use std::time::Duration;

// ---------------------------------------------------------------------
// Back-request routing + per-page dismiss policy.
// ---------------------------------------------------------------------

// --- Pure: a navigator's OWN stack wants a press iff depth>1 OR the top
//     policy is not `Pop`. Tested directly so the depth-1-with-overlay case
//     (which `can_pop` cannot express) is covered without needing a depth-1
//     overlay through the push API. The R23 reach gate is deliberately NOT a
//     parameter here — it is ANDed on at read time by
//     `NavigatorController::back_interest`, which the next test pins. ---
#[test]
fn compute_back_interest_covers_depth_and_policy() {
    // Root only: not poppable, plain Pop policy -> no interest (a root back
    // must bubble to the platform).
    assert!(!compute_back_interest(1, BackPolicy::Pop));
    // Depth 2 (plain pages): poppable -> interest.
    assert!(compute_back_interest(2, BackPolicy::Pop));
    // Root + a Veto overlay: not poppable, but the overlay claims back.
    assert!(compute_back_interest(1, BackPolicy::Veto));
    // Root + a dismissable overlay: same — it wants the press to animate out.
    assert!(compute_back_interest(1, BackPolicy::DismissAnimated));
}

// --- R23: an unreachable host page vetoes that answer, at READ time. A
//     navigator whose hosting page input cannot reach must not claim the
//     press, whatever its own stack looks like — and it must stop claiming
//     it the instant the page goes unreachable, without waiting for a
//     rebuild that (under `cull_covered_builds`) may never come. This is the
//     controller-seam proof; the widget-level ones live further down. ---
#[test]
fn back_interest_is_vetoed_at_read_time_by_an_unreachable_host_page() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
    };
    let mut state = ();

    // A real, poppable stack at the top level: nothing gates it.
    root.rebuild(&mut app, &mut state);
    controller.push(|| sized_page(20.0, 20.0));
    root.rebuild(&mut app, &mut state);
    assert!(controller.back_interest(), "top level -> back interest");

    // Bind a hosting page input cannot reach. No rebuild in between: the
    // gate is read live, so the veto lands immediately.
    let reach = Rc::new(Cell::new(false));
    controller.bind_host_reach(Some(Rc::clone(&reach)));
    assert!(
        !controller.back_interest(),
        "an unreachable host page vetoes the press"
    );
    assert!(
        controller.back_interest.get(),
        "...while the PUBLISHED cell still reports the stack's own answer — \
         the reach gate is ANDed on at read time, never baked into the cell"
    );

    // Un-vetoes just as live, again with no rebuild — the frozen-page case.
    reach.set(true);
    assert!(controller.back_interest(), "revealed -> interest returns");

    // The gate is an AND over whatever the stack published, so it covers the
    // policy half too, including the depth-1 overlay shape the push API
    // cannot build (hence driving the published cell straight from the pure
    // helper above).
    controller
        .back_interest
        .set(compute_back_interest(1, BackPolicy::Veto));
    assert!(
        controller.back_interest(),
        "a reachable Veto overlay claims"
    );
    reach.set(false);
    assert!(
        !controller.back_interest(),
        "a Veto overlay on an unreachable page claims nothing either"
    );

    // And an unbound host (a top-level navigator) is unconditionally
    // reachable — a single-navigator app is untouched by the gate.
    controller.bind_host_reach(None);
    assert!(controller.back_interest(), "no hosting page -> no gate");
}

// --- The controller publishes back_interest
//     through a real rebuild for the reachable cases. ---
#[test]
fn back_interest_publishes_through_the_controller() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
    };
    let mut state = ();

    // Root only: false.
    root.rebuild(&mut app, &mut state);
    assert!(!controller.back_interest(), "root only -> no back interest");

    // Depth 2 (a plain page): true (poppable).
    controller.push(|| sized_page(20.0, 20.0));
    root.rebuild(&mut app, &mut state);
    assert!(controller.back_interest(), "depth 2 -> back interest");

    // Root + a Veto overlay (pop back to root, then push a Veto page): still
    // depth 2 here, so this exercises the reachable overlay case.
    controller.pop();
    root.rebuild(&mut app, &mut state);
    controller.push_with_options(
        || sized_page(30.0, 30.0),
        PushOptions::transparent().back(BackPolicy::Veto),
    );
    root.rebuild(&mut app, &mut state);
    assert!(
        controller.back_interest(),
        "a Veto overlay claims back interest"
    );
}

// --- R23 back reach: a nested navigator's back interest is gated on its
//     HOSTING page being input-routed. These are the widget-level proofs;
//     `frust::back_glue`'s tests prove the arbitration consequence (which
//     navigator a real press reaches). ---

/// An outer navigator whose ROOT page hosts a nested navigator, with the
/// outer navigator's covered-build cull set to `cull` — the shape R23 back
/// reach is about (a section stack with a detail page pushed over it).
struct NestedHarness {
    outer: NavigatorController<()>,
    nested: NavigatorController<()>,
    root: RenderRoot<(), NavigatorView<()>>,
    app: AppLogic,
}

impl NestedHarness {
    fn new(cull: bool) -> Self {
        let outer: NavigatorController<()> = NavigatorController::new();
        let nested: NavigatorController<()> = NavigatorController::new();
        let app: AppLogic = {
            let outer = outer.clone();
            let nested = nested.clone();
            Box::new(move |_: &mut ()| {
                let nested = nested.clone();
                navigator(&outer, move || {
                    navigator(&nested, || sized_page(10.0, 10.0))
                })
                .cull_covered_builds(cull)
            })
        };
        Self {
            outer,
            nested,
            root: RenderRoot::new(),
            app,
        }
    }

    fn rebuild(&mut self) {
        self.root.rebuild(&mut self.app, &mut ());
    }
}

#[test]
fn a_nested_navigator_loses_back_interest_when_its_page_is_covered() {
    for cull in [false, true] {
        let mut h = NestedHarness::new(cull);
        let (outer, nested) = (h.outer.clone(), h.nested.clone());
        h.rebuild();

        // Both at their own roots: nobody claims a press.
        assert!(!outer.back_interest(), "outer at its root (cull={cull})");
        assert!(!nested.back_interest(), "nested at its root (cull={cull})");

        // The nested stack becomes poppable while its page is still the
        // outer navigator's top: it claims the press.
        nested.push(|| sized_page(20.0, 20.0));
        h.rebuild();
        assert!(
            nested.back_interest(),
            "a nested navigator on the CURRENT page claims back (cull={cull})"
        );

        // The outer navigator pushes a page OVER the one hosting it. Input
        // can no longer reach the nested navigator, so neither can back.
        outer.push(|| sized_page(30.0, 30.0));
        h.rebuild();
        assert_eq!((outer.depth(), nested.depth()), (2, 2));
        assert!(
            !nested.back_interest(),
            "a nested navigator on a COVERED page claims nothing (cull={cull})"
        );
        assert!(
            outer.back_interest(),
            "the outer navigator claims it instead (cull={cull})"
        );

        // A frozen page must not go stale: with `cull_covered_builds(true)`
        // the nested navigator stops rebuilding entirely here.
        for _ in 0..3 {
            h.rebuild();
        }
        assert!(
            !nested.back_interest(),
            "and keeps claiming nothing while covered (cull={cull})"
        );

        // Revealed again: interest comes straight back, same pass.
        outer.pop();
        h.rebuild();
        assert!(
            nested.back_interest(),
            "revealed: the nested navigator claims back again (cull={cull})"
        );
    }
}

/// The same gate under a *transparent* overlay: the hosting page is still
/// `PageVisibility::Visible` (painted!) but routed no input, so the nested
/// navigator must not claim back either — reach follows input routing
/// exactly, not painting (R23).
#[test]
fn a_nested_navigator_under_a_transparent_overlay_reports_no_back_interest() {
    let mut h = NestedHarness::new(false);
    let (outer, nested) = (h.outer.clone(), h.nested.clone());
    h.rebuild();
    nested.push(|| sized_page(20.0, 20.0));
    h.rebuild();
    assert!(nested.back_interest());

    outer.push_transparent(|| sized_page(30.0, 30.0));
    h.rebuild();
    assert!(
        !nested.back_interest(),
        "painted but not routed input: no back interest either"
    );
}

/// A page *stashed* by an animated pop is out of `pages` entirely, so
/// `publish_reach` can never see it again — it is marked unreachable at the
/// stash instead. Otherwise a nested navigator on the leaving page would
/// keep claiming presses for the whole flight, during which the navigator
/// suppresses input outright.
#[test]
fn a_nested_navigator_on_a_page_leaving_in_a_pop_transition_claims_nothing() {
    let outer: NavigatorController<()> = NavigatorController::new();
    let nested: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let outer = outer.clone();
        // An animated default: the pop below stashes the leaving page
        // instead of tearing it down immediately.
        move |_: &mut ()| {
            navigator(&outer, || sized_page(10.0, 10.0))
                .transition(TransitionSpec::duration(PageTransition::IosPush))
        }
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);

    // Push the page hosting the nested navigator, then make its stack
    // poppable so it would otherwise claim the press.
    {
        let nested = nested.clone();
        outer.push(move || {
            let nested = nested.clone();
            any(navigator(&nested, || sized_page(20.0, 20.0)))
        });
    }
    root.rebuild(&mut app, &mut state);
    nested.push(|| sized_page(30.0, 30.0));
    root.rebuild(&mut app, &mut state);
    assert!(nested.back_interest(), "current page: it claims back");

    // An ANIMATED pop stashes the hosting page out of the stack.
    outer.pop();
    root.rebuild(&mut app, &mut state);
    assert!(
        !nested.back_interest(),
        "a navigator on the leaving page claims nothing mid-flight"
    );
}

/// Reach composes down an arbitrarily deep chain: with THREE navigators
/// nested one page inside another, covering the outermost page silences
/// both descendants — each level folds its own reachability into what it
/// publishes to its pages, so nobody walks the tree.
#[test]
fn back_reach_composes_through_three_nesting_levels() {
    let outer: NavigatorController<()> = NavigatorController::new();
    let middle: NavigatorController<()> = NavigatorController::new();
    let inner: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let outer = outer.clone();
        let middle = middle.clone();
        let inner = inner.clone();
        move |_: &mut ()| {
            let middle = middle.clone();
            let inner = inner.clone();
            navigator(&outer, move || {
                let inner = inner.clone();
                any(navigator(&middle, move || {
                    navigator(&inner, || sized_page(10.0, 10.0))
                }))
            })
        }
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);

    inner.push(|| sized_page(20.0, 20.0));
    root.rebuild(&mut app, &mut state);
    assert!(inner.back_interest(), "the innermost navigator claims back");

    // Cover the OUTERMOST navigator's page: two levels below it go quiet.
    outer.push(|| sized_page(30.0, 30.0));
    root.rebuild(&mut app, &mut state);
    assert!(
        !inner.back_interest(),
        "an ancestor two levels up covering its page silences the innermost"
    );
    assert!(!middle.back_interest());
    assert!(outer.back_interest(), "the outermost claims it");
}

// --- request_back on a plain (Pop-policy) stack pops one page,
//     and the pop transition is preserved (routes through the same animated
//     pop path as `pop()`). ---
#[test]
fn request_back_pops_plain_page_preserving_transition() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Push B with an animated iOS transition, then settle the push.
    controller.push_with(
        || sized_page(100.0, 60.0),
        TransitionSpec::new(
            PageTransition::IosPush,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        ),
    );
    for t in [0u64, 50, 150, 300] {
        full_frame(&mut root, &mut app, &mut state, ft(t));
    }
    assert_eq!(controller.depth(), 2, "B pushed");

    // A back request routes through the popped page's own transition: both
    // pages paint during the animated pop (transition preserved).
    controller.request_back();
    let (f0, _) = full_frame(&mut root, &mut app, &mut state, ft(1000));
    assert!(
        f0.iter().any(|(_, s, _)| (s.height - 100.0).abs() < 1e-9),
        "A paints during the pop"
    );
    assert!(
        f0.iter().any(|(_, s, _)| (s.height - 60.0).abs() < 1e-9),
        "B (leaving) still paints during the pop"
    );

    // Drive to completion: only A remains (depth back to 1).
    for t in [1050u64, 1150, 1300] {
        full_frame(&mut root, &mut app, &mut state, ft(t));
    }
    assert_eq!(controller.depth(), 1, "request_back popped one page");
}

// --- request_back at the root (Pop policy, depth 1) is a safe
//     no-op. ---
#[test]
fn request_back_at_root_is_a_noop() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 1);

    controller.request_back();
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 1, "root back does not pop the root");
}

// --- A DismissAnimated top page leaves the stack unchanged and
//     fires its observable dismiss signal exactly once per request. ---
#[test]
fn request_back_dismiss_animated_fires_signal_once_no_pop() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);

    // Push a transparent, dismissable overlay carrying a shared dismiss
    // signal (the seam the overlay widget would observe).
    let signal = Rc::new(Cell::new(0u64));
    controller.push_with_options(
        || sized_page(20.0, 20.0),
        PushOptions::transparent()
            .back(BackPolicy::DismissAnimated)
            .dismiss_signal(signal.clone()),
    );
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 2, "overlay pushed");
    assert_eq!(signal.get(), 0, "no back yet");

    // First back request: signal fires once, stack unchanged.
    controller.request_back();
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 2, "DismissAnimated does not pop");
    assert_eq!(signal.get(), 1, "signal fired exactly once");

    // Second back request: fires again (once more), still no pop.
    controller.request_back();
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 2);
    assert_eq!(signal.get(), 2, "signal fired once per request");
}

// --- A Veto top page consumes the press without changing the
//     stack and without firing any signal. ---
#[test]
fn request_back_veto_consumes_without_change() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);

    // A non-dismissable overlay: Veto policy, still carrying a signal to prove
    // it is NOT fired.
    let signal = Rc::new(Cell::new(0u64));
    controller.push_with_options(
        || sized_page(20.0, 20.0),
        PushOptions::transparent()
            .back(BackPolicy::Veto)
            .dismiss_signal(signal.clone()),
    );
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 2);

    controller.request_back();
    root.rebuild(&mut app, &mut state);
    assert_eq!(controller.depth(), 2, "Veto leaves the stack unchanged");
    assert_eq!(signal.get(), 0, "Veto fires no dismiss signal");
}

// ---------------------------------------------------------------------
// The root overlay host (`overlay_host`).
// ---------------------------------------------------------------------

/// The host is the ordinary navigator with exactly two defaults changed —
/// asserted on the view, and then behaviourally: a left-edge drag over an
/// open overlay must not dismiss it (an edge swipe is a navigation gesture,
/// never a modal dismissal), and the overlay must appear with no host
/// transition of its own (each overlay stages its own enter/exit).
#[test]
fn overlay_host_is_a_navigator_with_two_defaults_changed() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let view = overlay_host(&controller, || sized_page(10.0, 10.0));
    assert_eq!(
        view.pop_swipe,
        Some(false),
        "the host pins pop_swipe off explicitly, not by default-derivation"
    );
    assert!(!view.resolve_pop_swipe());
    assert_eq!(view.default_transition, TransitionSpec::NONE);

    let (controller, mut root, mut app, ..) = overlay_host_fixture();
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    controller.push_with_options(|| sized_page(1e4, 1e4), PushOptions::transparent());
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    // No host transition: the overlay is settled on the very frame it is
    // pushed, so it takes input immediately rather than after ≤340ms of
    // mid-transition input suppression.
    assert!(
        !controller.transition().active,
        "TransitionSpec::NONE: the overlay push settles instantly"
    );
    assert_eq!(controller.depth(), 2);

    // A full left-edge drag: arm at x <= EDGE_SWIPE_ZONE_DP, cross the slop,
    // release well past the commit threshold.
    root.event(&mut state, &down(2.0, 50.0));
    root.event(&mut state, &move_to(80.0, 50.0));
    root.event(&mut state, &up(90.0, 50.0));
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        controller.depth(),
        2,
        "pop_swipe(false): an edge swipe never dismisses an overlay"
    );
}

/// The chrome-inertness half of the overlay host, and the reason it must sit
/// *above* the chrome rather than beside it: with an overlay up, a pointer
/// at the chrome's own coordinates reaches the overlay. No new suppression
/// code — `route_top` already routes to `input_routed_pages()` only.
#[test]
fn an_overlay_takes_the_pointer_from_the_chrome_beneath_it() {
    let (controller, mut root, mut app, content_hits, chrome_hits) = overlay_host_fixture();
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Baseline: with no overlay, a press at (50, 50) lands on the chrome.
    root.event(&mut state, &down(50.0, 50.0));
    assert_eq!(chrome_hits.get(), 1, "no overlay: the chrome takes presses");
    assert_eq!(content_hits.get(), 0, "the chrome is above the content");

    let overlay_hits = Rc::new(Cell::new(0u32));
    controller.push_with_options(
        {
            let overlay_hits = overlay_hits.clone();
            move || host_probe("overlay", &overlay_hits)
        },
        PushOptions::transparent(),
    );
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    chrome_hits.set(0);
    content_hits.set(0);
    root.event(&mut state, &down(50.0, 50.0));
    assert_eq!(overlay_hits.get(), 1, "the overlay takes the press");
    assert_eq!(
        chrome_hits.get(),
        0,
        "the chrome under a root overlay is inert — the whole point of #44"
    );
    assert_eq!(content_hits.get(), 0, "so is the app content");
}

/// The host owns no scrim, and does not need to: an overlay page already
/// fills `ctx.origin()..ctx.size()`, and at the ROOT that rect is the
/// window. This is what lets `glyph::dialog`/`material::sheet` dim the whole
/// app with no change at all.
#[test]
fn an_overlays_scrim_rect_equals_the_window_rect() {
    let (controller, mut root, mut app, ..) = overlay_host_fixture();
    let mut state = ();
    let window = Size::new(320.0, 640.0);
    root.rebuild(&mut app, &mut state);
    root.layout(window);

    // A page that fills whatever box it is given — the scrim shape every
    // overlay catalog widget paints first.
    controller.push_with_options(|| sized_page(1e4, 1e4), PushOptions::transparent());
    root.rebuild(&mut app, &mut state);
    root.layout(window);
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);

    assert!(
        scene.rects.contains(&(Point::ZERO, window)),
        "the overlay's scrim covers the whole window, not a sub-rect \
         (recorded rects: {:?})",
        scene.rects
    );
}
