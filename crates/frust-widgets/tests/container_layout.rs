//! Integration tests for `container`/`colored_box`
//! (`crates/frust-widgets/src/container.rs`): child-sizing passthrough (the
//! wrapper hugs its child; a childless `.expand()`/`.size(...)` box;
//! `.size_centered(...)`'s fixed-size-with-centered-child case), paint
//! discipline (glow then fill then border, the border's half-width inset
//! generalized per corner, per-corner radius forwarding, dashed vs. solid
//! border style), the "no decoration = no paint commands" contract, and
//! semantics passthrough (purely decorative — the container itself
//! contributes no accessibility node). Also carries a lighter integration
//! pass for `divider` (`crates/frust-widgets/src/divider.rs`) alongside its
//! own in-module unit tests.

use std::any::Any;

use frust_core::accesskit::Role;
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, CornerRadii, DashPattern, LayoutCtx, PaintCtx,
    PaintScene, RenderRoot, View, Widget,
};
use frust_widgets::{BorderStyle, colored_box, container, divider, text};
use kurbo::{BezPath, Point, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

// ---------------------------------------------------------------------------
// Local fixture: a leaf of fixed intrinsic size (mirrors
// `frust_widgets::test_support::leaf`, unavailable here without the crate's
// non-default `test-support` feature — the same local-fixture approach
// `tests/visit_children.rs`/`tests/semantics_tree.rs` already use).
// ---------------------------------------------------------------------------

struct Leaf {
    intrinsic: Size,
}

fn leaf(width: f64, height: f64) -> Leaf {
    Leaf {
        intrinsic: Size::new(width, height),
    }
}

struct LeafWidget {
    intrinsic: Size,
}

impl View<()> for Leaf {
    type Element = LeafWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LeafWidget {
        LeafWidget {
            intrinsic: self.intrinsic,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut LeafWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.intrinsic = self.intrinsic;
        ChangeFlags::LAYOUT
    }
}

impl Widget for LeafWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.intrinsic)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
    }
}

fn build<S: 'static, V: View<S>>(view: &V) -> V::Element {
    let mut counter = 0u64;
    view.build(&mut BuildCtx::new(&mut counter))
}

// ---------------------------------------------------------------------------
// Paint recorder: glow/fill/border/plain-fill_rect calls in emission order
// (`calls`), plus the border's exact stroke path (`border_paths`) so a
// per-corner border geometry test can compare it against an independently
// reconstructed `kurbo::RoundedRect` path, and a plain `fill_rect` log
// (`rects`) `divider`/a child leaf paint into.
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum PaintCall {
    /// A [`frust_core::PaintScene::fill_rounded_rect_radii`] call — the only
    /// fill path the widget uses now; a uniform `.radius(f64)` arrives here
    /// as `CornerRadii::uniform`.
    Fill { radii: CornerRadii, color: Color },
    /// A solid border stroke ([`BorderStyle::Solid`], the default).
    Border { width: f64, color: Color },
    /// A dashed border stroke ([`BorderStyle::Dashed`]).
    DashedBorder {
        width: f64,
        color: Color,
        dash: DashPattern,
    },
    /// A `.glow(...)` ambient shadow.
    Shadow {
        origin: Point,
        size: Size,
        radius: f64,
        std_dev: f64,
        color: Color,
    },
}

#[derive(Default)]
struct Recorder {
    calls: Vec<PaintCall>,
    border_paths: Vec<BezPath>,
    rects: Vec<(Point, Size)>,
}

