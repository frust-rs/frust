//! Page-transition machinery: preset paint sequences, `ThemeDefault`/
//! `reduce_motion`/`Layer::scale` wiring, the `SlideUp` preset, shared-element
//! ("hero") transitions, and the `TransitionState` observation seam.

use super::super::*;
use super::support::*;
use crate::nav::transition::{PageTransition, Timing, TransitionSpec};
use crate::stack;
use frust_core::Curve;
use frust_core::{FrameTime, RenderRoot, any};
use frust_theme::{MotionSpring, Theme};
use kurbo::{Affine, Rect};
use std::cell::Cell;
use std::time::Duration;

// ---------------------------------------------------------------------
// Page-transition machinery.
// ---------------------------------------------------------------------

/// A recording scene that separates the retained bracket
/// (`push_retained`/`pop_retained`) from the plain `push_layer`/
/// `push_transform` bracket a retained-ineligible page still uses.
/// `crate::test_support::RecordingScene` doesn't override `push_retained`, so
/// it would fold an eligible page's bracket into [`PaintScene::push_retained`]'s
/// emulating default (a `push_transform` + `push_layer` pair) —
/// indistinguishable from the plain bracket even at identity alpha/scale,
/// since the retained bracket, unlike the plain one, is never skipped at the
/// identity value. This scene keeps the two apart — mirroring
/// `motion::switcher`'s own local test scene for the same reason.
#[derive(Default)]
struct PageScene {
    rects: Vec<(Point, Size)>,
    layers: Vec<(Point, Size, f32)>,
    layer_pops: u32,
    transforms: Vec<Affine>,
    transform_pops: u32,
    retained_pushes: Vec<(u64, Point, Size, f32, f64)>,
    retained_pops: u32,
    draws: Vec<(u64, Point, Size, f32, f64)>,
}
impl PaintScene for PageScene {
    fn fill_rect(&mut self, origin: Point, size: Size, _color: peniko::Color) {
        self.rects.push((origin, size));
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
        self.layers.push((origin, size, alpha));
    }
    fn pop_layer(&mut self) {
        self.layer_pops += 1;
    }
    fn push_transform(&mut self, transform: Affine) {
        self.transforms.push(transform);
    }
    fn pop_transform(&mut self) {
        self.transform_pops += 1;
    }
    fn push_retained(&mut self, key: u64, origin: Point, size: Size, alpha: f32, scale: f64) {
        self.retained_pushes.push((key, origin, size, alpha, scale));
    }
    fn pop_retained(&mut self) {
        self.retained_pops += 1;
    }
    fn draw_retained(&mut self, key: u64, origin: Point, size: Size, alpha: f32, scale: f64) {
        self.draws.push((key, origin, size, alpha, scale));
    }
}

// --- Push animates both pages with moving origins, settling at
//     final geometry; controller disposed after settle. ---

#[test]
fn push_animates_moving_origins_and_disposes_after_settle() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        // Root page A is 100 tall; pushed page B is 80 tall (so a test can
        // tell them apart in the recorded fills).
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let spec = TransitionSpec::new(
        PageTransition::M3SharedAxisX,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| sized_page(100.0, 80.0), spec);

    // Seed frame: driver at 0. A (leaving) at rest, still fully opaque. B
    // (entering) is exactly alpha 0 at the seed frame (`resolve_layers`'s
    // 0.35 split hasn't opened yet), so it now paints into a `DiscardScene`
    // sink instead of sliding in under a zero-opacity layer — it records no
    // fill here at all, not just an invisible one.
    let (f0, nf0) = full_frame(&mut root, &mut app, &mut state, ft(0));
    assert!(nf0, "a running transition requests frames");
    assert_eq!(f0.len(), 1, "B is discarded at alpha 0 at the seed frame");
    assert_eq!(fill_h(&f0, 100.0).0.x, 0.0, "A at rest at start");

    // Early frame (20ms → progress 0.2, before the 0.35 split): B is still
    // exactly alpha 0 and discarded; A is visible under an alpha<1 bracket
    // and has begun sliding left, leaving.dx = -pc * 30dp = -6.0. Property:
    // layer-to-pod-origin wiring for split presets stays asserted at paint
    // level.
    let (f_early, _) = full_frame(&mut root, &mut app, &mut state, ft(20));
    assert_eq!(f_early.len(), 1, "B is still discarded before the split");
    let a_early_x = fill_h(&f_early, 100.0).0.x;
    assert!(
        (a_early_x - (-6.0)).abs() < 1e-6,
        "A slides left under the alpha<1 bracket (was {a_early_x})"
    );

    // Mid frame (50ms → progress 0.5, past the 0.35 split): the split is a
    // hard cut, not an overlapping crossfade, so A has now fully faded to
    // alpha 0 in turn and is discarded here exactly like B was above; B, the
    // only visible page, has slid to entering.dx = (1 - pc) * 30dp = 15.0.
    let (f1, _) = full_frame(&mut root, &mut app, &mut state, ft(50));
    assert_eq!(f1.len(), 1, "A is discarded at alpha 0 past the split");
    let b1x = fill_h(&f1, 80.0).0.x;
    assert!(
        (b1x - 15.0).abs() < 1e-6,
        "B slides toward rest (was {b1x})"
    );

    // End frame (150ms → past the 100ms duration): settles at final geometry.
    let (f2, nf2) = full_frame(&mut root, &mut app, &mut state, ft(150));
    assert_eq!(fill_h(&f2, 80.0).0.x, 0.0, "B rests exactly at 0");
    assert!(nf2, "the settling frame still requests one finalize frame");

    // Finalize frame: the controller is disposed, culling resumes, and no
    // further frame is requested.
    let (f3, nf3) = full_frame(&mut root, &mut app, &mut state, ft(300));
    assert!(!nf3, "no frame requested once the transition is disposed");
    assert_eq!(f3.len(), 1, "culling resumed: only the top page paints");
    assert_eq!(f3[0].1, Size::new(100.0, 80.0), "the surviving page is B");
}

// --- Outside a transition (settled culling), a page paints with no bracket
//     at all — no `push_layer`, no `push_transform`, and no retained bracket
//     either: the retained bracket is a transition-time device only. ---

#[test]
fn settled_no_transition_paints_with_no_bracket_at_all() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let spec = TransitionSpec::new(
        PageTransition::M3SharedAxisX,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| sized_page(100.0, 80.0), spec);
    // Drive to settle + finalize (mirrors the disposal frames above).
    for t in [0u64, 150, 300] {
        full_frame(&mut root, &mut app, &mut state, ft(t));
    }

    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene = PageScene::default();
    root.paint(&mut scene, ft(316));

    assert_eq!(scene.rects.len(), 1, "only the surviving top page paints");
    assert!(
        scene.layers.is_empty() && scene.transforms.is_empty() && scene.retained_pushes.is_empty(),
        "no bracket of any kind once culling has resumed: layers={:?} \
         transforms={:?} retained_pushes={:?}",
        scene.layers,
        scene.transforms,
        scene.retained_pushes
    );
}

// --- A page's retained key is stable across frames (the point of caching)
//     and distinct from every other page's — drawn once, at construction,
//     from the process-wide `RetainedKey::mint` counter. The seed frame
//     records the outgoing page under it (still at rest), and once the
//     renderer reports that page resident the next frame composites it,
//     moved to the slid origin, instead of painting the page again. ---

