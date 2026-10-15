//! Retained-page boundaries at the display-list level: `repaint_boundary`,
//! `AnimatedOpacity::cached(true)` and the navigator's pages, driven through a
//! `RenderRoot` with a hand-set resident-key ledger (the renderer's role) and
//! painted into a recorder that keeps every command in order.

use std::time::Duration;

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, Curve, FrameTime, LayoutCtx, PaintCtx, PaintScene,
    RenderRoot, RetainedLedger, View, Widget,
};
use frust_widgets::motion::animated_opacity;
use frust_widgets::{
    NavigatorController, NavigatorView, PageTransition, Timing, TransitionSpec, navigator,
    repaint_boundary,
};
use kurbo::{Affine, Point, Size};

const WINDOW: Size = Size::new(100.0, 100.0);

// ---------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------

/// One recorded paint command.
#[derive(Clone, Debug, PartialEq)]
enum Op {
    Fill(Point, Size),
    PushRetained {
        key: u64,
        origin: Point,
        size: Size,
        alpha: f32,
        scale: f64,
    },
    PopRetained,
    DrawRetained {
        key: u64,
        origin: Point,
        size: Size,
        alpha: f32,
        scale: f64,
    },
    PushLayer(f32),
    PopLayer,
    PushTransform(Affine),
    PopTransform,
}

/// A recorder tracking the transform stack the way `SceneBuilder` does, from a
/// root device scale, so a boundary reads the scale its page would be drawn at.
struct Rec {
    ops: Vec<Op>,
    stack: Vec<Affine>,
}

impl Rec {
    fn at_scale(scale: f64) -> Self {
        Self {
            ops: Vec::new(),
            stack: vec![Affine::scale(scale)],
        }
    }

    fn new() -> Self {
        Self::at_scale(1.0)
    }

    fn fills(&self) -> usize {
        self.ops
            .iter()
            .filter(|op| matches!(op, Op::Fill(..)))
            .count()
    }

    fn pushes(&self) -> Vec<(u64, Point, Size, f32, f64)> {
        self.ops
            .iter()
            .filter_map(|op| match *op {
                Op::PushRetained {
                    key,
                    origin,
                    size,
                    alpha,
                    scale,
                } => Some((key, origin, size, alpha, scale)),
                _ => None,
            })
            .collect()
    }

    fn draws(&self) -> Vec<(u64, Point, Size, f32, f64)> {
        self.ops
            .iter()
            .filter_map(|op| match *op {
                Op::DrawRetained {
                    key,
                    origin,
                    size,
                    alpha,
                    scale,
                } => Some((key, origin, size, alpha, scale)),
                _ => None,
            })
            .collect()
    }

    fn retained_ops(&self) -> usize {
        self.ops
            .iter()
            .filter(|op| {
                matches!(
                    op,
                    Op::PushRetained { .. } | Op::PopRetained | Op::DrawRetained { .. }
                )
            })
            .count()
    }
}

impl PaintScene for Rec {
    fn fill_rect(&mut self, origin: Point, size: Size, _color: peniko::Color) {
        self.ops.push(Op::Fill(origin, size));
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
        self.ops.push(Op::PushLayer(alpha));
    }
    fn pop_layer(&mut self) {
        self.ops.push(Op::PopLayer);
    }
    fn push_transform(&mut self, transform: Affine) {
        let current = *self.stack.last().expect("root transform");
        self.stack.push(current * transform);
        self.ops.push(Op::PushTransform(transform));
    }
    fn pop_transform(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
        self.ops.push(Op::PopTransform);
    }
    fn push_retained(&mut self, key: u64, origin: Point, size: Size, alpha: f32, scale: f64) {
        self.ops.push(Op::PushRetained {
            key,
            origin,
            size,
            alpha,
            scale,
        });
    }
    fn pop_retained(&mut self) {
        self.ops.push(Op::PopRetained);
    }
    fn draw_retained(&mut self, key: u64, origin: Point, size: Size, alpha: f32, scale: f64) {
        self.ops.push(Op::DrawRetained {
            key,
            origin,
            size,
            alpha,
            scale,
        });
    }
    fn current_transform(&self) -> Option<Affine> {
        self.stack.last().copied()
    }
}

/// A fixed-size leaf filling its rect; `shade` stands for its paint content,
/// so changing it is a paint-only change.
struct Swatch {
    size: Size,
    shade: u32,
}

fn swatch(w: f64, h: f64, shade: u32) -> Swatch {
    Swatch {
        size: Size::new(w, h),
        shade,
    }
}

struct SwatchWidget {
    size: Size,
}