impl PaintScene for Recorder {
    fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
        self.rects.push((origin, size));
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}

    fn fill_rounded_rect_radii(
        &mut self,
        _origin: Point,
        _size: Size,
        radii: CornerRadii,
        color: Color,
    ) {
        self.calls.push(PaintCall::Fill { radii, color });
    }

    fn stroke_path(&mut self, _origin: Point, path: &BezPath, width: f64, brush: &Brush) {
        if let Brush::Solid(color) = brush {
            self.calls.push(PaintCall::Border {
                width,
                color: *color,
            });
            self.border_paths.push(path.clone());
        }
    }

    fn stroke_path_dashed(
        &mut self,
        _origin: Point,
        path: &BezPath,
        width: f64,
        dash: DashPattern,
        brush: &Brush,
    ) {
        if let Brush::Solid(color) = brush {
            self.calls.push(PaintCall::DashedBorder {
                width,
                color: *color,
                dash,
            });
            self.border_paths.push(path.clone());
        }
    }

    fn draw_shadow(&mut self, origin: Point, size: Size, radius: f64, std_dev: f64, color: Color) {
        self.calls.push(PaintCall::Shadow {
            origin,
            size,
            radius,
            std_dev,
            color,
        });
    }
}

fn paint_at(w: &mut impl Widget, size: Size) -> Recorder {
    let mut rec = Recorder::default();
    let mut ctx = PaintCtx::new(Point::ZERO, size);
    w.paint(&mut ctx, &mut rec);
    rec
}

// ---------------------------------------------------------------------------
// Child sizing passthrough
// ---------------------------------------------------------------------------

#[test]
fn wrapper_hugs_the_child_exactly() {
    let view: frust_widgets::ContainerView<()> = container(leaf(40.0, 20.0)).fill(Color::WHITE);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    assert_eq!(
        size,
        Size::new(40.0, 20.0),
        "decoration (fill/border/radius) never grows the box beyond the child's own size"
    );
}

#[test]
fn childless_default_collapses_to_the_incoming_minimum() {
    // No `.expand()`/`.size(...)`: the `Sizing::Hug` default, mirroring
    // `SizedBox`'s childless-spacer precedent.
    let view: frust_widgets::ContainerView<()> = colored_box();
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    let bc = BoxConstraints::new(Size::new(10.0, 15.0), Size::new(500.0, 500.0));
    let size = w.layout(&mut lctx, &bc);
    assert_eq!(size, Size::new(10.0, 15.0));
}

#[test]
fn childless_expand_fills_a_loose_constraints_max() {
    // The `AppBackground` full-bleed case: a `Stack` hands a loose (min-zero)
    // constraint, and `.expand()` must still claim the whole viewport.
    let view: frust_widgets::ContainerView<()> = colored_box().fill(Color::WHITE).expand();
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(320.0, 640.0)));
    assert_eq!(size, Size::new(320.0, 640.0));
}

#[test]
fn childless_expand_fills_a_tight_constraint_too() {
    let view: frust_widgets::ContainerView<()> = colored_box().expand();
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    let size = w.layout(&mut lctx, &BoxConstraints::tight(Size::new(200.0, 100.0)));
    assert_eq!(size, Size::new(200.0, 100.0));
}

#[test]
fn childless_size_forces_a_fixed_extent() {
    // The `FillBox`/swatch case: an explicit fixed size, clamped into
    // whatever constraint is loose enough to allow it.
    let view: frust_widgets::ContainerView<()> = colored_box().fill(Color::WHITE).size(48.0, 24.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    assert_eq!(size, Size::new(48.0, 24.0));
}

#[test]
fn childless_size_clamps_into_a_tighter_incoming_max() {
    let view: frust_widgets::ContainerView<()> = colored_box().size(48.0, 24.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(30.0, 30.0)));
    assert_eq!(
        size,
        Size::new(30.0, 24.0),
        "a fixed size larger than the incoming max still clamps, same as SizedBox"
    );
}

// ---------------------------------------------------------------------------
// Paint discipline
// ---------------------------------------------------------------------------

#[test]
fn no_decoration_paints_nothing_of_its_own() {
    let view: frust_widgets::ContainerView<()> = container(leaf(40.0, 20.0));
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, Size::new(40.0, 20.0));
    assert!(
        rec.calls.is_empty(),
        "neither .fill nor .border were set — the container paints no commands of its own"
    );
}

