//! Ports beUI's `infinite-masonry` block — `components/motion/infinite-masonry.tsx`
//! (beUI rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01),
//! registry slug `infinite-masonry`: *"Responsive virtualized masonry that
//! measures variable-height cards and loads more data as the user nears the
//! end."*
//!
//! | upstream | here |
//! |---|---|
//! | `minColumnWidth = 208`, `maxColumns = 4`, `gap = 12` | [`MASONRY_MIN_COLUMN_WIDTH`], [`MASONRY_MAX_COLUMNS`], [`MASONRY_GAP`] |
//! | `Math.min(max, Math.max(1, Math.floor((w + gap) / (minW + gap))))` | [`masonry_columns`] |
//! | `(width - gap * (columns - 1)) / columns` | [`masonry_column_width`] |
//! | `useVirtualizer({ lanes: columns })`'s shortest-lane packing | [`masonry_pack`] |
//! | `prefetch = 3` plus `loadPendingRef` | [`MasonryLoadGate`] |
//! | `MasonryItemReveal` `{opacity 0→1, y 12→0}` | [`MASONRY_REVEAL_TRAVEL`] |
//! | `delay = Math.min(lane, 3) * 0.04` | [`MASONRY_LANE_DELAY`], [`MASONRY_LANE_DELAY_CAP`] |
//! | `y: SPRING_PANEL`, `opacity: 0.2s EASE_OUT` | [`MASONRY_FADE`] over `SPRING_PANEL` |
//! | `renderLoadingItem` skeletons, `columns` of them | [`InfiniteMasonryView::loading`] |
//! | the error card and its `Try again` | [`InfiniteMasonryView::error`] / [`InfiniteMasonryView::on_retry`] |
//! | `emptyState` / `endState` | [`InfiniteMasonryView::empty_label`] / [`InfiniteMasonryView::end_label`] |
//!
//! # The virtualization mechanism, and its limits
//!
//! The porting card asks for this to be built on [`frust::ListView`] *if its
//! variable-extent contract can host a masonry column layout*. It cannot, for a
//! structural reason: `ListView` maps one linear item index onto one vertical
//! slot stacked after the previous one, and its whole O(window) offset-to-index
//! walk depends on that. A masonry has `n` independent lane cursors, and its
//! item `i` sits after whichever *earlier* item last landed in the lane `i` is
//! packed into — a mapping `ListView`'s prefix anchor cannot express. Wrapping
//! [`frust::ScrollView`] is out for the reason
//! [`message_scroller`](crate::agents::message_scroller)'s module docs record at
//! length: the baseline publishes neither an offset read seam nor an offset
//! write seam, so a wrapped scroller could be neither windowed against nor
//! re-pinned.
//!
//! So this widget is a scroll surface in its own right — it clips a viewport,
//! owns the offset, and consumes the wheel and drag itself — and virtualizes by
//! **windowing an already-packed slot table**:
//!
//! 1. `layout` packs every declared item into lanes ([`masonry_pack`]) from its
//!    *declared* extent, producing one `(lane, y, height)` slot per item plus
//!    the content height.
//! 2. The window is the slot range intersecting
//!    `offset - overscan ..= offset + viewport + overscan`, resolved by
//!    [`masonry_window`].
//! 3. Only windowed items are shaped and painted. Everything outside costs one
//!    interval test.
//!
//! **The limits this leaves, stated rather than left to be discovered:**
//!
//! - **Extents are declared, not measured.** [`MasonryItem::extent`] takes the
//!   card's height as data. Upstream re-measures each mounted card through
//!   `virtualizer.measureElement` and corrects its estimate; there is no
//!   measure-then-correct loop here, because the cards are painted by this
//!   widget rather than mounted as arbitrary children (next point).
//! - **The packing pass is O(items) per layout, not O(window).** There is no
//!   retained prefix anchor: a lane cursor cannot be resumed at a windowed index
//!   without knowing every earlier item's lane. What the mechanism buys is the
//!   O(window) *shaping and paint* cost, which is the expensive half; the
//!   arithmetic is a few flops per item.
//! - **Cards are data, not child views.** A `renderItem` equivalent would need
//!   the window's views built at layout time, and a view builder never runs
//!   outside `rebuild` — the same constraint `ListView`'s own docs state. An
//!   item is therefore a title, a caption and an extent, painted by this block.
//! - **No fling.** The offset follows the wheel and the drag and stops with
//!   them; the baseline's fling-decay math is not carried here.
//!
//! # `on_near_end` fires from the event pass, once per approach
//!
//! [`MasonryLoadGate`] is upstream's `loadPendingRef` as a pure state machine:
//! it latches on the approach that fires and unlatches only once the app has
//! answered (`loading` fell back to `false`, or the item count grew). It is
//! consulted from the **scroll/drag event arm** — never from `paint`, which
//! carries no `&mut State` — so a viewport parked inside the prefetch band
//! reports one approach, not one per frame.
//!
//! One consequence worth stating: a masonry whose items all fit the viewport at
//! mount never scrolls, so it never reports an approach. Upstream fires from an
//! effect and would; a caller who wants that asks for the first page itself.
//!
//! # Degradations against the web original
//!
//! - **No blur, and no `animate-pulse` skeleton shimmer.** The skeleton is a
//!   static muted card; `filter: blur()` and CSS keyframe pulses have no
//!   `PaintScene` primitive.
//! - **No scrollbar gutter and no `overflow-anchor` control.** Both are browser
//!   scroll-container features with no framework equivalent.
//! - **`aria-live` "Loading more items" is not announced.** The framework's
//!   semantics surface is role/label/state only — live regions are a shell
//!   concern, which `docs/CODE_STANDARDS.md`'s Semantics Conventions state.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, BoxConstraints, BuildCtx, ChangeFlags, Color, ErasedCallback, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerEvent, PointerPhase, Rect, Role,
    ScrollDelta, SemanticsCtx, Size, Vec2, View, Widget, erase_callback, text::TextStyle,
};
use frust::input::{TOUCH_SLOP, WHEEL_LINE_PX};
use frust::{FrameTime, Theme};

use crate::components::popover::{PanelChrome, paint_panel_hairline, resolve_panel};
use crate::motion::Ramp;
use crate::press::{inside, presses};
use crate::style;
use crate::text::LabelRun;
use crate::tokens::motion::{EASE_OUT, SPRING_PANEL};

// ---- Metrics ---------------------------------------------------------------

/// `minColumnWidth = 208` — the least a column may be before one is dropped.
pub const MASONRY_MIN_COLUMN_WIDTH: f64 = 208.0;

/// `maxColumns = 4` — the most columns the grid ever resolves to.
pub const MASONRY_MAX_COLUMNS: usize = 4;

/// `gap = 12` — the gap between columns and between stacked cards.
pub const MASONRY_GAP: f64 = 12.0;

/// `p-3` — the scroll surface's own padding, in logical px.
pub const MASONRY_PADDING: f64 = 12.0;

/// `prefetch = 3` — how many items from the end an approach fires at.
pub const MASONRY_PREFETCH: usize = 3;

/// How far past each viewport edge the window still materializes, in logical
/// px.
///
/// Upstream counts overscan in *rows*, which it can because its virtualizer
/// knows a row's estimate. This window is resolved against the packed slot
/// table in content space, so the same intent is expressed as a distance — one
/// default card extent per edge.
pub const MASONRY_OVERSCAN: f64 = 240.0;

/// `estimateSize = () => 240` — the extent a card starts from.
pub const MASONRY_DEFAULT_EXTENT: f64 = 240.0;

/// `rounded-3xl` — the surface's own corner radius.
pub const MASONRY_SURFACE_RADIUS: f64 = style::RADIUS_3XL;

/// `rounded-2xl` — one card's corner radius.
pub const MASONRY_CARD_RADIUS: f64 = style::RADIUS_2XL;

