//! `CustomScrollView`: a scroll surface whose children are slivers — Flutter's
//! `CustomScrollView` over `RenderViewport` (`rendering/viewport.dart`).
//!
//! A [`ScrollView`](crate::ScrollView) lays its one child out at unbounded
//! height and scrolls the result. A [`CustomScrollView`] instead lays each of
//! its children out with the sliver protocol ([`SliverConstraints`] in,
//! [`SliverGeometry`] out; see [`frust_core::SliverWidget`]): every child is
//! told where the viewport has scrolled to and how much paint space is left,
//! so a sliver can lay out only what is visible or inside the cache area. The
//! drag, fling, overscroll, nested-claim and controller machinery is the
//! shared [`crate::scroll_core`] that `ScrollView` and `ListView` run on, so all
//! three behave the same under the finger, the wheel and a
//! [`ScrollController`].
//!
//! # The layout loop
//!
//! [`ViewportWidget`]'s layout is `RenderViewport.performLayout` for a viewport
//! anchored at its leading edge:
//!
//! 1. The main-axis extent is the incoming constraint along the scroll axis;
//!    the cross-axis extent the constraint across it. The cache area reaches
//!    [`CustomScrollView::cache_extent`] past both ends of the visible region.
//! 2. Children are laid out in order. Each gets a [`SliverConstraints`] built
//!    from the running state: `scroll_offset = max(0, offset − preceding)`,
//!    the preceding scroll extent, the overlap left by earlier slivers' paint
//!    (`max_paint_offset − layout_offset`), the paint extent still unclaimed,
//!    the cache window `[offset − cache, offset + extent + cache)` expressed
//!    from the child's own scroll offset (`cache_origin` never reaching before
//!    the child's start, `remaining_cache_extent` measured from there), and
//!    the user's scroll direction. Every value is sanitised
//!    first (NaN dropped, negatives clamped to zero, the cache origin kept
//!    non-positive), so a transient zero or NaN window size can never trip
//!    [`SliverConstraints::debug_assert_valid`].
//! 3. A child answers through [`ChildPod::layout_sliver_child`], which applies
//!    the per-pod layout skip. A child that is not a sliver answers `None` and
//!    is laid out as a box through the same geometry as [`crate::sliver_box`]
//!    (tight across, unbounded along, painted at `−scroll_offset` inside its
//!    paint extent and clipped when cut).
//! 4. A child reporting a
//!    [`scroll_offset_correction`](SliverGeometry::scroll_offset_correction)
//!    ends the attempt: the correction is applied to the scroll position (and
//!    to the drag accumulator, so a live drag continues from the corrected
//!    position; a running release simulation restarts from it at its current
//!    velocity, while a controller tween keeps its absolute target, as
//!    Flutter's does) and the loop starts again. The restarts are bounded at
//!    ten cycles: a sliver still correcting on the last one trips a
//!    `debug_assert!`, and a release build keeps that cycle's layout,
//!    correction ignored.
//! 5. Each child's pod is placed at the running paint position (plus its
//!    `paint_origin`), and the maximum scroll extent — the sum of every
//!    child's scroll extent less the viewport extent, never negative — is
//!    published to the core, the attached controller and [`ScrollInfo`].
//!
//! After the loop, a position the new extent leaves out of range is clamped
//! (outside a live drag, settle or simulation, exactly as `ScrollView` does)
//! and any queued controller command is applied; either one moving the
//! position runs the loop once more.
//!
//! Under an unbounded main axis the viewport shrink-wraps: every child is
//! given infinite remaining paint extent and the viewport sizes itself to the
//! sum of the children's scroll extents, so nothing scrolls. An unbounded
//! cross axis lays children out at zero cross extent, as `ScrollView` treats
//! an unbounded width.
//!
//! # Placement is a layout product
//!
//! A child's origin is decided by the loop alone, never adjusted at paint:
//! moving a sliver changes its constraints, which only a layout can answer.
//! Motion the paint pump drives — a fling, a release settle, a controller
//! tween — therefore shows up on the frame after the pump moved the position:
//! the paint that moved it requests a layout
//! ([`PaintCtx::request_layout`]), and that frame lays out at the new
//! position. Input-driven motion (a drag, the wheel) and a jump drained at
//! layout have no such lag.
//!
//! # Paint, hit testing and semantics
//!
//! Paint clips to the viewport, publishes it as the visible rect
//! ([`PaintCtx::constrain_visible_rect`]) so a `Flex` or a list inside a sliver
//! can cull, and skips every child whose geometry is not
//! [`visible`](SliverGeometry::visible) — a child laid out only because it
//! sits in the cache area is never painted. Children paint last to first, so
//! an earlier sliver paints over a later one it overlaps (Flutter's paint
//! order), and are hit-tested first to last, each only within its
//! [`hit_test_extent`](SliverGeometry::hit_test_extent) and the viewport's
//! bounds. Semantics is a [`Role::ScrollView`] node carrying the scroll
//! position and range along the axis, over the children that are visible or
//! cached.
//!
//! # Which children are slivers
//!
//! A child is a sliver when its widget answers [`Widget::as_sliver`]. The
//! transparent wrappers forward that answer to what they wrap, so a sliver
//! stays a sliver inside them: [`Either`](crate::Either) and
//! [`EitherOf3`](crate::EitherOf3) through [`EitherOf8`](crate::EitherOf8),
//! [`hero`](crate::hero), [`shield`](crate::shield),
//! [`auto_scroll_zone`](crate::auto_scroll_zone) and
//! [`GestureDetector`](crate::GestureDetector) — and an [`AnyView`] or a
//! component boundary, which forward through the boxed widget. Any other
//! single-child wrapper (`Padding`, `Align`, `SizedBox`, a container, …)
//! answers `None`, so the viewport lays it out as a box, exactly as
//! [`crate::sliver_box`] would, whatever it wraps.
//!
//! # Axis
//!
//! [`CustomScrollView::axis`] picks vertical (the default) or horizontal
//! scrolling. The shared core is built with the viewport's axis and reads
//! pointer travel and wheel deltas along it, so every event reaches the core
//! untouched. The one transposition left is the stretch overscroll effect: the
//! core's stretch transform is vertical-only, so a horizontal viewport mirrors
//! its origin and size through a swap around it.
//!
//! **Nested claims are axis-aware.** The ambient inner-scroll claim
//! (`scroll.rs`'s *Nested scrolling*) carries no axis, and every other scroll
//! surface is vertical. A viewport therefore registers into its nearest
//! enclosing surface's claim only when the two scroll along the same axis, and
//! discards a claim registered into its own cell by a surface on the other
//! axis. A horizontal carousel inside a vertical list thus never makes the
//! list defer a vertical drag to it, and vice versa.
//!
//! # Limits
//!
//! Growth is forward only: slivers are laid out from the leading edge, in the
//! axis' natural direction ([`frust_core::GrowthDirection::Forward`]). There is
//! no center anchor (Flutter's `center`/`anchor`), so nothing can grow toward
//! the leading edge, and no overlap absorber (Flutter's
//! `SliverOverlapAbsorber`/`NestedScrollView`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase, ScrollAxis,
    ScrollDirection, SemanticsCtx, SliverConstraints, SliverGeometry, View, ViewSeq, Widget,
};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, Size};