#[test]
fn fill_only_paints_exactly_one_fill_command() {
    let view: frust_widgets::ContainerView<()> = colored_box()
        .fill(Color::from_rgb8(0x11, 0x22, 0x33))
        .size(10.0, 10.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, Size::new(10.0, 10.0));
    assert_eq!(
        rec.calls,
        vec![PaintCall::Fill {
            radii: CornerRadii::default(),
            color: Color::from_rgb8(0x11, 0x22, 0x33)
        }]
    );
}

#[test]
fn fill_then_border_paint_in_that_order_with_radius_forwarded() {
    let view: frust_widgets::ContainerView<()> = colored_box()
        .fill(Color::from_rgb8(0xAA, 0xBB, 0xCC))
        .radius(6.0)
        .border(Color::from_rgb8(0x00, 0x00, 0x00), 2.0)
        .size(50.0, 50.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, Size::new(50.0, 50.0));
    assert_eq!(
        rec.calls,
        vec![
            PaintCall::Fill {
                radii: CornerRadii::uniform(6.0),
                color: Color::from_rgb8(0xAA, 0xBB, 0xCC)
            },
            PaintCall::Border {
                width: 2.0,
                color: Color::from_rgb8(0x00, 0x00, 0x00)
            },
        ],
        "fill paints before the border, and radius forwards to the fill call"
    );
}

#[test]
fn border_only_paints_without_a_fill_command() {
    let view: frust_widgets::ContainerView<()> = colored_box()
        .border(Color::from_rgb8(0x99, 0x00, 0x00), 1.0)
        .size(20.0, 20.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, Size::new(20.0, 20.0));
    assert_eq!(
        rec.calls,
        vec![PaintCall::Border {
            width: 1.0,
            color: Color::from_rgb8(0x99, 0x00, 0x00)
        }]
    );
}

/// Recreates the exact geometry `Widget::paint` builds for the border's
/// rounded-rect stroke path, so the half-width inset can be asserted against
/// the resulting `kurbo::Rect` bounds directly (mirrors `Button`'s/
/// `material::card`'s outlined-variant test shape).
#[test]
fn border_strokes_fully_inside_the_container_bounds_inset_by_half_width() {
    let width = 4.0;
    let radius = 10.0;
    let size = Size::new(60.0, 40.0);
    let half = width / 2.0;
    let rr = RoundedRect::new(
        half,
        half,
        size.width - half,
        size.height - half,
        (radius - half).max(0.0),
    );
    let bounds = rr.rect();
    assert_eq!(bounds.x0, half);
    assert_eq!(bounds.y0, half);
    assert_eq!(bounds.x1, size.width - half);
    assert_eq!(bounds.y1, size.height - half);

    // Same geometry, exercised through the real widget: a border wider than
    // twice the radius clamps the inset corner radius at zero rather than
    // going negative.
    let view: frust_widgets::ContainerView<()> = colored_box()
        .fill(Color::WHITE)
        .radius(2.0)
        .border(Color::BLACK, 10.0)
        .size(60.0, 40.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, size);
    assert_eq!(
        rec.calls[1],
        PaintCall::Border {
            width: 10.0,
            color: Color::BLACK
        }
    );
}

// ---------------------------------------------------------------------------
// Per-corner radius, dashed border, glow — the Arc C wave 2 container extras.
// ---------------------------------------------------------------------------

#[test]
fn per_corner_radii_forward_to_the_fill_call_uncollapsed() {
    // Four distinct corners: `fill_rounded_rect_radii` must receive them
    // exactly, not collapsed to a single uniform value.
    let radii = CornerRadii::new(2.0, 4.0, 6.0, 8.0);
    let view: frust_widgets::ContainerView<()> = colored_box()
        .fill(Color::from_rgb8(0x10, 0x20, 0x30))
        .radius(radii)
        .size(50.0, 50.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, Size::new(50.0, 50.0));
    assert_eq!(
        rec.calls,
        vec![PaintCall::Fill {
            radii,
            color: Color::from_rgb8(0x10, 0x20, 0x30)
        }]
    );
}

