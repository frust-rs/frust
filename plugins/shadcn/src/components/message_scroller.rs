//! Ports shadcn/ui's **MessageScroller** (the auto-scrolling chat message
//! viewport) from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/message-scroller.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17).
//!
//! Upstream is a thin styling shell over `@shadcn/react/message-scroller`, whose
//! *behavior* — not its markup — is the component: a viewport that stays pinned
//! to the live edge while new messages stream in, detaches the moment the reader
//! scrolls up, and offers a "scroll to end" affordance until they come back. The
//! port re-implements that manager rather than taking on the dependency.
//!
//! # The behavior contract
//!
//! - **Stuck** (the initial state): every layout re-pins the offset to the end,
//!   so content growth is *pinned*, never chased — a message that arrives while
//!   the reader is at the live edge is already fully visible on the frame it
//!   lands, with no catch-up animation leaving it below the fold.
//! - **Detached**: a user scroll (wheel or drag) that leaves the offset more than
//!   [`STICK_BAND`] px from the end detaches, and from then on growth *preserves
//!   the viewport* — the offset is untouched, so the reader's place holds while
//!   the content gets taller below them.
//! - **Re-stick**: any user scroll that lands back inside the band re-sticks, as
//!   does a layout that clamps the offset onto the end (content shrinking away
//!   under a detached reader).
//! - **The button** ([`MessageScrollerView::button_label`]) is the source's
//!   `MessageScrollerButton` with `direction="end"`: it fades and slides in while
//!   detached (`data-[active=false]:opacity-0 translate-y-full`, `duration-200`),
//!   is inert while stuck (`pointer-events-none`), and on press glides the
//!   viewport to the end and re-sticks.
//! - Every stick/detach transition is reported once through
//!   [`on_stick_change`](MessageScrollerView::on_stick_change).
//!
//! # Why the manager owns the offset (and what that cost)
//!
//! The obvious shape — wrap [`frust::scroll_view`] the way
//! [`scroll_area`](crate::components::scroll_area) does and drive it — does not
//! work, for a reason worth recording rather than rediscovering:
//!
//! - **There is no offset *write* seam.** Nothing in the baseline's public
//!   surface sets a scroll position from outside (the same limit
//!   `scroll_area`'s docs record for its un-draggable thumb). The only way to
//!   move it is to synthesize an `InputEvent::Scroll` into it, which needs an
//!   `EventCtx` — available *only* during an event pass. Content growth arrives
//!   on a rebuild, and a rebuild dispatches no event, so a wrapped baseline
//!   would stay put until the reader next touched the screen: the one moment
//!   stick-to-bottom exists for is exactly the moment it could not act.
//! - **There is no offset *read* seam either.** `ScrollWidget::offset` sits on a
//!   type the facade does not export, so a wrapper cannot ask its child where it
//!   is; `scroll_area` solves that by making the app feed the position back,
//!   which turns every frame of a fling into an app-state round trip.
//!
//! So this widget is a scroll surface in its own right: it clips a viewport,
//! owns the offset, and consumes the wheel/drag itself — using the framework's
//! shared gesture vocabulary ([`frust::input`]'s `TOUCH_SLOP`, `WHEEL_LINE_PX`,
//! `VelocityTracker` and the fling-decay math) so its feel matches the baseline
//! rather than being re-tuned here.
//!
//! **User vs. programmatic is structural, not heuristic.** Because the offset
//! has exactly one owner, a *user* scroll is precisely an offset change made in
//! the wheel or drag arm of [`Widget::event`] — the only two places that
//! re-evaluate stickiness from the offset. Every other write (the layout re-pin,
//! the button's glide, a fling settling onto the end) is programmatic by
//! construction, and none of them can be mistaken for the reader scrolling away.
//!
//! **What riding the baseline would have given, and does not:** overscroll
//! rubber-banding and pull-to-refresh are not implemented here (a chat's live
//! edge is not a rubber-band surface); wheel, drag, fling and clipping are.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    CursorIcon, ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, Point, PointerPhase, Role, RoundedRect, ScrollDelta, SemanticsCtx, Shape, Size,
    Vec2, View, Widget, any, build_child, erase_callback_arg, rebuild_child, rebuild_children,
    route_event, route_event_single, teardown_child, visit_children,
};
use frust::input::{
    FLING_STOP, TOUCH_SLOP, VelocityTracker, WHEEL_LINE_PX, fling_decay, fling_displacement,
};
use frust::{AnimationController, Curve, FrameTime, Theme};

use crate::components::input::FALLBACK;
use crate::hit::inside;
use crate::style::{self, PATH_TOLERANCE};
use crate::tokens::ShadcnTokens;

/// `gap-8` — the content column's spacing between messages.
const CONTENT_GAP: f64 = style::SPACING_UNIT * 8.0;
/// How close to the end (in logical px) counts as *at* the end.
///
/// One band, used in both directions: a user scroll leaving more than this much
/// content below the fold detaches, and one landing back inside it re-sticks. A
/// single threshold is what keeps the two rules from fighting — a detach
/// distance smaller than the re-stick distance would re-stick on the same event
/// that detached. The value is this port's own (upstream's manager exposes a
/// comparable few-pixel tolerance); it is large enough to absorb sub-pixel
/// layout drift and trackpad jitter, small enough that a deliberate scroll of a
/// single wheel notch detaches.
pub const STICK_BAND: f64 = 8.0;
/// `size-8` — the button's edge (`size="icon-sm"`).
const BUTTON_EDGE: f64 = style::HEIGHT_SM;
/// `bottom-4` — the button's inset from the viewport's bottom edge.
const BUTTON_INSET: f64 = style::SPACING_UNIT * 4.0;
/// `duration-200` — the button's fade/slide.
const REVEAL_MS: u64 = 200;
/// How long the button's press takes to glide the viewport to the end.
///
/// Upstream animates this with a spring supplied by its scroll-manager library;
/// this port picks a duration on the catalog's own `duration-200`-family scale,
/// one step longer because it covers a whole viewport rather than 32px of
/// button.
const GLIDE_MS: u64 = 300;
/// Lucide's icon viewBox edge — `arrow-down` is authored on a 24-unit grid.
const LUCIDE_VIEWBOX: f64 = 24.0;
/// Lucide's stroke width, on that same grid.
const LUCIDE_STROKE: f64 = 2.0;