/// A card's inner padding, in logical px — upstream's `p-3` skeleton and `p-4`
/// error card are one box here, so they take one value.
pub const MASONRY_CARD_PADDING: f64 = 14.0;

/// `min-h-64` — the empty state's own height, in logical px.
pub const MASONRY_EMPTY_HEIGHT: f64 = 256.0;

/// `min-h-36` — the error card's height, in logical px.
pub const MASONRY_ERROR_HEIGHT: f64 = 144.0;

/// The skeleton card's base height (`144 + (index % 3) * 36`).
pub const MASONRY_SKELETON_BASE: f64 = 144.0;

/// The per-slot step of that same expression.
pub const MASONRY_SKELETON_STEP: f64 = 36.0;

/// `bg-destructive/5` — the alpha the error card's own wash is tinted at.
pub const MASONRY_ERROR_WASH_ALPHA: f32 = 0.05;

/// The alpha one skeleton bar (`bg-muted`) is drawn at.
pub const MASONRY_SKELETON_BAR_ALPHA: f32 = 0.25;

/// The vertical pitch of the skeleton's three bars, in logical px.
pub const MASONRY_SKELETON_PITCH: f64 = 14.0;

// ---- Motion ----------------------------------------------------------------

/// `initial={{ y: 12 }}` — how far below its slot a revealing card starts, in
/// logical px.
pub const MASONRY_REVEAL_TRAVEL: f64 = 12.0;

/// `delay = Math.min(lane, 3) * 0.04` — the per-lane reveal offset.
pub const MASONRY_LANE_DELAY: Duration = Duration::from_millis(40);

/// The `Math.min(lane, 3)` cap on that offset: lane 4 and beyond share lane 3's
/// delay rather than trailing further behind.
pub const MASONRY_LANE_DELAY_CAP: usize = 3;

/// `opacity: { duration: 0.2, ease: EASE_OUT }` — the reveal's fade.
pub const MASONRY_FADE: Duration = Duration::from_millis(200);

// ---- Items -----------------------------------------------------------------

/// One card in the feed — a title, an optional caption, and the extent the pack
/// places it by.
#[derive(Clone, Debug, PartialEq)]
pub struct MasonryItem {
    title: String,
    caption: String,
    extent: f64,
}

/// A card showing `title`, [`MASONRY_DEFAULT_EXTENT`] tall.
pub fn masonry_item(title: impl Into<String>) -> MasonryItem {
    MasonryItem {
        title: title.into(),
        caption: String::new(),
        extent: MASONRY_DEFAULT_EXTENT,
    }
}

impl MasonryItem {
    /// Set the card's second line.
    pub fn caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = caption.into();
        self
    }

    /// Set the card's own height, in logical px.
    ///
    /// A non-positive or non-finite extent falls back to
    /// [`MASONRY_DEFAULT_EXTENT`]: a zero-height card would leave two lane
    /// cursors indistinguishable, which is the one input that makes a pack
    /// wrong rather than merely ugly.
    pub fn extent(mut self, extent: f64) -> Self {
        self.extent = sane_extent(extent);
        self
    }

    /// The card's title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The card's caption.
    pub fn item_caption(&self) -> &str {
        &self.caption
    }

    /// The card's height, in logical px.
    pub fn item_extent(&self) -> f64 {
        self.extent
    }
}

/// One extent with the fallback [`MasonryItem::extent`] documents applied.
fn sane_extent(extent: f64) -> f64 {
    if extent.is_finite() && extent > 0.0 {
        extent
    } else {
        MASONRY_DEFAULT_EXTENT
    }
}

// ---- The layout arithmetic -------------------------------------------------

/// How many columns `width` resolves to.
///
/// Upstream's `Math.min(maxColumns, Math.max(1, Math.floor((width + gap) /
/// (minColumnWidth + gap))))`, kept literal. Always at least one — a viewport
/// too narrow for a full column still shows a squeezed one rather than nothing
/// — and never more than `max_columns`.
///
/// A non-finite width, or a `min_column_width + gap` that is not positive,
/// resolves to one column: there is no meaningful division to take.
pub fn masonry_columns(width: f64, gap: f64, min_column_width: f64, max_columns: usize) -> usize {
    let cap = max_columns.max(1);
    let pitch = min_column_width + gap;
    if !width.is_finite() || pitch <= 0.0 {
        return 1;
    }
    let fits = ((width + gap) / pitch).floor();
    if !fits.is_finite() || fits < 1.0 {
        return 1;
    }
    (fits as usize).clamp(1, cap)
}

/// One column's width: `(width - gap * (columns - 1)) / columns`, floored at
/// zero.
pub fn masonry_column_width(width: f64, gap: f64, columns: usize) -> f64 {
    if columns == 0 {
        return 0.0;
    }
    ((width - gap * (columns - 1) as f64) / columns as f64).max(0.0)
}

/// Where one packed card sits: which lane it landed in, its content-space top,
/// and its own height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MasonrySlot {
    /// The column this card was packed into, in `0..columns`.
    pub lane: usize,
    /// The card's top edge in content space, in logical px.
    pub y: f64,
    /// The card's own height, in logical px.
    pub height: f64,
}

impl MasonrySlot {
    /// The card's bottom edge in content space.
    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }
}

/// Pack `extents` into `columns` lanes — the balancing rule
/// `@tanstack/react-virtual`'s `lanes` option implements, restated as a pure
/// function.
///
/// The first `columns` items seed lane `index % columns` at `y = 0`. Every later
/// item goes to the lane whose current end is **smallest**, ties broken by the
/// smaller index of the item already sitting at that end (upstream's
/// `getFurthestMeasurement` sorts by `end`, then by `index`), and starts one
/// `gap` below it.
///
/// Zero columns, or an empty item list, pack to nothing.
pub fn masonry_pack(extents: &[f64], columns: usize, gap: f64) -> Vec<MasonrySlot> {
    if columns == 0 {
        return Vec::new();
    }
    // Per lane: the end of its last card, and that card's item index.
    let mut ends = vec![(0.0f64, usize::MAX); columns];
    let mut slots = Vec::with_capacity(extents.len());
    for (index, extent) in extents.iter().copied().enumerate() {
        let height = sane_extent(extent);
        let (lane, y) = if index < columns {
            (index % columns, 0.0)
        } else {
            let lane = (0..columns)
                .min_by(|&a, &b| {
                    ends[a]
                        .0
                        .total_cmp(&ends[b].0)
                        .then_with(|| ends[a].1.cmp(&ends[b].1))
                })
                .unwrap_or(0);
            (lane, ends[lane].0 + gap)
        };
        ends[lane] = (y + height, index);
        slots.push(MasonrySlot { lane, y, height });
    }
    slots
}

/// The content height `slots` occupy: the lowest bottom edge, or zero when
/// nothing was packed.
pub fn masonry_content_height(slots: &[MasonrySlot]) -> f64 {
    slots.iter().map(MasonrySlot::bottom).fold(0.0f64, f64::max)
}

/// The half-open slot range intersecting the viewport `offset ..= offset +
/// viewport`, widened by `overscan` at each edge, over `slots`' own indices.
///
/// The scan is a filter, not a binary search: a masonry's slots are ordered by
/// *item* index, not by `y` (a short lane keeps taking items long after a tall
/// one stopped), so a sorted-position search would be wrong. Taking the first
/// and last intersecting indices is what keeps the returned range contiguous in
/// item order, which is what the shaping loop needs.
pub fn masonry_window(
    slots: &[MasonrySlot],
    offset: f64,
    viewport: f64,
    overscan: f64,
) -> std::ops::Range<usize> {
    let top = offset - overscan;
    let bottom = offset + viewport + overscan;
    let mut start = slots.len();
    let mut end = 0usize;
    for (index, slot) in slots.iter().enumerate() {
        if slot.bottom() >= top && slot.y <= bottom {
            start = start.min(index);
            end = end.max(index + 1);
        }
    }
    if start >= end { 0..0 } else { start..end }
}

