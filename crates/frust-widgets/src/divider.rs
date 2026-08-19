//! `Divider`: a themeless hairline separator.
//!
//! [`divider`] builds a horizontal line ([`DividerView`]) by default —
//! [`DividerView::vertical`] switches it to a vertical one. Either way the
//! divider is a leaf: no child, no events, no semantics node of its own
//! (mirrors [`crate::sized::SizedBox`]'s childless-spacer shape, minus the
//! spacer's own nothing-painted case — a divider always paints its line).
//!
//! # Sizing
//!
//! A horizontal divider (the default) fills the incoming max **width**
//! (the cross axis of the vertical `Column` it typically separates rows
//! inside) and takes a fixed `.thickness(...)` on the **height** — its own
//! main axis, in the sense that a `Column`'s main axis is vertical.
//! [`DividerView::vertical`] flips both: fixed thickness on width, fills the
//! incoming max height (the typical `Row`-of-panels separator).
//!
//! The fill axis uses the incoming `bc.max()` when it is a real bound, but
//! **collapses to `bc.min()`** (usually `0`) when that axis is unbounded
//! (`max == f64::INFINITY`) — the shape `Flex`'s inflexible-child
//! intrinsic-probe pass, `ScrollView`'s content layout, and `ListView`'s row
//! layout all hand a child measuring its natural extent along that axis. A
//! fixed "larger than any real viewport" sentinel cannot stand in for the
//! fill axis here: `BoxConstraints::constrain`'s clamp is a no-op against an
//! infinite max (`x.clamp(min, f64::INFINITY)` never lowers `x`), so any
//! such sentinel would leak through unclamped and report a fictitious
//! multi-thousand-pixel divider to the caller. So a divider dropped into a
//! genuinely unbounded axis (no enclosing `Column`/`Row`/other bound
//! constraining it) is effectively invisible on that axis — `0`-width (or
//! `0`-height) under a loose incoming minimum — rather than growing
//! unbounded; the thickness axis is unaffected either way. See
//! [`crate::container`]'s `.expand()` doc for the same collapse rule, shared
//! with `Container`'s childless expand case.
//!
//! # Color: required, no `Theme` dependency
//!
//! Unlike `Text`/`Button`/`Checkbox`/`Slider` — every themed baseline widget
//! that resolves a default color via `Theme::from_paint_ctx`/
//! `from_layout_ctx` with an unthemed-fallback constant (see
//! `docs/WIDGETS_CODE_STANDARDS.md`'s token-resolution precedence) —
//! `divider` has no `Theme` dependency at all and no default color,
//! matching [`crate::container`]'s own theme-free design (that module never
//! touches `Theme`, and its `.fill`/`.border` colors have no default either).
//! No baseline widget sources a default paint color *without* consulting
//! `Theme` first, so there is no neutral constant to fall back to here that
//! wouldn't itself need `Theme` — [`divider`] instead takes `color` as a
//! required constructor argument, overridable via [`DividerView::color`].

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use kurbo::Size;
use peniko::Color;

/// The hairline thickness [`divider`] uses unless overridden via
/// [`DividerView::thickness`] — a standard 1px separator (Material's
/// `Divider` spec and the Human Interface Guidelines' hairline separator
/// both converge on 1px at 1x scale).
const DEFAULT_THICKNESS: f64 = 1.0;

/// A declarative hairline separator. See the [module docs](self).
pub struct DividerView {
    color: Color,
    thickness: f64,
    vertical: bool,
}

/// A horizontal hairline of `color`, [`DEFAULT_THICKNESS`] (1px) thick,
/// filling the available width. `color` is a required argument, not
/// defaulted — see the [module docs](self)' "Color: required, no `Theme`
/// dependency" section.
pub fn divider(color: Color) -> DividerView {
    DividerView {
        color,
        thickness: DEFAULT_THICKNESS,
        vertical: false,
    }
}

impl DividerView {
    /// Override the line color set at construction.
    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Set the line's thickness, in logical px. [`DEFAULT_THICKNESS`] (1px)
    /// by default.
    pub fn thickness(mut self, thickness: f64) -> Self {
        self.thickness = thickness;
        self
    }

    /// Switch to a vertical line: fixed thickness on width, filling the
    /// available height — the typical separator between side-by-side
    /// panels. Horizontal (fixed thickness on height, filling width) by
    /// default; see the [module docs](self)' "Sizing" section.
    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }
}

/// The retained widget for a [`DividerView`]. See the [module docs](self).
pub struct DividerWidget {
    color: Color,
    thickness: f64,
    vertical: bool,
}

