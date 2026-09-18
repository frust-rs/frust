//! The Material 3 Expressive `Badge`: a small dot or numeric indicator
//! anchored to the top edge of any child view.
//!
//! Ported from `material_3_expressive` v1.0.8's `M3EBadge`/`M3EBadgeTheme`/
//! `M3EBadgeLayout` (MIT, © 2026 Paa Developments;
//! `tmp/material_3_expressive/lib/components/badges/m3e_badges.dart`,
//! `styles/m3e_badge_theme.dart`, `components/m3e_badge_layout.dart`,
//! retrieved 2026-08-20).
//! Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! [`badge`] wraps any child view; [`BadgeView::dot`] shows an unlabeled dot,
//! [`BadgeView::count`] shows a numeric label (formatted against
//! [`BadgeView::max_count`], default 99 — e.g. `count(150)` at the default
//! max paints `"99+"`). Neither called leaves the child exactly as given, no
//! wrapper geometry at all (`M3EBadge.build`'s `if (!showDot && count ==
//! null) { return child; }`) — [`BadgeAlignment`] places the indicator at the
//! child's own top-left/top-center/top-right corner, nudged by
//! [`BadgeView::offset`] (default `(8, -6)`, `dx` ignored when centered).
//!
//! # Relationship to `IconButtonView::badge`
//!
//! [`mod@crate::icon_button`] already carries its own inline badge slot
//! (`IconButtonView::badge`, `BadgeValue`) — that port is deliberately
//! **not** rebased onto this module: `IconButtonValue::Count` auto-shows an
//! unlabeled dot at `0` and clamps to `0..=999_999` with no overflow-`"+"`
//! rule, positions purely decoratively at the icon button's own visual-box
//! corner with no reserved layout space, and was ported from a *different*
//! upstream file (`icon_button_m3e`'s `_buildBadge`, not `m3e_badges.dart`).
//! This module's [`resolve`] has different precedence (`showDot` always wins
//! over `count`, never auto-triggered by a zero count), a `maxCount`
//! overflow rule, and reserves real layout space for the indicator (the
//! wrapper's own reported size covers both boxes — see
//! [`BadgeWidget::layout`]) — the two badge shapes are siblings, not one
//! generalizing the other, so `icon_button` keeps its own copy rather than
//! composing this one.
//!
//! # Composition
//!
//! Unlike [`mod@crate::divider`], this component has no baseline
//! `frust-widgets` counterpart to build over — [`BadgeWidget`] hand-rolls its
//! own two-region layout (content + indicator), mirroring the reference's
//! `RenderM3EBadgeLayout::performLayout` exactly (union-of-boxes sizing, a
//! `shift` clamped to what the (possibly constrained) final size can absorb
//! — see that method's own `_shift` doc for why a tight parent still keeps
//! content inside the box).
//!
//! # Color roles: `MaterialTokens`-adjacent, not extension-scoped
//!
//! `errorContainer`/`onErrorContainer` (the reference's `containerColor`/
//! `labelColor`) are baseline `ColorScheme` roles (`Theme::scheme()`), not
//! two of the nine [`crate::MaterialTokens`]-only semantic roles — see
//! `crate::tokens`'s module docs' role-partition table. This module never
//! touches `MaterialTokens` or `frust::StatusPalette`; it resolves the plain
//! `ColorScheme` roles a themed `Theme` always carries.

use frust::Theme;
use frust::authoring::text::{FontWeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Rect, Size, Vec2};
use peniko::{Brush, Color};

// ---- Theme-token constants (m3e_badge_theme.dart's `M3EBadgeTheme.defaults`) --