impl<S: 'static> View<S> for Swatch {
    type Element = SwatchWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SwatchWidget {
        SwatchWidget { size: self.size }
    }
    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SwatchWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT;
        }
        if prev.shade != self.shade {
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for SwatchWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.size)
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
    }
}

fn ms(ms: u64) -> FrameTime {
    FrameTime::from_nanos(ms * 1_000_000)
}

/// A ledger holding `key` resident at `extent_px` and `scale`.
fn ledger(key: u64, extent_px: (u32, u32), scale: f32) -> RetainedLedger {
    RetainedLedger::from_entries(1, [(key, extent_px, scale)])
}

/// Rebuild, lay out and paint one frame into `scene`.
fn frame<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    app: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    scene: &mut Rec,
    time: FrameTime,
) {
    root.rebuild(app, state);
    root.layout(WINDOW);
    root.paint(scene, time);
}

// ---------------------------------------------------------------------------
// repaint_boundary.
// ---------------------------------------------------------------------------

/// The boundary over a 40x30 swatch: record on the first frame, return its key.
fn recorded_boundary<V: View<u32>>(
    root: &mut RenderRoot<u32, V>,
    app: &mut impl FnMut(&mut u32) -> V,
    state: &mut u32,
) -> u64 {
    let mut scene = Rec::new();
    frame(root, app, state, &mut scene, ms(0));
    let pushes = scene.pushes();
    assert_eq!(pushes.len(), 1, "frame 1 records the body: {:?}", scene.ops);
    pushes[0].0
}