#[test]
fn a_bare_f64_radius_still_builds_a_uniform_corner_radii() {
    // `.radius(impl Into<CornerRadii>)`'s uniform ergonomics: `.radius(6.0)`
    // must still forward `CornerRadii::uniform(6.0)`, matching the pre-existing
    // `.radius(f64)` call shape exactly.
    let view: frust_widgets::ContainerView<()> = colored_box()
        .fill(Color::WHITE)
        .radius(6.0)
        .size(10.0, 10.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, Size::new(10.0, 10.0));
    assert_eq!(
        rec.calls,
        vec![PaintCall::Fill {
            radii: CornerRadii::uniform(6.0),
            color: Color::WHITE
        }]
    );
}

/// Per-corner fills and the border stroke path must agree: the border's
/// rounded-rect path is built from the *same* per-corner radii the fill call
/// receives, each corner independently inset by half the stroke width —
/// reconstructed here exactly as `Widget::paint` builds it, mirroring
/// `border_strokes_fully_inside_the_container_bounds_inset_by_half_width`'s
/// independent-geometry approach for the uniform case.
#[test]
fn per_corner_border_path_agrees_with_the_per_corner_fill_inset_by_half_width() {
    let radii = CornerRadii::new(2.0, 6.0, 10.0, 14.0);
    let width = 4.0;
    let size = Size::new(60.0, 50.0);
    let half = width / 2.0;

    let view: frust_widgets::ContainerView<()> = colored_box()
        .radius(radii)
        .border(Color::BLACK, width)
        .size(size.width, size.height);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, size);

    let expected = RoundedRect::new(
        half,
        half,
        size.width - half,
        size.height - half,
        (
            (radii.top_left - half).max(0.0),
            (radii.top_right - half).max(0.0),
            (radii.bottom_right - half).max(0.0),
            (radii.bottom_left - half).max(0.0),
        ),
    )
    // Mirrors `BORDER_TOLERANCE` in `container.rs` — a private const, so
    // reconstructed here as the same literal.
    .to_path(0.1);
    assert_eq!(rec.border_paths, vec![expected]);
}

#[test]
fn border_style_defaults_to_solid() {
    let view: frust_widgets::ContainerView<()> =
        colored_box().border(Color::BLACK, 2.0).size(20.0, 20.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, Size::new(20.0, 20.0));
    assert_eq!(
        rec.calls,
        vec![PaintCall::Border {
            width: 2.0,
            color: Color::BLACK
        }]
    );
}

#[test]
fn dashed_border_style_produces_a_dashed_stroke_command() {
    let dash = DashPattern::new(4.0, 2.0);
    let view: frust_widgets::ContainerView<()> = colored_box()
        .border(Color::BLACK, 2.0)
        .border_style(BorderStyle::Dashed(dash))
        .size(20.0, 20.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, Size::new(20.0, 20.0));
    assert_eq!(
        rec.calls,
        vec![PaintCall::DashedBorder {
            width: 2.0,
            color: Color::BLACK,
            dash
        }],
        "`.border_style(BorderStyle::Dashed(..))` must route through \
         `stroke_path_dashed`, not the solid `stroke_path`"
    );
}

#[test]
fn glow_paints_before_fill_and_border_shadow_beneath_the_shape() {
    let glow_color = Color::from_rgb8(0x00, 0x00, 0x00);
    let view: frust_widgets::ContainerView<()> = colored_box()
        .glow(glow_color, 4.0, 0.0)
        .fill(Color::WHITE)
        .border(Color::BLACK, 1.0)
        .size(30.0, 30.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, Size::new(30.0, 30.0));
    assert_eq!(
        rec.calls,
        vec![
            PaintCall::Shadow {
                origin: Point::ZERO,
                size: Size::new(30.0, 30.0),
                radius: 0.0,
                std_dev: 4.0,
                color: glow_color,
            },
            PaintCall::Fill {
                radii: CornerRadii::default(),
                color: Color::WHITE,
            },
            PaintCall::Border {
                width: 1.0,
                color: Color::BLACK,
            },
        ],
        "glow paints beneath the fill/border, mirroring `material::card`'s \
         `draw_shadow`-then-`fill` elevation order"
    );
}