use crate::authoring::presses;
use crate::physics::effect::OverscrollEffect;
use crate::physics::{ScrollPhysics, default_overscroll_effect};
use crate::scroll::{InnerScrollState, ScrollInfo, ambient_scroll_claim};
use crate::scroll_controller::ScrollController;
use crate::scroll_core::{BallisticState, ScrollSurface, ScrollSurfaceCore};
use crate::slivers::adapter::{
    axis_point, box_child_origin, box_geometry, finite_extent, non_negative,
};

/// How far past each end of the visible region a [`CustomScrollView`] lays
/// children out by default, in logical px — Flutter's
/// `RenderAbstractViewport.defaultCacheExtent`.
pub const DEFAULT_CACHE_EXTENT: f64 = 250.0;

/// How many times one layout may restart for a
/// [`scroll_offset_correction`](SliverGeometry::scroll_offset_correction)
/// before the viewport gives up on converging (the module docs' *The layout
/// loop*).
const MAX_LAYOUT_CYCLES: usize = 10;

/// A view-held scroll-observation callback (erased on build).
type OnScroll<State> = Rc<dyn Fn(&mut State, ScrollInfo)>;

/// A view-held pull-to-refresh release callback (erased on build).
type OnRefresh<State> = Rc<dyn Fn(&mut State)>;

/// A declarative scroll surface over a sequence of slivers. See the
/// [module docs](self).
pub struct CustomScrollView<State: 'static> {
    slivers: Vec<AnyView<State>>,
    axis: ScrollAxis,
    cache_extent: f64,
    on_scroll: Option<OnScroll<State>>,
    on_refresh_release: Option<OnRefresh<State>>,
    physics: Option<Rc<dyn ScrollPhysics>>,
    effect: OverscrollEffect,
    controller: Option<ScrollController>,
}

/// A vertical [`CustomScrollView`] over `slivers`.
///
/// `slivers` is any [`ViewSeq`]: a `Vec`/array of views, a tuple of mixed
/// view types, an `Option` (`None` drops the slot), or `views(iter)`. A child
/// that is not a sliver is laid out as a box through the
/// [`sliver_box`](crate::sliver_box) geometry, so plain views mix freely with
/// slivers.
///
/// ```
/// use frust_widgets::{CustomScrollView, custom_scroll_view, sliver_box, text};
/// let view: CustomScrollView<()> = custom_scroll_view((text("a"), sliver_box(text("b"))));
/// # let _ = view;
/// ```
pub fn custom_scroll_view<State: 'static, M>(
    slivers: impl ViewSeq<State, M>,
) -> CustomScrollView<State> {
    let mut list = Vec::new();
    slivers.extend_views(&mut list);
    CustomScrollView {
        slivers: list,
        axis: ScrollAxis::Vertical,
        cache_extent: DEFAULT_CACHE_EXTENT,
        on_scroll: None,
        on_refresh_release: None,
        physics: None,
        effect: default_overscroll_effect(),
        controller: None,
    }
}