#[test]
fn frame_one_records_and_an_unchanged_frame_two_draws_the_page() {
    let mut root: RenderRoot<u32, _> = RenderRoot::new();
    let mut app = |s: &mut u32| repaint_boundary(swatch(40.0, 30.0, *s));
    let mut state = 0u32;

    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
    let key = match scene.ops.first() {
        Some(Op::PushRetained { key, .. }) => *key,
        other => panic!("frame 1 opens with a PushRetained, got {other:?}"),
    };
    let size = Size::new(40.0, 30.0);
    assert_eq!(
        scene.ops,
        vec![
            Op::PushRetained {
                key,
                origin: Point::ZERO,
                size,
                alpha: 1.0,
                scale: 1.0,
            },
            Op::Fill(Point::ZERO, size),
            Op::PopRetained,
        ],
        "frame 1: PushRetained + body + PopRetained"
    );

    root.set_retained_ledger(&ledger(key, (40, 30), 1.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
    assert_eq!(
        scene.ops,
        vec![Op::DrawRetained {
            key,
            origin: Point::ZERO,
            size,
            alpha: 1.0,
            scale: 1.0,
        }],
        "frame 2: the page alone, zero body commands"
    );
}

#[test]
fn a_leaf_change_under_the_boundary_re_emits_the_body() {
    let mut root: RenderRoot<u32, _> = RenderRoot::new();
    let mut app = |s: &mut u32| repaint_boundary(swatch(40.0, 30.0, *s));
    let mut state = 0u32;
    let key = recorded_boundary(&mut root, &mut app, &mut state);
    root.set_retained_ledger(&ledger(key, (40, 30), 1.0));

    state = 1;
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
    assert!(
        scene.draws().is_empty(),
        "a changed body never draws the stale page"
    );
    assert_eq!(
        scene.pushes().len(),
        1,
        "it is recorded again: {:?}",
        scene.ops
    );
    assert_eq!(scene.pushes()[0].0, key, "under the same key");
    assert_eq!(scene.fills(), 1, "the body is painted");

    // Unchanged again: the re-recorded page is drawn.
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(32));
    assert_eq!(scene.draws().len(), 1, "{:?}", scene.ops);
    assert_eq!(scene.fills(), 0);
}

#[test]
fn a_ledger_entry_at_another_extent_or_scale_re_emits_the_body() {
    let mut root: RenderRoot<u32, _> = RenderRoot::new();
    let mut app = |s: &mut u32| repaint_boundary(swatch(40.0, 30.0, *s));
    let mut state = 0u32;
    let key = recorded_boundary(&mut root, &mut app, &mut state);

    for (extent, scale) in [((41, 30), 1.0), ((40, 31), 1.0), ((40, 30), 2.0)] {
        root.set_retained_ledger(&ledger(key, extent, scale));
        let mut scene = Rec::new();
        frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
        assert!(
            scene.draws().is_empty(),
            "a page at {extent:?}/{scale} is never drawn: {:?}",
            scene.ops
        );
        assert_eq!(scene.pushes().len(), 1, "the body is bracketed again");
        assert_eq!(scene.fills(), 1);
    }
}

#[test]
fn the_boundary_asks_for_its_page_at_the_scenes_device_scale() {
    let mut root: RenderRoot<u32, _> = RenderRoot::new();
    let mut app = |s: &mut u32| repaint_boundary(swatch(40.0, 30.0, *s));
    let mut state = 0u32;
    let mut scene = Rec::at_scale(2.0);
    frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
    let key = scene.pushes()[0].0;
    assert_eq!(
        scene.pushes()[0].4,
        1.0,
        "the bracket's own scale is always 1"
    );

    // A 1x page is not the 2x page this scene draws at.
    root.set_retained_ledger(&ledger(key, (40, 30), 1.0));
    let mut scene = Rec::at_scale(2.0);
    frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
    assert!(scene.draws().is_empty(), "{:?}", scene.ops);

    root.set_retained_ledger(&ledger(key, (80, 60), 2.0));
    let mut scene = Rec::at_scale(2.0);
    frame(&mut root, &mut app, &mut state, &mut scene, ms(32));
    assert_eq!(
        scene.draws(),
        vec![(key, Point::ZERO, Size::new(40.0, 30.0), 1.0, 1.0)],
        "the 2x page is drawn at a 2x device scale"
    );
}

#[test]
fn a_page_recorded_under_one_device_scale_is_not_drawn_under_another() {
    let mut root: RenderRoot<u32, _> = RenderRoot::new();
    let mut app = |s: &mut u32| repaint_boundary(swatch(40.0, 30.0, *s));
    let mut state = 0u32;
    let mut scene = Rec::at_scale(2.0);
    frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
    let key = scene.pushes()[0].0;

    // The ledger (one frame stale) still says 2x, but this frame draws at 3x.
    root.set_retained_ledger(&ledger(key, (80, 60), 2.0));
    let mut scene = Rec::at_scale(3.0);
    frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
    assert!(scene.draws().is_empty(), "{:?}", scene.ops);
    assert_eq!(scene.pushes().len(), 1, "re-recorded under the new scale");
}

#[test]
fn the_composite_alpha_rides_both_the_bracket_and_the_draw() {
    let mut root: RenderRoot<u32, _> = RenderRoot::new();
    let mut app = |s: &mut u32| repaint_boundary(swatch(40.0, 30.0, *s)).alpha(0.5);
    let mut state = 0u32;
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
    let (key, _, _, alpha, _) = scene.pushes()[0];
    assert_eq!(alpha, 0.5, "the bracket carries the composite alpha");
    assert!(
        !scene.ops.iter().any(|op| matches!(op, Op::PushLayer(_))),
        "and no layer of its own"
    );

    root.set_retained_ledger(&ledger(key, (40, 30), 1.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
    assert_eq!(scene.draws()[0].3, 0.5, "the draw carries it too");
}

#[test]
fn the_kill_switch_paints_plain_bodies_with_no_bracket() {
    let mut root: RenderRoot<u32, _> = RenderRoot::new();
    root.set_retained_layers(false);
    let mut app = |s: &mut u32| repaint_boundary(swatch(40.0, 30.0, *s)).alpha(0.5);
    let mut state = 0u32;
    for t in [0, 16, 32] {
        let mut scene = Rec::new();
        frame(&mut root, &mut app, &mut state, &mut scene, ms(t));
        assert_eq!(
            scene.ops,
            vec![
                Op::PushLayer(0.5),
                Op::Fill(Point::ZERO, Size::new(40.0, 30.0)),
                Op::PopLayer,
            ],
            "frame at {t}ms: a plain body (under its fade), no retained command"
        );
        // Whatever the ledger says.
        root.set_retained_ledger(&ledger(0, (40, 30), 1.0));
    }
}

/// A leaf that reports an input shield every paint — a paint-time side
/// channel a composited page could not reproduce.
struct ShieldLeaf;

struct ShieldLeafWidget;

impl<S: 'static> View<S> for ShieldLeaf {
    type Element = ShieldLeafWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ShieldLeafWidget {
        ShieldLeafWidget
    }
    fn rebuild(&self, _: &Self, _: &mut ShieldLeafWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for ShieldLeafWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(40.0, 30.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
        ctx.report_input_shield(kurbo::Rect::from_origin_size(ctx.origin(), ctx.size()));
    }
}

#[test]
fn a_body_publishing_a_paint_side_channel_is_painted_every_frame() {
    let mut root: RenderRoot<(), _> = RenderRoot::new();
    let mut app = |_: &mut ()| repaint_boundary(ShieldLeaf);
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(0));
    let key = scene.pushes()[0].0;

    root.set_retained_ledger(&ledger(key, (40, 30), 1.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(16));
    assert!(
        scene.draws().is_empty(),
        "a page would drop the shield: {:?}",
        scene.ops
    );
    assert_eq!(
        scene.fills(),
        1,
        "the body paints and re-reports its shield"
    );
    assert_eq!(root.input_shields().len(), 1);
}

// ---------------------------------------------------------------------------
// AnimatedOpacity::cached(true).
// ---------------------------------------------------------------------------

#[test]
fn a_cached_fade_over_an_unchanged_child_draws_the_page_at_the_changing_alpha() {
    let timing = Timing::Duration(Duration::from_millis(100), Curve::Linear);
    let mut root: RenderRoot<f64, _> = RenderRoot::new();
    let mut app = move |s: &mut f64| {
        animated_opacity(*s, swatch(40.0, 30.0, 0))
            .timing(timing)
            .cached(true)
    };
    let mut state = 1.0f64;

    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
    let pushes = scene.pushes();
    assert_eq!(
        pushes.len(),
        1,
        "the child is recorded at rest: {:?}",
        scene.ops
    );
    let key = pushes[0].0;
    root.set_retained_ledger(&ledger(key, (40, 30), 1.0));

    // Fade out: the child itself never changes.
    state = 0.0;
    let mut alphas = Vec::new();
    for t in [16, 66, 116] {
        let mut scene = Rec::new();
        frame(&mut root, &mut app, &mut state, &mut scene, ms(t));
        let draws = scene.draws();
        assert_eq!(
            draws.len(),
            1,
            "frame at {t}ms draws the page: {:?}",
            scene.ops
        );
        assert_eq!(draws[0].0, key);
        assert_eq!(scene.fills(), 0, "no body commands while fading");
        alphas.push(draws[0].3);
    }
    assert!(
        alphas.windows(2).all(|w| w[1] < w[0]),
        "the page's alpha follows the fade: {alphas:?}"
    );
    assert_eq!(*alphas.last().unwrap(), 0.0, "and lands on the target");
}

#[test]
fn an_uncached_fade_keeps_its_plain_layer() {
    let mut root: RenderRoot<f64, _> = RenderRoot::new();
    let mut app = |s: &mut f64| animated_opacity(*s, swatch(40.0, 30.0, 0));
    let mut state = 0.5f64;
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
    assert_eq!(scene.retained_ops(), 0, "{:?}", scene.ops);
    assert_eq!(scene.ops[0], Op::PushLayer(0.5));
}

// ---------------------------------------------------------------------------
// The navigator's pages.
// ---------------------------------------------------------------------------

type NavApp = Box<dyn FnMut(&mut ()) -> NavigatorView<()>>;

fn nav() -> (
    NavigatorController<()>,
    RenderRoot<(), NavigatorView<()>>,
    NavApp,
) {
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let ctrl = controller.clone();
    let mut app: NavApp = Box::new(move |_: &mut ()| navigator(&ctrl, || swatch(100.0, 100.0, 0)));
    root.rebuild(&mut app, &mut ());
    root.layout(WINDOW);
    let mut scene = Rec::new();
    root.paint(&mut scene, ms(0));
    (controller, root, app)
}

fn spec(preset: PageTransition) -> TransitionSpec {
    TransitionSpec::new(
        preset,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    )
}

#[test]
fn a_navigator_fade_composites_the_outgoing_page_once_it_is_resident() {
    let (controller, mut root, mut app) = nav();
    controller.push_with(
        || swatch(100.0, 80.0, 0),
        spec(PageTransition::ReducedCrossfade),
    );

    // Seed frame (progress 0): the outgoing page still at rest is recorded; the
    // incoming one is at alpha 0.
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(1000));
    let pushes = scene.pushes();
    assert_eq!(pushes.len(), 1, "{:?}", scene.ops);
    let (outgoing, origin, size, alpha, scale) = pushes[0];
    assert_eq!(
        (origin, size, alpha, scale),
        (Point::ZERO, WINDOW, 1.0, 1.0)
    );

    root.set_retained_ledger(&ledger(outgoing, (100, 100), 1.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(1050));
    let draws = scene.draws();
    assert_eq!(
        draws.len(),
        1,
        "the outgoing page is composited: {:?}",
        scene.ops
    );
    assert_eq!(draws[0].0, outgoing);
    assert!(
        (draws[0].3 - 0.5).abs() < 1e-6,
        "at the fade's alpha (was {})",
        draws[0].3
    );
    // The incoming page (never recorded, freshly built) is bracketed at its
    // own alpha under its own key: only its body is painted.
    let pushes = scene.pushes();
    assert_eq!(pushes.len(), 1);
    assert_ne!(pushes[0].0, outgoing, "each page has its own key");
    assert!((pushes[0].3 - 0.5).abs() < 1e-6);
    assert_eq!(scene.fills(), 1, "only the incoming body is painted");
}

#[test]
fn a_navigator_slide_moves_the_composited_page_by_a_translation() {
    let (controller, mut root, mut app) = nav();
    controller.push_with(|| swatch(100.0, 80.0, 0), spec(PageTransition::IosPush));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(1000));
    let outgoing = scene.pushes()[0].0;

    // IosPush, progress 0.5: the page below parallaxes left by a third of the
    // width times the progress and dims; the incoming page overhangs the
    // navigator and paints directly.
    root.set_retained_ledger(&ledger(outgoing, (100, 100), 1.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(1050));
    let at = scene
        .ops
        .iter()
        .position(|op| matches!(op, Op::DrawRetained { .. }))
        .unwrap_or_else(|| panic!("the outgoing page is composited: {:?}", scene.ops));
    let Op::PushTransform(shift) = scene.ops[at - 1] else {
        panic!("a translation precedes the draw: {:?}", scene.ops);
    };
    assert_eq!(
        shift.as_coeffs()[..4],
        [1.0, 0.0, 0.0, 1.0],
        "a pure translation"
    );
    let t = shift.translation();
    assert!(
        (t.x - (-0.5 * 100.0 / 3.0)).abs() < 1e-9 && t.y == 0.0,
        "by the parallax (was {t:?})"
    );
    assert_eq!(scene.ops[at + 1], Op::PopTransform);
    assert_eq!(
        scene.pushes().len(),
        0,
        "the overhanging incoming page is not bracketed"
    );
    assert_eq!(scene.fills(), 1, "only the incoming body is painted");
}

#[test]
fn a_navigator_pop_composites_the_popped_page_as_it_leaves() {
    let (controller, mut root, mut app) = nav();
    controller.push_with(
        || swatch(100.0, 80.0, 0),
        spec(PageTransition::ReducedCrossfade),
    );
    for t in [1000, 1150, 1300] {
        frame(&mut root, &mut app, &mut (), &mut Rec::new(), ms(t));
    }

    controller.pop();
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(2000));
    // Progress 0: the popped page (on top) is recorded at rest.
    let popped = scene
        .pushes()
        .into_iter()
        .find(|p| p.3 == 1.0)
        .unwrap_or_else(|| panic!("the popped page is recorded at rest: {:?}", scene.ops))
        .0;

    root.set_retained_ledger(&ledger(popped, (100, 100), 1.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(2050));
    assert_eq!(scene.draws().len(), 1, "{:?}", scene.ops);
    assert_eq!(scene.draws()[0].0, popped);
}

#[test]
fn a_navigator_scale_step_paints_inline() {
    let (controller, mut root, mut app) = nav();
    controller.push_with(
        || swatch(100.0, 80.0, 0),
        spec(PageTransition::M3FadeThrough),
    );
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(1000));
    let outgoing = scene.pushes()[0].0;
    root.set_retained_ledger(&ledger(outgoing, (100, 100), 1.0));

    // Progress 0.5, past the 0.3 split: the incoming page fades in AND scales
    // up from 0.92 — inline, never through a bracket.
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(1050));
    assert_eq!(scene.retained_ops(), 0, "{:?}", scene.ops);
    let Op::PushTransform(scale) = scene.ops[0] else {
        panic!("a scale transform opens the frame: {:?}", scene.ops);
    };
    let sx = scale.as_coeffs()[0];
    assert!(sx > 0.92 && sx < 1.0, "scaled below 1 (was {sx})");
    assert!(matches!(scene.ops[1], Op::PushLayer(a) if a > 0.0 && a < 1.0));
    assert_eq!(scene.fills(), 1);
}

#[test]
fn the_kill_switch_paints_navigator_pages_with_no_bracket() {
    let (controller, mut root, mut app) = nav();
    root.set_retained_layers(false);
    controller.push_with(
        || swatch(100.0, 80.0, 0),
        spec(PageTransition::ReducedCrossfade),
    );
    for t in [1000, 1050, 1150, 1300] {
        let mut scene = Rec::new();
        frame(&mut root, &mut app, &mut (), &mut scene, ms(t));
        assert_eq!(scene.retained_ops(), 0, "frame at {t}ms: {:?}", scene.ops);
        root.set_retained_ledger(&ledger(0, (100, 100), 1.0));
    }
}
