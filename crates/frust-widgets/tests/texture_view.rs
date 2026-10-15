//! `texture_view`: the fit math that places a scene texture in the widget's
//! box, the clip commands that bound it (rounded corners, a `Cover`
//! overflow), the sizing modes, the rebuild flags and the semantics label —
//! all through a recording scene, no GPU.

use frust_core::accesskit::Role;
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, RenderRoot, View,
    Widget,
};
use frust_widgets::{TextureFit, TextureView, TextureWidget, texture_fit_rect, texture_view};
use kurbo::{Point, Rect, Size};
use peniko::Color;

/// Every paint call a texture view can make, in order.
#[derive(Debug, PartialEq)]
enum Op {
    Clip(Point, Size),
    ClipRounded(Point, Size, f64),
    PopClip,
    SceneTexture(u64, Rect),
}

#[derive(Default)]
struct Recorder(Vec<Op>);

impl PaintScene for Recorder {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn push_clip(&mut self, origin: Point, size: Size) {
        self.0.push(Op::Clip(origin, size));
    }
    fn push_clip_rounded(&mut self, origin: Point, size: Size, radius: f64) {
        self.0.push(Op::ClipRounded(origin, size, radius));
    }
    fn pop_clip(&mut self) {
        self.0.push(Op::PopClip);
    }
    fn draw_scene_texture(&mut self, id: u64, dest: Rect) {
        self.0.push(Op::SceneTexture(id, dest));
    }
}

fn build(view: &TextureView) -> TextureWidget {
    let mut counter = 0u64;
    <TextureView as View<()>>::build(view, &mut BuildCtx::new(&mut counter))
}

/// Lays `view` out under `bc` and paints it at `origin`, returning the laid
/// out size and the recorded commands.
fn paint(view: &TextureView, bc: BoxConstraints, origin: Point) -> (Size, Vec<Op>) {
    let mut widget = build(view);
    let size = widget.layout(&mut LayoutCtx::new(), &bc);
    let mut recorder = Recorder::default();
    widget.paint(&mut PaintCtx::new(origin, size), &mut recorder);
    (size, recorder.0)
}

/// A 200x100 box: wider than tall, so a square texture letterboxes
/// horizontally under `Contain` and overflows vertically under `Cover`.
const BOX: Size = Size::new(200.0, 100.0);

// -- fit math ---------------------------------------------------------------

#[test]
fn fill_stretches_to_exactly_the_box() {
    let rect = texture_fit_rect(Size::new(64.0, 64.0), BOX, TextureFit::Fill);
    assert_eq!(rect, Rect::new(0.0, 0.0, 200.0, 100.0));
}

#[test]
fn contain_scales_to_fit_inside_and_centres_the_letterbox() {
    let rect = texture_fit_rect(Size::new(64.0, 64.0), BOX, TextureFit::Contain);
    assert_eq!(
        rect,
        Rect::new(50.0, 0.0, 150.0, 100.0),
        "a square in a 2:1 box is height-bound and centred horizontally"
    );

    let wide = texture_fit_rect(Size::new(400.0, 100.0), BOX, TextureFit::Contain);
    assert_eq!(
        wide,
        Rect::new(0.0, 25.0, 200.0, 75.0),
        "a 4:1 texture is width-bound and centred vertically"
    );
}

#[test]
fn cover_scales_to_cover_the_box_and_centres_the_overflow() {
    let rect = texture_fit_rect(Size::new(64.0, 64.0), BOX, TextureFit::Cover);
    assert_eq!(
        rect,
        Rect::new(0.0, -50.0, 200.0, 150.0),
        "a square covering a 2:1 box is width-bound and overflows evenly above and below"
    );
}

#[test]
fn aspect_ratio_is_kept_by_contain_and_cover() {
    let natural = Size::new(1920.0, 1080.0);
    for fit in [TextureFit::Contain, TextureFit::Cover] {
        let rect = texture_fit_rect(natural, BOX, fit);
        let ratio = rect.width() / rect.height();
        assert!(
            (ratio - natural.width / natural.height).abs() < 1e-9,
            "{fit:?} keeps the source's aspect ratio, got {ratio}"
        );
    }
}

#[test]
fn a_degenerate_extent_falls_back_to_the_box() {
    for fit in [TextureFit::Fill, TextureFit::Contain, TextureFit::Cover] {
        for natural in [Size::ZERO, Size::new(0.0, 10.0), Size::new(f64::NAN, 10.0)] {
            assert_eq!(
                texture_fit_rect(natural, BOX, fit),
                Rect::new(0.0, 0.0, 200.0, 100.0),
                "{fit:?} with a {natural:?} source has no aspect ratio to keep"
            );
        }
    }
}

// -- paint ------------------------------------------------------------------

#[test]
fn contain_paints_the_texture_at_the_absolute_letterboxed_rect_unclipped() {
    let view = texture_view(7)
        .source_size(Size::new(64.0, 64.0))
        .fit(TextureFit::Contain);
    let (size, ops) = paint(&view, BoxConstraints::tight(BOX), Point::new(10.0, 20.0));
    assert_eq!(size, BOX);
    assert_eq!(
        ops,
        vec![Op::SceneTexture(7, Rect::new(60.0, 20.0, 160.0, 120.0))],
        "the fitted rect is offset by the widget's own origin, and nothing overflows to clip"
    );
}

#[test]
fn cover_clips_its_overflow_to_the_box() {
    let view = texture_view(7)
        .source_size(Size::new(64.0, 64.0))
        .fit(TextureFit::Cover);
    let origin = Point::new(10.0, 20.0);
    let (_, ops) = paint(&view, BoxConstraints::tight(BOX), origin);
    assert_eq!(
        ops,
        vec![
            Op::Clip(origin, BOX),
            Op::SceneTexture(7, Rect::new(10.0, -30.0, 210.0, 170.0)),
            Op::PopClip,
        ]
    );
}

