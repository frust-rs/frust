//! `CupertinoNavBar`: the iOS top navigation
//! bar — a 44pt content-height bar with an absolutely-centered title, optional
//! leading/trailing slots hugging the edges, and a hairline bottom separator.
//!
//! `CupertinoNavBarView`/`CupertinoNavBarWidget` follow the same
//! widget-authoring recipe as a Material `appbar`: the `leading`/
//! `trailing` slots are opaque caller-supplied [`AnyView`] children routed
//! through [`ChildPod`]s (tint is the caller's own responsibility — this widget
//! has no icon primitive to tint), while the **title** is the one child this
//! widget fully owns: a child [`frust::text`] styled at the iOS *Headline*
//! type-role (17pt Semibold) and themed `on_surface` (iOS `label`), with its
//! FAMILY opted into the theme's `titleLarge` role
//! ([`frust::authoring::ThemeTextType::TitleLarge`]), so it participates in
//! a live theme swap through `Text`'s own layout-time color *and* family
//! resolution.
//!
//! Unlike a Material `appbar`'s left-aligned title, the iOS title is
//! **absolutely centered** in the bar (not centered in the space *between* the
//! slots) — the platform convention. A title wide enough to collide with a slot
//! is clamped to the available centered width.
//!
//! # Background: Liquid Glass
//!
//! The bar background is read from the theme's `bar`-tier glass token
//! (`Theme.glass.bar`, `frust_theme::glass`), not a hardcoded fill. On the
//! Cupertino design language that tier is a translucent Liquid-Glass lens
//! ([`crate::ios27`]'s `r=45` recipe: over-light `white a=0.07` + `white
//! a=0.03`); the bar composites that wash stack over the live content behind
//! it (no real backdrop blur yet), then draws a **specular hairline** along
//! its content-facing bottom edge whose alpha is the tier's `hairline_alpha`,
//! plus the tier's drop shadow (the `bar` tier's is zero, so effectively
//! none). The wash stack for the active brightness (over-light vs over-dark)
//! is chosen from the token.
//!
//! The navbar keeps its full-width, square, docked geometry — only the
//! *background* becomes glass (the floating-pill idiom is the tab bar's). On
//! the **Material** design language (or a bare-core/unthemed bar) the
//! same code takes the opaque path instead: `glass.bar` is opaque
//! ([`frust::GlassScale::opaque_material`]),
//! so the bar paints an opaque `surface` fill + an `outline_variant` separator
//! hairline — the pre-glass look, unchanged.
//!
//! # Semantics
//!
//! The whole bar is one [`Role::TitleBar`] container node labelled with the
//! title text, whose children are the leading slot, the title, and the trailing
//! slot (in that order).

use frust::authoring::Role;
use frust::authoring::text::{FontWeight, LineHeight};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, ThemeTextType, View, Widget,
};
use frust::{Brightness, GlassFill, GlassMaterial, Theme, text};
use kurbo::{Point, Size};
use peniko::Color;

/// Content height of the iOS navigation bar, in logical px (source: Apple HIG —
/// the standard navigation bar is 44pt tall, excluding the status-bar inset a
/// shell adds).
const HEIGHT: f64 = 44.0;
/// Horizontal inset from the bar edges to the leading/trailing slots, in
/// logical px (iOS standard layout margin).
const PAD_X: f64 = 16.0;

/// The title's iOS *Headline* type-role: 17pt Semibold (source:
/// `crate::tokens::type_scale` — Headline maps to SF 17pt Semibold).
/// Size/weight stay hardcoded here rather than read from a live
/// `Theme::type_scale` for the same reason a Material `appbar`'s title
/// tokens are: `Text` defers *color* resolution, and an opt-in family
/// resolution, past `View::build`, never size/weight — [`title_view`] opts
/// the FAMILY into `ThemeTextType::TitleLarge` (`titleLarge` matches this
/// slot's own 17pt/Semibold exactly), so only the family, not these two
/// constants, follows a live theme.
const TITLE_SIZE: f32 = 17.0;
const TITLE_LINE_HEIGHT: f32 = 22.0;
const TITLE_WEIGHT: FontWeight = FontWeight::SEMI_BOLD;

