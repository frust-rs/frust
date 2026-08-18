//! Integration tests for `container`/`colored_box`
//! (`crates/frust-widgets/src/container.rs`): child-sizing passthrough (the
//! wrapper hugs its child; a childless `.expand()`/`.size(...)` box), paint
//! discipline (fill then border, the border's half-width inset, radius
//! forwarding), the "no decoration = no paint commands" contract, and
//! semantics passthrough (purely decorative — the container itself
//! contributes no accessibility node).

use std::any::Any;

use frust_core::accesskit::Role;
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, RenderRoot, View,
    Widget,
};
use frust_widgets::{colored_box, container, text};
use kurbo::{BezPath, Point, Size};
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
// Paint recorder: fill/border calls in emission order, plus a bare count for
// the "no decoration" assertion.
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum PaintCall {
    Fill { radius: f64, color: Color },
    Border { width: f64, color: Color },
}

#[derive(Default)]
struct Recorder {
    calls: Vec<PaintCall>,
}

impl PaintScene for Recorder {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}

    fn fill_rounded_rect(&mut self, _origin: Point, _size: Size, radius: f64, color: Color) {
        self.calls.push(PaintCall::Fill { radius, color });
    }

    fn stroke_path(&mut self, _origin: Point, _path: &BezPath, width: f64, brush: &Brush) {
        if let Brush::Solid(color) = brush {
            self.calls.push(PaintCall::Border {
                width,
                color: *color,
            });
        }
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
            radius: 0.0,
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
                radius: 6.0,
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
    use kurbo::RoundedRect;

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
