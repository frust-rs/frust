//! The M3 Expressive **carousel**: a snapping row (or column) of items whose
//! extents morph as they cross the viewport, in the three reference layouts —
//! `hero`, `contained`, and `uncontained`.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/carousel/` — `m3e_carousel.dart`, `components/
//! m3e_carousel_view.dart`, `m3e_carousel_wrapper.dart`,
//! `m3e_sliver_weighted_carousel_layout.dart`,
//! `m3e_sliver_fixed_extent_carousel.dart`, `m3e_carousel_scroll_physics.dart`,
//! `m3e_carousel_position.dart`, `utils/m3e_carousel_scroll_helper.dart`,
//! `styles/m3e_carousel_theme.dart`, `enums/m3e_carousel_type.dart` (retrieved
//! 2026-08-19). Those files are themselves vendored from `m3_carousel` (MIT,
//! © 2024 Paa) and derived from Flutter's `CarouselView` — **BSD-3-Clause,
//! © 2014 The Flutter Authors**; both notices are in `plugins/material/NOTICE`.
//! [`layout`] and [`physics`] carry the same header over the halves of the port
//! they own.
//!
//! # The three layouts, and the weights behind them
//!
//! Every layout but `uncontained` is the same weighted solver with a different
//! weight list — `M3ECarousel._applyLayoutWeights`' own table:
//!
//! | Layout | Weights | Shape |
//! |---|---|---|
//! | [`CarouselLayout::Hero`] + [`HeroAlignment::Start`] | `[8, 2]` | one large item, one small peek after it |
//! | [`CarouselLayout::Hero`] + [`HeroAlignment::Center`] | `[2, 6, 2]` | a peek, the focal item, a peek |
//! | [`CarouselLayout::Hero`] + [`HeroAlignment::End`] | `[2, 8]` | one small peek, then the large item |
//! | [`CarouselLayout::Contained`] | `[5, 4, 1]` | large / medium / small, all inside the bounds |
//! | [`CarouselLayout::Contained`] + [`CarouselView::extended`] | `[4, 3, 2, 1]` | four descending items |
//! | [`CarouselLayout::Uncontained`] | — | uniform [`CarouselView::item_extent`] items, scrolling to the edge |
//!
//! A weight list is the *visible window*: each weight's share of the viewport is
//! one slot, and an item **morphs between adjacent slots** as the leading item
//! scrolls off — the signature M3E behavior, pinned by [`layout`]'s tests at
//! several viewport widths and scroll offsets.
//!
//! # Items are laid out once, at the largest slot, and clipped
//!
//! A slot's extent changes continuously during a scroll, but re-laying every
//! child out per frame would make a drag a relayout storm. Upstream solves this
//! in `M3ECarouselWrapper` — "item content is laid out at the largest slot size
//! and clipped to the current item bounds; scrolling only moves that clip
//! window" — and this port takes the same route: every child is laid out at
//! [`WeightedLayout::max_child_extent`](layout::WeightedLayout::max_child_extent)
//! (or the fixed item extent) once per `layout`, and `paint` clips it to its
//! current slot with a **rounded** clip, centring the content inside. The
//! rounded clip is load-bearing, not decoration: baseline `Clip` does not
//! visually clip, so `PaintScene::push_clip_rounded` is what actually shapes an
//! item.
//!
//! # Snapping, and the velocity that picks the target
//!
//! A press captures the pointer and forwards to the item under it; crossing
//! [`frust::input::TOUCH_SLOP`] takes the gesture over (cancelling the item's
//! own press) and turns it into a scroll. The release picks a target through
//! [`physics::snap_offset`] — the nearest item boundary, or one further along
//! when the release velocity clears [`physics::SNAP_VELOCITY_TOLERANCE`] — and
//! a [`SETTLE_DURATION`](physics::SETTLE_DURATION) ramp takes the content
//! there, advanced from the shell frame clock in `paint` like every other
//! motion in this crate. `Theme.motion.reduce_motion` jumps to the target
//! instead and asks for no further frames.
//!
//! Velocity comes from a [`frust::input::VelocityTracker`] fed with the last
//! *painted* frame time, since the event pass carries no clock of its own —
//! the same seam baseline `ScrollView` uses, and the reason the event body is
//! parameterised on an explicit timestamp ([`CarouselWidget::event_at`]).
//!
//! A mouse wheel over the carousel moves it and settles the same way. Upstream
//! has no wheel path at all (its carousel is swipe-driven under
//! `NeverScrollableScrollPhysics`); this is the port's desktop completion of
//! it.
//!
//! # Controlled and uncontrolled
//!
//! Uncontrolled by default: a drag or a settle moves the carousel's own index
//! and reports it through [`CarouselView::on_change`].
//! [`CarouselView::selected`] flips it to this catalog's controlled contract —
//! the carousel then reports the *requested* index and moves nothing until the
//! next `rebuild` feeds a confirmed one back down. Both that and
//! [`CarouselView::initial_item`] name the **focal** item (the one in the first
//! max-weight slot), which is what upstream's `M3ECarouselController.initialItem`
//! means for a weighted carousel.
//!
//! # Deliberately not ported
//!
//! * **Free scroll** (`M3ECarousel.freeScroll`) — upstream's own default is
//!   off, and with it on the widget swaps in a `ScrollSpringSimulation` plus
//!   Flutter's ballistic scroll machinery. Snapping is the only mode here.
//! * **Infinite looping** (`infinite`) — needs the sliver's cycle-length
//!   pixel correction; a finite item list is the whole surface here.
//! * **Lazy item building** (`M3ECarouselView.builder`) — this view takes a
//!   finite list of views like every other container in this catalog.
//! * **The tap pulse** (`M3ECarouselWrapper`'s `fixedPulseDelta`) — it animates
//!   a per-item clip window against content laid out with a pulse budget on
//!   top of the max slot. A pressed item takes this crate's shared
//!   [`state_layer`](crate::state_layer) overlay instead, which is the M3
//!   interaction affordance the rest of the catalog uses.

pub(crate) mod layout;
pub(crate) mod physics;

use std::rc::Rc;

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent,
    PointerPhase, Role, ScrollDelta, SemanticsCtx, TypedArgCallback, View, ViewSeq, Widget,
};
use frust::input::{TOUCH_SLOP, VelocityTracker, WHEEL_LINE_PX};
use frust::{AnimationController, FrameTime, Theme};
use kurbo::{Point, Rect, Size};
use peniko::Color;

use self::layout::{
    CONTAINED_EXTENDED_WEIGHTS, CONTAINED_WEIGHTS, FixedLayout, HERO_CENTER_WEIGHTS,
    HERO_END_WEIGHTS, HERO_START_WEIGHTS, Slot, WeightedLayout,
};
use crate::press::presses;
use crate::state_layer::StateLayer;

/// Space around each item, in logical px (`M3ECarouselTheme.itemPadding`,
/// `EdgeInsets.all(4)`) — inset from the item's own slot, so adjacent items
/// show an 8px gutter between them.
pub const CAROUSEL_ITEM_PADDING: f64 = 4.0;

/// Unthemed-fallback item corner radius, in logical px
/// (`M3ECarouselTheme.defaultBorderRadiusValue`). A theme resolves this from
/// `shape.extra_large`, which carries the same 28dp value.
pub const CAROUSEL_ITEM_RADIUS: f64 = 28.0;

/// Default main-axis extent of an [`CarouselLayout::Uncontained`] item, in
/// logical px (`M3ECarouselTheme.defaultUncontainedItemExtent`).
pub const UNCONTAINED_ITEM_EXTENT: f64 = 270.0;

/// Default shrink floor for an [`CarouselLayout::Uncontained`] carousel, in
/// logical px (`M3ECarouselTheme.defaultUncontainedShrinkExtent`) — how small
/// an item may get at the viewport edge before it slides off instead.
pub const UNCONTAINED_SHRINK_EXTENT: f64 = 150.0;

/// Offset differences below which the content counts as settled, in logical px.
const OFFSET_EPSILON: f64 = 1e-3;

/// Unthemed-fallback state-layer content color for a pressed item (a theme
/// resolves this from `colors.on_surface`).
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);

