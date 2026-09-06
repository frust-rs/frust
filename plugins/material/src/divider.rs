//! The Material 3 Expressive `Divider`: a thin line grouping content in
//! lists and containers, in either orientation, with optional leading/
//! trailing insets.
//!
//! Ported from `material_3_expressive` v1.0.8's `M3EDivider`/`M3EDividerTheme`
//! (MIT, © 2026 Paa Developments;
//! `tmp/material_3_expressive/lib/components/divider/m3e_divider.dart`,
//! `styles/m3e_divider_theme.dart`, retrieved 2026-08-20).
//! Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! # Relationship to `frust::divider`
//!
//! Baseline `frust-widgets` already ships [`frust::divider`] — a themeless
//! hairline with no default color, no insets, and a required `color` argument
//! (see that module's own doc for why: divider has no `Theme` dependency at
//! all, deliberately). This module reuses the plain `divider`/`DividerView`
//! names in `frust_material` on purpose, the same cross-crate naming
//! precedent [`mod@crate::checkbox`]/[`mod@crate::switch`] already set against
//! their own baseline counterparts — an app picks exactly one by import path
//! (`frust_material::divider` vs. `frust::divider`), never both at the same
//! call site.
//!
//! [`divider`] layers the reference's three additions over that primitive:
//! a theme-resolved default color (`outlineVariant`, `m3e_divider_theme.dart`'s
//! `M3EDividerTheme::color`), leading/trailing insets (`indent`/`endIndent`),
//! and a live per-repaint theme re-resolution baseline's own theme-free
//! design can't offer.
//!
//! # Composition, not reimplementation
//!
//! [`DividerWidget::layout`]/[`DividerWidget::paint`] never draw a line of
//! their own — both construct a genuine `frust::divider(..)` (via
//! [`base_divider`]) and delegate the *entire* sizing algorithm (the
//! fill-axis-collapse rule, thickness handling, tight-constraint clamping —
//! see [`frust::authoring`]'s worked composition example and
//! `frust-widgets`' `divider` module docs for what that buys) and the *entire*
//! paint (`PaintScene::fill_rect`) to it. Only the leading/trailing inset
//! arithmetic — reducing the fill axis before layout, offsetting the origin
//! before paint, both a two-line rect adjustment rather than a drawing
//! primitive — is this module's own.
//!
//! # Color resolution: paint-time, not build-time
//!
//! Unlike most of this crate's themed defaults (resolved once and cached),
//! [`DividerWidget::paint`] resolves `outlineVariant` fresh from
//! [`Theme::from_paint_ctx`] on *every* paint call, matching every other
//! themed widget here (`crate::card`, `crate::list_item`, …). This is load
//! -bearing, not decorative: `BuildCtx` carries no theme (see
//! `crates/frust-core/src/view.rs`), and an app-forced theme flip
//! (`frust::set_app_theme`/`clear_app_theme`) is explicitly documented as
//! **not** tracked by any `Component::build` — only a widget's own paint/
//! layout sees it live. Baking the resolved color into the `View` tree once
//! (the `AvatarGroupCountView`-style delegation `frust-shadcn` uses elsewhere)
//! would silently freeze the divider's color across a theme-only flip; this
//! module resolves the default fresh every frame instead, at the small,
//! well-contained cost of a throwaway `BuildCtx`/`View::build` pair per
//! layout/paint call (harmless here: baseline `DividerView::build`/`rebuild`
//! never touch `ctx`, so the scratch id counter is never read).
//!
//! # Color role: `MaterialTokens`-adjacent, not extension-scoped
//!
//! `outlineVariant` is a baseline `ColorScheme` role (`Theme::scheme()`), not
//! one of the nine [`crate::MaterialTokens`]-only semantic roles (`emphasis`/
//! `info`/`success`/…) — see `crate::tokens`'s module docs' role-partition
//! table. This module therefore never touches `MaterialTokens` or
//! `frust::StatusPalette`; it resolves the plain `ColorScheme` role a themed
//! `Theme` always carries, honoring this crate's binding rule of never
//! reaching for `StatusPalette` where a Material-native role exists.

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use frust::{Theme, divider as base_divider};
use kurbo::{Point, Size};
use peniko::Color;

/// The hairline thickness [`divider`] uses unless overridden via
/// [`DividerView::thickness`] — matches baseline `frust::divider`'s own
/// `DEFAULT_THICKNESS` (1px, `M3EDividerTheme.defaults`'s `thickness: 1`).
const DEFAULT_THICKNESS: f64 = 1.0;

/// Unthemed-fallback divider line color (a theme resolves this from
/// `colors.outline_variant`) — the M3 baseline light `outlineVariant` tone,
/// matching [`mod@crate::card`]'s own `OUTLINE_VARIANT` fallback exactly.
const FALLBACK_OUTLINE_VARIANT: Color = Color::from_rgb8(0xCA, 0xC4, 0xD0);

/// A declarative M3 Expressive divider. See the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DividerView {
    vertical: bool,
    thickness: f64,
    indent: f64,
    end_indent: f64,
    color: Option<Color>,
}

