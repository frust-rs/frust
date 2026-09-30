// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/app_bars/m3e_app_bars.dart`'s `M3EAppBar.sliver` constructor
// and its `_buildSliver`/`_buildFlexibleSpace` helpers (themselves a
// `SliverAppBar` + `FlexibleSpaceBar` configuration). See `super`'s module docs
// for the family's porting decisions.

//! The collapsing app bar ([`sliver_app_bar`]): a bar whose band and headline
//! interpolate between an expanded and a collapsed state as the app's own
//! content scrolls. Read [`super`]'s module docs first — the scroll-wiring
//! contract, metric table, and shape families all live there.
//!
//! # Why this is a box widget with a controlled input, not a sliver
//!
//! Upstream hands the whole job to Flutter's sliver protocol: `SliverAppBar`
//! participates in a `CustomScrollView`, which drives its extent every frame
//! from the shared scroll position. This framework has no sliver protocol and
//! no seam by which a widget can observe the scroll surface above it — the two
//! seams [`frust::ScrollView`] exposes are installed on the *view* at
//! construction and are the app's to hold. So the collapse is a **controlled
//! prop** ([`SliverAppBarView::scroll_offset`] /
//! [`SliverAppBarView::collapse`]) and this widget is a pure function of it —
//! the same app-side-rule shape [`crate::SearchViewMode::for_width`] takes.
//! Every input is clamped and NaN-safe at the boundary
//! ([`AppBarCollapse`]), so a degenerate offset can never reach a layout.
//!
//! # Geometry
//!
//! The **top row** (leading slot, actions) is one band of
//! [`AppBarCollapse::collapsed_height`], anchored to the *bottom* of the bar
//! once the bar is shorter than it — an unpinned bar slides its own row up and
//! out of view exactly as a scrolled-away `SliverAppBar` does. The bar clips its
//! children in that state and only in that state (upstream's `clipBehavior` is
//! `Clip.none`, but Flutter's own viewport clips a scrolled-off sliver for it;
//! this is that clip, reached from the other side).
//!
//! The **headline** interpolates as a pure function of
//! [`AppBarCollapse::headline`]: from `headlineSmallEmphasized` at
//! [`HEADLINE_INSET`](super) from the bar's start and bottom edges, to
//! `titleLargeEmphasized` in the top row beside the leading slot. Both the type
//! scale and the position interpolate continuously, where upstream switches
//! styles at the endpoints and pins the position
//! (`FlexibleSpaceBar(collapseMode: pin, expandedTitleScale: 1)` — the `1`
//! matters: upstream deliberately does *not* scale its title, which is what
//! makes a continuous type-scale interpolation the faithful reading rather than
//! a transform).
//!
//! [`AppBarVariant::Small`] has no expanded headline at all (upstream returns no
//! `flexibleSpace` for it), so its title stays in the top row at every collapse
//! value.

use frust::Theme;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx, View, Widget, build_child,
    rebuild_children, route_event, teardown_child, visit_children,
};
use kurbo::{Point, Size};
use peniko::Color;

use super::{
    AppBarCollapse, AppBarDensity, AppBarShapeFamily, AppBarVariant, GAP, HEADLINE_INSET,
    HEADLINE_LINE_HEIGHT, HEADLINE_SIZE, PAD_X, TITLE_LINE_HEIGHT, TITLE_SIZE, TitleSlot,
    fill_container, finite_or_zero, lerp, push_bar_semantics, resolve_container, resolve_radius,
    title_ref,
};

/// How a caller states the bar's collapse.
///
/// Held unresolved so the two builders are order-independent: a
/// [`SliverAppBarView::scroll_offset`] set before
/// [`SliverAppBarView::variant`] still resolves against the final geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CollapseInput {
    /// A fraction in `[0, 1]`, clamped on resolution.
    Fraction(f64),
    /// A scroll offset in logical px, mapped through
    /// [`AppBarCollapse::fraction_for_offset`].
    Offset(f64),
}