/// A view-held, typed stick-state callback.
type OnStickChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative shadcn message scroller. See the [module docs](self).
pub struct MessageScrollerView<State: 'static> {
    items: Vec<AnyView<State>>,
    button_label: String,
    on_stick_change: Option<OnStickChange<State>>,
}

/// A stick-to-bottom chat viewport over `items`, oldest first.
///
/// Wrap each item in [`message_scroller_item`] (the source's `Item` slot) or
/// hand any view in directly — the column stretches every child to its own
/// width either way.
pub fn message_scroller<State: 'static>(items: Vec<AnyView<State>>) -> MessageScrollerView<State> {
    MessageScrollerView {
        items,
        button_label: DEFAULT_BUTTON_LABEL.to_owned(),
        on_stick_change: None,
    }
}

/// The button's accessible name — the source's `<span className="sr-only">`
/// for `direction="end"`.
const DEFAULT_BUTTON_LABEL: &str = "Scroll to end";

impl<State: 'static> MessageScrollerView<State> {
    /// Replace the scroll-to-end button's accessible name (the source's
    /// screen-reader-only label; the button itself is always the icon).
    pub fn button_label(mut self, label: impl Into<String>) -> Self {
        self.button_label = label.into();
        self
    }

    /// Observe the stick state: `true` when the viewport (re-)pins to the live
    /// edge, `false` when the reader detaches from it.
    ///
    /// Fired once per transition, never per scroll event. A transition resolved
    /// during paint (a fling settling onto the end) is delivered on the next
    /// event pass — the same one-event deferral `ScrollView::on_scroll` uses for
    /// its own paint-driven motion, since paint carries no `EventCtx`; a
    /// `Cancel` drops a pending delivery without firing it.
    pub fn on_stick_change<F: Fn(&mut State, bool) + 'static>(mut self, callback: F) -> Self {
        self.on_stick_change = Some(Rc::new(callback));
        self
    }
}

/// A programmatic glide of the offset toward the live edge.
struct Glide {
    anim: AnimationController,
    from: f64,
    to: f64,
}

/// The retained widget for a [`MessageScrollerView`]. See the [module
/// docs](self) for the behavior contract it implements.
pub struct MessageScrollerWidget {
    items: Vec<ChildPod>,
    /// Each item's top in *content* space, resolved at layout.
    item_tops: Vec<f64>,
    /// The scroll offset in `[0, max_offset]` — this widget's, not a child's.
    offset: f64,
    viewport: Size,
    content_height: f64,
    /// Whether the viewport is pinned to the live edge.
    stuck: bool,

    // Drag/fling gesture state, mirroring the baseline scroll surface's shape.
    down_active: bool,
    dragging: bool,
    down_start: Point,
    last_drag: Point,
    tracker: VelocityTracker,
    fling: Option<f64>,
    /// The most recent paint clock, reused as the event-pass timestamp for
    /// velocity tracking (the event pass carries no clock of its own).
    last_frame_time: FrameTime,
    /// Last animation frame time for the fling pump; `None` seeds the clock.
    last_anim: Option<FrameTime>,

    // The scroll-to-end button.
    button_label: String,
    button_hovered: bool,
    button_pressed: bool,
    button_captured: bool,
    reveal: AnimationController,
    reveal_target: f64,
    glide: Option<Glide>,

    /// A stick transition resolved during paint or layout, delivered on the
    /// next event.
    pending_stick_notify: Option<bool>,
    /// The last state the app was actually told about, so a report is owed only
    /// for a change it can observe (see [`notify_stick`](Self::notify_stick)).
    last_notified: bool,
    on_stick_change: Option<ErasedArgCallback<bool>>,
}

