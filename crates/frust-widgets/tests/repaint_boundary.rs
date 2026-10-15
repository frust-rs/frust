//! Retained-page boundaries at the display-list level: `repaint_boundary`,
//! `AnimatedOpacity::cached(true)` and the navigator's pages, driven through a
//! `RenderRoot` with a hand-set resident-key ledger (the renderer's role) and
//! painted into a recorder that keeps every command in order.

use std::time::Duration;

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, EventCtx, EventResult, FrameTime,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase,
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

/// A ledger holding `key` resident at `extent_px` and `scale`, published once
/// the renderer committed the scene painted at serial `committed` (the
/// `PaintOutcome::paint_serial` a frame returned).
fn ledger(committed: u64, key: u64, extent_px: (u32, u32), scale: f32) -> RetainedLedger {
    RetainedLedger::from_entries(committed, [(key, extent_px, scale)])
}

/// Rebuild, lay out and paint one frame into `scene`, returning the paint
/// serial the root minted for it.
fn frame<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    app: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    scene: &mut Rec,
    time: FrameTime,
) -> u64 {
    root.rebuild(app, state);
    root.layout(WINDOW);
    root.paint(scene, time).paint_serial
}

// ---------------------------------------------------------------------------
// repaint_boundary.
// ---------------------------------------------------------------------------

/// The boundary over a 40x30 swatch: record on the first frame, return its key
/// and the serial of the frame that recorded it.
fn recorded_boundary<V: View<u32>>(
    root: &mut RenderRoot<u32, V>,
    app: &mut impl FnMut(&mut u32) -> V,
    state: &mut u32,
) -> (u64, u64) {
    let mut scene = Rec::new();
    let serial = frame(root, app, state, &mut scene, ms(0));
    let pushes = scene.pushes();
    assert_eq!(pushes.len(), 1, "frame 1 records the body: {:?}", scene.ops);
    (pushes[0].0, serial)
}