#[test]
fn each_pages_retained_key_is_stable_and_the_outgoing_page_composites_once_resident() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let key_a = nav_widget(&root).pages[0].boundary.key().get();

    let spec = TransitionSpec::new(
        PageTransition::M3SharedAxisX,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| sized_page(100.0, 80.0), spec);
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let key_b = nav_widget(&root).pages[1].boundary.key().get();
    assert_ne!(
        key_a, key_b,
        "two different pages never share a retained key"
    );

    // Seed frame (progress 0): A — the leaving page — still sits at rest,
    // fully opaque, so it is recorded under its own key; B is at alpha 0.
    let mut scene1 = PageScene::default();
    root.paint(&mut scene1, ft(0));
    assert_eq!(
        scene1.retained_pushes,
        vec![(key_a, Point::ZERO, Size::new(100.0, 100.0), 1.0, 1.0)],
        "the leaving page is recorded at rest under its own PageEntry key"
    );
    assert_eq!(scene1.retained_pops, 1, "the bracket is balanced");
    assert_eq!(scene1.rects.len(), 1, "A's body paints inside the bracket");

    // The renderer reports A's page resident (100x100 at scale 1 — what the
    // boundary asks for under a recorder with no transform). A is unchanged,
    // so the next early frame (before the 0.35 split, A sliding left by
    // 0.01 * 30dp) composites the page instead of painting A again.
    root.set_retained_ledger(&frust_core::RetainedLedger::from_entries(
        1,
        [(key_a, (100, 100), 1.0)],
    ));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene2 = PageScene::default();
    root.paint(&mut scene2, ft(1));

    assert_eq!(
        scene2.draws.len(),
        1,
        "A's page is composited: {:?}",
        scene2.draws
    );
    let (key, origin, size, alpha, scale) = scene2.draws[0];
    assert_eq!(key, key_a, "the same page's key, frame after frame");
    assert_eq!(
        (origin, size, scale),
        (Point::ZERO, Size::new(100.0, 100.0), 1.0),
        "drawn at the rectangle it was recorded at, at scale 1"
    );
    assert!(
        alpha > 0.0 && alpha < 1.0,
        "at A's fading alpha (was {alpha})"
    );
    assert_eq!(
        scene2.transforms.len(),
        1,
        "one translation: {:?}",
        scene2.transforms
    );
    let shift = scene2.transforms[0].translation();
    assert!(
        (shift.x - (-0.3)).abs() < 1e-9 && shift.y == 0.0,
        "moved to A's slid origin by a translation (was {shift:?})"
    );
    assert_eq!(scene2.transform_pops, 1, "the translation is balanced");
    assert!(
        scene2.rects.is_empty() && scene2.retained_pushes.is_empty(),
        "A's body is not painted again (B is still discarded at alpha 0)"
    );
}

// --- Never rasterize a page whose resolved `Layer.alpha` is 0: it paints
//     into a `DiscardScene` sink instead of under a zero-opacity
//     `push_layer` bracket. `resolve_layers`'s split presets (M3SharedAxisX,
//     M3FadeThrough, Glyph) hold exactly one page at alpha 0 at any given
//     instant — never both, and never neither, except at the split instant
//     itself. ---

#[test]
fn shared_axis_x_push_seed_frame_discards_the_alpha_zero_entering_page() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let spec = TransitionSpec::new(
        PageTransition::M3SharedAxisX,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| sized_page(100.0, 80.0), spec);

    // Seed frame (p=0): the entering page (B) is exactly alpha 0 (the 0.35
    // split hasn't opened yet). It now paints into a `DiscardScene` sink
    // rather than under a zero-opacity `push_layer` bracket, so its fill AND
    // that bracket are both absent from the real scene; the leaving page (A),
    // at rest and fully opaque with the transition programmatic and no hero
    // in play, is retained-eligible — it paints through exactly one
    // `push_retained`/`pop_retained` bracket instead of a plain one (the
    // retained bracket, unlike the plain bracket, is never skipped at
    // alpha/scale identity).
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene = PageScene::default();
    root.paint(&mut scene, ft(0));

    assert_eq!(
        scene.rects.len(),
        1,
        "the alpha-0 entering page records no fill at all"
    );
    assert_eq!(
        scene.rects[0].1,
        Size::new(100.0, 100.0),
        "the surviving fill is the leaving page A"
    );
    assert!(
        scene.layers.is_empty() && scene.transforms.is_empty(),
        "no plain push_layer/push_transform bracket at all: A paints through \
         a retained bracket instead, B is alpha 0.0 (discarded)"
    );
    assert_eq!(
        scene.retained_pushes.len(),
        1,
        "A paints through exactly one retained bracket: {:?}",
        scene.retained_pushes
    );
    let (_, retained_origin, _, alpha, scale) = scene.retained_pushes[0];
    assert_eq!(alpha, 1.0, "A is at rest, fully opaque");
    assert_eq!(scale, 1.0, "M3SharedAxisX never scales either page");
    assert_eq!(scene.retained_pops, 1, "the bracket is balanced");

    // The leaving page A has dx=0 at progress 0 (not yet sliding out). The
    // bracket origin must still follow the pod's absolute paint origin, which
    // equals ctx.origin() when dx=0. At the root navigator level, ctx.origin()
    // is (0, 0), so the bracket origin should be (0, 0).
    assert_eq!(
        retained_origin,
        Point::new(0.0, 0.0),
        "bracket origin follows ctx.origin() when dx=0 (was {retained_origin:?})"
    );
}

#[test]
fn shared_axis_x_past_the_split_the_slid_page_paints_directly_under_its_fade() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let spec = TransitionSpec::new(
        PageTransition::M3SharedAxisX,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| sized_page(100.0, 80.0), spec);
    full_frame(&mut root, &mut app, &mut state, ft(0)); // seed

    // Mid frame (50ms → progress 0.5, past the 0.35 split): `resolve_layers`
    // is a hard split, not an overlapping crossfade, so the leaving page (A)
    // has by now fully faded to alpha 0 and is discarded here exactly like B
    // was at the seed frame above. B, the only page with `0 < alpha < 1`, is
    // eligible for its boundary (a programmatic transition, no hero in play),
    // but it has slid 15dp right and overhangs the navigator's area, where a
    // renderer never retains a body — so it is not bracketed: it paints
    // directly under one `push_layer` carrying its genuine partial alpha.
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene = PageScene::default();
    root.paint(&mut scene, ft(50));

    assert_eq!(
        scene.rects.len(),
        1,
        "the leaving page A is fully faded out (alpha 0) and discarded"
    );
    assert_eq!(
        scene.rects[0].1,
        Size::new(100.0, 80.0),
        "the surviving fill is the entering page B"
    );
    assert!(
        scene.retained_pushes.is_empty() && scene.draws.is_empty(),
        "an overhanging page is never bracketed: {:?}",
        scene.retained_pushes
    );
    assert!(
        scene.transforms.is_empty(),
        "M3SharedAxisX never scales either page"
    );
    assert_eq!(
        scene.layers.len(),
        1,
        "B fades under one layer: {:?}",
        scene.layers
    );
    let (layer_origin, _, alpha) = scene.layers[0];
    assert!(
        alpha > 0.0 && alpha < 1.0,
        "the layer's alpha is a genuine partial value (was {alpha})"
    );

    // The entering page B slides in from the right at progress 0.5, so its dx
    // is 0.5 * 30dp = 15.0 (M3_SHARED_AXIS_SLIDE_DP = 30); its fill and its
    // layer both follow the pod's absolute paint origin.
    assert_eq!(
        scene.rects[0].0,
        Point::new(15.0, 0.0),
        "B paints at its slid origin"
    );
    assert_eq!(
        layer_origin,
        Point::new(15.0, 0.0),
        "the layer follows the entering page's dx slide (was {layer_origin:?})"
    );
}

#[test]
fn glyph_push_seed_frame_discards_the_alpha_zero_entering_page() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let spec = TransitionSpec::new(
        PageTransition::Glyph,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| sized_page(100.0, 80.0), spec);

    // Seed frame (p=0): the Glyph preset's 0.44 split is the same shape as
    // M3SharedAxisX's 0.35 one — the entering page is exactly alpha 0, so it
    // paints into a `DiscardScene` sink instead of under a zero-opacity
    // layer; the leaving page, at rest and fully opaque, is
    // retained-eligible and paints through a `push_retained`/`pop_retained`
    // bracket instead of plainly.
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene = PageScene::default();
    root.paint(&mut scene, ft(0));

    assert_eq!(
        scene.rects.len(),
        1,
        "the alpha-0 entering page records no fill at all"
    );
    assert_eq!(
        scene.rects[0].1,
        Size::new(100.0, 100.0),
        "the surviving fill is the leaving page"
    );
    assert!(
        scene.layers.is_empty() && scene.transforms.is_empty(),
        "no plain push_layer/push_transform bracket at all: leaving paints \
         through a retained bracket, entering is alpha 0.0 (discarded)"
    );
    assert_eq!(
        scene.retained_pushes.len(),
        1,
        "the leaving page paints under exactly one retained bracket: {:?}",
        scene.retained_pushes
    );
    let (_, _, _, alpha, _) = scene.retained_pushes[0];
    assert_eq!(alpha, 1.0, "the leaving page is at rest, fully opaque");
}

