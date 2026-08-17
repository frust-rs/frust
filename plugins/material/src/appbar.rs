//! The M3 top `AppBar`: small (64dp), center-aligned variant only —
//! medium/large flexible variants are deferred. Navigation icon tint resolves
//! to `onSurface`.
//!
//! `AppBarView`/`AppBarWidget` follow the widget-authoring recipe: a `leading`
//! slot (a nav icon, typically) and a `Vec<AnyView>` of trailing `actions` are
//! opaque caller-supplied children routed through `ChildPod`s exactly like
//! [`frust::Row`]'s children — this widget lays them out and routes events to
//! them, but does **not** paint or tint their content itself (the leading
//! slot's `onSurface`/actions' `onSurfaceVariant` tinting guidance is the
//! *caller's* responsibility to apply to whatever `AnyView` it supplies, e.g.
//! an icon button — this widget has no icon primitive to tint). The
//! **title** is the one child this widget fully owns: composed as a child
//! [`frust::TextView`] (no hand-shaped text), styled at the M3
//! `titleLarge` type-scale token and themed `onSurface` (the `Text` default
//! role), so it participates in a live theme swap through `Text`'s own
//! layout-time color resolution (see `docs/ARCHITECTURE.md`'s Theme
//! delivery). The title consumes the M3 Expressive **emphasized**
//! `titleLarge` sibling (`titleLargeEmphasized`; see
//! `frust-theme::typography`'s "Emphasized type scale" module docs) — same
//! size/line-height as the baseline token, weight stepped from Regular to
//! Medium.
//!
//! Layout is a fixed 64dp-tall row: the leading slot (if any) hugs the left
//! edge, actions hug the right edge in reading order, and the title fills the
//! remaining middle space, vertically centered throughout. The container
//! itself paints a `colors.surface`-filled background (the M3 "surface
//! container" role for a small/center-aligned app bar); no elevation shadow is
//! painted (FAB/Card own the elevation-token precedent).
//!
//! # Semantics
//!
//! The whole bar is one accesskit container node of [`Role::TitleBar`] (the
//! closest accesskit vocabulary to "a window/app title bar region" — chosen
//! over a generic `Role::GenericContainer` since accesskit publishes a
//! purpose-built role for exactly this UI region), labelled with the title
//! text, whose children are the leading slot, the title, and each action (in
//! that order) via [`frust::authoring::SemanticsCtx::push_container`].

use frust::authoring::Role;
use frust::authoring::text::{FontWeight, LineHeight};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget,
};
use frust::{Theme, text};
use kurbo::{Point, Size};
use peniko::Color;

/// Container height of the small/center-aligned Top App Bar, in logical px.
///
/// Source: m3.material.io/components/top-app-bars/specs (also
/// material-components-android `docs/components/TopAppBar.md`). This is the
/// *only* height this widget ships — the flexible medium (112/136dp)/large
/// (120/152dp) variants are deferred (not deprecated, just out of v1 scope
/// here).
const HEIGHT: f64 = 64.0;
/// Horizontal inset from the bar's leading/trailing edges to the
/// leading/action slots, in logical px.
const PAD_X: f64 = 4.0;
/// Gap between adjacent slots (leading↔title, title↔actions, action↔action),
/// in logical px.
const GAP: f64 = 4.0;

/// The title's M3 `titleLargeEmphasized` type-scale token, hardcoded here
/// rather than read from a live `Theme::type_scale` (unlike a themed
/// *color*, `Text` has no layout-time-deferred *size/weight* resolution
/// seam — see `docs/CODE_STANDARDS.md`'s Theming conventions; only a color
/// role can be resolved after `View::build`). Matches
/// `frust-theme::typography`'s `TITLE_LARGE_EMPHASIZED` token: same
/// size/line-height as the baseline `TITLE_LARGE` (m3.material.io, weight
/// 400 — not the contested 500 secondary-source claim), weight stepped up to
/// Medium (the emphasized-type consumption; was `REGULAR` previously).
const TITLE_SIZE: f32 = 22.0;
const TITLE_LINE_HEIGHT: f32 = 28.0;
const TITLE_WEIGHT: FontWeight = FontWeight::MEDIUM;

/// Unthemed fallback container fill (a theme resolves this from
/// `colors.surface`).
const CONTAINER: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);

/// The resolved container fill. Themed: `colors.surface`. Unthemed: the
/// [`CONTAINER`] constant exactly.
fn resolve_container(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().surface,
        None => CONTAINER,
    }
}