// ---- The load gate ---------------------------------------------------------

/// Upstream's `loadPendingRef`, as a pure state machine: whether the feed is
/// waiting on a load it already asked for.
///
/// Kept separate from the widget so "exactly once per approach" is testable
/// against a synthetic sequence rather than only through a scroll gesture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MasonryLoadGate {
    pending: bool,
}

impl MasonryLoadGate {
    /// A gate with nothing outstanding.
    pub const fn new() -> Self {
        MasonryLoadGate { pending: false }
    }

    /// Whether a load this gate asked for is still outstanding.
    pub const fn is_pending(&self) -> bool {
        self.pending
    }

    /// Clear the latch — the app answered. Called when `loading` falls back to
    /// `false` or the item count grows.
    pub fn release(&mut self) {
        self.pending = false;
    }

    /// Whether the viewport's `last_visible` item (an index, or `None` when
    /// nothing is on screen) is close enough to the end to fire an approach —
    /// latching it when it is.
    ///
    /// Upstream's guard verbatim: nothing fires while an error is showing,
    /// while a load is running, when there is no more data, when one is already
    /// outstanding, or while the last visible item is short of
    /// `items - prefetch`.
    pub fn approach(
        &mut self,
        last_visible: Option<usize>,
        items: usize,
        prefetch: usize,
        has_more: bool,
        loading: bool,
        has_error: bool,
    ) -> bool {
        if has_error || loading || !has_more || self.pending {
            return false;
        }
        let Some(last) = last_visible else {
            return false;
        };
        if last < items.saturating_sub(prefetch) {
            return false;
        }
        self.pending = true;
        true
    }
}

// ---- The component ---------------------------------------------------------

/// A view-held callback (erased on build).
type OnFeed<State> = Rc<dyn Fn(&mut State)>;

/// What the feed renders from, beyond its items.
#[derive(Clone, Debug, PartialEq)]
struct FeedConfig {
    has_more: bool,
    loading: bool,
    error: Option<String>,
    retryable: bool,
    min_column_width: f64,
    max_columns: usize,
    gap: f64,
    prefetch: usize,
    overscan: f64,
    animate_items: bool,
    empty_label: String,
    end_label: String,
    retry_label: String,
    label: String,
}

/// A declarative beUI infinite masonry. See [`infinite_masonry`].
pub struct InfiniteMasonryView<State: 'static> {
    items: Vec<MasonryItem>,
    config: FeedConfig,
    on_near_end: OnFeed<State>,
    on_retry: OnFeed<State>,
}

/// Build an infinite masonry over `items`, asking for more through
/// `on_near_end`.
///
/// `on_near_end(state)` fires once per approach of the prefetch band and does
/// not fire again until the app answers — by flipping
/// [`loading`](InfiniteMasonryView::loading) back off, or by handing over more
/// items. See the [module docs](self) for when an approach can be observed at
/// all.
pub fn infinite_masonry<State: 'static, F: Fn(&mut State) + 'static>(
    items: Vec<MasonryItem>,
    on_near_end: F,
) -> InfiniteMasonryView<State> {
    InfiniteMasonryView {
        items,
        config: FeedConfig {
            has_more: true,
            loading: false,
            error: None,
            retryable: false,
            min_column_width: MASONRY_MIN_COLUMN_WIDTH,
            max_columns: MASONRY_MAX_COLUMNS,
            gap: MASONRY_GAP,
            prefetch: MASONRY_PREFETCH,
            overscan: MASONRY_OVERSCAN,
            animate_items: true,
            empty_label: "No items yet".to_string(),
            end_label: String::new(),
            retry_label: "Try again".to_string(),
            label: "Infinite masonry feed".to_string(),
        },
        on_near_end: Rc::new(on_near_end),
        on_retry: Rc::new(|_| {}),
    }
}

impl<State: 'static> InfiniteMasonryView<State> {
    /// Whether more data exists behind the last item (`hasMore`).
    pub fn has_more(mut self, has_more: bool) -> Self {
        self.config.has_more = has_more;
        self
    }

    /// Whether a load is in flight (`loading`) — shows one skeleton per column
    /// and holds the gate shut.
    pub fn loading(mut self, loading: bool) -> Self {
        self.config.loading = loading;
        self
    }

    /// Show the error tail instead of the skeletons (`error`).
    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.config.error = Some(error.into());
        self
    }

    /// Set the retry callback, which also makes the error tail's button appear
    /// (upstream's `onRetry`).
    pub fn on_retry<F: Fn(&mut State) + 'static>(mut self, on_retry: F) -> Self {
        self.on_retry = Rc::new(on_retry);
        self.config.retryable = true;
        self
    }

    /// Set the least a column may be (`minColumnWidth`).
    pub fn min_column_width(mut self, width: f64) -> Self {
        self.config.min_column_width = width;
        self
    }

    /// Set the column cap (`maxColumns`).
    pub fn max_columns(mut self, columns: usize) -> Self {
        self.config.max_columns = columns.max(1);
        self
    }

    /// Set the grid gap (`gap`).
    pub fn gap(mut self, gap: f64) -> Self {
        self.config.gap = gap.max(0.0);
        self
    }

    /// Set how many items from the end an approach fires at (`prefetch`).
    pub fn prefetch(mut self, prefetch: usize) -> Self {
        self.config.prefetch = prefetch;
        self
    }

    /// Set how far past each viewport edge the window reaches, in logical px.
    pub fn overscan(mut self, overscan: f64) -> Self {
        self.config.overscan = overscan.max(0.0);
        self
    }

    /// Turn the per-card reveal off (`animateItems = false`).
    pub fn animate_items(mut self, animate: bool) -> Self {
        self.config.animate_items = animate;
        self
    }

    /// Set the empty-state line (`emptyState`).
    pub fn empty_label(mut self, label: impl Into<String>) -> Self {
        self.config.empty_label = label.into();
        self
    }

    /// Set the end-of-feed line (`endState`).
    pub fn end_label(mut self, label: impl Into<String>) -> Self {
        self.config.end_label = label.into();
        self
    }

    /// Set the feed's accessible name (`ariaLabel`).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.config.label = label.into();
        self
    }
}

/// One retained card: its shaped runs and its declared extent.
struct Card {
    title: LabelRun,
    caption: LabelRun,
    extent: f64,
}

/// The retained widget for an [`InfiniteMasonryView`].
pub struct MasonryWidget {
    cards: Vec<Card>,
    config: FeedConfig,
    empty: LabelRun,
    end: LabelRun,
    error: LabelRun,
    retry: LabelRun,
    /// The packed table as of the last layout, one slot per card.
    slots: Vec<MasonrySlot>,
    /// The tail slots — skeletons, or the error card — packed after the cards.
    tail: Vec<MasonrySlot>,
    /// The window into [`MasonryWidget::slots`] the last layout resolved.
    window: std::ops::Range<usize>,
    /// How many columns the last layout resolved to.
    columns: usize,
    /// One column's width as of the last layout.
    column_width: f64,
    /// The packed content height, in logical px.
    content_height: f64,
    /// The scroll offset this widget owns, in content space.
    offset: f64,
    /// The scrollable viewport's height as of the last layout.
    viewport: f64,
    /// Set when an event moved the offset: an event pass cannot ask for a
    /// relayout, so the next paint does (the `overflow_actions` arrangement).
    relayout_owed: bool,
    /// The first item index that plays the reveal — items below it were already
    /// on screen (upstream's `initialItemCountRef`/`revealedKeys` pair).
    reveal_from: usize,
    /// The frame the current reveal run started on.
    reveal_start: Option<FrameTime>,
    /// The load latch.
    gate: MasonryLoadGate,
    /// Whether a pointer drag has taken the gesture over.
    dragging: bool,
    /// Where the live press last was, while one is held.
    down: Option<Point>,
    /// The retry button's box in the widget's own space, or `Rect::ZERO`.
    retry_box: Rect,
    /// Whether the retry button holds a press.
    retry_pressed: bool,
    on_near_end: ErasedCallback,
    on_retry: ErasedCallback,
}