// --- Input is blocked mid-transition; no page receives the Down
//     and no stale capture is left; routing resumes after settle. ---

#[test]
fn input_blocked_mid_transition_then_resumes() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let observed_a = Rc::new(Cell::new(0u32));
    let observed_b = Rc::new(Cell::new(0u32));
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        let a = observed_a.clone();
        move |_: &mut ()| {
            let a = a.clone();
            navigator(&ctrl, move || counter_page(&a))
        }
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Push B (a counter) with a long, still-running transition.
    let spec = TransitionSpec::new(
        PageTransition::M3FadeThrough,
        Timing::Duration(Duration::from_millis(1000), Curve::Linear),
    );
    {
        let b = observed_b.clone();
        controller.push_with(move || counter_page(&b), spec);
    }
    full_frame(&mut root, &mut app, &mut state, ft(0)); // seed; running

    // A pointer Down mid-transition reaches NO page and records no capture.
    root.event(&mut state, &down(5.0, 5.0));
    assert!(
        !root.is_pointer_captured(),
        "no capture is recorded mid-transition"
    );
    full_frame(&mut root, &mut app, &mut state, ft(16));
    assert_eq!(observed_a.get(), 0, "the Down did not reach page A");
    assert_eq!(observed_b.get(), 0, "the Down did not reach page B");

    // Advance past the end to settle, then finalize on the next rebuild.
    full_frame(&mut root, &mut app, &mut state, ft(1100));
    full_frame(&mut root, &mut app, &mut state, ft(1116));

    // Routing has resumed with no stale block: a Down now reaches top page B.
    root.event(&mut state, &down(5.0, 5.0));
    full_frame(&mut root, &mut app, &mut state, ft(1132));
    assert_eq!(observed_b.get(), 1, "routing resumed after the transition");
    assert!(!root.is_pointer_captured());
}

// --- The below page's secondary animation (iOS push parallax +
//     dim). ---

#[test]
fn ios_push_parallaxes_and_dims_below_page() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let spec = TransitionSpec::new(
        PageTransition::IosPush,
        Timing::Duration(Duration::from_millis(350), Curve::Linear),
    );
    controller.push_with(|| sized_page(100.0, 80.0), spec);

    // Seed: below page A at rest, full opacity; incoming B a full width off.
    let (f0, _) = full_frame(&mut root, &mut app, &mut state, ft(0));
    assert_eq!(fill_h(&f0, 100.0).0.x, 0.0);
    assert_eq!(fill_h(&f0, 100.0).2, 1.0);
    assert_eq!(
        fill_h(&f0, 80.0).0.x,
        100.0,
        "B enters a full width to the right"
    );

    // Halfway (175ms → 0.5): A (below) has parallaxed left and dimmed.
    let (f1, _) = full_frame(&mut root, &mut app, &mut state, ft(175));
    let a = fill_h(&f1, 100.0);
    assert!(a.0.x < 0.0, "below page parallaxes left (was {})", a.0.x);
    assert!(a.2 < 1.0, "below page is dimmed (alpha {})", a.2);
}

// --- A spatial spring overshoots position, never opacity. ---

#[test]
fn spring_spatial_overshoots_position_but_not_opacity() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // M3 default spatial preset (damping 0.9) — overshoots.
    let spring = MotionSpring {
        damping_ratio: 0.9,
        stiffness: 700.0,
    };
    let spec = TransitionSpec::spring(PageTransition::M3SharedAxisX, spring);
    controller.push_with(|| sized_page(100.0, 80.0), spec);

    let mut min_b_x = f64::MAX;
    let mut max_alpha = f32::MIN;
    let mut running = true;
    let mut t = 0u64;
    for _ in 0..2_000 {
        let (fills, needs_frame) = full_frame(&mut root, &mut app, &mut state, ft(t));
        // B (entering, height 80) is only present while the transition runs.
        if let Some((p, _, a)) = fills
            .iter()
            .find(|(_, s, _)| (s.height - 80.0).abs() < 1e-9)
        {
            min_b_x = min_b_x.min(p.x);
            max_alpha = max_alpha.max(*a);
        }
        running = needs_frame;
        if !running {
            break;
        }
        t += 8; // ~120fps
    }
    assert!(!running, "spring transition failed to settle");
    // The entering page rests at x = 0; an under-damped spring carries it past
    // that (x < 0) before settling — the overshoot the spatial preset exists
    // to produce.
    assert!(
        min_b_x < -1e-3,
        "expected a position overshoot past the resting x=0, min x was {min_b_x}"
    );
    // Opacity is fed the clamped value, so it never exceeds 1.0 even as the
    // spatial spring overshoots.
    assert!(
        max_alpha <= 1.0 + 1e-6,
        "opacity must never overshoot 1.0 (saw {max_alpha})"
    );
}

// --- Replace with a transition retains + tears down the outgoing page. ---

#[test]
fn animated_replace_settles_to_new_page() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let spec = TransitionSpec::new(
        PageTransition::M3FadeThrough,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.replace_with(|| sized_page(100.0, 40.0), spec);

    // Drive to completion.
    let mut last = Vec::new();
    for t in [0u64, 50, 150, 300] {
        let (fills, _) = full_frame(&mut root, &mut app, &mut state, ft(t));
        last = fills;
    }
    // After settle only the new page (height 40) remains — the replaced page
    // was retained during the animation and torn down at settle.
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].1, Size::new(100.0, 40.0));
}

// --- Pop reverses the popped page's transition and reveals the page below. ---

#[test]
fn animated_pop_reverses_and_reveals_below() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Push B instantly (no transition), then animate the pop.
    controller.push_with(
        || sized_page(100.0, 60.0),
        TransitionSpec::new(
            PageTransition::IosPush,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        ),
    );
    // Settle the push first.
    for t in [0u64, 50, 150, 300] {
        full_frame(&mut root, &mut app, &mut state, ft(t));
    }

    // Now pop: the popped page (B) reverses its iOS transition (slides right),
    // revealing A below it.
    controller.pop();
    let (f0, _) = full_frame(&mut root, &mut app, &mut state, ft(1000));
    // Both A (100) and B (60) paint during the pop.
    assert!(f0.iter().any(|(_, s, _)| (s.height - 100.0).abs() < 1e-9));
    assert!(f0.iter().any(|(_, s, _)| (s.height - 60.0).abs() < 1e-9));

    // Drive to completion: B is torn down, A revealed and culled to top.
    let mut last = Vec::new();
    for t in [1050u64, 1150, 1300] {
        let (fills, _) = full_frame(&mut root, &mut app, &mut state, ft(t));
        last = fills;
    }
    assert_eq!(last.len(), 1, "only the revealed page remains");
    assert_eq!(last[0].1, Size::new(100.0, 100.0), "the revealed page is A");
}

// --- The pop/replace stashed page (retained in `ActiveTransition::stashed`,
//     no longer in `pages`) still paints through its own boundary, same as
//     any other eligible page: recorded at rest on the pop's seed frame, then
//     composited as it slides off. ---