impl<State: 'static> CustomScrollView<State> {
    /// Scroll along `axis` — [`ScrollAxis::Vertical`] by default. See the
    /// module docs' *Axis*.
    ///
    /// ```
    /// use frust_core::ScrollAxis;
    /// use frust_widgets::{CustomScrollView, custom_scroll_view, text};
    /// let view: CustomScrollView<()> =
    ///     custom_scroll_view((text("a"), text("b"))).axis(ScrollAxis::Horizontal);
    /// # let _ = view;
    /// ```
    pub fn axis(mut self, axis: ScrollAxis) -> Self {
        self.axis = axis;
        self
    }

    /// Lay out children up to `extent` logical px past each end of the visible
    /// region ([`DEFAULT_CACHE_EXTENT`] unless set), so content about to
    /// scroll in is already laid out. A cached child is laid out but not
    /// painted. A negative, NaN or infinite value is treated as `0.0`.
    pub fn cache_extent(mut self, extent: f64) -> Self {
        self.cache_extent = extent;
        self
    }

    /// Observe scroll position changes — the same contract as
    /// [`ScrollView::on_scroll`](crate::ScrollView::on_scroll): fired on each
    /// input-driven change, one event late for paint-driven motion, and
    /// flushed on the next frame for a controller jump or tween.
    pub fn on_scroll<F: Fn(&mut State, ScrollInfo) + 'static>(mut self, callback: F) -> Self {
        self.on_scroll = Some(Rc::new(callback));
        self
    }

    /// The pull-to-refresh trigger — the same contract as
    /// [`ScrollView::on_refresh_release`](crate::ScrollView::on_refresh_release):
    /// fires on pointer `Up` when the surface was pulled past its leading edge
    /// by more than the shared trigger distance, never on a `Cancel`.
    pub fn on_refresh_release<F: Fn(&mut State) + 'static>(mut self, callback: F) -> Self {
        self.on_refresh_release = Some(Rc::new(callback));
        self
    }

    /// Install a custom [`ScrollPhysics`] — the same build/rebuild contract as
    /// [`ScrollView::physics`](crate::ScrollView::physics): a view carrying one
    /// reinstalls it every rebuild, a view with none leaves the installed
    /// physics alone, and a fresh build starts at the platform default.
    ///
    /// ```
    /// use frust_widgets::{Clamping, CustomScrollView, custom_scroll_view, text};
    /// let view: CustomScrollView<()> = custom_scroll_view(text("a")).physics(Clamping::new());
    /// # let _ = view;
    /// ```
    pub fn physics(mut self, physics: impl ScrollPhysics + 'static) -> Self {
        self.physics = Some(Rc::new(physics));
        self
    }

    /// Select how past-edge pull is visualized — the same contract as
    /// [`ScrollView::overscroll_effect`](crate::ScrollView::overscroll_effect).
    /// Under [`OverscrollEffect::Translate`] the pull moves the slivers (they
    /// are laid out at the out-of-range position); the other two lay them out
    /// at the in-range one.
    pub fn overscroll_effect(mut self, effect: OverscrollEffect) -> Self {
        self.effect = effect;
        self
    }

    /// Attach a [`ScrollController`] — the same build/rebuild contract as
    /// [`ScrollView::controller`](crate::ScrollView::controller). Item commands
    /// ([`ScrollController::scroll_to_item`]) are list-only and ignored here.
    pub fn controller(mut self, controller: ScrollController) -> Self {
        self.controller = Some(controller);
        self
    }
}

/// What the last layout decided for one child.
#[derive(Clone, Copy, Debug)]
struct Slot {
    /// The geometry the child reported (or the box adapter computed).
    geometry: SliverGeometry,
    /// Where the child's paint starts along the scroll axis, in viewport space.
    paint_offset: f64,
    /// Whether the child was laid out as a box through the adapter geometry.
    is_box: bool,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            geometry: SliverGeometry::ZERO,
            paint_offset: 0.0,
            is_box: false,
        }
    }
}

/// The retained widget for a [`CustomScrollView`]: the children's pods and
/// what the last layout decided for each, the scroll position, and the shared
/// scroll core (the module docs).
pub struct ViewportWidget {
    children: Vec<ChildPod>,
    slots: Vec<Slot>,
    axis: ScrollAxis,
    /// The view's cache extent, sanitised to a finite non-negative value.
    cache_extent: f64,
    /// The effective scroll position, past-edge displacement included —
    /// `ScrollWidget`'s storage shape: a drag past an edge lets it run out of
    /// range, and the clamped paths go through [`ViewportWidget::set_offset`].
    offset: f64,
    /// This widget's own size from the last layout.
    viewport: Size,
    /// The main-axis extent children were laid out against: the incoming
    /// constraint, or `+∞` when shrink-wrapping.
    main_extent: f64,
    /// The cross-axis extent children were laid out against.
    cross_extent: f64,
    /// The finite main-axis extent the viewport occupies — what the physics
    /// reads as the viewport dimension.
    viewport_main: f64,
    /// The largest in-range position from the last completed layout.
    max_extent: f64,
    /// The position the last layout laid the children out at
    /// ([`ViewportWidget::layout_pixels`] at the time).
    laid_out_pixels: f64,
    /// The direction the position last moved in between two layouts.
    direction: ScrollDirection,
    /// Set for exactly the primary `Down` forward, so the forward installs this
    /// viewport's axis frame around it (the module docs' *Axis*).
    arming_down: bool,
    /// The axis a nested viewport reported when it registered into this
    /// viewport's claim cell on the last `Down` — `None` when nothing did, or
    /// when the registrant was a (vertical) `ScrollView`/`ListView`.
    down_inner_axis: Option<ScrollAxis>,
    core: ScrollSurfaceCore,
}

impl ViewportWidget {
    fn new(children: Vec<ChildPod>, axis: ScrollAxis, cache_extent: f64) -> Self {
        let slots = vec![Slot::default(); children.len()];
        Self {
            children,
            slots,
            axis,
            cache_extent: finite_extent(cache_extent),
            offset: 0.0,
            viewport: Size::ZERO,
            main_extent: 0.0,
            cross_extent: 0.0,
            viewport_main: 0.0,
            max_extent: 0.0,
            laid_out_pixels: 0.0,
            direction: ScrollDirection::Idle,
            arming_down: false,
            down_inner_axis: None,
            core: ScrollSurfaceCore::with_axis(axis),
        }
    }

    /// The current scroll position (in range unless a drag, settle or
    /// simulation holds it past an edge).
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// The largest in-range scroll position: the children's summed scroll
    /// extent less the viewport extent, never negative.
    pub fn max_offset(&self) -> f64 {
        self.max_extent
    }

