// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/app_bars/m3e_app_bars.dart`'s `M3EAppBar.top`/`.search`
// constructors and their `_buildTop`/`_TitleSlot`/`_M3EAppBarSearchTitle`
// helpers. See `super`'s module docs for the family's porting decisions.

//! The fixed top app bar ([`app_bar`]) and its search-titled sibling
//! ([`search_app_bar`]) — the family's two flat, single-band bars. Read
//! [`super`]'s module docs first: the slot contract, metric table, shape
//! families, and semantics all live there.
//!
//! Layout is one band of [`AppBarMetrics::small_height`] (or an explicit
//! [`AppBarView::toolbar_height`]), below the top window inset and inside the
//! left/right ones the bar consumes itself (see [`AppBarView::safe_area`]):
//! the leading slot hugs the leading edge (4dp in), actions hug the trailing
//! edge in reading order (4dp in), and the title takes the space between
//! them — 16dp from an edge that
//! has no slot on it, start-aligned by default,
//! [`centered`](AppBarView::center_title) *within that remaining space* on
//! request, which is what upstream's own `_TitleSlot` (an `Align` inside the
//! `Expanded`) centers within too. Everything is vertically centered in the
//! band.
//!
//! Upstream's `_TitleSlot` additionally branches on whether the title is a
//! search anchor, capping it at `searchBarTheme.maxWidth` and letting it fill
//! the slot; that branch has nothing to port here. The cap's own default is
//! `double.infinity`, and [`crate::search_bar`] already fills whatever finite
//! width it is offered (its `layout` takes `bc.max().width`), so one uniform
//! "lay the title out against the remaining width" rule reproduces both arms.

use frust::Theme;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx, ThemeTextColor, View, Widget, build_child,
    rebuild_children, route_event, teardown_child, visit_children,
};
use kurbo::{Point, Size};
use peniko::Color;

use super::{
    AppBarContainer, AppBarDensity, AppBarMetrics, AppBarShapeFamily, GAP, MAX_ELEVATION, PAD_X,
    TITLE_EDGE_INSET, TITLE_LINE_HEIGHT, TITLE_SIZE, TitleSlot, fill_container, finite_or_zero,
    push_bar_semantics, resolve_elevation_shadow, resolve_radius, resolve_top_container,
    title_ref_in,
};
use crate::search::SearchBarView;

/// A declarative M3 fixed top app bar. See the [module docs](self).
pub struct AppBarView<State: 'static> {
    title: TitleSlot<State>,
    leading: Option<AnyView<State>>,
    actions: Vec<AnyView<State>>,
    center_title: bool,
    corner_shift: bool,
    density: AppBarDensity,
    shape_family: AppBarShapeFamily,
    toolbar_height: Option<f64>,
    background: Option<Color>,
    container: Option<AppBarContainer>,
    elevation: u8,
    safe_area: bool,
    semantic_label: Option<String>,
}

/// Create a top app bar titled `title`, with no leading slot or actions (attach
/// them with [`AppBarView::leading`]/[`AppBarView::actions`]) — upstream's
/// `M3EAppBar.top`.
pub fn app_bar<State: 'static>(title: impl Into<String>) -> AppBarView<State> {
    AppBarView {
        title: TitleSlot::Text(title.into()),
        leading: None,
        actions: Vec::new(),
        center_title: false,
        corner_shift: true,
        density: AppBarDensity::Regular,
        shape_family: AppBarShapeFamily::Square,
        toolbar_height: None,
        background: None,
        container: None,
        elevation: 0,
        safe_area: true,
        semantic_label: None,
    }
}

/// PascalCase alias for [`app_bar`], matching the container view-fn vocabulary.
#[allow(non_snake_case)]
pub fn AppBar<State: 'static>(title: impl Into<String>) -> AppBarView<State> {
    app_bar(title)
}

/// Create a top app bar whose title slot is `bar`, a [`crate::search_bar`] —
/// upstream's `M3EAppBar.search`, which is itself a factory delegating to
/// `M3EAppBar.top` with a `_M3EAppBarSearchTitle` in the title slot.
///
/// The bar is composed, not re-implemented: configure it (hint, leading,
/// trailing, and the `on_tap` that opens the search view) exactly as a
/// standalone one, then hand it over.
///
/// ```ignore
/// search_app_bar(
///     search_bar(state.query.clone(), |s: &mut App, q| s.query = q)
///         .hint("Search recipes")
///         .on_tap(|s: &mut App| s.search_open = true),
/// )
/// .actions(vec![any(icon_button(icons::MORE_VERT, |_: &mut App| {}))])
/// ```
///
/// Upstream additionally overrides the embedded bar's fill to
/// `colorScheme.surface`, "contrasting against the app bar's
/// surfaceContainerHigh". This port's bar container is `colors.surface` itself
/// (see [`super`]'s metrics divergence), and [`crate::search_bar`] carries the
/// M3-published `surfaceContainerHigh` pill, so the two already contrast — from
/// the other side, without an override.
pub fn search_app_bar<State: 'static>(bar: SearchBarView<State>) -> AppBarView<State> {
    AppBarView {
        title: TitleSlot::View(frust::authoring::any::<State, _>(bar)),
        ..app_bar(String::new())
    }
}

/// PascalCase alias for [`search_app_bar`].
#[allow(non_snake_case)]
pub fn SearchAppBar<State: 'static>(bar: SearchBarView<State>) -> AppBarView<State> {
    search_app_bar(bar)
}