#[test]
fn pop_stashed_page_paints_through_its_own_boundary() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Push B instantly, then animate the pop with IosPush, whose entering AND
    // leaving alpha are always exactly 1.0 (parallax + dim, never reaching
    // 0) — both pages stay retained-eligible for the whole pop, with no
    // alpha-0 discard to complicate the picture.
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

    // Pop: B is stashed (removed from `pages`, retained in
    // `ActiveTransition::stashed`) and reverses its transition; A is
    // revealed.
    controller.pop();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    let stashed_key = nav_widget(&root)
        .transition
        .as_ref()
        .and_then(|t| t.stashed.as_ref())
        .map(|p| p.boundary.key().get())
        .expect("the popped page is stashed");

    let mut scene = PageScene::default();
    // The pop's own fresh-driver seed frame; its serial is what the ledger
    // below must cover for the recorded page to be drawable.
    let seed = root.paint(&mut scene, ft(1000)).paint_serial;

    // At progress 0 the stashed page sits at rest and is recorded; the
    // revealed page starts parallaxed 30dp left, overhanging the navigator,
    // and paints directly.
    assert_eq!(
        scene.retained_pushes,
        vec![(stashed_key, Point::ZERO, Size::new(100.0, 100.0), 1.0, 1.0)],
        "only the stashed popped page is recorded, under its own key"
    );
    assert_eq!(scene.rects.len(), 2, "both pages paint their bodies");

    // Resident and unchanged: the next frame composites the stashed page as it
    // slides right (IosPush pop: dx = p * w = 0.5 * 100).
    root.set_retained_ledger(&frust_core::RetainedLedger::from_entries(
        seed,
        [(stashed_key, (100, 100), 1.0)],
    ));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene = PageScene::default();
    root.paint(&mut scene, ft(1050));
    assert_eq!(
        scene.draws,
        vec![(stashed_key, Point::ZERO, Size::new(100.0, 100.0), 1.0, 1.0)],
        "the stashed page is composited from its page"
    );
    assert_eq!(scene.transforms, vec![Affine::translate((50.0, 0.0))]);
    assert_eq!(scene.rects.len(), 1, "only the revealed page paints a body");
}

// ---------------------------------------------------------------------
// ThemeDefault / reduce_motion / Layer::scale wiring through the REAL
// navigator.
// ---------------------------------------------------------------------

// --- A `ThemeDefault` Glyph push resolves the *enter duration* from
//     the active MotionScheme (the neutral baseline's `slow` = 400ms), not
//     the 300ms unthemed M3 fallback. Proven by the transition still
//     running at 340ms under the theme, where the unthemed control has
//     already finalized. ---

#[test]
fn glyph_theme_default_resolves_enter_duration_from_scheme() {
    // Themed: push `TransitionSpec::glyph()` (ThemeDefault timing) under a
    // theme whose Glyph enter timing is the scheme's slow = 400ms.
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    root.set_theme(Box::new(Theme::neutral()));
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    controller.push_with(|| sized_page(100.0, 80.0), TransitionSpec::glyph());
    full_frame(&mut root, &mut app, &mut state, ft(0)); // seed + resolve
    let (_f1, nf1) = full_frame(&mut root, &mut app, &mut state, ft(320));
    let (_f2, nf2) = full_frame(&mut root, &mut app, &mut state, ft(340));
    assert!(
        nf1 && nf2,
        "the theme-resolved 400ms enter duration is still running at 320/340ms"
    );

    // Control: the identical push with NO theme threaded falls back to the
    // 300ms M3 default, which settles (ft 320) and finalizes (ft 340) — so it
    // requests no frame at 340ms. The divergence proves the theme resolution
    // changed the enter duration (300ms → 400ms) at first paint.
    let ctrl2: NavigatorController<()> = NavigatorController::new();
    let mut root2: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app2 = {
        let c = ctrl2.clone();
        move |_: &mut ()| navigator(&c, || sized_page(100.0, 100.0))
    };
    let mut s2 = ();
    root2.rebuild(&mut app2, &mut s2);
    root2.layout(Size::new(100.0, 100.0));
    ctrl2.push_with(|| sized_page(100.0, 80.0), TransitionSpec::glyph());
    full_frame(&mut root2, &mut app2, &mut s2, ft(0));
    full_frame(&mut root2, &mut app2, &mut s2, ft(320)); // settle
    let (_c2, nfc) = full_frame(&mut root2, &mut app2, &mut s2, ft(340)); // finalize
    assert!(
        !nfc,
        "the unthemed 300ms M3 fallback has finalized by 340ms (no frame requested)"
    );
}

// --- A Glyph push under a `reduce_motion` MotionScheme collapses per
//     `resolve_spec`'s contract — the 16px directional Glyph slide becomes the
//     non-directional M3 fade-through crossfade (no slide). ---

#[test]
fn reduce_motion_collapses_glyph_push_to_crossfade() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut reduced = Theme::neutral();
    reduced.motion.reduce_motion = true;
    root.set_theme(Box::new(reduced));
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // A Glyph push normally slides the incoming page in by 16px at p=0; under
    // reduce_motion it must collapse to the crossfade family and NOT slide.
    // `ReducedCrossfade`'s entering alpha is exactly the raw progress value,
    // so the literal seed frame (p=0) is exactly alpha 0 and the entering
    // page is now discarded rather than painted under a zero-opacity layer —
    // seed first (establishing the driver's baseline), then sample one tick
    // later, where alpha is a hair positive, to keep this assertion about
    // the (lack of) slide, not about that discard.
    controller.push_with(|| sized_page(100.0, 80.0), TransitionSpec::glyph());
    full_frame(&mut root, &mut app, &mut state, ft(0)); // seed
    let (f0, _) = full_frame(&mut root, &mut app, &mut state, ft(1));
    assert_eq!(
        fill_h(&f0, 80.0).0.x,
        0.0,
        "reduce_motion collapses the 16px Glyph slide to a non-directional crossfade"
    );

    // Scale-leak guard: the reduced crossfade must also paint
    // ZERO scale transforms mid-transition — a reduce_motion user never sees
    // the fade-through 0.92→1.0 zoom. At p=0.5 (60ms of the 120ms
    // REDUCE_MOTION_DURATION) both pages are genuinely visible at once (a
    // real crossfade, unlike the split presets) and both are
    // retained-eligible — each paints through its own `push_retained`
    // bracket, never a `push_transform` of its own, and the bracket's own
    // `scale` field is still exactly 1.0.
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene = PageScene::default();
    root.paint(&mut scene, ft(60));
    assert_eq!(
        scene.transforms.len(),
        0,
        "reduced-motion crossfade must not scale either page"
    );
    assert_eq!(
        scene.retained_pushes.len(),
        2,
        "both pages are visible under a genuine crossfade: {:?}",
        scene.retained_pushes
    );
    for (_, _, _, _, scale) in &scene.retained_pushes {
        assert_eq!(
            *scale, 1.0,
            "reduced-motion crossfade must not scale either page"
        );
    }
}

// --- A mid-transition M3 fade-through paint scales the incoming page (the
//     0.92 → 1.0 scale-up) inline under a `push_transform` and a `push_layer`
//     — a scale step never goes through a retained bracket, since a page is
//     only served at the scale it was recorded at; the leaving page
//     (discarded, past the split) records nothing. ---

#[test]
fn fade_through_paint_scales_incoming_page_below_one() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let spec = TransitionSpec::new(
        PageTransition::M3FadeThrough,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| sized_page(100.0, 80.0), spec);

    // Seed frame (p=0): the incoming page holds at the 0.92 fade-through start
    // scale AND alpha exactly 0 (the 0.30 split hasn't opened yet), so it now
    // paints into a `DiscardScene` sink instead — NEITHER the scale bracket
    // nor an alpha one reaches the real scene. The outgoing page is still at
    // rest, fully opaque with the transition programmatic and no hero in
    // play — it is eligible for its boundary and is recorded through a
    // `push_retained` bracket at scale 1.0, not a `push_transform` of its own.
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene = PageScene::default();
    root.paint(&mut scene, ft(0));
    assert!(
        scene.transforms.is_empty(),
        "the alpha-0 incoming page is discarded before its scale bracket ever runs, \
         and the outgoing page's own bracket is a retained one, never a push_transform"
    );
    assert_eq!(scene.rects.len(), 1, "only the outgoing page paints at p=0");
    assert_eq!(
        scene.retained_pushes.len(),
        1,
        "the outgoing page paints through its own retained bracket: {:?}",
        scene.retained_pushes
    );
    let (_, _, _, alpha0, scale0) = scene.retained_pushes[0];
    assert_eq!(alpha0, 1.0, "the outgoing page is at rest, fully opaque");
    assert_eq!(scale0, 1.0, "the outgoing page is never scaled");

    // Mid frame (50ms → progress 0.5, past the 0.30 split): the incoming page
    // has started fading in and is genuinely scaled below 1.0 (alpha > 0, not
    // discarded); the now fully-faded-out leaving page is discarded in turn.
    // The scale step paints inline: a `push_transform` scale about the page's
    // centre, then a `push_layer` for the fade, and no retained bracket.
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene = PageScene::default();
    root.paint(&mut scene, ft(50));
    assert!(
        scene.retained_pushes.is_empty() && scene.draws.is_empty(),
        "a scale step is never bracketed: {:?}",
        scene.retained_pushes
    );
    assert_eq!(
        scene.transforms.len(),
        1,
        "one scale transform: {:?}",
        scene.transforms
    );
    let sx = scene.transforms[0].as_coeffs()[0];
    assert!(
        sx < 1.0 && sx > 0.9,
        "the incoming page is scaled below 1.0 (was {sx})"
    );
    assert_eq!(scene.layers.len(), 1, "one fade layer: {:?}", scene.layers);
    assert!(
        scene.layers[0].2 > 0.0,
        "the incoming page is no longer discarded"
    );
    assert_eq!(
        (scene.transform_pops, scene.layer_pops),
        (1, 1),
        "both inline brackets are balanced"
    );
}