    /// The axis this viewport scrolls along.
    pub fn axis(&self) -> ScrollAxis {
        self.core.axis()
    }

    /// How many children the viewport holds.
    pub fn sliver_count(&self) -> usize {
        self.children.len()
    }

    /// The geometry child `index` reported from the last layout — the box
    /// adapter's geometry for a child laid out as a box — or `None` past the
    /// last child.
    pub fn sliver_geometry(&self, index: usize) -> Option<SliverGeometry> {
        self.slots.get(index).map(|slot| slot.geometry)
    }

    /// Whether post-release motion is in flight (a fling or a simulation).
    pub fn is_flinging(&self) -> bool {
        self.core.is_flinging()
    }

    fn set_offset(&mut self, value: f64) {
        self.offset = value.clamp(0.0, self.max_extent);
    }

    /// The position children are laid out at: the live position under
    /// [`OverscrollEffect::Translate`], the in-range one under the other two
    /// effects (which paint the pull differently or not at all). Never NaN.
    fn layout_pixels(&self) -> f64 {
        let pixels = match self.core.effect {
            OverscrollEffect::Translate => self.offset,
            OverscrollEffect::Stretch | OverscrollEffect::None => self.offset - self.displacement(),
        };
        if pixels.is_finite() { pixels } else { 0.0 }
    }

    /// Whether user input owns the current motion — a drag, a release fling,
    /// settle or simulation — rather than a controller tween or nothing.
    fn user_motion(&self) -> bool {
        let core = &self.core;
        core.scrolling
            || core.settling
            || core.fling.is_some()
            || (core.ballistic.is_some() && !core.controller_animating)
    }

    /// The `user_scroll_direction` children are laid out with.
    fn user_scroll_direction(&self) -> ScrollDirection {
        if self.user_motion() {
            self.direction
        } else {
            ScrollDirection::Idle
        }
    }

    /// Record which way the position moved since the last layout.
    fn observe_direction(&mut self) {
        let pixels = self.layout_pixels();
        if pixels < self.laid_out_pixels {
            self.direction = ScrollDirection::Forward;
        } else if pixels > self.laid_out_pixels {
            self.direction = ScrollDirection::Reverse;
        }
    }

    /// Run the layout loop, restarting it for each correction a child
    /// reports, at most [`MAX_LAYOUT_CYCLES`] times.
    fn layout_slivers(&mut self, ctx: &mut LayoutCtx) {
        for cycle in 0..MAX_LAYOUT_CYCLES {
            let last = cycle + 1 == MAX_LAYOUT_CYCLES;
            match self.attempt_layout(ctx, !last) {
                Some(correction) => self.apply_correction(correction),
                None => return,
            }
        }
    }

    /// Shift the position by a child's `correction` before the loop restarts.
    /// A live drag's accumulator moves with it; a release simulation restarts
    /// from the corrected position at its current velocity (its curve is in
    /// absolute positions and would otherwise drag the content straight back);
    /// a controller tween keeps its absolute target.
    fn apply_correction(&mut self, correction: f64) {
        if !correction.is_finite() {
            return;
        }
        self.offset += correction;
        self.core.drag_position += correction;
        if self.core.controller_animating {
            return;
        }
        let Some(velocity) = self
            .core
            .ballistic
            .as_ref()
            .map(|state| state.sim.dx(state.elapsed_secs(self.core.last_frame_time)))
        else {
            return;
        };
        let metrics = self.metrics();
        match self
            .core
            .physics
            .create_ballistic_simulation(&metrics, velocity)
        {
            Some(sim) => self.core.ballistic = Some(BallisticState { sim, start: None }),
            None => self.core.stop_ballistic(),
        }
    }

    /// One pass of the layout loop (the module docs' *The layout loop*).
    /// Returns the correction that ended it early, if any — never on the
    /// final cycle (`allow_correction` false), which lays every child out and
    /// keeps the result.
    fn attempt_layout(&mut self, ctx: &mut LayoutCtx, allow_correction: bool) -> Option<f64> {
        let axis = self.axis;
        let main = self.main_extent;
        let cross = self.cross_extent;
        let cache = self.cache_extent;
        let pixels = self.layout_pixels();
        let direction = self.user_scroll_direction();

        // The viewport's leading edge is its anchor: content starts `pixels`
        // before it.
        let center_offset = -pixels;
        let reverse_remaining_paint = center_offset.max(0.0).min(main);
        let remaining_paint = (main - center_offset).max(0.0).min(main);
        let mut scroll_offset = pixels.max(0.0);
        let mut layout_offset = if center_offset >= main {
            center_offset
        } else {
            reverse_remaining_paint
        };
        let initial_layout_offset = layout_offset;
        let mut max_paint_offset = layout_offset + pixels.min(0.0);
        let mut preceding = 0.0;

        for (pod, slot) in self.children.iter_mut().zip(self.slots.iter_mut()) {
            let sliver_scroll_offset = scroll_offset.max(0.0);
            // The cache window is `[pixels − cache, pixels + main + cache)` in
            // scroll terms; in this child's own terms it starts `cache` before
            // its scroll offset (never before its own start) and runs from
            // there to the window's end.
            let cache_origin = (-cache).max(-sliver_scroll_offset);
            let remaining_cache =
                scroll_offset + main + cache - sliver_scroll_offset - cache_origin;
            let sc = sanitized(SliverConstraints {
                axis,
                scroll_offset: sliver_scroll_offset,
                preceding_scroll_extent: preceding,
                overlap: max_paint_offset - layout_offset,
                remaining_paint_extent: remaining_paint - layout_offset + initial_layout_offset,
                cross_axis_extent: cross,
                viewport_main_axis_extent: main,
                cache_origin,
                remaining_cache_extent: remaining_cache,
                user_scroll_direction: direction,
            });
            let (geometry, is_box) = match pod.layout_sliver_child(ctx, &sc) {
                Some(geometry) => (geometry, false),
                None => {
                    let size = pod.layout_child(ctx, &sc.as_box_constraints());
                    (box_geometry(axis.main_extent(size), &sc), true)
                }
            };
            if let Some(correction) = geometry.scroll_offset_correction {
                if allow_correction {
                    return Some(correction);
                }
                debug_assert!(
                    false,
                    "CustomScrollView: a sliver still asked for a scroll-offset correction \
                     ({correction}) after {MAX_LAYOUT_CYCLES} layout cycles"
                );
            }
            let effective_offset = layout_offset + geometry.paint_origin;
            let paint_offset = if geometry.visible || scroll_offset > 0.0 {
                effective_offset
            } else {
                -scroll_offset + initial_layout_offset
            };
            let origin = axis_point(axis, paint_offset, 0.0);
            pod.set_origin(if is_box {
                origin + box_child_origin(&sc).to_vec2()
            } else {
                origin
            });
            *slot = Slot {
                geometry,
                paint_offset,
                is_box,
            };
            max_paint_offset = (effective_offset + geometry.paint_extent).max(max_paint_offset);
            scroll_offset -= geometry.scroll_extent;
            preceding += geometry.scroll_extent;
            layout_offset += geometry.layout_extent;
        }

        let total = finite_extent(preceding);
        if main.is_finite() {
            self.viewport_main = main;
            self.max_extent = (total - main).max(0.0);
        } else {
            // Shrink-wrapping: the viewport is as long as its content.
            self.viewport_main = total;
            self.max_extent = 0.0;
        }
        self.laid_out_pixels = pixels;
        None
    }