/// The hairline separator's stroke width, in logical px.
///
/// **Community-approximate**: a 1px "hairline" is device-pixel-thin on iOS
/// (0.5pt at 2×); Frust paints a 1px logical hairline as the closest
/// backend-agnostic approximation.
const HAIRLINE_W: f64 = 1.0;

/// Unthemed fallback bar fill (a theme resolves this from `colors.surface`,
/// iOS systemBackground).
const CONTAINER: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
/// Unthemed fallback hairline color (a theme resolves this from
/// `colors.outline_variant`, iOS `separator`).
///
/// **Composite-derived** (re-derived for the 2026-07-18 kit refresh — see
/// `frust-theme::color`'s module docs): the themed `outline_variant` token
/// is now a flat `rgba(0,0,0,0.12)` overlay (light; `rgba(255,255,255,0.12)`
/// dark), a changed token from the pre-refresh translucent
/// `rgba(60,60,67,0.29)` this constant used to approximate (as `#C6C6C8`).
/// This fallback composites the new token's alpha over white the same way,
/// so an unthemed bar's hairline stays visually consistent with a themed
/// one: `0.12 × black + 0.88 × white = 0.88 × 255 ≈ 224 = 0xE0` per channel,
/// i.e. `#E0E0E0`.
const SEPARATOR: Color = Color::from_rgb8(0xE0, 0xE0, 0xE0);

/// The specular-highlight color of a glass edge. The glass recipe
/// (`frust_theme::glass`) stores only the hairline's *alpha* per tier
/// (`GlassMaterial::hairline_alpha`); white is the specular color the glass
/// module documents the consuming widget draws over light content. The
/// load-bearing value (alpha) is read from the token — this is the fixed
/// specular color, not a recipe rgba (no hardcoded recipe rgba lives in the
/// widget).
const SPECULAR: Color = Color::WHITE;

/// The resolved `(container, separator)` colors for the **opaque** path
/// (Material design language, or an unthemed bar). Themed: `colors.surface` /
/// `colors.outline_variant` (iOS systemBackground / separator). Unthemed: the
/// [`CONTAINER`]/[`SEPARATOR`] constants.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
    match theme {
        Some(theme) => (theme.scheme().surface, theme.scheme().outline_variant),
        None => (CONTAINER, SEPARATOR),
    }
}

/// The Liquid-Glass `bar`-tier material to paint from, or `None` when the bar
/// should take the **opaque** path instead: no theme (bare-core/pre-theme), or
/// a theme whose `glass.bar` is opaque (the Material design language — see
/// [`frust::GlassScale::opaque_material`]). This is the
/// single-API branch that keeps Material bars opaque while Cupertino bars read
/// glass.
fn glass_bar(theme: Option<&Theme>) -> Option<&GlassMaterial> {
    let material = &theme?.glass.bar;
    (!material.is_opaque()).then_some(material)
}

/// The wash stack to composite for the active `brightness` (over-light vs
/// over-dark content), bottom-to-top — read purely from the tier token.
fn glass_fills(material: &GlassMaterial, brightness: Brightness) -> &[GlassFill] {
    match brightness {
        Brightness::Light => &material.fills_light,
        Brightness::Dark => &material.fills_dark,
    }
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors a
/// Material `toolbar`'s helper of the same shape).
fn specular_from(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// `SPECULAR` white with its alpha replaced by the tier's `hairline_alpha`.
fn specular(alpha: f32) -> Color {
    specular_from(SPECULAR, alpha)
}

/// Build the title's type-erased child view: Headline-styled text, defaulting
/// to the `Text` widget's own `OnSurface` themed role (iOS `label`), family
/// opted into the theme's `titleLarge` role.
fn title_view<State: 'static>(title: String) -> AnyView<State> {
    frust::authoring::any::<State, _>(
        text(title)
            .size(TITLE_SIZE)
            .weight(TITLE_WEIGHT)
            .line_height(LineHeight::Absolute(TITLE_LINE_HEIGHT))
            .themed_family(ThemeTextType::TitleLarge),
    )
}

/// A declarative iOS top navigation bar. See the [module docs](self).
pub struct CupertinoNavBarView<State: 'static> {
    title: String,
    leading: Option<AnyView<State>>,
    trailing: Option<AnyView<State>>,
}