// ---------------------------------------------------------------------
// SlideUp preset + push_transparent_for_result.
// ---------------------------------------------------------------------

// --- SlideUp paint sequence: the entering sheet's origin moves bottom→top
//     as progress advances, settles exactly at rest, and is disposed (culling
//     resumes) once the transition finalizes — mirroring
//     `push_animates_moving_origins_and_disposes_after_settle` above, but
//     checking the vertical origin a horizontal preset never moves. ---

#[test]
fn slide_up_moves_origin_bottom_to_top_and_disposes_after_settle() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        // Root page A is 100x100; the pushed "sheet" B is 100 wide, 40 tall
        // (a distinct height so a test can tell it apart in the fills).
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let spec = TransitionSpec::new(
        PageTransition::SlideUp,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| sized_page(100.0, 40.0), spec);

    // Seed frame: driver at 0. The sheet (B) sits a full 100px (the
    // navigator's height) below rest; the page below (A) is untouched.
    let (f0, nf0) = full_frame(&mut root, &mut app, &mut state, ft(0));
    assert!(nf0, "a running transition requests frames");
    assert_eq!(fill_h(&f0, 100.0).0, Point::ZERO, "A never moves");
    let b0 = fill_h(&f0, 40.0).0;
    assert_eq!(b0.x, 0.0, "SlideUp never offsets horizontally");
    assert_eq!(b0.y, 100.0, "B starts a full height below rest");

    // Mid frame (50ms → progress 0.5): B has moved halfway up; A is still
    // static.
    let (f1, _) = full_frame(&mut root, &mut app, &mut state, ft(50));
    assert_eq!(
        fill_h(&f1, 100.0).0,
        Point::ZERO,
        "A stays static mid-transition"
    );
    let b1 = fill_h(&f1, 40.0).0;
    assert_eq!(b1.x, 0.0);
    assert!((b1.y - 50.0).abs() < 1e-6, "B is halfway up (was {})", b1.y);
    assert!(b1.y < b0.y, "B's origin moves toward the top (up)");

    // End frame (150ms → past the 100ms duration): settles at exact rest.
    let (f2, nf2) = full_frame(&mut root, &mut app, &mut state, ft(150));
    assert_eq!(
        fill_h(&f2, 40.0).0,
        Point::ZERO,
        "B rests exactly at origin"
    );
    assert!(nf2, "the settling frame still requests one finalize frame");

    // Finalize frame: culling resumes, only the sheet (now opaque top of the
    // *animated* stack — still the same page) is considered; here it was
    // pushed with `push_with` (opaque, the default), so only B survives.
    let (f3, nf3) = full_frame(&mut root, &mut app, &mut state, ft(300));
    assert!(!nf3, "no frame requested once the transition is disposed");
    assert_eq!(f3.len(), 1, "culling resumed: only the top page paints");
    assert_eq!(f3[0].1, Size::new(100.0, 40.0), "the surviving page is B");
}

// --- SlideUp pop: the sheet reverses (slides back down and out) while the
//     revealed page below never moves — the modal-sheet paint contract. ---

#[test]
fn slide_up_pop_slides_sheet_down_without_moving_revealed_page() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Push the sheet instantly, then animate the pop.
    controller.push_with(
        || sized_page(100.0, 40.0),
        TransitionSpec::new(
            PageTransition::SlideUp,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        ),
    );
    for t in [0u64, 50, 150, 300] {
        full_frame(&mut root, &mut app, &mut state, ft(t));
    }

    // Pop: the sheet (B) reverses, sliding back down; A (revealed) never
    // moves throughout. The pop starts a *fresh* driver (a new
    // `AnimationController` per `start_transition`), so this first frame
    // after `pop()` is its seed (progress 0, matching the push seed's
    // convention above) — the sheet still sits at rest here.
    controller.pop();
    let (f0, _) = full_frame(&mut root, &mut app, &mut state, ft(1000));
    assert_eq!(fill_h(&f0, 100.0).0, Point::ZERO, "A never moves on pop");
    assert_eq!(
        fill_h(&f0, 40.0).0,
        Point::ZERO,
        "the pop's seed frame: sheet still at rest"
    );

    // 50ms in (half the 100ms duration): the sheet has started sliding down;
    // A still hasn't moved.
    let (f1, _) = full_frame(&mut root, &mut app, &mut state, ft(1050));
    assert_eq!(fill_h(&f1, 100.0).0, Point::ZERO, "A still never moves");
    let b1 = fill_h(&f1, 40.0).0;
    assert_eq!(b1.x, 0.0);
    assert!(b1.y > 0.0, "the sheet has begun sliding down (y={})", b1.y);

    let mut last = Vec::new();
    for t in [1150u64, 1300] {
        let (fills, _) = full_frame(&mut root, &mut app, &mut state, ft(t));
        last = fills;
    }
    assert_eq!(last.len(), 1, "only the revealed page remains");
    assert_eq!(last[0].1, Size::new(100.0, 100.0), "the revealed page is A");
    assert_eq!(
        last[0].0,
        Point::ZERO,
        "A settles back at its resting origin"
    );
}

// --- Composed: transparent + result + SlideUp — the dialog/sheet-shaped
//     usage `push_transparent_for_result` exists for. The page below stays
//     visible throughout (transparent), the sheet animates in/out via
//     SlideUp, and the pop result still reaches the pusher's callback. ---

