//! Ports shadcn/ui's **Separator** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/separator.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`) — a 1px hairline rule, running
//! horizontal or vertical, in the theme's border color (`bg-border`).
//!
//! # Deviation: no fill-parent layout
//!
//! The source is `h-px w-full` (horizontal) / `h-full w-px` (vertical) — it
//! stretches to whatever cross-axis size its flex/grid parent gives it. This
//! port has no such parent contract to lean on, so it fills the **finite**
//! side of its incoming [`BoxConstraints`] max (the loose axis collapses to
//! zero) — a caller inside a fixed-width row/column sees the expected full
//! bleed; a caller under unconstrained space gets a degenerate zero-length
//! rule rather than an infinite one.
//!
//! # Decorative by default
//!
//! Radix's own `decorative={true}` default means a plain separator paints no
//! accessibility node at all (this port's [`Widget::semantics`] is simply the
//! inherited no-op). [`SeparatorView::role_separator`] opts a *meaningful*
//! divider — one that actually delimits sections a screen reader should
//! announce — into a `Role::Splitter` node (Radix's `decorative={false}`).

use frust::Theme;
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, Color, LayoutCtx, PaintCtx, PaintScene, Role,
    SemanticsCtx, Size, View, Widget,
};

use crate::style::BORDER_WIDTH;

/// The axis a [`SeparatorView`] runs along.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SeparatorOrientation {
    /// A 1px-tall horizontal rule, full width.
    #[default]
    Horizontal,
    /// A 1px-wide vertical rule, full height.
    Vertical,
}

/// Unthemed fallback rule color (a light grey; a theme resolves this from
/// `colors.outline`, shadcn's `--border`).
const FALLBACK_BORDER: Color = Color::from_rgb8(0xE5, 0xE5, 0xE5);

/// A declarative shadcn separator. See the [module docs](self).
pub struct SeparatorView {
    orientation: SeparatorOrientation,
    decorative: bool,
}

/// Create a horizontal, decorative separator.
pub fn separator() -> SeparatorView {
    SeparatorView {
        orientation: SeparatorOrientation::Horizontal,
        decorative: true,
    }
}

impl SeparatorView {
    /// Select the running axis (default [`SeparatorOrientation::Horizontal`]).
    pub fn orientation(mut self, orientation: SeparatorOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Shorthand for `.orientation(SeparatorOrientation::Vertical)`.
    pub fn vertical(mut self) -> Self {
        self.orientation = SeparatorOrientation::Vertical;
        self
    }

    /// Mark this rule as *meaningful* rather than purely decorative — see the
    /// [module docs](self)'s "Decorative by default" section.
    pub fn role_separator(mut self) -> Self {
        self.decorative = false;
        self
    }
}

/// The retained widget for a [`SeparatorView`].
pub struct SeparatorWidget {
    orientation: SeparatorOrientation,
    decorative: bool,
}

impl<State: 'static> View<State> for SeparatorView {
    type Element = SeparatorWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SeparatorWidget {
        SeparatorWidget {
            orientation: self.orientation,
            decorative: self.decorative,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SeparatorWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.orientation != self.orientation {
            element.orientation = self.orientation;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.decorative != self.decorative {
            element.decorative = self.decorative;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// The rule color: themed `colors.outline`, else [`FALLBACK_BORDER`].
fn resolve_color(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK_BORDER, |t| t.scheme().outline)
}

impl Widget for SeparatorWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        match self.orientation {
            SeparatorOrientation::Horizontal => {
                let width = if bc.max().width.is_finite() {
                    bc.max().width
                } else {
                    0.0
                };
                bc.constrain(Size::new(width, BORDER_WIDTH))
            }
            SeparatorOrientation::Vertical => {
                let height = if bc.max().height.is_finite() {
                    bc.max().height
                } else {
                    0.0
                };
                bc.constrain(Size::new(BORDER_WIDTH, height))
            }
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let color = resolve_color(theme);
        scene.fill_rect(ctx.origin(), ctx.size(), color);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if !self.decorative {
            ctx.push_node(Role::Splitter, |_node| {});
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{BuildCtx, Point};

    fn build(view: &SeparatorView) -> SeparatorWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    fn paint(w: &mut SeparatorWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn horizontal_layout_fills_finite_width_at_hairline_height() {
        let mut w = build(&separator());
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size, Size::new(300.0, BORDER_WIDTH));
    }

    #[test]
    fn vertical_layout_fills_finite_height_at_hairline_width() {
        let mut w = build(&separator().vertical());
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size, Size::new(BORDER_WIDTH, 200.0));
    }

    #[test]
    fn unthemed_paint_uses_the_fallback_constant() {
        let mut w = build(&separator());
        let rec = paint(&mut w, Size::new(100.0, BORDER_WIDTH), None);
        assert_eq!(rec.rects[0].2, FALLBACK_BORDER);
    }

    #[test]
    fn themed_paint_resolves_the_outline_role() {
        let theme = crate::tokens::theme();
        let mut w = build(&separator());
        let rec = paint(&mut w, Size::new(100.0, BORDER_WIDTH), Some(&theme));
        assert_eq!(rec.rects[0].2, theme.scheme().outline);
    }

    #[test]
    fn decorative_by_default_emits_no_semantics_node() {
        let w = build(&separator());
        assert!(w.decorative);
    }

    #[test]
    fn role_separator_flips_the_decorative_flag() {
        let w = build(&separator().role_separator());
        assert!(!w.decorative);
    }

    #[test]
    fn rebuild_reports_paint_only_when_just_decorative_changes() {
        let prev = separator();
        let next = separator().role_separator();
        let mut element = build(&prev);
        let mut counter = 0u64;
        let flags =
            View::<()>::rebuild(&next, &prev, &mut element, &mut BuildCtx::new(&mut counter));
        assert_eq!(flags, ChangeFlags::PAINT);
        assert!(!element.decorative);
    }
}