#[test]
fn frame_one_records_and_an_unchanged_frame_two_draws_the_page() {
    let mut root: RenderRoot<u32, _> = RenderRoot::new();
    let mut app = |s: &mut u32| repaint_boundary(swatch(40.0, 30.0, *s));
    let mut state = 0u32;

    let mut scene = Rec::new();
    let recorded = frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
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

    root.set_retained_ledger(&ledger(recorded, key, (40, 30), 1.0));
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
    let (key, recorded) = recorded_boundary(&mut root, &mut app, &mut state);
    root.set_retained_ledger(&ledger(recorded, key, (40, 30), 1.0));

    state = 1;
    let mut scene = Rec::new();
    let re_recorded = frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
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

    // Unchanged again: once the renderer committed the re-recording, the page
    // is drawn.
    root.set_retained_ledger(&ledger(re_recorded, key, (40, 30), 1.0));
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
    let (key, mut committed) = recorded_boundary(&mut root, &mut app, &mut state);

    for (extent, scale) in [((41, 30), 1.0), ((40, 31), 1.0), ((40, 30), 2.0)] {
        root.set_retained_ledger(&ledger(committed, key, extent, scale));
        let mut scene = Rec::new();
        committed = frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
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
    let recorded = frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
    let key = scene.pushes()[0].0;
    assert_eq!(
        scene.pushes()[0].4,
        1.0,
        "the bracket's own scale is always 1"
    );

    // A 1x page is not the 2x page this scene draws at.
    root.set_retained_ledger(&ledger(recorded, key, (40, 30), 1.0));
    let mut scene = Rec::at_scale(2.0);
    let re_recorded = frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
    assert!(scene.draws().is_empty(), "{:?}", scene.ops);

    root.set_retained_ledger(&ledger(re_recorded, key, (80, 60), 2.0));
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
    let recorded = frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
    let key = scene.pushes()[0].0;

    // The ledger (one frame stale) still says 2x, but this frame draws at 3x.
    root.set_retained_ledger(&ledger(recorded, key, (80, 60), 2.0));
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
    let recorded = frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
    let (key, _, _, alpha, _) = scene.pushes()[0];
    assert_eq!(alpha, 0.5, "the bracket carries the composite alpha");
    assert!(
        !scene.ops.iter().any(|op| matches!(op, Op::PushLayer(_))),
        "and no layer of its own"
    );

    root.set_retained_ledger(&ledger(recorded, key, (40, 30), 1.0));
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
        root.set_retained_ledger(&ledger(u64::MAX, 0, (40, 30), 1.0));
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
    let recorded = frame(&mut root, &mut app, &mut (), &mut scene, ms(0));
    let key = scene.pushes()[0].0;

    root.set_retained_ledger(&ledger(recorded, key, (40, 30), 1.0));
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
    let recorded = frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
    let pushes = scene.pushes();
    assert_eq!(
        pushes.len(),
        1,
        "the child is recorded at rest: {:?}",
        scene.ops
    );
    let key = pushes[0].0;
    // Published once, after the recording scene was committed: the first draw
    // confirms the page, so the later frames draw against the same ledger.
    root.set_retained_ledger(&ledger(recorded, key, (40, 30), 1.0));

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
    let recorded = frame(&mut root, &mut app, &mut (), &mut scene, ms(1000));
    let pushes = scene.pushes();
    assert_eq!(pushes.len(), 1, "{:?}", scene.ops);
    let (outgoing, origin, size, alpha, scale) = pushes[0];
    assert_eq!(
        (origin, size, alpha, scale),
        (Point::ZERO, WINDOW, 1.0, 1.0)
    );

    root.set_retained_ledger(&ledger(recorded, outgoing, (100, 100), 1.0));
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
    let recorded = frame(&mut root, &mut app, &mut (), &mut scene, ms(1000));
    let outgoing = scene.pushes()[0].0;

    // IosPush, progress 0.5: the page below parallaxes left by a third of the
    // width times the progress and dims; the incoming page overhangs the
    // navigator and paints directly.
    root.set_retained_ledger(&ledger(recorded, outgoing, (100, 100), 1.0));
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
    let recorded = frame(&mut root, &mut app, &mut (), &mut scene, ms(2000));
    // Progress 0: the popped page (on top) is recorded at rest.
    let popped = scene
        .pushes()
        .into_iter()
        .find(|p| p.3 == 1.0)
        .unwrap_or_else(|| panic!("the popped page is recorded at rest: {:?}", scene.ops))
        .0;

    root.set_retained_ledger(&ledger(recorded, popped, (100, 100), 1.0));
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
    let recorded = frame(&mut root, &mut app, &mut (), &mut scene, ms(1000));
    let outgoing = scene.pushes()[0].0;
    root.set_retained_ledger(&ledger(recorded, outgoing, (100, 100), 1.0));

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
        root.set_retained_ledger(&ledger(u64::MAX, 0, (100, 100), 1.0));
    }
}

// ---------------------------------------------------------------------------
// Which recording a resident page holds: the paint serial.
// ---------------------------------------------------------------------------