impl<State: 'static> View<State> for DividerView {
    type Element = DividerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> DividerWidget {
        DividerWidget {
            color: self.color,
            thickness: self.thickness,
            vertical: self.vertical,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut DividerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.color != self.color {
            element.color = self.color;
            flags |= ChangeFlags::PAINT;
        }
        if prev.thickness != self.thickness || prev.vertical != self.vertical {
            element.thickness = self.thickness;
            element.vertical = self.vertical;
            flags |= ChangeFlags::LAYOUT;
        }
        flags
    }
}

impl Widget for DividerWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Fill axis: the incoming max when it's a real bound, else collapse
        // to the incoming min (see the module docs' "Sizing" section) —
        // `BoxConstraints::constrain`'s clamp can't be relied on to cap an
        // unbounded max, so a finite "large enough" intrinsic would leak
        // straight through unclamped.
        let fill = |max: f64, min: f64| if max.is_finite() { max } else { min };
        let intrinsic = if self.vertical {
            Size::new(self.thickness, fill(bc.max().height, bc.min().height))
        } else {
            Size::new(fill(bc.max().width, bc.min().width), self.thickness)
        };
        bc.constrain(intrinsic)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), self.color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::BuildCtx;
    use kurbo::Point;

    fn build(view: &DividerView) -> DividerWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    // ---- Layout: fills cross-axis, thickness on main -----------------

    #[test]
    fn horizontal_divider_fills_width_and_takes_thickness_on_height() {
        let view = divider(Color::BLACK);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(320.0, 640.0)));
        assert_eq!(size, Size::new(320.0, DEFAULT_THICKNESS));
    }

    #[test]
    fn horizontal_divider_honors_a_custom_thickness() {
        let view = divider(Color::BLACK).thickness(4.0);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert_eq!(size, Size::new(200.0, 4.0));
    }

    #[test]
    fn vertical_divider_fills_height_and_takes_thickness_on_width() {
        let view = divider(Color::BLACK).vertical();
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(320.0, 640.0)));
        assert_eq!(size, Size::new(DEFAULT_THICKNESS, 640.0));
    }

    #[test]
    fn a_tight_constraint_forces_the_divider_to_it_regardless_of_thickness() {
        // `BoxConstraints::constrain` always wins — mirrors `Container`'s own
        // `.expand()` precedent under a tight constraint.
        let view = divider(Color::BLACK);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::tight(Size::new(50.0, 50.0)));
        assert_eq!(size, Size::new(50.0, 50.0));
    }

    #[test]
    fn an_unbounded_fill_axis_collapses_to_the_incoming_minimum() {
        // No enclosing Column/Row bounding the fill axis (width, here):
        // `BoxConstraints::constrain`'s clamp is a no-op against an infinite
        // max, so the fill axis instead collapses to `bc.min()` (zero under
        // a loose incoming constraint) — the same "no real max to fill"
        // fallback `Container::expand` shares (see `container.rs`'s module
        // doc). The thickness axis (height) is unaffected either way.
        let view = divider(Color::BLACK);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let bc = BoxConstraints::loose(Size::new(f64::INFINITY, 200.0));
        let size = w.layout(&mut lctx, &bc);
        assert_eq!(size, Size::new(0.0, DEFAULT_THICKNESS));
    }

    #[test]
    fn an_unbounded_fill_axis_collapses_to_a_nonzero_incoming_minimum() {
        // A non-zero `bc.min()` on the unbounded axis is honored, not
        // silently zeroed — the collapse target is genuinely `bc.min()`,
        // not a hardcoded zero. The thickness axis (width, here) stays
        // loose enough that its own minimum doesn't also force it.
        let view = divider(Color::BLACK).vertical();
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let bc = BoxConstraints::new(Size::new(0.0, 12.0), Size::new(500.0, f64::INFINITY));
        let size = w.layout(&mut lctx, &bc);
        assert_eq!(size, Size::new(DEFAULT_THICKNESS, 12.0));
    }

    // ---- Paint ---------------------------------------------------------

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }

    #[test]
    fn paint_fills_exactly_one_rect_in_the_divider_color() {
        let view = divider(Color::from_rgb8(0xCC, 0xCC, 0xCC));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 100.0)));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(
            rec.rects,
            vec![(Point::ZERO, size, Color::from_rgb8(0xCC, 0xCC, 0xCC))]
        );
    }

    #[test]
    fn color_override_wins_over_the_constructor_argument() {
        let view = divider(Color::BLACK).color(Color::from_rgb8(0x11, 0x22, 0x33));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(10.0, 10.0)));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(
            rec.rects,
            vec![(Point::ZERO, size, Color::from_rgb8(0x11, 0x22, 0x33))]
        );
    }
}