#[test]
fn glow_spread_inflates_the_shadow_rect_symmetrically() {
    let view: frust_widgets::ContainerView<()> =
        colored_box().glow(Color::BLACK, 3.0, 5.0).size(40.0, 20.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, Size::new(40.0, 20.0));
    assert_eq!(
        rec.calls,
        vec![PaintCall::Shadow {
            origin: Point::new(-5.0, -5.0),
            size: Size::new(50.0, 30.0),
            radius: 0.0,
            std_dev: 3.0,
            color: Color::BLACK,
        }]
    );
}

#[test]
fn glow_negative_spread_shrinks_the_shadow_rect_and_clamps_at_zero() {
    // A spread more negative than half the smaller dimension would drive the
    // shadow rect negative; it clamps to zero instead of underflowing.
    let view: frust_widgets::ContainerView<()> = colored_box()
        .glow(Color::BLACK, 1.0, -100.0)
        .size(40.0, 20.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, Size::new(40.0, 20.0));
    assert_eq!(
        rec.calls,
        vec![PaintCall::Shadow {
            origin: Point::new(100.0, 100.0),
            size: Size::ZERO,
            radius: 0.0,
            std_dev: 1.0,
            color: Color::BLACK,
        }]
    );
}

#[test]
fn glow_radius_lowers_through_the_largest_corner() {
    let radii = CornerRadii::new(2.0, 9.0, 3.0, 4.0);
    let view: frust_widgets::ContainerView<()> = colored_box()
        .radius(radii)
        .glow(Color::BLACK, 1.0, 0.0)
        .size(20.0, 20.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, Size::new(20.0, 20.0));
    assert_eq!(
        rec.calls,
        vec![PaintCall::Shadow {
            origin: Point::ZERO,
            size: Size::new(20.0, 20.0),
            radius: 9.0,
            std_dev: 1.0,
            color: Color::BLACK,
        }]
    );
}

// ---------------------------------------------------------------------------
// `.size_centered(...)` — the `Panel::fixed` fixed-size-with-centered-child
// case.
// ---------------------------------------------------------------------------