/// A declarative M3 collapsing app bar. See the [module docs](self).
pub struct SliverAppBarView<State: 'static> {
    title: TitleSlot<State>,
    leading: Option<AnyView<State>>,
    actions: Vec<AnyView<State>>,
    variant: AppBarVariant,
    density: AppBarDensity,
    shape_family: AppBarShapeFamily,
    center_title: bool,
    corner_shift: bool,
    pinned: bool,
    collapse: CollapseInput,
    background: Option<Color>,
    semantic_label: Option<String>,
}

/// Create a collapsing app bar titled `title`, fully expanded — upstream's
/// `M3EAppBar.sliver`, with its own defaults ([`AppBarVariant::Medium`],
/// [`AppBarShapeFamily::Round`], pinned).
///
/// Feed it the scroll offset your body reports; see [`super`]'s scroll-wiring
/// contract for the whole shape.
pub fn sliver_app_bar<State: 'static>(title: impl Into<String>) -> SliverAppBarView<State> {
    SliverAppBarView {
        title: TitleSlot::Text(title.into()),
        leading: None,
        actions: Vec::new(),
        variant: AppBarVariant::Medium,
        density: AppBarDensity::Regular,
        shape_family: AppBarShapeFamily::Round,
        center_title: false,
        corner_shift: true,
        pinned: true,
        collapse: CollapseInput::Fraction(0.0),
        background: None,
        semantic_label: None,
    }
}

/// PascalCase alias for [`sliver_app_bar`], matching the container view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn SliverAppBar<State: 'static>(title: impl Into<String>) -> SliverAppBarView<State> {
    sliver_app_bar(title)
}

impl<State: 'static> SliverAppBarView<State> {
    /// Attach a leading slot (typically a nav/back icon button). It stays in
    /// the top row at every collapse value.
    pub fn leading(mut self, leading: AnyView<State>) -> Self {
        self.leading = Some(leading);
        self
    }

    /// Attach trailing action slots, in reading order. They stay in the top row
    /// at every collapse value.
    pub fn actions(mut self, actions: Vec<AnyView<State>>) -> Self {
        self.actions = actions;
        self
    }

    /// Replace the title string with a caller view — upstream's `title` widget
    /// slot. The view is placed (and moved between the expanded and collapsed
    /// anchors) but never styled, so it carries no type-scale interpolation of
    /// its own.
    pub fn title_view(mut self, title: AnyView<State>) -> Self {
        self.title = TitleSlot::View(title);
        self
    }

    /// Select the expanded layout (`variant`, default
    /// [`AppBarVariant::Medium`]).
    pub fn variant(mut self, variant: AppBarVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set the vertical density — see [`super`]'s metrics table.
    pub fn density(mut self, density: AppBarDensity) -> Self {
        self.density = density;
        self
    }

    /// Set the container's corner shape family (default
    /// [`AppBarShapeFamily::Round`], upstream's own `M3EAppBar.sliver`
    /// default).
    pub fn shape_family(mut self, family: AppBarShapeFamily) -> Self {
        self.shape_family = family;
        self
    }

    /// Center the title at both collapse anchors (`centerTitle`).
    pub fn center_title(mut self, center: bool) -> Self {
        self.center_title = center;
        self
    }

    /// Shift the row out from under a protruding window-control corner (default
    /// `true`). Pass `false` for a bar that does not span the window's top edge
    /// (detail pane, sheet, dialog, below other content): layout cannot see the
    /// bar's window-space position and the corner value is never consumed, so
    /// such a bar would otherwise over-shift.
    #[must_use]
    pub fn corner_shift(mut self, enabled: bool) -> Self {
        self.corner_shift = enabled;
        self
    }

    /// Whether the bar bottoms out at its collapsed band (`true`, the default
    /// and upstream's) or keeps shrinking to nothing (`false`).
    ///
    /// Upstream's `floating`/`snap` companions are not ported — see [`super`]'s
    /// *Not ported* list.
    pub fn pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    /// Set the collapse fraction directly: `0.0` fully expanded, `1.0` fully
    /// collapsed. Clamped (and NaN-mapped to `0.0`) on resolution.
    pub fn collapse(mut self, collapse: f64) -> Self {
        self.collapse = CollapseInput::Fraction(collapse);
        self
    }

    /// Set the collapse from a scroll offset in logical px — the `offset` an
    /// app reads off the [`ScrollInfo`](frust::ScrollInfo) its body reports.
    ///
    /// Mapped through [`AppBarCollapse::fraction_for_offset`] against this
    /// bar's own final geometry, so the call order relative to
    /// [`Self::variant`]/[`Self::density`]/[`Self::pinned`] does not matter.
    pub fn scroll_offset(mut self, offset: f64) -> Self {
        self.collapse = CollapseInput::Offset(offset);
        self
    }

    /// Override the container fill (`backgroundColor`).
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Label the bar's accessibility container explicitly (`semanticLabel`,
    /// which upstream routes through its own `M3ESliverSemantic` wrapper).
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }

    /// This bar's collapse geometry.
    fn geometry(&self) -> AppBarCollapse {
        AppBarCollapse::new(self.variant, self.density, self.pinned)
    }

    /// The resolved collapse fraction, whichever way the caller stated it.
    fn collapse_fraction(&self) -> f64 {
        match self.collapse {
            CollapseInput::Fraction(t) => super::clamp01(t),
            CollapseInput::Offset(offset) => self.geometry().fraction_for_offset(offset),
        }
    }

    /// The label this bar's semantics container carries.
    fn label(&self) -> Option<&str> {
        self.semantic_label
            .as_deref()
            .or_else(|| self.title.label())
    }
}