/// The body change whose recording never reached the renderer: the scene that
/// recorded it was dropped (the split's latest-wins slot), so the page the
/// renderer still holds under the key is the previous body's. A ledger
/// published before that recording was committed must not let the boundary
/// draw; the one published after a later recording was committed must.
#[test]
fn a_recording_whose_scene_was_dropped_is_recorded_again_not_drawn() {
    let mut root: RenderRoot<u32, _> = RenderRoot::new();
    let mut app = |s: &mut u32| repaint_boundary(swatch(40.0, 30.0, *s));
    let mut state = 0u32;
    let (key, first) = recorded_boundary(&mut root, &mut app, &mut state);
    root.set_retained_ledger(&ledger(first, key, (40, 30), 1.0));
    let mut scene = Rec::new();
    let drawn = frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
    assert_eq!(scene.draws().len(), 1, "settled: {:?}", scene.ops);

    // Paint N: the body changed, so it is recorded again (v2). The renderer
    // never sees this scene.
    state = 1;
    let mut scene = Rec::new();
    let n = frame(&mut root, &mut app, &mut state, &mut scene, ms(32));
    assert_eq!(scene.pushes().len(), 1, "{:?}", scene.ops);

    // Paint N+1, the body unchanged since N: the latest ledger was published
    // after scene N-1 and still lists the key (holding v1). It does not cover
    // N, so the boundary records again instead of drawing v1.
    assert_eq!(drawn, n - 1);
    root.set_retained_ledger(&ledger(n - 1, key, (40, 30), 1.0));
    let mut scene = Rec::new();
    let n1 = frame(&mut root, &mut app, &mut state, &mut scene, ms(48));
    assert!(
        scene.draws().is_empty(),
        "the resident page predates the recording: {:?}",
        scene.ops
    );
    assert_eq!(scene.pushes().len(), 1, "recorded again: {:?}", scene.ops);
    assert_eq!(scene.pushes()[0].0, key);
    assert_eq!(scene.fills(), 1);

    // The renderer committed scene N+1 (which carries v2): its ledger lets the
    // next paint draw the page.
    root.set_retained_ledger(&ledger(n1, key, (40, 30), 1.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(64));
    assert_eq!(
        scene.draws(),
        vec![(key, Point::ZERO, Size::new(40.0, 30.0), 1.0, 1.0)],
        "{:?}",
        scene.ops
    );
    assert_eq!(scene.fills(), 0);
}

/// A renderer that lags the UI thread by a frame never confirms the latest
/// recording before the next paint; a clean body recorded on consecutive
/// paints is one run, so the late ledger still lands inside it and the
/// boundary draws — it does not record forever behind the renderer.
#[test]
fn a_ledger_one_frame_late_still_confirms_the_run_of_recordings() {
    let mut root: RenderRoot<u32, _> = RenderRoot::new();
    let mut app = |s: &mut u32| repaint_boundary(swatch(40.0, 30.0, *s));
    let mut state = 0u32;
    let (key, recorded) = recorded_boundary(&mut root, &mut app, &mut state);

    // Paint 2: the renderer has not committed scene 1 yet.
    root.set_retained_ledger(&RetainedLedger::from_entries(recorded - 1, []));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
    assert_eq!(scene.pushes().len(), 1, "{:?}", scene.ops);

    // Paint 3: scene 1 is committed (scene 2 is still in flight) — it carried
    // the same body, so the page is good.
    root.set_retained_ledger(&ledger(recorded, key, (40, 30), 1.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(32));
    assert_eq!(scene.draws().len(), 1, "{:?}", scene.ops);
    assert_eq!(scene.fills(), 0);

    // Confirmed: later frames draw against the same (now older) ledger.
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(48));
    assert_eq!(scene.draws().len(), 1, "{:?}", scene.ops);
}

/// Wraps its child and moves it at paint time: the child's origin is `y` below
/// the wrapper's, set in `paint` the way a navigator slides a clean page under
/// an edge swipe. Moving the child dirties nothing under it.
struct Moved<V> {
    y: f64,
    child: V,
}

struct MovedWidget {
    y: f64,
    child: ChildPod,
}

impl<S: 'static, V: View<S>> View<S> for Moved<V> {
    type Element = MovedWidget;
    fn build(&self, ctx: &mut BuildCtx<'_>) -> MovedWidget {
        MovedWidget {
            y: self.y,
            child: frust_widgets::authoring::build_child(&self.child, ctx),
        }
    }
    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MovedWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = frust_widgets::authoring::rebuild_child(
            &prev.child,
            &self.child,
            &mut element.child,
            ctx,
        );
        if prev.y != self.y {
            element.y = self.y;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for MovedWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        size
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.set_origin(Point::new(0.0, self.y));
        self.child.paint_child(ctx, scene);
    }
    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        visitor(&self.child);
    }
}