/// A horizontal M3 Expressive divider, `outlineVariant`-colored unless
/// overridden. See the [module docs](self).
pub fn divider() -> DividerView {
    DividerView {
        vertical: false,
        thickness: DEFAULT_THICKNESS,
        indent: 0.0,
        end_indent: 0.0,
        color: None,
    }
}

impl DividerView {
    /// Switch to a vertical line (fixed thickness on width, filling height) —
    /// mirrors [`frust::DividerView::vertical`]. Horizontal by default.
    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }

    /// Override the line's thickness, in logical px. [`DEFAULT_THICKNESS`]
    /// (1px) by default.
    pub fn thickness(mut self, thickness: f64) -> Self {
        self.thickness = thickness;
        self
    }

    /// Inset the line's leading edge (left for horizontal, top for vertical)
    /// by `indent` logical px — the reference's `indent`. `0` by default.
    pub fn indent(mut self, indent: f64) -> Self {
        self.indent = indent;
        self
    }

    /// Inset the line's trailing edge (right for horizontal, bottom for
    /// vertical) by `end_indent` logical px — the reference's `endIndent`.
    /// `0` by default. Setting both [`Self::indent`] and this produces the
    /// reference's "middle" inset (both edges pulled in).
    pub fn end_indent(mut self, end_indent: f64) -> Self {
        self.end_indent = end_indent;
        self
    }

    /// Override the theme-resolved `outlineVariant` default with an explicit
    /// color.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
}

/// The retained widget for a [`DividerView`]. See the [module docs](self).
pub struct DividerWidget {
    vertical: bool,
    thickness: f64,
    indent: f64,
    end_indent: f64,
    color: Option<Color>,
}

impl<State: 'static> View<State> for DividerView {
    type Element = DividerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> DividerWidget {
        DividerWidget {
            vertical: self.vertical,
            thickness: self.thickness,
            indent: self.indent,
            end_indent: self.end_indent,
            color: self.color,
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
        if prev.vertical != self.vertical
            || prev.thickness != self.thickness
            || prev.indent != self.indent
            || prev.end_indent != self.end_indent
        {
            element.vertical = self.vertical;
            element.thickness = self.thickness;
            element.indent = self.indent;
            element.end_indent = self.end_indent;
            flags |= ChangeFlags::LAYOUT;
        }
        flags
    }
}

impl DividerWidget {
    /// The `(leading, trailing)` inset amounts, clamped non-negative —
    /// mirrors `frust::Padding`'s own clamp-negative-to-zero rule.
    fn insets(&self) -> (f64, f64) {
        (self.indent.max(0.0), self.end_indent.max(0.0))
    }

    /// Build a fresh, one-shot baseline [`frust::divider`] at `color` for
    /// this widget's current axis/thickness — the single construction point
    /// both [`Widget::layout`] and [`Widget::paint`] call through, so the
    /// two never drift (see the [module docs](self)' Composition section).
    fn base(&self, color: Color) -> frust::DividerView {
        let mut view = base_divider(color).thickness(self.thickness);
        if self.vertical {
            view = view.vertical();
        }
        view
    }

    /// Resolve this instance's paint color: explicit override, else the
    /// active theme's `outlineVariant`, else [`FALLBACK_OUTLINE_VARIANT`] —
    /// see the [module docs](self)' Color resolution section for why this
    /// runs fresh every paint rather than once.
    fn resolve_color(&self, theme: Option<&Theme>) -> Color {
        if let Some(color) = self.color {
            return color;
        }
        match theme {
            Some(theme) => theme.scheme().outline_variant,
            None => FALLBACK_OUTLINE_VARIANT,
        }
    }
}

/// Build a throwaway `DividerWidget` element for `view` — the scratch
/// `BuildCtx` this needs is never read by baseline `DividerView::build`
/// (see the [module docs](self)' Color resolution section), so allocating
/// one per call is a harmless formality, not a real id allocation.
fn build_line<State: 'static>(view: &frust::DividerView) -> frust::DividerWidget {
    let mut counter = 0u64;
    View::<State>::build(view, &mut BuildCtx::new(&mut counter))
}

