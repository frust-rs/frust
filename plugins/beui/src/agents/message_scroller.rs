//! Ports beUI's `message-scroller` agent-interface part.
//!
//! **Source:** `components/agents/message-scroller.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `message-scroller`: *"A reader-aware conversation viewport that follows
//! streamed output at the live edge and releases control when the reader moves
//! away."*
//!
//! # The behaviour contract
//!
//! Upstream's `followOutput`/`followThreshold`/`onFollowChange` trio, and the
//! same contract `frust_shadcn`'s own message scroller states — the two ports
//! deliberately agree, because the mechanics are the framework's, not the
//! catalog's:
//!
//! - **Pinned** (the initial state): every layout re-pins the offset onto the
//!   end, so content growth is *pinned*, never chased — a message that arrives
//!   while the reader is at the live edge is fully visible on the frame it
//!   lands, with no catch-up scroll leaving it below the fold.
//! - **Unpinned**: a user scroll that leaves the offset more than
//!   [`MESSAGE_SCROLLER_FOLLOW_THRESHOLD`] px from the end unpins, and from then
//!   on growth *preserves the viewport* — the offset is untouched, so the
//!   reader's place holds while the transcript gets taller below them.
//! - **Re-pin**: any user scroll landing back inside the band re-pins, as does a
//!   layout that clamps the offset onto the end (content shrinking away under an
//!   unpinned reader).
//! - Every transition is reported once through
//!   [`MessageScrollerView::on_pin_change`].
//!
//! [`MessageScrollerPin`] is that rule on its own — a pure state machine over
//! the [`ScrollInfo`] stream, with no widget, no scene and no clock, so the
//! whole contract is table-testable against synthetic sequences.
//!
//! # Why the widget owns its offset
//!
//! The card asks for this to be built on [`frust::scroll_view`] plus
//! [`ScrollFx`](crate::motion::ScrollFx), and only half of that is reachable —
//! for a reason worth recording rather than rediscovering, and the same one
//! `frust_shadcn`'s port records:
//!
//! - **There is no offset *write* seam.** Nothing in the baseline's public
//!   surface sets a scroll position from outside; the only way to move it is to
//!   synthesize an `InputEvent::Scroll` into it, which needs an `EventCtx` —
//!   available *only* during an event pass. Content growth arrives on a
//!   rebuild, and a rebuild dispatches no event, so a wrapped `scroll_view`
//!   would stay put until the reader next touched the screen: the one moment
//!   pin-to-bottom exists for is exactly the moment it could not act.
//! - **There is no offset *read* seam either.** `ScrollWidget::offset` sits on a
//!   type the facade does not export.
//!
//! So this widget is a scroll surface in its own right: it clips a viewport,
//! owns the offset, and consumes the wheel/drag itself — over the framework's
//! *shared* gesture vocabulary ([`frust::input`]'s `TOUCH_SLOP`,
//! `WHEEL_LINE_PX`, `VelocityTracker` and the fling-decay math), so its feel is
//! the baseline's rather than re-tuned here. What *is* taken from the card's
//! substrate is the observation fold: every offset change is published as a
//! [`ScrollInfo`] into a [`ScrollFx`](crate::motion::ScrollFx), which is what
//! the pin machine and the velocity estimate both read.
//!
//! **User vs. programmatic is structural, not heuristic.** Because the offset
//! has exactly one owner, a *user* scroll is precisely an offset change made in
//! the wheel or drag arm of [`Widget::event`]. Every other write (the layout
//! re-pin, the jump button's glide, a fling settling onto the end) is
//! programmatic by construction and cannot be mistaken for the reader leaving.
//!
//! # Degradations against upstream
//!
//! - **No preview rail.** Upstream's `navigation="rail"` mounts a
//!   [`preview_rail`](crate::components::preview_rail) built from
//!   `textContent` scraped out of each rendered message's DOM node. There is no
//!   equivalent of reading a child subtree's text back out of a widget tree, so
//!   the rail is not ported; a caller composes `preview_rail` beside the
//!   scroller with labels it already has.
//! - **No overscroll rubber-band and no pull-to-refresh.** A transcript's live
//!   edge is not a rubber-band surface, and riding the baseline surface is what
//!   would have brought them.
//! - **`smooth` is not a flag.** Upstream picks `scrollTo({behavior})` per
//!   growth; here the *pin* is instantaneous (that is what pinning means) and
//!   only the jump button glides, over [`MESSAGE_SCROLLER_GLIDE_MS`].
//! - **The entrance is the scroller's, not the row's.** Upstream leaves each
//!   `Message` to play its own `animateIn`. This port additionally staggers a
//!   newly-appended *batch* through [`Stagger`](crate::motion::Stagger), which
//!   is what makes a burst of tool results read as arriving rather than
//!   appearing; a caller who wants only the row's own entrance passes
//!   [`MessageScrollerView::stagger_entrance`]`(false)`.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{Action, RoundedRect};
use frust::authoring::{
    Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerEvent, PointerPhase, Rect, Role, ScrollDelta, SemanticsCtx, Shape, Size, View, Widget,
    build_child, erase_callback_arg, rebuild_children, route_event, teardown_child, visit_children,
};
use frust::input::{
    FLING_STOP, TOUCH_SLOP, VelocityTracker, WHEEL_LINE_PX, fling_decay, fling_displacement,
};
use frust::{FrameTime, ScrollInfo, Theme};

use crate::motion::{Presence, Ramp, ScrollFx, Stagger};
use crate::press::{Lane, inside, presses};
use crate::style::{self, scale_alpha};
use crate::text::{Label, ThemeTextType};
use crate::tokens::motion::{EASE_OUT, SPRING_PRESS};
use crate::tokens::{BEUI_LIGHT, BeuiPalette, sans_family};

/// How close to the live edge still counts as following the output, in logical
/// px — upstream's `followThreshold = 56`.
///
/// One band, used in both directions: a user scroll leaving more than this much
/// content below the fold unpins, and one landing back inside it re-pins. A
/// single threshold is what keeps the two rules from fighting — an unpin
/// distance smaller than the re-pin distance would re-pin on the same event
/// that unpinned.
pub const MESSAGE_SCROLLER_FOLLOW_THRESHOLD: f64 = 56.0;

/// The gap between rows, in logical px (`MessageGroup`'s `gap-3`).
pub const MESSAGE_SCROLLER_GAP: f64 = style::SPACING_UNIT * 3.0;

/// How long the jump button's glide to the live edge takes, in ms.
pub const MESSAGE_SCROLLER_GLIDE_MS: u64 = 300;

/// Per-row delay through a newly-appended batch's entrance, in ms.
pub const MESSAGE_SCROLLER_STAGGER_MS: u64 = 60;

/// How far a newly-arrived row rises into place, in logical px.
pub const MESSAGE_SCROLLER_ENTRANCE_RISE: f64 = 8.0;

/// The jump button's height, in logical px (`h-8`).
pub const MESSAGE_SCROLLER_BUTTON_HEIGHT: f64 = style::HEIGHT_SM;

/// Horizontal padding inside the jump button, in logical px (`px-3`).
pub const MESSAGE_SCROLLER_BUTTON_PADDING_X: f64 = style::SPACING_UNIT * 3.0;