impl MasonryWidget {
    /// The scroll offset, in logical px from content start.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// How many columns the last layout resolved to.
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// One column's width as of the last layout, in logical px.
    pub fn column_width(&self) -> f64 {
        self.column_width
    }

    /// The packed slot table as of the last layout, one entry per declared item.
    pub fn slots(&self) -> &[MasonrySlot] {
        &self.slots
    }

    /// The materialized window into [`slots`](Self::slots).
    pub fn window(&self) -> std::ops::Range<usize> {
        self.window.clone()
    }

    /// The packed content height, in logical px.
    pub fn content_height(&self) -> f64 {
        self.content_height
    }

    /// Whether a load this widget asked for is still outstanding.
    pub fn is_load_pending(&self) -> bool {
        self.gate.is_pending()
    }

    /// The content-space `y` the viewport's top edge sits at.
    fn content_top(&self) -> f64 {
        self.offset - MASONRY_PADDING
    }

    /// The furthest offset the viewport may reach.
    fn max_offset(&self) -> f64 {
        (self.content_height + MASONRY_PADDING * 2.0 - self.viewport).max(0.0)
    }

    /// Move the offset, clamped to the content, reporting whether it moved.
    fn set_offset(&mut self, offset: f64) -> bool {
        let next = offset.clamp(0.0, self.max_offset());
        if next == self.offset {
            return false;
        }
        self.offset = next;
        true
    }

    /// The last item index the *viewport* (not the overscanned window) shows,
    /// which is what an approach is measured against.
    fn last_visible(&self) -> Option<usize> {
        let window = masonry_window(&self.slots, self.content_top(), self.viewport, 0.0);
        if window.is_empty() {
            None
        } else {
            Some(window.end - 1)
        }
    }

    /// Consult the gate and fire the approach, from the event pass that moved
    /// the offset.
    fn maybe_load(&mut self, ctx: &mut EventCtx) {
        let fired = self.gate.approach(
            self.last_visible(),
            self.cards.len(),
            self.config.prefetch,
            self.config.has_more,
            self.config.loading,
            self.config.error.is_some(),
        );
        if fired {
            (self.on_near_end)(ctx);
        }
    }

    /// Move the offset from a user gesture and re-test the prefetch band.
    fn user_scroll(&mut self, ctx: &mut EventCtx, delta: f64) {
        if self.set_offset(self.offset + delta) {
            self.relayout_owed = true;
            ctx.request_redraw();
        }
        self.maybe_load(ctx);
    }

    /// How many tail cards the feed is showing: one for an error, one per
    /// column while loading, none otherwise (upstream's `tailCount`).
    fn tail_count(&self) -> usize {
        if self.config.error.is_some() {
            1
        } else if self.config.loading {
            self.columns
        } else {
            0
        }
    }

    /// The tail card's extent at `index` — the error card's fixed height, or
    /// the skeleton's `144 + (index % 3) * 36`.
    fn tail_extent(&self, index: usize) -> f64 {
        if self.config.error.is_some() {
            MASONRY_ERROR_HEIGHT
        } else {
            MASONRY_SKELETON_BASE + (index % 3) as f64 * MASONRY_SKELETON_STEP
        }
    }

    /// Whether the feed is showing its empty state instead of a grid.
    fn is_empty(&self) -> bool {
        self.cards.is_empty()
            && !self.config.has_more
            && !self.config.loading
            && self.config.error.is_none()
    }

    /// One card's reveal at `elapsed` into the run: its alpha, and how far below
    /// its slot it still sits.
    fn reveal(&self, index: usize, elapsed: Duration, reduce: bool) -> (f64, f64) {
        if reduce || !self.config.animate_items || index < self.reveal_from {
            return (1.0, 0.0);
        }
        let lane = self
            .slots
            .get(index)
            .map_or(0, |slot| slot.lane.min(MASONRY_LANE_DELAY_CAP));
        let delay = MASONRY_LANE_DELAY * lane as u32;
        let Some(after) = elapsed.checked_sub(delay) else {
            return (0.0, MASONRY_REVEAL_TRAVEL);
        };
        let alpha = Ramp::eased(MASONRY_FADE, EASE_OUT).progress_clamped(after);
        let rise = Ramp::spring(SPRING_PANEL).progress(after).clamp(0.0, 1.0);
        (alpha, (1.0 - rise) * MASONRY_REVEAL_TRAVEL)
    }

    /// The whole reveal run's length: the last lane's delay plus the slower of
    /// its two ramps.
    fn reveal_span() -> Duration {
        MASONRY_LANE_DELAY * MASONRY_LANE_DELAY_CAP as u32
            + Ramp::spring(SPRING_PANEL).settle().max(MASONRY_FADE)
    }

    /// Whether any card still owes a reveal frame at `elapsed`.
    fn reveal_running(&self, elapsed: Duration, reduce: bool) -> bool {
        if reduce || !self.config.animate_items || self.reveal_from >= self.cards.len() {
            return false;
        }
        elapsed < Self::reveal_span()
    }
}

/// Build the retained cards for a declared list.
fn cards(items: &[MasonryItem]) -> Vec<Card> {
    items
        .iter()
        .map(|item| Card {
            title: LabelRun::new(item.title.clone()),
            caption: LabelRun::new(item.caption.clone()),
            extent: item.extent,
        })
        .collect()
}