    /// Whether `position` (viewport space) falls inside child `index`'s hit
    /// region: from its paint start for its hit-test extent, inside the
    /// viewport.
    fn hits(&self, index: usize, position: Point) -> bool {
        let Some(slot) = self.slots.get(index) else {
            return false;
        };
        let main = along(self.axis, position);
        let across = along(transpose_axis(self.axis), position);
        let local = main - slot.paint_offset;
        main >= 0.0
            && main < self.viewport_main
            && across >= 0.0
            && across < self.cross_extent
            && local >= 0.0
            && local < slot.geometry.hit_test_extent
    }

    /// Route a content event to the children: a broadcast, a focus-routed
    /// event or a captured gesture through
    /// [`crate::authoring::route_event`]'s own paths; anything else hit-tested
    /// first child to last against each child's hit region
    /// ([`ViewportWidget::hits`]), with `route_event`'s blur-on-outside-`Down`
    /// sweep.
    fn route_to_slivers(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast()
            || event.is_focus_routed()
            || self.children.iter().any(ChildPod::is_active)
        {
            return crate::authoring::route_event(&mut self.children, ctx, event);
        }
        let position = event.position();
        let mut handled = EventResult::Ignored;
        let mut kept_focus = None;
        for index in 0..self.children.len() {
            if self.hits(index, position)
                && self.children[index].event_child(ctx, event) == EventResult::Handled
            {
                if self.children[index].holds_live_focus() {
                    kept_focus = Some(index);
                }
                handled = EventResult::Handled;
                break;
            }
        }
        if matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Down) {
            for (index, pod) in self.children.iter_mut().enumerate() {
                if Some(index) != kept_focus && pod.is_focused() {
                    pod.set_focused(false);
                }
            }
        }
        handled
    }

    /// Hand a pointer event to the core, with the axis-aware claim rule around
    /// a primary `Down` (the module docs' *Axis*).
    fn pointer_on_axis(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        p: &PointerEvent,
        t_ms: f64,
    ) -> EventResult {
        let primary_down = p.phase == PointerPhase::Down && presses(p);
        // A registration this viewport must not leave in its host's cell: the
        // host scrolls along the other axis.
        let mut restore = None;
        if primary_down && let Some(host) = ambient_scroll_claim() {
            match enclosing_viewport(&host) {
                Some(frame) if frame.axis == self.axis => frame.inner_axis.set(Some(self.axis)),
                Some(_) => restore = Some((host.get(), host)),
                // The enclosing surface is a `ScrollView` or `ListView`.
                None if self.axis == ScrollAxis::Vertical => {}
                None => restore = Some((host.get(), host)),
            }
        }
        self.arming_down = primary_down;
        if primary_down {
            self.down_inner_axis = None;
        }
        let result = self.handle_pointer(ctx, event, p, t_ms);
        self.arming_down = false;
        if let Some((saved, host)) = restore {
            host.set(saved);
        }
        if primary_down
            && self.core.inner_at_down.registered
            && self.down_inner_axis.unwrap_or(ScrollAxis::Vertical) != self.axis
        {
            self.core.inner_at_down = InnerScrollState::default();
        }
        result
    }

    /// The event entry, parameterised on an explicit timestamp so velocity
    /// math is deterministic in tests; [`Widget::event`] supplies the real
    /// clock. Publishes whatever position the event left behind.
    fn event_at(&mut self, ctx: &mut EventCtx, event: &InputEvent, t_ms: f64) -> EventResult {
        let result = self.route_event_at(ctx, event, t_ms);
        self.publish_to_controller();
        result
    }

    /// The event body behind [`ViewportWidget::event_at`]: content events
    /// are routed here, the wheel and pointer gesture by the shared core.
    fn route_event_at(&mut self, ctx: &mut EventCtx, event: &InputEvent, t_ms: f64) -> EventResult {
        if !matches!(
            event,
            InputEvent::Pointer(p) if p.phase == PointerPhase::Cancel
        ) {
            self.deliver_pending_scroll(ctx);
        }
        match event {
            InputEvent::Scroll { delta, .. } => self.handle_wheel(ctx, delta),
            InputEvent::Pointer(p) => self.pointer_on_axis(ctx, event, p, t_ms),
            // Broadcasts, focus-routed events and anything hit-tested (a
            // `Scale`, a file drop) belong to the content.
            _ => self.route_to_slivers(ctx, event),
        }
    }

    /// The stretch overscroll transform for the viewport at `origin`/`size`
    /// (absolute), along this viewport's axis.
    fn stretch_transform(&self, origin: Point, size: Size) -> Option<Affine> {
        match self.axis {
            ScrollAxis::Vertical => self.core.stretch_transform(origin, size),
            ScrollAxis::Horizontal => {
                let swap = Affine::new([0.0, 1.0, 1.0, 0.0, 0.0, 0.0]);
                self.core
                    .stretch_transform(transpose(origin), Size::new(size.height, size.width))
                    .map(|transform| swap * transform * swap)
            }
        }
    }
}

