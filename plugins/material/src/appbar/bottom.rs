// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/app_bars/m3e_app_bars.dart`'s `M3EAppBar.bottom` constructor
// and its `_buildBottom` helper. See `super`'s module docs for the family's
// porting decisions.

//! The bottom app bar ([`bottom_app_bar`]): an action row docked to the bottom
//! edge with an optional prominent/FAB slot at the trailing edge. Read
//! [`super`]'s module docs first for the shared slot contract and metrics.
//!
//! Layout mirrors upstream's own `Row`: the actions run from the leading edge
//! in reading order, a spacer eats the rest, and the fab slot (if any) sits
//! flush against the trailing edge, everything vertically centered in the
//! [`AppBarMetrics::bottom_height`] band. The fab slot is an opaque
//! caller-supplied view like every other slot here — typically a real
//! [`fn@crate::fab`], to which this widget adds no chrome of its own.
//!
//! **No cutout/notch.** Upstream's bottom bar has none — it is a plain
//! `Material` with the FAB inside the same row (`_buildBottom`), not Flutter's
//! own `BottomAppBar(shape: NotchedShape)`. This port carries that shape over
//! rather than inventing a notch the reference never draws.
//!
//! The band is density-independent: `M3EAppBar.bottom` pins `density: regular`
//! and reads `bottomHeight` straight off the theme, so there is no compact
//! bottom bar to port. Its container role is `colors.surface_container`
//! (upstream's `bottomBackgroundColor`), one step up from the top bar's
//! `colors.surface`, and its corners are square with no shape-family knob —
//! again upstream's own fixed choices for this constructor.

use frust::Theme;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx, View, Widget, build_child,
    rebuild_children, route_event, teardown_child, visit_children,
};
use kurbo::{Point, Size};
use peniko::Color;

use super::{
    AppBarDensity, AppBarMetrics, GAP, PAD_X, finite_or_zero, push_bar_semantics,
    resolve_bottom_container,
};

/// A declarative M3 bottom app bar. See the [module docs](self).
pub struct BottomAppBarView<State: 'static> {
    actions: Vec<AnyView<State>>,
    fab: Option<AnyView<State>>,
    background: Option<Color>,
    semantic_label: Option<String>,
}

/// Create a bottom app bar with no actions and no fab slot (attach them with
/// [`BottomAppBarView::actions`]/[`BottomAppBarView::fab`]) — upstream's
/// `M3EAppBar.bottom`.
pub fn bottom_app_bar<State: 'static>() -> BottomAppBarView<State> {
    BottomAppBarView {
        actions: Vec::new(),
        fab: None,
        background: None,
        semantic_label: None,
    }
}

/// PascalCase alias for [`bottom_app_bar`], matching the container view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn BottomAppBar<State: 'static>() -> BottomAppBarView<State> {
    bottom_app_bar()
}

impl<State: 'static> BottomAppBarView<State> {
    /// Attach the action slots, in reading order from the leading edge. Tint is
    /// each supplied view's own responsibility — see [`super`]'s slot contract.
    pub fn actions(mut self, actions: impl IntoIterator<Item = impl View<State>>) -> Self {
        self.actions = actions.into_iter().map(AnyView::new).collect();
        self
    }

    /// Attach the prominent/FAB slot at the trailing edge
    /// (`floatingActionButton`).
    pub fn fab(mut self, fab: impl View<State>) -> Self {
        self.fab = Some(AnyView::new(fab));
        self
    }

    /// Override the container fill, winning over the theme role and its
    /// fallback alike.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Label the bar's accessibility container. Unlabelled by default — a
    /// bottom bar owns no title text to borrow one from.
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }
}

/// Collect `view`'s actions followed by its fab slot (if any) into one ordered
/// slice of [`AnyView`] references — the order the pods, hit-testing, and
/// semantics all keep.
fn slot_views<State: 'static>(view: &BottomAppBarView<State>) -> Vec<&AnyView<State>> {
    let mut views = Vec::with_capacity(view.actions.len() + usize::from(view.fab.is_some()));
    views.extend(view.actions.iter());
    if let Some(fab) = &view.fab {
        views.push(fab);
    }
    views
}

/// The retained widget for a [`BottomAppBarView`].
pub struct BottomAppBarWidget {
    /// Every action, then the fab slot (if any).
    slots: Vec<ChildPod>,
    action_count: usize,
    has_fab: bool,
    background: Option<Color>,
    label: Option<String>,
}