impl<State: 'static> View<State> for InfiniteMasonryView<State> {
    type Element = MasonryWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> MasonryWidget {
        MasonryWidget {
            cards: cards(&self.items),
            empty: LabelRun::new(self.config.empty_label.clone()),
            end: LabelRun::new(self.config.end_label.clone()),
            error: LabelRun::new(self.config.error.clone().unwrap_or_default()),
            retry: LabelRun::new(self.config.retry_label.clone()),
            config: self.config.clone(),
            slots: Vec::new(),
            tail: Vec::new(),
            window: 0..0,
            columns: 1,
            column_width: 0.0,
            content_height: 0.0,
            offset: 0.0,
            viewport: 0.0,
            relayout_owed: false,
            // `virtualItem.index >= initialItemCountRef.current`: what is
            // already there on mount is simply there, it does not play in.
            reveal_from: self.items.len(),
            reveal_start: None,
            gate: MasonryLoadGate::new(),
            dragging: false,
            down: None,
            retry_box: Rect::ZERO,
            retry_pressed: false,
            on_near_end: erase_callback(&self.on_near_end),
            on_retry: erase_callback(&self.on_retry),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MasonryWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        if prev.items != self.items {
            if self.items.len() > element.cards.len() {
                // The appended tail reveals; everything already on screen stays
                // put, which is `revealedKeys`' whole job upstream.
                element.reveal_from = element.cards.len();
                element.reveal_start = None;
                element.gate.release();
            } else {
                element.reveal_from = self.items.len();
            }
            element.cards = cards(&self.items);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if element.config != self.config {
            if element.config.loading && !self.config.loading {
                element.gate.release();
            }
            element.empty.set_content(self.config.empty_label.clone());
            element.end.set_content(self.config.end_label.clone());
            element
                .error
                .set_content(self.config.error.clone().unwrap_or_default());
            element.retry.set_content(self.config.retry_label.clone());
            element.config = self.config.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Closures are not comparable; reinstalling the adapters is cheap.
        element.on_near_end = erase_callback(&self.on_near_end);
        element.on_retry = erase_callback(&self.on_retry);
        flags
    }

    fn teardown(&self, _element: &mut MasonryWidget, _ctx: &mut BuildCtx<'_>) {}
}

/// The label family: the theme's own scale, with the catalog's sans stack as
/// the unthemed fallback.
fn family_of(theme: Option<&Theme>) -> frust::authoring::text::FontFamily {
    theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    })
}

/// A card title's style.
fn title_style(theme: Option<&Theme>) -> TextStyle {
    TextStyle {
        family: family_of(theme),
        ..crate::text::label_style(style::TEXT_SM)
    }
}

/// A card caption's style.
fn caption_style(theme: Option<&Theme>) -> TextStyle {
    TextStyle {
        family: family_of(theme),
        ..crate::text::label_style(style::TEXT_XS)
    }
}

impl Widget for MasonryWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let title = title_style(theme);
        let caption = caption_style(theme);

        let outer = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            MASONRY_MIN_COLUMN_WIDTH * MASONRY_MAX_COLUMNS as f64
        };
        let inner = (outer - MASONRY_PADDING * 2.0).max(0.0);
        self.columns = masonry_columns(
            inner,
            self.config.gap,
            self.config.min_column_width,
            self.config.max_columns,
        );
        self.column_width = masonry_column_width(inner, self.config.gap, self.columns);

        if self.is_empty() {
            self.slots.clear();
            self.tail.clear();
            self.window = 0..0;
            self.content_height = 0.0;
            self.viewport = MASONRY_EMPTY_HEIGHT;
            self.empty.layout(ctx, &title);
            return bc.constrain(Size::new(outer, MASONRY_EMPTY_HEIGHT));
        }

        // Pack the cards, then the tail after them — upstream packs one list of
        // `items.length + tailCount` entries, so the tail lands in whichever
        // lanes the cards left shortest.
        let tail_count = self.tail_count();
        let mut extents: Vec<f64> = self.cards.iter().map(|card| card.extent).collect();
        for index in 0..tail_count {
            extents.push(self.tail_extent(index));
        }
        let packed = masonry_pack(&extents, self.columns, self.config.gap);
        self.tail = packed[self.cards.len()..].to_vec();
        self.slots = packed[..self.cards.len()].to_vec();
        self.content_height = masonry_content_height(&packed);

        // The end line sits below the scroll box, as upstream's does.
        let end_room = if self.config.end_label.is_empty() || self.config.has_more {
            0.0
        } else {
            style::HEIGHT_MD
        };
        let natural = self.content_height + MASONRY_PADDING * 2.0 + end_room;
        let height = if bc.max().height.is_finite() {
            bc.max().height.min(natural).max(bc.min().height)
        } else {
            natural
        };
        self.viewport = (height - end_room).max(0.0);
        self.offset = self.offset.clamp(0.0, self.max_offset());
        self.window = masonry_window(
            &self.slots,
            self.content_top(),
            self.viewport,
            self.config.overscan,
        );

        // Only the window is shaped — the point of the mechanism.
        for index in self.window.clone() {
            let card = &mut self.cards[index];
            card.title.layout(ctx, &title);
            card.caption.layout(ctx, &caption);
        }
        if self.config.error.is_some() {
            self.error.layout(ctx, &caption);
            self.retry.layout(ctx, &caption);
        }
        if !self.config.end_label.is_empty() {
            self.end.layout(ctx, &caption);
        }

        bc.constrain(Size::new(outer, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let (background, muted) = match theme {
            Some(t) => (t.scheme().surface, t.scheme().surface_container_highest),
            None => (crate::BEUI_LIGHT.background, crate::BEUI_LIGHT.muted),
        };
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        if std::mem::take(&mut self.relayout_owed) {
            ctx.request_layout();
        }

        let radius = style::resolve_radius(MASONRY_SURFACE_RADIUS, size.width, size.height);
        scene.fill_rounded_rect(origin, size, radius, background);
        paint_panel_hairline(scene, origin, size, radius, chrome.border);

        if self.is_empty() {
            let text = self.empty.size();
            self.empty.paint(
                origin
                    + Vec2::new(
                        (size.width - text.width) / 2.0,
                        (size.height - text.height) / 2.0,
                    ),
                chrome.dim_ink,
                scene,
            );
            return;
        }

        scene.push_clip_rounded(origin, Size::new(size.width, self.viewport), radius);
        let shift = Vec2::new(MASONRY_PADDING, MASONRY_PADDING - self.offset);

        let started = *self.reveal_start.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if self.reveal_running(elapsed, reduce) {
            ctx.request_frame();
        }

        for index in self.window.clone() {
            let slot = self.slots[index];
            let (alpha, rise) = self.reveal(index, elapsed, reduce);
            if alpha <= 0.0 {
                continue;
            }
            let at = origin
                + shift
                + Vec2::new(
                    slot.lane as f64 * (self.column_width + self.config.gap),
                    slot.y + rise,
                );
            let box_size = Size::new(self.column_width, slot.height);
            let layered = alpha < 1.0;
            if layered {
                scene.push_layer(at, box_size, alpha as f32);
            }
            scene.fill_rounded_rect(at, box_size, MASONRY_CARD_RADIUS, chrome.surface);
            paint_panel_hairline(scene, at, box_size, MASONRY_CARD_RADIUS, chrome.border);
            let card = &self.cards[index];
            card.title.paint(
                at + Vec2::new(MASONRY_CARD_PADDING, MASONRY_CARD_PADDING),
                chrome.ink,
                scene,
            );
            if !card.caption.content().is_empty() {
                card.caption.paint(
                    at + Vec2::new(
                        MASONRY_CARD_PADDING,
                        MASONRY_CARD_PADDING + card.title.size().height + style::GAP_SM,
                    ),
                    chrome.dim_ink,
                    scene,
                );
            }
            if layered {
                scene.pop_layer();
            }
        }

        self.retry_box = Rect::ZERO;
        for slot in self.tail.clone() {
            let at = origin
                + shift
                + Vec2::new(
                    slot.lane as f64 * (self.column_width + self.config.gap),
                    slot.y,
                );
            let box_size = Size::new(self.column_width, slot.height);
            if self.config.error.is_some() {
                self.paint_error_tail(scene, origin, at, box_size, chrome);
            } else {
                paint_skeleton(scene, at, box_size, muted, chrome.dim_ink);
            }
        }
        scene.pop_clip();

        if !self.config.has_more && !self.cards.is_empty() && !self.config.end_label.is_empty() {
            let text = self.end.size();
            self.end.paint(
                origin
                    + Vec2::new(
                        (size.width - text.width) / 2.0,
                        self.viewport + (style::HEIGHT_MD - text.height) / 2.0,
                    ),
                chrome.dim_ink,
                scene,
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Scroll { delta, .. } = event {
            let dy = match delta {
                ScrollDelta::Lines(_, y) => y * WHEEL_LINE_PX,
                ScrollDelta::Pixels(_, y) => *y,
            };
            self.user_scroll(ctx, dy);
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        self.handle_pointer(ctx, p)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.config.label.clone();
        ctx.push_container(
            Role::List,
            |node| {
                node.set_label(label.as_str());
            },
            |ctx| {
                if self.is_empty() {
                    let empty = self.empty.content().to_string();
                    ctx.push_node(Role::Label, |node| {
                        node.set_label(empty.as_str());
                    });
                    return;
                }
                // Every declared card is announced, not only the windowed ones:
                // the window is a paint budget, and a screen reader's cursor is
                // not the viewport.
                for card in &self.cards {
                    let title = card.title.content().to_string();
                    ctx.push_node(Role::ListItem, |node| {
                        node.set_label(title.as_str());
                    });
                }
                if self.config.error.is_some() {
                    let error = self.error.content().to_string();
                    ctx.push_node(Role::Label, |node| {
                        node.set_label(error.as_str());
                    });
                    if self.config.retryable {
                        let retry = self.retry.content().to_string();
                        ctx.push_node(Role::Button, |node| {
                            node.set_label(retry.as_str());
                            node.add_action(Action::Click);
                        });
                    }
                }
            },
        );
    }
}

/// Paint one loading skeleton: a muted card under three bars.
fn paint_skeleton(scene: &mut dyn PaintScene, at: Point, size: Size, fill: Color, bar: Color) {
    scene.fill_rounded_rect(at, size, MASONRY_CARD_RADIUS, fill);
    let width = (size.width - MASONRY_CARD_PADDING * 2.0).max(0.0);
    for (row, fraction, height) in [(0usize, 0.66, 12.0), (1, 1.0, 8.0), (2, 0.8, 8.0)] {
        scene.fill_rounded_rect(
            Point::new(
                at.x + MASONRY_CARD_PADDING,
                at.y + MASONRY_CARD_PADDING + row as f64 * MASONRY_SKELETON_PITCH,
            ),
            Size::new(width * fraction, height),
            style::RADIUS_CONTROL,
            style::with_alpha(bar, MASONRY_SKELETON_BAR_ALPHA),
        );
    }
}

impl MasonryWidget {
    /// Paint the error tail — its wash, its message and (when the app supplied
    /// one) its retry button, whose box this also records for the event pass.
    fn paint_error_tail(
        &mut self,
        scene: &mut dyn PaintScene,
        origin: Point,
        at: Point,
        size: Size,
        chrome: PanelChrome,
    ) {
        scene.fill_rounded_rect(
            at,
            size,
            MASONRY_CARD_RADIUS,
            style::with_alpha(chrome.danger_ink, MASONRY_ERROR_WASH_ALPHA),
        );
        self.error.paint(
            at + Vec2::new(MASONRY_CARD_PADDING, MASONRY_CARD_PADDING),
            chrome.danger_ink,
            scene,
        );
        if !self.config.retryable {
            return;
        }
        let label = self.retry.size();
        let button = Rect::from_origin_size(
            Point::new(
                at.x - origin.x + MASONRY_CARD_PADDING,
                at.y - origin.y + size.height - MASONRY_CARD_PADDING - style::HEIGHT_SM,
            ),
            Size::new(label.width + style::PADDING_X_MD * 2.0, style::HEIGHT_SM),
        );
        self.retry_box = button;
        let tint = if self.retry_pressed {
            style::scale_alpha(chrome.surface, style::HOVER_SOLID_ALPHA)
        } else {
            chrome.surface
        };
        let box_at = origin + button.origin().to_vec2();
        scene.fill_rounded_rect(box_at, button.size(), style::RADIUS_CONTROL, tint);
        paint_panel_hairline(
            scene,
            box_at,
            button.size(),
            style::RADIUS_CONTROL,
            chrome.border,
        );
        self.retry.paint(
            box_at + Vec2::new(style::PADDING_X_MD, (button.height() - label.height) / 2.0),
            chrome.ink,
            scene,
        );
    }

    /// The `Widget::event` pointer arm: the retry button owns its press, and
    /// everything else is a drag on the scroll surface.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        let size = ctx.size();
        match p.phase {
            PointerPhase::Move => {
                if self.retry_box.contains(p.position) && self.down.is_none() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                let Some(last) = self.down else {
                    return EventResult::Ignored;
                };
                if !self.dragging {
                    if (p.position.y - last.y).abs() <= TOUCH_SLOP {
                        return EventResult::Ignored;
                    }
                    // Take the gesture over: whatever the press armed is off.
                    self.dragging = true;
                    if self.retry_pressed {
                        self.retry_pressed = false;
                        ctx.request_redraw();
                    }
                    self.down = Some(p.position);
                    return EventResult::Handled;
                }
                // The content follows the finger: dragging up scrolls down.
                self.down = Some(p.position);
                self.user_scroll(ctx, -(p.position.y - last.y));
                EventResult::Handled
            }
            PointerPhase::Down => {
                if !presses(p) || !inside(p.position, size) {
                    return EventResult::Ignored;
                }
                ctx.capture_pointer();
                self.down = Some(p.position);
                self.dragging = false;
                if self.retry_box.contains(p.position) {
                    self.retry_pressed = true;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                let was_pressed = self.retry_pressed;
                let dragged = self.dragging;
                self.retry_pressed = false;
                self.dragging = false;
                let held = self.down.take().is_some();
                if was_pressed {
                    ctx.request_redraw();
                    if !dragged && self.retry_box.contains(p.position) {
                        (self.on_retry)(ctx);
                    }
                }
                if held {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            // A `Cancel` arm never reaches app state.
            PointerPhase::Cancel => {
                let held = self.retry_pressed || self.down.is_some();
                self.retry_pressed = false;
                self.dragging = false;
                self.down = None;
                if held {
                    ctx.request_redraw();
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, ft_ms, light, pointer, reduced};
    use frust::authoring::any;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    const WINDOW: Size = Size::new(900.0, 400.0);

    // ---- The column arithmetic ---------------------------------------------

    #[test]
    fn the_column_count_follows_the_upstream_floor_and_its_two_clamps() {
        // 208 + 12 = 220 per column: 208 fits one, 428 two, 648 three.
        assert_eq!(masonry_columns(208.0, 12.0, 208.0, 4), 1);
        assert_eq!(masonry_columns(427.0, 12.0, 208.0, 4), 1);
        assert_eq!(masonry_columns(428.0, 12.0, 208.0, 4), 2);
        assert_eq!(masonry_columns(648.0, 12.0, 208.0, 4), 3);
        assert_eq!(masonry_columns(868.0, 12.0, 208.0, 4), 4);
        // Capped rather than growing without bound...
        assert_eq!(masonry_columns(4_000.0, 12.0, 208.0, 4), 4);
        // ...and floored at one however narrow the box.
        assert_eq!(masonry_columns(10.0, 12.0, 208.0, 4), 1);
        assert_eq!(masonry_columns(0.0, 12.0, 208.0, 4), 1);
        // Degenerate inputs resolve rather than divide by zero.
        assert_eq!(masonry_columns(f64::INFINITY, 12.0, 208.0, 4), 1);
        assert_eq!(masonry_columns(500.0, 0.0, 0.0, 4), 1);
    }

    #[test]
    fn a_column_width_divides_the_box_after_the_gaps_are_taken_out() {
        // 900 wide, three columns, 12px gaps: 900 - 24 = 876, / 3 = 292.
        assert_eq!(masonry_column_width(900.0, 12.0, 3), 292.0);
        assert_eq!(masonry_column_width(900.0, 12.0, 1), 900.0);
        assert_eq!(masonry_column_width(0.0, 12.0, 3), 0.0, "never negative");
        assert_eq!(masonry_column_width(900.0, 12.0, 0), 0.0);
    }

    // ---- The pack -----------------------------------------------------------

    /// The acceptance property: variable-height items land in balanced columns.
    #[test]
    fn the_pack_seeds_every_lane_then_always_feeds_the_shortest_one() {
        let extents = [100.0, 200.0, 50.0, 60.0, 30.0];
        let slots = masonry_pack(&extents, 3, 10.0);
        assert_eq!(
            slots[0],
            MasonrySlot {
                lane: 0,
                y: 0.0,
                height: 100.0
            }
        );
        assert_eq!(
            slots[1],
            MasonrySlot {
                lane: 1,
                y: 0.0,
                height: 200.0
            }
        );
        assert_eq!(
            slots[2],
            MasonrySlot {
                lane: 2,
                y: 0.0,
                height: 50.0
            }
        );
        // Lane 2 ends at 50 — the shortest — so item 3 starts at 50 + gap.
        assert_eq!(
            slots[3],
            MasonrySlot {
                lane: 2,
                y: 60.0,
                height: 60.0
            }
        );
        // Lanes now end at 100 / 200 / 120, so lane 0 takes item 4.
        assert_eq!(
            slots[4],
            MasonrySlot {
                lane: 0,
                y: 110.0,
                height: 30.0
            }
        );
        assert_eq!(masonry_content_height(&slots), 200.0);
    }

    #[test]
    fn a_balanced_pack_keeps_every_lane_within_one_card_of_the_others() {
        // Fifty identical cards over four lanes: no lane can run a whole card
        // past another, which is what "balanced" means for this rule.
        let extents = vec![100.0; 50];
        let slots = masonry_pack(&extents, 4, 8.0);
        let mut ends = [0.0f64; 4];
        for slot in &slots {
            ends[slot.lane] = ends[slot.lane].max(slot.bottom());
        }
        let tallest = ends.iter().copied().fold(0.0f64, f64::max);
        let shortest = ends.iter().copied().fold(f64::INFINITY, f64::min);
        assert!(tallest - shortest <= 108.0, "lanes drifted apart: {ends:?}");
    }

    #[test]
    fn a_pack_with_no_columns_or_no_items_is_empty_rather_than_a_panic() {
        assert!(masonry_pack(&[100.0], 0, 12.0).is_empty());
        assert!(masonry_pack(&[], 3, 12.0).is_empty());
        assert_eq!(masonry_content_height(&[]), 0.0);
    }

    #[test]
    fn a_non_positive_extent_falls_back_rather_than_collapsing_a_lane() {
        let slots = masonry_pack(&[0.0, -5.0, f64::NAN], 3, 0.0);
        for slot in &slots {
            assert_eq!(slot.height, MASONRY_DEFAULT_EXTENT);
        }
        // The same fallback the builder applies at declaration time.
        assert_eq!(
            masonry_item("x").extent(0.0).item_extent(),
            MASONRY_DEFAULT_EXTENT
        );
        assert_eq!(masonry_item("x").extent(80.0).item_extent(), 80.0);
    }

    // ---- The window ---------------------------------------------------------

    #[test]
    fn the_window_covers_the_viewport_plus_its_overscan_and_nothing_else() {
        // One lane, ten 100px cards, no gap: card i spans [100i, 100i + 100].
        let slots = masonry_pack(&[100.0; 10], 1, 0.0);
        // Viewport [300, 500], no overscan. The test is closed at both edges,
        // so card 2 (whose bottom edge touches 300) and card 5 (whose top edge
        // touches 500) both count — a card flush against a seam is on screen.
        assert_eq!(masonry_window(&slots, 300.0, 200.0, 0.0), 2..6);
        // 100px of overscan reaches one card further at each end.
        assert_eq!(masonry_window(&slots, 300.0, 200.0, 100.0), 1..7);
        // At the very top nothing before card 0 exists to reach.
        assert_eq!(masonry_window(&slots, 0.0, 200.0, 0.0), 0..3);
        // An empty table, and a viewport past the content, window to nothing.
        assert_eq!(masonry_window(&[], 0.0, 200.0, 0.0), 0..0);
        assert_eq!(masonry_window(&slots, 5_000.0, 200.0, 0.0), 0..0);
    }

    #[test]
    fn the_window_stays_contiguous_when_lanes_run_out_of_step() {
        // Lane 0 takes one very tall card; lane 1 takes many short ones, so
        // slot order and vertical order disagree.
        let mut extents = vec![600.0];
        extents.extend(std::iter::repeat_n(50.0, 8));
        let slots = masonry_pack(&extents, 2, 0.0);
        let window = masonry_window(&slots, 0.0, 100.0, 0.0);
        assert_eq!(window.start, 0, "the tall card is in view");
        assert!(window.end >= 3, "so are the first short ones: {window:?}");
        assert!(window.end <= slots.len());
    }

    // ---- The load gate ------------------------------------------------------

    /// The other acceptance property: one approach, one call.
    #[test]
    fn the_gate_fires_once_per_approach_and_stays_shut_until_released() {
        let mut gate = MasonryLoadGate::new();
        // Ten items, prefetch three: the band opens at index 7.
        assert!(!gate.approach(Some(6), 10, 3, true, false, false));
        assert!(gate.approach(Some(7), 10, 3, true, false, false));
        assert!(gate.is_pending());
        // Every later frame inside the band is silent.
        assert!(!gate.approach(Some(8), 10, 3, true, false, false));
        assert!(!gate.approach(Some(9), 10, 3, true, false, false));
        // The app answers; the next approach is heard again.
        gate.release();
        assert!(!gate.is_pending());
        assert!(gate.approach(Some(9), 10, 3, true, false, false));
    }

    #[test]
    fn the_gate_respects_every_upstream_guard() {
        let mut gate = MasonryLoadGate::new();
        assert!(
            !gate.approach(Some(9), 10, 3, false, false, false),
            "no more data"
        );
        assert!(
            !gate.approach(Some(9), 10, 3, true, true, false),
            "a load is already running"
        );
        assert!(
            !gate.approach(Some(9), 10, 3, true, false, true),
            "an error is showing"
        );
        assert!(
            !gate.approach(None, 10, 3, true, false, false),
            "nothing visible"
        );
        assert!(!gate.is_pending(), "a refused approach latches nothing");
        // An empty feed's band opens at zero, so any visible index fires.
        assert!(gate.approach(Some(0), 0, 3, true, false, false));
    }

    // ---- The reveal ---------------------------------------------------------

    /// A masonry built with items already in it does not play them in — the
    /// `initialItemCountRef` rule.
    #[test]
    fn a_freshly_built_feed_rests_at_full_presence() {
        let widget = bare(4);
        assert_eq!(widget.reveal_from, 4);
        assert_eq!(widget.reveal(0, Duration::ZERO, false), (1.0, 0.0));
        assert!(!widget.reveal_running(Duration::ZERO, false));
    }

    /// A card inside the reveal set starts below its slot and climbs in.
    #[test]
    fn an_appended_card_rises_into_its_slot_and_settles() {
        let mut widget = bare(4);
        widget.reveal_from = 2;
        assert_eq!(
            widget.reveal(2, Duration::ZERO, false),
            (0.0, MASONRY_REVEAL_TRAVEL)
        );
        // Its lane's slot opens at `lane * 40ms`, so 160ms is mid-flight for
        // any lane the cap allows.
        let (mid_alpha, mid_rise) = widget.reveal(2, Duration::from_millis(160), false);
        assert!(mid_alpha > 0.0 && mid_alpha <= 1.0, "{mid_alpha}");
        assert!(mid_rise < MASONRY_REVEAL_TRAVEL, "{mid_rise}");
        assert!(widget.reveal_running(Duration::from_millis(160), false));
        assert_eq!(widget.reveal(2, Duration::from_secs(5), false), (1.0, 0.0));
        assert!(!widget.reveal_running(Duration::from_secs(5), false));
    }

    /// The per-lane offset caps at the fourth lane rather than trailing on.
    #[test]
    fn the_lane_delay_caps_at_the_fourth_lane() {
        assert_eq!(MASONRY_LANE_DELAY_CAP, 3);
        let mut widget = bare(0);
        widget.reveal_from = 0;
        widget.cards = cards(&[masonry_item("a"), masonry_item("b")]);
        widget.slots = vec![
            MasonrySlot {
                lane: 3,
                y: 0.0,
                height: 10.0,
            },
            MasonrySlot {
                lane: 7,
                y: 0.0,
                height: 10.0,
            },
        ];
        let at = Duration::from_millis(150);
        assert_eq!(
            widget.reveal(0, at, false),
            widget.reveal(1, at, false),
            "lane 7 shares lane 3's delay"
        );
    }

    #[test]
    fn reduced_motion_places_every_card_and_asks_for_no_frames() {
        let mut widget = bare(4);
        widget.reveal_from = 0;
        assert_eq!(widget.reveal(0, Duration::ZERO, true), (1.0, 0.0));
        assert!(!widget.reveal_running(Duration::ZERO, true));
        // ...and so does an explicitly un-animated feed.
        widget.config.animate_items = false;
        assert_eq!(widget.reveal(0, Duration::ZERO, false), (1.0, 0.0));
        assert!(!widget.reveal_running(Duration::ZERO, false));
    }

    /// A widget with `count` items, laid out once without a render root —
    /// enough for the pure reveal and pack reads above.
    fn bare(count: usize) -> MasonryWidget {
        let items: Vec<MasonryItem> = (0..count)
            .map(|index| masonry_item(format!("Card {index}")).extent(100.0 + index as f64 * 10.0))
            .collect();
        let view = infinite_masonry::<(), _>(items, |_| {});
        let mut counter = 0u64;
        let mut widget = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        widget.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(1_400.0, 400.0)),
        );
        widget
    }

    // ---- The mounted feed ---------------------------------------------------

    #[derive(Default)]
    struct App {
        loads: u32,
        retries: u32,
        items: usize,
        loading: bool,
        has_more: bool,
        error: Option<String>,
    }

    struct Harness {
        root: RenderRoot<App, frust::StackView<App>>,
        state: App,
        tcx: TextContext,
        clock: f64,
    }

    impl Harness {
        fn new(items: usize) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App {
                    items,
                    has_more: true,
                    ..App::default()
                },
                tcx: TextContext::new(),
                clock: 0.0,
            };
            h.root.set_theme(Box::new(light()));
            h.step(0.0);
            h
        }

        fn step(&mut self, ms: f64) -> Recorder {
            self.clock += ms;
            let mut logic = move |s: &mut App| {
                let items: Vec<MasonryItem> = (0..s.items)
                    .map(|index| {
                        masonry_item(format!("Card {index}"))
                            .caption("caption")
                            .extent(120.0 + (index % 4) as f64 * 40.0)
                    })
                    .collect();
                let mut feed = infinite_masonry(items, |s: &mut App| s.loads += 1)
                    .has_more(s.has_more)
                    .loading(s.loading)
                    .on_retry(|s: &mut App| s.retries += 1)
                    .end_label("That is everything");
                if let Some(error) = s.error.clone() {
                    feed = feed.error(error);
                }
                frust::Stack(vec![any(feed)])
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let mut rec = Recorder::default();
            let now = self.clock;
            self.root.paint(&mut rec, ft_ms(now));
            rec
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        fn wheel(&mut self, dy: f64) {
            self.event(InputEvent::Scroll {
                position: Point::new(10.0, 10.0),
                delta: ScrollDelta::Pixels(0.0, dy),
            });
        }
    }

    #[test]
    fn a_mounted_feed_paints_its_windowed_cards_and_nothing_below_the_fold() {
        let mut h = Harness::new(60);
        let rec = h.step(0.0);
        // Sixty cards over three-plus columns is far taller than a 400px
        // viewport, so the painted set must be a strict subset.
        assert!(
            rec.rrects.len() < 60,
            "the whole feed was painted: {} rects",
            rec.rrects.len()
        );
        assert!(rec.rrects.len() > 2, "nothing was painted");
        assert!(!rec.clips.is_empty(), "the viewport is clipped");
    }

    #[test]
    fn scrolling_to_the_tail_reports_one_approach_and_then_stays_quiet() {
        let mut h = Harness::new(40);
        assert_eq!(h.state.loads, 0, "no approach before any gesture");
        // A nudge well short of the prefetch band says nothing.
        h.wheel(60.0);
        h.step(0.0);
        assert_eq!(h.state.loads, 0);
        // Riding to the end fires exactly one approach...
        h.wheel(100_000.0);
        h.step(0.0);
        assert_eq!(h.state.loads, 1, "the approach fired once");
        h.wheel(100.0);
        h.wheel(100.0);
        h.step(0.0);
        assert_eq!(h.state.loads, 1, "and does not repeat while outstanding");

        // ...and the app answering with more items releases the latch, so the
        // next approach is heard again.
        h.state.items = 80;
        h.step(0.0);
        h.wheel(100_000.0);
        assert_eq!(h.state.loads, 2);
    }

    #[test]
    fn a_feed_with_no_more_data_never_reports_an_approach() {
        let mut h = Harness::new(40);
        h.state.has_more = false;
        h.step(0.0);
        h.wheel(100_000.0);
        assert_eq!(h.state.loads, 0);
    }

    #[test]
    fn the_offset_clamps_at_both_ends_of_the_content() {
        let mut h = Harness::new(40);
        // Scrolling back from the top goes nowhere.
        let top = h.step(0.0).rrects.len();
        h.wheel(-4_000.0);
        assert_eq!(h.step(0.0).rrects.len(), top);
        // ...and the bottom clamp holds just as firmly.
        h.wheel(100_000.0);
        h.step(0.0);
        let bottom = h.step(0.0).rrects.len();
        assert_ne!(bottom, top, "the feed did scroll");
        h.wheel(100_000.0);
        assert_eq!(h.step(0.0).rrects.len(), bottom);
    }

    #[test]
    fn an_empty_finished_feed_shows_its_empty_state_instead_of_a_grid() {
        let mut h = Harness::new(0);
        h.state.has_more = false;
        let rec = h.step(0.0);
        // Exactly the surface — no cards, no clip.
        assert_eq!(rec.rrects.len(), 1);
        assert!(rec.clips.is_empty());
        assert!(!rec.inks.is_empty(), "the empty line is drawn");
    }

    #[test]
    fn a_loading_feed_shows_skeletons_and_an_error_replaces_them_with_one_card() {
        let mut h = Harness::new(4);
        h.state.loading = true;
        let loading = h.step(0.0).rrects.len();
        h.state.loading = false;
        h.state.error = Some("Network unreachable".to_string());
        let errored = h.step(0.0).rrects.len();
        assert!(
            errored < loading,
            "one error card replaces the per-column skeletons: {errored} vs {loading}"
        );
    }

    #[test]
    fn the_retry_button_fires_from_its_own_press_and_not_from_a_drag() {
        let mut h = Harness::new(4);
        h.state.error = Some("Network unreachable".to_string());
        h.step(0.0);
        let at = retry_centre(&mut h);
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        assert_eq!(h.state.retries, 1);
        // A press that drags away is a scroll, not a click.
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Move, at.x, at.y - 200.0));
        h.event(pointer(PointerPhase::Up, at.x, at.y - 200.0));
        assert_eq!(h.state.retries, 1);
    }

    /// The centre of the error card's retry button, in window space.
    fn retry_centre(h: &mut Harness) -> Point {
        let rec = h.step(0.0);
        let button = rec
            .rrects
            .iter()
            .rev()
            .find(|(_, size, _, _)| size.height == style::HEIGHT_SM)
            .copied()
            .expect("the retry button");
        Rect::from_origin_size(button.0, button.1).center()
    }

    #[test]
    fn appended_cards_fade_in_and_a_reduced_motion_feed_places_them_at_once() {
        let mut h = Harness::new(4);
        h.step(0.0);
        h.state.items = 12;
        // The first frame after the append latches the run's clock; the alpha
        // it computes is zero, so the fade is observable from the next one.
        h.step(0.0);
        let rec = h.step(60.0);
        assert!(
            rec.layers.iter().any(|alpha| *alpha < 1.0),
            "no appended card was mid-reveal: {:?}",
            rec.layers
        );

        let mut quiet = Harness::new(4);
        quiet.root.set_theme(Box::new(reduced()));
        quiet.step(0.0);
        quiet.state.items = 12;
        quiet.step(0.0);
        let rec = quiet.step(60.0);
        assert!(
            rec.layers.iter().all(|alpha| *alpha >= 1.0),
            "a reduced-motion reveal faded: {:?}",
            rec.layers
        );
    }
}