/// A body that paints at a new origin between two recordings of one run does
/// not continue that run: the page recorded at the first origin is never
/// confirmed by a ledger that only covers the second run's start, and a page
/// confirmed at its recorded origin is drawn through a translation to where
/// the body now sits.
#[test]
fn a_moved_origin_starts_a_new_run() {
    let mut root: RenderRoot<f64, _> = RenderRoot::new();
    let mut app = |s: &mut f64| Moved {
        y: *s,
        child: repaint_boundary(swatch(40.0, 30.0, 0)),
    };
    let mut state = 0.0f64;
    let size = Size::new(40.0, 30.0);

    // Paint 1 records the body at origin A = (0, 0).
    let mut scene = Rec::new();
    let first = frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
    let key = scene.pushes()[0].0;
    assert_eq!(scene.pushes()[0].1, Point::ZERO, "{:?}", scene.ops);

    // Paint 2: the renderer has not committed scene 1 yet, and the body now
    // paints at B = (0, 20) while clean. The moved body records a new run.
    root.set_retained_ledger(&RetainedLedger::from_entries(first - 1, []));
    state = 20.0;
    let mut scene = Rec::new();
    let second = frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
    assert_eq!(
        scene.pushes().len(),
        1,
        "moved body records: {:?}",
        scene.ops
    );
    assert_eq!(
        scene.pushes()[0].1,
        Point::new(0.0, 20.0),
        "{:?}",
        scene.ops
    );
    assert!(scene.draws().is_empty(), "{:?}", scene.ops);

    // Paint 3: scene 1 (the page at A) is committed. Its ledger covers paint 1,
    // not the run that started at paint 2, so it must not draw the page at B.
    root.set_retained_ledger(&ledger(first, key, (40, 30), 1.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(32));
    assert!(
        scene.draws().is_empty(),
        "scene 1's page sits at A, not at B: {:?}",
        scene.ops
    );
    assert_eq!(scene.pushes().len(), 1, "recorded again: {:?}", scene.ops);
    assert_eq!(
        scene.pushes()[0].1,
        Point::new(0.0, 20.0),
        "{:?}",
        scene.ops
    );

    // Paint 4: scene 2 (recorded at B) is committed. Its ledger covers the
    // run's start, so the page is drawn at the origin it was recorded at.
    root.set_retained_ledger(&ledger(second, key, (40, 30), 1.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(48));
    assert_eq!(
        scene.draws(),
        vec![(key, Point::new(0.0, 20.0), size, 1.0, 1.0)],
        "{:?}",
        scene.ops
    );
    assert_eq!(scene.fills(), 0, "{:?}", scene.ops);

    // Paint 5: the confirmed page is moved again, to C = (0, 50), while clean.
    // It still draws at its recorded origin, translated by the move.
    state = 50.0;
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(64));
    assert_eq!(
        scene.draws(),
        vec![(key, Point::new(0.0, 20.0), size, 1.0, 1.0)],
        "{:?}",
        scene.ops
    );
    assert!(
        scene
            .ops
            .contains(&Op::PushTransform(Affine::translate((0.0, 30.0)))),
        "the move is a translation from B to C: {:?}",
        scene.ops
    );
    assert_eq!(scene.fills(), 0, "{:?}", scene.ops);
}

/// Wraps its child, painting it only while `shown` — a frame in which the
/// boundary is not painted at all commits a scene with no recording in it.
struct Shown<V> {
    shown: bool,
    child: V,
}

struct ShownWidget {
    shown: bool,
    child: ChildPod,
}

impl<S: 'static, V: View<S>> View<S> for Shown<V> {
    type Element = ShownWidget;
    fn build(&self, ctx: &mut BuildCtx<'_>) -> ShownWidget {
        ShownWidget {
            shown: self.shown,
            child: frust_widgets::authoring::build_child(&self.child, ctx),
        }
    }
    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ShownWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = frust_widgets::authoring::rebuild_child(
            &prev.child,
            &self.child,
            &mut element.child,
            ctx,
        );
        if prev.shown != self.shown {
            element.shown = self.shown;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for ShownWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        size
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.shown {
            self.child.paint_child(ctx, scene);
        }
    }
    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        visitor(&self.child);
    }
}