#[test]
fn transparent_result_slide_up_composed_dialog_usage() {
    let controller: NavigatorController<ResultState> = NavigatorController::new();
    let mut root: RenderRoot<ResultState, NavigatorView<ResultState>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ResultState| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ResultState::default();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let spec = TransitionSpec::new(
        PageTransition::SlideUp,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_transparent_for_result(
        || sized_page(100.0, 40.0),
        spec,
        |state: &mut ResultState, result: PopResult| {
            state.received = result.take::<i32>();
        },
    );

    // Seed frame (progress 0): the page below (A, 100 tall) already paints —
    // the sheet is transparent, so culling never hides it — while the sheet
    // (B, 40 tall) starts a full height below rest.
    let (f0, _) = full_frame(&mut root, &mut app, &mut state, ft(0));
    assert_eq!(fill_h(&f0, 100.0).0, Point::ZERO, "A stays visible below");
    assert_eq!(
        fill_h(&f0, 40.0).0,
        Point::new(0.0, 100.0),
        "B starts a full height below rest"
    );

    // Mid-animation (50ms of the 100ms duration): B is partway up; A is
    // still visible and unmoved.
    let (f1, _) = full_frame(&mut root, &mut app, &mut state, ft(50));
    assert_eq!(fill_h(&f1, 100.0).0, Point::ZERO, "A stays visible below");
    let b1 = fill_h(&f1, 40.0).0;
    assert!(b1.y > 0.0 && b1.y < 100.0, "B is mid-slide (y={})", b1.y);

    // Settle the push.
    for t in [150u64, 300] {
        full_frame(&mut root, &mut app, &mut state, ft(t));
    }
    // Still transparent: A remains visible under the settled sheet.
    let (settled, _) = full_frame(&mut root, &mut app, &mut state, ft(316));
    assert_eq!(settled.len(), 2, "both A and the settled sheet paint");

    // Pop the sheet with a result payload. `NavOp::Pop` queues (and, through
    // the housekeeping broadcast, flushes) the callback eagerly on the rebuild
    // that applies the pop — the out-animation that follows is purely visual.
    controller.pop_with_result(PopResult::of(99i32));
    full_frame(&mut root, &mut app, &mut state, ft(1000));
    assert_eq!(
        state.received,
        Some(99),
        "the transparent+SlideUp dialog delivers its pop result on the pop \
         frame, with no input"
    );
    // Drive the reverse transition to settle; nothing re-delivers.
    for t in [1050u64, 1150, 1300] {
        full_frame(&mut root, &mut app, &mut state, ft(t));
    }
    assert_eq!(state.received, Some(99));
}

// ---------------------------------------------------------------------
// Shared-element ("hero") transitions.
// ---------------------------------------------------------------------

/// A recording scene that separates page fills painted at the identity
/// transform (`plain`) from fills painted under a pushed transform
/// (`morphs`, recorded as their transformed bounding boxes) — so a hero test
/// can assert the morph overlay's interpolated rect and that a suppressed
/// endpoint painted nothing at its rest position.
///
/// `push_retained`/`pop_retained` are overridden separately, recording into
/// their own `retained_pushes`/`retained_pops` fields rather than falling into the
/// trait's emulating default: a retained-eligible page's bracket is entirely
/// unrelated to the hero morph's own `push_transform` (pushed from inside the
/// page's subtree by `hero.rs`'s `HeroDirective::Morph` arm), and letting the
/// default emulation push its own (identity, at `scale == 1.0`) transform
/// here would corrupt the `plain`/`morphs` classification above.
#[derive(Default)]
struct HeroScene {
    transforms: Vec<Affine>,
    plain: Vec<(Point, Size)>,
    morphs: Vec<Rect>,
    retained_pushes: Vec<(u64, Point, Size, f32, f64)>,
    retained_pops: u32,
}
impl PaintScene for HeroScene {
    fn fill_rect(&mut self, origin: Point, size: Size, _color: peniko::Color) {
        let rect = Rect::from_origin_size(origin, size);
        match self.transforms.last() {
            Some(t) => self.morphs.push(t.transform_rect_bbox(rect)),
            None => self.plain.push((origin, size)),
        }
    }
    fn push_retained(&mut self, key: u64, origin: Point, size: Size, alpha: f32, scale: f64) {
        self.retained_pushes.push((key, origin, size, alpha, scale));
    }
    fn pop_retained(&mut self) {
        self.retained_pops += 1;
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn push_transform(&mut self, transform: Affine) {
        let composed = self.transforms.last().copied().unwrap_or(Affine::IDENTITY) * transform;
        self.transforms.push(composed);
    }
    fn pop_transform(&mut self) {
        self.transforms.pop();
    }
}

/// A page that is a single tagged hero wrapping a fixed-size leaf (hero at
/// page-local origin).
fn hero_leaf_page(tag: &'static str, w: f64, h: f64) -> impl View<()> {
    crate::hero(
        tag,
        SizedLeaf {
            size: Size::new(w, h),
        },
    )
}

/// A page whose tagged hero is inset by `(left, top)` — so its page-local
/// rect differs from a [`hero_leaf_page`]'s, giving the morph a real
/// translation *and* (with a different size) scale to interpolate.
fn hero_offset_page(tag: &'static str, w: f64, h: f64, left: f64, top: f64) -> impl View<()> {
    crate::Padding(
        crate::EdgeInsets {
            left,
            top,
            right: 0.0,
            bottom: 0.0,
        },
        crate::hero(
            tag,
            SizedLeaf {
                size: Size::new(w, h),
            },
        ),
    )
}

fn hero_frame(
    root: &mut RenderRoot<(), NavigatorView<()>>,
    app: &mut impl FnMut(&mut ()) -> NavigatorView<()>,
    state: &mut (),
    time: FrameTime,
) -> (HeroScene, bool) {
    root.rebuild(app, state);
    root.layout(Size::new(200.0, 200.0));
    let mut scene = HeroScene::default();
    let out = root.paint(&mut scene, time);
    (scene, out.needs_frame)
}

/// Whether `rects` holds a rect approximately equal to `(origin, size)`.
fn has_rect(rects: &[Rect], origin: Point, size: Size) -> bool {
    rects.iter().any(|r| {
        (r.x0 - origin.x).abs() < 1e-6
            && (r.y0 - origin.y).abs() < 1e-6
            && (r.width() - size.width).abs() < 1e-6
            && (r.height() - size.height).abs() < 1e-6
    })
}

#[test]
fn hero_push_morphs_between_pages_and_suppresses_endpoints() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        // Root A: a 40x40 hero at (0,0). It is the leaving page's endpoint.
        move |_: &mut ()| navigator(&ctrl, || hero_leaf_page("avatar", 40.0, 40.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 200.0));

    // Push B: an 80x80 hero inset to (20,30) — the entering endpoint.
    let spec = TransitionSpec::new(
        PageTransition::M3SharedAxisX,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| hero_offset_page("avatar", 80.0, 80.0, 20.0, 30.0), spec);

    // Seed frame (p=0): no rects captured yet, so both endpoints paint
    // normally and no morph overlay is painted (discovery frame).
    let (f0, _) = hero_frame(&mut root, &mut app, &mut state, ft(0));
    assert!(f0.morphs.is_empty(), "no morph on the discovery frame");
    assert!(
        has_rect(
            &f0.plain
                .iter()
                .map(|(o, s)| Rect::from_origin_size(*o, *s))
                .collect::<Vec<_>>(),
            Point::new(0.0, 0.0),
            Size::new(40.0, 40.0),
        ),
        "A's hero paints normally on the seed frame: {:?}",
        f0.plain
    );
    // B (entering) is exactly alpha 0 at this same seed frame (the
    // M3SharedAxisX 0.35 split hasn't opened) — its own hero paints into a
    // `DiscardScene` sink and never reaches the real scene here. But
    // `ctx.report_hero` lives on `PaintCtx`, not the scene, so its rect is
    // still captured; the mid-frame morph below, which needs exactly that
    // captured rect to interpolate toward, is the proof invisibility never
    // loses a hero endpoint.
    assert!(
        !has_rect(
            &f0.plain
                .iter()
                .map(|(o, s)| Rect::from_origin_size(*o, *s))
                .collect::<Vec<_>>(),
            Point::new(50.0, 30.0),
            Size::new(80.0, 80.0),
        ),
        "B's hero is discarded (alpha 0) at the seed frame: {:?}",
        f0.plain
    );

    // Mid frame (p=0.5): the matched endpoints morph. The overlay rect is the
    // interpolation of A's (0,0,40,40) and B's (20,30,80,80):
    //   origin = (10, 15), size = (60, 60). Both endpoints are suppressed at
    //   their rest positions (only the morph paints the shared element).
    let (f1, _) = hero_frame(&mut root, &mut app, &mut state, ft(50));
    assert!(
        has_rect(&f1.morphs, Point::new(10.0, 15.0), Size::new(60.0, 60.0)),
        "morph overlay interpolates source→target: {:?}",
        f1.morphs
    );
    let plain_rects: Vec<Rect> = f1
        .plain
        .iter()
        .map(|(o, s)| Rect::from_origin_size(*o, *s))
        .collect();
    assert!(
        !has_rect(&plain_rects, Point::new(0.0, 0.0), Size::new(40.0, 40.0)),
        "A's endpoint is suppressed mid-flight: {:?}",
        f1.plain
    );

    // Settle + finalize: culling resumes, only B's hero paints — normally.
    let mut t = 150u64;
    loop {
        let (fr, needs) = hero_frame(&mut root, &mut app, &mut state, ft(t));
        if !needs {
            assert!(
                fr.morphs.is_empty(),
                "no morph once settled: {:?}",
                fr.morphs
            );
            assert!(
                has_rect(
                    &fr.plain
                        .iter()
                        .map(|(o, s)| Rect::from_origin_size(*o, *s))
                        .collect::<Vec<_>>(),
                    Point::new(20.0, 30.0),
                    Size::new(80.0, 80.0),
                ),
                "B's hero rests at its own position after settle: {:?}",
                fr.plain
            );
            break;
        }
        t += 16;
        assert!(t < 5000, "transition failed to settle");
    }
}

// --- A hero push never paints either matched page through a retained
//     bracket while it carries a hero directive this frame: a morph paints
//     inside the page and must move every frame, so a page with a hero in
//     play is never an "ideal cache candidate". ---

#[test]
fn hero_push_mid_transition_carries_no_retained_bracket_on_either_page() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || hero_leaf_page("avatar", 40.0, 40.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 200.0));

    let spec = TransitionSpec::new(
        PageTransition::M3SharedAxisX,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| hero_offset_page("avatar", 80.0, 80.0, 20.0, 30.0), spec);

    // Seed frame: discovery, no rects captured yet, so `hero_directives`
    // returns empty maps for both pages this frame — nothing to assert here
    // (B is also alpha 0 and discarded at the seed of this preset), the
    // interesting frame is the next one.
    hero_frame(&mut root, &mut app, &mut state, ft(0));

    // Mid frame (p=0.5): the tag is matched on both pages now, so each
    // carries a hero directive this frame (Morph on the entering side,
    // Suppress on the leaving one) — neither is retained-eligible.
    let (f1, _) = hero_frame(&mut root, &mut app, &mut state, ft(50));
    assert!(
        f1.retained_pushes.is_empty(),
        "no page paints through a retained bracket while it carries a hero \
         directive: {:?}",
        f1.retained_pushes
    );
}