/// The dot indicator's diameter, in logical px (`dotSize` default,
/// `m3e_badge_theme.dart:10`).
const DOT_SIZE: f64 = 8.0;
/// The numeric label pill's minimum width *and* height, in logical px
/// (`labelMinSize` default).
const LABEL_MIN_SIZE: f64 = 18.0;
/// Horizontal padding inside the label pill around its text, in logical px
/// (`labelHorizontalPadding` default).
const LABEL_H_PAD: f64 = 6.0;
/// Vertical padding inside the label pill around its text, in logical px
/// (`labelVerticalPadding` default).
const LABEL_V_PAD: f64 = 2.0;
/// The label pill's corner radius, in logical px (`labelCornerRadius`
/// default) — a fixed radius, not `height / 2.0`
/// ([`mod@crate::icon_button`]'s own badge slot uses the latter; this port
/// follows the reference's own token instead, see the [module docs](self)).
const LABEL_CORNER_RADIUS: f64 = 10.0;
/// The label text's font size, in logical px (`labelFontSize` default).
const LABEL_FONT_SIZE: f32 = 10.0;
/// The nudge away from the anchored top edge unless overridden via
/// [`BadgeView::offset`] — `dx` is ignored when [`BadgeAlignment::TopCenter`]
/// (`defaultOffset` default, `Offset(8, -6)`).
const DEFAULT_OFFSET: Vec2 = Vec2::new(8.0, -6.0);
/// The overflow ceiling before [`resolve`]'s numeric label switches to
/// `"{max}+"`, unless overridden via [`BadgeView::max_count`] (`M3EBadge`'s
/// own `maxCount` default).
const DEFAULT_MAX_COUNT: u32 = 99;

/// Unthemed-fallback badge container fill (a theme resolves this from
/// `colors.error_container`) — the M3 baseline light `errorContainer` tone.
const FALLBACK_CONTAINER: Color = Color::from_rgb8(0xF9, 0xDE, 0xDC);
/// Unthemed-fallback badge dot/label content color (a theme resolves this
/// from `colors.on_error_container`).
const FALLBACK_LABEL: Color = Color::from_rgb8(0x41, 0x0E, 0x0B);

/// Where the indicator anchors on `child`'s own top edge — the reference's
/// `M3EBadgeAlignment`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BadgeAlignment {
    /// The child's top-left corner.
    TopLeft,
    /// Horizontally centered on the child's top edge (`offset.dx` ignored).
    TopCenter,
    /// The child's top-right corner. Default.
    #[default]
    TopRight,
}

/// `count` formatted against `max` — the reference's `_format`
/// (`value > max ? '$max+' : '$value'`, `m3e_badges.dart:137`).
fn format_count(count: u32, max: u32) -> String {
    if count > max {
        format!("{max}+")
    } else {
        count.to_string()
    }
}

/// What [`BadgeView::dot`]/[`BadgeView::count`] resolve to for one layout/
/// paint pass — the union of `M3EBadge.build`'s three outcomes (no badge /
/// a bare dot / a labeled pill). `showDot` always wins over `count` when
/// both are set (`showDot ? _dot(...) : _label(...)`, `m3e_badges.dart:75`).
#[derive(Clone, Debug, PartialEq)]
enum ResolvedBadge {
    /// Neither [`BadgeView::dot`] nor [`BadgeView::count`] was called — the
    /// child paints exactly as given, no reserved indicator space.
    None,
    Dot,
    Label(String),
}

fn resolve(show_dot: bool, count: Option<u32>, max_count: u32) -> ResolvedBadge {
    match (show_dot, count) {
        (false, None) => ResolvedBadge::None,
        (true, _) => ResolvedBadge::Dot,
        (false, Some(count)) => ResolvedBadge::Label(format_count(count, max_count)),
    }
}

/// The numeric label's resolved style. Its 10px SemiBold is the badge
/// theme's own `labelFontSize`, not a type-scale role, so size and weight stay
/// literal; the family is borrowed from the live theme's `labelSmall` — the
/// role nearest that size — at layout (unthemed: the platform system UI
/// family), so a theme swap or a font picker restyles the label along with
/// the rest of the catalog.
fn label_style(theme: Option<&Theme>) -> TextStyle {
    let mut style = TextStyle {
        weight: FontWeight::SEMI_BOLD,
        ..TextStyle::new(LABEL_FONT_SIZE, Color::BLACK)
    };
    if let Some(theme) = theme {
        style.family = theme.type_scale.label_small.family.clone();
    }
    style
}