/// The carousel's structural layout (`M3ECarouselType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CarouselLayout {
    /// One large, prominent item with small peeking neighbours — the
    /// reference's own default. Positioned by [`HeroAlignment`].
    #[default]
    Hero,
    /// Large / medium / small items, all kept inside the carousel's bounds.
    /// [`CarouselView::extended`] adds a fourth, smaller step.
    Contained,
    /// Uniform items that scroll to the edge of the container, sized by
    /// [`CarouselView::item_extent`].
    Uncontained,
}

/// Where a [`CarouselLayout::Hero`] carousel puts its focal item
/// (`M3ECarouselHeroAlignment`).
///
/// Named for the scroll axis rather than the screen: `Start` is the left edge
/// of a horizontal carousel and the top of a vertical one (upstream's `left`),
/// `End` the right/bottom (upstream's `right`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeroAlignment {
    /// Focal item at the leading edge, one small peek after it (`[8, 2]`).
    Start,
    /// Focal item between two small peeks (`[2, 6, 2]`) — the reference default.
    #[default]
    Center,
    /// One small peek, then the focal item at the trailing edge (`[2, 8]`).
    End,
}

/// The axis a carousel scrolls along (`M3ECarousel.axis`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CarouselAxis {
    /// Items flow along the x axis.
    #[default]
    Horizontal,
    /// Items flow along the y axis.
    Vertical,
}

/// What a carousel reports when the item under the layout changes
/// (`M3ECarouselChangeDetails`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarouselChange {
    /// The first visible layout slot's item.
    pub leading_index: usize,
    /// The item in the largest (focal) slot — equal to
    /// [`leading_index`](Self::leading_index) for an uncontained carousel,
    /// which has no larger slot to be focal in.
    pub focal_index: usize,
    /// How many items the carousel holds.
    pub item_count: usize,
}

impl CarouselChange {
    /// Whether `index` is the current focal (largest) item.
    pub fn is_focal(&self, index: usize) -> bool {
        index == self.focal_index
    }
}

/// A declarative M3E carousel. See the [module docs](self).
pub struct CarouselView<State: 'static> {
    items: Vec<AnyView<State>>,
    layout: CarouselLayout,
    hero_alignment: HeroAlignment,
    extended: bool,
    axis: CarouselAxis,
    item_extent: f64,
    shrink_extent: Option<f64>,
    item_radius: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    consume_max_weight: bool,
    initial_item: usize,
    selected: Option<usize>,
    on_change: Option<TypedArgCallback<State, CarouselChange>>,
    on_tap: Option<TypedArgCallback<State, usize>>,
}

/// A hero carousel over `items` — the reference's own default layout, focal
/// item centred. See the [module docs](self).
pub fn carousel<State: 'static, M>(items: impl ViewSeq<State, M>) -> CarouselView<State> {
    let mut erased = Vec::new();
    items.extend_views(&mut erased);
    CarouselView {
        items: erased,
        layout: CarouselLayout::default(),
        hero_alignment: HeroAlignment::default(),
        extended: false,
        axis: CarouselAxis::default(),
        item_extent: UNCONTAINED_ITEM_EXTENT,
        shrink_extent: None,
        item_radius: None,
        width: None,
        height: None,
        consume_max_weight: false,
        initial_item: 0,
        selected: None,
        on_change: None,
        on_tap: None,
    }
}

/// A [`CarouselLayout::Hero`] carousel over `items` — one large item plus
/// peeks, placed by `alignment`.
pub fn hero_carousel<State: 'static, M>(
    items: impl ViewSeq<State, M>,
    alignment: HeroAlignment,
) -> CarouselView<State> {
    carousel(items).hero_alignment(alignment)
}

/// A [`CarouselLayout::Contained`] carousel over `items` — large/medium/small
/// items kept inside the bounds.
pub fn contained_carousel<State: 'static, M>(items: impl ViewSeq<State, M>) -> CarouselView<State> {
    carousel(items).layout(CarouselLayout::Contained)
}

/// A [`CarouselLayout::Uncontained`] carousel over `items` — uniform
/// `item_extent`-wide items scrolling to the container's edge.
pub fn uncontained_carousel<State: 'static, M>(
    items: impl ViewSeq<State, M>,
    item_extent: f64,
) -> CarouselView<State> {
    carousel(items)
        .layout(CarouselLayout::Uncontained)
        .item_extent(item_extent)
}

impl<State: 'static> CarouselView<State> {
    /// Set the structural layout (`type`).
    pub fn layout(mut self, layout: CarouselLayout) -> Self {
        self.layout = layout;
        self
    }

    /// Where a [`CarouselLayout::Hero`] carousel's focal item sits. Ignored by
    /// the other two layouts.
    pub fn hero_alignment(mut self, alignment: HeroAlignment) -> Self {
        self.hero_alignment = alignment;
        self
    }

    /// Show four descending items instead of three (`isExtended`). Applies to
    /// [`CarouselLayout::Contained`] only.
    pub fn extended(mut self, extended: bool) -> Self {
        self.extended = extended;
        self
    }

    /// Set the scroll axis (default [`CarouselAxis::Horizontal`]).
    pub fn axis(mut self, axis: CarouselAxis) -> Self {
        self.axis = axis;
        self
    }

    /// Main-axis extent of an [`CarouselLayout::Uncontained`] item, in logical
    /// px (default [`UNCONTAINED_ITEM_EXTENT`]). Capped at the viewport by the
    /// solver; ignored by the weighted layouts, which derive every extent from
    /// the viewport instead.
    pub fn item_extent(mut self, extent: f64) -> Self {
        self.item_extent = extent;
        self
    }

    /// How small an item may shrink at the viewport edge before it slides off
    /// instead, in logical px (`shrinkExtent`). Defaults to `0.0` for the
    /// weighted layouts and [`UNCONTAINED_SHRINK_EXTENT`] for
    /// [`CarouselLayout::Uncontained`], matching the reference's own two
    /// defaults. Never exceeds the smallest slot.
    pub fn shrink_extent(mut self, extent: f64) -> Self {
        self.shrink_extent = Some(extent);
        self
    }

    /// Item corner radius, in logical px (`childElementBorderRadius`).
    /// Unset resolves `shape.extra_large` from the theme, then
    /// [`CAROUSEL_ITEM_RADIUS`].
    pub fn item_radius(mut self, radius: f64) -> Self {
        self.item_radius = Some(radius);
        self
    }

    /// Pin the carousel's own width, in logical px (`M3ECarousel.width`).
    /// Unset takes the incoming constraint.
    pub fn width(mut self, width: f64) -> Self {
        self.width = Some(width);
        self
    }

    /// Pin the carousel's own height, in logical px (`M3ECarousel.height`).
    /// Unset takes the incoming constraint.
    pub fn height(mut self, height: f64) -> Self {
        self.height = Some(height);
        self
    }

    /// Let the leading items expand into the largest slot
    /// (`consumeMaxWeight`), leaving a phantom gap before item 0 while they do.
    ///
    /// Off by default, which is the choice upstream's own top-level
    /// `M3ECarousel` makes: with it on, a theme rebuild can strand the extra
    /// leading extent offstage.
    pub fn consume_max_weight(mut self, consume: bool) -> Self {
        self.consume_max_weight = consume;
        self
    }

    /// The **focal** item to start on (`M3ECarouselController.initialItem`).
    /// Only read on the first build; use [`selected`](Self::selected) to drive
    /// the carousel afterwards.
    pub fn initial_item(mut self, index: usize) -> Self {
        self.initial_item = index;
        self
    }

    /// Drive the carousel from the app: it shows exactly this **focal** item
    /// and reports every requested index through
    /// [`on_change`](Self::on_change) instead of moving itself (see the
    /// [module docs](self)).
    pub fn selected(mut self, focal_index: usize) -> Self {
        self.selected = Some(focal_index);
        self
    }

    /// Report the leading/focal item whenever either changes (`onChange`).
    pub fn on_change<F: Fn(&mut State, CarouselChange) + 'static>(mut self, on_change: F) -> Self {
        self.on_change = Some(Rc::new(on_change));
        self
    }

    /// Report a tap on an item, by index (`onTap`).
    pub fn on_tap<F: Fn(&mut State, usize) + 'static>(mut self, on_tap: F) -> Self {
        self.on_tap = Some(Rc::new(on_tap));
        self
    }

    /// The item-sizing rule this view's layout knobs resolve to.
    fn sizing(&self) -> Sizing {
        match self.layout {
            CarouselLayout::Hero => Sizing::Weighted(match self.hero_alignment {
                HeroAlignment::Start => HERO_START_WEIGHTS,
                HeroAlignment::Center => HERO_CENTER_WEIGHTS,
                HeroAlignment::End => HERO_END_WEIGHTS,
            }),
            CarouselLayout::Contained => Sizing::Weighted(if self.extended {
                CONTAINED_EXTENDED_WEIGHTS
            } else {
                CONTAINED_WEIGHTS
            }),
            CarouselLayout::Uncontained => Sizing::Fixed {
                item_extent: self.item_extent,
            },
        }
    }

    /// The shrink floor, defaulted per layout (see
    /// [`shrink_extent`](Self::shrink_extent)).
    fn resolved_shrink_extent(&self) -> f64 {
        self.shrink_extent.unwrap_or(match self.layout {
            CarouselLayout::Uncontained => UNCONTAINED_SHRINK_EXTENT,
            _ => 0.0,
        })
    }
}

