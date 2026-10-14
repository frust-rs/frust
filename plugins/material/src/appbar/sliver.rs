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
//! # Two modes: a sliver in a viewport, a box under a controlled prop
//!
//! Upstream hands the whole job to Flutter's sliver protocol: `SliverAppBar`
//! participates in a `CustomScrollView`, which drives its extent every frame
//! from the shared scroll position. This bar does the same inside a
//! [`frust::custom_scroll_view`] whenever it is **uncontrolled** — neither
//! [`SliverAppBarView::scroll_offset`] nor [`SliverAppBarView::collapse`] set.
//! Its widget then answers [`Widget::as_sliver`] and lays out as a persistent
//! header: maximum extent the expanded band, minimum extent the collapsed band
//! (the [`AppBarCollapse`] metrics), [`pinned`](SliverAppBarView::pinned) by
//! default and [`floating`](SliverAppBarView::floating) on request. See
//! *In a viewport* below.
//!
//! **A controlled prop wins.** Set either one and the bar is the box widget it
//! has always been, wherever it sits: a pure function of the prop — the
//! app-side-rule shape [`crate::SearchViewMode::for_width`] takes, fed from
//! [`frust::ScrollView::on_scroll`] (see [`super`]'s wiring contract). Inside
//! a viewport such a bar answers `as_sliver` with `None`, so the viewport lays
//! it out as a box child ([`frust::sliver_box`]'s geometry: it scrolls away
//! with the content, pinned or not) and the viewport's scroll offset never
//! reaches it. A bar therefore never has two sources for one collapse, and an
//! app already wiring `on_scroll` into `scroll_offset(..)` is unaffected.
//! Outside a viewport an uncontrolled bar is that box at collapse `0.0`, fully
//! expanded.
//!
//! Every input is clamped and NaN-safe at the boundary ([`AppBarCollapse`]),
//! so a degenerate offset can never reach a layout.
//!
//! # In a viewport
//!
//! The sliver geometry — the scrolling, pinned, floating and floating-pinned
//! behaviours, a floating bar's own scroll offset, the overlap the next sliver
//! is told about — is not a second implementation here. The widget holds a
//! [`frust::sliver_persistent_header`] core over an empty child, configured
//! with the bar's two extents and its `pinned`/`floating` flags, answers the
//! viewport with that core's geometry, and lays its own slots out at the
//! collapse the core's shrink offset maps to through
//! [`AppBarCollapse::fraction_for_offset`]. That band is exactly the extent
//! the header paints in every behaviour: a pinned bar bottoms out at the
//! collapsed band, and an unpinned (scrolling or floating) bar's band keeps
//! shrinking with its top row bottom-anchored (*Geometry* below), which is the
//! header pushing its content out past the leading edge.
//!
//! The title's type scale is composed when the view builds, so — like a
//! persistent header's content — it follows the shrink offset one frame late:
//! layout records the new collapse, paint requests a frame when the title was
//! composed more than [`TITLE_REBUILD_TOLERANCE`] of headline travel away from
//! it (or the headline has just reached either end), and the next rebuild
//! composes it. The band, the slots and the headline's anchor follow at
//! layout, with no lag.
//!
//! The bar is meant for a vertical viewport; in a horizontal one it is laid out
//! across the viewport's cross extent as if that were its width.
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

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx, SliverConstraints, SliverGeometry,
    SliverWidget, View, ViewSeq, Widget, build_child, rebuild_children, route_event,
    teardown_child, visit_children,
};
use frust::{
    SizedBox, SliverPersistentHeaderDelegate, SliverPersistentHeaderView, Theme,
    sliver_persistent_header,
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

/// How far, in logical px of headline travel, the collapse a viewport laid the
/// bar out at may move away from the one its title was composed at before the
/// bar asks for a rebuild — the same half pixel `sliver_persistent_header`
/// allows its content (the [module docs](self)' *In a viewport*).
pub const TITLE_REBUILD_TOLERANCE: f64 = 0.5;

/// The persistent-header core a [`SliverAppBarWidget`] holds for its sliver
/// geometry. Named through the view's `Element` because the facade exports
/// the header's view but not its widget type.
type HeaderCore = <SliverPersistentHeaderView<()> as View<()>>::Element;

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
    floating: bool,
    /// `None` leaves the bar uncontrolled: a sliver in a viewport, expanded
    /// elsewhere (the [module docs](self)).
    collapse: Option<CollapseInput>,
    background: Option<Color>,
    semantic_label: Option<String>,
}