impl<State: 'static> AppBarView<State> {
    /// Attach a leading slot (typically a nav/back icon button), erased as an
    /// [`AnyView`]. Tint is the supplied view's own responsibility — see
    /// [`super`]'s slot contract.
    pub fn leading(mut self, leading: AnyView<State>) -> Self {
        self.leading = Some(leading);
        self
    }

    /// Attach trailing action slots, in reading order (the last one sits
    /// closest to the trailing edge). Tint is each supplied view's own
    /// responsibility.
    pub fn actions(mut self, actions: Vec<AnyView<State>>) -> Self {
        self.actions = actions;
        self
    }

    /// Replace the title string with a caller view — upstream's `title` widget
    /// slot, which takes precedence over its `titleText`. The view is placed
    /// but never styled, and (unlike a string title) labels itself for
    /// accessibility.
    pub fn title_view(mut self, title: AnyView<State>) -> Self {
        self.title = TitleSlot::View(title);
        self
    }

    /// Center the title within the space left between the leading slot and the
    /// actions (`centerTitle`, default `false`).
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

    /// Set the vertical density — see [`super`]'s metrics table.
    pub fn density(mut self, density: AppBarDensity) -> Self {
        self.density = density;
        self
    }

    /// Set the container's corner shape family (default
    /// [`AppBarShapeFamily::Square`], upstream's own `M3EAppBar.top` default).
    pub fn shape_family(mut self, family: AppBarShapeFamily) -> Self {
        self.shape_family = family;
        self
    }

    /// Override the band height outright (`toolbarHeight`), bypassing the
    /// density table — the seam that reaches upstream's own 72dp band, among
    /// anything else.
    pub fn toolbar_height(mut self, height: f64) -> Self {
        self.toolbar_height = Some(height);
        self
    }

    /// Override the container fill (`backgroundColor`), winning over
    /// [`Self::container`], the elevation surface role, the theme default and
    /// its fallback alike.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Fill the container with a theme color role (default
    /// [`AppBarContainer::Surface`]), resolved at paint time so it follows a
    /// brightness flip or theme swap. An explicit role also wins over the
    /// elevation surface role [`Self::elevation`] would otherwise pick, and a
    /// string title takes the role's foreground (see [`AppBarContainer`]).
    /// [`Self::background`] still wins over it.
    ///
    /// ```
    /// use frust_material::{AppBarContainer, app_bar};
    ///
    /// // Flutter's counter-template bar: an inverse-primary fill, raised.
    /// let bar = app_bar::<()>("Counter")
    ///     .container(AppBarContainer::InversePrimary)
    ///     .elevation(2);
    /// # let _ = bar;
    /// ```
    pub fn container(mut self, role: AppBarContainer) -> Self {
        self.container = Some(role);
        self
    }

    /// Raise the bar to M3 elevation `level` (`0..=5`, clamped; default 0),
    /// resolved at paint time against the theme's elevation table: a level
    /// above 0 paints that level's drop shadow beneath the container and, with
    /// no [`Self::container`]/[`Self::background`] set, fills with the level's
    /// surface role (level 2 is `surfaceContainer`). See [`super`]'s module
    /// docs for the paint order.
    pub fn elevation(mut self, level: u8) -> Self {
        self.elevation = level.min(MAX_ELEVATION);
        self
    }

    /// Whether the bar consumes the **top, left and right** window insets
    /// itself (default `true`) — Flutter's `AppBar` wrapping its toolbar in
    /// `SafeArea(bottom: false)`. It grows by
    /// `ctx.window_insets().padding().top`, paints its fill (and shadow) edge
    /// to edge through that band and across the full width, and lays its
    /// slots out below the top inset and inside the left/right ones (a
    /// landscape notch or home-indicator edge). A parent `frust::safe_area`
    /// that already consumed an edge leaves nothing on it to consume, so it
    /// never double-insets. Pass `false` for a bar that is not docked to the
    /// window's top edge (a preview, a pane, a sheet); it then consumes no
    /// edge at all.
    pub fn safe_area(mut self, enabled: bool) -> Self {
        self.safe_area = enabled;
        self
    }

    /// Label the bar's accessibility container explicitly (`semanticLabel`).
    /// Without one, a string title labels it and a caller view leaves it
    /// unlabelled.
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }

    /// The band this bar occupies, in logical px.
    fn band(&self) -> f64 {
        self.toolbar_height
            .unwrap_or_else(|| AppBarMetrics::for_density(self.density).small_height)
    }

    /// The themed color role a string title takes over this bar's container.
    fn title_role(&self) -> ThemeTextColor {
        self.container.unwrap_or_default().title_role()
    }

    /// This bar's title slot as a view reference, composing a string title
    /// into `storage` at the top bar's size and container-driven color role.
    fn title_ref<'a>(&'a self, storage: &'a mut Option<AnyView<State>>) -> &'a AnyView<State> {
        title_ref_in(
            &self.title,
            storage,
            TITLE_SIZE,
            TITLE_LINE_HEIGHT,
            self.title_role(),
        )
    }

    /// The label this bar's semantics container carries.
    fn label(&self) -> Option<&str> {
        self.semantic_label
            .as_deref()
            .or_else(|| self.title.label())
    }
}