impl<State: 'static> View<State> for BottomAppBarView<State> {
    type Element = BottomAppBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> BottomAppBarWidget {
        let slots = slot_views(self)
            .into_iter()
            .map(|view| build_child(view, ctx))
            .collect();
        BottomAppBarWidget {
            slots,
            action_count: self.actions.len(),
            has_fab: self.fab.is_some(),
            background: self.background,
            label: self.semantic_label.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BottomAppBarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_children(
            &slot_views(prev),
            &slot_views(self),
            &mut element.slots,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );

        if element.action_count != self.actions.len() {
            element.action_count = self.actions.len();
            flags |= ChangeFlags::LAYOUT;
        }
        let has_fab = self.fab.is_some();
        if element.has_fab != has_fab {
            element.has_fab = has_fab;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.background != self.background {
            element.background = self.background;
            flags |= ChangeFlags::PAINT;
        }
        element.label = self.semantic_label.clone();
        flags
    }

    fn teardown(&self, element: &mut BottomAppBarWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in slot_views(self).into_iter().zip(element.slots.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for BottomAppBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = finite_or_zero(bc.max().width);
        let band = AppBarMetrics::for_density(AppBarDensity::Regular).bottom_height;
        let slot_bc = BoxConstraints::loose(Size::new(f64::INFINITY, band));

        let mut left = PAD_X;
        for pod in self.slots[..self.action_count].iter_mut() {
            let size = pod.layout_child(ctx, &slot_bc);
            pod.set_origin(Point::new(left, (band - size.height) / 2.0));
            left += size.width + GAP;
        }

        // The fab slot hugs the trailing edge, whatever the action row's own
        // width came to (upstream's `Spacer` between the two).
        if self.has_fab {
            let pod = &mut self.slots[self.action_count];
            let size = pod.layout_child(ctx, &slot_bc);
            pod.set_origin(Point::new(
                width - PAD_X - size.width,
                (band - size.height) / 2.0,
            ));
        }

        bc.constrain(Size::new(width, band))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let fill = self
            .background
            .unwrap_or_else(|| resolve_bottom_container(Theme::from_paint_ctx(ctx)));
        scene.fill_rect(ctx.origin(), ctx.size(), fill);
        for pod in &mut self.slots {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.slots, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        push_bar_semantics(ctx, Role::Toolbar, self.label.as_deref(), &self.slots);
    }

    visit_children!(slots);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_widgets::test_support::{RecordingScene, leaf_any};

    /// The band every assertion below is written against.
    const BAND: f64 = 80.0;

    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    fn build(view: &BottomAppBarView<()>) -> BottomAppBarWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut ctx(&mut counter))
    }

    fn layout(w: &mut BottomAppBarWidget, bc: &BoxConstraints) -> Size {
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, bc)
    }

    #[test]
    fn actions_run_from_the_leading_edge_and_the_fab_hugs_the_trailing_one() {
        let view: BottomAppBarView<()> = bottom_app_bar()
            .actions(vec![
                leaf_any(24.0, 24.0),
                leaf_any(24.0, 24.0),
                leaf_any(24.0, 24.0),
            ])
            .fab(leaf_any(56.0, 56.0));
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        assert_eq!(size, Size::new(400.0, BAND));

        assert_eq!(w.slots[0].origin().x, PAD_X);
        assert_eq!(w.slots[1].origin().x, PAD_X + 24.0 + GAP);
        assert_eq!(w.slots[2].origin().x, PAD_X + 2.0 * (24.0 + GAP));
        // The fab is the last pod, flush against the trailing edge.
        assert_eq!(
            w.slots[3].origin().x + w.slots[3].size().width,
            400.0 - PAD_X
        );
        // Everything is centered in the band.
        assert_eq!(w.slots[0].origin().y, (BAND - 24.0) / 2.0);
        assert_eq!(w.slots[3].origin().y, (BAND - 56.0) / 2.0);
    }

    #[test]
    fn a_bar_without_a_fab_still_spans_the_width() {
        let view: BottomAppBarView<()> = bottom_app_bar().actions(vec![leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(320.0, 200.0)));
        assert_eq!(size, Size::new(320.0, BAND));
        assert_eq!(w.slots.len(), 1);
    }

    #[test]
    fn unthemed_paint_uses_the_fallback_container() {
        let view: BottomAppBarView<()> = bottom_app_bar();
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, BAND));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].1, Size::new(300.0, BAND));
    }

    #[test]
    fn themed_paint_resolves_surface_container() {
        struct ColorRecorder {
            colors: Vec<Color>,
        }
        impl PaintScene for ColorRecorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, c: Color) {
                self.colors.push(c);
            }
            fn draw_text(&mut self, _o: Point, _t: &str) {}
        }

        let theme = crate::baseline();
        let view: BottomAppBarView<()> = bottom_app_bar();
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let mut scene = ColorRecorder { colors: Vec::new() };
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, BAND)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.colors[0], theme.scheme().surface_container);
    }

    #[test]
    fn rebuild_adopts_a_new_fab_slot() {
        let mut counter = 0u64;
        let prev: BottomAppBarView<()> = bottom_app_bar().actions(vec![leaf_any(24.0, 24.0)]);
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        assert!(!w.has_fab);

        let next: BottomAppBarView<()> = bottom_app_bar()
            .actions(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(56.0, 56.0));
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_layout());
        assert!(w.has_fab);
        assert_eq!(w.slots.len(), 2);
    }

    #[test]
    fn semantics_node_is_a_toolbar_forwarding_every_slot() {
        // A leaf without its own `semantics()` override (e.g. `leaf_any`)
        // contributes no node — use `Text` children instead, so the "forwards
        // every slot" assertion is meaningful (mirrors `toolbar.rs`'s identical
        // semantics-test note).
        fn logic(_state: &mut ()) -> BottomAppBarView<()> {
            bottom_app_bar()
                .actions(vec![frust::authoring::any::<(), _>(frust::text("One"))])
                .fab(frust::authoring::any::<(), _>(frust::text("Two")))
        }
        let mut root: frust_core::RenderRoot<(), BottomAppBarView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(300.0, BAND), &mut tcx as &mut dyn std::any::Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Toolbar)
            .expect("a Toolbar node is contributed");
        assert_eq!(node.children().len(), 2, "actions + fab forwarded");
    }
}