impl Widget for DividerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let (before, after) = self.insets();
        let total = before + after;
        let inner_bc = if self.vertical {
            BoxConstraints::new(
                Size::new(bc.min().width, (bc.min().height - total).max(0.0)),
                Size::new(bc.max().width, (bc.max().height - total).max(0.0)),
            )
        } else {
            BoxConstraints::new(
                Size::new((bc.min().width - total).max(0.0), bc.min().height),
                Size::new((bc.max().width - total).max(0.0), bc.max().height),
            )
        };
        let view = self.base(FALLBACK_OUTLINE_VARIANT); // color has zero effect on sizing
        let mut line = build_line::<()>(&view);
        let inner_size = line.layout(ctx, &inner_bc);
        let full = if self.vertical {
            Size::new(inner_size.width, inner_size.height + total)
        } else {
            Size::new(inner_size.width + total, inner_size.height)
        };
        bc.constrain(full)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let color = self.resolve_color(theme);
        let (before, after) = self.insets();
        let total = before + after;
        let (inner_origin, inner_size) = if self.vertical {
            (
                Point::new(ctx.origin().x, ctx.origin().y + before),
                Size::new(ctx.size().width, (ctx.size().height - total).max(0.0)),
            )
        } else {
            (
                Point::new(ctx.origin().x + before, ctx.origin().y),
                Size::new((ctx.size().width - total).max(0.0), ctx.size().height),
            )
        };
        let view = self.base(color);
        let mut line = build_line::<()>(&view);
        // Baseline `DividerWidget::paint` reads geometry solely from the
        // `PaintCtx` it's handed (never its own internal state), so no prior
        // `layout()` call on this throwaway instance is needed — see the
        // module docs' Composition section.
        let mut inner_ctx = PaintCtx::new(inner_origin, inner_size);
        line.paint(&mut inner_ctx, scene);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Point;

    fn build(view: &DividerView) -> DividerWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    /// [`Theme::neutral`] with `outline_variant` overridden — `Brightness`
    /// stays `Light`, matching `neutral`'s own default, so `theme.scheme()`
    /// resolves `theme.light` (the one we mutate).
    fn themed(outline_variant: Color) -> Theme {
        let mut theme = Theme::neutral();
        theme.light.outline_variant = outline_variant;
        theme
    }

    // ---- Layout ----------------------------------------------------------

    #[test]
    fn horizontal_default_fills_width_and_takes_default_thickness_on_height() {
        let mut w = build(&divider());
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(320.0, 640.0)));
        assert_eq!(size, Size::new(320.0, DEFAULT_THICKNESS));
    }

    #[test]
    fn vertical_fills_height_and_takes_thickness_on_width() {
        let mut w = build(&divider().vertical());
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(320.0, 640.0)));
        assert_eq!(size, Size::new(DEFAULT_THICKNESS, 640.0));
    }

    #[test]
    fn custom_thickness_is_honored() {
        let mut w = build(&divider().thickness(4.0));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert_eq!(size, Size::new(200.0, 4.0));
    }

    #[test]
    fn insets_shrink_the_line_but_the_wrapper_still_reports_the_full_fill_size() {
        let mut w = build(&divider().indent(8.0).end_indent(4.0));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 50.0)));
        // The outer box still fills to the incoming max — only the painted
        // line (see the paint test below) is narrower.
        assert_eq!(size, Size::new(100.0, DEFAULT_THICKNESS));
    }

    #[test]
    fn vertical_insets_apply_to_the_fill_axis_too() {
        let mut w = build(&divider().vertical().indent(10.0).end_indent(5.0));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(50.0, 200.0)));
        assert_eq!(size, Size::new(DEFAULT_THICKNESS, 200.0));
    }

    #[test]
    fn a_tight_constraint_forces_the_divider_to_it_regardless_of_thickness() {
        let mut w = build(&divider());
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::tight(Size::new(50.0, 50.0)));
        assert_eq!(size, Size::new(50.0, 50.0));
    }

    // ---- Paint -------------------------------------------------------------

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
    fn paint_with_no_theme_uses_the_unthemed_fallback() {
        let mut w = build(&divider());
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 100.0)));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(
            rec.rects,
            vec![(Point::ZERO, size, FALLBACK_OUTLINE_VARIANT)]
        );
    }

    #[test]
    fn paint_resolves_the_theme_s_outline_variant_when_no_explicit_color() {
        let mut w = build(&divider());
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 100.0)));

        let expected = Color::from_rgb8(0x11, 0x22, 0x33);
        let theme = themed(expected);
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(rec.rects, vec![(Point::ZERO, size, expected)]);
    }

    #[test]
    fn explicit_color_wins_over_the_theme() {
        let explicit = Color::from_rgb8(0xAA, 0xBB, 0xCC);
        let mut w = build(&divider().color(explicit));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 100.0)));

        let theme = themed(Color::from_rgb8(0x11, 0x22, 0x33));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(rec.rects, vec![(Point::ZERO, size, explicit)]);
    }

    #[test]
    fn horizontal_insets_offset_and_shrink_the_painted_line() {
        let mut w = build(&divider().indent(8.0).end_indent(4.0));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 50.0)));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::new(2.0, 3.0), size);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(
            rec.rects,
            vec![(
                Point::new(2.0 + 8.0, 3.0),
                Size::new(100.0 - 8.0 - 4.0, DEFAULT_THICKNESS),
                FALLBACK_OUTLINE_VARIANT
            )]
        );
    }

    #[test]
    fn vertical_insets_offset_and_shrink_the_painted_line() {
        let mut w = build(&divider().vertical().indent(10.0).end_indent(5.0));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(50.0, 200.0)));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(
            rec.rects,
            vec![(
                Point::new(0.0, 10.0),
                Size::new(DEFAULT_THICKNESS, 200.0 - 10.0 - 5.0),
                FALLBACK_OUTLINE_VARIANT
            )]
        );
    }

    #[test]
    fn insets_larger_than_the_line_collapse_to_zero_rather_than_going_negative() {
        let mut w = build(&divider().indent(80.0).end_indent(80.0));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 50.0)));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(rec.rects[0].1.width, 0.0);
    }
}