/// Collect `view`'s leading (if any), its already-materialized `title`, and its
/// actions into one ordered slice of [`AnyView`] references — the shared shape
/// `build`/`teardown` and [`frust::authoring::rebuild_children`] both walk, and
/// the order [`AppBarWidget`]'s pods, hit-testing, and semantics all keep.
fn slot_views<'a, State: 'static>(
    view: &'a AppBarView<State>,
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

/// The retained widget for an [`AppBarView`].
pub struct AppBarWidget {
    /// The leading slot (if any), then the title, then every action — the one
    /// list [`frust::authoring::route_event`] hit-tests and
    /// [`frust::authoring::rebuild_children`] reconciles.
    ///
    /// The title rides this list rather than sitting beside it because a title
    /// can be interactive ([`search_app_bar`]'s embedded bar is), and only a
    /// routed pod receives events. The list is reconciled positionally like
    /// every other container in this catalog, so gaining or losing the leading
    /// slot shifts the pods after it — an in-place type swap where the kinds
    /// differ, with the framework's own stable-prefix rules applying from there.
    slots: Vec<ChildPod>,
    /// Whether `slots[0]` is the leading slot (vs. the title).
    has_leading: bool,
    center_title: bool,
    corner_shift: bool,
    band: f64,
    shape_family: AppBarShapeFamily,
    background: Option<Color>,
    container: Option<AppBarContainer>,
    /// Already clamped to `0..=5`.
    elevation: u8,
    safe_area: bool,
    /// The top window inset consumed on the last `layout` pass — `0.0` when
    /// `safe_area` is off or nothing is left to consume. The slots sit below
    /// it; the fill and shadow cover it.
    top_inset: f64,
    /// The `(left, right)` window insets consumed on the last `layout` pass,
    /// `0.0` under the same conditions as [`Self::top_inset`]. The slots sit
    /// inside them; the fill and shadow still span the full width.
    side_insets: (f64, f64),
    /// The semantics container's label, retained across rebuilds.
    label: Option<String>,
}

impl AppBarWidget {
    /// Index of the title pod within [`Self::slots`].
    fn title_index(&self) -> usize {
        usize::from(self.has_leading)
    }
}

impl<State: 'static> View<State> for AppBarView<State> {
    type Element = AppBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AppBarWidget {
        let mut storage = None;
        let title = self.title_ref(&mut storage);
        let slots = slot_views(self, title)
            .into_iter()
            .map(|view| build_child(view, ctx))
            .collect();
        AppBarWidget {
            slots,
            has_leading: self.leading.is_some(),
            center_title: self.center_title,
            corner_shift: self.corner_shift,
            band: self.band(),
            shape_family: self.shape_family,
            background: self.background,
            container: self.container,
            elevation: self.elevation,
            safe_area: self.safe_area,
            top_inset: 0.0,
            side_insets: (0.0, 0.0),
            label: self.label().map(str::to_string),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AppBarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut prev_storage = None;
        let mut next_storage = None;
        let prev_title = prev.title_ref(&mut prev_storage);
        let next_title = self.title_ref(&mut next_storage);
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
        let band = self.band();
        if element.band != band {
            element.band = band;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.safe_area != self.safe_area {
            element.safe_area = self.safe_area;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.shape_family != self.shape_family
            || element.background != self.background
            || element.container != self.container
            || element.elevation != self.elevation
        {
            element.shape_family = self.shape_family;
            element.background = self.background;
            element.container = self.container;
            element.elevation = self.elevation;
            flags |= ChangeFlags::PAINT;
        }
        // A label carries no geometry or pixels of its own: assigned outright,
        // flagged by nothing (the same shape `icon_button`'s own
        // `semantic_label` rebuild takes).
        element.label = self.label().map(str::to_string);
        flags
    }

    fn teardown(&self, element: &mut AppBarWidget, ctx: &mut BuildCtx<'_>) {
        let mut storage = None;
        let title = self.title_ref(&mut storage);
        for (view, pod) in slot_views(self, title)
            .into_iter()
            .zip(element.slots.iter_mut())
        {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for AppBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = finite_or_zero(bc.max().width);
        let band = self.band;
        let slot_bc = BoxConstraints::loose(Size::new(f64::INFINITY, band));
        let title_index = self.title_index();
        let has_actions = self.slots.len() > title_index + 1;

        // Consume the top, left and right window insets (Flutter's
        // `SafeArea(bottom: false)`): the bar grows by the top one, and the
        // slots sit below it and inside the side ones. A parent safe area that
        // already consumed an edge leaves 0 here.
        let (pad_l, top, pad_r) = if self.safe_area {
            let padding = ctx.window_insets().padding();
            (
                padding.left.max(0.0),
                padding.top.max(0.0),
                padding.right.max(0.0),
            )
        } else {
            (0.0, 0.0, 0.0)
        };
        self.top_inset = top;
        self.side_insets = (pad_l, pad_r);

        // Shift the row out from under a protruding window-control corner (the
        // iPadOS 26+ traffic lights); see the module docs of `appbar`. A
        // corner's width is measured beyond the safe-area edge, so it stacks
        // on the consumed side padding rather than overlapping it.
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

        // Slots sit `PAD_X` in (their touch targets carry the glyph to the M3
        // 16dp mark); a bare title edge sits `TITLE_EDGE_INSET` in instead.
        let mut left = pad_l + shift_l;
        if self.has_leading {
            left += PAD_X;
            let size = self.slots[0].layout_child(ctx, &slot_bc);
            self.slots[0].set_origin(Point::new(left, top + (band - size.height) / 2.0));
            left += size.width + GAP;
        } else {
            left += TITLE_EDGE_INSET;
        }

        // Lay out actions in reverse so the *last* action lands flush against
        // the trailing edge, preserving left-to-right reading order.
        let mut right = width - pad_r - shift_r;
        if has_actions {
            right -= PAD_X;
            for pod in self.slots[title_index + 1..].iter_mut().rev() {
                let size = pod.layout_child(ctx, &slot_bc);
                right -= size.width;
                pod.set_origin(Point::new(right, top + (band - size.height) / 2.0));
                right -= GAP;
            }
        } else {
            right -= TITLE_EDGE_INSET;
        }

        let available = (right - left).max(0.0);
        let title_pod = &mut self.slots[title_index];
        let title_size =
            title_pod.layout_child(ctx, &BoxConstraints::loose(Size::new(available, band)));
        let title_x = if self.center_title {
            left + (available - title_size.width).max(0.0) / 2.0
        } else {
            left
        };
        title_pod.set_origin(Point::new(title_x, top + (band - title_size.height) / 2.0));

        bc.constrain(Size::new(width, band + top))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let origin = ctx.origin();
        let size = ctx.size();
        let fill = self
            .background
            .unwrap_or_else(|| resolve_top_container(theme, self.container, self.elevation));
        let radius = resolve_radius(theme, self.shape_family, size.width, size.height);
        // Shadow first, then the fill over it, then the slots — the order an
        // elevated card paints in, so only the offset/blurred rim shows.
        if let Some((y_offset, blur, color)) = resolve_elevation_shadow(theme, self.elevation) {
            scene.draw_shadow(
                Point::new(origin.x, origin.y + y_offset),
                size,
                radius,
                blur,
                color,
            );
        }
        fill_container(scene, origin, size, radius, fill);
        for pod in &mut self.slots {
            pod.paint_child(ctx, scene);
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
    use frust::authoring::{CornerInset, CornerInsets, WindowEdgeInsets, WindowInsets};
    use frust_widgets::test_support::{RecordingScene, leaf_any};
    use std::any::Any;

    /// The default band every assertion below is written against.
    const BAND: f64 = 64.0;

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

    /// Lay out under a window carrying the given window-control corners.
    fn layout_with_corners(
        w: &mut AppBarWidget,
        bc: &BoxConstraints,
        corners: CornerInsets,
    ) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        lctx.with_window_insets(WindowInsets::default().with_corner_insets(corners), |ctx| {
            w.layout(ctx, bc)
        })
    }

    /// Lay out under a window carrying `top` px of status-bar inset and
    /// nothing else.
    fn layout_with_top_inset(w: &mut AppBarWidget, bc: &BoxConstraints, top: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        lctx.with_window_insets(status_bar(top), |ctx| w.layout(ctx, bc))
    }

    fn status_bar(top: f64) -> WindowInsets {
        WindowInsets::new(
            WindowEdgeInsets::new(0.0, top, 0.0, 0.0),
            WindowEdgeInsets::ZERO,
        )
    }

    /// Lay out `view` under `theme` and paint it at its own laid-out size,
    /// returning what the paint pass recorded.
    fn paint_themed(view: &AppBarView<()>, theme: &Theme) -> FillRecorder {
        let mut w = build(view);
        let size = layout_themed(
            &mut w,
            &BoxConstraints::loose(Size::new(300.0, 100.0)),
            theme,
        );
        let mut scene = FillRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size).with_theme(theme);
        w.paint(&mut pctx, &mut scene);
        scene
    }

    fn dark_baseline() -> Theme {
        let mut theme = crate::baseline();
        theme.brightness = frust::Brightness::Dark;
        theme
    }

    fn top_corners() -> CornerInsets {
        CornerInsets::new(
            CornerInset::new(44.0, 30.0),
            CornerInset::new(52.0, 30.0),
            CornerInset::ZERO,
            CornerInset::ZERO,
        )
    }

    /// Records both container shapes — `fill_rect` (square family) and
    /// `fill_rounded_rect` (round family) — with their colors, plus every
    /// drop shadow, which [`RecordingScene`] (geometry only, `fill_rect`
    /// only) cannot observe. `ops` keeps the call order.
    #[derive(Default)]
    struct FillRecorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        ops: Vec<&'static str>,
    }
    impl PaintScene for FillRecorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
            self.ops.push("fill");
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
            self.ops.push("rounded");
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
            self.ops.push("shadow");
        }
    }

    #[test]
    fn layout_places_leading_title_and_actions() {
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0), leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        assert_eq!(size, Size::new(400.0, BAND));

        // Leading hugs the left edge; slots are [leading, title, a0, a1].
        assert_eq!(w.slots[0].origin().x, PAD_X);
        // The two actions sit right-to-left from the trailing edge, in reading
        // order (slots[2] left of slots[3]).
        assert!(w.slots[2].origin().x < w.slots[3].origin().x);
        assert_eq!(
            w.slots[3].origin().x + w.slots[3].size().width,
            400.0 - PAD_X
        );
        // Title sits between leading and the first action.
        assert!(w.slots[1].origin().x > w.slots[0].origin().x);
        assert!(w.slots[1].origin().x < w.slots[2].origin().x);
    }

    #[test]
    fn layout_without_leading_starts_title_at_the_m3_title_inset() {
        let view: AppBarView<()> = app_bar("Home");
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        assert_eq!(TITLE_EDGE_INSET, 16.0);
        assert_eq!(w.slots[0].origin().x, TITLE_EDGE_INSET);
    }

    #[test]
    fn layout_without_actions_ends_the_title_width_at_the_m3_title_inset() {
        // A greedy title view fills whatever width it is offered, so its end
        // is exactly where the available width stops.
        let view: AppBarView<()> = app_bar("").title_view(leaf_any(1_000.0, 40.0));
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        let title = &w.slots[0];
        assert_eq!(title.origin().x, TITLE_EDGE_INSET);
        assert_eq!(
            title.origin().x + title.size().width,
            300.0 - TITLE_EDGE_INSET
        );
    }

    #[test]
    fn leading_and_actions_keep_the_4dp_slot_edge_inset() {
        let view: AppBarView<()> = app_bar("")
            .title_view(leaf_any(1_000.0, 40.0))
            .leading(leaf_any(48.0, 48.0))
            .actions(vec![leaf_any(48.0, 48.0)]);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        assert_eq!(w.slots[0].origin().x, PAD_X);
        assert_eq!(
            w.slots[2].origin().x + w.slots[2].size().width,
            300.0 - PAD_X
        );
        // The title sits one gap off each slot, not 16dp off either edge.
        assert_eq!(w.slots[1].origin().x, PAD_X + 48.0 + GAP);
        assert_eq!(
            w.slots[1].origin().x + w.slots[1].size().width,
            300.0 - PAD_X - 48.0 - GAP
        );
    }

    #[test]
    fn the_title_inset_applies_after_the_corner_shift() {
        let view: AppBarView<()> = app_bar("").title_view(leaf_any(1_000.0, 40.0));
        let mut w = build(&view);
        layout_with_corners(
            &mut w,
            &BoxConstraints::loose(Size::new(400.0, 200.0)),
            top_corners(),
        );
        let title = &w.slots[0];
        assert_eq!(title.origin().x, 44.0 + TITLE_EDGE_INSET);
        assert_eq!(
            title.origin().x + title.size().width,
            400.0 - 52.0 - TITLE_EDGE_INSET
        );
    }

    #[test]
    fn center_title_centers_within_the_space_left_between_the_slots() {
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0)])
            .center_title(true);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));

        let leading_end = w.slots[0].origin().x + w.slots[0].size().width;
        let action_start = w.slots[2].origin().x;
        let title_start = w.slots[1].origin().x;
        let title_end = title_start + w.slots[1].size().width;
        let left_gap = title_start - leading_end;
        let right_gap = action_start - title_end;
        assert!(
            (left_gap - right_gap).abs() < 0.01,
            "the title is centered in the remaining space \
             (left_gap={left_gap}, right_gap={right_gap})"
        );
    }

    #[test]
    fn corner_shift_moves_leading_and_actions() {
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0), leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        let size = layout_with_corners(
            &mut w,
            &BoxConstraints::loose(Size::new(400.0, 200.0)),
            top_corners(),
        );
        assert_eq!(size.height, BAND);
        assert_eq!(w.slots[0].origin().x, PAD_X + 44.0);
        assert_eq!(
            w.slots[3].origin().x + w.slots[3].size().width,
            400.0 - PAD_X - 52.0
        );
    }

    #[test]
    fn corner_shift_opt_out_is_byte_identical_with_nonzero_corners() {
        let bc = BoxConstraints::loose(Size::new(400.0, 200.0));
        let mk = |shift: Option<bool>| {
            let mut view: AppBarView<()> = app_bar("Home")
                .leading(leaf_any(40.0, 40.0))
                .actions(vec![leaf_any(24.0, 24.0), leaf_any(24.0, 24.0)]);
            if let Some(b) = shift {
                view = view.corner_shift(b);
            }
            build(&view)
        };
        let mut plain = mk(None);
        let plain_size = layout(&mut plain, &bc);
        let mut opted = mk(Some(false));
        let opted_size = layout_with_corners(&mut opted, &bc, top_corners());
        assert_eq!(plain_size, opted_size);
        assert_eq!(plain.slots.len(), opted.slots.len());
        for (a, b) in plain.slots.iter().zip(opted.slots.iter()) {
            assert_eq!(a.origin(), b.origin());
            assert_eq!(a.size(), b.size());
        }
    }

    #[test]
    fn corner_shift_defaults_on() {
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0), leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        layout_with_corners(
            &mut w,
            &BoxConstraints::loose(Size::new(400.0, 200.0)),
            top_corners(),
        );
        assert_eq!(w.slots[0].origin().x, PAD_X + 44.0);
    }

    #[test]
    fn corner_with_zero_height_shifts_nothing() {
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        let corners = CornerInsets::new(
            CornerInset::new(44.0, 0.0),
            CornerInset::new(52.0, 0.0),
            CornerInset::ZERO,
            CornerInset::ZERO,
        );
        layout_with_corners(
            &mut w,
            &BoxConstraints::loose(Size::new(400.0, 200.0)),
            corners,
        );
        assert_eq!(w.slots[0].origin().x, PAD_X);
        assert_eq!(
            w.slots[2].origin().x + w.slots[2].size().width,
            400.0 - PAD_X
        );
    }

    #[test]
    fn center_title_still_centers_in_the_remaining_space_with_shifted_slots() {
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0)])
            .center_title(true);
        let mut w = build(&view);
        layout_with_corners(
            &mut w,
            &BoxConstraints::loose(Size::new(400.0, 200.0)),
            top_corners(),
        );
        let leading_end = w.slots[0].origin().x + w.slots[0].size().width;
        let action_start = w.slots[2].origin().x;
        let title_start = w.slots[1].origin().x;
        let title_end = title_start + w.slots[1].size().width;
        let left_gap = title_start - leading_end;
        let right_gap = action_start - title_end;
        assert!(
            (left_gap - right_gap).abs() < 0.01,
            "left_gap={left_gap}, right_gap={right_gap}"
        );
    }

    #[test]
    fn zero_corners_are_byte_identical() {
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0), leaf_any(24.0, 24.0)]);
        let bc = BoxConstraints::loose(Size::new(400.0, 200.0));
        let mut plain = build(&view);
        layout(&mut plain, &bc);
        let mut zeroed = build(&view);
        layout_with_corners(&mut zeroed, &bc, CornerInsets::ZERO);
        assert_eq!(plain.slots.len(), zeroed.slots.len());
        for (a, b) in plain.slots.iter().zip(zeroed.slots.iter()) {
            assert_eq!(a.origin(), b.origin());
            assert_eq!(a.size(), b.size());
        }
    }

    #[test]
    fn compact_density_and_an_explicit_height_both_move_the_band() {
        let compact: AppBarView<()> = app_bar("Home").density(AppBarDensity::Compact);
        let mut w = build(&compact);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size.height, BAND - super::super::COMPACT_REDUCTION);

        // The explicit override wins over the density table — the seam that
        // reaches upstream's own 72dp band.
        let tall: AppBarView<()> = app_bar("Home")
            .density(AppBarDensity::Compact)
            .toolbar_height(72.0);
        let mut w = build(&tall);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size.height, 72.0);
    }

    #[test]
    fn unthemed_paint_uses_fallback_container() {
        let view: AppBarView<()> = app_bar("Home");
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, BAND));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].1, Size::new(300.0, BAND));
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
        let mut scene = FillRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, BAND)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].2, theme.scheme().surface);
    }

    #[test]
    fn an_explicit_background_wins_over_the_theme_role() {
        let theme = crate::baseline();
        let custom = Color::from_rgb8(0x12, 0x34, 0x56);
        let view: AppBarView<()> = app_bar("Home").background(custom);
        let mut w = build(&view);
        layout_themed(
            &mut w,
            &BoxConstraints::loose(Size::new(300.0, 100.0)),
            &theme,
        );
        let mut scene = FillRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, BAND)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].2, custom);
    }

    // ---- Container role ---------------------------------------------------

    #[test]
    fn the_inverse_primary_container_follows_the_theme_brightness() {
        let view: AppBarView<()> = app_bar("Home").container(AppBarContainer::InversePrimary);
        let light = crate::baseline();
        let dark = dark_baseline();
        assert_eq!(
            paint_themed(&view, &light).rects[0].2,
            light.scheme().inverse_primary
        );
        assert_eq!(
            paint_themed(&view, &dark).rects[0].2,
            dark.scheme().inverse_primary
        );
        assert_ne!(
            light.scheme().inverse_primary,
            dark.scheme().inverse_primary
        );
    }

    #[test]
    fn every_container_role_resolves_its_own_scheme_color() {
        let theme = crate::baseline();
        let scheme = theme.scheme();
        for (role, expected) in [
            (AppBarContainer::Surface, scheme.surface),
            (AppBarContainer::SurfaceContainer, scheme.surface_container),
            (AppBarContainer::InversePrimary, scheme.inverse_primary),
            (AppBarContainer::PrimaryContainer, scheme.primary_container),
        ] {
            let view: AppBarView<()> = app_bar("Home").container(role);
            assert_eq!(paint_themed(&view, &theme).rects[0].2, expected, "{role:?}");
        }
    }

    #[test]
    fn only_the_primary_container_changes_the_title_role() {
        assert_eq!(
            app_bar::<()>("Home").title_role(),
            ThemeTextColor::OnSurface
        );
        for (role, expected) in [
            (AppBarContainer::Surface, ThemeTextColor::OnSurface),
            (AppBarContainer::SurfaceContainer, ThemeTextColor::OnSurface),
            (AppBarContainer::InversePrimary, ThemeTextColor::OnSurface),
            (
                AppBarContainer::PrimaryContainer,
                ThemeTextColor::OnPrimaryContainer,
            ),
        ] {
            assert_eq!(app_bar::<()>("Home").container(role).title_role(), expected);
        }
    }

    #[test]
    fn an_explicit_background_wins_over_container_and_elevation() {
        let theme = crate::baseline();
        let custom = Color::from_rgb8(0x12, 0x34, 0x56);
        let view: AppBarView<()> = app_bar("Home")
            .container(AppBarContainer::InversePrimary)
            .elevation(2)
            .background(custom);
        let scene = paint_themed(&view, &theme);
        assert_eq!(scene.rects[0].2, custom);
        // The background replaces the fill only; the shadow still paints.
        assert_eq!(scene.shadows.len(), 1);
    }

    // ---- Elevation --------------------------------------------------------

    #[test]
    fn elevation_two_paints_a_shadow_then_a_surface_container_fill() {
        let theme = crate::baseline();
        let view: AppBarView<()> = app_bar("Home").elevation(2);
        let scene = paint_themed(&view, &theme);

        assert_eq!(scene.ops[..2], ["shadow", "fill"], "shadow before the fill");
        let level = theme.elevation.level2;
        let spec = level.shadow(theme.brightness);
        let (origin, size, radius, blur, color) = scene.shadows[0];
        assert_eq!(origin, Point::new(0.0, spec.y_offset));
        assert_eq!(size, Size::new(300.0, BAND));
        assert_eq!(radius, 0.0);
        assert_eq!(blur, spec.blur_std_dev);
        assert_eq!(color.components[3], spec.color_alpha);
        assert_eq!((spec.blur_std_dev, spec.y_offset), (3.0, 2.5));

        // M3 tonal elevation: level 2's surface role.
        assert_eq!(level.surface_role, frust::SurfaceRole::SurfaceContainer);
        assert_eq!(scene.rects[0].2, theme.scheme().surface_container);
    }

    #[test]
    fn an_explicit_container_wins_over_the_elevation_surface_role() {
        let theme = crate::baseline();
        let view: AppBarView<()> = app_bar("Home")
            .elevation(2)
            .container(AppBarContainer::Surface);
        let scene = paint_themed(&view, &theme);
        assert_eq!(scene.rects[0].2, theme.scheme().surface);
        assert_eq!(scene.shadows.len(), 1);
    }

    #[test]
    fn elevation_zero_paints_no_shadow_and_large_levels_clamp() {
        let theme = crate::baseline();
        assert!(paint_themed(&app_bar("Home"), &theme).shadows.is_empty());

        let clamped = paint_themed(&app_bar("Home").elevation(9), &theme);
        let top = paint_themed(&app_bar("Home").elevation(5), &theme);
        assert_eq!(clamped.shadows, top.shadows);
        assert_eq!(clamped.rects[0].2, top.rects[0].2);
        assert_eq!(clamped.rects[0].2, theme.scheme().surface_container_highest);
    }

    #[test]
    fn an_unthemed_elevated_bar_uses_the_catalog_table_and_fallback_shadow() {
        let view: AppBarView<()> = app_bar("Home").elevation(2);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        let mut scene = FillRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, BAND));
        w.paint(&mut pctx, &mut scene);
        let (origin, _, _, blur, color) = scene.shadows[0];
        assert_eq!((origin.y, blur), (2.5, 3.0));
        assert_eq!(color, super::super::FALLBACK_SHADOW_COLOR);
        assert_eq!(
            scene.rects[0].2,
            crate::tokens::color_scheme_light().surface_container
        );
    }

    // ---- Self-inset (`safe_area`) -----------------------------------------

    #[test]
    fn safe_area_grows_the_band_by_the_top_inset_and_fills_it() {
        let theme = crate::baseline();
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0)])
            .container(AppBarContainer::InversePrimary)
            .elevation(2);
        let mut w = build(&view);
        let size = layout_with_top_inset(
            &mut w,
            &BoxConstraints::loose(Size::new(300.0, 200.0)),
            24.0,
        );
        assert_eq!(size, Size::new(300.0, BAND + 24.0));
        assert_eq!(w.top_inset, 24.0);
        // Every slot is centred in the band below the inset.
        assert_eq!(w.slots[0].origin().y, 24.0 + (BAND - 40.0) / 2.0);
        assert_eq!(w.slots[2].origin().y, 24.0 + (BAND - 24.0) / 2.0);
        let title = &w.slots[1];
        assert_eq!(title.origin().y, 24.0 + (BAND - title.size().height) / 2.0);

        // The fill and the shadow both cover the consumed band.
        let mut scene = FillRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].0, Point::ZERO);
        assert_eq!(scene.rects[0].1, Size::new(300.0, BAND + 24.0));
        assert_eq!(scene.rects[0].2, theme.scheme().inverse_primary);
        assert_eq!(scene.shadows[0].1, Size::new(300.0, BAND + 24.0));
    }

    #[test]
    fn safe_area_false_ignores_the_top_inset() {
        let view: AppBarView<()> = app_bar("Home").safe_area(false);
        let mut w = build(&view);
        let size = layout_with_top_inset(
            &mut w,
            &BoxConstraints::loose(Size::new(300.0, 200.0)),
            24.0,
        );
        assert_eq!(size.height, BAND);
        assert_eq!(w.top_inset, 0.0);
    }

    /// Lay out under an arbitrary `insets` value.
    fn layout_with_insets(w: &mut AppBarWidget, bc: &BoxConstraints, insets: WindowInsets) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        lctx.with_window_insets(insets, |ctx| w.layout(ctx, bc))
    }

    /// A landscape phone: a 47px cutout / home-indicator band on each side,
    /// no status bar.
    fn landscape() -> WindowInsets {
        WindowInsets::new(
            WindowEdgeInsets::new(47.0, 0.0, 47.0, 0.0),
            WindowEdgeInsets::ZERO,
        )
    }

    #[test]
    fn safe_area_pads_the_slots_inside_the_side_insets_and_fills_full_width() {
        let theme = crate::baseline();
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0), leaf_any(24.0, 24.0)])
            .elevation(2);
        let mut w = build(&view);
        let size = layout_with_insets(
            &mut w,
            &BoxConstraints::loose(Size::new(800.0, 200.0)),
            landscape(),
        );
        assert_eq!(size, Size::new(800.0, BAND));
        assert_eq!(w.side_insets, (47.0, 47.0));
        assert_eq!(w.slots[0].origin().x, 47.0 + PAD_X);
        assert_eq!(
            w.slots[3].origin().x + w.slots[3].size().width,
            800.0 - 47.0 - PAD_X
        );
        // The title starts one gap past the leading slot.
        assert_eq!(w.slots[1].origin().x, 47.0 + PAD_X + 40.0 + GAP);

        // The fill and the shadow still span the whole width.
        let mut scene = FillRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].0, Point::ZERO);
        assert_eq!(scene.rects[0].1, Size::new(800.0, BAND));
        assert_eq!(scene.shadows[0].0.x, 0.0);
        assert_eq!(scene.shadows[0].1, Size::new(800.0, BAND));
    }

    #[test]
    fn a_bare_title_keeps_the_m3_title_inset_inside_the_side_insets() {
        let view: AppBarView<()> = app_bar("").title_view(leaf_any(1_000.0, 40.0));
        let mut w = build(&view);
        layout_with_insets(
            &mut w,
            &BoxConstraints::loose(Size::new(800.0, 200.0)),
            landscape(),
        );
        let title = &w.slots[0];
        assert_eq!(title.origin().x, 47.0 + TITLE_EDGE_INSET);
        assert_eq!(
            title.origin().x + title.size().width,
            800.0 - 47.0 - TITLE_EDGE_INSET
        );
    }

    #[test]
    fn center_title_centers_between_the_slots_inside_the_side_insets() {
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0)])
            .center_title(true);
        let mut w = build(&view);
        layout_with_insets(
            &mut w,
            &BoxConstraints::loose(Size::new(800.0, 200.0)),
            WindowInsets::new(
                WindowEdgeInsets::new(47.0, 0.0, 10.0, 0.0),
                WindowEdgeInsets::ZERO,
            ),
        );
        let leading_end = w.slots[0].origin().x + w.slots[0].size().width;
        let title_start = w.slots[1].origin().x;
        let title_end = title_start + w.slots[1].size().width;
        let left_gap = title_start - leading_end;
        let right_gap = w.slots[2].origin().x - title_end;
        assert!(
            (left_gap - right_gap).abs() < 0.01,
            "left_gap={left_gap}, right_gap={right_gap}"
        );
    }

    #[test]
    fn the_corner_shift_stacks_beyond_the_consumed_side_padding() {
        // A corner's width is measured from the safe-area edge (see
        // `CornerInsets`), so the slot origin is padding + corner, never the
        // larger of the two — and it equals what the same bar gets under a
        // parent safe area that consumed the side padding first.
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0)]);
        let bc = BoxConstraints::loose(Size::new(800.0, 200.0));
        let insets = WindowInsets::new(
            WindowEdgeInsets::new(20.0, 24.0, 30.0, 0.0),
            WindowEdgeInsets::ZERO,
        )
        .with_corner_insets(top_corners());

        let mut w = build(&view);
        layout_with_insets(&mut w, &bc, insets);
        assert_eq!(w.slots[0].origin().x, 20.0 + 44.0 + PAD_X);
        assert_eq!(
            w.slots[2].origin().x + w.slots[2].size().width,
            800.0 - 30.0 - 52.0 - PAD_X
        );

        // The parent-consumed composition: the bar sees no padding, sits 20px
        // in, and still shifts for the (never consumed) corner.
        let mut nested = build(&view);
        layout_with_insets(
            &mut nested,
            &BoxConstraints::loose(Size::new(800.0 - 20.0 - 30.0, 200.0)),
            insets.consuming(true, true, true, false),
        );
        assert_eq!(nested.side_insets, (0.0, 0.0));
        assert_eq!(20.0 + nested.slots[0].origin().x, w.slots[0].origin().x);
        assert_eq!(
            20.0 + nested.slots[2].origin().x + nested.slots[2].size().width,
            w.slots[2].origin().x + w.slots[2].size().width
        );
    }

    #[test]
    fn a_side_consuming_safe_area_leaves_no_side_padding_to_consume_again() {
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        layout_with_insets(
            &mut w,
            &BoxConstraints::loose(Size::new(706.0, 200.0)),
            landscape().consuming(true, true, true, false),
        );
        assert_eq!(w.side_insets, (0.0, 0.0));
        assert_eq!(w.slots[0].origin().x, PAD_X);
        assert_eq!(
            w.slots[2].origin().x + w.slots[2].size().width,
            706.0 - PAD_X
        );

        // Through a real `frust::safe_area`: the bar is laid out 47px in and
        // the whole stack keeps the window width.
        let wrapped = frust::safe_area(app_bar::<()>("Home")).bottom(false);
        let mut counter = 0u64;
        let mut sw = View::<()>::build(&wrapped, &mut ctx(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        let size = lctx.with_window_insets(landscape(), |ctx| {
            sw.layout(ctx, &BoxConstraints::loose(Size::new(800.0, 400.0)))
        });
        assert_eq!(size.height, BAND);
    }

    #[test]
    fn safe_area_false_ignores_the_side_insets_too() {
        let view: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0)])
            .safe_area(false);
        let mut w = build(&view);
        let insets = WindowInsets::new(
            WindowEdgeInsets::new(47.0, 24.0, 47.0, 0.0),
            WindowEdgeInsets::ZERO,
        );
        let size = layout_with_insets(
            &mut w,
            &BoxConstraints::loose(Size::new(800.0, 200.0)),
            insets,
        );
        assert_eq!(size.height, BAND);
        assert_eq!(w.side_insets, (0.0, 0.0));
        assert_eq!(w.slots[0].origin().x, PAD_X);
        assert_eq!(
            w.slots[2].origin().x + w.slots[2].size().width,
            800.0 - PAD_X
        );
    }

    #[test]
    fn a_search_app_bar_self_insets_too() {
        let view: AppBarView<()> = search_app_bar(crate::search_bar("", |_: &mut (), _| {}));
        let mut w = build(&view);
        let size = layout_with_top_inset(
            &mut w,
            &BoxConstraints::loose(Size::new(300.0, 200.0)),
            24.0,
        );
        assert_eq!(size.height, BAND + 24.0);

        // And consumes the side insets: the pill sits inside them.
        let mut w = build(&view);
        layout_with_insets(
            &mut w,
            &BoxConstraints::loose(Size::new(800.0, 200.0)),
            landscape(),
        );
        let title = &w.slots[0];
        assert_eq!(title.origin().x, 47.0 + TITLE_EDGE_INSET);
        assert!(title.origin().x + title.size().width <= 800.0 - 47.0 - TITLE_EDGE_INSET);
    }

    #[test]
    fn a_top_consuming_safe_area_leaves_no_inset_to_consume_again() {
        // `safe_area(bar).bottom(false)` is how material3-demo and the gallery
        // dock the bar: the safe area pads 24 and hands the bar zero, so the
        // whole stack is 24 + 64, never 24 + 24 + 64.
        let wrapped = frust::safe_area(app_bar::<()>("Home")).bottom(false);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&wrapped, &mut ctx(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        let size = lctx.with_window_insets(status_bar(24.0), |ctx| {
            w.layout(ctx, &BoxConstraints::loose(Size::new(300.0, 400.0)))
        });
        assert_eq!(size.height, 24.0 + BAND);
    }

    #[test]
    fn rebuild_flags_container_and_elevation_as_paint_and_safe_area_as_layout() {
        let mut counter = 0u64;
        let prev: AppBarView<()> = app_bar("Home");
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));

        let next: AppBarView<()> = app_bar("Home").elevation(2);
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(flags.contains(ChangeFlags::PAINT));
        assert!(!flags.needs_layout());
        assert_eq!(w.elevation, 2);

        let prev = next;
        let next: AppBarView<()> = app_bar("Home")
            .elevation(2)
            .container(AppBarContainer::InversePrimary);
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(flags.contains(ChangeFlags::PAINT));
        assert_eq!(w.container, Some(AppBarContainer::InversePrimary));

        let prev = next;
        let next: AppBarView<()> = app_bar("Home")
            .elevation(2)
            .container(AppBarContainer::InversePrimary)
            .safe_area(false);
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_layout());
        assert!(!w.safe_area);
    }

    #[test]
    fn the_round_shape_family_paints_a_rounded_container() {
        let theme = crate::baseline();
        let view: AppBarView<()> = app_bar("Home").shape_family(AppBarShapeFamily::Round);
        let mut w = build(&view);
        layout_themed(
            &mut w,
            &BoxConstraints::loose(Size::new(300.0, 100.0)),
            &theme,
        );
        let mut scene = FillRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, BAND)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert!(
            scene.rects.is_empty(),
            "a round-family bar paints no plain rect"
        );
        assert_eq!(scene.rrects[0].2, theme.shape.small);
    }

    #[test]
    fn rebuild_adopts_new_title() {
        let mut counter = 0u64;
        let prev: AppBarView<()> = app_bar("Home");
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        let next: AppBarView<()> = app_bar("Settings");
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_layout());
        assert_eq!(w.label.as_deref(), Some("Settings"));
    }

    #[test]
    fn rebuild_adopts_a_new_leading_slot_and_shifts_the_title_index() {
        let mut counter = 0u64;
        let prev: AppBarView<()> = app_bar("Home").actions(vec![leaf_any(24.0, 24.0)]);
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        assert_eq!(w.slots.len(), 2);
        assert_eq!(w.title_index(), 0);

        let next: AppBarView<()> = app_bar("Home")
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0)]);
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_layout());
        assert_eq!(w.slots.len(), 3);
        assert_eq!(w.title_index(), 1);
    }

    #[test]
    fn a_search_titled_bar_fills_the_slot_between_leading_and_actions() {
        let view: AppBarView<()> = search_app_bar(crate::search_bar("", |_: &mut (), _| {}))
            .leading(leaf_any(40.0, 40.0))
            .actions(vec![leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));

        let leading_end = w.slots[0].origin().x + w.slots[0].size().width;
        let action_start = w.slots[2].origin().x;
        let title = &w.slots[1];
        assert_eq!(title.origin().x, leading_end + GAP);
        assert!(
            (title.origin().x + title.size().width - (action_start - GAP)).abs() < 0.01,
            "the search title fills the slot up to the first action's gap"
        );
        // Its own 56dp pill, centered in the 64dp band.
        assert_eq!(title.size().height, crate::SEARCH_BAR_MIN_HEIGHT);
        assert_eq!(
            title.origin().y,
            (BAND - crate::SEARCH_BAR_MIN_HEIGHT) / 2.0
        );

        // And it paints: the bar's own container band, then the embedded bar's
        // pill inside it (`search_bar` fills a rounded rect of its own).
        let theme = crate::baseline();
        let mut scene = FillRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, BAND)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].1, Size::new(400.0, BAND));
        assert_eq!(scene.rects[0].2, theme.scheme().surface);
        assert!(
            !scene.rrects.is_empty(),
            "the embedded search bar paints its own pill"
        );
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
        root.layout_with_text(Size::new(300.0, BAND), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TitleBar)
            .expect("a TitleBar node is contributed");
        assert_eq!(node.label(), Some("Inbox"));
    }

    #[test]
    fn an_explicit_semantic_label_wins_over_the_title() {
        fn logic(_state: &mut ()) -> AppBarView<()> {
            app_bar("Inbox").semantic_label("Mail, inbox folder")
        }
        let mut root: frust_core::RenderRoot<(), AppBarView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, BAND), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TitleBar)
            .expect("a TitleBar node is contributed");
        assert_eq!(node.label(), Some("Mail, inbox folder"));
    }

    // --- Render-time typeface identity ---

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_paints_in_roboto_flex_under_the_material_theme() {
        use super::typeface_probe::{Face, assert_paints_only_in};
        let flex = crate::tokens::font_data()[0];
        assert_paints_only_in(
            "the app bar title",
            |_: &mut ()| app_bar::<()>("Hello"),
            crate::baseline(),
            &[flex],
            Size::new(300.0, BAND),
            Face {
                bytes: flex,
                name: "Roboto Flex",
            },
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_follows_a_live_theme_family_change() {
        // The demo gallery's font picker rewrites every type-scale role's
        // family and pushes the theme; the title must follow it like the
        // buttons do, not stay pinned to the catalog's own Roboto Flex.
        use super::typeface_probe::{Face, assert_paints_only_in};
        use frust::authoring::text::{FontFamily, GenericSlot};
        let (flex, mono) = (crate::tokens::font_data()[0], crate::tokens::font_data()[1]);
        let mono_family = FontFamily::stack_with_generic(
            [crate::tokens::ROBOTO_MONO_FAMILY],
            GenericSlot::Monospace,
        );
        let mut theme = crate::baseline();
        macro_rules! every_role {
            ($($role:ident),+ $(,)?) => {
                $( theme.type_scale.$role.family = mono_family.clone(); )+
            };
        }
        every_role!(
            display_large,
            display_medium,
            display_small,
            headline_large,
            headline_medium,
            headline_small,
            title_large,
            title_medium,
            title_small,
            body_large,
            body_medium,
            body_small,
            label_large,
            label_medium,
            label_small,
            display_large_emphasized,
            display_medium_emphasized,
            display_small_emphasized,
            headline_large_emphasized,
            headline_medium_emphasized,
            headline_small_emphasized,
            title_large_emphasized,
            title_medium_emphasized,
            title_small_emphasized,
            body_large_emphasized,
            body_medium_emphasized,
            body_small_emphasized,
            label_large_emphasized,
            label_medium_emphasized,
            label_small_emphasized,
        );
        // Both faces registered, as `install()` does: the title must pick the
        // theme's Roboto Mono even with Roboto Flex available to it.
        assert_paints_only_in(
            "the app bar title",
            |_: &mut ()| app_bar::<()>("Hello"),
            theme,
            &[flex, mono],
            Size::new(300.0, BAND),
            Face {
                bytes: mono,
                name: "Roboto Mono",
            },
        );
    }
}

/// A render-time typeface probe for the Material components that build their
/// text from plain `text(..)`: lays a view out and paints it through a real
/// `RenderRoot` under a given theme, with only the given faces registered, and
/// checks each painted glyph run's font bytes against one exact face.
///
/// Registering a face proves nothing about what paints — a text run that never
/// asks for the theme's family paints the platform's system font right beside a
/// successfully registered Roboto Flex — so these tests assert on the runs.
///
/// Every identity assertion is paired with a control, because "every run is
/// the expected face" only means something where a run that does NOT opt in
/// would come out differently. The control paints a plain `text(..)` — the
/// `SystemUi` request an un-opted Material text makes — through the same
/// theme and faces, and requires that none of its runs is the expected face.
/// If a host ever resolved `SystemUi` to that face, the control fails, so an
/// identity test can never pass there without the behaviour it guards.
///
/// The control deliberately uses no process-wide fallback decoy
/// (`frust_text::register_generic_fallback`): that registry is append-only and
/// shared by every `TextContext` this test binary builds, so a decoy would
/// change other tests' `SystemUi` resolution depending on scheduling, and on a
/// host with system fonts it is appended after them and never reached anyway.
/// A host whose `SystemUi` has no face at all shapes zero runs for the control
/// (allowed) and for an un-opted component (which then fails the identity
/// assertion's at-least-one-run check).
#[cfg(all(test, feature = "bundled-fonts"))]
pub(crate) mod typeface_probe {
    use frust::Theme;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PaintScene, View};
    use frust_core::{FrameTime, RenderRoot};
    use kurbo::{Point, Size};
    use peniko::Color;
    use std::any::Any;

    /// Records, per painted glyph run, whether its font bytes are `expected`.
    struct FaceRecorder {
        expected: &'static [u8],
        matches: Vec<bool>,
    }

    impl PaintScene for FaceRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.matches
                .push(run.font.font().data.as_ref() == self.expected);
        }
    }

    /// Rebuilds `logic`'s view under `theme`, lays it out in `window` with a
    /// text context holding `faces`, paints it, and returns one entry per
    /// glyph run: `true` when that run shaped against `expected`'s bytes.
    fn painted_run_faces<V: View<()>>(
        mut logic: impl FnMut(&mut ()) -> V,
        theme: Theme,
        faces: &[&'static [u8]],
        window: Size,
        expected: &'static [u8],
    ) -> Vec<bool> {
        let mut tcx = TextContext::new();
        for face in faces {
            tcx.register_fonts(face.to_vec())
                .expect("a bundled Material face registers");
        }
        let mut root: RenderRoot<(), V> = RenderRoot::new();
        root.set_theme(Box::new(theme));
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(window, &mut tcx as &mut dyn Any);
        let mut recorder = FaceRecorder {
            expected,
            matches: Vec::new(),
        };
        root.paint(&mut recorder, FrameTime::ZERO);
        recorder.matches
    }

    /// The face every run must shape against: its exact bytes and a name for
    /// failure messages.
    pub(crate) struct Face {
        pub(crate) bytes: &'static [u8],
        pub(crate) name: &'static str,
    }

    /// Paints `logic`'s component under `theme` with `faces` registered and
    /// asserts it painted at least one glyph run, every one in `expected` —
    /// after first asserting the control (see the [module docs](self)): a plain
    /// `text(..)` painted the same way has no run in `expected`. Returns the
    /// component's run count, for a caller's own fixture checks.
    #[track_caller]
    pub(crate) fn assert_paints_only_in<V: View<()>>(
        what: &str,
        logic: impl FnMut(&mut ()) -> V,
        theme: Theme,
        faces: &[&'static [u8]],
        window: Size,
        expected: Face,
    ) -> usize {
        let control = painted_run_faces(
            |_: &mut ()| frust::text("Hello"),
            theme.clone(),
            faces,
            window,
            expected.bytes,
        );
        let leaked = control.iter().filter(|matched| **matched).count();
        assert_eq!(
            leaked,
            0,
            "control: {leaked} of {} run(s) of a plain text() that never opts in shaped \
             in {} — on this host an un-opted run is indistinguishable from the \
             expected face, so the identity assertion for {what} would prove nothing",
            control.len(),
            expected.name
        );

        let runs = painted_run_faces(logic, theme, faces, window, expected.bytes);
        assert!(!runs.is_empty(), "{what} painted no glyph run at all");
        let foreign = runs.iter().filter(|matched| !**matched).count();
        assert_eq!(
            foreign,
            0,
            "{foreign} of {} glyph run(s) in {what} shaped against a face other than \
             {} — the text is not asking for the theme's type-scale family",
            runs.len(),
            expected.name
        );
        runs.len()
    }
}