/// Create a nav bar titled `title`, with no leading/trailing slot (attach them
/// with [`CupertinoNavBarView::leading`]/[`CupertinoNavBarView::trailing`]).
pub fn cupertino_nav_bar<State: 'static>(title: impl Into<String>) -> CupertinoNavBarView<State> {
    CupertinoNavBarView {
        title: title.into(),
        leading: None,
        trailing: None,
    }
}

/// PascalCase alias for [`cupertino_nav_bar`].
#[allow(non_snake_case)]
pub fn CupertinoNavBar<State: 'static>(title: impl Into<String>) -> CupertinoNavBarView<State> {
    cupertino_nav_bar(title)
}

impl<State: 'static> CupertinoNavBarView<State> {
    /// Attach a leading slot (typically a back button), erased as an
    /// [`AnyView`]. Tint is the supplied view's own responsibility.
    pub fn leading(mut self, leading: impl View<State>) -> Self {
        self.leading = Some(AnyView::new(leading));
        self
    }

    /// Attach a trailing slot (typically an action button), erased as an
    /// [`AnyView`]. Tint is the supplied view's own responsibility.
    pub fn trailing(mut self, trailing: impl View<State>) -> Self {
        self.trailing = Some(AnyView::new(trailing));
        self
    }
}

/// Collect `view`'s leading (if any) then trailing (if any) into one ordered
/// slice of [`AnyView`] references — the shared shape both build/teardown and
/// [`frust::authoring::rebuild_children`] walk over.
fn interactive_views<State: 'static>(view: &CupertinoNavBarView<State>) -> Vec<&AnyView<State>> {
    let mut views = Vec::with_capacity(2);
    if let Some(leading) = &view.leading {
        views.push(leading);
    }
    if let Some(trailing) = &view.trailing {
        views.push(trailing);
    }
    views
}

/// The retained widget for a [`CupertinoNavBarView`].
pub struct CupertinoNavBarWidget {
    title: ChildPod,
    title_text: String,
    /// The leading slot (if any) then the trailing slot (if any), in that fixed
    /// order — the one list [`frust::authoring::route_event`] hit-tests and
    /// [`frust::authoring::rebuild_children`] reconciles.
    interactive: Vec<ChildPod>,
    has_leading: bool,
    has_trailing: bool,
}