#[test]
fn size_centered_forces_the_fixed_size_even_with_a_child_attached() {
    let view: frust_widgets::ContainerView<()> =
        container(leaf(20.0, 10.0)).size_centered(100.0, 60.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    assert_eq!(
        size,
        Size::new(100.0, 60.0),
        "`.size_centered` forces the fixed size even though a child is attached, \
         unlike plain `.size`"
    );
}

#[test]
fn size_centered_centers_the_child_in_the_free_space() {
    let view: frust_widgets::ContainerView<()> =
        container(leaf(20.0, 10.0)).size_centered(100.0, 60.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    let rec = paint_at(&mut w, size);
    // (100-20)/2 = 40, (60-10)/2 = 25 — the child's leaf paints a fill_rect at
    // its absolute origin, which threads through the container's own origin
    // (zero here) plus the child's centered local origin.
    assert!(
        rec.rects
            .contains(&(Point::new(40.0, 25.0), Size::new(20.0, 10.0))),
        "expected the 20x10 child centered at (40, 25) inside the 100x60 box; \
         recorded rects: {:?}",
        rec.rects
    );
}

#[test]
fn plain_size_still_hugs_the_child_tight_once_attached() {
    // The split between `.size` (childless only) and `.size_centered` (the
    // one exception): `.size` on a container with a child attached is
    // ignored, and layout keeps hugging the child exactly.
    let view: frust_widgets::ContainerView<()> = container(leaf(20.0, 10.0)).size(100.0, 60.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    assert_eq!(
        size,
        Size::new(20.0, 10.0),
        "`.size` (not `.size_centered`) stays ignored once a child is attached"
    );
}

#[test]
fn size_centered_childless_behaves_like_plain_size() {
    let view: frust_widgets::ContainerView<()> = colored_box().size_centered(48.0, 24.0);
    let mut w = build(&view);
    let mut lctx = LayoutCtx::new();
    let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
    assert_eq!(size, Size::new(48.0, 24.0));
}

// ---------------------------------------------------------------------------
// `divider` — a lighter integration pass alongside its own in-module unit
// tests (`crates/frust-widgets/src/divider.rs`): fills the cross-axis, takes
// a fixed thickness on the main axis, paints exactly one `fill_rect` in its
// color.
// ---------------------------------------------------------------------------

#[test]
fn divider_default_orientation_fills_width_with_thickness_on_height() {
    let view = divider(Color::from_rgb8(0xEE, 0xEE, 0xEE));
    let mut w: frust_widgets::DividerWidget = build::<(), _>(&view);
    let mut lctx = LayoutCtx::new();
    let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
    assert_eq!(size, Size::new(400.0, 1.0));
}

#[test]
fn divider_vertical_fills_height_with_thickness_on_width() {
    let view = divider(Color::BLACK).vertical().thickness(2.0);
    let mut w: frust_widgets::DividerWidget = build::<(), _>(&view);
    let mut lctx = LayoutCtx::new();
    let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
    assert_eq!(size, Size::new(2.0, 400.0));
}

#[test]
fn divider_paints_its_line_via_a_single_fill_rect() {
    let view = divider(Color::from_rgb8(0x10, 0x10, 0x10));
    let mut w: frust_widgets::DividerWidget = build::<(), _>(&view);
    let mut lctx = LayoutCtx::new();
    let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 100.0)));
    let rec = paint_at(&mut w, size);
    assert_eq!(rec.rects, vec![(Point::ZERO, size)]);
}

// ---------------------------------------------------------------------------
// Semantics passthrough — purely decorative
// ---------------------------------------------------------------------------

/// Every contributed role *except* the always-present `Role::Window` root
/// node `RenderRoot::semantics` publishes for any tree (see
/// `tests/semantics_tree.rs`'s identical precedent) — the roles this test
/// file actually cares about are the widget tree's own.
fn inspect_roles<V: View<()>>(mut app_logic: impl FnMut(&mut ()) -> V) -> Vec<Role> {
    let mut root: RenderRoot<(), V> = RenderRoot::new();
    let mut state = ();
    root.rebuild(&mut app_logic, &mut state);
    let mut text_ctx = frust_text::TextContext::new();
    root.layout_with_text(Size::new(400.0, 400.0), &mut text_ctx as &mut dyn Any);
    root.semantics()
        .nodes
        .iter()
        .map(|(_, node)| node.role())
        .filter(|role| *role != Role::Window)
        .collect()
}

#[test]
fn a_decorated_container_contributes_no_node_of_its_own() {
    // `text(...)` contributes exactly one `Role::Label` node (see
    // `tests/semantics_tree.rs`'s precedent) — a container wrapping it with
    // fill/border/radius set must add nothing beyond that single node.
    let roles = inspect_roles(|_: &mut ()| {
        container(text("hello"))
            .fill(Color::WHITE)
            .radius(8.0)
            .border(Color::BLACK, 1.0)
    });
    assert_eq!(
        roles.len(),
        1,
        "the container is purely decorative and forwards the child's node only"
    );
}

#[test]
fn a_childless_container_contributes_no_node() {
    let roles = inspect_roles(|_: &mut ()| colored_box().fill(Color::WHITE).expand());
    assert!(
        roles.is_empty(),
        "a childless container has nothing to forward and adds nothing itself"
    );
}