/// A recording followed by a frame that did not paint the boundary at all: a
/// ledger describing that later frame proves nothing about the recording (its
/// scene may have been the dropped one), so the boundary records again.
#[test]
fn a_ledger_from_after_an_unrecorded_frame_does_not_vouch_for_an_unconfirmed_page() {
    let mut root: RenderRoot<(u32, bool), _> = RenderRoot::new();
    let mut app = |s: &mut (u32, bool)| Shown {
        shown: s.1,
        child: repaint_boundary(swatch(40.0, 30.0, s.0)),
    };
    let mut state = (0u32, true);
    let mut scene = Rec::new();
    let first = frame(&mut root, &mut app, &mut state, &mut scene, ms(0));
    let key = scene.pushes()[0].0;
    root.set_retained_ledger(&ledger(first, key, (40, 30), 1.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(16));
    assert_eq!(scene.draws().len(), 1, "confirmed v1: {:?}", scene.ops);

    // v2 recorded (scene dropped), then a frame that hides the boundary.
    state.0 = 1;
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(32));
    assert_eq!(scene.pushes().len(), 1, "{:?}", scene.ops);
    state.1 = false;
    let mut scene = Rec::new();
    let hidden = frame(&mut root, &mut app, &mut state, &mut scene, ms(48));
    assert_eq!(scene.retained_ops(), 0, "{:?}", scene.ops);

    // The renderer committed the hidden frame; v1 is still resident.
    root.set_retained_ledger(&ledger(hidden, key, (40, 30), 1.0));
    state.1 = true;
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut state, &mut scene, ms(64));
    assert!(scene.draws().is_empty(), "v1 is not drawn: {:?}", scene.ops);
    assert_eq!(scene.pushes().len(), 1, "v2 is recorded again");
}

// ---------------------------------------------------------------------------
// Hover and focus links moving off a boundary's body.
// ---------------------------------------------------------------------------

/// A 40x30 leaf that claims hover on a `Move` over it and takes focus on a
/// `Down` over it, asking for a repaint when either changes (the consumer's
/// change-gated redraw), and paints a marker while hovered or focused.
struct Linked;

struct LinkedWidget {
    hovered: bool,
}

impl<S: 'static> View<S> for Linked {
    type Element = LinkedWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LinkedWidget {
        LinkedWidget { hovered: false }
    }
    fn rebuild(&self, _: &Self, _: &mut LinkedWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for LinkedWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(40.0, 30.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
        if ctx.is_hovered() || ctx.has_focus() {
            scene.fill_rect(ctx.origin(), Size::new(1.0, 1.0), peniko::Color::WHITE);
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Move => {
                ctx.claim_hover();
                if !self.hovered {
                    self.hovered = true;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                ctx.request_focus();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
}

/// Two children stacked vertically, routed by hit test.
struct Pair<A, B>(A, B);

struct PairWidget {
    children: [ChildPod; 2],
}

impl<S: 'static, A: View<S>, B: View<S>> View<S> for Pair<A, B> {
    type Element = PairWidget;
    fn build(&self, ctx: &mut BuildCtx<'_>) -> PairWidget {
        PairWidget {
            children: [
                frust_widgets::authoring::build_child(&self.0, ctx),
                frust_widgets::authoring::build_child(&self.1, ctx),
            ],
        }
    }
    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PairWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let [a, b] = &mut element.children;
        frust_widgets::authoring::rebuild_child(&prev.0, &self.0, a, ctx)
            | frust_widgets::authoring::rebuild_child(&prev.1, &self.1, b, ctx)
    }
}

impl Widget for PairWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let mut y = 0.0;
        for pod in &mut self.children {
            let size = pod.layout_child(ctx, &BoxConstraints::loose(bc.max()));
            pod.set_origin(Point::new(0.0, y));
            y += size.height;
        }
        bc.constrain(Size::new(40.0, y))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust_widgets::authoring::route_event(&mut self.children, ctx, event)
    }
    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        for pod in &self.children {
            visitor(pod);
        }
    }
}

fn pointer(phase: PointerPhase, y: f64) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase,
        position: Point::new(5.0, y),
        button: PointerButton::Primary,
    })
}