/// Build the title's type-erased child view: `titleLarge`-styled text,
/// defaulting to the `Text` widget's own `OnSurface` themed role (an app bar
/// title reads as ordinary on-surface content, no special role needed).
fn title_view<State: 'static>(title: String) -> AnyView<State> {
    frust::authoring::any::<State, _>(
        text(title)
            .size(TITLE_SIZE)
            .weight(TITLE_WEIGHT)
            .line_height(LineHeight::Absolute(TITLE_LINE_HEIGHT)),
    )
}

/// A declarative M3 small/center-aligned Top App Bar. See the [module
/// docs](self).
pub struct AppBarView<State: 'static> {
    title: String,
    leading: Option<AnyView<State>>,
    actions: Vec<AnyView<State>>,
}

/// Create an app bar titled `title`, with no leading slot or actions (attach
/// them with [`AppBarView::leading`]/[`AppBarView::actions`]).
pub fn app_bar<State: 'static>(title: impl Into<String>) -> AppBarView<State> {
    AppBarView {
        title: title.into(),
        leading: None,
        actions: Vec::new(),
    }
}

/// PascalCase alias for [`app_bar`], matching the container view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn AppBar<State: 'static>(title: impl Into<String>) -> AppBarView<State> {
    app_bar(title)
}

impl<State: 'static> AppBarView<State> {
    /// Attach a leading slot (typically a nav/back icon button), erased as an
    /// [`AnyView`]. Tint (`onSurface`, per R8) is the supplied view's own
    /// responsibility — see the [module docs](self).
    pub fn leading(mut self, leading: AnyView<State>) -> Self {
        self.leading = Some(leading);
        self
    }

    /// Attach trailing action slots, in reading order (the last one sits
    /// closest to the trailing edge). Tint (`onSurfaceVariant`) is each
    /// supplied view's own responsibility — see the [module docs](self).
    pub fn actions(mut self, actions: Vec<AnyView<State>>) -> Self {
        self.actions = actions;
        self
    }
}

/// Collect `view`'s leading (if any) and actions into one ordered slice of
/// [`AnyView`] references — the shared shape both `build`/`teardown` and the
/// [`frust::authoring::rebuild_children`] reconciliation walk over.
fn interactive_views<State: 'static>(view: &AppBarView<State>) -> Vec<&AnyView<State>> {
    let mut views = Vec::with_capacity(1 + view.actions.len());
    if let Some(leading) = &view.leading {
        views.push(leading);
    }
    views.extend(view.actions.iter());
    views
}

/// The retained widget for an [`AppBarView`].
pub struct AppBarWidget {
    title: ChildPod,
    /// The title text, retained for the semantics node's label.
    title_text: String,
    /// The leading slot (if any) followed by every action, in that fixed
    /// order — the one list [`frust::authoring::route_event`] hit-tests and
    /// [`frust::authoring::rebuild_children`] reconciles.
    interactive: Vec<ChildPod>,
    /// Whether `interactive[0]` is the leading slot (vs. the first action).
    has_leading: bool,
}