/// `sc` with every caller-derived extent made safe for
/// [`SliverConstraints::debug_assert_valid`]: NaN dropped, negatives clamped
/// to zero (infinity kept), the cache origin kept non-positive.
fn sanitized(sc: SliverConstraints) -> SliverConstraints {
    let not_nan = |value: f64| if value.is_nan() { 0.0 } else { value };
    SliverConstraints {
        scroll_offset: non_negative(sc.scroll_offset),
        preceding_scroll_extent: not_nan(sc.preceding_scroll_extent),
        overlap: not_nan(sc.overlap),
        remaining_paint_extent: non_negative(sc.remaining_paint_extent),
        cross_axis_extent: non_negative(sc.cross_axis_extent),
        viewport_main_axis_extent: non_negative(sc.viewport_main_axis_extent),
        cache_origin: not_nan(sc.cache_origin).min(0.0),
        remaining_cache_extent: non_negative(sc.remaining_cache_extent),
        ..sc
    }
}

/// `point`'s component along `axis`.
fn along(axis: ScrollAxis, point: Point) -> f64 {
    match axis {
        ScrollAxis::Vertical => point.y,
        ScrollAxis::Horizontal => point.x,
    }
}

/// The other axis.
fn transpose_axis(axis: ScrollAxis) -> ScrollAxis {
    match axis {
        ScrollAxis::Vertical => ScrollAxis::Horizontal,
        ScrollAxis::Horizontal => ScrollAxis::Vertical,
    }
}

/// `point` with its two components swapped.
fn transpose(point: Point) -> Point {
    Point::new(point.y, point.x)
}

// --- Axis-aware nested claims ------------------------------------------------

/// A viewport currently forwarding a primary `Down`: the claim cell it pushed
/// around that forward, its axis, and the axis of the nested viewport that
/// registered into that cell (if one did).
struct AxisFrame {
    claim: Rc<Cell<InnerScrollState>>,
    axis: ScrollAxis,
    inner_axis: Cell<Option<ScrollAxis>>,
}

thread_local! {
    /// The viewports currently forwarding a primary `Down`, innermost last —
    /// what lets a nested viewport learn its enclosing surface's axis (the
    /// module docs' *Axis*). UI-thread-affine like the claim stack it shadows.
    static VIEWPORT_FRAMES: RefCell<Vec<Rc<AxisFrame>>> = const { RefCell::new(Vec::new()) };
}

/// Pops [`VIEWPORT_FRAMES`] on drop, so an unwinding dispatch cannot leave a
/// stale frame behind.
struct AxisFrameGuard;

impl Drop for AxisFrameGuard {
    fn drop(&mut self) {
        VIEWPORT_FRAMES.with(|frames| {
            frames.borrow_mut().pop();
        });
    }
}

/// Run `f` with `frame` installed as the innermost viewport frame.
fn with_axis_frame<R>(frame: &Rc<AxisFrame>, f: impl FnOnce() -> R) -> R {
    VIEWPORT_FRAMES.with(|frames| frames.borrow_mut().push(Rc::clone(frame)));
    let _guard = AxisFrameGuard;
    f()
}

/// The viewport whose claim cell is `host`, when `host` belongs to one — the
/// innermost frame, if it pushed that very cell. `None` means the enclosing
/// surface is a `ScrollView` or a `ListView`.
fn enclosing_viewport(host: &Rc<Cell<InnerScrollState>>) -> Option<Rc<AxisFrame>> {
    VIEWPORT_FRAMES.with(|frames| {
        frames
            .borrow()
            .last()
            .filter(|frame| Rc::ptr_eq(&frame.claim, host))
            .cloned()
    })
}

/// The viewport owner side of the core: one effective `offset`, exactly
/// `ScrollWidget`'s storage shape, with the extent the layout loop measured.
impl ScrollSurface for ViewportWidget {
    fn core(&self) -> &ScrollSurfaceCore {
        &self.core
    }

    fn core_mut(&mut self) -> &mut ScrollSurfaceCore {
        &mut self.core
    }

    fn max_scroll_extent(&self) -> f64 {
        self.max_extent
    }

    fn viewport_extent(&self) -> f64 {
        self.viewport_main
    }

    fn base_offset(&self) -> f64 {
        self.offset
    }

    fn set_base_offset(&mut self, value: f64) {
        self.set_offset(value);
    }

    fn pixels(&self) -> f64 {
        self.offset
    }

    fn displacement(&self) -> f64 {
        self.offset - self.offset.clamp(0.0, self.max_extent)
    }

    fn set_allowed_position(&mut self, allowed: f64) {
        self.offset = allowed;
    }