#[test]
fn corner_radius_clips_the_box_to_rounded_corners() {
    let view = texture_view(9)
        .source_size(Size::new(64.0, 64.0))
        .fit(TextureFit::Cover)
        .corner_radius(12.0);
    let origin = Point::new(5.0, 5.0);
    let (_, ops) = paint(&view, BoxConstraints::tight(BOX), origin);
    assert_eq!(
        ops,
        vec![
            Op::ClipRounded(origin, BOX, 12.0),
            Op::SceneTexture(9, Rect::new(5.0, -45.0, 205.0, 155.0)),
            Op::PopClip,
        ],
        "the rounded clip bounds the overflow as well — one clip, not two"
    );

    let fill = texture_view(9).fit(TextureFit::Fill).corner_radius(4.0);
    let (_, ops) = paint(&fill, BoxConstraints::tight(BOX), Point::ZERO);
    assert_eq!(
        ops,
        vec![
            Op::ClipRounded(Point::ZERO, BOX, 4.0),
            Op::SceneTexture(9, Rect::new(0.0, 0.0, 200.0, 100.0)),
            Op::PopClip,
        ]
    );
}

#[test]
fn a_negative_corner_radius_is_square() {
    let view = texture_view(1).corner_radius(-3.0);
    let (_, ops) = paint(&view, BoxConstraints::tight(BOX), Point::ZERO);
    assert_eq!(
        ops,
        vec![Op::SceneTexture(1, Rect::new(0.0, 0.0, 200.0, 100.0))]
    );
}

#[test]
fn without_a_source_size_every_fit_fills_the_box() {
    for fit in [TextureFit::Contain, TextureFit::Cover, TextureFit::Fill] {
        let (_, ops) = paint(
            &texture_view(3).fit(fit),
            BoxConstraints::tight(BOX),
            Point::ZERO,
        );
        assert_eq!(
            ops,
            vec![Op::SceneTexture(3, Rect::new(0.0, 0.0, 200.0, 100.0))],
            "{fit:?} with no source extent"
        );
    }
}

// -- layout -----------------------------------------------------------------

#[test]
fn expand_is_the_default_and_fills_the_constraints() {
    let (size, _) = paint(
        &texture_view(1),
        BoxConstraints::loose(Size::new(300.0, 200.0)),
        Point::ZERO,
    );
    assert_eq!(size, Size::new(300.0, 200.0));
}

#[test]
fn an_explicit_size_is_clamped_into_the_constraints() {
    let loose = BoxConstraints::loose(Size::new(300.0, 200.0));
    let (size, _) = paint(
        &texture_view(1).size(Size::new(120.0, 80.0)),
        loose,
        Point::ZERO,
    );
    assert_eq!(size, Size::new(120.0, 80.0));

    let (size, _) = paint(
        &texture_view(1).size(Size::new(1000.0, 80.0)),
        loose,
        Point::ZERO,
    );
    assert_eq!(size, Size::new(300.0, 80.0));

    let (size, _) = paint(
        &texture_view(1).size(Size::new(10.0, 10.0)).expand(),
        loose,
        Point::ZERO,
    );
    assert_eq!(size, Size::new(300.0, 200.0), "the last sizing call wins");
}

// -- rebuild ----------------------------------------------------------------

fn rebuild(prev: &TextureView, next: &TextureView) -> (ChangeFlags, TextureWidget) {
    let mut widget = build(prev);
    let mut counter = 0u64;
    let flags = <TextureView as View<()>>::rebuild(
        next,
        prev,
        &mut widget,
        &mut BuildCtx::new(&mut counter),
    );
    (flags, widget)
}

#[test]
fn an_unchanged_rebuild_requests_nothing() {
    let view = texture_view(4).fit(TextureFit::Cover).corner_radius(2.0);
    let (flags, _) = rebuild(&view, &view.clone());
    assert_eq!(flags, ChangeFlags::NONE);
}

#[test]
fn a_new_id_or_fit_repaints_and_a_new_size_relayouts() {
    let (flags, widget) = rebuild(&texture_view(4), &texture_view(5));
    assert!(flags.needs_paint());
    assert!(!flags.contains(ChangeFlags::LAYOUT));
    assert_eq!(widget.scene_texture_id(), 5, "the widget adopts the new id");

    let (flags, _) = rebuild(&texture_view(4), &texture_view(4).fit(TextureFit::Fill));
    assert!(flags.needs_paint());

    let (flags, _) = rebuild(
        &texture_view(4),
        &texture_view(4).size(Size::new(10.0, 10.0)),
    );
    assert!(flags.contains(ChangeFlags::LAYOUT));
}

// -- semantics --------------------------------------------------------------

fn semantics_of(view: TextureView) -> frust_core::SemanticsUpdate {
    let mut root: RenderRoot<(), TextureView> = RenderRoot::new();
    let mut state = ();
    root.rebuild(&mut |_: &mut ()| view.clone(), &mut state);
    root.layout(Size::new(320.0, 240.0));
    root.semantics()
}

#[test]
fn a_labelled_texture_view_is_an_image_node() {
    let update = semantics_of(texture_view(1).label("Live camera"));
    let image = update
        .nodes
        .iter()
        .map(|(_, node)| node)
        .find(|node| node.role() == Role::Image)
        .expect("the labelled view contributes an image node");
    assert_eq!(image.label(), Some("Live camera"));
}

#[test]
fn an_unlabelled_texture_view_is_decorative() {
    let update = semantics_of(texture_view(1));
    assert!(
        update
            .nodes
            .iter()
            .all(|(_, node)| node.role() != Role::Image),
        "no label, no node"
    );
}