impl<State: 'static> View<State> for AppBarView<State> {
    type Element = AppBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AppBarWidget {
        let title_view = title_view::<State>(self.title.clone());
        let mut interactive = Vec::with_capacity(1 + self.actions.len());
        if let Some(leading) = &self.leading {
            interactive.push(frust::authoring::build_child(leading, ctx));
        }
        for action in &self.actions {
            interactive.push(frust::authoring::build_child(action, ctx));
        }
        AppBarWidget {
            title: frust::authoring::build_child(&title_view, ctx),
            title_text: self.title.clone(),
            interactive,
            has_leading: self.leading.is_some(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AppBarWidget,
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
        if element.has_leading != self.leading.is_some() {
            element.has_leading = self.leading.is_some();
            flags |= ChangeFlags::LAYOUT;
        }
        flags
    }

    fn teardown(&self, element: &mut AppBarWidget, ctx: &mut BuildCtx<'_>) {
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

impl Widget for AppBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let slot_bc = BoxConstraints::loose(Size::new(f64::INFINITY, HEIGHT));

        let mut left = PAD_X;
        let action_start = if self.has_leading {
            let size = self.interactive[0].layout_child(ctx, &slot_bc);
            self.interactive[0].set_origin(Point::new(left, (HEIGHT - size.height) / 2.0));
            left += size.width + GAP;
            1
        } else {
            0
        };

        // Lay out actions in reverse so the *last* action lands flush against
        // the trailing edge, preserving left-to-right reading order.
        let mut right = width - PAD_X;
        for pod in self.interactive[action_start..].iter_mut().rev() {
            let size = pod.layout_child(ctx, &slot_bc);
            right -= size.width;
            pod.set_origin(Point::new(right, (HEIGHT - size.height) / 2.0));
            right -= GAP;
        }

        let title_max_width = (right - left).max(0.0);
        let title_size = self.title.layout_child(
            ctx,
            &BoxConstraints::loose(Size::new(title_max_width, HEIGHT)),
        );
        self.title
            .set_origin(Point::new(left, (HEIGHT - title_size.height) / 2.0));

        bc.constrain(Size::new(width, HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let fill = resolve_container(Theme::from_paint_ctx(ctx));
        scene.fill_rect(ctx.origin(), ctx.size(), fill);
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
                for pod in &interactive[if has_leading { 1 } else { 0 }..] {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(interactive, title);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::BuildCtx;
    use frust::authoring::text::TextContext;
    use frust_widgets::test_support::{RecordingScene, leaf_any};
    use std::any::Any;

    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    fn build(view: &AppBarView<()>) -> AppBarWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut ctx(&mut counter))
    }

    /// A layout pass with a real `TextContext` (the title is a real `Text`
    /// child, which panics without one threaded in) and no theme.
    fn layout(w: &mut AppBarWidget, bc: &BoxConstraints) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        w.layout(&mut lctx, bc)
    }

    /// Like [`layout`], but with a theme threaded in too.
    fn layout_themed(w: &mut AppBarWidget, bc: &BoxConstraints, theme: &Theme) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(theme as &dyn Any));
        w.layout(&mut lctx, bc)
    }

    #[test]
    fn layout_places_leading_title_and_actions() {
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0), leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        assert_eq!(size, Size::new(400.0, HEIGHT));

        // Leading hugs the left edge.
        assert_eq!(w.interactive[0].origin().x, PAD_X);
        // The two actions sit right-to-left from the trailing edge, in
        // reading order (interactive[1] left of interactive[2]).
        assert!(w.interactive[1].origin().x < w.interactive[2].origin().x);
        assert_eq!(
            w.interactive[2].origin().x + w.interactive[2].size().width,
            400.0 - PAD_X
        );
        // Title sits between leading and the first action.
        assert!(w.title.origin().x > w.interactive[0].origin().x);
        assert!(w.title.origin().x < w.interactive[1].origin().x);
    }

    #[test]
    fn layout_without_leading_starts_title_at_pad_x() {
        let view: AppBarView<()> = app_bar("Home");
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        assert_eq!(w.title.origin().x, PAD_X);
    }

    #[test]
    fn unthemed_paint_uses_fallback_container() {
        let view: AppBarView<()> = app_bar("Home");
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].1, Size::new(300.0, HEIGHT));
    }

    #[test]
    fn themed_paint_resolves_surface() {
        let theme = crate::baseline();
        let view: AppBarView<()> = app_bar("Home");
        let mut w = build(&view);
        layout_themed(
            &mut w,
            &BoxConstraints::loose(Size::new(300.0, 100.0)),
            &theme,
        );

        struct ColorRecorder {
            colors: Vec<Color>,
        }
        impl PaintScene for ColorRecorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, c: Color) {
                self.colors.push(c);
            }
            fn draw_text(&mut self, _o: Point, _t: &str) {}
        }
        let mut scene = ColorRecorder { colors: Vec::new() };
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.colors[0], theme.scheme().surface);
    }

    #[test]
    fn rebuild_adopts_new_title() {
        let mut counter = 0u64;
        let prev: AppBarView<()> = app_bar("Home");
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        let next: AppBarView<()> = app_bar("Settings");
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_layout());
        assert_eq!(w.title_text, "Settings");
    }

    #[test]
    fn semantics_node_is_titlebar_labelled_with_the_title() {
        // Drive through a real `RenderRoot` (see
        // `frust-widgets/tests/semantics_tree.rs`'s pattern) since
        // `SemanticsCtx::new` is crate-private to `frust-core`.
        fn logic(_state: &mut ()) -> AppBarView<()> {
            app_bar("Inbox")
        }
        let mut root: frust_core::RenderRoot<(), AppBarView<()>> = frust_core::RenderRoot::new();
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