/// Create a collapsing app bar titled `title`, fully expanded — upstream's
/// `M3EAppBar.sliver`, with its own defaults ([`AppBarVariant::Medium`],
/// [`AppBarShapeFamily::Round`], pinned).
///
/// Place it in a [`frust::custom_scroll_view`] and it collapses with the
/// viewport's scroll offset; or feed it the offset your own body reports
/// through [`SliverAppBarView::scroll_offset`]. See [`super`]'s scroll-wiring
/// contract for both shapes.
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
        floating: false,
        collapse: None,
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
    pub fn leading(mut self, leading: impl View<State>) -> Self {
        self.leading = Some(AnyView::new(leading));
        self
    }

    /// Attach trailing action slots, in reading order. They stay in the top row
    /// at every collapse value.
    pub fn actions<M>(mut self, actions: impl ViewSeq<State, M>) -> Self {
        self.actions.clear();
        actions.extend_views(&mut self.actions);
        self
    }

    /// Replace the title string with a caller view — upstream's `title` widget
    /// slot. The view is placed (and moved between the expanded and collapsed
    /// anchors) but never styled, so it carries no type-scale interpolation of
    /// its own.
    pub fn title_view(mut self, title: impl View<State>) -> Self {
        self.title = TitleSlot::View(AnyView::new(title));
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
    /// In a viewport an unpinned bar shrinks to its collapsed band and then
    /// scrolls away with the content, as a scrolled-away `SliverAppBar` does.
    /// Upstream's `snap` companion is not ported — see [`super`]'s *Not
    /// ported* list.
    pub fn pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    /// Reveal the bar as soon as the user scrolls back toward the content's
    /// start, wherever the content is (`false` by default; no snap) —
    /// upstream's `floating`. Combined with [`Self::pinned`] the revealed bar
    /// never shrinks below its collapsed band.
    ///
    /// Takes effect only for an uncontrolled bar inside a viewport, which
    /// tracks the scroll direction for it (`sliver_persistent_header`'s
    /// floating behaviour); a controlled or box-laid-out bar ignores it.
    #[must_use]
    pub fn floating(mut self, floating: bool) -> Self {
        self.floating = floating;
        self
    }

    /// Set the collapse fraction directly: `0.0` fully expanded, `1.0` fully
    /// collapsed. Clamped (and NaN-mapped to `0.0`) on resolution.
    ///
    /// Makes the bar **controlled**: it stays a box even inside a viewport,
    /// whose scroll offset no longer collapses it (the [module docs](self)).
    pub fn collapse(mut self, collapse: f64) -> Self {
        self.collapse = Some(CollapseInput::Fraction(collapse));
        self
    }

    /// Set the collapse from a scroll offset in logical px — the `offset` an
    /// app reads off the [`ScrollInfo`](frust::ScrollInfo) its body reports.
    ///
    /// Mapped through [`AppBarCollapse::fraction_for_offset`] against this
    /// bar's own final geometry, so the call order relative to
    /// [`Self::variant`]/[`Self::density`]/[`Self::pinned`] does not matter.
    ///
    /// Makes the bar **controlled**, exactly as [`Self::collapse`] does.
    pub fn scroll_offset(mut self, offset: f64) -> Self {
        self.collapse = Some(CollapseInput::Offset(offset));
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

    /// The resolved collapse fraction, whichever way the caller stated it —
    /// `0.0`, fully expanded, for an uncontrolled bar.
    fn collapse_fraction(&self) -> f64 {
        match self.collapse {
            None => 0.0,
            Some(CollapseInput::Fraction(t)) => super::clamp01(t),
            Some(CollapseInput::Offset(offset)) => self.geometry().fraction_for_offset(offset),
        }
    }

    /// Whether a collapse prop is set (the [module docs](self)' precedence).
    fn controlled(&self) -> bool {
        self.collapse.is_some()
    }

    /// The persistent-header core's view: the bar's two bands as its extents,
    /// its `pinned`/`floating` flags, and an empty child — the bar lays its
    /// own slots out, so the core contributes geometry only.
    fn header_view(&self) -> SliverPersistentHeaderView<State> {
        let geometry = self.geometry();
        sliver_persistent_header(SliverPersistentHeaderDelegate::new(
            geometry.collapsed_height(),
            geometry.expanded_height(),
            |_shrink: f64, _overlaps: bool| SizedBox::<State>(None, None),
        ))
        .pinned(self.pinned)
        .floating(self.floating)
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
    /// The collapse fraction the live title was composed at (its type scale);
    /// behind [`Self::collapse`] by up to a frame inside a viewport.
    built_collapse: f64,
    /// Whether a collapse prop is set — a controlled bar is never a sliver.
    controlled: bool,
    /// The sliver geometry this bar answers a viewport with.
    header: HeaderCore,
    shape_family: AppBarShapeFamily,
    background: Option<Color>,
    label: Option<String>,
}

impl SliverAppBarWidget {
    /// Index of the title pod within [`Self::slots`].
    fn title_index(&self) -> usize {
        usize::from(self.has_leading)
    }

    /// Whether the live title was composed too far from the collapse the last
    /// layout found (the [module docs](self)' *In a viewport*): by at least
    /// [`TITLE_REBUILD_TOLERANCE`] of headline travel, or onto either end of
    /// it. Compared on the headline, which an unpinned bar saturates before
    /// its band stops shrinking.
    fn title_is_stale(&self) -> bool {
        let built = self.geometry.headline(self.built_collapse);
        let wanted = self.geometry.headline(self.collapse);
        if built == wanted {
            return false;
        }
        let span = self.geometry.expanded_height() - self.geometry.collapsed_height();
        (built - wanted).abs() * span >= TITLE_REBUILD_TOLERANCE || wanted <= 0.0 || wanted >= 1.0
    }

    /// Lay the slots out for a bar `width` wide at [`Self::collapse`] and
    /// return the band's height.
    fn layout_slots(&mut self, ctx: &mut LayoutCtx, width: f64) -> f64 {
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
        height
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
        let header = View::<State>::build(&self.header_view(), ctx);
        SliverAppBarWidget {
            slots,
            has_leading: self.leading.is_some(),
            center_title: self.center_title,
            corner_shift: self.corner_shift,
            geometry,
            collapse,
            built_collapse: collapse,
            controlled: self.controlled(),
            header,
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
        // The previous title is the one the live pod holds: composed at
        // `built_collapse`. The next one composes at the prop when controlled,
        // and otherwise at the collapse the last layout found once the live
        // title has fallen far enough behind it.
        let prev_geometry = prev.geometry();
        let (prev_size, prev_line) =
            title_type_scale(prev_geometry.headline(element.built_collapse));
        let geometry = self.geometry();
        let controlled = self.controlled();
        let built = if controlled {
            self.collapse_fraction()
        } else if element.title_is_stale() {
            element.collapse
        } else {
            element.built_collapse
        };
        let (size, line_height) = title_type_scale(geometry.headline(built));

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
        element.built_collapse = built;
        // A controlled collapse drives the band, the headline anchor, and the
        // clip, so a changed value is a relayout even when the title view
        // itself reconciled clean (an unstyled `title_view`). An uncontrolled
        // bar's collapse belongs to its layout.
        let collapse = if controlled {
            self.collapse_fraction()
        } else {
            element.collapse
        };
        if element.geometry != geometry
            || element.collapse != collapse
            || element.controlled != controlled
        {
            element.geometry = geometry;
            element.collapse = collapse;
            element.controlled = controlled;
            flags |= ChangeFlags::LAYOUT;
        }
        flags |= View::<State>::rebuild(
            &self.header_view(),
            &prev.header_view(),
            &mut element.header,
            ctx,
        );
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
        let (size, line_height) = title_type_scale(geometry.headline(element.built_collapse));
        let mut storage = None;
        let title = title_ref(&self.title, &mut storage, size, line_height);
        for (view, pod) in slot_views(self, title)
            .into_iter()
            .zip(element.slots.iter_mut())
        {
            teardown_child(view, pod, ctx);
        }
        View::<State>::teardown(&self.header_view(), &mut element.header, ctx);
    }
}

impl Widget for SliverAppBarWidget {
    /// The box layout: a controlled bar at its prop, an uncontrolled one fully
    /// expanded (the [module docs](self)).
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        if !self.controlled {
            self.collapse = 0.0;
        }
        let width = finite_or_zero(bc.max().width);
        let height = self.layout_slots(ctx, width);
        bc.constrain(Size::new(width, height))
    }

    /// Paints the band and the slots, and inside a viewport requests the frame
    /// whose rebuild composes the title at the collapse this frame's layout
    /// found.
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
        if !self.controlled && self.title_is_stale() {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.slots, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        push_bar_semantics(ctx, Role::TitleBar, self.label.as_deref(), &self.slots);
    }

    /// A sliver exactly while uncontrolled (the [module docs](self)).
    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        if self.controlled { None } else { Some(self) }
    }

    // The header core's empty child is an implementation detail, not part of
    // the inspectable tree.
    visit_children!(slots);
}

impl SliverWidget for SliverAppBarWidget {
    /// The header core's geometry, with the slots laid out at the collapse its
    /// shrink offset maps to (the [module docs](self)' *In a viewport*).
    fn layout_sliver(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        let geometry = self.header.layout_sliver(ctx, sc);
        self.collapse = self
            .geometry
            .fraction_for_offset(self.header.shrink_offset());
        self.layout_slots(ctx, finite_or_zero(sc.cross_axis_extent));
        geometry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{CornerInset, CornerInsets, WindowInsets};
    use frust::authoring::{ScrollAxis, ScrollDirection};
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

    // -----------------------------------------------------------------------
    // In a viewport.
    // -----------------------------------------------------------------------

    /// The large bar's expanded band.
    const LARGE: f64 = 152.0;

    /// Vertical sliver constraints for a bar at the top of a 400x600 viewport
    /// scrolled to `scroll_offset`, moving in `direction`.
    fn sliver_constraints(scroll_offset: f64, direction: ScrollDirection) -> SliverConstraints {
        SliverConstraints {
            axis: ScrollAxis::Vertical,
            scroll_offset,
            preceding_scroll_extent: 0.0,
            overlap: 0.0,
            remaining_paint_extent: 600.0,
            cross_axis_extent: 400.0,
            viewport_main_axis_extent: 600.0,
            cache_origin: -scroll_offset.min(250.0),
            remaining_cache_extent: 850.0 + scroll_offset.min(250.0),
            user_scroll_direction: direction,
        }
    }

    /// A sliver layout pass at `scroll_offset` with no user direction, its
    /// geometry checked against the protocol.
    fn layout_at(w: &mut SliverAppBarWidget, scroll_offset: f64) -> SliverGeometry {
        layout_moving(w, scroll_offset, ScrollDirection::Idle)
    }

    fn layout_moving(
        w: &mut SliverAppBarWidget,
        scroll_offset: f64,
        direction: ScrollDirection,
    ) -> SliverGeometry {
        let sc = sliver_constraints(scroll_offset, direction);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        let geometry = w.layout_sliver(&mut lctx, &sc);
        geometry.debug_assert_valid(&sc);
        geometry
    }

    /// Whether painting `w` at `paint_extent` asks for another frame.
    fn paint_requests_frame(w: &mut SliverAppBarWidget, paint_extent: f64) -> bool {
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, paint_extent));
        w.paint(&mut pctx, &mut ClipRecorder::default());
        pctx.needs_frame()
    }

    #[test]
    fn only_an_uncontrolled_bar_is_a_sliver() {
        let mut uncontrolled = build(&sliver_app_bar("Inbox"));
        assert!(uncontrolled.as_sliver().is_some());
        let mut by_offset = build(&sliver_app_bar("Inbox").scroll_offset(0.0));
        assert!(by_offset.as_sliver().is_none());
        let mut by_fraction = build(&sliver_app_bar("Inbox").collapse(0.5));
        assert!(by_fraction.as_sliver().is_none());
    }

    #[test]
    fn in_a_viewport_the_bar_collapses_as_the_offset_grows() {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Large)
            .leading(leaf_any(40.0, 40.0));
        let mut w = build(&view);
        let mut last_band = f64::INFINITY;
        for (offset, band) in [(0.0, LARGE), (22.0, 130.0), (44.0, 108.0), (88.0, ROW)] {
            let g = layout_at(&mut w, offset);
            assert_eq!(g.scroll_extent, LARGE, "scroll extent at {offset}");
            assert_eq!(g.paint_extent, band, "painted band at {offset}");
            assert_eq!(
                w.geometry.height(w.collapse),
                band,
                "laid-out band at {offset}"
            );
            assert!(band < last_band, "the band shrinks with the offset");
            last_band = band;
            // The leading slot stays centred in the top row throughout.
            assert_eq!(w.slots[0].origin().y, (ROW - 40.0) / 2.0);
        }
        assert_eq!(w.collapse, 1.0);
        // Fully collapsed, the headline sits in the top row beside the
        // leading slot.
        let title = &w.slots[w.title_index()];
        assert_eq!(title.origin().y, (ROW - title.size().height) / 2.0);
    }

    #[test]
    fn a_pinned_bar_in_a_viewport_stays_at_its_collapsed_band() {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox").variant(AppBarVariant::Large);
        let mut w = build(&view);
        for offset in [88.0, 120.0, 151.0, 300.0, 5_000.0] {
            let g = layout_at(&mut w, offset);
            assert_eq!(g.paint_extent, ROW, "painted band at {offset}");
            assert_eq!(g.paint_origin, 0.0, "stays at the leading edge at {offset}");
            assert_eq!(g.layout_extent, (LARGE - offset).max(0.0), "at {offset}");
            assert_eq!(w.collapse, 1.0, "collapse at {offset}");
        }
    }

    #[test]
    fn an_unpinned_bar_in_a_viewport_shrinks_then_scrolls_away() {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Large)
            .pinned(false)
            .leading(leaf_any(40.0, 40.0));
        let mut w = build(&view);
        let g = layout_at(&mut w, 44.0);
        assert_eq!(g.paint_extent, 108.0);
        assert_eq!(w.slots[0].origin().y, (ROW - 40.0) / 2.0);
        // Past the collapsed band the top row leaves with the content.
        let g = layout_at(&mut w, 120.0);
        assert_eq!(g.paint_extent, 32.0);
        assert_eq!(w.geometry.height(w.collapse), 32.0);
        assert_eq!(w.slots[0].origin().y, 32.0 - ROW + (ROW - 40.0) / 2.0);
        let g = layout_at(&mut w, 400.0);
        assert!(!g.visible);
        assert_eq!(g.paint_extent, 0.0);
    }

    #[test]
    fn a_floating_bar_reveals_on_a_forward_user_scroll() {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Large)
            .pinned(false)
            .floating(true);
        let mut w = build(&view);
        assert_eq!(layout_at(&mut w, 1_000.0).paint_extent, 0.0);
        // Back without a user direction (a controller jump): still hidden.
        assert_eq!(layout_at(&mut w, 960.0).paint_extent, 0.0);
        // Back under a forward user scroll: revealed by as much.
        let g = layout_moving(&mut w, 920.0, ScrollDirection::Forward);
        assert_eq!(g.paint_extent, 40.0);
        assert_eq!(w.geometry.height(w.collapse), 40.0);

        // Floating and pinned: never below the collapsed band.
        let pinned: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Large)
            .floating(true);
        let mut w = build(&pinned);
        assert_eq!(layout_at(&mut w, 1_000.0).paint_extent, ROW);
        let g = layout_moving(&mut w, 900.0, ScrollDirection::Forward);
        assert_eq!(g.paint_extent, 100.0);
        assert_eq!(w.geometry.height(w.collapse), 100.0);
    }

    #[test]
    fn the_title_follows_the_viewport_collapse_one_frame_late() {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox").variant(AppBarVariant::Large);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut ctx(&mut counter));
        layout_at(&mut w, 0.0);
        assert!(!paint_requests_frame(&mut w, LARGE));

        // Layout moves the band at once; the title still holds the expanded
        // type scale, so paint asks for the frame that recomposes it.
        layout_at(&mut w, 88.0);
        assert_eq!((w.collapse, w.built_collapse), (1.0, 0.0));
        assert!(paint_requests_frame(&mut w, ROW));
        let flags = View::<()>::rebuild(&view, &view, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_layout(), "the recomposed title relays out");
        assert_eq!(w.built_collapse, 1.0);
        layout_at(&mut w, 88.0);
        assert!(!paint_requests_frame(&mut w, ROW));

        // A move below the tolerance keeps the composed title.
        layout_at(&mut w, 87.8);
        assert!(!w.title_is_stale());
        View::<()>::rebuild(&view, &view, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.built_collapse, 1.0);
        // Back onto the expanded end always recomposes.
        layout_at(&mut w, 0.0);
        assert!(paint_requests_frame(&mut w, LARGE));
        View::<()>::rebuild(&view, &view, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.built_collapse, 0.0);
    }

    #[test]
    fn a_controlled_bar_never_requests_a_recomposing_frame() {
        let view: SliverAppBarView<()> = sliver_app_bar("Inbox")
            .variant(AppBarVariant::Large)
            .scroll_offset(44.0);
        let mut w = build(&view);
        let size = layout(&mut w, 400.0);
        assert!(!paint_requests_frame(&mut w, size.height));
    }

    /// The view function a [`Viewport`] rebuilds each frame.
    type ViewportLogic = Box<dyn FnMut(&mut ()) -> frust::CustomScrollView<()>>;

    /// A 400x600 `custom_scroll_view` holding `bar` above a 100-row list,
    /// driven by a `ScrollController`, with the layout audit on.
    struct Viewport {
        root: frust_core::RenderRoot<(), frust::CustomScrollView<()>>,
        logic: ViewportLogic,
        controller: frust::ScrollController,
        tcx: TextContext,
        ms: u64,
    }

    impl Viewport {
        fn new(bar: fn() -> SliverAppBarView<()>) -> Self {
            let controller = frust::ScrollController::new();
            let attached = controller.clone();
            let mut root = frust_core::RenderRoot::new();
            root.set_layout_audit(true);
            let mut viewport = Self {
                root,
                logic: Box::new(move |_: &mut ()| {
                    frust::custom_scroll_view((
                        bar(),
                        frust::sliver_fixed_extent_list(100, 50.0, |_| {
                            frust_widgets::test_support::leaf(10.0, 50.0)
                        }),
                    ))
                    .controller(attached.clone())
                }),
                controller,
                tcx: TextContext::new(),
                ms: 0,
            };
            viewport.frames(2);
            viewport
        }

        fn frames(&mut self, count: usize) {
            for _ in 0..count {
                self.root.rebuild(&mut self.logic, &mut ());
                self.root
                    .layout_with_text(Size::new(400.0, 600.0), &mut self.tcx as &mut dyn Any);
                self.ms += 16;
                self.root.paint(
                    &mut frust::authoring::DiscardScene,
                    frust_core::FrameTime::from_nanos(self.ms * 1_000_000),
                );
            }
        }

        fn jump(&mut self, offset: f64) {
            self.controller.jump_to(offset);
            self.frames(2);
        }

        /// The bar's window bounds.
        fn bar(&self) -> kurbo::Rect {
            self.root
                .inspect()
                .iter()
                .find(|node| node.type_name.contains("SliverAppBarWidget"))
                .expect("the bar is laid out")
                .bounds
        }
    }

    #[test]
    fn in_a_custom_scroll_view_an_uncontrolled_bar_follows_the_offset() {
        let mut viewport = Viewport::new(|| sliver_app_bar("Inbox").variant(AppBarVariant::Large));
        assert_eq!(viewport.bar().height(), LARGE);
        viewport.jump(44.0);
        assert_eq!(viewport.bar().y0, 0.0);
        assert_eq!(viewport.bar().height(), 108.0);
        viewport.jump(1_000.0);
        assert_eq!(viewport.bar().y0, 0.0, "pinned at the leading edge");
        assert_eq!(viewport.bar().height(), ROW, "pinned at the collapsed band");
        viewport.jump(0.0);
        assert_eq!(viewport.bar().height(), LARGE);
        assert_eq!(viewport.root.layout_audit_mismatches(), 0);
    }

    #[test]
    fn in_a_custom_scroll_view_a_controlled_prop_wins_over_the_offset() {
        // The prop pins the bar fully expanded: the viewport lays it out as a
        // box child that scrolls away with the content, never collapsing it.
        let mut viewport = Viewport::new(|| {
            sliver_app_bar("Inbox")
                .variant(AppBarVariant::Large)
                .scroll_offset(0.0)
        });
        viewport.jump(44.0);
        assert_eq!(viewport.bar().height(), LARGE);
        assert_eq!(viewport.bar().y0, -44.0);
        assert_eq!(viewport.root.layout_audit_mismatches(), 0);
    }
}