impl<State: 'static> View<State> for CupertinoNavBarView<State> {
    type Element = CupertinoNavBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CupertinoNavBarWidget {
        let title_view = title_view::<State>(self.title.clone());
        let mut interactive = Vec::with_capacity(2);
        if let Some(leading) = &self.leading {
            interactive.push(frust::authoring::build_child(leading, ctx));
        }
        if let Some(trailing) = &self.trailing {
            interactive.push(frust::authoring::build_child(trailing, ctx));
        }
        CupertinoNavBarWidget {
            title: frust::authoring::build_child(&title_view, ctx),
            title_text: self.title.clone(),
            interactive,
            has_leading: self.leading.is_some(),
            has_trailing: self.trailing.is_some(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CupertinoNavBarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.title != self.title {
            element.title_text = self.title.clone();
            let prev_view = title_view::<State>(prev.title.clone());
            let next_view = title_view::<State>(self.title.clone());
            flags |=
                frust::authoring::rebuild_child(&prev_view, &next_view, &mut element.title, ctx);
        }
        let prev_views = interactive_views(prev);
        let next_views = interactive_views(self);
        flags |= frust::authoring::rebuild_children(
            &prev_views,
            &next_views,
            &mut element.interactive,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );
        if element.has_leading != self.leading.is_some()
            || element.has_trailing != self.trailing.is_some()
        {
            element.has_leading = self.leading.is_some();
            element.has_trailing = self.trailing.is_some();
            flags |= ChangeFlags::LAYOUT;
        }
        flags
    }

    fn teardown(&self, element: &mut CupertinoNavBarWidget, ctx: &mut BuildCtx<'_>) {
        let title_view = title_view::<State>(self.title.clone());
        frust::authoring::teardown_child(&title_view, &mut element.title, ctx);
        for (view, pod) in interactive_views(self)
            .into_iter()
            .zip(element.interactive.iter_mut())
        {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl CupertinoNavBarWidget {
    /// The index of the trailing pod in `interactive`, if a trailing slot
    /// exists (it follows the leading slot, when present).
    fn trailing_index(&self) -> Option<usize> {
        if !self.has_trailing {
            return None;
        }
        Some(if self.has_leading { 1 } else { 0 })
    }
}

impl Widget for CupertinoNavBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let slot_bc = BoxConstraints::loose(Size::new(f64::INFINITY, HEIGHT));

        // Leading hugs the left edge.
        let mut left_edge = PAD_X;
        if self.has_leading {
            let size = self.interactive[0].layout_child(ctx, &slot_bc);
            self.interactive[0].set_origin(Point::new(PAD_X, (HEIGHT - size.height) / 2.0));
            left_edge = PAD_X + size.width;
        }

        // Trailing hugs the right edge.
        let mut right_edge = width - PAD_X;
        if let Some(ti) = self.trailing_index() {
            let size = self.interactive[ti].layout_child(ctx, &slot_bc);
            let x = width - PAD_X - size.width;
            self.interactive[ti].set_origin(Point::new(x, (HEIGHT - size.height) / 2.0));
            right_edge = x;
        }

        // Title is absolutely centered, clamped to the width left free by the
        // slots (symmetrically — the larger of the two intrusions bounds it).
        let intrusion = (left_edge.max(width - right_edge)).max(PAD_X);
        let title_max_width = (width - 2.0 * intrusion).max(0.0);
        let title_size = self.title.layout_child(
            ctx,
            &BoxConstraints::loose(Size::new(title_max_width, HEIGHT)),
        );
        let title_x = (width - title_size.width) / 2.0;
        self.title
            .set_origin(Point::new(title_x, (HEIGHT - title_size.height) / 2.0));

        bc.constrain(Size::new(width, HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let origin = ctx.origin();
        let size = ctx.size();
        // Bottom edge (the content-facing edge of a top bar), where the
        // hairline/separator sits.
        let hairline_y = origin.y + size.height - HAIRLINE_W / 2.0;
        let hairline = |scene: &mut dyn PaintScene, color: Color| {
            scene.stroke_line(
                Point::new(origin.x, hairline_y),
                Point::new(origin.x + size.width, hairline_y),
                HAIRLINE_W,
                color,
            );
        };

        match glass_bar(theme) {
            // Cupertino Liquid Glass: layered translucent wash stack +
            // specular hairline (+ shadow spec), all read from the `bar` tier
            // token — the navbar keeps its full-width geometry, adopting only
            // the glass background/hairline.
            Some(material) => {
                let brightness = theme.map(|t| t.brightness).unwrap_or_default();
                // Drop shadow: the ios27 `bar` tier carries none
                // (`color_alpha == 0.0`), so this is a no-op there — drawn only
                // when the token asks for one.
                let shadow = material.shadow;
                if shadow.color_alpha > 0.0 {
                    let shadow_color = theme
                        .map(|t| specular_from(t.scheme().shadow, shadow.color_alpha))
                        .unwrap_or_else(|| specular_from(Color::BLACK, shadow.color_alpha));
                    scene.draw_shadow(
                        Point::new(origin.x, origin.y + shadow.y_offset),
                        size,
                        0.0,
                        shadow.blur_std_dev,
                        shadow_color,
                    );
                }
                for fill in glass_fills(material, brightness) {
                    scene.fill_rect(origin, size, fill.color);
                }
                hairline(scene, specular(material.hairline_alpha));
            }
            // Opaque path (Material design language, or an unthemed bar): the
            // pre-glass look — an opaque surface fill + a separator hairline.
            None => {
                let (fill, separator) = resolve_colors(theme);
                scene.fill_rect(origin, size, fill);
                hairline(scene, separator);
            }
        }

        for pod in &mut self.interactive {
            pod.paint_child(ctx, scene);
        }
        self.title.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event(&mut self.interactive, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let title = self.title_text.clone();
        let has_leading = self.has_leading;
        let trailing_index = self.trailing_index();
        let title_pod = &self.title;
        let interactive = &self.interactive;
        ctx.push_container(
            Role::TitleBar,
            |node| node.set_label(title.as_str()),
            |ctx| {
                if has_leading {
                    interactive[0].semantics_child(ctx);
                }
                title_pod.semantics_child(ctx);
                if let Some(ti) = trailing_index {
                    interactive[ti].semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(interactive, title);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;

    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    /// A recorder capturing filled rects (with color) and stroked lines — the
    /// shared `test_support::RecordingScene` drops color and lines.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        lines: Vec<(Point, Point, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn stroke_line(&mut self, p0: Point, p1: Point, width: f64, color: Color) {
            self.lines.push((p0, p1, width, color));
        }
    }

    fn build(view: &CupertinoNavBarView<()>) -> CupertinoNavBarWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut ctx(&mut counter))
    }

    fn layout(w: &mut CupertinoNavBarWidget, bc: &BoxConstraints) -> Size {
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        w.layout(&mut lctx, bc)
    }

    #[test]
    fn bar_is_44pt_tall() {
        let view: CupertinoNavBarView<()> = cupertino_nav_bar("Home");
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        assert_eq!(size, Size::new(400.0, HEIGHT));
    }

    #[test]
    fn title_is_absolutely_centered() {
        let view: CupertinoNavBarView<()> = cupertino_nav_bar("Title");
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        let title_center = w.title.origin().x + w.title.size().width / 2.0;
        assert!(
            (title_center - 200.0).abs() < 1e-6,
            "title centered at bar midpoint"
        );
    }

    #[test]
    fn leading_hugs_left_and_trailing_hugs_right() {
        let view: CupertinoNavBarView<()> = cupertino_nav_bar("Title")
            .leading(leaf_any(30.0, 30.0))
            .trailing(leaf_any(30.0, 30.0));
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        // interactive[0] = leading, interactive[1] = trailing.
        assert_eq!(w.interactive[0].origin().x, PAD_X);
        assert_eq!(
            w.interactive[1].origin().x + w.interactive[1].size().width,
            400.0 - PAD_X
        );
        // Title still centered even with both slots present.
        let title_center = w.title.origin().x + w.title.size().width / 2.0;
        assert!((title_center - 200.0).abs() < 1e-6);
    }

    #[test]
    fn trailing_only_lays_out_at_the_right_edge() {
        let view: CupertinoNavBarView<()> =
            cupertino_nav_bar("Title").trailing(leaf_any(30.0, 30.0));
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        // The single interactive pod is the trailing slot.
        assert_eq!(
            w.interactive[0].origin().x + w.interactive[0].size().width,
            400.0 - PAD_X
        );
        assert_eq!(w.trailing_index(), Some(0));
    }

    #[test]
    fn unthemed_paint_fills_container_and_draws_hairline() {
        let view: CupertinoNavBarView<()> = cupertino_nav_bar("Home");
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        let mut scene = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].1, Size::new(300.0, HEIGHT));
        assert_eq!(scene.lines.len(), 1, "one hairline separator");
        assert_eq!(scene.lines[0].3, SEPARATOR);
    }

    #[test]
    fn cupertino_theme_paints_the_glass_bar_wash_stack_and_specular_hairline() {
        // Default (light) Cupertino baseline: the `bar` tier is a glass lens,
        // so the bar composites the token's over-light wash stack (read purely
        // from `Theme.glass.bar.fills_light`) and draws a specular white
        // hairline whose alpha is the tier's `hairline_alpha`.
        let theme = crate::baseline();
        let material = &theme.glass.bar;
        assert!(
            !material.is_opaque(),
            "the cupertino bar tier is a glass lens"
        );

        let view: CupertinoNavBarView<()> = cupertino_nav_bar("Home");
        let mut w = build(&view);
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(&theme as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 100.0)));

        let mut scene = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);

        // One filled rect per wash in the (light) stack, in token order — the
        // fill stack reaches the paint layer.
        let washes: Vec<Color> = material.fills_light.iter().map(|f| f.color).collect();
        assert!(!washes.is_empty());
        let painted: Vec<Color> = scene.rects.iter().map(|(_, _, c)| *c).collect();
        assert_eq!(&painted[..washes.len()], &washes[..]);
        // The specular hairline is white at the tier's hairline_alpha.
        assert_eq!(scene.lines.len(), 1, "one specular hairline");
        assert_eq!(scene.lines[0].3, specular(material.hairline_alpha));
    }

    #[test]
    fn material_theme_keeps_the_bar_opaque() {
        // An M3 theme's `glass.bar` is opaque, so the same code
        // paints the opaque surface fill + outline_variant separator — the
        // pre-glass look, never the glass wash stack.
        let theme = Theme::neutral();
        assert!(theme.glass.bar.is_opaque());
        let view: CupertinoNavBarView<()> = cupertino_nav_bar("Home");
        let mut w = build(&view);
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(&theme as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        let mut scene = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects.len(), 1, "one opaque surface fill");
        assert_eq!(scene.rects[0].2, theme.scheme().surface);
        assert_eq!(scene.lines[0].3, theme.scheme().outline_variant);
    }

    #[test]
    fn rebuild_adopts_new_title() {
        let mut counter = 0u64;
        let prev: CupertinoNavBarView<()> = cupertino_nav_bar("Home");
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        let next: CupertinoNavBarView<()> = cupertino_nav_bar("Settings");
        View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.title_text, "Settings");
    }

    #[test]
    fn semantics_node_is_titlebar_labelled_with_the_title() {
        fn logic(_state: &mut ()) -> CupertinoNavBarView<()> {
            cupertino_nav_bar("Inbox")
        }
        let mut root: frust_core::RenderRoot<(), CupertinoNavBarView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(300.0, HEIGHT), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TitleBar)
            .expect("a TitleBar node is contributed");
        assert_eq!(node.label(), Some("Inbox"));
    }

    // --- Render-time typeface identity: the title's family follows the theme ---

    #[test]
    fn the_title_paints_in_the_themes_title_large_family() {
        use crate::tokens::typeface_probe::{TUFFY, assert_paints_only_in, baseline_with_family};
        let theme =
            baseline_with_family(|scale, family| scale.title_large.family = family, "Tuffy");
        assert_paints_only_in(
            "the nav bar title",
            |_: &mut ()| cupertino_nav_bar::<()>("Hello"),
            theme,
            &[TUFFY],
            Size::new(300.0, HEIGHT),
            TUFFY,
        );
    }

    #[test]
    fn the_title_follows_a_live_theme_family_change() {
        // A font picker rewriting the theme's `titleLarge` family must be
        // followed on the next layout, not stay pinned to whichever face
        // happened to register first.
        use crate::tokens::typeface_probe::{
            TUFFY, TUFFY_AS_HELVETICA, assert_paints_only_in, baseline_with_family,
        };
        let faces = [TUFFY, TUFFY_AS_HELVETICA];

        let tuffy_theme =
            baseline_with_family(|scale, family| scale.title_large.family = family, "Tuffy");
        assert_paints_only_in(
            "the nav bar title",
            |_: &mut ()| cupertino_nav_bar::<()>("Hello"),
            tuffy_theme,
            &faces,
            Size::new(300.0, HEIGHT),
            TUFFY,
        );

        let helvetica_theme = baseline_with_family(
            |scale, family| scale.title_large.family = family,
            "Helvetica",
        );
        assert_paints_only_in(
            "the nav bar title",
            |_: &mut ()| cupertino_nav_bar::<()>("Hello"),
            helvetica_theme,
            &faces,
            Size::new(300.0, HEIGHT),
            TUFFY_AS_HELVETICA,
        );
    }
}