/// The jump button's inset from the viewport's bottom edge, in logical px
/// (`bottom-4`).
pub const MESSAGE_SCROLLER_BUTTON_INSET: f64 = style::SPACING_UNIT * 4.0;

/// The gap between the jump button's label and its arrow, in logical px.
const BUTTON_ICON_GAP: f64 = style::GAP_SM;

/// The jump button's arrow edge, in logical px.
const BUTTON_ICON_SIZE: f64 = 14.0;

/// How far the jump button rises into place, in logical px.
const BUTTON_RISE: f64 = 8.0;

/// The jump button's reveal/hide ramp, in ms — upstream's `duration-200`.
const BUTTON_REVEAL_MS: u64 = 200;

/// Lucide's icon viewBox edge — `arrow-down` is authored on a 24-unit grid.
const LUCIDE_VIEWBOX: f64 = 24.0;

/// Lucide's stroke width, on that same grid.
const LUCIDE_STROKE: f64 = 2.0;

/// The jump button's own label — upstream's `sr-only` "scroll to end".
pub const MESSAGE_SCROLLER_JUMP_LABEL: &str = "Jump to latest";

/// The viewport's accessible name — upstream's `label = "Conversation"`.
pub const MESSAGE_SCROLLER_LABEL: &str = "Conversation";

/// Unthemed fallback palette — see [`super::message_bubble`]'s own note.
const FALLBACK: BeuiPalette = BEUI_LIGHT;

// ---- The pin state machine -------------------------------------------------

/// The pin/unpin rule on its own: a pure state machine over the [`ScrollInfo`]
/// stream, with no widget, no scene and no clock.
///
/// Fed from wherever offsets come from — this module's own scroll surface, or a
/// `ScrollView::on_scroll` callback in an app that wants the rule without the
/// surface. See the [module docs](self) for the contract it implements.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MessageScrollerPin {
    threshold: f64,
    pinned: bool,
    offset: f64,
    max_offset: f64,
}

impl MessageScrollerPin {
    /// A viewport pinned to the live edge, unpinning past `threshold` px from
    /// it. A negative threshold clamps to zero.
    pub fn new(threshold: f64) -> Self {
        MessageScrollerPin {
            threshold: threshold.max(0.0),
            pinned: true,
            offset: 0.0,
            max_offset: 0.0,
        }
    }

    /// Whether the viewport is following the live edge.
    pub const fn is_pinned(&self) -> bool {
        self.pinned
    }

    /// The band this machine unpins and re-pins across.
    pub const fn threshold(&self) -> f64 {
        self.threshold
    }

    /// How far the viewport sits from the live edge, as of the last
    /// observation.
    pub fn distance_from_end(&self) -> f64 {
        (self.max_offset - self.offset).max(0.0)
    }

    /// Fold one notification in and re-derive the pin state, returning the new
    /// value **only when it changed** — so a caller reports one transition per
    /// observable change rather than one per scroll event.
    ///
    /// Every observation is treated as a reader-driven one. The widget calls
    /// this from its wheel and drag arms unconditionally, and from layout only
    /// while unpinned (a pinned layout *sets* the offset, and asking a machine
    /// about a position it just wrote would be circular).
    pub fn observe(&mut self, info: ScrollInfo) -> Option<bool> {
        self.offset = info.offset;
        self.max_offset = info.max_offset.max(0.0);
        let next = self.distance_from_end() <= self.threshold;
        (next != self.pinned).then(|| {
            self.pinned = next;
            next
        })
    }

    /// Force the machine back onto the live edge — what the jump-to-latest
    /// affordance does. Returns the transition, if any.
    pub fn repin(&mut self) -> Option<bool> {
        (!self.pinned).then(|| {
            self.pinned = true;
            self.offset = self.max_offset;
            true
        })
    }

    /// The offset a pinned viewport sits at for a given maximum.
    pub const fn pinned_offset(max_offset: f64) -> f64 {
        max_offset
    }
}

impl Default for MessageScrollerPin {
    fn default() -> Self {
        Self::new(MESSAGE_SCROLLER_FOLLOW_THRESHOLD)
    }
}

// ---- The view --------------------------------------------------------------

/// A view-held, typed pin-state callback (erased on build).
type OnPinChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative beUI message scroller. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust::text;
/// use frust_beui::agents::message_scroller::message_scroller;
///
/// #[derive(Default)]
/// struct Chat {
///     following: bool,
/// }
///
/// let transcript = message_scroller(vec![text("hello"), text("hi")])
///     .on_pin_change(|state: &mut Chat, pinned| state.following = pinned);
/// ```
pub struct MessageScrollerView<State: 'static> {
    items: Vec<AnyView<State>>,
    threshold: f64,
    gap: f64,
    jump_label: String,
    stagger_entrance: bool,
    on_pin_change: Option<OnPinChange<State>>,
}

/// A pin-to-the-live-edge transcript viewport over `items`, oldest first.
pub fn message_scroller<State: 'static>(
    items: impl IntoIterator<Item = impl View<State>>,
) -> MessageScrollerView<State> {
    MessageScrollerView {
        items: items.into_iter().map(AnyView::new).collect(),
        threshold: MESSAGE_SCROLLER_FOLLOW_THRESHOLD,
        gap: MESSAGE_SCROLLER_GAP,
        jump_label: MESSAGE_SCROLLER_JUMP_LABEL.to_owned(),
        stagger_entrance: true,
        on_pin_change: None,
    }
}

impl<State: 'static> MessageScrollerView<State> {
    /// How close to the live edge still counts as following it
    /// (`followThreshold`).
    pub fn follow_threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold.max(0.0);
        self
    }

    /// The gap between rows, in logical px.
    pub fn gap(mut self, gap: f64) -> Self {
        self.gap = gap.max(0.0);
        self
    }

    /// Replace the jump-to-latest button's label.
    pub fn jump_label(mut self, label: impl Into<String>) -> Self {
        self.jump_label = label.into();
        self
    }

    /// Whether a newly-appended batch of rows staggers in (default `true`).
    pub fn stagger_entrance(mut self, stagger: bool) -> Self {
        self.stagger_entrance = stagger;
        self
    }

    /// Observe the pin state: `true` when the viewport (re-)pins to the live
    /// edge, `false` when the reader leaves it (`onFollowChange`).
    ///
    /// Fired once per transition, never per scroll event. A transition resolved
    /// during paint or layout is delivered on the next event pass — neither
    /// carries an `EventCtx` — which is the same one-event deferral
    /// `ScrollView::on_scroll` makes for its own paint-driven motion; a
    /// `Cancel` drops a pending delivery without firing it.
    pub fn on_pin_change<F: Fn(&mut State, bool) + 'static>(mut self, callback: F) -> Self {
        self.on_pin_change = Some(Rc::new(callback));
        self
    }
}

/// A programmatic glide of the offset toward the live edge.
struct Glide {
    lane: Lane,
    from: f64,
}