impl<State: 'static> View<State> for CarouselView<State> {
    type Element = CarouselWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CarouselWidget {
        let sizing = self.sizing();
        let mut widget = CarouselWidget {
            pods: self
                .items
                .iter()
                .map(|item| frust::authoring::build_child(item, ctx))
                .collect(),
            sizing,
            axis: self.axis,
            shrink_extent: self.resolved_shrink_extent(),
            item_radius: self.item_radius,
            width: self.width,
            height: self.height,
            consume_max_weight: self.consume_max_weight,
            controlled: self.selected.is_some(),
            index: 0,
            offset: 0.0,
            settle: physics::settle_controller(),
            settle_from: 0.0,
            settle_to: 0.0,
            main: 0.0,
            cross: 0.0,
            slots: Vec::new(),
            drag: None,
            pressed: None,
            state: StateLayer::new(),
            tracker: VelocityTracker::new(),
            last_frame_time: FrameTime::ZERO,
            reported: (0, 0),
            on_change: self
                .on_change
                .as_ref()
                .map(frust::authoring::erase_callback_arg),
            on_tap: self
                .on_tap
                .as_ref()
                .map(frust::authoring::erase_callback_arg),
        };
        widget.index = widget.leading_for_focal(self.selected.unwrap_or(self.initial_item));
        widget.reported = widget.indices_for_leading(widget.index);
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CarouselWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = frust::authoring::rebuild_children(
            &prev.items,
            &self.items,
            &mut element.pods,
            ctx,
            |view: &AnyView<State>| view,
            |_| None,
        );
        let sizing = self.sizing();
        let shrink_extent = self.resolved_shrink_extent();
        if element.sizing != sizing
            || element.axis != self.axis
            || element.shrink_extent != shrink_extent
            || element.width != self.width
            || element.height != self.height
            || element.consume_max_weight != self.consume_max_weight
        {
            element.sizing = sizing;
            element.axis = self.axis;
            element.shrink_extent = shrink_extent;
            element.width = self.width;
            element.height = self.height;
            element.consume_max_weight = self.consume_max_weight;
            // A different weight list re-slots every item: the carousel returns
            // to its own index rather than keeping a now-meaningless offset,
            // exactly as `M3ECarousel.didUpdateWidget` jumps back to 0.
            element.settle.stop();
            element.offset = element.offset_for(element.index);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.item_radius != self.item_radius {
            element.item_radius = self.item_radius;
            flags |= ChangeFlags::PAINT;
        }
        element.controlled = self.selected.is_some();
        let last = element.pods.len().saturating_sub(1);
        if let Some(selected) = self.selected {
            // Controlled: the app is the source of truth, so adopt the
            // confirmed focal item and settle onto it.
            let index = element.leading_for_focal(selected);
            if element.index != index {
                element.index = index;
                element.reported = element.indices_for_leading(index);
                element.start_settle();
                flags |= ChangeFlags::PAINT;
            }
        } else if element.index > last {
            // A shrunk item list cannot leave the index past the end.
            element.index = last;
            element.reported = element.indices_for_leading(last);
            element.start_settle();
            flags |= ChangeFlags::PAINT;
        }
        element.on_change = self
            .on_change
            .as_ref()
            .map(frust::authoring::erase_callback_arg);
        element.on_tap = self
            .on_tap
            .as_ref()
            .map(frust::authoring::erase_callback_arg);
        flags
    }

    fn teardown(&self, element: &mut CarouselWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.items.iter().zip(element.pods.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

/// How a carousel sizes its items: by weight share of the viewport, or at one
/// fixed extent.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sizing {
    /// The weighted layouts (hero, contained) and their weight list.
    Weighted(&'static [u16]),
    /// The uncontained layout and its per-item main-axis extent.
    Fixed { item_extent: f64 },
}

/// One of the two solvers in [`layout`], resolved against a live viewport and
/// scroll offset.
enum Solver {
    Weighted(WeightedLayout<'static>),
    Fixed(FixedLayout),
}

impl Solver {
    /// Every visible item's viewport-space geometry, in index order.
    fn slots(&self, item_count: usize) -> Vec<Slot> {
        match self {
            Solver::Weighted(l) => l.slots(item_count),
            Solver::Fixed(l) => l.slots(item_count),
        }
    }

    /// The scroll distance one item step moves the content.
    fn stride(&self) -> f64 {
        match self {
            Solver::Weighted(l) => l.first_child_extent(),
            Solver::Fixed(l) => l.stride(),
        }
    }

    /// The largest extent an item can occupy — the size every child is laid
    /// out at (see the [module docs](self)).
    fn slot_extent(&self) -> f64 {
        match self {
            Solver::Weighted(l) => l.max_child_extent(),
            Solver::Fixed(l) => l.stride(),
        }
    }

    /// The furthest the content may scroll.
    fn max_scroll_offset(&self, item_count: usize) -> f64 {
        match self {
            Solver::Weighted(l) => l.max_scroll_offset(item_count),
            Solver::Fixed(l) => l.max_scroll_offset(item_count),
        }
    }
}

/// A live drag on the carousel.
struct Drag {
    /// Main-axis coordinate of the `Down`.
    start: f64,
    /// The scroll offset the drag started from.
    from: f64,
    /// The item the `Down` landed on, if any.
    item: Option<usize>,
    /// Whether the gesture has crossed the slop and become a scroll — until it
    /// does, every move still belongs to the item under the pointer.
    scrolling: bool,
}

/// The retained widget for a [`CarouselView`]. See the [module docs](self).
pub struct CarouselWidget {
    pods: Vec<ChildPod>,
    sizing: Sizing,
    axis: CarouselAxis,
    shrink_extent: f64,
    item_radius: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    consume_max_weight: bool,
    /// Whether the app drives the index (see the [module docs](self)).
    controlled: bool,
    /// The **slot item** the carousel rests on (or is settling onto): the one
    /// whose leading edge a resting offset sits at, so `offset == index *
    /// stride`. Not always the item *reported* as leading — see
    /// [`CarouselWidget::indices_for_leading`].
    index: usize,
    /// The current scroll offset, in logical px.
    offset: f64,
    settle: AnimationController,
    /// The offset the running settle started from, and the one it ends at.
    settle_from: f64,
    settle_to: f64,
    /// Main/cross extents resolved at layout.
    main: f64,
    cross: f64,
    /// The visible items' geometry, recomputed whenever the offset moves.
    slots: Vec<Slot>,
    drag: Option<Drag>,
    /// The item a `Down` armed, cleared on `Up`/`Cancel` or a slop takeover.
    pressed: Option<usize>,
    state: StateLayer,
    tracker: VelocityTracker,
    /// The most recent frame time seen at paint, reused as the event pass's
    /// timestamp for velocity tracking (the event pass carries no clock).
    last_frame_time: FrameTime,
    /// The last `(leading, focal)` pair reported through `on_change`.
    reported: (usize, usize),
    on_change: Option<ErasedArgCallback<CarouselChange>>,
    on_tap: Option<ErasedArgCallback<usize>>,
}

impl CarouselWidget {
    /// The item in the first visible slot at rest — the
    /// [`CarouselChange::leading_index`] the carousel would report.
    pub fn leading_index(&self) -> usize {
        self.indices_for_leading(self.index).0
    }

    /// The item in the largest (focal) slot at rest.
    pub fn focal_index(&self) -> usize {
        self.indices_for_leading(self.index).1
    }

    /// The current scroll offset, in logical px.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// The solver for this carousel at `offset`.
    fn solver(&self, offset: f64) -> Solver {
        match self.sizing {
            Sizing::Weighted(weights) => Solver::Weighted(WeightedLayout::new(
                weights,
                self.main,
                self.shrink_extent,
                offset,
                self.consume_max_weight,
            )),
            Sizing::Fixed { item_extent } => Solver::Fixed(FixedLayout::new(
                item_extent,
                self.shrink_extent,
                self.main,
                offset,
            )),
        }
    }

    /// The index of the first maximum weight in the active weight list — the
    /// distance from the leading item to the focal one. `0` for the
    /// uncontained layout, which has no focal slot.
    fn focal_slot(&self) -> usize {
        match self.sizing {
            Sizing::Weighted(weights) => {
                let max = weights.iter().copied().max().unwrap_or(0);
                weights.iter().position(|w| *w == max).unwrap_or(0)
            }
            Sizing::Fixed { .. } => 0,
        }
    }

    /// The slot item that puts `focal` in the focal slot — the inverse of
    /// [`Self::indices_for_leading`], and upstream's `_getInitialLeadingItem`.
    fn leading_for_focal(&self, focal: usize) -> usize {
        let last = self.pods.len().saturating_sub(1);
        let raw = if self.consume_max_weight {
            focal
        } else {
            focal.saturating_sub(self.focal_slot())
        };
        raw.min(last)
    }

    /// The `(leading, focal)` pair a **slot item** reads as — the item index
    /// `offset / stride` names, which is not the reported leading item under
    /// `consume_max_weight` (the leading slots then hold items *before* it).
    ///
    /// Upstream computes the focal item from the already-shifted leading index
    /// (`_focalIndexForLeading(_reportedLeadingIndex(..))`), which double-counts
    /// the shift for the first `focal_slot` items; the focal item is derived
    /// from the unshifted slot item here instead, so item 0 reads as focal when
    /// it is the one actually occupying the largest slot.
    fn indices_for_leading(&self, raw: usize) -> (usize, usize) {
        let last = self.pods.len().saturating_sub(1);
        let raw = raw.min(last);
        if self.consume_max_weight {
            (raw.saturating_sub(self.focal_slot()), raw)
        } else {
            (raw, (raw + self.focal_slot()).min(last))
        }
    }

    /// The `(leading, focal)` pair a scroll offset reads as — upstream's
    /// `_reportedLeadingIndex`, which rounds so both swipe directions report at
    /// the halfway mark.
    fn indices_at(&self, offset: f64) -> (usize, usize) {
        let raw = physics::nearest_item(offset, self.solver(offset).stride());
        self.indices_for_leading(raw)
    }

    /// The scroll offset that rests on slot item `index`.
    fn offset_for(&self, index: usize) -> f64 {
        let solver = self.solver(self.offset);
        let target = index as f64 * solver.stride();
        // Ordered explicitly rather than `clamp`: `max_scroll_offset` is
        // computed and floored to `0.0` here (`layout`'s clamp discipline).
        target
            .max(0.0)
            .min(solver.max_scroll_offset(self.pods.len()).max(0.0))
    }

    /// The furthest the content may scroll at the current viewport.
    fn max_offset(&self) -> f64 {
        self.solver(self.offset)
            .max_scroll_offset(self.pods.len())
            .max(0.0)
    }

    /// Whether there is anything to scroll at all.
    fn scrollable(&self) -> bool {
        self.max_offset() > 0.0
    }

    /// Set the offset, bounded to the scrollable range, and re-slot.
    fn set_offset(&mut self, offset: f64) {
        let offset = if offset.is_finite() { offset } else { 0.0 };
        // Ordered explicitly rather than `clamp` (see `offset_for`).
        self.offset = offset.max(0.0).min(self.max_offset());
        self.sync_geometry();
    }

    /// Recompute the visible slots for the current offset and place every pod.
    ///
    /// Called from `layout`, from the drag arm (so a drag moves the content
    /// without a relayout) and from `paint` while settling.
    fn sync_geometry(&mut self) {
        self.slots = self.solver(self.offset).slots(self.pods.len());
        let content = self.content_size();
        let first_visible = self.slots.first().map(|s| s.index);
        for (index, pod) in self.pods.iter_mut().enumerate() {
            let origin = match self.slots.iter().find(|s| s.index == index) {
                Some(slot) => {
                    // The child is laid out at the largest slot and centred in
                    // the current one, so only the clip window moves (module
                    // docs). Half a slot of it can overhang either edge.
                    let along =
                        slot.offset + (slot.extent - content_main(self.axis, content)) / 2.0;
                    let across = (self.cross - content_cross(self.axis, content)) / 2.0;
                    axis_point(self.axis, along, across)
                }
                None => {
                    // Not visible: parked wholly outside the carousel's own
                    // bounds so no pointer inside it can ever hit the pod
                    // (`route_event` hit-tests by pod bounds).
                    let along = if first_visible.is_some_and(|first| index < first) {
                        -content_main(self.axis, content) - 1.0
                    } else {
                        self.main + 1.0
                    };
                    axis_point(self.axis, along, 0.0)
                }
            };
            pod.set_origin(origin);
        }
    }

    /// The size every child is laid out at: the largest slot, inset by the item
    /// padding on all four sides.
    fn content_size(&self) -> Size {
        let slot = self.solver(self.offset).slot_extent();
        let main = (slot - CAROUSEL_ITEM_PADDING * 2.0).max(0.0);
        let cross = (self.cross - CAROUSEL_ITEM_PADDING * 2.0).max(0.0);
        match self.axis {
            CarouselAxis::Horizontal => Size::new(main, cross),
            CarouselAxis::Vertical => Size::new(cross, main),
        }
    }

    /// A visible item's clip window in widget-local coordinates: its slot,
    /// inset by the item padding.
    fn item_rect(&self, slot: &Slot) -> Rect {
        let main = (slot.extent - CAROUSEL_ITEM_PADDING * 2.0).max(0.0);
        let cross = (self.cross - CAROUSEL_ITEM_PADDING * 2.0).max(0.0);
        let origin = axis_point(
            self.axis,
            slot.offset + CAROUSEL_ITEM_PADDING,
            CAROUSEL_ITEM_PADDING,
        );
        let size = match self.axis {
            CarouselAxis::Horizontal => Size::new(main, cross),
            CarouselAxis::Vertical => Size::new(cross, main),
        };
        Rect::from_origin_size(origin, size)
    }

    /// The item whose clip window contains widget-local `pos`, if any.
    fn item_at(&self, pos: Point) -> Option<usize> {
        self.slots
            .iter()
            .find(|slot| self.item_rect(slot).contains(pos))
            .map(|slot| slot.index)
    }

    /// A position's main-axis component under this carousel's axis.
    fn main_of(&self, point: Point) -> f64 {
        match self.axis {
            CarouselAxis::Horizontal => point.x,
            CarouselAxis::Vertical => point.y,
        }
    }

    /// The last painted frame time in milliseconds — the event pass's
    /// timestamp source for velocity tracking (see [`Self::last_frame_time`]).
    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    /// Start settling from wherever the content is onto `target`.
    fn settle_to(&mut self, target: f64) {
        self.settle_from = self.offset;
        self.settle_to = target;
        self.settle = physics::settle_controller();
        self.settle.forward();
    }

    /// Start settling back onto the carousel's own index.
    fn start_settle(&mut self) {
        let target = self.offset_for(self.index);
        self.settle_to(target);
    }

    /// Report `(leading, focal)` for `offset` if either changed since the last
    /// report.
    fn report(&mut self, ctx: &mut EventCtx, offset: f64) {
        let pair = self.indices_at(offset);
        if pair == self.reported {
            return;
        }
        self.reported = pair;
        let count = self.pods.len();
        if let Some(on_change) = self.on_change.as_mut() {
            on_change(
                ctx,
                CarouselChange {
                    leading_index: pair.0,
                    focal_index: pair.1,
                    item_count: count,
                },
            );
        }
    }

    /// Forward a synthesized `Cancel` to the item a drag has taken the gesture
    /// away from.
    fn cancel_item(&mut self, ctx: &mut EventCtx, item: usize, pos: Point) {
        if let Some(pod) = self.pods.get_mut(item) {
            let cancel = InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Cancel,
                position: pos,
                button: PointerButton::Primary,
            });
            pod.event_child(ctx, &cancel);
            pod.set_active(false);
        }
    }

    /// Clear the recorded focus link on every item but the one a `Down` just
    /// landed on and left focused — the blur
    /// [`frust::authoring::route_event`] performs for containers that route
    /// through it, which this widget's hand-rolled routing owes too
    /// (`docs/CODE_STANDARDS.md`'s focus-routing rule).
    fn blur_unhit(&mut self, hit: Option<usize>) {
        let kept = hit.filter(|index| self.pods.get(*index).is_some_and(ChildPod::is_focused));
        for (index, pod) in self.pods.iter_mut().enumerate() {
            if Some(index) != kept && pod.is_focused() {
                pod.set_focused(false);
            }
        }
    }

    /// The event body, parameterised on an explicit timestamp so the velocity
    /// math is deterministic in tests; [`Widget::event`] supplies the real
    /// clock (see the [module docs](self)).
    fn event_at(&mut self, ctx: &mut EventCtx, event: &InputEvent, t_ms: f64) -> EventResult {
        // A broadcast is not user input: it reaches every child ahead of the
        // capture/hit-test branches and is never consumed.
        if event.is_broadcast() {
            return frust::authoring::route_event(&mut self.pods, ctx, event);
        }
        if let InputEvent::Scroll { delta, .. } = event
            && self.drag.is_none()
            && self.scrollable()
        {
            return self.wheel(ctx, delta);
        }
        let InputEvent::Pointer(p) = event else {
            // Key/Ime are focus-routed; the carousel itself has no key handling.
            return frust::authoring::route_event(&mut self.pods, ctx, event);
        };

        // A live gesture owns every pointer pass until it ends.
        if self.drag.is_some() {
            return self.drag_event(ctx, event, p, t_ms);
        }

        match p.phase {
            PointerPhase::Down => {
                let item = self.item_at(p.position);
                if !presses(p) {
                    // A secondary press is a context gesture: it reaches the
                    // item but starts no drag and arms no press.
                    let result = self.forward_to_item(ctx, event, item);
                    self.blur_unhit(item);
                    return result;
                }
                let main = self.main_of(p.position);
                self.drag = Some(Drag {
                    start: main,
                    from: self.offset,
                    item,
                    scrolling: false,
                });
                self.tracker.clear();
                self.tracker.record(t_ms, main);
                self.settle.stop();
                self.pressed = item;
                self.state.set_pressed(item.is_some());
                ctx.capture_pointer();
                self.forward_to_item(ctx, event, item);
                self.blur_unhit(item);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                // An uncaptured move is a hover pass: the item under the
                // pointer claims for itself.
                self.forward_to_item(ctx, event, self.item_at(p.position));
                EventResult::Ignored
            }
            PointerPhase::Up | PointerPhase::Cancel => EventResult::Ignored,
        }
    }

    /// The captured arm: every pass between the `Down` that armed a drag and
    /// the `Up`/`Cancel` that ends it.
    fn drag_event(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        p: &PointerEvent,
        t_ms: f64,
    ) -> EventResult {
        let main = self.main_of(p.position);
        match p.phase {
            PointerPhase::Down => EventResult::Handled,
            PointerPhase::Move => {
                self.tracker.record(t_ms, main);
                let Some(drag) = self.drag.as_ref() else {
                    return EventResult::Handled;
                };
                let (from, start, item, scrolling) =
                    (drag.from, drag.start, drag.item, drag.scrolling);
                if scrolling {
                    self.set_offset(from - (main - start));
                    self.report(ctx, self.offset);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if self.scrollable() && (main - start).abs() > TOUCH_SLOP {
                    // Take the gesture over: cancel the item's own press and
                    // scroll from here.
                    if let Some(item) = item {
                        self.cancel_item(ctx, item, p.position);
                    }
                    self.pressed = None;
                    self.state.set_pressed(false);
                    if let Some(drag) = self.drag.as_mut() {
                        drag.scrolling = true;
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                self.forward_to_item(ctx, event, item);
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(drag) = self.drag.take() else {
                    return EventResult::Handled;
                };
                self.pressed = None;
                self.state.set_pressed(false);
                if drag.scrolling {
                    self.release(ctx);
                } else {
                    self.forward_to_item(ctx, event, drag.item);
                    // Fire on up-inside: the release must land on the same item
                    // the press did.
                    if let Some(item) = drag.item
                        && self.item_at(p.position) == Some(item)
                        && let Some(on_tap) = self.on_tap.as_mut()
                    {
                        on_tap(ctx, item);
                    }
                }
                if let Some(item) = drag.item
                    && let Some(pod) = self.pods.get_mut(item)
                {
                    pod.set_active(false);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                // Internal flags only — a `Cancel` arm never reaches app state
                // and never fires a callback.
                let drag = self.drag.take();
                self.pressed = None;
                self.state.set_pressed(false);
                if let Some(drag) = &drag
                    && let Some(item) = drag.item
                {
                    if !drag.scrolling {
                        self.forward_to_item(ctx, event, Some(item));
                    }
                    if let Some(pod) = self.pods.get_mut(item) {
                        pod.set_active(false);
                    }
                }
                self.start_settle();
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    /// Choose and start the settle a released scroll lands on, reporting the
    /// item it will land on right away (the paint-driven ramp that follows
    /// carries no `EventCtx` to report from).
    fn release(&mut self, ctx: &mut EventCtx) {
        // The content moves opposite the finger, so a forward scroll is a
        // negative finger velocity (`physics`' sign convention).
        let scroll_velocity = -self.tracker.velocity();
        self.settle_snapped(ctx, scroll_velocity);
    }

    /// Snap onto the item a scroll at `scroll_velocity` lands on, reporting it.
    fn settle_snapped(&mut self, ctx: &mut EventCtx, scroll_velocity: f64) {
        let stride = self.solver(self.offset).stride();
        let target = physics::snap_offset(self.offset, stride, scroll_velocity, self.max_offset());
        self.report(ctx, target);
        if self.controlled {
            // Controlled: the content still has to settle back onto the item
            // the app last confirmed.
            self.start_settle();
            return;
        }
        self.index = physics::nearest_item(target, stride).min(self.pods.len().saturating_sub(1));
        self.settle_to(target);
    }

    /// The mouse-wheel arm: move the content by the wheel delta, then settle
    /// onto the nearest item boundary.
    ///
    /// Upstream has no wheel path of its own (its carousel is swipe-driven
    /// under `NeverScrollableScrollPhysics`); this is the port's desktop
    /// completion of it, taking [`crate::button_group`]'s per-axis delta
    /// selection and baseline `ScrollView`'s sign convention (a positive delta
    /// moves the content forward).
    fn wheel(&mut self, ctx: &mut EventCtx, delta: &ScrollDelta) -> EventResult {
        let (dx, dy) = match delta {
            ScrollDelta::Lines(x, y) => (x * WHEEL_LINE_PX, y * WHEEL_LINE_PX),
            ScrollDelta::Pixels(x, y) => (*x, *y),
        };
        // A horizontal carousel takes the wheel's horizontal axis when it
        // carries one, and its vertical axis otherwise (the usual shift-less
        // mouse over a horizontal strip).
        let main = match self.axis {
            CarouselAxis::Horizontal if dx != 0.0 => dx,
            CarouselAxis::Horizontal | CarouselAxis::Vertical => dy,
        };
        self.set_offset(self.offset + main);
        // A wheel tick carries no velocity of its own: it always settles onto
        // the nearest boundary.
        self.settle_snapped(ctx, 0.0);
        ctx.request_redraw();
        EventResult::Handled
    }

    /// Forward an event to one item's pod, if there is one under the pointer.
    fn forward_to_item(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        item: Option<usize>,
    ) -> EventResult {
        match item.and_then(|index| self.pods.get_mut(index)) {
            Some(pod) => pod.event_child(ctx, event),
            None => EventResult::Ignored,
        }
    }

    /// Advance the settle ramp from the shell frame clock, moving the content
    /// and asking for the next frame while it runs.
    fn advance_settle(&mut self, ctx: &mut PaintCtx, reduce_motion: bool) {
        if self.drag.is_some() || !self.settle.is_animating() {
            return;
        }
        let next = if reduce_motion {
            self.settle.stop();
            self.settle_to
        } else {
            if self.settle.advance(ctx.frame_time()) {
                ctx.request_frame();
            }
            let ramp = self.settle.value_clamped();
            self.settle_from + (self.settle_to - self.settle_from) * ramp
        };
        if (next - self.offset).abs() > OFFSET_EPSILON {
            self.set_offset(next);
        }
    }
}

/// The main-axis component of a size under `axis`.
fn content_main(axis: CarouselAxis, size: Size) -> f64 {
    match axis {
        CarouselAxis::Horizontal => size.width,
        CarouselAxis::Vertical => size.height,
    }
}

/// The cross-axis component of a size under `axis`.
fn content_cross(axis: CarouselAxis, size: Size) -> f64 {
    match axis {
        CarouselAxis::Horizontal => size.height,
        CarouselAxis::Vertical => size.width,
    }
}

/// A `(main, cross)` pair mapped onto `(x, y)` under `axis` — the one place
/// the vertical carousel differs from the horizontal one.
fn axis_point(axis: CarouselAxis, main: f64, cross: f64) -> Point {
    match axis {
        CarouselAxis::Horizontal => Point::new(main, cross),
        CarouselAxis::Vertical => Point::new(cross, main),
    }
}

/// The resolved item corner radius: the explicit builder value, then the
/// theme's `shape.extra_large`, then [`CAROUSEL_ITEM_RADIUS`].
fn resolve_radius(explicit: Option<f64>, theme: Option<&Theme>) -> f64 {
    explicit.unwrap_or(match theme {
        Some(theme) => theme.shape.extra_large,
        None => CAROUSEL_ITEM_RADIUS,
    })
}

/// The resolved state-layer content color for a pressed item. Themed:
/// `colors.on_surface`. Unthemed: [`ON_SURFACE`].
fn resolve_content_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface,
        None => ON_SURFACE,
    }
}

impl Widget for CarouselWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max = bc.max();
        // A carousel derives every extent from its viewport, so an unbounded
        // axis has nothing to derive from: it takes the pinned width/height if
        // one was given and collapses otherwise (upstream requires a bounded
        // box too — `M3ECarousel` reads `width ?? constraints.maxWidth`).
        let width = self
            .width
            .unwrap_or(if max.width.is_finite() {
                max.width
            } else {
                0.0
            })
            .max(0.0);
        let height = self
            .height
            .unwrap_or(if max.height.is_finite() {
                max.height
            } else {
                0.0
            })
            .max(0.0);
        let size = Size::new(width, height);
        (self.main, self.cross) = match self.axis {
            CarouselAxis::Horizontal => (width, height),
            CarouselAxis::Vertical => (height, width),
        };

        let content = self.content_size();
        for pod in self.pods.iter_mut() {
            pod.layout_child(ctx, &BoxConstraints::tight(content));
        }

        // A resize rescales every extent, and the carousel must stay on its own
        // item unless a gesture or a settle currently owns the offset.
        if self.drag.is_none() && !self.settle.is_animating() {
            self.index = self.index.min(self.pods.len().saturating_sub(1));
            let target = self.offset_for(self.index);
            self.offset = target;
        }
        self.set_offset(self.offset);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Record the shared frame clock so the between-frames event pass has a
        // timestamp for velocity tracking.
        self.last_frame_time = ctx.frame_time();
        let (radius, content_color, reduce_motion) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                resolve_radius(self.item_radius, theme),
                resolve_content_color(theme),
                theme.is_some_and(|t| t.motion.reduce_motion),
            )
        };
        self.advance_settle(ctx, reduce_motion);

        let origin = ctx.origin();
        let size = ctx.size();
        scene.push_clip(origin, size);
        // `slots` is a snapshot; the pods it names are borrowed mutably below.
        let slots = std::mem::take(&mut self.slots);
        for slot in &slots {
            let local = self.item_rect(slot);
            if local.width() <= 0.0 || local.height() <= 0.0 {
                continue;
            }
            let at = Point::new(origin.x + local.x0, origin.y + local.y0);
            let item_size = local.size();
            // The rounded clip *is* the item's shape: baseline `Clip` does not
            // visually clip (module docs). The radius is deliberately not
            // capped here — `kurbo::RoundedRect::from_rect` clamps it to half
            // the shortest side, so an item shrinking past `2 × radius` reads
            // as a pill rather than as an artefact.
            scene.push_clip_rounded(at, item_size, radius);
            if let Some(pod) = self.pods.get_mut(slot.index) {
                pod.paint_child(ctx, scene);
            }
            if self.pressed == Some(slot.index) {
                self.state.paint(
                    ctx,
                    scene,
                    Rect::from_origin_size(at, item_size),
                    radius,
                    content_color,
                );
            }
            scene.pop_clip();
        }
        self.slots = slots;
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.event_at(ctx, event, self.event_time_ms())
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let pods = &self.pods;
        ctx.push_container(
            Role::Group,
            |_| {},
            |ctx| {
                for pod in pods {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(pods);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{PointerButton, PointerEvent, PointerPhase, any};
    use std::any::Any;

    /// A 600×200 carousel: three `[5, 4, 1]` slots of 300/240/60px.
    const WINDOW: Size = Size::new(600.0, 200.0);
    /// The contained layout's leading-slot extent at [`WINDOW`] — one item step.
    const STRIDE: f64 = 300.0;

    #[derive(Default)]
    struct Reports {
        changes: Vec<CarouselChange>,
        taps: Vec<usize>,
        presses: usize,
    }

    /// A leaf that fills whatever it is given and counts the presses that reach
    /// it, generic over the app state.
    struct Item;

    /// The retained half of [`Item`].
    struct ItemWidget;

    impl<S: 'static> View<S> for Item {
        type Element = ItemWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ItemWidget {
            ItemWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ItemWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for ItemWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                ctx.state_mut::<Reports>().presses += 1;
                // An item with its own focusable content, so the carousel's
                // blur bookkeeping has something to clear.
                ctx.request_focus();
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    #[derive(Default)]
    struct RecordingScene {
        clips: Vec<(Point, Size)>,
        rounded: Vec<(Point, Size, f64)>,
        rrects: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn push_clip_rounded(&mut self, origin: Point, size: Size, radius: f64) {
            self.rounded.push((origin, size, radius));
        }
        fn pop_clip(&mut self) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
    }

    fn items(count: usize) -> Vec<AnyView<Reports>> {
        (0..count).map(|_| any(Item)).collect()
    }

    fn view(count: usize) -> CarouselView<Reports> {
        contained_carousel(items(count))
            .on_change(|s: &mut Reports, change: CarouselChange| s.changes.push(change))
            .on_tap(|s: &mut Reports, index: usize| s.taps.push(index))
    }

    fn build(v: &CarouselView<Reports>) -> CarouselWidget {
        let mut counter = 0u64;
        View::<Reports>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn laid_out(v: &CarouselView<Reports>) -> CarouselWidget {
        let mut w = build(v);
        let mut ctx = LayoutCtx::new();
        w.layout(&mut ctx, &BoxConstraints::loose(WINDOW));
        w
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn secondary_ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Secondary,
        })
    }

    /// Dispatch one event at an explicit timestamp (ms), the seam the velocity
    /// math is parameterised on.
    fn dispatch(w: &mut CarouselWidget, state: &mut Reports, event: &InputEvent, t_ms: f64) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        w.event_at(&mut ctx, event, t_ms);
    }

    fn paint(w: &mut CarouselWidget) -> RecordingScene {
        let mut ctx = PaintCtx::new(Point::ZERO, WINDOW);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        scene
    }

    /// Drive a started settle to completion without a shell clock, then apply
    /// the resulting offset the way `paint` would.
    fn finish_settle(w: &mut CarouselWidget) {
        w.settle.advance(FrameTime::from_nanos(0));
        w.settle.advance(FrameTime::from_nanos(2_000_000_000));
        let value = w.settle.value_clamped();
        let next = w.settle_from + (w.settle_to - w.settle_from) * value;
        w.set_offset(next);
    }

    // ---- layout wiring ----------------------------------------------------

    #[test]
    fn every_child_is_laid_out_at_the_largest_slot_less_its_padding() {
        let w = laid_out(&view(6));
        for pod in &w.pods {
            assert_eq!(
                pod.size(),
                Size::new(
                    STRIDE - CAROUSEL_ITEM_PADDING * 2.0,
                    WINDOW.height - CAROUSEL_ITEM_PADDING * 2.0
                ),
                "a slot's extent changes with the scroll; the child's does not"
            );
        }
    }

    #[test]
    fn the_visible_slots_are_the_layouts_own_weights() {
        let w = laid_out(&view(6));
        assert_eq!(
            w.slots.iter().map(|s| s.extent).collect::<Vec<_>>(),
            vec![300.0, 240.0, 60.0]
        );
    }

    #[test]
    fn items_beyond_the_viewport_are_parked_outside_the_carousels_bounds() {
        let w = laid_out(&view(6));
        for pod in &w.pods[3..] {
            assert!(
                pod.origin().x >= WINDOW.width || pod.origin().x + pod.size().width <= 0.0,
                "an invisible item must be unhittable, not merely unpainted"
            );
        }
    }

    #[test]
    fn a_vertical_carousel_slots_along_the_y_axis() {
        let w = laid_out(&view(6).axis(CarouselAxis::Vertical));
        // 200px of main axis over `[5, 4, 1]`: 100/80/20.
        assert_eq!(
            w.slots.iter().map(|s| s.extent).collect::<Vec<_>>(),
            vec![100.0, 80.0, 20.0]
        );
        assert_eq!(w.pods[0].origin().y, CAROUSEL_ITEM_PADDING);
        assert_eq!(
            w.pods[0].size().width,
            WINDOW.width - CAROUSEL_ITEM_PADDING * 2.0
        );
    }

    #[test]
    fn an_unbounded_axis_collapses_unless_a_size_is_pinned() {
        let mut w = build(&view(6));
        let mut ctx = LayoutCtx::new();
        let size = w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(f64::INFINITY, f64::INFINITY)),
        );
        assert_eq!(size, Size::ZERO);

        let mut w = build(&view(6).width(400.0).height(120.0));
        let mut ctx = LayoutCtx::new();
        let size = w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(f64::INFINITY, f64::INFINITY)),
        );
        assert_eq!(size, Size::new(400.0, 120.0));
    }

    #[test]
    fn the_hero_layouts_focal_item_is_offset_from_the_leading_one() {
        let w = laid_out(&view(6).layout(CarouselLayout::Hero));
        assert_eq!(w.focal_slot(), 1, "`[2, 6, 2]`'s max weight is in slot 1");
        assert_eq!(w.leading_index(), 0);
        assert_eq!(w.focal_index(), 1);

        let start = laid_out(
            &view(6)
                .layout(CarouselLayout::Hero)
                .hero_alignment(HeroAlignment::Start),
        );
        assert_eq!(start.focal_slot(), 0, "`[8, 2]` leads with its max weight");
    }

    #[test]
    fn an_uncontained_carousel_uses_a_fixed_extent_and_the_list_scroll_bound() {
        let w = laid_out(&uncontained_carousel(items(6), 200.0));
        assert_eq!(
            w.slots.iter().map(|s| s.extent).collect::<Vec<_>>(),
            vec![200.0, 200.0, 200.0]
        );
        assert_eq!(w.max_offset(), 6.0 * 200.0 - WINDOW.width);
        assert_eq!(
            w.focal_index(),
            w.leading_index(),
            "no focal slot to shift to"
        );
    }

    #[test]
    fn the_initial_item_names_the_focal_item_not_the_leading_one() {
        let w = laid_out(&view(8).layout(CarouselLayout::Hero).initial_item(3));
        assert_eq!(
            w.leading_index(),
            2,
            "`[2, 6, 2]` shifts the focal item by 1"
        );
        assert_eq!(w.focal_index(), 3);
        assert_eq!(
            w.offset(),
            2.0 * 120.0,
            "and rests on the leading slot item"
        );
    }

    #[test]
    fn consume_max_weight_makes_the_slot_item_the_focal_one() {
        // With the leading items allowed to expand into the max slot, the item
        // the offset rests on is the focal one and the *leading* one is the
        // phantom-shifted index behind it.
        let w = laid_out(
            &view(8)
                .layout(CarouselLayout::Hero)
                .consume_max_weight(true)
                .initial_item(3),
        );
        assert_eq!(w.focal_index(), 3);
        assert_eq!(w.leading_index(), 2);
        assert_eq!(w.offset(), 3.0 * 120.0);

        let at_rest = laid_out(
            &view(8)
                .layout(CarouselLayout::Hero)
                .consume_max_weight(true),
        );
        assert_eq!(
            at_rest.focal_index(),
            0,
            "item 0 occupies the largest slot at offset 0"
        );
    }

    // ---- paint ------------------------------------------------------------

    #[test]
    fn each_visible_item_is_painted_inside_its_own_rounded_clip() {
        let mut w = laid_out(&view(6));
        let scene = paint(&mut w);
        assert_eq!(scene.clips.len(), 1, "the carousel clips its own viewport");
        assert_eq!(scene.rounded.len(), 3, "one rounded clip per visible item");
        let (origin, size, radius) = scene.rounded[0];
        assert_eq!(
            origin,
            Point::new(CAROUSEL_ITEM_PADDING, CAROUSEL_ITEM_PADDING)
        );
        assert_eq!(
            size,
            Size::new(
                STRIDE - CAROUSEL_ITEM_PADDING * 2.0,
                WINDOW.height - CAROUSEL_ITEM_PADDING * 2.0
            )
        );
        assert_eq!(radius, CAROUSEL_ITEM_RADIUS);
    }

    #[test]
    fn a_theme_resolves_the_item_radius_and_an_explicit_value_wins() {
        let theme = crate::baseline();
        let mut w = laid_out(&view(6));
        let mut ctx = PaintCtx::new(Point::ZERO, WINDOW).with_theme(&theme);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert_eq!(scene.rounded[0].2, theme.shape.extra_large);

        let mut w = laid_out(&view(6).item_radius(4.0));
        let mut ctx = PaintCtx::new(Point::ZERO, WINDOW).with_theme(&theme);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert_eq!(scene.rounded[0].2, 4.0);
    }

    #[test]
    fn a_pressed_item_paints_the_state_layer_over_its_own_clip() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 100.0, 100.0),
            0.0,
        );
        let scene = paint(&mut w);
        assert_eq!(scene.rrects.len(), 1, "exactly the pressed item's overlay");
        assert_eq!(
            scene.rrects[0].0,
            Point::new(CAROUSEL_ITEM_PADDING, CAROUSEL_ITEM_PADDING)
        );
        assert_eq!(
            scene.rrects[0].3.components[3],
            crate::interaction::PRESSED_OPACITY
        );

        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 100.0, 100.0),
            16.0,
        );
        let scene = paint(&mut w);
        assert!(scene.rrects.is_empty(), "the overlay clears on release");
    }

    // ---- gestures and snapping --------------------------------------------

    #[test]
    fn a_press_reaches_the_item_under_it_and_a_tap_reports_that_item() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 400.0, 100.0),
            0.0,
        );
        assert_eq!(
            state.presses, 1,
            "the item under the pointer sees the press"
        );
        assert_eq!(w.pressed, Some(1));
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 400.0, 100.0),
            16.0,
        );
        assert_eq!(state.taps, vec![1]);
        assert!(w.pressed.is_none());
    }

    #[test]
    fn a_release_outside_the_pressed_item_reports_no_tap() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 100.0, 100.0),
            0.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 400.0, 100.0),
            16.0,
        );
        assert!(state.taps.is_empty());
    }

    #[test]
    fn a_secondary_press_reaches_the_item_but_arms_no_drag() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &secondary_ev(PointerPhase::Down, 100.0, 100.0),
            0.0,
        );
        assert!(w.drag.is_none());
        assert!(w.pressed.is_none());
    }

    #[test]
    fn a_drag_past_the_slop_takes_the_gesture_over_and_moves_the_content() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 400.0, 100.0),
            0.0,
        );
        // Inside the slop: still the item's gesture.
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 398.0, 100.0),
            8.0,
        );
        assert_eq!(w.offset, 0.0);
        assert_eq!(w.pressed, Some(1), "the press survives a sub-slop wobble");
        // Past it: the carousel takes over and the press is released.
        let past = 400.0 - TOUCH_SLOP - 5.0;
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, past, 100.0),
            16.0,
        );
        assert!(w.pressed.is_none());
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 250.0, 100.0),
            24.0,
        );
        assert_eq!(w.offset, 150.0, "the content follows the finger");
    }

    #[test]
    fn a_slow_release_mid_item_settles_onto_the_nearest_boundary() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        // Drag 160px (past the 150px half-item mark) slowly enough that the
        // release velocity is below the tolerance.
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 500.0, 100.0),
            0.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 400.0, 100.0),
            400.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 340.0, 100.0),
            900.0,
        );
        assert_eq!(w.offset, 160.0);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 340.0, 100.0),
            1000.0,
        );
        assert_eq!(w.settle_to, STRIDE, "settles forward to the item boundary");
        finish_settle(&mut w);
        assert_eq!(w.offset, STRIDE);
        assert_eq!(w.leading_index(), 1);
    }

    #[test]
    fn a_slow_release_before_the_halfway_mark_settles_back() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 500.0, 100.0),
            0.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 440.0, 100.0),
            500.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 420.0, 100.0),
            1000.0,
        );
        assert_eq!(w.offset, 80.0);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 420.0, 100.0),
            1100.0,
        );
        assert_eq!(w.settle_to, 0.0);
        finish_settle(&mut w);
        assert_eq!(w.offset, 0.0);
        assert_eq!(w.leading_index(), 0);
    }

    #[test]
    fn a_short_fast_fling_still_advances_one_whole_item() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 500.0, 100.0),
            0.0,
        );
        // The first move crosses the slop and only takes the gesture over; the
        // second scrolls. 60px in 16ms is 3750px/s — far past the fling
        // tolerance, and well short of the 150px a slow release would need.
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 470.0, 100.0),
            8.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 440.0, 100.0),
            16.0,
        );
        assert_eq!(w.offset, 60.0);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 440.0, 100.0),
            16.0,
        );
        assert_eq!(w.settle_to, STRIDE, "a fling advances one item");
        finish_settle(&mut w);
        assert_eq!(w.leading_index(), 1);
    }

    #[test]
    fn a_backward_fling_returns_to_the_previous_item() {
        let mut w = laid_out(&view(6));
        w.set_offset(STRIDE);
        w.index = 1;
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 300.0, 100.0),
            0.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 330.0, 100.0),
            8.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 360.0, 100.0),
            16.0,
        );
        assert_eq!(
            w.offset, 240.0,
            "only 60px back — a slow release would stay"
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 360.0, 100.0),
            16.0,
        );
        assert_eq!(w.settle_to, 0.0);
        finish_settle(&mut w);
        assert_eq!(w.leading_index(), 0);
    }

    #[test]
    fn a_scroll_never_leaves_the_scrollable_range() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 500.0, 100.0),
            0.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 100.0, 100.0),
            8.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, -5000.0, 100.0),
            16.0,
        );
        assert_eq!(w.offset, w.max_offset());
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 9000.0, 100.0),
            24.0,
        );
        assert_eq!(w.offset, 0.0);
    }

    #[test]
    fn a_carousel_with_nothing_to_scroll_never_takes_a_gesture_over() {
        // Three items in three slots: `max_scroll_offset` is zero.
        let mut w = laid_out(&view(3));
        assert!(!w.scrollable());
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 100.0, 100.0),
            0.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, -500.0, 100.0),
            16.0,
        );
        assert_eq!(w.offset, 0.0);
        assert_eq!(w.pressed, Some(0), "the press stays with the item");
    }

    #[test]
    fn a_wheel_tick_moves_the_content_and_settles_onto_a_boundary() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        let wheel = InputEvent::Scroll {
            position: Point::new(100.0, 100.0),
            delta: frust::authoring::ScrollDelta::Pixels(180.0, 0.0),
        };
        dispatch(&mut w, &mut state, &wheel, 0.0);
        assert_eq!(w.offset, 180.0, "the content follows the wheel");
        assert_eq!(w.settle_to, STRIDE, "and settles onto the nearest boundary");
        finish_settle(&mut w);
        assert_eq!(w.leading_index(), 1);
    }

    #[test]
    fn a_press_on_another_item_clears_the_previous_ones_focus_link() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 100.0, 100.0),
            0.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 100.0, 100.0),
            16.0,
        );
        assert!(
            w.pods[0].is_focused(),
            "the item claimed focus on its press"
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 400.0, 100.0),
            32.0,
        );
        assert!(!w.pods[0].is_focused(), "a press elsewhere blurs it");
        assert!(w.pods[1].is_focused());
    }

    #[test]
    fn a_cancel_clears_the_gesture_and_settles_back_without_reporting() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 500.0, 100.0),
            0.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 350.0, 100.0),
            16.0,
        );
        let reported = state.changes.len();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Cancel, 350.0, 100.0),
            24.0,
        );
        assert!(w.drag.is_none());
        assert!(w.pressed.is_none());
        assert_eq!(w.settle_to, 0.0, "back onto the item it started from");
        assert_eq!(state.changes.len(), reported, "a cancel reports nothing");
        assert!(state.taps.is_empty());
    }

    // ---- reporting and the controlled contract ----------------------------

    #[test]
    fn a_scroll_reports_the_leading_and_focal_item_at_the_halfway_mark() {
        let mut w = laid_out(&view(8).layout(CarouselLayout::Hero));
        let mut state = Reports::default();
        // `[2, 6, 2]` over 600px: a 120px stride, so halfway is 60px.
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 500.0, 100.0),
            0.0,
        );
        // Crosses the slop, taking the gesture over without scrolling yet.
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 470.0, 100.0),
            100.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 450.0, 100.0),
            200.0,
        );
        assert!(state.changes.is_empty(), "50px in: still nearest to item 0");
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 420.0, 100.0),
            400.0,
        );
        assert_eq!(
            state.changes.last().copied(),
            Some(CarouselChange {
                leading_index: 1,
                focal_index: 2,
                item_count: 8
            })
        );
    }

    #[test]
    fn a_controlled_carousel_reports_the_request_and_moves_nothing() {
        let mut w = laid_out(&view(6).selected(1));
        assert!(w.controlled);
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 500.0, 100.0),
            0.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 400.0, 100.0),
            400.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 340.0, 100.0),
            900.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 340.0, 100.0),
            1000.0,
        );
        assert!(!state.changes.is_empty(), "the request is still reported");
        assert_eq!(
            w.settle_to,
            w.offset_for(w.index),
            "it settles back onto the app-confirmed item"
        );
    }

    #[test]
    fn a_rebuild_with_a_new_selection_settles_onto_it() {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let first = view(8).selected(0);
        let mut w = View::<Reports>::build(&first, &mut ctx);
        let mut layout_ctx = LayoutCtx::new();
        w.layout(&mut layout_ctx, &BoxConstraints::loose(WINDOW));
        let next = view(8).selected(3);
        View::<Reports>::rebuild(&next, &first, &mut w, &mut ctx);
        assert_eq!(w.leading_index(), 3);
        finish_settle(&mut w);
        assert_eq!(w.offset, STRIDE * 3.0);
    }

    #[test]
    fn reduce_motion_lands_on_the_target_without_asking_for_a_frame() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 500.0, 100.0),
            0.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 400.0, 100.0),
            400.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 340.0, 100.0),
            900.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 340.0, 100.0),
            1000.0,
        );
        let mut ctx = PaintCtx::new(Point::ZERO, WINDOW).with_theme(&theme);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert_eq!(w.offset, STRIDE, "jumped straight to the target");
        assert!(!ctx.needs_frame(), "and asked for no continuation frame");
    }

    #[test]
    fn a_settle_asks_for_the_next_frame_while_it_runs() {
        let mut w = laid_out(&view(6));
        let mut state = Reports::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 500.0, 100.0),
            0.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 400.0, 100.0),
            400.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 340.0, 100.0),
            900.0,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 340.0, 100.0),
            1000.0,
        );
        let mut ctx = PaintCtx::new(Point::ZERO, WINDOW);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(ctx.needs_frame());
    }

    #[test]
    fn an_empty_carousel_lays_out_and_paints_without_panicking() {
        let mut w = laid_out(&view(0));
        assert!(w.slots.is_empty());
        let scene = paint(&mut w);
        assert!(scene.rounded.is_empty());
        let mut state = Reports::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0), 0.0);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0), 16.0);
        assert!(state.taps.is_empty());
    }

    #[test]
    fn a_zero_sized_carousel_degrades_instead_of_dividing_by_its_viewport() {
        let mut w = build(&view(6));
        let mut ctx = LayoutCtx::new();
        let size = w.layout(&mut ctx, &BoxConstraints::tight(Size::ZERO));
        assert_eq!(size, Size::ZERO);
        assert!(w.slots.is_empty());
        assert_eq!(w.max_offset(), 0.0);
        let scene = paint(&mut w);
        assert!(scene.rounded.is_empty());
    }
}