    fn decay_displacement(&mut self, retained: f64) {
        let target = self.offset.clamp(0.0, self.max_extent);
        let remaining = target - self.offset;
        self.offset = target - remaining * retained;
    }

    fn settle_to_edge(&mut self) {
        self.set_offset(self.offset);
    }

    /// Nothing to do here: placement is a layout product (the module docs'
    /// *Placement is a layout product*). A position moved by input dirties
    /// this pod through the handled event; one moved at paint requests the
    /// layout from [`Widget::paint`].
    fn sync_origins(&mut self) {}

    fn scroll_info(&self) -> ScrollInfo {
        ScrollInfo {
            offset: self.offset.clamp(0.0, self.max_extent),
            max_offset: self.max_extent,
            overscroll: self.displacement(),
        }
    }

    fn forward_pointer(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !std::mem::take(&mut self.arming_down) {
            return self.route_to_slivers(ctx, event);
        }
        // The primary `Down`'s forward: the core has just pushed this
        // viewport's claim cell, so it is the ambient one.
        let Some(claim) = ambient_scroll_claim() else {
            return self.route_to_slivers(ctx, event);
        };
        let frame = Rc::new(AxisFrame {
            claim,
            axis: self.axis,
            inner_axis: Cell::new(None),
        });
        let result = with_axis_frame(&frame, || self.route_to_slivers(ctx, event));
        self.down_inner_axis = frame.inner_axis.get();
        result
    }

    fn take_over_from_content(&mut self, ctx: &mut EventCtx, position: Point) {
        let cancel = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Cancel,
            position,
            button: PointerButton::Primary,
        });
        // A takeover, not the gesture's end: the captured child gets its
        // `Cancel` and is then released through the context, which also ends
        // a contact opt-in held inside it.
        if let Some(pod) = self.children.iter_mut().find(|pod| pod.is_active()) {
            pod.event_child(ctx, &cancel);
            ctx.release_captured_child(pod);
        } else {
            crate::authoring::route_event(&mut self.children, ctx, &cancel);
        }
    }
}