/// The retained widget for a [`MessageScrollerView`].
pub struct MessageScrollerWidget {
    items: Vec<ChildPod>,
    /// Each row's top in *content* space, resolved at layout.
    item_tops: Vec<f64>,
    gap: f64,
    /// The scroll offset in `[0, max_offset]` — this widget's, not a child's.
    offset: f64,
    viewport: Size,
    content_height: f64,
    /// The pin rule, and the observation fold feeding it.
    pin: MessageScrollerPin,
    fx: ScrollFx,

    // Drag/fling gesture state, mirroring the baseline scroll surface's shape.
    down_active: bool,
    dragging: bool,
    down_start: Point,
    last_drag: Point,
    tracker: VelocityTracker,
    fling: Option<f64>,
    /// The most recent paint clock, reused as the event pass's timestamp (an
    /// event pass carries no clock of its own).
    last_frame_time: FrameTime,
    /// The last animation frame time for the fling pump; `None` seeds it.
    last_anim: Option<FrameTime>,

    // The jump-to-latest affordance.
    jump: Label,
    /// The jump button's accessible name — [`Label`] caches a shape, not a
    /// readable string, so the text is kept beside it for semantics.
    jump_label: String,
    jump_width: f64,
    jump_presence: Presence,
    jump_staged: bool,
    jump_hovered: bool,
    jump_pressed: bool,
    jump_captured: bool,
    glide: Option<Glide>,

    // The newly-appended batch's entrance.
    stagger: Stagger,
    stagger_entrance: bool,
    /// Index of the first row of the batch currently entering.
    entrance_from: usize,
    /// The frame that batch's entrance started on; `None` until its first paint.
    entrance_started: Option<FrameTime>,

    /// A transition resolved during paint or layout, delivered on the next
    /// event pass.
    pending_notify: Option<bool>,
    /// The last state the app was actually told about, so a report is owed only
    /// for a change it can observe.
    last_notified: bool,
    on_pin_change: Option<ErasedArgCallback<bool>>,
}

impl MessageScrollerWidget {
    /// The maximum scroll offset (`content − viewport`, never negative).
    pub fn max_offset(&self) -> f64 {
        (self.content_height - self.viewport.height).max(0.0)
    }

    /// The current scroll offset.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// Whether the viewport is following the live edge.
    pub fn is_pinned(&self) -> bool {
        self.pin.is_pinned()
    }

    /// The pin machine itself, for a caller inspecting the rule's own state.
    pub fn pin(&self) -> MessageScrollerPin {
        self.pin
    }

    /// How far the viewport sits from the live edge right now.
    pub fn distance_from_end(&self) -> f64 {
        (self.max_offset() - self.offset).max(0.0)
    }

    /// The scroll-effect fold every offset change is published into — progress,
    /// delta and the velocity estimate.
    pub fn scroll_fx(&self) -> &ScrollFx {
        &self.fx
    }

    /// Whether the jump-to-latest affordance is on screen at all.
    pub fn jump_visible(&self) -> bool {
        self.jump_presence.is_visible()
    }

    /// The jump button's box in widget-local coordinates.
    pub fn jump_box(&self) -> Rect {
        let width = self.jump_width;
        Rect::from_origin_size(
            Point::new(
                ((self.viewport.width - width) / 2.0).max(0.0),
                (self.viewport.height
                    - MESSAGE_SCROLLER_BUTTON_HEIGHT
                    - MESSAGE_SCROLLER_BUTTON_INSET)
                    .max(0.0),
            ),
            Size::new(width, MESSAGE_SCROLLER_BUTTON_HEIGHT),
        )
    }

    /// The [`ScrollInfo`] this surface's current position publishes — what the
    /// pin machine and the [`ScrollFx`] both observe.
    fn info(&self) -> ScrollInfo {
        ScrollInfo {
            offset: self.offset,
            max_offset: self.max_offset(),
            overscroll: 0.0,
        }
    }

    /// Clamp and store an offset.
    fn set_offset(&mut self, value: f64) {
        self.offset = value.clamp(0.0, self.max_offset());
    }

    /// Re-place every row for the current offset. Called from layout and again
    /// from paint, so a fling or glide advanced at paint time moves the content
    /// without a relayout.
    fn sync_item_origins(&mut self) {
        let offset = self.offset;
        for (pod, top) in self.items.iter_mut().zip(self.item_tops.iter()) {
            pod.set_origin(Point::new(0.0, top - offset));
        }
    }

    /// Publish the current position into the fold and the pin machine,
    /// returning the transition if the rule changed its mind.
    fn observe(&mut self) -> Option<bool> {
        let info = self.info();
        self.fx.observe_at(info, self.event_time_ms());
        self.pin.observe(info)
    }

    /// Apply a user-driven offset delta and report the resulting transition.
    fn user_scroll(&mut self, ctx: &mut EventCtx, delta: f64) {
        self.set_offset(self.offset + delta);
        self.sync_item_origins();
        if let Some(pinned) = self.observe() {
            self.notify(ctx, pinned);
        }
        ctx.request_redraw();
    }

    /// Fire the pin callback (event pass only — it needs `&mut State`).
    ///
    /// Gated on what the app was last told rather than on the transition that
    /// reached here: two transitions resolved between event passes net out to
    /// nothing, and the app hears one report per *observable* change.
    fn notify(&mut self, ctx: &mut EventCtx, pinned: bool) {
        if self.last_notified == pinned {
            return;
        }
        self.last_notified = pinned;
        if let Some(cb) = self.on_pin_change.as_mut() {
            cb(ctx, pinned);
        }
    }

    /// Start the glide to the live edge, and re-pin as of the press — the
    /// affordance starts fading immediately and the glide chases whatever the
    /// edge becomes meanwhile.
    fn jump_to_end(&mut self) -> Option<bool> {
        let mut lane = Lane::at_rest(
            Ramp::eased(Duration::from_millis(MESSAGE_SCROLLER_GLIDE_MS), EASE_OUT),
            0.0,
        );
        lane.retarget(1.0);
        self.fling = None;
        self.glide = Some(Glide {
            lane,
            from: self.offset,
        });
        self.pin.repin()
    }

    /// Advance an in-flight fling by `dt_ms`, returning whether it is still
    /// running. Mirrors the baseline surface's own tick, over the shared
    /// `frust::input` decay math.
    fn fling_tick(&mut self, dt_ms: f64) -> bool {
        let Some(v) = self.fling else {
            return false;
        };
        self.set_offset(self.offset + fling_displacement(v, dt_ms));
        let next = fling_decay(v, dt_ms);
        let at_bound = self.offset <= 0.0 || self.offset >= self.max_offset();
        if next.abs() < FLING_STOP || at_bound {
            self.fling = None;
            false
        } else {
            self.fling = Some(next);
            true
        }
    }

    /// Advance the jump button's glide, returning whether it is still running.
    fn glide_tick(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        let Some(glide) = self.glide.as_mut() else {
            return false;
        };
        if reduce_motion {
            self.glide = None;
            let max = self.max_offset();
            self.set_offset(max);
            return false;
        }
        let animating = glide.lane.advance(now);
        let t = glide.lane.value().clamp(0.0, 1.0);
        let from = glide.from;
        let max = self.max_offset();
        self.set_offset(from + (max - from) * t);
        if !animating {
            self.glide = None;
            self.set_offset(max);
        }
        animating
    }