impl<State: 'static> View<State> for MessageScrollerView<State> {
    type Element = MessageScrollerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MessageScrollerWidget {
        MessageScrollerWidget {
            items: self.items.iter().map(|v| build_child(v, ctx)).collect(),
            item_tops: Vec::new(),
            offset: 0.0,
            viewport: Size::ZERO,
            content_height: 0.0,
            stuck: true,
            down_active: false,
            dragging: false,
            down_start: Point::ZERO,
            last_drag: Point::ZERO,
            tracker: VelocityTracker::new(),
            fling: None,
            last_frame_time: FrameTime::ZERO,
            last_anim: None,
            button_label: self.button_label.clone(),
            button_hovered: false,
            button_pressed: false,
            button_captured: false,
            reveal: AnimationController::new(Duration::from_millis(REVEAL_MS))
                .with_curve(Curve::EaseOut),
            reveal_target: 0.0,
            glide: None,
            pending_stick_notify: None,
            // The surface starts stuck, which is what an app assumes too.
            last_notified: true,
            on_stick_change: self.on_stick_change.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MessageScrollerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable — always reinstall the erased adapter.
        element.on_stick_change = self.on_stick_change.as_ref().map(erase_callback_arg);
        let mut flags = rebuild_children(
            &prev.items,
            &self.items,
            &mut element.items,
            ctx,
            |v| v,
            |_| None,
        );
        if element.button_label != self.button_label {
            element.button_label = self.button_label.clone();
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut MessageScrollerWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.items.iter().zip(element.items.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl MessageScrollerWidget {
    /// The maximum scroll offset (`content − viewport`, never negative).
    pub fn max_offset(&self) -> f64 {
        (self.content_height - self.viewport.height).max(0.0)
    }

    /// How far the viewport currently sits from the live edge.
    fn distance_from_end(&self) -> f64 {
        (self.max_offset() - self.offset).max(0.0)
    }

    /// Whether the viewport is pinned to the live edge — the state
    /// [`MessageScrollerView::on_stick_change`] reports.
    pub fn is_stuck(&self) -> bool {
        self.stuck
    }

    fn set_offset(&mut self, value: f64) {
        self.offset = value.clamp(0.0, self.max_offset());
    }

    /// Re-place every item for the current offset. Called from layout and again
    /// from paint, so a fling/glide advanced at paint time moves the content
    /// without a relayout (the baseline scroll surface's own arrangement).
    fn sync_item_origins(&mut self) {
        let offset = self.offset;
        for (pod, top) in self.items.iter_mut().zip(self.item_tops.iter()) {
            pod.set_origin(Point::new(0.0, top - offset));
        }
    }

    /// Re-evaluate stickiness from the offset, returning the new state when it
    /// changed. The single rule both user scroll arms apply.
    fn resolve_stick(&mut self) -> Option<bool> {
        let next = self.distance_from_end() <= STICK_BAND;
        (next != self.stuck).then(|| {
            self.stuck = next;
            next
        })
    }

    /// Apply a user-driven offset delta and report the resulting stick change.
    fn user_scroll(&mut self, ctx: &mut EventCtx, delta: f64) {
        self.set_offset(self.offset + delta);
        self.sync_item_origins();
        if let Some(stuck) = self.resolve_stick() {
            self.notify_stick(ctx, stuck);
        }
        ctx.request_redraw();
    }

    /// Fire the stick callback (event pass only — it needs `&mut State`).
    ///
    /// Gated on what the app was last told, not on the transition that reached
    /// here: two transitions resolved between event passes (a fling detaching
    /// and re-sticking within one paint sequence) net out to nothing, and the
    /// app hears one report per *observable* change rather than a spurious echo
    /// of the state it already holds.
    fn notify_stick(&mut self, ctx: &mut EventCtx, stuck: bool) {
        if self.last_notified == stuck {
            return;
        }
        self.last_notified = stuck;
        if let Some(cb) = self.on_stick_change.as_mut() {
            cb(ctx, stuck);
        }
    }

    /// Deliver a stick transition resolved during paint or layout, neither of
    /// which has an `EventCtx` of its own.
    fn deliver_pending_stick(&mut self, ctx: &mut EventCtx) {
        if let Some(stuck) = self.pending_stick_notify.take() {
            self.notify_stick(ctx, stuck);
        }
    }

    /// Start the button's glide to the live edge. The theme (and therefore
    /// `reduce_motion`) is unreadable in the event pass, so the glide is only
    /// *armed* here and resolved on the next paint.
    fn glide_to_end(&mut self) {
        let mut anim =
            AnimationController::new(Duration::from_millis(GLIDE_MS)).with_curve(Curve::EaseOut);
        anim.forward();
        self.fling = None;
        self.glide = Some(Glide {
            anim,
            from: self.offset,
            to: self.max_offset(),
        });
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

    /// Advance the button's glide, returning whether it is still running.
    fn glide_tick(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        let Some(glide) = self.glide.as_mut() else {
            return false;
        };
        if reduce_motion {
            let to = glide.to;
            self.glide = None;
            self.set_offset(to);
            return false;
        }
        let animating = glide.anim.advance(now);
        let t = glide.anim.value_clamped();
        let value = glide.from + (glide.to - glide.from) * t;
        if animating {
            self.set_offset(value);
            true
        } else {
            let to = glide.to;
            self.glide = None;
            self.set_offset(to);
            false
        }
    }

    /// The last painted frame time in ms — the event pass's timestamp source
    /// for velocity tracking (the pass carries no clock of its own).
    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    /// The button's box in widget-local coordinates: `size-8`, centered,
    /// `bottom-4`.
    fn button_rect(&self) -> (Point, Size) {
        let size = Size::new(BUTTON_EDGE, BUTTON_EDGE);
        let origin = Point::new(
            (self.viewport.width - BUTTON_EDGE) / 2.0,
            (self.viewport.height - BUTTON_EDGE - BUTTON_INSET).max(0.0),
        );
        (origin, size)
    }

    /// Whether `position` (widget-local) lands on a button that is currently
    /// interactive — `data-[active=false]:pointer-events-none` means a stuck
    /// viewport's button is not.
    fn hits_button(&self, position: Point) -> bool {
        if self.stuck {
            return false;
        }
        let (origin, size) = self.button_rect();
        inside(position - origin.to_vec2(), size)
    }
}

impl Widget for MessageScrollerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        // `flex h-max min-h-full flex-col gap-8`: a column at the viewport's
        // width, as tall as it needs to be.
        let item_bc = BoxConstraints::new(Size::new(width, 0.0), Size::new(width, f64::INFINITY));
        self.item_tops.clear();
        let mut y = 0.0_f64;
        for (index, pod) in self.items.iter_mut().enumerate() {
            if index > 0 {
                y += CONTENT_GAP;
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
        if let Some(glide) = self.glide.as_mut() {
            // A glide in flight chases the moving edge rather than being
            // overridden by the pin below.
            glide.to = max;
        } else if self.stuck {
            self.offset = max;
        } else {
            self.offset = self.offset.clamp(0.0, max);
            // Content that shrank away under a detached reader can leave the
            // viewport sitting on the end: that is being at the live edge, so
            // it re-sticks (reported on the next event — layout has no
            // `EventCtx` either).
            if let Some(stuck) = self.resolve_stick() {
                self.pending_stick_notify = Some(stuck);
            }
        }
        self.sync_item_origins();
        bc.constrain(self.viewport)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The authoritative hover read: a pointer that left sends this widget
        // nothing.
        if !ctx.is_hovered() {
            self.button_hovered = false;
        }
        let now = ctx.frame_time();
        self.last_frame_time = now;

        // Every theme read up front: `&Theme` borrows the context immutably,
        // and the child paints below need it mutably.
        let (reduce_motion, colors, radius) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                ButtonColors::resolve(theme),
                ShadcnTokens::resolve_radius(None, theme).md,
            )
        };

        let mut owes_frame = false;
        // 1. The fling, integrated from the shared decay math.
        if self.fling.is_some() {
            let dt_ms = match self.last_anim {
                Some(last) => now.saturating_sub(last).as_secs_f64() * 1000.0,
                None => 0.0,
            };
            self.last_anim = Some(now);
            owes_frame |= self.fling_tick(dt_ms);
            if let Some(stuck) = self.resolve_stick() {
                self.pending_stick_notify = Some(stuck);
            }
        }
        // 2. The button's glide (`reduce_motion` lands it immediately).
        owes_frame |= self.glide_tick(now, reduce_motion);
        self.sync_item_origins();

        // 3. The button's own reveal: `data-active` follows the stick state.
        let target = if self.stuck { 0.0 } else { 1.0 };
        if reduce_motion {
            self.reveal.stop();
            self.reveal_target = target;
        } else if self.reveal_target != target {
            self.reveal_target = target;
            self.reveal.animate_to(target);
        }
        if self.reveal.is_animating() {
            owes_frame |= self.reveal.advance(now);
        }
        let reveal = if reduce_motion {
            target
        } else {
            self.reveal.value_clamped()
        };

        // The scrolled content, clipped to the viewport.
        let (origin, size) = (ctx.origin(), ctx.size());
        scene.push_clip(origin, size);
        for pod in &mut self.items {
            pod.paint_child(ctx, scene);
        }
        scene.pop_clip();

        // The button rides *over* the viewport (`absolute`), so it is painted
        // outside the content clip.
        if reveal > 0.0 {
            self.paint_button(scene, origin, reveal, colors, radius);
        }
        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A transition resolved at paint is delivered here — except on a
        // `Cancel`, which drops it without firing (the baseline surface's own
        // convention for its deferred scroll notification).
        let is_cancel = matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Cancel);
        if is_cancel {
            self.pending_stick_notify = None;
        } else {
            self.deliver_pending_stick(ctx);
        }

        // The wheel belongs to the surface, never to an item (the baseline
        // scroll surface consumes it the same way).
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
            // Broadcasts and focus-routed events reach the items untouched.
            return route_event(&mut self.items, ctx, event);
        };
        let position = p.position;
        match p.phase {
            PointerPhase::Down => {
                if self.hits_button(position) {
                    self.button_pressed = true;
                    self.button_captured = true;
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                self.dragging = false;
                self.down_active = true;
                self.fling = None;
                self.glide = None;
                self.last_anim = None;
                self.down_start = position;
                self.last_drag = position;
                self.tracker.clear();
                self.tracker.record(self.event_time_ms(), position.y);
                ctx.capture_pointer();
                route_event(&mut self.items, ctx, event);
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.button_captured {
                    let over = self.hits_button(position);
                    // The pressed cursor is restated from the `Move` arm, the
                    // only pass that resolves one.
                    if over {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.button_pressed != over {
                        self.button_pressed = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Handled;
                }
                if !self.down_active {
                    // A hover move: the items get it first, so a claiming
                    // descendant is the one recorded, and the button's own
                    // claim is the fallback.
                    let routed = route_event(&mut self.items, ctx, event);
                    let over = self.hits_button(position);
                    if over {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.button_hovered != over {
                        self.button_hovered = over;
                        ctx.request_redraw();
                    }
                    return routed;
                }
                self.tracker.record(self.event_time_ms(), position.y);
                if self.dragging {
                    let dy = position.y - self.last_drag.y;
                    self.last_drag = position;
                    // The content follows the finger: dragging up (negative dy)
                    // moves the offset down the content.
                    self.user_scroll(ctx, -dy);
                } else if (position.y - self.down_start.y).abs() > TOUCH_SLOP {
                    // Take the gesture over: cancel whatever item was armed,
                    // then stop forwarding to it.
                    self.dragging = true;
                    self.last_drag = position;
                    let cancel = InputEvent::Pointer(frust::authoring::PointerEvent {
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
                self.button_hovered = false;
                if self.button_captured {
                    let fired = self.button_pressed && self.hits_button(position);
                    self.button_pressed = false;
                    self.button_captured = false;
                    if fired {
                        // Re-stuck as of the press, not as of the landing: the
                        // affordance starts fading immediately and the glide
                        // below chases whatever the edge becomes meanwhile.
                        self.glide_to_end();
                        self.stuck = true;
                        self.notify_stick(ctx, true);
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if self.dragging {
                    let finger_v = self.tracker.velocity();
                    if finger_v.abs() > FLING_STOP {
                        // The offset moves opposite the finger.
                        self.fling = Some(-finger_v);
                        self.last_anim = None;
                    }
                } else {
                    route_event(&mut self.items, ctx, event);
                }
                self.dragging = false;
                self.down_active = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                // Clears flags only: never a callback, never app state.
                self.button_pressed = false;
                self.button_captured = false;
                self.button_hovered = false;
                self.dragging = false;
                self.down_active = false;
                route_event(&mut self.items, ctx, event);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let max_offset = self.max_offset();
        ctx.push_container(
            Role::ScrollView,
            |node| {
                node.set_scroll_y(self.offset);
                node.set_scroll_y_min(0.0);
                node.set_scroll_y_max(max_offset);
            },
            |ctx| {
                for pod in &self.items {
                    pod.semantics_child(ctx);
                }
                // The affordance is only offered while it can be activated.
                if !self.stuck {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(self.button_label.as_str());
                        node.add_action(Action::Click);
                    });
                }
            },
        );
    }

    visit_children!(items);
}

/// The button's paint, kept beside the palette it reads rather than inside the
/// widget's behavior block above.
impl MessageScrollerWidget {
    /// Paint the scroll-to-end button at `reveal` ∈ `[0, 1]`:
    /// `data-[active=false]` is `opacity-0 translate-y-full`, so the button
    /// fades in as it slides up out of the viewport's bottom edge.
    fn paint_button(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        reveal: f64,
        colors: ButtonColors,
        radius: f64,
    ) {
        let (local, size) = self.button_rect();
        let slide = (1.0 - reveal) * (BUTTON_EDGE + BUTTON_INSET);
        let at = origin + local.to_vec2() + Vec2::new(0.0, slide);
        let alpha = reveal as f32;
        // `hover:bg-muted` — a fill-token swap, no state layer.
        let fill = if self.button_hovered || self.button_pressed {
            colors.hover_fill
        } else {
            colors.fill
        };
        // `shadow-xs`, faded with everything else rather than left standing
        // under a button that is on its way out.
        scene.draw_shadow(
            Point::new(at.x, at.y + style::SHADOW_XS.y_offset),
            size,
            radius,
            style::SHADOW_XS.std_dev,
            style::scale_alpha(colors.shadow, alpha),
        );
        scene.fill_rounded_rect(at, size, radius, style::scale_alpha(fill, alpha));
        let half = style::BORDER_WIDTH / 2.0;
        let border = RoundedRect::new(
            half,
            half,
            size.width - half,
            size.height - half,
            (radius - half).max(0.0),
        );
        scene.stroke_path(
            at,
            &border.to_path(PATH_TOLERANCE),
            style::BORDER_WIDTH,
            &Brush::Solid(style::scale_alpha(colors.border, alpha)),
        );
        draw_arrow_down(
            scene,
            at + Vec2::new(size.width / 2.0, size.height / 2.0),
            style::ICON_SIZE,
            style::scale_alpha(colors.ink, alpha),
        );
    }
}

/// The button's resolved palette: `border-border bg-background text-foreground
/// hover:bg-muted`.
#[derive(Clone, Copy)]
struct ButtonColors {
    fill: Color,
    hover_fill: Color,
    border: Color,
    ink: Color,
    shadow: Color,
}

impl ButtonColors {
    fn resolve(theme: Option<&Theme>) -> Self {
        let shadow = style::SHADOW_XS.color(theme);
        match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                ButtonColors {
                    fill: scheme.surface,
                    hover_fill: scheme.surface_container_highest,
                    border: scheme.outline,
                    ink: scheme.on_surface,
                    shadow,
                }
            }
            None => ButtonColors {
                fill: FALLBACK.background,
                hover_fill: FALLBACK.muted,
                border: FALLBACK.border,
                ink: FALLBACK.foreground,
                shadow,
            },
        }
    }
}

/// Paint lucide's `arrow-down` centered on `center`, `extent` px on a side.
///
/// The glyph is `M12 5v14` + `m19 12-7 7-7-7` on a 24-unit viewBox with a
/// 2-unit stroke — a stem and a chevron head, both scaled by `extent / 24`.
/// It lives here rather than beside `native_select`'s shared `draw_chevron`
/// because this is the catalog's only arrow: a chevron alone reads as a
/// disclosure, and this button means "go to the end".
fn draw_arrow_down(scene: &mut dyn PaintScene, center: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let width = LUCIDE_STROKE * scale;
    let point = |x: f64, y: f64| Point::new(x * scale, y * scale);

    let mut stem = BezPath::new();
    stem.move_to(point(0.0, -7.0));
    stem.line_to(point(0.0, 7.0));
    scene.stroke_path(center, &stem, width, &Brush::Solid(color));

    let mut head = BezPath::new();
    head.move_to(point(7.0, 0.0));
    head.line_to(point(0.0, 7.0));
    head.line_to(point(-7.0, 0.0));
    scene.stroke_path(center, &head, width, &Brush::Solid(color));
}

// ---- MessageScrollerItem --------------------------------------------------

/// A declarative `MessageScrollerItem`: one row of the scroller's content
/// column.
pub struct MessageScrollerItemView<State: 'static> {
    child: AnyView<State>,
}

/// Wrap `child` as one scroller row — the source's `Item` slot
/// (`min-w-0 shrink-0`).
///
/// A transparent box: it takes the column's full width and its child's natural
/// height, contributes no paint of its own, and forwards events and semantics
/// straight through. It is the seam the source's two *browser* affordances
/// would attach to — `content-visibility:auto` (offscreen render skipping) and
/// `scrollAnchor` (anchor-to-this-item instead of to the end) — neither of
/// which is ported: frust has no content-visibility equivalent, and this
/// manager's only anchor is the live edge.
pub fn message_scroller_item<State: 'static, V: View<State>>(
    child: V,
) -> MessageScrollerItemView<State> {
    MessageScrollerItemView { child: any(child) }
}

/// The retained widget for a [`MessageScrollerItemView`].
pub struct MessageScrollerItemWidget {
    child: ChildPod,
}

impl<State: 'static> View<State> for MessageScrollerItemView<State> {
    type Element = MessageScrollerItemWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MessageScrollerItemWidget {
        MessageScrollerItemWidget {
            child: build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MessageScrollerItemWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut MessageScrollerItemWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for MessageScrollerItemWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

/// The cursor the scroll-to-end button asks for, exposed for a host matching
/// the affordance elsewhere.
pub fn message_scroller_button_cursor() -> CursorIcon {
    style::ACTIVE_CURSOR
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Brightness;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent};
    use frust_core::RenderRoot;
    use std::any::Any;

    const VIEWPORT: Size = Size::new(240.0, 200.0);
    /// Each item is this tall, so the content height is predictable.
    const ITEM_H: f64 = 60.0;

    /// A fixed-size content leaf.
    struct Block;

    /// The retained half of [`Block`].
    struct BlockWidget;

    impl<S: 'static> View<S> for Block {
        type Element = BlockWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> BlockWidget {
            BlockWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut BlockWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for BlockWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(bc.max().width, ITEM_H))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }

    #[derive(Default)]
    struct AppState {
        items: usize,
        stick_events: Vec<bool>,
    }

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        clips: Vec<(Point, Size)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
            self.rects.push((origin, size));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _brush: &Brush) {}
        fn draw_shadow(
            &mut self,
            _origin: Point,
            _size: Size,
            _radius: f64,
            _std_dev: f64,
            _color: Color,
        ) {
        }
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {}
    }

    impl Recorder {
        /// The painted button, if any — the only rounded rect this widget emits.
        fn button(&self) -> Option<(Point, Size, f64, Color)> {
            self.rrects.first().copied()
        }
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// The view under test at `items` messages.
    fn view_for(items: usize) -> MessageScrollerView<AppState> {
        let items = (0..items)
            .map(|_| any(message_scroller_item::<AppState, _>(Block)))
            .collect();
        message_scroller(items)
            .on_stick_change(|s: &mut AppState, stuck| s.stick_events.push(stuck))
    }

    /// Drives the widget directly — a scroll surface keeps its offset and stick
    /// state to itself, so the assertions read them off the live widget rather
    /// than through an app-state round trip.
    struct Harness {
        view: MessageScrollerView<AppState>,
        widget: MessageScrollerWidget,
        state: AppState,
        tcx: TextContext,
        theme: Theme,
        counter: u64,
    }

    impl Harness {
        fn new(items: usize) -> Self {
            Self::with_motion(items, false)
        }

        fn with_motion(items: usize, reduce_motion: bool) -> Self {
            let mut theme = crate::theme().with_brightness(Brightness::Light);
            theme.motion.reduce_motion = reduce_motion;
            let view = view_for(items);
            let mut counter = 0u64;
            let widget = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
            let mut h = Harness {
                view,
                widget,
                state: AppState {
                    items,
                    ..AppState::default()
                },
                tcx: TextContext::new(),
                theme,
                counter,
            };
            h.layout();
            h
        }

        fn layout(&mut self) {
            let mut ctx = LayoutCtx::with_resources(
                Some(&mut self.tcx as &mut dyn Any),
                Some(&self.theme as &dyn Any),
            );
            self.widget
                .layout(&mut ctx, &BoxConstraints::loose(VIEWPORT));
        }

        /// Append `n` messages and re-lay out — the growth path.
        fn grow(&mut self, n: usize) {
            self.state.items += n;
            let next = view_for(self.state.items);
            let mut ctx = BuildCtx::new(&mut self.counter);
            View::<AppState>::rebuild(&next, &self.view, &mut self.widget, &mut ctx);
            self.view = next;
            self.layout();
        }

        /// Paint at `ms` past the epoch, then re-lay out (the pin is resolved in
        /// layout, so the geometry follows the paint).
        fn frame(&mut self, ms: f64) -> Recorder {
            let mut rec = Recorder::default();
            {
                let ctx = PaintCtx::for_test(Point::ORIGIN, VIEWPORT, ft_ms(ms));
                let mut ctx = ctx.with_theme(&self.theme as &dyn Any);
                self.widget.paint(&mut ctx, &mut rec);
            }
            self.layout();
            rec
        }

        fn dispatch(&mut self, event: &InputEvent) {
            let state: &mut dyn Any = &mut self.state;
            let mut ctx = EventCtx::new(state, Point::ORIGIN, VIEWPORT);
            self.widget.event(&mut ctx, event);
        }

        fn wheel(&mut self, dy: f64) {
            self.dispatch(&InputEvent::Scroll {
                position: Point::new(100.0, 100.0),
                delta: ScrollDelta::Pixels(0.0, dy),
            });
        }

        fn pointer(&mut self, phase: PointerPhase, at: Point) {
            self.dispatch(&InputEvent::Pointer(PointerEvent {
                phase,
                position: at,
                button: PointerButton::Primary,
            }));
        }

        fn offset(&self) -> f64 {
            self.widget.offset
        }

        fn max_offset(&self) -> f64 {
            self.widget.max_offset()
        }

        fn stuck(&self) -> bool {
            self.widget.is_stuck()
        }

        /// Where the button sits, in widget-local coordinates.
        fn button_center(&self) -> Point {
            let (origin, size) = self.widget.button_rect();
            origin + Vec2::new(size.width / 2.0, size.height / 2.0)
        }
    }

    #[test]
    fn a_short_thread_has_nothing_to_scroll_and_shows_no_button() {
        let mut h = Harness::new(2);
        assert_eq!(h.max_offset(), 0.0, "content fits the viewport");
        assert!(h.stuck(), "trivially at the live edge");
        let rec = h.frame(0.0);
        assert!(rec.button().is_none(), "the affordance stays hidden");
        assert_eq!(rec.clips.len(), 1, "the content is clipped to the viewport");
    }

    #[test]
    fn growth_while_stuck_pins_the_viewport_to_the_live_edge() {
        let mut h = Harness::new(6);
        let max = h.max_offset();
        assert!(max > 0.0, "the thread overflows: {max}");
        assert_eq!(h.offset(), max, "already at the end");

        h.grow(3);
        let grown = h.max_offset();
        assert!(grown > max, "the content got taller");
        assert_eq!(h.offset(), grown, "and the viewport followed it");
        assert!(h.stuck());
        assert!(
            h.state.stick_events.is_empty(),
            "no transition: it never left the edge"
        );
    }

    #[test]
    fn a_user_scroll_up_detaches_and_growth_then_preserves_the_viewport() {
        let mut h = Harness::new(6);
        h.wheel(-120.0);
        assert!(!h.stuck(), "scrolled clear of the band");
        assert_eq!(h.state.stick_events, vec![false], "reported once");
        let held = h.offset();

        h.grow(4);
        assert_eq!(h.offset(), held, "the reader's place is preserved");
        assert!(!h.stuck());
        assert_eq!(h.state.stick_events, vec![false], "still one transition");
        assert!(
            h.max_offset() - h.offset() > STICK_BAND,
            "and there is now more below the fold"
        );
    }

    #[test]
    fn scrolling_back_to_the_bottom_re_sticks() {
        let mut h = Harness::new(6);
        h.wheel(-120.0);
        assert!(!h.stuck());
        // Back down past the end: the offset clamps and the band is reached.
        h.wheel(400.0);
        assert!(h.stuck(), "re-stuck at the live edge");
        assert_eq!(h.state.stick_events, vec![false, true]);
        assert_eq!(h.offset(), h.max_offset());

        // A scroll *inside* the band never detaches.
        h.wheel(-(STICK_BAND / 2.0));
        assert!(h.stuck());
        assert_eq!(h.state.stick_events, vec![false, true], "no extra reports");
    }

    #[test]
    fn the_button_fades_in_while_detached_and_out_again_on_re_stick() {
        let mut h = Harness::new(6);
        assert!(h.frame(0.0).button().is_none(), "hidden while stuck");

        h.wheel(-120.0);
        // The first paint seeds the reveal clock (zero delta), so the button is
        // still fully out of the way; it slides up and fades in over the
        // source's `duration-200`.
        h.frame(0.0);
        let mid = h.frame(REVEAL_MS as f64 / 2.0).button().expect("revealing");
        let done = h
            .frame(REVEAL_MS as f64 * 2.0)
            .button()
            .expect("fully revealed");
        assert!(
            mid.3.components[3] < done.3.components[3],
            "opacity ramps: {mid:?} then {done:?}"
        );
        assert!(mid.0.y > done.0.y, "and it slides up as it fades in");
        assert_eq!(done.1, Size::new(BUTTON_EDGE, BUTTON_EDGE), "size-8");
        assert!(
            (done.0.y + BUTTON_EDGE + BUTTON_INSET - VIEWPORT.height).abs() < 1e-9,
            "bottom-4"
        );

        h.wheel(400.0);
        assert!(h.stuck());
        h.frame(0.0);
        let hidden = h.frame(REVEAL_MS as f64 * 2.0);
        assert!(hidden.button().is_none(), "fully faded out again");
    }

    #[test]
    fn pressing_the_button_glides_to_the_end_and_re_sticks() {
        let mut h = Harness::new(8);
        h.wheel(-300.0);
        assert!(!h.stuck());
        h.frame(REVEAL_MS as f64 * 2.0);
        let from = h.offset();

        let center = h.button_center();
        h.pointer(PointerPhase::Down, center);
        h.pointer(PointerPhase::Up, center);
        assert!(h.stuck(), "the press re-sticks immediately");
        assert_eq!(h.state.stick_events, vec![false, true]);
        assert_eq!(h.offset(), from, "but the offset only moves on a frame");

        h.frame(0.0);
        let mid_offset = {
            h.frame(GLIDE_MS as f64 / 2.0);
            h.offset()
        };
        assert!(
            mid_offset > from && mid_offset < h.max_offset(),
            "partway there: {mid_offset}"
        );
        h.frame(GLIDE_MS as f64 * 2.0);
        assert_eq!(h.offset(), h.max_offset(), "landed on the live edge");
    }

    #[test]
    fn reduce_motion_lands_the_press_on_the_first_frame() {
        let mut h = Harness::with_motion(8, true);
        h.wheel(-300.0);
        let center = h.button_center();
        h.pointer(PointerPhase::Down, center);
        h.pointer(PointerPhase::Up, center);
        h.frame(0.0);
        assert_eq!(h.offset(), h.max_offset(), "no intermediate frame");
        // ...and the button is already gone, with no fade.
        assert!(h.frame(0.0).button().is_none());
    }

    #[test]
    fn a_press_that_leaves_the_button_before_release_does_nothing() {
        let mut h = Harness::new(8);
        h.wheel(-300.0);
        h.frame(REVEAL_MS as f64 * 2.0);
        let before = h.offset();
        let center = h.button_center();
        h.pointer(PointerPhase::Down, center);
        h.pointer(PointerPhase::Move, Point::new(center.x, 10.0));
        h.pointer(PointerPhase::Up, Point::new(center.x, 10.0));
        assert!(!h.stuck(), "the press was abandoned");
        assert_eq!(h.offset(), before);
        assert_eq!(h.state.stick_events, vec![false]);
    }

    #[test]
    fn a_drag_scrolls_the_surface_and_detaches_it() {
        let mut h = Harness::new(8);
        assert!(h.stuck());
        let start = Point::new(100.0, 150.0);
        h.pointer(PointerPhase::Down, start);
        // The first move past the slop only takes the gesture over; the next
        // one scrolls.
        h.pointer(
            PointerPhase::Move,
            Point::new(100.0, start.y + TOUCH_SLOP + 5.0),
        );
        h.pointer(PointerPhase::Move, Point::new(100.0, start.y + 120.0));
        h.pointer(PointerPhase::Up, Point::new(100.0, start.y + 120.0));
        assert!(!h.stuck(), "dragging down walks back up the thread");
        assert!(h.offset() < h.max_offset());
        assert_eq!(h.state.stick_events, vec![false]);
    }

    #[test]
    fn a_cancel_clears_the_gesture_without_reporting_anything() {
        let mut h = Harness::new(8);
        h.wheel(-300.0);
        h.frame(REVEAL_MS as f64 * 2.0);
        let before = h.offset();
        let center = h.button_center();
        h.pointer(PointerPhase::Down, center);
        h.pointer(PointerPhase::Cancel, center);
        assert_eq!(h.offset(), before);
        assert_eq!(
            h.state.stick_events,
            vec![false],
            "no callback from a cancel"
        );
        assert!(!h.widget.button_pressed);
        assert!(!h.widget.button_captured);
    }

    /// The hover link and the cursor are root-owned, so these two tests drive
    /// the component the way an app does rather than through the direct
    /// harness above.
    fn rooted(
        items: usize,
    ) -> (
        RenderRoot<AppState, MessageScrollerView<AppState>>,
        AppState,
    ) {
        let mut root: RenderRoot<AppState, MessageScrollerView<AppState>> = RenderRoot::new();
        let mut state = AppState {
            items,
            ..AppState::default()
        };
        let mut tcx = TextContext::new();
        root.set_theme(Box::new(crate::theme().with_brightness(Brightness::Light)));
        let mut logic = |s: &mut AppState| view_for(s.items);
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(VIEWPORT, &mut tcx as &mut dyn Any);
        (root, state)
    }

    /// The button's local box, in root coordinates (the surface fills the
    /// window, so the two spaces coincide).
    fn rooted_button_center() -> Point {
        Point::new(
            VIEWPORT.width / 2.0,
            VIEWPORT.height - BUTTON_INSET - BUTTON_EDGE / 2.0,
        )
    }

    #[test]
    fn hovering_the_affordance_claims_the_link_and_asks_for_the_pointer_cursor() {
        let (mut root, mut state) = rooted(8);
        let center = rooted_button_center();
        let hover = |root: &mut RenderRoot<AppState, MessageScrollerView<AppState>>,
                     state: &mut AppState,
                     at: Point| {
            root.event(
                state,
                &InputEvent::Pointer(PointerEvent {
                    phase: PointerPhase::Move,
                    position: at,
                    button: PointerButton::Primary,
                }),
            );
        };

        // While stuck the button is `pointer-events-none`: no claim, no cursor.
        hover(&mut root, &mut state, center);
        assert_eq!(root.cursor(), CursorIcon::Default);
        assert!(!root.is_hover_active());

        root.event(
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(100.0, 100.0),
                delta: ScrollDelta::Pixels(0.0, -300.0),
            },
        );
        hover(&mut root, &mut state, center);
        assert!(root.is_hover_active(), "the container claimed the link");
        assert_eq!(root.cursor(), style::ACTIVE_CURSOR);

        // Off the button: the claim lapses and the cursor resets.
        hover(&mut root, &mut state, Point::new(center.x, 10.0));
        assert!(!root.is_hover_active());
        assert_eq!(root.cursor(), CursorIcon::Default);
    }

    #[test]
    fn the_semantics_node_is_a_scroll_view_that_offers_the_button_only_while_detached() {
        let (mut root, mut state) = rooted(8);
        let update = root.semantics();
        let scroll = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ScrollView)
            .expect("the surface reports itself");
        assert!(scroll.1.scroll_y_max().unwrap_or(0.0) > 0.0, "with a range");
        assert!(
            !update.nodes.iter().any(|(_, n)| n.role() == Role::Button),
            "no affordance while stuck"
        );

        root.event(
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(100.0, 100.0),
                delta: ScrollDelta::Pixels(0.0, -300.0),
            },
        );
        assert_eq!(state.stick_events, vec![false], "the app was told");
        let update = root.semantics();
        let button = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("the affordance is offered while detached");
        assert_eq!(button.1.label(), Some(DEFAULT_BUTTON_LABEL));
    }

    #[test]
    fn every_item_reaches_paint_and_the_inspector() {
        let mut h = Harness::new(4);
        let rec = h.frame(0.0);
        assert_eq!(rec.rects.len(), 4, "all four items painted");
        let mut seen = 0usize;
        Widget::visit_children(&h.widget, &mut |_pod| seen += 1);
        assert_eq!(seen, 4, "and all four are visible to the inspector");
    }

    #[test]
    fn the_item_wrapper_is_transparent() {
        let view: MessageScrollerItemView<()> = message_scroller_item(Block);
        let mut counter = 0u64;
        let mut widget = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = widget.layout(
            &mut ctx,
            &BoxConstraints::new(Size::new(VIEWPORT.width, 0.0), VIEWPORT),
        );
        assert_eq!(size, Size::new(VIEWPORT.width, ITEM_H));
        assert_eq!(widget.child.origin(), Point::ORIGIN);

        let mut rec = Recorder::default();
        let mut paint_ctx = PaintCtx::new(Point::ORIGIN, size);
        widget.paint(&mut paint_ctx, &mut rec);
        assert_eq!(rec.rects.len(), 1, "only the child paints");
        assert_eq!(message_scroller_button_cursor(), style::ACTIVE_CURSOR);
    }
}