impl<State: 'static> View<State> for CustomScrollView<State> {
    type Element = ViewportWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ViewportWidget {
        let children = self
            .slivers
            .iter()
            .map(|view| crate::authoring::build_child(view, ctx))
            .collect();
        let mut widget = ViewportWidget::new(children, self.axis, self.cache_extent);
        widget.core.on_scroll = self
            .on_scroll
            .as_ref()
            .map(crate::authoring::erase_callback_arg);
        widget.core.on_refresh_release = self
            .on_refresh_release
            .as_ref()
            .map(crate::authoring::erase_callback);
        widget.core.install(self.physics.as_ref(), self.effect);
        widget.core.attach_controller(self.controller.as_ref());
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ViewportWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable — always reinstall the erased adapters.
        element.core.on_scroll = self
            .on_scroll
            .as_ref()
            .map(crate::authoring::erase_callback_arg);
        element.core.on_refresh_release = self
            .on_refresh_release
            .as_ref()
            .map(crate::authoring::erase_callback);
        element.core.install(self.physics.as_ref(), self.effect);
        let mut flags = crate::authoring::rebuild_children(
            &prev.slivers,
            &self.slivers,
            &mut element.children,
            ctx,
            |view| view,
            |_| None,
        );
        element
            .slots
            .resize(element.children.len(), Slot::default());
        let cache_extent = finite_extent(self.cache_extent);
        if element.axis != self.axis || element.cache_extent != cache_extent {
            element.axis = self.axis;
            element.core.set_axis(self.axis);
            element.cache_extent = cache_extent;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // A new binding publishes, and a queued command applies, at the next
        // layout or paint — make sure a frame-gated shell runs one.
        if element.core.attach_controller(self.controller.as_ref()) {
            flags |= ChangeFlags::PAINT;
        }
        // A queued command moves the children when it applies, and the layout
        // drain is where it applies first: report LAYOUT so the layout skip
        // runs that drain (`ScrollView`'s rebuild does the same).
        if element.core.has_pending_commands() {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ViewportWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.slivers.iter().zip(element.children.iter_mut()) {
            crate::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for ViewportWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max = bc.max();
        self.cross_extent = finite_extent(self.axis.cross_extent(max));
        self.main_extent = non_negative(self.axis.main_extent(max));
        self.observe_direction();
        self.layout_slivers(ctx);
        // The content's extent may have moved under a resting position: clamp
        // it, as `ScrollView` does — never while a drag, settle or simulation
        // legitimately holds it past an edge.
        let core = &self.core;
        if !core.scrolling && !core.settling && core.ballistic.is_none() {
            self.set_offset(self.offset);
            if self.layout_pixels() != self.laid_out_pixels {
                self.layout_slivers(ctx);
            }
        }
        // A controller command clamps against the extent just measured.
        self.core.laid_out = true;
        let reduce_motion = Theme::from_layout_ctx(ctx)
            .map(|t| t.motion.reduce_motion)
            .unwrap_or(false);
        self.drain_controller(reduce_motion);
        if self.layout_pixels() != self.laid_out_pixels {
            self.layout_slivers(ctx);
        }
        self.publish_to_controller();
        self.viewport = self.axis.size(self.viewport_main, self.cross_extent);
        bc.constrain(self.viewport)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Record the frame clock, apply a command recorded since layout, pump
        // post-release motion and publish — see `ScrollSurface::paint_motion`.
        let reduce_motion = Theme::from_paint_ctx(ctx)
            .map(|t| t.motion.reduce_motion)
            .unwrap_or(false);
        self.paint_motion(ctx, reduce_motion);
        // Whatever moved the position here is shown by the next layout.
        if self.layout_pixels() != self.laid_out_pixels {
            ctx.request_layout();
        }
        let origin = ctx.origin();
        scene.push_clip(origin, ctx.size());
        ctx.constrain_visible_rect(Rect::from_origin_size(origin, ctx.size()));
        // Paint-only, inside the viewport clip, exactly as `ScrollView`'s.
        let stretch = self.stretch_transform(origin, ctx.size());
        if let Some(transform) = stretch {
            scene.push_transform(transform);
        }
        let axis = self.axis;
        let cross = self.cross_extent;
        for (pod, slot) in self.children.iter_mut().zip(&self.slots).rev() {
            if !slot.geometry.visible {
                continue;
            }
            // The box adapter clips a cut child to its paint extent; a sliver
            // clips itself.
            let clip = slot.is_box && slot.geometry.has_visual_overflow;
            if clip {
                let start = origin + axis_point(axis, slot.paint_offset, 0.0).to_vec2();
                scene.push_clip(start, axis.size(slot.geometry.paint_extent, cross));
            }
            pod.paint_child(ctx, scene);
            if clip {
                scene.pop_clip();
            }
        }
        if stretch.is_some() {
            scene.pop_transform();
        }
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let t = self.core.event_time_ms();
        self.event_at(ctx, event, t)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let offset = self.offset;
        let max_offset = self.max_extent;
        let axis = self.axis;
        ctx.push_container(
            Role::ScrollView,
            move |node| match axis {
                ScrollAxis::Vertical => {
                    node.set_scroll_y(offset);
                    node.set_scroll_y_min(0.0);
                    node.set_scroll_y_max(max_offset);
                }
                ScrollAxis::Horizontal => {
                    node.set_scroll_x(offset);
                    node.set_scroll_x_min(0.0);
                    node.set_scroll_x_max(max_offset);
                }
            },
            |ctx| {
                // Visible and cached children, so assistive scrolling can
                // reach what is just past the edge.
                for (pod, slot) in self.children.iter().zip(&self.slots) {
                    if slot.geometry.visible || slot.geometry.cache_extent > 0.0 {
                        pod.semantics_child(ctx);
                    }
                }
            },
        );
    }

    crate::authoring::visit_children!(children);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{FrameTime, RenderRoot};
    use std::any::Any;

    use crate::test_support::{RecordingScene, leaf};

    const WINDOW: Size = Size::new(400.0, 500.0);

    fn widget(root: &RenderRoot<(), CustomScrollView<()>>) -> &ViewportWidget {
        let id = root.root_id().expect("root built");
        (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
            .downcast_ref::<ViewportWidget>()
            .expect("root is a ViewportWidget")
    }

    fn mount(
        logic: &mut impl FnMut(&mut ()) -> CustomScrollView<()>,
    ) -> RenderRoot<(), CustomScrollView<()>> {
        let mut root = RenderRoot::new();
        root.rebuild(logic, &mut ());
        root.layout(WINDOW);
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        root
    }

    #[test]
    fn sanitising_drops_nan_and_negative_extents() {
        let sc = sanitized(SliverConstraints {
            axis: ScrollAxis::Vertical,
            scroll_offset: f64::NAN,
            preceding_scroll_extent: f64::NAN,
            overlap: f64::NAN,
            remaining_paint_extent: -3.0,
            cross_axis_extent: f64::NAN,
            viewport_main_axis_extent: -1.0,
            cache_origin: 5.0,
            remaining_cache_extent: f64::NAN,
            user_scroll_direction: ScrollDirection::Idle,
        });
        sc.debug_assert_valid();
        assert_eq!(sc.remaining_paint_extent, 0.0);
        assert_eq!(sc.cache_origin, 0.0);
    }

    #[test]
    fn a_zero_window_lays_out_without_tripping_the_constraint_checks() {
        let mut logic = |_: &mut ()| custom_scroll_view((leaf(100.0, 100.0), leaf(100.0, 50.0)));
        let mut root = RenderRoot::new();
        root.rebuild(&mut logic, &mut ());
        root.layout(Size::ZERO);
        let w = widget(&root);
        assert_eq!(w.max_offset(), 150.0);
        assert!(!w.sliver_geometry(0).expect("child 0").visible);
    }

    #[test]
    fn an_unbounded_main_axis_shrink_wraps() {
        let mut logic = |_: &mut ()| custom_scroll_view((leaf(100.0, 100.0), leaf(100.0, 50.0)));
        let mut root = RenderRoot::new();
        root.rebuild(&mut logic, &mut ());
        root.layout(Size::new(400.0, f64::INFINITY));
        let w = widget(&root);
        assert_eq!(w.viewport.height, 150.0);
        assert_eq!(w.max_offset(), 0.0);
        assert_eq!(w.sliver_geometry(1).expect("child 1").paint_extent, 50.0);
    }

    #[test]
    fn plain_box_children_are_adapted_and_placed_in_sequence() {
        let mut logic = |_: &mut ()| {
            custom_scroll_view((leaf(400.0, 300.0), leaf(400.0, 300.0), leaf(400.0, 300.0)))
                .cache_extent(0.0)
        };
        let root = mount(&mut logic);
        let w = widget(&root);
        assert_eq!(w.max_offset(), 400.0);
        let g = |i| w.sliver_geometry(i).expect("child");
        assert_eq!(g(0).paint_extent, 300.0);
        assert_eq!(g(1).paint_extent, 200.0);
        assert!(g(1).has_visual_overflow);
        assert!(!g(2).visible);
        assert_eq!(w.slots[1].paint_offset, 300.0);
        assert!(w.slots.iter().all(|slot| slot.is_box));
    }
}
