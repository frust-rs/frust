//! `CupertinoNavBar` (Phase 6c, PLAN.md D5, task 13): the iOS top navigation
//! bar — a 44pt content-height bar with an absolutely-centered title, optional
//! leading/trailing slots hugging the edges, and a hairline bottom separator.
//!
//! `CupertinoNavBarView`/`CupertinoNavBarWidget` follow the same
//! widget-authoring recipe as [`crate::material::appbar`]: the `leading`/
//! `trailing` slots are opaque caller-supplied [`AnyView`] children routed
//! through [`ChildPod`]s (tint is the caller's own responsibility — this widget
//! has no icon primitive to tint), while the **title** is the one child this
//! widget fully owns: a child [`crate::text`] styled at the iOS *Headline*
//! type-role (17pt Semibold) and themed `on_surface` (iOS `label`), so it
//! participates in a live theme swap through `Text`'s own layout-time color
//! resolution.
//!
//! Unlike [`crate::material::appbar`]'s left-aligned title, the iOS title is
//! **absolutely centered** in the bar (not centered in the space *between* the
//! slots) — the platform convention. A title wide enough to collide with a slot
//! is clamped to the available centered width.
//!
//! # Semantics
//!
//! The whole bar is one [`Role::TitleBar`] container node labelled with the
//! title text, whose children are the leading slot, the title, and the trailing
//! slot (in that order).

use forgekit_core::accesskit::Role;
use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget,
};
use forgekit_text::{FontWeight, LineHeight};
use forgekit_theme::Theme;
use kurbo::{Point, Size};
use peniko::Color;

use crate::text;

/// Content height of the iOS navigation bar, in logical px (source: Apple HIG —
/// the standard navigation bar is 44pt tall, excluding the status-bar inset a
/// shell adds).
const HEIGHT: f64 = 44.0;
/// Horizontal inset from the bar edges to the leading/trailing slots, in
/// logical px (iOS standard layout margin).
const PAD_X: f64 = 16.0;

/// The title's iOS *Headline* type-role: 17pt Semibold (source:
/// `forgekit-theme::typography`'s `TypeScale::cupertino` — Headline maps to SF
/// 17pt Semibold). Hardcoded here rather than read from a live
/// `Theme::type_scale` for the same reason [`crate::material::appbar`]'s title
/// tokens are: `Text` defers only *color* resolution past `View::build`, never
/// size/weight.
const TITLE_SIZE: f32 = 17.0;
const TITLE_LINE_HEIGHT: f32 = 22.0;
const TITLE_WEIGHT: FontWeight = FontWeight::SEMI_BOLD;

/// The hairline separator's stroke width, in logical px.
///
/// **Community-approximate**: a 1px "hairline" is device-pixel-thin on iOS
/// (0.5pt at 2×); ForgeKit paints a 1px logical hairline as the closest
/// backend-agnostic approximation.
const HAIRLINE_W: f64 = 1.0;

/// Unthemed fallback bar fill (a theme resolves this from `colors.surface`,
/// iOS systemBackground).
const CONTAINER: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
/// Unthemed fallback hairline color (a theme resolves this from
/// `colors.outline_variant`, iOS `separator`).
const SEPARATOR: Color = Color::from_rgb8(0xC6, 0xC6, 0xC8);

/// The resolved `(container, separator)` colors. Themed: `colors.surface` /
/// `colors.outline_variant` (iOS systemBackground / separator). Unthemed: the
/// [`CONTAINER`]/[`SEPARATOR`] constants.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
    match theme {
        Some(theme) => (theme.scheme().surface, theme.scheme().outline_variant),
        None => (CONTAINER, SEPARATOR),
    }
}

/// Build the title's type-erased child view: Headline-styled text, defaulting
/// to the `Text` widget's own `OnSurface` themed role (iOS `label`).
fn title_view<State: 'static>(title: String) -> AnyView<State> {
    forgekit_core::any::<State, _>(
        text(title)
            .size(TITLE_SIZE)
            .weight(TITLE_WEIGHT)
            .line_height(LineHeight::Absolute(TITLE_LINE_HEIGHT)),
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
    pub fn leading(mut self, leading: AnyView<State>) -> Self {
        self.leading = Some(leading);
        self
    }

    /// Attach a trailing slot (typically an action button), erased as an
    /// [`AnyView`]. Tint is the supplied view's own responsibility.
    pub fn trailing(mut self, trailing: AnyView<State>) -> Self {
        self.trailing = Some(trailing);
        self
    }
}

/// Collect `view`'s leading (if any) then trailing (if any) into one ordered
/// slice of [`AnyView`] references — the shared shape both build/teardown and
/// [`crate::rebuild_children`] walk over.
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
    /// order — the one list [`crate::route_event`] hit-tests and
    /// [`crate::rebuild_children`] reconciles.
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
            interactive.push(crate::build_child(leading, ctx));
        }
        if let Some(trailing) = &self.trailing {
            interactive.push(crate::build_child(trailing, ctx));
        }
        CupertinoNavBarWidget {
            title: crate::build_child(&title_view, ctx),
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
            flags |= crate::rebuild_child(&prev_view, &next_view, &mut element.title, ctx);
        }
        let prev_views = interactive_views(prev);
        let next_views = interactive_views(self);
        flags |= crate::rebuild_children(
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
        crate::teardown_child(&title_view, &mut element.title, ctx);
        for (view, pod) in interactive_views(self)
            .into_iter()
            .zip(element.interactive.iter_mut())
        {
            crate::teardown_child(view, pod, ctx);
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
        let (fill, separator) = resolve_colors(Theme::from_paint_ctx(ctx));
        let origin = ctx.origin();
        let size = ctx.size();
        scene.fill_rect(origin, size, fill);
        // Hairline along the bottom edge.
        let y = origin.y + size.height - HAIRLINE_W / 2.0;
        scene.stroke_line(
            Point::new(origin.x, y),
            Point::new(origin.x + size.width, y),
            HAIRLINE_W,
            separator,
        );
        for pod in &mut self.interactive {
            pod.paint_child(ctx, scene);
        }
        self.title.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::route_event(&mut self.interactive, ctx, event)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf_any;
    use forgekit_core::BuildCtx;
    use forgekit_text::TextContext;
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
        let mut tcx = TextContext::new();
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
    fn themed_paint_resolves_surface_and_separator() {
        let theme = Theme::cupertino_baseline();
        let view: CupertinoNavBarView<()> = cupertino_nav_bar("Home");
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(&theme as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 100.0)));

        let mut scene = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
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
        let mut root: forgekit_core::RenderRoot<(), CupertinoNavBarView<()>> =
            forgekit_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, HEIGHT), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TitleBar)
            .expect("a TitleBar node is contributed");
        assert_eq!(node.label(), Some("Inbox"));
    }
}