    /// The last painted frame time in ms — the event pass's timestamp source.
    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    /// Whether `position` (widget-local) lands on a jump button that is
    /// currently interactive. A pinned viewport's button is not.
    fn hits_jump(&self, position: Point) -> bool {
        if self.pin.is_pinned() {
            return false;
        }
        let button = self.jump_box();
        inside(position - button.origin().to_vec2(), button.size())
    }

    /// How many rows are in the batch currently entering.
    fn entering_count(&self) -> usize {
        self.items.len().saturating_sub(self.entrance_from)
    }

    /// The entrance progress of row `index` at `elapsed` — `1.0` for a row that
    /// arrived before the current batch.
    fn entrance_progress(&self, index: usize, elapsed: Duration) -> f64 {
        if !self.stagger_entrance || index < self.entrance_from {
            return 1.0;
        }
        self.stagger
            .progress_clamped(elapsed, index - self.entrance_from, self.entering_count())
    }
}

impl<State: 'static> View<State> for MessageScrollerView<State> {
    type Element = MessageScrollerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MessageScrollerWidget {
        MessageScrollerWidget {
            items: self.items.iter().map(|v| build_child(v, ctx)).collect(),
            item_tops: Vec::new(),
            gap: self.gap,
            offset: 0.0,
            viewport: Size::ZERO,
            content_height: 0.0,
            pin: MessageScrollerPin::new(self.threshold),
            fx: ScrollFx::new(),
            down_active: false,
            dragging: false,
            down_start: Point::ZERO,
            last_drag: Point::ZERO,
            tracker: VelocityTracker::new(),
            fling: None,
            last_frame_time: FrameTime::ZERO,
            last_anim: None,
            jump: Label::new(&self.jump_label),
            jump_label: self.jump_label.clone(),
            jump_width: 0.0,
            jump_presence: Presence::symmetric(Ramp::eased(
                Duration::from_millis(BUTTON_REVEAL_MS),
                EASE_OUT,
            )),
            jump_staged: false,
            jump_hovered: false,
            jump_pressed: false,
            jump_captured: false,
            glide: None,
            stagger: Stagger::sprung(
                Duration::from_millis(MESSAGE_SCROLLER_STAGGER_MS),
                SPRING_PRESS,
            ),
            stagger_entrance: self.stagger_entrance,
            // The first batch arrives settled: a transcript that mounts a page
            // of history plays nothing.
            entrance_from: self.items.len(),
            entrance_started: None,
            pending_notify: None,
            // The surface starts pinned, which is what an app assumes too.
            last_notified: true,
            on_pin_change: self.on_pin_change.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MessageScrollerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable — always reinstall the erased adapter.
        element.on_pin_change = self.on_pin_change.as_ref().map(erase_callback_arg);

        let before = element.items.len();
        let mut flags = rebuild_children(
            &prev.items,
            &self.items,
            &mut element.items,
            ctx,
            |v| v,
            |_| None,
        );
        if self.items.len() > before {
            // A newly-appended batch: its entrance is timed from its own first
            // painted frame.
            element.entrance_from = before;
            element.entrance_started = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if self.items.len() < before {
            // Rows went away: nothing is entering.
            element.entrance_from = self.items.len();
            element.entrance_started = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if element.gap != self.gap {
            element.gap = self.gap;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.pin.threshold() != self.threshold.max(0.0) {
            let pinned = element.pin.is_pinned();
            element.pin = MessageScrollerPin::new(self.threshold);
            if !pinned {
                element.pin.observe(element.info());
            }
            flags |= ChangeFlags::PAINT;
        }
        if element.stagger_entrance != self.stagger_entrance {
            element.stagger_entrance = self.stagger_entrance;
            flags |= ChangeFlags::PAINT;
        }
        if element.jump.set_content(&self.jump_label) {
            element.jump_label = self.jump_label.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut MessageScrollerWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.items.iter().zip(element.items.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for MessageScrollerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let ink = ScrollerPaint::resolve(Theme::from_layout_ctx(ctx)).button_ink;
        let label = self.jump.layout_themed(ctx, &jump_style(ink), JUMP_ROLE);
        self.jump_width = label.width
            + MESSAGE_SCROLLER_BUTTON_PADDING_X * 2.0
            + BUTTON_ICON_GAP
            + BUTTON_ICON_SIZE;

        // A column at the viewport's width, as tall as it needs to be.
        let item_bc = BoxConstraints::new(Size::new(width, 0.0), Size::new(width, f64::INFINITY));
        self.item_tops.clear();
        let mut y = 0.0_f64;
        for (index, pod) in self.items.iter_mut().enumerate() {
            if index > 0 {
                y += self.gap;
            }
            let size = pod.layout_child(ctx, &item_bc);
            self.item_tops.push(y);
            y += size.height;
        }
        self.content_height = y;
        let height = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            y
        };
        self.viewport = Size::new(width, height);

        let max = self.max_offset();
        if self.pin.is_pinned() {
            // Pinned: growth is *pinned*, never chased.
            self.offset = max;
            self.fx.observe(self.info());
        } else {
            self.offset = self.offset.clamp(0.0, max);
            // Content that shrank away under an unpinned reader can leave the
            // viewport sitting on the end: that *is* being at the live edge, so
            // it re-pins (reported on the next event — layout has no `EventCtx`).
            if let Some(pinned) = self.pin.observe(self.info()) {
                self.pending_notify = Some(pinned);
            }
            self.fx.observe(self.info());
        }
        self.sync_item_origins();
        bc.constrain(self.viewport)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The authoritative hover read: a pointer that left sends this widget
        // nothing.
        if !ctx.is_hovered() {
            self.jump_hovered = false;
        }
        let now = ctx.frame_time();
        self.last_frame_time = now;

        // Every theme read up front: `&Theme` borrows the context immutably and
        // the child paints below need it mutably.
        let (reduce_motion, paint) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                ScrollerPaint::resolve(theme),
            )
        };
        if !self.jump_staged {
            self.jump_staged = true;
            if reduce_motion {
                self.jump_presence = self.jump_presence.collapsed();
                self.stagger = self.stagger.collapsed();
            }
        }

        let mut owes_frame = false;
        // 1. The fling, integrated from the shared decay math.
        if self.fling.is_some() {
            let dt_ms = match self.last_anim {
                Some(last) => now.saturating_sub(last).as_secs_f64() * 1000.0,
                None => 0.0,
            };
            self.last_anim = Some(now);
            owes_frame |= self.fling_tick(dt_ms);
            if let Some(pinned) = self.observe() {
                self.pending_notify = Some(pinned);
            }
        }
        // 2. The jump button's glide (`reduce_motion` lands it immediately).
        owes_frame |= self.glide_tick(now, reduce_motion);
        self.sync_item_origins();

        // 3. The affordance itself follows the pin state.
        self.jump_presence.set_open(!self.pin.is_pinned());
        let reveal = self.jump_presence.advance(now);
        owes_frame |= self.jump_presence.is_animating();

        // 4. The newly-appended batch's entrance.
        let entering = self.entering_count();
        let elapsed = if entering > 0 && self.stagger_entrance {
            let started = *self.entrance_started.get_or_insert(now);
            let elapsed = now.saturating_sub(started);
            if !self.stagger.is_settled(elapsed, entering) {
                owes_frame = true;
            }
            elapsed
        } else {
            Duration::ZERO
        };

        let (origin, size) = (ctx.origin(), ctx.size());
        scene.push_clip(origin, size);
        for index in 0..self.items.len() {
            let progress = self.entrance_progress(index, elapsed);
            if progress >= 1.0 {
                self.items[index].paint_child(ctx, scene);
                continue;
            }
            if progress <= 0.0 {
                // Not yet started: nothing to paint, but the pod still holds
                // its place.
                continue;
            }
            let rise = MESSAGE_SCROLLER_ENTRANCE_RISE * (1.0 - progress);
            scene.push_transform(Affine::translate((0.0, rise)));
            scene.push_layer(origin, size, progress as f32);
            self.items[index].paint_child(ctx, scene);
            scene.pop_layer();
            scene.pop_transform();
        }
        scene.pop_clip();

        // The button rides *over* the viewport, so it is painted outside the
        // content clip.
        if reveal > 0.0 {
            self.paint_jump(scene, origin, reveal, paint);
        }
        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A transition resolved at paint or layout is delivered here — except
        // on a `Cancel`, which drops it without firing (the baseline surface's
        // own convention for its deferred scroll notification).
        let is_cancel = matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Cancel);
        if is_cancel {
            self.pending_notify = None;
        } else if let Some(pinned) = self.pending_notify.take() {
            self.notify(ctx, pinned);
        }

        // The wheel belongs to the surface, never to a row.
        if let InputEvent::Scroll { delta, .. } = event {
            let dy = match delta {
                ScrollDelta::Lines(_, y) => y * WHEEL_LINE_PX,
                ScrollDelta::Pixels(_, y) => *y,
            };
            self.fling = None;
            self.glide = None;
            self.user_scroll(ctx, dy);
            return EventResult::Handled;
        }

        let InputEvent::Pointer(p) = event else {
            // Broadcasts and focus-routed events reach the rows untouched.
            return route_event(&mut self.items, ctx, event);
        };
        let position = p.position;
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                if self.hits_jump(position) {
                    self.jump_pressed = true;
                    self.jump_captured = true;
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                self.dragging = false;
                self.down_active = true;
                // A glide cut short here leaves the viewport mid-content, so
                // the pin state — latched true the moment the button was
                // pressed, ahead of the glide finishing — needs re-deriving
                // from where the offset actually landed.
                let glide_cancelled = self.glide.is_some();
                self.fling = None;
                self.glide = None;
                self.last_anim = None;
                self.down_start = position;
                self.last_drag = position;
                self.tracker.clear();
                self.tracker.record(self.event_time_ms(), position.y);
                if glide_cancelled && let Some(pinned) = self.observe() {
                    self.notify(ctx, pinned);
                }
                ctx.capture_pointer();
                route_event(&mut self.items, ctx, event);
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.jump_captured {
                    let over = self.hits_jump(position);
                    if over {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.jump_pressed != over {
                        self.jump_pressed = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Handled;
                }
                if !self.down_active {
                    // A hover move: the rows get it first, so a claiming
                    // descendant is the one recorded, and the button's own
                    // claim is the fallback.
                    let routed = route_event(&mut self.items, ctx, event);
                    let over = self.hits_jump(position);
                    if over {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.jump_hovered != over {
                        self.jump_hovered = over;
                        ctx.request_redraw();
                    }
                    return routed;
                }
                self.tracker.record(self.event_time_ms(), position.y);
                if self.dragging {
                    let dy = position.y - self.last_drag.y;
                    self.last_drag = position;
                    // The content follows the finger: dragging up (negative dy)
                    // moves the offset down the transcript.
                    self.user_scroll(ctx, -dy);
                } else if (position.y - self.down_start.y).abs() > TOUCH_SLOP {
                    // Take the gesture over: cancel whatever row was armed,
                    // then stop forwarding to it.
                    self.dragging = true;
                    self.last_drag = position;
                    let cancel = InputEvent::Pointer(PointerEvent {
                        phase: PointerPhase::Cancel,
                        position,
                        button: p.button,
                    });
                    route_event(&mut self.items, ctx, &cancel);
                    ctx.request_redraw();
                } else {
                    route_event(&mut self.items, ctx, event);
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                // An `Up` ends the hover link outright — the latch is cleared
                // here and re-claimed on the next move.
                self.jump_hovered = false;
                if self.jump_captured {
                    let fired = self.jump_pressed && self.hits_jump(position);
                    self.jump_pressed = false;
                    self.jump_captured = false;
                    if fired && let Some(pinned) = self.jump_to_end() {
                        self.notify(ctx, pinned);
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if self.down_active {
                    self.down_active = false;
                    if self.dragging {
                        self.dragging = false;
                        self.tracker.record(self.event_time_ms(), position.y);
                        // The content moves opposite the finger, so the fling
                        // does too.
                        let velocity = -self.tracker.velocity();
                        if velocity.abs() >= FLING_STOP {
                            self.fling = Some(velocity);
                            self.last_anim = None;
                        }
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                }
                route_event(&mut self.items, ctx, event)
            }
            PointerPhase::Cancel => {
                self.jump_pressed = false;
                self.jump_captured = false;
                self.jump_hovered = false;
                self.down_active = false;
                self.dragging = false;
                route_event(&mut self.items, ctx, event)
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // `role="log" aria-live="polite"` upstream, over a labelled viewport.
        let label = self.jump_label.clone();
        let pinned = self.pin.is_pinned();
        ctx.push_container(
            Role::List,
            |node| node.set_label(MESSAGE_SCROLLER_LABEL),
            |ctx| {
                for pod in &self.items {
                    pod.semantics_child(ctx);
                }
                if !pinned {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(label.as_str());
                        node.add_action(Action::Click);
                    });
                }
            },
        );
    }

    visit_children!(items);
}

/// The colours one scroller resolves.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollerPaint {
    /// The jump button's fill.
    button: Color,
    /// The jump button's ink.
    button_ink: Color,
    /// The jump button's hairline.
    border: Color,
}

impl ScrollerPaint {
    /// Resolve against `theme`, falling back to beUI light.
    fn resolve(theme: Option<&Theme>) -> Self {
        match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                ScrollerPaint {
                    button: scheme.surface_container_high,
                    button_ink: scheme.on_surface,
                    border: scheme.outline_variant,
                }
            }
            None => ScrollerPaint {
                button: FALLBACK.popover,
                button_ink: FALLBACK.foreground,
                border: FALLBACK.border,
            },
        }
    }
}

/// The type-scale role the jump button's label takes its family from at
/// layout.
const JUMP_ROLE: ThemeTextType = ThemeTextType::LabelMedium;

/// The jump button's label style: `text-xs font-medium`. The family here is
/// the unthemed base; `layout` shapes in [`JUMP_ROLE`]'s family.
fn jump_style(ink: Color) -> frust::authoring::text::TextStyle {
    frust::authoring::text::TextStyle {
        family: sans_family(),
        weight: frust::authoring::text::FontWeight::MEDIUM,
        size: style::TEXT_XS as f32,
        color: ink,
        ..frust::authoring::text::TextStyle::default()
    }
}

/// Lucide's `arrow-down` on a 24-unit grid, scaled to `size`.
fn arrow_down_path(size: f64) -> BezPath {
    let unit = size / LUCIDE_VIEWBOX;
    let at = |x: f64, y: f64| Point::new(x * unit, y * unit);
    let mut path = BezPath::new();
    // `M12 5v14` — the shaft.
    path.move_to(at(12.0, 5.0));
    path.line_to(at(12.0, 19.0));
    // `m19 12-7 7-7-7` — the head.
    path.move_to(at(19.0, 12.0));
    path.line_to(at(12.0, 19.0));
    path.line_to(at(5.0, 12.0));
    path
}

impl MessageScrollerWidget {
    /// Paint the jump-to-latest pill at `reveal` presence.
    fn paint_jump(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        reveal: f64,
        paint: ScrollerPaint,
    ) {
        let alpha = reveal.clamp(0.0, 1.0) as f32;
        let rise = BUTTON_RISE * (1.0 - reveal);
        let button = self.jump_box();
        let at = Point::new(origin.x + button.x0, origin.y + button.y0 + rise);
        let size = button.size();
        let radius = size.height / 2.0;

        // `hover:brightness` and the pressed state land on the same wash
        // upstream — the pill's own press feedback is its glide, not a second
        // fill.
        let mut fill = paint.button;
        if self.jump_pressed || self.jump_hovered {
            fill = scale_alpha(fill, style::HOVER_SOLID_ALPHA);
        }
        scene.fill_rounded_rect(at, size, radius, scale_alpha(fill, alpha));
        let hairline = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, size).inset(-style::BORDER_WIDTH / 2.0),
            (radius - style::BORDER_WIDTH / 2.0).max(0.0),
        );
        scene.stroke_path(
            at,
            &Shape::to_path(&hairline, style::PATH_TOLERANCE),
            style::BORDER_WIDTH,
            &Brush::Solid(scale_alpha(paint.border, alpha)),
        );

        let ink = scale_alpha(paint.button_ink, alpha);
        let label = self.jump.size();
        self.jump.paint(
            Point::new(
                at.x + MESSAGE_SCROLLER_BUTTON_PADDING_X,
                at.y + (size.height - label.height) / 2.0,
            ),
            scene,
        );
        scene.stroke_path(
            Point::new(
                at.x + MESSAGE_SCROLLER_BUTTON_PADDING_X + label.width + BUTTON_ICON_GAP,
                at.y + (size.height - BUTTON_ICON_SIZE) / 2.0,
            ),
            &arrow_down_path(BUTTON_ICON_SIZE),
            LUCIDE_STROKE * BUTTON_ICON_SIZE / LUCIDE_VIEWBOX,
            &Brush::Solid(ink),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, ScrollDelta};
    use frust::{any, text};
    use std::any::Any;

    /// The viewport every scroller test lays itself into.
    const BOX: Size = Size::new(320.0, 200.0);

    /// One synthetic notification.
    fn info(offset: f64, max_offset: f64) -> ScrollInfo {
        ScrollInfo {
            offset,
            max_offset,
            overscroll: 0.0,
        }
    }

    // ---- The pin state machine, against synthetic sequences -----------------

    /// A fresh machine follows the live edge, which is what an app assumes too.
    #[test]
    fn a_fresh_pin_machine_follows_the_live_edge() {
        let pin = MessageScrollerPin::default();
        assert!(pin.is_pinned());
        assert_eq!(pin.threshold(), MESSAGE_SCROLLER_FOLLOW_THRESHOLD);
        assert_eq!(pin.distance_from_end(), 0.0);
        // A negative threshold clamps rather than inverting the rule.
        assert_eq!(MessageScrollerPin::new(-10.0).threshold(), 0.0);
    }

    /// The core rule: leaving the band unpins, coming back re-pins, and each
    /// transition is reported exactly once.
    #[test]
    fn the_pin_machine_unpins_past_the_band_and_repins_inside_it() {
        let mut pin = MessageScrollerPin::default();
        // At the end.
        assert_eq!(pin.observe(info(1000.0, 1000.0)), None, "no change");
        // Inside the band: still following.
        assert_eq!(pin.observe(info(960.0, 1000.0)), None);
        assert!(pin.is_pinned());
        assert_eq!(pin.distance_from_end(), 40.0);
        // Exactly on the band's edge is still following (`<=`).
        assert_eq!(
            pin.observe(info(1000.0 - MESSAGE_SCROLLER_FOLLOW_THRESHOLD, 1000.0)),
            None
        );
        assert!(pin.is_pinned());
        // One pixel past it, and the reader has left.
        assert_eq!(
            pin.observe(info(
                1000.0 - MESSAGE_SCROLLER_FOLLOW_THRESHOLD - 1.0,
                1000.0
            )),
            Some(false)
        );
        assert!(!pin.is_pinned());
        // Staying away reports nothing further.
        assert_eq!(pin.observe(info(100.0, 1000.0)), None);
        assert_eq!(pin.observe(info(0.0, 1000.0)), None);
        // Coming back inside the band re-pins, once.
        assert_eq!(pin.observe(info(970.0, 1000.0)), Some(true));
        assert_eq!(pin.observe(info(1000.0, 1000.0)), None);
        assert!(pin.is_pinned());
    }

    /// Growth under an unpinned reader keeps them unpinned; content shrinking
    /// away *onto* the end re-pins them, which is the one case a naive
    /// "distance only ever grows" rule gets wrong.
    #[test]
    fn growth_keeps_the_reader_away_and_shrinkage_onto_the_end_repins() {
        let mut pin = MessageScrollerPin::default();
        assert_eq!(pin.observe(info(500.0, 1000.0)), Some(false));

        // The transcript grows below them: still unpinned, no report.
        assert_eq!(pin.observe(info(500.0, 1400.0)), None);
        assert_eq!(pin.observe(info(500.0, 2000.0)), None);
        assert!(!pin.is_pinned());

        // Content shrinks away until the viewport *is* the end.
        assert_eq!(pin.observe(info(500.0, 520.0)), Some(true));
        assert!(pin.is_pinned());
    }

    /// A zero threshold is a legitimate "only exactly at the end counts".
    #[test]
    fn a_zero_threshold_pins_only_exactly_at_the_end() {
        let mut pin = MessageScrollerPin::new(0.0);
        assert_eq!(pin.observe(info(1000.0, 1000.0)), None);
        assert_eq!(pin.observe(info(999.9, 1000.0)), Some(false));
        assert_eq!(pin.observe(info(1000.0, 1000.0)), Some(true));
    }

    /// A short transcript (nothing to scroll) is always at the live edge.
    #[test]
    fn an_unscrollable_transcript_stays_pinned() {
        let mut pin = MessageScrollerPin::default();
        for _ in 0..5 {
            assert_eq!(pin.observe(info(0.0, 0.0)), None);
        }
        assert!(pin.is_pinned());
        // A negative maximum (a viewport taller than its content) is clamped.
        assert_eq!(pin.observe(info(0.0, -50.0)), None);
        assert_eq!(pin.distance_from_end(), 0.0);
    }

    /// `repin` is the jump affordance's own write: it reports once, and is
    /// inert on an already-pinned machine.
    #[test]
    fn repin_reports_once_and_is_inert_when_already_pinned() {
        let mut pin = MessageScrollerPin::default();
        assert_eq!(pin.repin(), None, "already following");
        pin.observe(info(0.0, 1000.0));
        assert!(!pin.is_pinned());
        assert_eq!(pin.repin(), Some(true));
        assert_eq!(pin.repin(), None);
        assert_eq!(pin.distance_from_end(), 0.0);
        assert_eq!(MessageScrollerPin::pinned_offset(1000.0), 1000.0);
    }

    // ---- The widget ---------------------------------------------------------

    #[derive(Default)]
    struct Chat {
        reports: Vec<bool>,
    }

    /// Records the rounded-rect fills and alpha layers a scroller paints.
    #[derive(Default)]
    struct Recorder {
        rounded: Vec<(Point, Size, Color)>,
        layers: Vec<f32>,
        clips: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, color: Color) {
            self.rounded.push((origin, size, color));
        }
        fn push_clip(&mut self, _origin: Point, _size: Size) {
            self.clips += 1;
        }
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, _transform: Affine) {}
        fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, _brush: &Brush) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {}
    }

    /// A transcript of `count` fixed-height rows.
    fn rows(count: usize) -> Vec<AnyView<Chat>> {
        (0..count)
            .map(|index| any(text(format!("message {index}"))))
            .collect()
    }

    fn view(count: usize) -> MessageScrollerView<Chat> {
        message_scroller(rows(count))
            .on_pin_change(|state: &mut Chat, pinned| state.reports.push(pinned))
    }

    fn laid_out(view: &MessageScrollerView<Chat>) -> MessageScrollerWidget {
        let mut next_id = 0u64;
        let mut widget = View::<Chat>::build(view, &mut BuildCtx::new(&mut next_id));
        relayout(&mut widget);
        widget
    }

    fn relayout(widget: &mut MessageScrollerWidget) {
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::tight(BOX));
    }

    /// Rebuild `widget` from `prev` to `next`, exactly as the tree would —
    /// the previous view has to be the real one, since positional
    /// reconciliation walks it against the retained pods.
    fn rebuild(
        widget: &mut MessageScrollerWidget,
        prev: &MessageScrollerView<Chat>,
        next: &MessageScrollerView<Chat>,
    ) {
        let mut next_id = 0u64;
        View::<Chat>::rebuild(next, prev, widget, &mut BuildCtx::new(&mut next_id));
        relayout(widget);
    }

    fn painted(
        widget: &mut MessageScrollerWidget,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame())
    }

    fn dispatch(
        widget: &mut MessageScrollerWidget,
        state: &mut Chat,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, BOX);
        widget.event(&mut ctx, event)
    }

    fn wheel(dy: f64) -> InputEvent {
        InputEvent::Scroll {
            position: Point::new(10.0, 10.0),
            delta: ScrollDelta::Pixels(0.0, dy),
        }
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// A pinned viewport sits on the live edge, and a growing transcript keeps
    /// it there: the offset lands on the new maximum on the frame the row
    /// arrives, never below it.
    #[test]
    fn a_pinned_viewport_stays_on_the_live_edge_as_the_transcript_grows() {
        let twenty = view(20);
        let mut widget = laid_out(&twenty);
        assert!(widget.is_pinned());
        assert_eq!(widget.offset(), widget.max_offset());
        let before = widget.max_offset();
        assert!(before > 0.0, "the transcript overflows the viewport");

        rebuild(&mut widget, &twenty, &view(24));
        assert!(widget.max_offset() > before, "it grew");
        assert_eq!(
            widget.offset(),
            widget.max_offset(),
            "growth is pinned, not chased"
        );
        assert_eq!(widget.distance_from_end(), 0.0);
    }

    /// A user scroll away unpins and reports once; further scrolling away
    /// reports nothing; growth then *preserves the viewport* rather than
    /// dragging the reader down.
    #[test]
    fn a_user_scroll_away_unpins_and_growth_then_preserves_the_viewport() {
        let twenty = view(20);
        let mut widget = laid_out(&twenty);
        let mut state = Chat::default();

        dispatch(&mut widget, &mut state, &wheel(-200.0));
        assert!(!widget.is_pinned());
        assert_eq!(state.reports, vec![false], "reported once");
        let held = widget.offset();

        dispatch(&mut widget, &mut state, &wheel(-40.0));
        assert_eq!(state.reports, vec![false], "still one report");

        let away = widget.offset();
        rebuild(&mut widget, &twenty, &view(26));
        assert_eq!(widget.offset(), away, "the reader's place holds");
        assert!(widget.distance_from_end() > MESSAGE_SCROLLER_FOLLOW_THRESHOLD);
        assert!(held > away, "the second scroll moved further away");
    }

    /// Scrolling back inside the band re-pins and reports once — and the next
    /// growth pins again.
    #[test]
    fn scrolling_back_into_the_band_repins() {
        let twenty = view(20);
        let mut widget = laid_out(&twenty);
        let mut state = Chat::default();
        dispatch(&mut widget, &mut state, &wheel(-300.0));
        assert!(!widget.is_pinned());

        dispatch(&mut widget, &mut state, &wheel(400.0));
        assert!(widget.is_pinned());
        assert_eq!(state.reports, vec![false, true]);

        rebuild(&mut widget, &twenty, &view(24));
        assert_eq!(widget.offset(), widget.max_offset());
    }

    /// The jump affordance shows only while unpinned, glides the viewport back
    /// to the live edge on a press, and re-pins as of that press.
    #[test]
    fn the_jump_affordance_returns_the_reader_to_the_live_edge() {
        let mut widget = laid_out(&view(20));
        let mut state = Chat::default();
        painted(&mut widget, 0, None);
        assert!(!widget.jump_visible(), "a pinned viewport hides it");

        dispatch(&mut widget, &mut state, &wheel(-400.0));
        painted(&mut widget, 10, None);
        assert!(widget.jump_visible());
        let button = widget.jump_box().center();
        assert!(widget.hits_jump(button));

        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Down, button.x, button.y),
        );
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Up, button.x, button.y),
        );
        assert!(widget.is_pinned(), "re-pinned as of the press");
        assert_eq!(state.reports, vec![false, true]);

        // The glide carries the offset home over its own ramp.
        let (_, needs_frame) = painted(&mut widget, 20, None);
        assert!(needs_frame, "the glide owes frames");
        painted(&mut widget, 5_000, None);
        assert_eq!(widget.offset(), widget.max_offset());
        painted(&mut widget, 5_400, None);
        assert!(!widget.jump_visible(), "and the affordance leaves");
    }

    /// A pinned viewport's button is inert: it takes no pointer even where it
    /// would have been.
    #[test]
    fn a_pinned_viewports_button_takes_no_pointer() {
        let mut widget = laid_out(&view(20));
        painted(&mut widget, 0, None);
        let where_it_would_be = widget.jump_box().center();
        assert!(!widget.hits_jump(where_it_would_be));
    }

    /// A newly-appended batch staggers in: each new row is at its own point of
    /// the reveal, and rows that were already there are untouched.
    #[test]
    fn a_newly_appended_batch_staggers_in() {
        let three = view(3);
        let mut widget = laid_out(&three);
        painted(&mut widget, 0, None);
        // The first batch arrives settled — nothing composites through a layer.
        let (rec, needs_frame) = painted(&mut widget, 10, None);
        assert!(rec.layers.is_empty(), "history plays no entrance");
        assert!(!needs_frame);

        rebuild(&mut widget, &three, &view(6));
        // The first paint after the batch lands latches its clock, so nothing
        // is through its ramp yet.
        let (_, needs_frame) = painted(&mut widget, 20, None);
        assert!(needs_frame, "the batch owes frames");

        // One stagger step in, the rows are at different points of the reveal:
        // the first is climbing, the last has not started.
        let (rec, needs_frame) = painted(&mut widget, 120, None);
        assert!(needs_frame);
        assert!(
            !rec.layers.is_empty() && rec.layers.len() <= 3,
            "only the three new rows composite through a layer: {:?}",
            rec.layers
        );
        let mut sorted = rec.layers.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(
            sorted.len() > 1 && sorted[0] < sorted[sorted.len() - 1],
            "the batch is staggered, not simultaneous: {sorted:?}"
        );

        let (rec, needs_frame) = painted(&mut widget, 5_000, None);
        assert!(!needs_frame, "the entrance settles");
        assert!(rec.layers.is_empty(), "and stops compositing");
    }

    /// `stagger_entrance(false)` leaves the rows to their own entrances.
    #[test]
    fn the_batch_entrance_can_be_switched_off() {
        let three = message_scroller(rows(3)).stagger_entrance(false);
        let mut widget = laid_out(&three);
        painted(&mut widget, 0, None);
        let mut next_id = 0u64;
        let next = message_scroller(rows(6)).stagger_entrance(false);
        View::<Chat>::rebuild(&next, &three, &mut widget, &mut BuildCtx::new(&mut next_id));
        relayout(&mut widget);
        let (rec, needs_frame) = painted(&mut widget, 20, None);
        assert!(rec.layers.is_empty());
        assert!(!needs_frame);
    }

    /// `reduce_motion` collapses both the affordance's reveal and the batch
    /// entrance, and lands the glide immediately.
    #[test]
    fn reduce_motion_collapses_the_reveal_the_stagger_and_the_glide() {
        let theme = reduced();
        let mut widget = laid_out(&view(20));
        let mut state = Chat::default();
        painted(&mut widget, 0, Some(&theme));

        dispatch(&mut widget, &mut state, &wheel(-400.0));
        let (_, needs_frame) = painted(&mut widget, 10, Some(&theme));
        assert!(!needs_frame, "the reveal lands at once");
        assert!(widget.jump_visible());

        let button = widget.jump_box().center();
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Down, button.x, button.y),
        );
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Up, button.x, button.y),
        );
        let (_, needs_frame) = painted(&mut widget, 20, Some(&theme));
        assert_eq!(widget.offset(), widget.max_offset(), "the glide landed");
        assert!(!needs_frame);
    }

    /// A drag scrolls the viewport, and lifting with speed leaves a fling that
    /// the paint pass integrates — over the framework's own decay math.
    #[test]
    fn a_drag_scrolls_and_a_flick_flings() {
        let mut widget = laid_out(&view(30));
        let mut state = Chat::default();
        painted(&mut widget, 0, None);
        let start = widget.offset();

        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Down, 40.0, 150.0),
        );
        // Past the slop, then a real drag downward (which scrolls up the
        // transcript).
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Move, 40.0, 150.0 + TOUCH_SLOP + 5.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Move, 40.0, 400.0),
        );
        assert!(widget.offset() < start, "the content followed the finger");
        assert!(!widget.is_pinned());
    }

    /// The rows are clipped to the viewport — a transcript taller than its box
    /// does not paint outside it.
    #[test]
    fn the_rows_are_clipped_to_the_viewport() {
        let mut widget = laid_out(&view(20));
        let (rec, _) = painted(&mut widget, 0, None);
        assert_eq!(rec.clips, 1, "one content clip");
    }

    /// Every offset change is published into the [`ScrollFx`] fold, so a
    /// caller reading progress or velocity sees this surface's motion.
    #[test]
    fn every_offset_change_is_published_into_the_scroll_fold() {
        let mut widget = laid_out(&view(20));
        let mut state = Chat::default();
        let observations = widget.scroll_fx().observations();
        assert!(observations > 0, "layout publishes the pinned position");
        assert!(
            (widget.scroll_fx().progress() - 1.0).abs() < 1e-6,
            "at the end"
        );

        dispatch(&mut widget, &mut state, &wheel(-200.0));
        assert!(widget.scroll_fx().observations() > observations);
        assert!(widget.scroll_fx().progress() < 1.0);
        assert!(widget.scroll_fx().delta() < 0.0, "moved up the transcript");
    }

    /// A `Cancel` drops a pending deferred report rather than firing it — the
    /// baseline surface's own convention.
    #[test]
    fn a_cancel_drops_a_pending_report() {
        let mut widget = laid_out(&view(20));
        let mut state = Chat::default();
        widget.pending_notify = Some(false);
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Cancel, 5.0, 5.0),
        );
        assert!(state.reports.is_empty(), "dropped, not fired");
        assert_eq!(widget.pending_notify, None);
    }

    /// A rebuild that shortens the transcript is not an entrance.
    #[test]
    fn a_shrinking_transcript_stages_no_entrance() {
        let six = view(6);
        let mut widget = laid_out(&six);
        painted(&mut widget, 0, None);
        rebuild(&mut widget, &six, &view(3));
        let (rec, needs_frame) = painted(&mut widget, 10, None);
        assert!(rec.layers.is_empty());
        assert!(!needs_frame);
    }

    // ---- Typeface: the jump button's label follows the live theme ------------

    use crate::text::typeface_probe::{
        Face, Probe, assert_all, assert_control, assert_follows_a_live_family_swap_on,
    };

    /// A glyph-free transcript scrolled away from its live edge, so the jump
    /// button's label is the one painted run.
    fn scrolled_probe()
    -> Probe<MessageScrollerView<()>, impl FnMut(&mut ()) -> MessageScrollerView<()>> {
        let logic = |_: &mut ()| {
            message_scroller::<()>(
                (0..20).map(|_| any(frust::SizedBox::<()>(Some(200.0), Some(40.0)))),
            )
        };
        let mut probe = Probe::new(logic, BOX, crate::theme());
        probe.frame();
        probe.event(&wheel(-300.0));
        probe
    }

    #[test]
    fn the_jump_label_paints_in_geist_under_the_beui_theme() {
        assert_control("the jump button", BOX);
        let faces = scrolled_probe().frame();
        assert_eq!(faces.len(), 1, "exactly the jump label paints");
        assert_all(
            "the jump button",
            "under the beUI theme",
            &faces,
            Face::Geist,
        );
    }

    #[test]
    fn the_jump_label_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap_on("the jump button", &mut scrolled_probe());
    }
}