#[test]
fn hero_tag_on_one_side_only_never_morphs() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || hero_leaf_page("avatar", 40.0, 40.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 200.0));

    // Push B carrying a DIFFERENT tag: no tag is present on both pages, so no
    // morph ever paints and both heroes paint normally throughout.
    let spec = TransitionSpec::new(
        PageTransition::M3SharedAxisX,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| hero_offset_page("other", 80.0, 80.0, 20.0, 30.0), spec);

    for t in [0u64, 50, 100] {
        let (fr, _) = hero_frame(&mut root, &mut app, &mut state, ft(t));
        assert!(
            fr.morphs.is_empty(),
            "an unmatched tag must not morph (frame {t}): {:?}",
            fr.morphs
        );
    }
}

#[test]
fn hero_pop_morphs_backward_and_finalizes_normal() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || hero_leaf_page("avatar", 40.0, 40.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 200.0));

    // Push B (animated) and let it settle so we start the pop from rest.
    let spec = TransitionSpec::new(
        PageTransition::M3SharedAxisX,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| hero_offset_page("avatar", 80.0, 80.0, 20.0, 30.0), spec);
    let mut t = 0u64;
    loop {
        let (_, needs) = hero_frame(&mut root, &mut app, &mut state, ft(t));
        if !needs {
            break;
        }
        t += 16;
        assert!(t < 5000, "push failed to settle");
    }

    // Pop B: the pop reverses B's stored transition and morphs the shared
    // element from B's rest rect back toward A's.
    controller.pop();
    // Seed frame (discovery).
    hero_frame(&mut root, &mut app, &mut state, ft(t));
    // Mid frame, BEFORE the 0.35 split (20ms of the 100ms transition): B (the
    // popped page) paints the morph overlay here (it is "on top" during a
    // pop), and its own `Layer.alpha` — same fade-out curve as the plain-page
    // tests above — is still positive before the split; past it, B is
    // discarded (alpha 0) and so is its morph, exactly as any other alpha-0
    // paint. Sampling before the split keeps this assertion
    // about the morph itself, not about that discard.
    let (fmid, _) = hero_frame(&mut root, &mut app, &mut state, ft(t + 20));
    assert!(
        !fmid.morphs.is_empty(),
        "the pop paints a hero morph overlay: {:?}",
        fmid.morphs
    );

    // Settle + finalize: only the revealed root A remains, painting normally.
    let mut t2 = t + 100;
    loop {
        let (fr, needs) = hero_frame(&mut root, &mut app, &mut state, ft(t2));
        if !needs {
            assert!(
                fr.morphs.is_empty(),
                "no morph once popped: {:?}",
                fr.morphs
            );
            assert!(
                has_rect(
                    &fr.plain
                        .iter()
                        .map(|(o, s)| Rect::from_origin_size(*o, *s))
                        .collect::<Vec<_>>(),
                    Point::new(0.0, 0.0),
                    Size::new(40.0, 40.0),
                ),
                "the revealed root hero paints normally after the pop: {:?}",
                fr.plain
            );
            break;
        }
        t2 += 16;
        assert!(t2 < 10000, "pop failed to settle");
    }
}

#[test]
fn hero_edge_swipe_cancel_restores_endpoints() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || hero_leaf_page("avatar", 40.0, 40.0)).pop_swipe(true)
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 200.0));

    // Push B with the iOS-push preset so it is swipe-poppable; settle it.
    let spec = TransitionSpec::new(
        PageTransition::IosPush,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.push_with(|| hero_offset_page("avatar", 80.0, 80.0, 20.0, 30.0), spec);
    let mut t = 0u64;
    loop {
        let (_, needs) = hero_frame(&mut root, &mut app, &mut state, ft(t));
        if !needs {
            break;
        }
        t += 16;
        assert!(t < 5000, "push failed to settle");
    }

    // Begin an edge swipe: a left-edge Down then a rightward drag steals into
    // an interactive (Held) pop.
    root.event(&mut state, &down(5.0, 100.0));
    hero_frame(&mut root, &mut app, &mut state, ft(t + 16));
    root.event(&mut state, &move_to(40.0, 100.0));
    assert!(
        nav_widget(&root).transition.is_some(),
        "the edge drag started an interactive pop"
    );
    // A held frame past discovery paints the morph following the drag.
    hero_frame(&mut root, &mut app, &mut state, ft(t + 32));
    let (held, _) = hero_frame(&mut root, &mut app, &mut state, ft(t + 48));
    assert!(
        !held.morphs.is_empty(),
        "the interactive pop paints a hero morph while held: {:?}",
        held.morphs
    );

    // Drag back toward the edge (low progress, leftward velocity) and release
    // there → the pop cancels and springs back rather than completing.
    root.event(&mut state, &move_to(8.0, 100.0));
    hero_frame(&mut root, &mut app, &mut state, ft(t + 64));
    root.event(&mut state, &up(7.0, 100.0));
    let mut t2 = t + 80;
    loop {
        let (fr, needs) = hero_frame(&mut root, &mut app, &mut state, ft(t2));
        if !needs {
            assert_eq!(
                nav_widget(&root).pages.len(),
                2,
                "the cancelled pop restored B onto the stack"
            );
            assert!(
                fr.morphs.is_empty(),
                "no morph lingers after the cancel settles: {:?}",
                fr.morphs
            );
            assert!(
                has_rect(
                    &fr.plain
                        .iter()
                        .map(|(o, s)| Rect::from_origin_size(*o, *s))
                        .collect::<Vec<_>>(),
                    Point::new(20.0, 30.0),
                    Size::new(80.0, 80.0),
                ),
                "B's hero paints normally after the cancel: {:?}",
                fr.plain
            );
            break;
        }
        t2 += 16;
        assert!(t2 < 10000, "cancel failed to settle");
    }
}

// --- The published TransitionState, observed from OUTSIDE the navigator
//     subtree (the case the seam exists for). ---

/// What a chrome probe stacked above the navigator saw, per pass.
#[derive(Default)]
struct ProbeLog {
    /// `TransitionState::active` as read from each `View::build`/`rebuild`
    /// (a `BuildCtx` pass).
    build_active: Vec<bool>,
    /// `TransitionState::progress` as read from each `Widget::paint`.
    paint_progress: Vec<f64>,
    /// `TransitionState::generation` as read from each `Widget::paint`.
    paint_generation: Vec<u32>,
}