/// The `(size, line_height)` the title text is composed at for a headline
/// fraction of `headline` — `headlineSmallEmphasized` fully expanded,
/// `titleLargeEmphasized` fully collapsed, linear between.
fn title_type_scale(headline: f64) -> (f32, f32) {
    (
        lerp(HEADLINE_SIZE as f64, TITLE_SIZE as f64, headline) as f32,
        lerp(
            HEADLINE_LINE_HEIGHT as f64,
            TITLE_LINE_HEIGHT as f64,
            headline,
        ) as f32,
    )
}

/// Collect `view`'s leading (if any), its already-materialized `title`, and its
/// actions into one ordered slice — the same order [`super::top`]'s bar keeps,
/// and the one the pods, hit-testing, and semantics all follow.
fn slot_views<'a, State: 'static>(
    view: &'a SliverAppBarView<State>,
    title: &'a AnyView<State>,
) -> Vec<&'a AnyView<State>> {
    let mut views = Vec::with_capacity(2 + view.actions.len());
    if let Some(leading) = &view.leading {
        views.push(leading);
    }
    views.push(title);
    views.extend(view.actions.iter());
    views
}

/// The retained widget for a [`SliverAppBarView`].
pub struct SliverAppBarWidget {
    /// The leading slot (if any), then the title, then every action.
    slots: Vec<ChildPod>,
    has_leading: bool,
    center_title: bool,
    corner_shift: bool,
    geometry: AppBarCollapse,
    /// The resolved collapse fraction this widget last laid out with.
    collapse: f64,
    shape_family: AppBarShapeFamily,
    background: Option<Color>,
    label: Option<String>,
}

impl SliverAppBarWidget {
    /// Index of the title pod within [`Self::slots`].
    fn title_index(&self) -> usize {
        usize::from(self.has_leading)
    }
}