/// A small, paint-time-rebrushed text run for the numeric label — mirrors
/// [`mod@crate::icon_button`]'s own `BadgeRun` (`pub(super)` to that module
/// and out of reach here — see the [module docs](self)' Relationship
/// section — so this is a parallel, badge-scoped copy of the same idiom:
/// lazily shaped, re-brushed at paint time so the ink can change
/// independently of the cached shaping). The cache is keyed on the content
/// *and* the resolved style, so a theme swap that changes the family reshapes
/// instead of serving the old face.
struct LabelRun {
    content: String,
    /// The style the cached layout was shaped with.
    style: TextStyle,
    layout: Option<TextLayout>,
}

impl LabelRun {
    fn new() -> Self {
        Self {
            content: String::new(),
            style: label_style(None),
            layout: None,
        }
    }

    fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.layout = None;
        }
    }

    /// Shape (or reuse) the run in `style`, returning its measured size.
    fn shape(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if self.style != *style {
            self.style = style.clone();
            self.layout = None;
        }
        if let Some(layout) = &self.layout {
            return layout.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, &self.style, None);
        let size = laid.size();
        self.layout = Some(laid);
        size
    }

    fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, TextLayout::size)
    }

    /// Paint the run at `origin` in `color`, overriding the shaping ink.
    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let Some(layout) = &self.layout else {
            return;
        };
        for mut run in layout.to_scene_runs(origin) {
            run.brush = Brush::Solid(color);
            scene.draw_glyph_run(run);
        }
    }
}

/// A declarative badge wrapping any child view. See the [module docs](self).
pub struct BadgeView<State: 'static> {
    child: AnyView<State>,
    show_dot: bool,
    count: Option<u32>,
    max_count: u32,
    alignment: BadgeAlignment,
    offset: Option<Vec2>,
    background_color: Option<Color>,
    foreground_color: Option<Color>,
    semantic_label: Option<String>,
}

/// Wrap `child` with a badge slot — no indicator paints until
/// [`BadgeView::dot`] or [`BadgeView::count`] is chained on. See the
/// [module docs](self).
pub fn badge<State: 'static, V: View<State>>(child: V) -> BadgeView<State> {
    BadgeView {
        child: any(child),
        show_dot: false,
        count: None,
        max_count: DEFAULT_MAX_COUNT,
        alignment: BadgeAlignment::default(),
        offset: None,
        background_color: None,
        foreground_color: None,
        semantic_label: None,
    }
}

impl<State: 'static> BadgeView<State> {
    /// Show an unlabeled dot — wins over [`Self::count`] if both are set.
    pub fn dot(mut self) -> Self {
        self.show_dot = true;
        self
    }

    /// Show a numeric label, formatted against [`Self::max_count`] (default
    /// [`DEFAULT_MAX_COUNT`], 99 — e.g. `150` paints `"99+"`).
    pub fn count(mut self, count: u32) -> Self {
        self.count = Some(count);
        self
    }

    /// Override the overflow ceiling [`Self::count`] formats against.
    /// [`DEFAULT_MAX_COUNT`] (99) unless set.
    pub fn max_count(mut self, max_count: u32) -> Self {
        self.max_count = max_count;
        self
    }

    /// Anchor the indicator at a different top-edge corner.
    /// [`BadgeAlignment::TopRight`] by default.
    pub fn alignment(mut self, alignment: BadgeAlignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Override the nudge away from the anchored edge. [`DEFAULT_OFFSET`]
    /// (`(8, -6)`) unless set; `dx` is ignored when
    /// [`BadgeAlignment::TopCenter`].
    pub fn offset(mut self, offset: Vec2) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Override the theme-resolved `errorContainer` container fill.
    pub fn background_color(mut self, color: Color) -> Self {
        self.background_color = Some(color);
        self
    }

    /// Override the theme-resolved `onErrorContainer` label/dot ink.
    pub fn foreground_color(mut self, color: Color) -> Self {
        self.foreground_color = Some(color);
        self
    }

    /// Override the indicator's accessibility label. Defaults to
    /// `"Notifications: {count}"` for a numeric badge or `"Notifications"`
    /// for a dot (the reference's own default label text).
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }
}