/// A boundary over the top leaf, a plain leaf below it: record, then settle
/// on the page. Returns the boundary's key and the latest paint serial.
fn settled_pair<V: View<()>>(
    root: &mut RenderRoot<(), V>,
    app: &mut impl FnMut(&mut ()) -> V,
) -> (u64, u64) {
    let mut scene = Rec::new();
    let recorded = frame(root, app, &mut (), &mut scene, ms(0));
    let key = scene.pushes()[0].0;
    root.set_retained_ledger(&ledger(recorded, key, (40, 30), 1.0));
    let mut scene = Rec::new();
    let serial = frame(root, app, &mut (), &mut scene, ms(16));
    assert_eq!(scene.draws().len(), 1, "settled: {:?}", scene.ops);
    (key, serial)
}

#[test]
fn a_hover_leaving_a_boundarys_body_for_a_target_outside_it_re_records_the_page() {
    let mut root: RenderRoot<(), _> = RenderRoot::new();
    let mut app = |_: &mut ()| Pair(repaint_boundary(Linked), Linked);
    let (key, _) = settled_pair(&mut root, &mut app);

    // Hover the body inside the boundary: it repaints itself, highlighted.
    root.event(&mut (), &pointer(PointerPhase::Move, 5.0));
    let mut scene = Rec::new();
    let highlighted = frame(&mut root, &mut app, &mut (), &mut scene, ms(32));
    assert_eq!(scene.pushes().len(), 1, "{:?}", scene.ops);
    root.set_retained_ledger(&ledger(highlighted, key, (40, 30), 1.0));

    // A move within the same claimant changes no link: the page is drawn.
    root.event(&mut (), &pointer(PointerPhase::Move, 6.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(48));
    assert_eq!(scene.draws().len(), 1, "same claimant: {:?}", scene.ops);
    assert_eq!(scene.pushes().len(), 0);

    // The pointer moves to the leaf below, outside the boundary. The Move
    // never reaches the boundary's body, which is clean, and the ledger still
    // holds its highlighted page — which must not be drawn now.
    root.event(&mut (), &pointer(PointerPhase::Move, 35.0));
    let mut scene = Rec::new();
    let unhighlighted = frame(&mut root, &mut app, &mut (), &mut scene, ms(64));
    assert!(scene.draws().is_empty(), "stale highlight: {:?}", scene.ops);
    assert_eq!(scene.pushes().len(), 1, "re-recorded: {:?}", scene.ops);
    assert_eq!(scene.pushes()[0].0, key);

    // Moving within the new claimant moves no link: the fresh page is drawn.
    root.set_retained_ledger(&ledger(unhighlighted, key, (40, 30), 1.0));
    root.event(&mut (), &pointer(PointerPhase::Move, 36.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(80));
    assert_eq!(scene.draws().len(), 1, "{:?}", scene.ops);
}

#[test]
fn focus_leaving_a_boundarys_body_for_a_target_outside_it_re_records_the_page() {
    let mut root: RenderRoot<(), _> = RenderRoot::new();
    let mut app = |_: &mut ()| Pair(repaint_boundary(Linked), Linked);
    let (key, _) = settled_pair(&mut root, &mut app);

    // Focus the body inside the boundary (a press ends hover; none stood).
    root.event(&mut (), &pointer(PointerPhase::Down, 5.0));
    root.event(&mut (), &pointer(PointerPhase::Up, 5.0));
    let mut scene = Rec::new();
    let focused = frame(&mut root, &mut app, &mut (), &mut scene, ms(32));
    assert_eq!(scene.pushes().len(), 1, "{:?}", scene.ops);
    root.set_retained_ledger(&ledger(focused, key, (40, 30), 1.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(48));
    assert_eq!(scene.draws().len(), 1, "settled focused: {:?}", scene.ops);

    // Focus moves to the leaf below; the press never reaches the boundary.
    root.event(&mut (), &pointer(PointerPhase::Down, 35.0));
    root.event(&mut (), &pointer(PointerPhase::Up, 35.0));
    let mut scene = Rec::new();
    frame(&mut root, &mut app, &mut (), &mut scene, ms(64));
    assert!(
        scene.draws().is_empty(),
        "stale focus ring: {:?}",
        scene.ops
    );
    assert_eq!(scene.pushes().len(), 1, "re-recorded: {:?}", scene.ops);
}