impl<State: 'static> View<State> for SliverAppBarView<State> {
    type Element = SliverAppBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SliverAppBarWidget {
        let geometry = self.geometry();
        let collapse = self.collapse_fraction();
        let (size, line_height) = title_type_scale(geometry.headline(collapse));
        let mut storage = None;
        let title = title_ref(&self.title, &mut storage, size, line_height);
        let slots = slot_views(self, title)
            .into_iter()
            .map(|view| build_child(view, ctx))
            .collect();
        SliverAppBarWidget {
            slots,
            has_leading: self.leading.is_some(),
            center_title: self.center_title,
            corner_shift: self.corner_shift,
            geometry,
            collapse,
            shape_family: self.shape_family,
            background: self.background,
            label: self.label().map(str::to_string),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SliverAppBarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let prev_geometry = prev.geometry();
        let prev_collapse = prev.collapse_fraction();
        let (prev_size, prev_line) = title_type_scale(prev_geometry.headline(prev_collapse));
        let geometry = self.geometry();
        let collapse = self.collapse_fraction();
        let (size, line_height) = title_type_scale(geometry.headline(collapse));

        let mut prev_storage = None;
        let mut next_storage = None;
        let prev_title = title_ref(&prev.title, &mut prev_storage, prev_size, prev_line);
        let next_title = title_ref(&self.title, &mut next_storage, size, line_height);
        let mut flags = rebuild_children(
            &slot_views(prev, prev_title),
            &slot_views(self, next_title),
            &mut element.slots,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );

        let has_leading = self.leading.is_some();
        if element.has_leading != has_leading {
            element.has_leading = has_leading;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.center_title != self.center_title {
            element.center_title = self.center_title;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.corner_shift != self.corner_shift {
            element.corner_shift = self.corner_shift;
            flags |= ChangeFlags::LAYOUT;
        }
        // The collapse drives the band, the headline anchor, and the clip, so a
        // changed value is a relayout even when the title view itself
        // reconciled clean (an unstyled `title_view`).
        if element.geometry != geometry || element.collapse != collapse {
            element.geometry = geometry;
            element.collapse = collapse;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.shape_family != self.shape_family || element.background != self.background {
            element.shape_family = self.shape_family;
            element.background = self.background;
            flags |= ChangeFlags::PAINT;
        }
        element.label = self.label().map(str::to_string);
        flags
    }

    fn teardown(&self, element: &mut SliverAppBarWidget, ctx: &mut BuildCtx<'_>) {
        let geometry = self.geometry();
        let (size, line_height) = title_type_scale(geometry.headline(self.collapse_fraction()));
        let mut storage = None;
        let title = title_ref(&self.title, &mut storage, size, line_height);
        for (view, pod) in slot_views(self, title)
            .into_iter()
            .zip(element.slots.iter_mut())
        {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for SliverAppBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = finite_or_zero(bc.max().width);
        let height = self.geometry.height(self.collapse);
        let headline = self.geometry.headline(self.collapse);
        let row = self.geometry.collapsed_height();
        // The top row is bottom-anchored once the bar is shorter than it, so an
        // unpinned bar carries its own row up and out of view.
        let row_center = row.min(height) - row / 2.0;
        let slot_bc = BoxConstraints::loose(Size::new(f64::INFINITY, row));
        let title_index = self.title_index();

        // The collapsed top row shifts out from under a protruding window-control
        // corner; the EXPANDED headline (`expanded_x`, HEADLINE_INSET-based) is
        // deliberately not shifted — only the collapsed row sits under the control.
        let (shift_l, shift_r) = if self.corner_shift {
            let corners = ctx.window_insets().corner_insets;
            (
                if corners.top_left.height > 0.0 {
                    corners.top_left.width
                } else {
                    0.0
                },
                if corners.top_right.height > 0.0 {
                    corners.top_right.width
                } else {
                    0.0
                },
            )
        } else {
            (0.0, 0.0)
        };

        let mut left = PAD_X + shift_l;
        if self.has_leading {
            let size = self.slots[0].layout_child(ctx, &slot_bc);
            self.slots[0].set_origin(Point::new(left, row_center - size.height / 2.0));
            left += size.width + GAP;
        }

        // Actions, laid out in reverse so the *last* one lands flush against
        // the trailing edge with reading order preserved.
        let mut right = width - PAD_X - shift_r;
        for pod in self.slots[title_index + 1..].iter_mut().rev() {
            let size = pod.layout_child(ctx, &slot_bc);
            right -= size.width;
            pod.set_origin(Point::new(right, row_center - size.height / 2.0));
            right -= GAP;
        }

        // The headline's own width budget interpolates between the expanded
        // band's insets and the top row's remaining space, so the title is
        // measured against the box it is actually heading toward.
        let collapsed_available = (right - left).max(0.0);
        let expanded_available = (width - 2.0 * HEADLINE_INSET).max(0.0);
        let available = lerp(expanded_available, collapsed_available, headline).max(0.0);
        let title_pod = &mut self.slots[title_index];
        let title_size =
            title_pod.layout_child(ctx, &BoxConstraints::loose(Size::new(available, height)));

        let expanded_x = if self.center_title {
            (width - title_size.width).max(0.0) / 2.0
        } else {
            HEADLINE_INSET
        };
        let collapsed_x = if self.center_title {
            left + (collapsed_available - title_size.width).max(0.0) / 2.0
        } else {
            left
        };
        let expanded_y = height - HEADLINE_INSET - title_size.height;
        let collapsed_y = row_center - title_size.height / 2.0;
        title_pod.set_origin(Point::new(
            lerp(expanded_x, collapsed_x, headline),
            lerp(expanded_y, collapsed_y, headline),
        ));

        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let origin = ctx.origin();
        let size = ctx.size();
        let fill = self.background.unwrap_or_else(|| resolve_container(theme));
        let radius = resolve_radius(theme, self.shape_family, size.width, size.height);
        fill_container(scene, origin, size, radius, fill);

        // Only a bar shorter than its own top row has content outside its box.
        let clipped = size.height + f64::EPSILON < self.geometry.collapsed_height();
        if clipped {
            scene.push_clip(origin, size);
        }
        for pod in &mut self.slots {
            pod.paint_child(ctx, scene);
        }
        if clipped {
            scene.pop_clip();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.slots, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        push_bar_semantics(ctx, Role::TitleBar, self.label.as_deref(), &self.slots);
    }

    visit_children!(slots);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{CornerInset, CornerInsets, WindowInsets};
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;

    /// The regular-density collapsed band every assertion below is written
    /// against.
    const ROW: f64 = 64.0;

    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    fn build(view: &SliverAppBarView<()>) -> SliverAppBarWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut ctx(&mut counter))
    }

    /// A layout pass with a real `TextContext` (the title is a real `Text`
    /// child, which panics without one threaded in).
    fn layout(w: &mut SliverAppBarWidget, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        w.layout(
            &mut lctx,
            &BoxConstraints::loose(Size::new(width, f64::INFINITY)),
        )
    }

    /// Like [`layout`], under a window carrying the given corners.
    fn layout_with_corners(w: &mut SliverAppBarWidget, width: f64, corners: CornerInsets) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        let bc = BoxConstraints::loose(Size::new(width, f64::INFINITY));
        lctx.with_window_insets(WindowInsets::default().with_corner_insets(corners), |ctx| {
            w.layout(ctx, &bc)
        })
    }

    fn top_corners() -> CornerInsets {
        CornerInsets::new(
            CornerInset::new(44.0, 30.0),
            CornerInset::new(52.0, 30.0),
            CornerInset::ZERO,
            CornerInset::ZERO,
        )
    }

    /// Build + lay out a large, pinned bar at collapse `t`, reporting its size
    /// and title pod origin.
    fn large_at(t: f64) -> (Size, Point, Size) {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Large)
            .collapse(t);
        let mut w = build(&view);
        let size = layout(&mut w, 400.0);
        let title = &w.slots[w.title_index()];
        (size, title.origin(), title.size())
    }

    /// Records the container fill plus clip pushes/pops, so both the painted
    /// band and the scrolled-away clip can be observed.
    #[derive(Default)]
    struct ClipRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        clips: Vec<(Point, Size)>,
        pops: u32,
    }
    impl PaintScene for ClipRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {
            self.pops += 1;
        }
    }

    #[test]
    fn corner_shift_moves_the_collapsed_row_slots() {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Large)
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0)])
            .collapse(1.0);
        let mut w = build(&view);
        layout_with_corners(&mut w, 400.0, top_corners());
        assert_eq!(w.slots[0].origin().x, PAD_X + 44.0);
        let action = w.slots.last().unwrap();
        assert_eq!(
            action.origin().x + action.size().width,
            400.0 - PAD_X - 52.0
        );
    }

    #[test]
    fn corner_shift_opt_out_leaves_the_collapsed_row_unshifted() {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Large)
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0)])
            .corner_shift(false)
            .collapse(1.0);
        let mut w = build(&view);
        layout_with_corners(&mut w, 400.0, top_corners());
        assert_eq!(w.slots[0].origin().x, PAD_X);
        let action = w.slots.last().unwrap();
        assert_eq!(action.origin().x + action.size().width, 400.0 - PAD_X);
    }

    #[test]
    fn expanded_headline_is_not_shifted() {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Large)
            .leading(leaf_any(40.0, 40.0))
            .collapse(0.0);
        let mut w = build(&view);
        layout_with_corners(&mut w, 400.0, top_corners());
        let shifted = w.slots[w.title_index()].origin();
        let mut w = build(&view);
        layout_with_corners(&mut w, 400.0, CornerInsets::ZERO);
        let plain = w.slots[w.title_index()].origin();
        assert_eq!(shifted.x, HEADLINE_INSET);
        assert_eq!(shifted, plain);
    }

    #[test]
    fn the_container_paints_its_own_band_at_both_endpoints() {
        let theme = crate::baseline();
        for (t, expected) in [(0.0, 152.0), (1.0, ROW)] {
            let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
                .variant(AppBarVariant::Large)
                .collapse(t);
            let mut w = build(&view);
            let size = layout(&mut w, 400.0);
            assert_eq!(size.height, expected);

            let mut scene = ClipRecorder::default();
            let mut pctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
            w.paint(&mut pctx, &mut scene);
            // Round is this constructor's own default shape family, so the
            // container lands as a rounded rect at `shape.small`.
            assert_eq!(scene.rrects[0].1, size, "container band at t={t}");
            assert_eq!(scene.rrects[0].2, theme.shape.small, "radius at t={t}");
            assert_eq!(
                scene.rrects[0].3,
                theme.scheme().surface,
                "container role at t={t}"
            );
        }
    }

    #[test]
    fn the_expanded_endpoint_seats_the_headline_at_the_bottom_inset() {
        let (size, origin, title) = large_at(0.0);
        assert_eq!(size, Size::new(400.0, 152.0));
        assert_eq!(origin.x, HEADLINE_INSET);
        assert_eq!(origin.y, 152.0 - HEADLINE_INSET - title.height);
        // The headline sits below the top row, not inside it.
        assert!(origin.y >= ROW);
    }

    #[test]
    fn the_collapsed_endpoint_seats_the_headline_in_the_top_row() {
        let (size, origin, title) = large_at(1.0);
        assert_eq!(size, Size::new(400.0, ROW));
        assert_eq!(origin.x, PAD_X);
        assert_eq!(origin.y, (ROW - title.height) / 2.0);
    }

    #[test]
    fn the_midpoint_interpolates_the_band_and_the_headline_anchor() {
        let (expanded_size, expanded_origin, _) = large_at(0.0);
        let (mid_size, mid_origin, _) = large_at(0.5);
        let (collapsed_size, collapsed_origin, _) = large_at(1.0);

        assert_eq!(mid_size.height, (152.0 + ROW) / 2.0);
        assert!(mid_size.height < expanded_size.height);
        assert!(mid_size.height > collapsed_size.height);
        // The anchor sits strictly between the two endpoints on both axes. The
        // start inset travels *inward* (the expanded headline's own 16dp, the
        // collapsed row's 4dp edge inset), so the bound is by min/max rather
        // than by an assumed direction.
        assert!(mid_origin.x > expanded_origin.x.min(collapsed_origin.x));
        assert!(mid_origin.x < expanded_origin.x.max(collapsed_origin.x));
        assert!(mid_origin.y < expanded_origin.y);
        assert!(mid_origin.y > collapsed_origin.y);
    }

    #[test]
    fn a_quarter_and_three_quarter_collapse_stay_inside_the_endpoints() {
        for t in [0.25, 0.75] {
            let (size, origin, _) = large_at(t);
            assert!(size.height > ROW && size.height < 152.0, "band at t={t}");
            assert!(origin.x >= HEADLINE_INSET.min(PAD_X), "anchor at t={t}");
            assert!(origin.y > 0.0, "anchor at t={t}");
        }
        // Monotone in the band, endpoint to endpoint.
        let heights: Vec<f64> = [0.0, 0.25, 0.5, 0.75, 1.0]
            .into_iter()
            .map(|t| large_at(t).0.height)
            .collect();
        assert!(
            heights.windows(2).all(|w| w[0] > w[1]),
            "the band shrinks monotonically: {heights:?}"
        );
    }

    #[test]
    fn the_headline_type_scale_interpolates_between_the_two_tokens() {
        assert_eq!(title_type_scale(0.0), (HEADLINE_SIZE, HEADLINE_LINE_HEIGHT));
        assert_eq!(title_type_scale(1.0), (TITLE_SIZE, TITLE_LINE_HEIGHT));
        let (mid_size, mid_line) = title_type_scale(0.5);
        assert_eq!(mid_size, (HEADLINE_SIZE + TITLE_SIZE) / 2.0);
        assert_eq!(mid_line, (HEADLINE_LINE_HEIGHT + TITLE_LINE_HEIGHT) / 2.0);

        // And the composed title actually shrinks with it.
        let (_, _, expanded) = large_at(0.0);
        let (_, _, collapsed) = large_at(1.0);
        assert!(
            expanded.height > collapsed.height,
            "expanded title {expanded:?} should be taller than collapsed {collapsed:?}"
        );
    }

    #[test]
    fn a_scroll_offset_maps_onto_the_same_geometry_whatever_the_builder_order() {
        // Set before the variant it depends on — the resolution is deferred, so
        // both spellings agree.
        let early: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .scroll_offset(44.0)
            .variant(AppBarVariant::Large);
        let late: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Large)
            .scroll_offset(44.0);
        assert_eq!(early.collapse_fraction(), late.collapse_fraction());
        assert_eq!(early.collapse_fraction(), 0.5);

        // Which is the same bar a direct fraction produces.
        let mut offset_bar = build(&early);
        let offset_size = layout(&mut offset_bar, 400.0);
        assert_eq!(offset_size.height, large_at(0.5).0.height);
    }

    #[test]
    fn a_degenerate_collapse_input_never_reaches_the_layout() {
        for input in [f64::NAN, -3.0, 7.0, f64::INFINITY] {
            let view: SliverAppBarView<()> = sliver_app_bar("Inbox").collapse(input);
            let mut w = build(&view);
            let size = layout(&mut w, 400.0);
            assert!(
                size.height.is_finite() && size.height >= ROW,
                "collapse {input} produced {size:?}"
            );
        }
        for offset in [f64::NAN, -50.0, f64::INFINITY] {
            let view: SliverAppBarView<()> = sliver_app_bar("Inbox").scroll_offset(offset);
            let mut w = build(&view);
            let size = layout(&mut w, 400.0);
            assert!(
                size.height.is_finite() && size.height >= ROW,
                "offset {offset} produced {size:?}"
            );
        }
    }

    #[test]
    fn an_unpinned_bar_scrolls_its_row_away_and_clips_while_it_does() {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .pinned(false)
            .leading(leaf_any(40.0, 40.0))
            .collapse(1.0);
        let mut w = build(&view);
        let size = layout(&mut w, 400.0);
        assert_eq!(size.height, 0.0);
        // The row is bottom-anchored, so the leading slot has left the box.
        assert!(w.slots[0].origin().y < 0.0);

        let mut scene = ClipRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.clips.len(), 1);
        assert_eq!(scene.pops, 1);

        // A pinned bar at the same fraction never clips: it bottoms out at the
        // top row, which always fits.
        let pinned: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .leading(leaf_any(40.0, 40.0))
            .collapse(1.0);
        let mut w = build(&pinned);
        let size = layout(&mut w, 400.0);
        assert_eq!(size.height, ROW);
        let mut scene = ClipRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut scene);
        assert!(scene.clips.is_empty());
    }

    #[test]
    fn the_small_variant_keeps_its_title_in_the_row_at_every_offset() {
        for t in [0.0, 0.5, 1.0] {
            let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
                .variant(AppBarVariant::Small)
                .collapse(t);
            let mut w = build(&view);
            let size = layout(&mut w, 400.0);
            let title = &w.slots[w.title_index()];
            assert_eq!(size.height, ROW, "band at t={t}");
            assert_eq!(title.origin().x, PAD_X, "anchor at t={t}");
            assert_eq!(
                title.origin().y,
                (ROW - title.size().height) / 2.0,
                "anchor at t={t}"
            );
        }
    }

    #[test]
    fn compact_density_shifts_both_endpoints() {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Medium)
            .density(AppBarDensity::Compact)
            .collapse(0.0);
        let mut w = build(&view);
        assert_eq!(layout(&mut w, 400.0).height, 112.0 - 8.0);

        let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Medium)
            .density(AppBarDensity::Compact)
            .collapse(1.0);
        let mut w = build(&view);
        assert_eq!(layout(&mut w, 400.0).height, ROW - 8.0);
    }

    #[test]
    fn leading_and_actions_stay_in_the_top_row_while_the_headline_moves() {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Large)
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0), leaf_any(24.0, 24.0)])
            .collapse(0.0);
        let mut w = build(&view);
        layout(&mut w, 400.0);

        assert_eq!(w.slots[0].origin().x, PAD_X);
        assert_eq!(w.slots[0].origin().y, (ROW - 40.0) / 2.0);
        assert_eq!(
            w.slots[3].origin().x + w.slots[3].size().width,
            400.0 - PAD_X
        );
        assert_eq!(w.slots[3].origin().y, (ROW - 24.0) / 2.0);
        // The headline is below them all, at the expanded anchor.
        assert!(w.slots[1].origin().y > ROW);
    }

    #[test]
    fn rebuild_adopts_a_new_collapse_value() {
        let mut counter = 0u64;
        let prev: SliverAppBarView<()> = sliver_app_bar("Inbox").variant(AppBarVariant::Large);
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        assert_eq!(w.collapse, 0.0);

        let next: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Large)
            .collapse(1.0);
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_layout());
        assert_eq!(w.collapse, 1.0);
        assert_eq!(layout(&mut w, 400.0).height, ROW);
    }

    #[test]
    fn semantics_node_is_titlebar_labelled_with_the_title() {
        fn logic(_state: &mut ()) -> SliverAppBarView<()> {
            sliver_app_bar("Inbox")
        }
        let mut root: frust_core::RenderRoot<(), SliverAppBarView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 112.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TitleBar)
            .expect("a TitleBar node is contributed");
        assert_eq!(node.label(), Some("Inbox"));
    }
}