/// A zero-content "chrome" widget that only *observes* the navigator through
/// a controller clone — the sibling-not-descendant position the seam exists
/// for.
struct ProbeView {
    controller: NavigatorController<()>,
    log: Rc<RefCell<ProbeLog>>,
}
struct ProbeWidget {
    controller: NavigatorController<()>,
    log: Rc<RefCell<ProbeLog>>,
}
impl ProbeView {
    fn record_build(&self) {
        let active = self.controller.transition().active;
        self.log.borrow_mut().build_active.push(active);
    }
}
impl View<()> for ProbeView {
    type Element = ProbeWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
        self.record_build();
        ProbeWidget {
            controller: self.controller.clone(),
            log: self.log.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut ProbeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.record_build();
        ChangeFlags::NONE
    }
}
impl Widget for ProbeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(1.0, 1.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        let t = self.controller.transition();
        let mut log = self.log.borrow_mut();
        log.paint_progress.push(t.progress);
        log.paint_generation.push(t.generation);
    }
}

/// One full frame of a `Stack(vec![navigator, probe])` root, returning the
/// navigator's recorded page fills plus whether another frame was requested.
fn probe_frame(
    root: &mut RenderRoot<(), AnyView<()>>,
    app: &mut impl FnMut(&mut ()) -> AnyView<()>,
    state: &mut (),
    time: FrameTime,
) -> (Vec<(Point, Size, f32)>, bool) {
    root.rebuild(app, state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene = TransitionScene::default();
    let out = root.paint(&mut scene, time);
    (scene.fills, out.needs_frame)
}

#[test]
fn transition_state_is_frame_exact_for_chrome_painted_after_the_navigator() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let log = Rc::new(RefCell::new(ProbeLog::default()));

    // Before any navigator attaches: the at-rest default.
    let idle = controller.transition();
    assert!(!idle.active, "no navigator attached yet");
    assert_eq!(idle.generation, 0);

    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        let log = log.clone();
        move |_: &mut ()| {
            any(stack()
                .child(navigator(&ctrl, || sized_page(100.0, 100.0)))
                .child(ProbeView {
                    controller: ctrl.clone(),
                    log: log.clone(),
                }))
        }
    };
    let mut state = ();

    probe_frame(&mut root, &mut app, &mut state, ft(0));
    let at_rest = controller.transition();
    assert!(
        !at_rest.active,
        "a settled navigator publishes an idle state"
    );
    assert_eq!(
        (at_rest.from_depth, at_rest.to_depth),
        (1, 1),
        "an idle snapshot's endpoints are both the current stack"
    );

    // Push B on a 100ms linear shared-axis transition: 30dp of entering slide
    // makes the navigator's own painted progress recoverable from the fills.
    let spec = TransitionSpec::new(
        PageTransition::M3SharedAxisX,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    let builds_before = log.borrow().build_active.len();
    controller.push_with(|| sized_page(100.0, 80.0), spec);

    // Seed frame: establishes the driver's baseline (progress 0). B is
    // exactly alpha 0 there (the 0.35 split hasn't opened) and is discarded
    // rather than painted, so it has no fill to invert progress from —
    // sampled separately, not folded into `painted` below.
    probe_frame(&mut root, &mut app, &mut state, ft(0));

    // Sample only past the 0.35 split, where B is genuinely visible (alpha >
    // 0) and paints under its ordinary bracket.
    let mut painted = Vec::new();
    for ms in [40u64, 55, 70, 85] {
        let (fills, needs) = probe_frame(&mut root, &mut app, &mut state, ft(ms));
        assert!(needs, "a running transition keeps requesting frames");
        painted.push(fill_h(&fills, 80.0).0.x);
    }

    let log = log.borrow();
    // (a) The BUILD-time edge: the probe builds after the navigator, and
    //     `start_transition` publishes from that same BuildCtx pass, so
    //     `active` is visible on the very frame of the push.
    assert!(
        log.build_active[builds_before],
        "a build-time reader sees `active` flip on the push frame: {:?}",
        log.build_active
    );

    // (b) The PAINT-time reads: exactly the value the navigator painted with.
    //     Entering dx is `30 * (1 - progress)` for a shared-axis push, so
    //     inverting the geometry recovers the navigator's own progress.
    let reads = &log.paint_progress[log.paint_progress.len() - painted.len()..];
    for (i, (&p, &x)) in reads.iter().zip(painted.iter()).enumerate() {
        assert!(
            (30.0 * (1.0 - p) - x).abs() < 1e-9,
            "frame {i}: probe read {p} but the navigator painted at x={x}"
        );
    }
    // (c) Strictly increasing across frames.
    for pair in reads.windows(2) {
        assert!(
            pair[1] > pair[0],
            "progress advances monotonically: {reads:?}"
        );
    }
    // (d) One transition = one generation, bumped exactly once.
    let generations = &log.paint_generation[log.paint_generation.len() - painted.len()..];
    assert!(
        generations.iter().all(|g| *g == 1),
        "one started transition = generation 1 throughout: {generations:?}"
    );
    drop(log);

    // Settle, then finalize: `active` falls, the depths collapse onto the new
    // stack, and the generation is retained so an observer can still tell
    // WHICH transition ended.
    probe_frame(&mut root, &mut app, &mut state, ft(150));
    let (_, needs) = probe_frame(&mut root, &mut app, &mut state, ft(200));
    assert!(!needs, "the transition finalized");
    let done = controller.transition();
    assert!(!done.active);
    assert!(!done.interactive);
    assert_eq!((done.from_depth, done.to_depth), (2, 2));
    assert_eq!(done.generation, 1);
    assert_eq!(done.progress, 1.0, "a settled snapshot is fully arrived");
}

#[test]
fn transition_state_marks_a_pop_and_survives_an_instant_stack_mutation() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // An INSTANT (non-animated) push starts no transition at all, but
    // `publish_state` still leaves a coherent snapshot on the new depth.
    controller.push(|| sized_page(100.0, 80.0));
    root.rebuild(&mut app, &mut state);
    let t = controller.transition();
    assert!(!t.active, "an instant push animates nothing");
    assert_eq!((t.from_depth, t.to_depth), (2, 2));
    assert_eq!(t.generation, 0, "no transition started, no generation bump");

    // An ANIMATED pop publishes `is_pop` with the shrinking depth pair.
    let spec = TransitionSpec::new(
        PageTransition::M3SharedAxisX,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    );
    controller.replace_with(|| sized_page(100.0, 80.0), spec);
    full_frame(&mut root, &mut app, &mut state, ft(0));
    let t = controller.transition();
    assert!(t.active);
    assert!(!t.is_pop, "a replace animates in the push direction");
    assert_eq!(
        (t.from_depth, t.to_depth),
        (2, 2),
        "a replace swaps in place: the depth is unchanged"
    );
    // Let it finish so the pop below starts clean.
    full_frame(&mut root, &mut app, &mut state, ft(150));
    full_frame(&mut root, &mut app, &mut state, ft(200));

    controller.pop();
    full_frame(&mut root, &mut app, &mut state, ft(216));
    let t = controller.transition();
    assert!(t.active);
    assert!(t.is_pop, "a pop of an animated page runs backwards");
    assert!(!t.interactive, "a programmatic pop is not a drag");
    assert_eq!((t.from_depth, t.to_depth), (2, 1));
    assert_eq!(t.generation, 2, "second transition of this navigator");
}

#[test]
fn transition_state_tracks_the_interactive_edge_swipe() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
    };
    let mut state = ();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(100.0, 100.0));
    controller.push(|| sized_page(100.0, 80.0));
    full_frame(&mut root, &mut app, &mut state, ft(0));

    // Arm on the left edge, then steal with a decisive rightward drag.
    root.event(&mut state, &down(5.0, 50.0));
    root.event(&mut state, &move_to(45.0, 50.0));
    let held = controller.transition();
    assert!(held.active, "the steal started a transition");
    assert!(held.is_pop);
    assert!(held.interactive, "a drag is holding the progress");
    assert_eq!((held.from_depth, held.to_depth), (2, 1));
    assert!(held.progress > 0.0, "the drag carried initial progress");

    // Dragging further advances the published progress in place.
    root.event(&mut state, &move_to(70.0, 50.0));
    let further = controller.transition();
    assert!(further.progress > held.progress);
    assert!(further.interactive);
    assert_eq!(
        further.generation, held.generation,
        "the same transition, later — not a new one"
    );

    // Release: a spring drives it from here, so `interactive` clears while the
    // transition stays active.
    root.event(&mut state, &up(70.0, 50.0));
    let released = controller.transition();
    assert!(released.active, "the settle is still a live transition");
    assert!(!released.interactive, "the finger let go");
}