/// The retained widget for a [`BadgeView`]. See the [module docs](self).
pub struct BadgeWidget {
    content: ChildPod,
    resolved: ResolvedBadge,
    alignment: BadgeAlignment,
    offset: Option<Vec2>,
    background_color: Option<Color>,
    foreground_color: Option<Color>,
    semantic_label: Option<String>,
    count: Option<u32>,
    label_run: LabelRun,
    indicator_size: Size,
    indicator_offset: Point,
}

impl<State: 'static> View<State> for BadgeView<State> {
    type Element = BadgeWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> BadgeWidget {
        BadgeWidget {
            content: frust::authoring::build_child(&self.child, ctx),
            resolved: resolve(self.show_dot, self.count, self.max_count),
            alignment: self.alignment,
            offset: self.offset,
            background_color: self.background_color,
            foreground_color: self.foreground_color,
            semantic_label: self.semantic_label.clone(),
            count: self.count,
            label_run: LabelRun::new(),
            indicator_size: Size::ZERO,
            indicator_offset: Point::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BadgeWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        let next_resolved = resolve(self.show_dot, self.count, self.max_count);
        if element.resolved != next_resolved {
            element.resolved = next_resolved;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.alignment != self.alignment || element.offset != self.offset {
            element.alignment = self.alignment;
            element.offset = self.offset;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.background_color != self.background_color
            || element.foreground_color != self.foreground_color
        {
            element.background_color = self.background_color;
            element.foreground_color = self.foreground_color;
            flags |= ChangeFlags::PAINT;
        }
        element.semantic_label = self.semantic_label.clone();
        element.count = self.count;

        flags |=
            frust::authoring::rebuild_child(&prev.child, &self.child, &mut element.content, ctx);
        flags
    }

    fn teardown(&self, element: &mut BadgeWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.child, &mut element.content, ctx);
    }
}

impl Widget for BadgeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let content_size = self.content.layout_child(ctx, bc);

        let ResolvedBadge::None = self.resolved else {
            self.indicator_size = match &self.resolved {
                ResolvedBadge::None => unreachable!("handled by the let-else arm above"),
                ResolvedBadge::Dot => Size::new(DOT_SIZE, DOT_SIZE),
                ResolvedBadge::Label(text) => {
                    self.label_run.set_content(text);
                    let style = label_style(Theme::from_layout_ctx(ctx));
                    let text_size = self.label_run.shape(ctx, &style);
                    Size::new(
                        (text_size.width + LABEL_H_PAD * 2.0).max(LABEL_MIN_SIZE),
                        (text_size.height + LABEL_V_PAD * 2.0).max(LABEL_MIN_SIZE),
                    )
                }
            };

            let offset = self.offset.unwrap_or(DEFAULT_OFFSET);
            let dx = match self.alignment {
                BadgeAlignment::TopLeft => -offset.x,
                BadgeAlignment::TopCenter => (content_size.width - self.indicator_size.width) / 2.0,
                BadgeAlignment::TopRight => {
                    content_size.width - self.indicator_size.width + offset.x
                }
            };
            let indicator_origin = Point::new(dx, offset.y);

            let content_rect = Rect::from_origin_size(Point::ZERO, content_size);
            let indicator_rect = Rect::from_origin_size(indicator_origin, self.indicator_size);
            let union = content_rect.union(indicator_rect);

            let size = bc.constrain(union.size());

            // Only shift by what the (possibly constrained) size can absorb,
            // so a tight parent still keeps content inside the box —
            // `RenderM3EBadgeLayout::_shift`.
            let shift_axis = |wanted: f64, available: f64| {
                if wanted <= 0.0 || available <= 0.0 {
                    0.0
                } else {
                    wanted.min(available)
                }
            };
            let shift = Vec2::new(
                shift_axis(-union.x0, size.width - content_size.width),
                shift_axis(-union.y0, size.height - content_size.height),
            );

            self.content.set_origin(Point::ZERO + shift);
            self.indicator_offset = indicator_origin + shift;

            return size;
        };

        // No badge: transparent passthrough, no reserved indicator space.
        self.content.set_origin(Point::ZERO);
        bc.constrain(content_size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.content.paint_child(ctx, scene);

        let theme = Theme::from_paint_ctx(ctx);
        let origin = ctx.origin() + self.indicator_offset.to_vec2();
        match &self.resolved {
            ResolvedBadge::None => {}
            ResolvedBadge::Dot => {
                let bg = self
                    .background_color
                    .unwrap_or_else(|| resolve_container(theme));
                scene.fill_rounded_rect(
                    origin,
                    self.indicator_size,
                    self.indicator_size.width / 2.0,
                    bg,
                );
            }
            ResolvedBadge::Label(_) => {
                let bg = self
                    .background_color
                    .unwrap_or_else(|| resolve_container(theme));
                let fg = self
                    .foreground_color
                    .unwrap_or_else(|| resolve_label(theme));
                scene.fill_rounded_rect(origin, self.indicator_size, LABEL_CORNER_RADIUS, bg);
                let text_size = self.label_run.size();
                let text_origin = Point::new(
                    origin.x + (self.indicator_size.width - text_size.width) / 2.0,
                    origin.y + (self.indicator_size.height - text_size.height) / 2.0,
                );
                self.label_run.paint(text_origin, fg, scene);
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event_single(&mut self.content, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if !matches!(self.resolved, ResolvedBadge::None) {
            ctx.push_node(Role::Label, |node| {
                let label = self
                    .semantic_label
                    .clone()
                    .unwrap_or_else(|| match self.count {
                        Some(count) => format!("Notifications: {count}"),
                        None => "Notifications".to_string(),
                    });
                node.set_value(label.as_str());
            });
        }
        self.content.semantics_child(ctx);
    }

    frust::authoring::visit_children!(content);
}

/// The resolved container fill. Themed: `colors.error_container`. Unthemed:
/// [`FALLBACK_CONTAINER`] exactly.
fn resolve_container(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().error_container,
        None => FALLBACK_CONTAINER,
    }
}

/// The resolved dot/label content color. Themed: `colors.on_error_container`.
/// Unthemed: [`FALLBACK_LABEL`] exactly.
fn resolve_label(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_error_container,
        None => FALLBACK_LABEL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::FontFamily;
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;

    fn build<S: 'static>(view: &BadgeView<S>) -> BadgeWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    // ---- format_count -----------------------------------------------------

    #[test]
    fn format_count_shows_the_exact_value_at_or_under_the_max() {
        assert_eq!(format_count(0, 99), "0");
        assert_eq!(format_count(99, 99), "99");
    }

    #[test]
    fn format_count_switches_to_overflow_past_the_max() {
        assert_eq!(format_count(100, 99), "99+");
        assert_eq!(format_count(150, 99), "99+");
    }

    #[test]
    fn format_count_honors_a_custom_max() {
        assert_eq!(format_count(999, 999), "999");
        assert_eq!(format_count(1000, 999), "999+");
    }

    // ---- resolve ------------------------------------------------------------

    #[test]
    fn resolve_is_none_with_neither_dot_nor_count() {
        assert_eq!(resolve(false, None, 99), ResolvedBadge::None);
    }

    #[test]
    fn resolve_dot_wins_over_count() {
        assert_eq!(resolve(true, Some(3), 99), ResolvedBadge::Dot);
    }

    #[test]
    fn resolve_count_without_dot_is_a_label() {
        assert_eq!(
            resolve(false, Some(3), 99),
            ResolvedBadge::Label("3".to_string())
        );
    }

    // ---- Layout: no badge is a transparent passthrough ---------------------

    #[test]
    fn no_badge_reports_the_child_s_own_size_with_zero_origin() {
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0));
        let mut w = build(&view);
        let mut ctx = LayoutCtx::new();
        let size = w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert_eq!(size, Size::new(40.0, 40.0));
        assert_eq!(w.content.origin(), Point::ZERO);
    }

    // ---- Layout: dot geometry -----------------------------------------------

    #[test]
    fn dot_badge_reserves_room_for_the_indicator() {
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0)).dot();
        let mut w = build(&view);
        let mut ctx = LayoutCtx::new();
        let size = w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        // Default offset (8, -6), top-right: indicator origin
        // = (40 - 8 + 8, -6) = (40, -6); union with (0,0)-(40,40) covers
        // x: 0..48, y: -6..40 -> size 48x46.
        assert_eq!(size, Size::new(48.0, 46.0));
    }

    #[test]
    fn dot_badge_shifts_content_to_keep_it_inside_the_reported_box() {
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0)).dot();
        let mut w = build(&view);
        let mut ctx = LayoutCtx::new();
        w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        // The indicator pokes 6px above the content box (offset.dy = -6),
        // so content shifts down by 6 to stay inside the union.
        assert_eq!(w.content.origin(), Point::new(0.0, 6.0));
    }

    // ---- Layout: 3 alignments -----------------------------------------------

    #[test]
    fn top_right_places_the_indicator_past_the_child_s_right_edge() {
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0)).dot();
        let mut w = build(&view);
        let mut ctx = LayoutCtx::new();
        w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert_eq!(w.indicator_offset.x, 40.0 - 8.0 + 8.0);
    }

    #[test]
    fn top_left_places_the_indicator_left_of_the_shifted_content() {
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0))
            .dot()
            .alignment(BadgeAlignment::TopLeft);
        let mut w = build(&view);
        let mut ctx = LayoutCtx::new();
        w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        // Indicator origin before the box-absorbing shift: -offset.x = -8;
        // content and indicator both shift right by 8 to keep everything
        // inside the reported box (`RenderM3EBadgeLayout::_shift`), so the
        // indicator ends up flush with the box's left edge while content
        // sits 8px in — the same 8px gap `offset.dx` requested.
        assert_eq!(w.indicator_offset.x, 0.0);
        assert_eq!(w.content.origin().x, 8.0);
    }

    #[test]
    fn top_center_ignores_offset_dx_and_centers_on_the_child() {
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0))
            .dot()
            .alignment(BadgeAlignment::TopCenter);
        let mut w = build(&view);
        let mut ctx = LayoutCtx::new();
        w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        // Indicator centered on the child's own width before shifting:
        // (40 - 8) / 2 = 16; content never shifts on x for a centered dot
        // whose left edge (16) is already >= 0.
        assert_eq!(w.indicator_offset.x, 16.0);
    }

    // ---- Layout: numeric label geometry --------------------------------------

    #[test]
    fn numeric_label_is_at_least_the_theme_s_minimum_pill_size() {
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0)).count(3);
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert!(w.indicator_size.width >= LABEL_MIN_SIZE);
        assert!(w.indicator_size.height >= LABEL_MIN_SIZE);
    }

    #[test]
    fn overflowing_count_paints_the_max_plus_label() {
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0)).count(150);
        let mut w = build(&view);
        assert_eq!(
            resolve(false, Some(150), DEFAULT_MAX_COUNT),
            ResolvedBadge::Label("99+".to_string())
        );
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert_eq!(w.resolved, ResolvedBadge::Label("99+".to_string()));
    }

    // ---- Paint ---------------------------------------------------------------

    #[derive(Default)]
    struct Recorder {
        rounded_rects: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rounded_rects.push((origin, size, radius, color));
        }
    }

    #[test]
    fn no_badge_paints_no_indicator() {
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0));
        let mut w = build(&view);
        let mut ctx = LayoutCtx::new();
        let size = w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);
        assert!(rec.rounded_rects.is_empty());
    }

    #[test]
    fn dot_paints_a_full_circle_radius_with_the_unthemed_fallback() {
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0)).dot();
        let mut w = build(&view);
        let mut ctx = LayoutCtx::new();
        let size = w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);
        assert_eq!(rec.rounded_rects.len(), 1);
        let (origin, indicator_size, radius, color) = rec.rounded_rects[0];
        assert_eq!(indicator_size, Size::new(DOT_SIZE, DOT_SIZE));
        assert_eq!(radius, DOT_SIZE / 2.0);
        assert_eq!(color, FALLBACK_CONTAINER);
        assert_eq!(origin, w.indicator_offset);
    }

    // ---- Typeface: the label's family follows the live theme ----------------

    /// A baseline theme whose `labelSmall` role names `family`.
    fn theme_with_label_small(family: FontFamily) -> Theme {
        let mut theme = crate::baseline();
        theme.type_scale.label_small.family = family;
        theme
    }

    /// Lay `w` out against `tcx`, threading `theme` the way the render root
    /// does.
    fn layout_themed(w: &mut BadgeWidget, tcx: &mut TextContext, theme: Option<&Theme>) {
        let mut ctx =
            LayoutCtx::with_resources(Some(tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
    }

    #[test]
    fn layout_takes_the_label_family_from_label_small() {
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0)).count(3);
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let probe = FontFamily::named("Badge Role Probe");
        layout_themed(
            &mut w,
            &mut tcx,
            Some(&theme_with_label_small(probe.clone())),
        );
        assert_eq!(w.label_run.style.family, probe);
        // Only the family is themed: the badge's own 10px SemiBold stays.
        assert_eq!(w.label_run.style.size, LABEL_FONT_SIZE);
        assert_eq!(w.label_run.style.weight, FontWeight::SEMI_BOLD);
    }

    #[test]
    fn without_a_theme_the_label_keeps_the_unthemed_style() {
        let unthemed = TextStyle {
            weight: FontWeight::SEMI_BOLD,
            ..TextStyle::new(LABEL_FONT_SIZE, Color::BLACK)
        };
        assert_eq!(label_style(None), unthemed);
        assert_eq!(unthemed.family, FontFamily::SystemUi);

        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0)).count(3);
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        layout_themed(&mut w, &mut tcx, None);
        assert_eq!(w.label_run.style, unthemed);
    }

    #[test]
    fn a_theme_swap_reshapes_the_cached_label() {
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0)).count(3);
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let first = theme_with_label_small(FontFamily::named("Badge Swap Probe A"));
        layout_themed(&mut w, &mut tcx, Some(&first));

        // Control: the same theme again reuses the cached run outright.
        let settled = tcx.shape_cache_stats();
        layout_themed(&mut w, &mut tcx, Some(&first));
        assert_eq!(
            tcx.shape_cache_stats(),
            settled,
            "an unchanged theme reshapes nothing"
        );

        let second = theme_with_label_small(FontFamily::named("Badge Swap Probe B"));
        layout_themed(&mut w, &mut tcx, Some(&second));
        assert!(
            tcx.shape_cache_stats().shapes > settled.shapes,
            "a family swap must reshape the label, not serve the old face"
        );
    }

    /// The first glyph run's font bytes — the face the label actually shaped in.
    #[cfg(feature = "bundled-fonts")]
    fn shaped_font(w: &BadgeWidget) -> Vec<u8> {
        let layout = w.label_run.layout.as_ref().expect("the label was shaped");
        let runs = layout.to_scene_runs(Point::ZERO);
        let run = runs.first().expect("the label shaped to at least one run");
        run.font.font().data.as_ref().to_vec()
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn a_theme_swap_between_registered_families_changes_the_shaped_face() {
        // `font_data()`'s documented order: Roboto Flex, then Roboto Mono.
        let (flex, mono) = (crate::tokens::font_data()[0], crate::tokens::font_data()[1]);
        let mut tcx = TextContext::new();
        for bytes in [flex, mono] {
            tcx.register_fonts(bytes.to_vec())
                .expect("the bundled faces must register");
        }
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0)).count(3);
        let mut w = build(&view);

        let flex_theme =
            theme_with_label_small(FontFamily::named(crate::tokens::ROBOTO_FLEX_FAMILY));
        layout_themed(&mut w, &mut tcx, Some(&flex_theme));
        assert!(
            shaped_font(&w) == flex,
            "labelSmall's family shapes the label"
        );

        let mono_theme =
            theme_with_label_small(FontFamily::named(crate::tokens::ROBOTO_MONO_FAMILY));
        layout_themed(&mut w, &mut tcx, Some(&mono_theme));
        assert!(
            shaped_font(&w) == mono,
            "after a theme swap the label must shape in the new family"
        );
    }

    #[test]
    fn explicit_colors_win_over_the_unthemed_fallback() {
        let bg = Color::from_rgb8(0x11, 0x22, 0x33);
        let view: BadgeView<()> = badge(leaf_any(40.0, 40.0)).dot().background_color(bg);
        let mut w = build(&view);
        let mut ctx = LayoutCtx::new();
        let size = w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);
        assert_eq!(rec.rounded_rects[0].3, bg);
    }
}
