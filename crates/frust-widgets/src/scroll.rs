//! The `ScrollView` widget (spec §6.4): a vertical scroll surface with drag,
//! fling, and wheel support and clipped, offset content.
//!
//! [`scroll_view`] wraps a child that is laid out with unbounded height; the
//! view itself takes the incoming constraints and paints the child offset by
//! `-scroll_offset` inside a clip. Offsets settle within `[0, content −
//! viewport]`; a pointer *drag* past an edge is allowed out of range with
//! iOS-style rubber-band resistance ([`OVERSCROLL_RESISTANCE`]) and settles back
//! on release (wheel and fling stay hard-clamped). See [`ScrollView::on_scroll`]
//! for scroll observation and [`ScrollView::on_refresh_release`] for the
//! pull-to-refresh trigger.
//!
//! # Gesture takeover
//!
//! ScrollView captures the pointer on `Down` and forwards events to the child
//! so descendant widgets stay interactive. It *observes* `Move` deltas before
//! forwarding: once the accumulated drag passes [`TOUCH_SLOP`] it enters
//! scrolling mode — it sends the child a synthetic `Cancel` (disarming any
//! armed descendant tap/press), stops forwarding, and consumes the drag itself.
//! This is how a scroll can be *taken* from a child after the slop, matching
//! masonry (`research/RESEARCH.md`).
//!
//! # Fling driver (v1)
//!
//! On release with sufficient velocity a fling begins, integrated
//! frame-by-frame with [`ScrollWidget::tick`] (pure, unit-tested). [`paint`]
//! pumps the fling from the shared shell frame clock ([`PaintCtx::frame_time`],
//! spec §8 — no wall-clock reads in widget code) so it animates for free on the
//! continuous-loop mobile shells, and calls [`PaintCtx::request_frame`] while the
//! fling is still in flight so the desktop shell (event-driven
//! `ControlFlow::Wait`) keeps scheduling frames via `window.request_redraw()`;
//! the signal stops once the fling reaches rest. The first paint after the
//! release seeds the fling clock from `frame_time` (a zero-delta frame), and each
//! subsequent paint advances it by the inter-frame delta.

use std::rc::Rc;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, FLING_STOP,
    FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent,
    PointerPhase, ScrollDelta, SemanticsCtx, TOUCH_SLOP, VelocityTracker, View, WHEEL_LINE_PX,
    Widget, any, fling_decay, fling_displacement,
};
use kurbo::{Point, Size};

use crate::{ErasedArgCallback, ErasedCallback};

/// iOS-style rubber-band resistance applied to the past-edge portion of a drag:
/// the visible out-of-range displacement is `raw_excess * OVERSCROLL_RESISTANCE`.
///
/// **Community-approximate**: UIScrollView's rubber-banding is a
/// diminishing-returns curve (roughly `d·(1 − 1/(1 + d/dim·c))`), not a
/// published constant. A flat `0.5` factor is the common community linear
/// approximation — half the raw finger travel shows past the edge, giving the
/// pull a heavier feel the further it is dragged in *raw* terms while staying
/// cheap and deterministic to reason about. Tunable in one place if a
/// diminishing curve is wanted later.
const OVERSCROLL_RESISTANCE: f64 = 0.5;

/// Pull-past-top distance (logical px, measured on the *resisted* overscroll)
/// beyond which releasing fires [`ScrollView::on_refresh_release`] — the
/// pull-to-refresh trigger.
///
/// **Community-approximate**: iOS's `UIRefreshControl` trigger distance is not a
/// published constant; ~64pt is the value community reimplementations converge
/// on for a comfortable pull.
const REFRESH_TRIGGER_PX: f64 = 64.0;

/// Per-millisecond retain factor for the release-settle animation that returns
/// an overscrolled surface to its clamped edge: after `dt` ms the remaining
/// distance to the edge is scaled by `SETTLE_DECAY.powf(dt)`.
///
/// **Community-approximate**: `0.988` settles ~95% of the way in ≈250 ms, an
/// iOS-like snap-back with no published spring spec to match.
const SETTLE_DECAY: f64 = 0.988;

/// Distance (logical px) below which the settle animation snaps exactly to the
/// edge and stops, so it terminates instead of asymptotically approaching.
const SETTLE_STOP_PX: f64 = 0.5;

/// A scroll observation snapshot handed to [`ScrollView::on_scroll`].
///
/// `offset` is the clamped scroll position in `[0, max_offset]`; `overscroll` is
/// the signed past-edge displacement (negative = pulled past the top, positive =
/// pulled past the bottom), zero while the surface rests in range. During a
/// drag past an edge, `offset` pins at the edge and `overscroll` carries the
/// (resisted) pull.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollInfo {
    /// The clamped scroll offset in `[0, max_offset]` (px scrolled down).
    pub offset: f64,
    /// The maximum scroll offset (`content − viewport`, never negative).
    pub max_offset: f64,
    /// Signed past-edge displacement: negative past the top, positive past the
    /// bottom, `0.0` while in range.
    pub overscroll: f64,
}

/// A view-held scroll-observation callback (erased to [`ErasedArgCallback`] on
/// build).
type OnScroll<State> = Rc<dyn Fn(&mut State, ScrollInfo)>;

/// A view-held pull-to-refresh release callback (erased to [`ErasedCallback`] on
/// build).
type OnRefresh<State> = Rc<dyn Fn(&mut State)>;

/// A declarative vertical scroll surface. See the [module docs](self).
pub struct ScrollView<State: 'static> {
    child: AnyView<State>,
    /// Fired whenever the offset/overscroll changes (drag, wheel, or — one event
    /// late — fling/settle). See [`ScrollView::on_scroll`].
    on_scroll: Option<OnScroll<State>>,
    /// Fired on pointer `Up` when the past-top overscroll exceeded
    /// [`REFRESH_TRIGGER_PX`]. See [`ScrollView::on_refresh_release`].
    on_refresh_release: Option<OnRefresh<State>>,
}

impl<State: 'static> ScrollView<State> {
    /// Wrap `child` in a vertical scroll view.
    pub fn new<V: View<State>>(child: V) -> Self {
        Self {
            child: any(child),
            on_scroll: None,
            on_refresh_release: None,
        }
    }

    /// Observe scroll position changes. The callback receives a [`ScrollInfo`]
    /// snapshot each time the offset or overscroll changes due to input (drag,
    /// wheel), and — one event late — for fling/settle motion driven at paint
    /// time (the paint pass carries no [`EventCtx`], so the notification is
    /// recorded and delivered on the next event, the same controlled-component
    /// convention the interactive widgets follow). A `Cancel` clears any pending
    /// notification without firing it.
    pub fn on_scroll<F: Fn(&mut State, ScrollInfo) + 'static>(mut self, callback: F) -> Self {
        self.on_scroll = Some(Rc::new(callback));
        self
    }

    /// The pull-to-refresh trigger: fires on pointer `Up` when the surface was
    /// pulled past the top by more than [`REFRESH_TRIGGER_PX`] (post-resistance),
    /// so an app gets a single "release past threshold" signal without
    /// reimplementing overscroll thresholding. Never fires on a `Cancel`.
    pub fn on_refresh_release<F: Fn(&mut State) + 'static>(mut self, callback: F) -> Self {
        self.on_refresh_release = Some(Rc::new(callback));
        self
    }
}

/// Wrap `child` in a vertical [`ScrollView`] — the free-function spelling of
/// [`ScrollView::new`].
pub fn scroll_view<State: 'static, V: View<State>>(child: V) -> ScrollView<State> {
    ScrollView::new(child)
}

/// The retained widget for a [`ScrollView`].
pub struct ScrollWidget {
    child: ChildPod,
    /// Current *effective* scroll offset (px scrolled down). Normally in
    /// `[0, max_offset]`, but a drag past an edge lets it go out of range (with
    /// [`OVERSCROLL_RESISTANCE`] applied) until the release-settle brings it back;
    /// the fling/wheel/layout paths still hard-clamp via [`ScrollWidget::set_offset`].
    offset: f64,
    /// Resolved viewport size (this widget's own size).
    viewport: Size,
    /// The child's (content) size after an unbounded-height layout.
    content: Size,
    /// Whether we have taken the gesture over as a scroll drag.
    scrolling: bool,
    /// The raw (un-resisted) drag position accumulated during an active scroll
    /// drag; seeded from `offset` at takeover and moved by each drag delta.
    /// [`OVERSCROLL_RESISTANCE`] is applied to its out-of-range portion to derive
    /// the effective `offset`, so the resistance never compounds across moves.
    drag_raw: f64,
    /// Whether a release-settle animation is returning an overscrolled surface to
    /// its clamped edge (driven at paint via [`ScrollWidget::settle_tick`]).
    settling: bool,
    /// A scroll notification produced by the paint-time fling/settle pump (which
    /// carries no [`EventCtx`]); delivered to `on_scroll` on the next event and
    /// cleared by a `Cancel` without firing.
    pending_scroll_notify: bool,
    /// The scroll observation callback (`None` if the view set none).
    on_scroll: Option<ErasedArgCallback<ScrollInfo>>,
    /// The pull-to-refresh release callback (`None` if the view set none).
    on_refresh_release: Option<ErasedCallback>,
    /// Whether a `Down` has armed an active gesture (distinct from `scrolling`,
    /// which only becomes true after the drag passes the slop). Set on `Down`
    /// and cleared on `Up`/`Cancel`; the slop/takeover math runs only while it
    /// is true, so a hover `Move` (dispatched on every cursor motion) is never
    /// mistaken for a drag and can never take the gesture from a child.
    down_active: bool,
    down_start: Point,
    last_drag: Point,
    tracker: VelocityTracker,
    /// Active fling velocity, in px/s of *offset* (opposite the finger), or
    /// `None` when not flinging.
    fling: Option<f64>,
    /// The most recent frame time observed during [`Widget::paint`], reused as the
    /// event-pass timestamp for velocity tracking (the event pass carries no clock
    /// of its own — spec §8 provides time only at paint; the fling starts from the
    /// last paint clock, which is today's behavior too).
    last_frame_time: FrameTime,
    /// Last animation frame time for the paint-time fling pump; `None` seeds the
    /// clock (zero-delta) on the first paint after a release.
    last_anim: Option<FrameTime>,
}

impl ScrollWidget {
    fn new(child: ChildPod) -> Self {
        Self {
            child,
            offset: 0.0,
            viewport: Size::ZERO,
            content: Size::ZERO,
            scrolling: false,
            drag_raw: 0.0,
            settling: false,
            pending_scroll_notify: false,
            on_scroll: None,
            on_refresh_release: None,
            down_active: false,
            down_start: Point::ZERO,
            last_drag: Point::ZERO,
            tracker: VelocityTracker::new(),
            fling: None,
            last_frame_time: FrameTime::ZERO,
            last_anim: None,
        }
    }

    /// The current scroll offset.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// The maximum scroll offset (`content − viewport`, never negative).
    pub fn max_offset(&self) -> f64 {
        (self.content.height - self.viewport.height).max(0.0)
    }

    /// Whether a fling animation is in flight.
    pub fn is_flinging(&self) -> bool {
        self.fling.is_some()
    }

    /// The last painted frame time as milliseconds — the event-pass timestamp
    /// source for velocity tracking (see [`ScrollWidget::last_frame_time`]).
    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    fn set_offset(&mut self, value: f64) {
        self.offset = value.clamp(0.0, self.max_offset());
    }

    fn sync_child_origin(&mut self) {
        self.child.set_origin(Point::new(0.0, -self.offset));
    }

    /// A snapshot of the current scroll position for [`ScrollView::on_scroll`]:
    /// the clamped `offset`, the `max_offset`, and the signed past-edge
    /// `overscroll` (negative past the top). See [`ScrollInfo`].
    fn scroll_info(&self) -> ScrollInfo {
        let max = self.max_offset();
        let overscroll = if self.offset < 0.0 {
            self.offset
        } else if self.offset > max {
            self.offset - max
        } else {
            0.0
        };
        ScrollInfo {
            offset: self.offset.clamp(0.0, max),
            max_offset: max,
            overscroll,
        }
    }

    /// Fire `on_scroll` (if set) with the current [`ScrollInfo`]. Called from the
    /// event pass after an input-driven offset/overscroll change.
    fn notify_scroll(&mut self, ctx: &mut EventCtx) {
        let info = self.scroll_info();
        if let Some(cb) = self.on_scroll.as_mut() {
            cb(ctx, info);
        }
    }

    /// Deliver a fling/settle notification recorded at paint time (which had no
    /// [`EventCtx`]) on the next event — one event of latency, the same
    /// controlled-component convention the fling clock already relies on.
    fn deliver_pending_scroll(&mut self, ctx: &mut EventCtx) {
        if self.pending_scroll_notify {
            self.pending_scroll_notify = false;
            self.notify_scroll(ctx);
        }
    }

    /// Derive the effective `offset` from the raw drag position, applying
    /// [`OVERSCROLL_RESISTANCE`] to whatever portion is past an edge. Keeping the
    /// raw position separate means the resistance is applied once per frame, not
    /// compounded across successive drag moves.
    fn apply_drag_offset(&mut self) {
        let max = self.max_offset();
        let raw = self.drag_raw;
        self.offset = if raw < 0.0 {
            raw * OVERSCROLL_RESISTANCE
        } else if raw > max {
            max + (raw - max) * OVERSCROLL_RESISTANCE
        } else {
            raw
        };
    }

    /// Advance a release-settle by `dt_ms`, easing the effective `offset` back to
    /// its clamped edge and returning whether it is still animating. Pure and
    /// deterministic (mirrors [`ScrollWidget::tick`]); the paint pump and the
    /// tests both drive it.
    pub fn settle_tick(&mut self, dt_ms: f64) -> bool {
        if !self.settling {
            return false;
        }
        let max = self.max_offset();
        let target = self.offset.clamp(0.0, max);
        let remaining = target - self.offset;
        if remaining.abs() <= SETTLE_STOP_PX {
            self.offset = target;
            self.settling = false;
            self.sync_child_origin();
            return false;
        }
        let retained = SETTLE_DECAY.powf(dt_ms);
        self.offset = target - remaining * retained;
        self.sync_child_origin();
        true
    }

    /// Advance an in-flight fling by `dt_ms`, returning whether it is still
    /// animating. Pure and deterministic — the paint-time pump and the tests
    /// both drive it.
    pub fn tick(&mut self, dt_ms: f64) -> bool {
        let Some(v) = self.fling else {
            return false;
        };
        self.set_offset(self.offset + fling_displacement(v, dt_ms));
        self.sync_child_origin();
        let next_v = fling_decay(v, dt_ms);
        let at_bound = self.offset <= 0.0 || self.offset >= self.max_offset();
        if next_v.abs() < FLING_STOP || at_bound {
            self.fling = None;
            false
        } else {
            self.fling = Some(next_v);
            true
        }
    }

    /// Advance the fling *or* the release-settle by the delta since the last
    /// paint, and signal [`PaintCtx::request_frame`] while either is still running
    /// so the shell keeps scheduling frames (the desktop `ControlFlow::Wait` loop
    /// would otherwise idle). A fling stops once [`ScrollWidget::tick`] brings it
    /// to rest (`|velocity|` below [`FLING_STOP`], or a scroll bound reached); a
    /// settle stops once [`ScrollWidget::settle_tick`] reaches the edge. Because
    /// this path carries no [`EventCtx`], an offset change here records a pending
    /// `on_scroll` notification delivered on the next event.
    fn pump_fling(&mut self, ctx: &mut PaintCtx) {
        if self.fling.is_none() && !self.settling {
            self.last_anim = None;
            return;
        }
        let now = ctx.frame_time();
        let dt = match self.last_anim {
            Some(t) => now.saturating_sub(t).as_secs_f64() * 1000.0,
            None => 0.0,
        };
        self.last_anim = Some(now);
        if dt > 0.0 {
            if self.fling.is_some() {
                self.tick(dt);
            } else {
                self.settle_tick(dt);
            }
            // The offset moved from a non-input source — record a notification the
            // next event delivers (the paint pass has no EventCtx to fire it now).
            self.pending_scroll_notify = true;
        }
        // While either animation is still in flight, ask the shell for another
        // frame to continue it.
        if self.fling.is_some() || self.settling {
            ctx.request_frame();
        }
    }

    fn send_child_cancel(&mut self, ctx: &mut EventCtx, pos: Point) {
        let cancel = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Cancel,
            position: pos,
            button: PointerButton::Primary,
        });
        self.child.event_child(ctx, &cancel);
    }

    /// The event body, parameterised on an explicit timestamp so velocity math
    /// is deterministic in tests; [`Widget::event`] supplies the real clock.
    fn event_at(&mut self, ctx: &mut EventCtx, event: &InputEvent, t_ms: f64) -> EventResult {
        // A fling/settle notification recorded at paint time is delivered on the
        // next event — except a Cancel, which clears it without firing (below).
        if !matches!(
            event,
            InputEvent::Pointer(p) if p.phase == PointerPhase::Cancel
        ) {
            self.deliver_pending_scroll(ctx);
        }
        match event {
            // Focus-routed events (Key/Ime) bypass the scroll gesture machinery
            // and go straight to the child if it holds the recorded focus path.
            InputEvent::Key(_) | InputEvent::Ime(_) => {
                if self.child.is_focused() {
                    self.child.event_child(ctx, event)
                } else {
                    EventResult::Ignored
                }
            }
            InputEvent::Scroll { delta, .. } => {
                let dy = match delta {
                    ScrollDelta::Lines(_, y) => y * WHEEL_LINE_PX,
                    ScrollDelta::Pixels(_, y) => *y,
                };
                // Wheel scrolling stays hard-clamped — no overscroll rubber-band on
                // desktop wheel input.
                self.fling = None;
                self.settling = false;
                self.set_offset(self.offset + dy);
                self.sync_child_origin();
                self.notify_scroll(ctx);
                ctx.request_redraw();
                EventResult::Handled
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    self.scrolling = false;
                    self.down_active = true;
                    self.fling = None;
                    self.settling = false;
                    self.last_anim = None;
                    self.down_start = p.position;
                    self.last_drag = p.position;
                    self.tracker.clear();
                    self.tracker.record(t_ms, p.position.y);
                    ctx.capture_pointer();
                    self.child.event_child(ctx, event);
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    // Without an armed `Down`, this is a hover move (the desktop
                    // shell dispatches `Move` on every cursor motion): never run
                    // the slop/takeover math against a stale `down_start`, just
                    // forward it to the child.
                    if !self.down_active {
                        return self.child.event_child(ctx, event);
                    }
                    self.tracker.record(t_ms, p.position.y);
                    if self.scrolling {
                        let dy = p.position.y - self.last_drag.y;
                        self.last_drag = p.position;
                        // Accumulate the raw drag position (unclamped) and derive
                        // the resisted effective offset — a drag past an edge shows
                        // an iOS-style rubber-band overscroll.
                        self.drag_raw -= dy;
                        self.apply_drag_offset();
                        self.sync_child_origin();
                        self.notify_scroll(ctx);
                        ctx.request_redraw();
                    } else if (p.position.y - self.down_start.y).abs() > TOUCH_SLOP {
                        // Take the gesture over: cancel the child, stop forwarding.
                        self.scrolling = true;
                        self.settling = false;
                        self.last_drag = p.position;
                        // Seed the raw drag position from the current (in-range)
                        // offset so overscroll accrues from here.
                        self.drag_raw = self.offset;
                        self.send_child_cancel(ctx, p.position);
                        self.child.set_active(false);
                        ctx.request_redraw();
                    } else {
                        self.child.event_child(ctx, event);
                    }
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    if self.scrolling {
                        let info = self.scroll_info();
                        // Pull-to-refresh: released past the top trigger fires the
                        // app hook (an Up, so mutating state is allowed).
                        if info.overscroll < -REFRESH_TRIGGER_PX
                            && let Some(cb) = self.on_refresh_release.as_mut()
                        {
                            cb(ctx);
                        }
                        if info.overscroll != 0.0 {
                            // Released while overscrolled: settle back to the edge,
                            // never fling out of range.
                            self.fling = None;
                            self.settling = true;
                            self.last_anim = None;
                        } else {
                            let finger_v = self.tracker.velocity();
                            if finger_v.abs() > FLING_STOP {
                                // Offset moves opposite the finger.
                                self.fling = Some(-finger_v);
                                self.last_anim = None;
                            }
                        }
                        self.notify_scroll(ctx);
                    } else {
                        self.child.event_child(ctx, event);
                    }
                    self.child.set_active(false);
                    self.scrolling = false;
                    self.down_active = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    self.child.event_child(ctx, event);
                    self.child.set_active(false);
                    self.scrolling = false;
                    self.down_active = false;
                    // Cancel never mutates state and never fires a callback: drop
                    // any pending notification and snap an overscrolled surface back
                    // into range (no settle animation, no on_scroll/on_refresh).
                    self.settling = false;
                    self.pending_scroll_notify = false;
                    self.set_offset(self.offset);
                    self.sync_child_origin();
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
        }
    }
}

impl<State: 'static> View<State> for ScrollView<State> {
    type Element = ScrollWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ScrollWidget {
        let mut widget = ScrollWidget::new(crate::build_child(&self.child, ctx));
        widget.on_scroll = self.on_scroll.as_ref().map(crate::erase_callback_arg);
        widget.on_refresh_release = self.on_refresh_release.as_ref().map(crate::erase_callback);
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ScrollWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable — always reinstall the erased adapters.
        element.on_scroll = self.on_scroll.as_ref().map(crate::erase_callback_arg);
        element.on_refresh_release = self.on_refresh_release.as_ref().map(crate::erase_callback);
        crate::rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut ScrollWidget, ctx: &mut BuildCtx<'_>) {
        crate::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for ScrollWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let vw = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        // The child is laid out at the viewport width with unbounded height.
        let child_bc = BoxConstraints::new(Size::new(vw, 0.0), Size::new(vw, f64::INFINITY));
        self.content = self.child.layout_child(ctx, &child_bc);
        let vh = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            self.content.height
        };
        self.viewport = Size::new(vw, vh);
        self.set_offset(self.offset); // re-clamp against new content/viewport
        self.sync_child_origin();
        bc.constrain(self.viewport)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Record the shared frame clock so the between-frames event pass (which
        // carries no clock) has a timestamp for velocity tracking.
        self.last_frame_time = ctx.frame_time();
        self.pump_fling(ctx);
        scene.push_clip(ctx.origin(), ctx.size());
        self.sync_child_origin();
        self.child.paint_child(ctx, scene);
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let t = self.event_time_ms();
        self.event_at(ctx, event, t)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A ScrollView node exposing the vertical scroll offset and its range,
        // wrapping its scrolled content: `semantics_child` translates by the
        // child's origin (which carries `-offset`), so descendant bounds reflect
        // the scrolled position.
        let max_offset = (self.content.height - self.viewport.height).max(0.0);
        ctx.push_container(
            Role::ScrollView,
            |node| {
                node.set_scroll_y(self.offset);
                node.set_scroll_y_min(0.0);
                node.set_scroll_y_max(max_offset);
            },
            |ctx| self.child.semantics_child(ctx),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf;
    use std::any::Any;

    /// Build and lay out a scroll widget over `()` state with a `content_h`-tall
    /// leaf child inside a `vw`×`vh` viewport.
    fn laid_out(vw: f64, vh: f64, content_h: f64) -> ScrollWidget {
        let view: ScrollView<()> = scroll_view(leaf(vw, content_h));
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(vw, vh)));
        w
    }

    fn scroll(y: f64, lines: bool, amount: f64) -> InputEvent {
        let delta = if lines {
            ScrollDelta::Lines(0.0, amount)
        } else {
            ScrollDelta::Pixels(0.0, amount)
        };
        InputEvent::Scroll {
            position: Point::new(10.0, y),
            delta,
        }
    }

    fn ev(phase: PointerPhase, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(10.0, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut ScrollWidget, event: &InputEvent, t_ms: f64) {
        let mut unit = ();
        let state_any: &mut dyn Any = &mut unit;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, w.viewport);
        w.event_at(&mut ctx, event, t_ms);
    }

    #[test]
    fn max_offset_is_content_minus_viewport() {
        let w = laid_out(200.0, 100.0, 1000.0);
        assert_eq!(w.max_offset(), 900.0);
        assert_eq!(w.offset(), 0.0);
    }

    #[test]
    fn wheel_scrolls_and_clamps_with_no_overscroll() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        // 3 lines * 40 px = 120.
        dispatch(&mut w, &scroll(50.0, true, 3.0), 0.0);
        assert_eq!(w.offset(), 120.0);
        // A huge line delta clamps to max_offset (no overscroll past the end).
        dispatch(&mut w, &scroll(50.0, true, 100.0), 0.0);
        assert_eq!(w.offset(), 900.0);
        // Scrolling back past the top clamps to 0.
        dispatch(&mut w, &scroll(50.0, false, -5000.0), 0.0);
        assert_eq!(w.offset(), 0.0);
    }

    #[test]
    fn drag_past_slop_scrolls_the_offset() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        // First move crosses the slop → takeover (no scroll on this move).
        dispatch(&mut w, &ev(PointerPhase::Move, 70.0), 16.0);
        assert_eq!(w.offset(), 0.0);
        assert!(w.scrolling);
        // Next move drags the finger up 30 px → content scrolls down 30 px.
        dispatch(&mut w, &ev(PointerPhase::Move, 40.0), 32.0);
        assert_eq!(w.offset(), 30.0);
    }

    #[test]
    fn takeover_sends_the_child_a_cancel() {
        // A child that records the pointer phases it receives.
        #[derive(Default)]
        struct Rec {
            downs: u32,
            cancels: u32,
        }
        struct Probe;
        struct ProbeW;
        impl View<Rec> for Probe {
            type Element = ProbeW;
            fn build(&self, _c: &mut BuildCtx<'_>) -> ProbeW {
                ProbeW
            }
            fn rebuild(&self, _p: &Self, _e: &mut ProbeW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
                ChangeFlags::NONE
            }
        }
        impl Widget for ProbeW {
            fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.constrain(Size::new(200.0, 1000.0))
            }
            fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
            fn event(&mut self, ctx: &mut EventCtx, e: &InputEvent) -> EventResult {
                if let InputEvent::Pointer(p) = e {
                    let rec = ctx.state_mut::<Rec>();
                    match p.phase {
                        PointerPhase::Down => rec.downs += 1,
                        PointerPhase::Cancel => rec.cancels += 1,
                        _ => {}
                    }
                }
                EventResult::Handled
            }
        }

        let view: ScrollView<Rec> = scroll_view(Probe);
        let mut counter = 0u64;
        let mut w = View::<Rec>::build(&view, &mut BuildCtx::new(&mut counter));
        w.viewport = Size::new(200.0, 100.0);

        let mut state = Rec::default();
        let run = |w: &mut ScrollWidget, state: &mut Rec, e: &InputEvent, t: f64| {
            let sa: &mut dyn Any = state;
            let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(200.0, 100.0));
            w.event_at(&mut ctx, e, t);
        };
        run(&mut w, &mut state, &ev(PointerPhase::Down, 100.0), 0.0);
        // Drag 30 px past the slop → takeover fires a Cancel at the child.
        run(&mut w, &mut state, &ev(PointerPhase::Move, 70.0), 16.0);
        assert_eq!(state.downs, 1);
        assert_eq!(state.cancels, 1);
        // Subsequent scrolling moves are not forwarded to the child.
        run(&mut w, &mut state, &ev(PointerPhase::Move, 50.0), 32.0);
        assert_eq!(state.cancels, 1);
    }

    /// A recording child probe, shared by the takeover/hover tests.
    #[derive(Default)]
    struct Rec {
        downs: u32,
        cancels: u32,
        moves: u32,
    }
    struct Probe;
    struct ProbeW;
    impl View<Rec> for Probe {
        type Element = ProbeW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> ProbeW {
            ProbeW
        }
        fn rebuild(&self, _p: &Self, _e: &mut ProbeW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for ProbeW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(200.0, 1000.0))
        }
        fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, e: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = e {
                let rec = ctx.state_mut::<Rec>();
                match p.phase {
                    PointerPhase::Down => rec.downs += 1,
                    PointerPhase::Cancel => rec.cancels += 1,
                    PointerPhase::Move => rec.moves += 1,
                    _ => {}
                }
            }
            EventResult::Ignored
        }
    }

    fn probe_scroll() -> ScrollWidget {
        let view: ScrollView<Rec> = scroll_view(Probe);
        let mut counter = 0u64;
        let mut w = View::<Rec>::build(&view, &mut BuildCtx::new(&mut counter));
        w.viewport = Size::new(200.0, 100.0);
        w
    }

    fn run_rec(w: &mut ScrollWidget, state: &mut Rec, e: &InputEvent, t: f64) -> bool {
        let sa: &mut dyn Any = state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(200.0, 100.0));
        w.event_at(&mut ctx, e, t);
        ctx.needs_redraw()
    }

    #[test]
    fn hover_move_without_down_never_scrolls_or_cancels_child() {
        let mut w = probe_scroll();
        let mut state = Rec::default();
        // A cursor drifting over the list with no prior Down: no takeover, no
        // Cancel to the child, no offset change, no self redraw request.
        let redraw = run_rec(&mut w, &mut state, &ev(PointerPhase::Move, 40.0), 16.0);
        assert!(!w.scrolling, "hover must not enter scrolling");
        assert!(!w.down_active);
        assert_eq!(w.offset(), 0.0, "hover must not move the offset");
        assert_eq!(state.cancels, 0, "hover must not cancel the child");
        assert!(!redraw, "hover must not request a redraw");
        // The hover move is forwarded to the child (which ignores it).
        assert_eq!(state.moves, 1);
    }

    #[test]
    fn cancel_clears_down_active() {
        let mut w = probe_scroll();
        let mut state = Rec::default();
        run_rec(&mut w, &mut state, &ev(PointerPhase::Down, 100.0), 0.0);
        assert!(w.down_active);
        run_rec(&mut w, &mut state, &ev(PointerPhase::Cancel, 100.0), 16.0);
        assert!(!w.down_active, "Cancel disarms the gesture");
        assert!(!w.scrolling);
        // A subsequent hover Move must not run the takeover math.
        run_rec(&mut w, &mut state, &ev(PointerPhase::Move, 20.0), 32.0);
        assert!(!w.scrolling, "hover after Cancel must not take over");
        assert_eq!(w.offset(), 0.0);
    }

    #[test]
    fn fling_after_release_decays_and_clamps() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 75.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 50.0), 32.0); // scroll, builds velocity
        dispatch(&mut w, &ev(PointerPhase::Up, 50.0), 32.0);
        assert!(w.is_flinging(), "release with velocity starts a fling");
        // Integrate to completion.
        let mut steps = 0;
        while w.tick(16.0) {
            steps += 1;
            assert!(steps < 100_000, "fling failed to terminate");
        }
        assert!(!w.is_flinging());
        assert!(w.offset() >= 0.0 && w.offset() <= w.max_offset());
    }

    #[test]
    fn pump_fling_signals_needs_frame_until_at_rest() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        // Drive a release-with-velocity to start a fling (deterministic seam).
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 75.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 50.0), 32.0); // build velocity
        dispatch(&mut w, &ev(PointerPhase::Up, 50.0), 32.0);
        assert!(w.is_flinging(), "release with velocity starts a fling");

        // A paint-time pump while flinging asks for another frame.
        let mut ctx = PaintCtx::new(Point::ZERO, w.viewport);
        w.pump_fling(&mut ctx);
        assert!(
            ctx.needs_frame(),
            "an in-flight fling requests continuation"
        );
        assert!(w.is_flinging());

        // Integrate the fling to rest via the deterministic tick seam.
        while w.tick(16.0) {}
        assert!(!w.is_flinging());

        // At rest, the pump no longer signals — the shell can idle again.
        let mut ctx_rest = PaintCtx::new(Point::ZERO, w.viewport);
        w.pump_fling(&mut ctx_rest);
        assert!(
            !ctx_rest.needs_frame(),
            "a fling at rest stops requesting frames"
        );
    }

    #[test]
    fn tick_without_a_fling_is_a_noop() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        assert!(!w.tick(16.0));
        assert_eq!(w.offset(), 0.0);
    }

    /// A no-op paint sink for the RenderRoot clock test.
    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: peniko::Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    fn scroll_widget(root: &frust_core::RenderRoot<(), ScrollView<()>>) -> &ScrollWidget {
        let id = root.root_id().expect("root built");
        (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
            .downcast_ref::<ScrollWidget>()
            .expect("root is a ScrollWidget")
    }

    #[test]
    fn fling_advances_from_injected_paint_frame_time() {
        // End-to-end through the real paint path (task 07 clock retrofit): the
        // event pass reads the last painted frame time for velocity tracking, and
        // the fling pump advances off the injected `RenderRoot::paint` frame time
        // — no wall clock anywhere. Paints are interleaved with the drag so the
        // velocity tracker sees distinct (paint-clock) timestamps.
        use frust_core::{FrameTime, RenderRoot};

        fn logic(_: &mut ()) -> ScrollView<()> {
            scroll_view(leaf(200.0, 1000.0))
        }
        let mut root: RenderRoot<(), ScrollView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 100.0));

        let ft = |ms: f64| FrameTime::from_nanos((ms * 1_000_000.0) as u64);
        let mut sink = NullScene;
        root.paint(&mut sink, ft(0.0));
        root.event(&mut state, &ev(PointerPhase::Down, 100.0));
        root.paint(&mut sink, ft(16.0));
        root.event(&mut state, &ev(PointerPhase::Move, 75.0)); // crosses slop → takeover
        root.paint(&mut sink, ft(32.0));
        root.event(&mut state, &ev(PointerPhase::Move, 50.0)); // scroll, builds velocity
        root.event(&mut state, &ev(PointerPhase::Up, 50.0)); // release → fling

        assert!(
            scroll_widget(&root).is_flinging(),
            "release with paint-clock velocity starts a fling"
        );
        let before = scroll_widget(&root).offset();

        // Advancing frame times drive the fling: the first paint seeds the fling
        // clock (zero delta), the next advances the offset.
        root.paint(&mut sink, ft(48.0));
        root.paint(&mut sink, ft(64.0));
        assert!(
            scroll_widget(&root).offset() > before,
            "the fling advanced from the injected paint frame time"
        );
    }

    #[test]
    fn offset_reclamps_when_content_shrinks() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        dispatch(&mut w, &scroll(50.0, false, 800.0), 0.0);
        assert_eq!(w.offset(), 800.0);
        // Content shrinks to just above the viewport → max_offset drops to 50,
        // and a re-clamp (what layout does) pulls the stale offset back in range.
        w.content = Size::new(200.0, 150.0);
        w.set_offset(w.offset);
        assert_eq!(w.max_offset(), 50.0);
        assert_eq!(w.offset(), 50.0);
    }

    // --- Overscroll (pull-to-refresh seam) ---

    #[test]
    fn drag_past_top_overscrolls_with_resistance_then_settles_back() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        // Down, then a drag downward crossing the slop takes the gesture over.
        dispatch(&mut w, &ev(PointerPhase::Down, 50.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 90.0), 16.0); // 40px > slop → takeover
        assert!(w.scrolling);
        assert_eq!(w.offset(), 0.0, "the takeover move does not itself scroll");
        // Drag 20px further down past the already-at-top edge → resisted overscroll.
        dispatch(&mut w, &ev(PointerPhase::Move, 110.0), 32.0);
        assert!(w.offset() < 0.0, "a drag past the top overscrolls negative");
        assert_eq!(
            w.offset(),
            -10.0,
            "overscroll is the raw excess (-20) * OVERSCROLL_RESISTANCE (0.5)"
        );
        // Release → a settle animation, not a fling; it returns to the edge.
        dispatch(&mut w, &ev(PointerPhase::Up, 110.0), 48.0);
        assert!(
            !w.is_flinging(),
            "an overscrolled release settles, never flings"
        );
        let mut steps = 0;
        while w.settle_tick(16.0) {
            steps += 1;
            assert!(steps < 10_000, "settle failed to terminate");
        }
        assert_eq!(
            w.offset(),
            0.0,
            "the surface settles back to the clamped edge"
        );
    }

    #[test]
    fn wheel_never_overscrolls_past_top() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        // A large negative wheel delta at the top stays hard-clamped at 0 — no
        // rubber-band on wheel input.
        dispatch(&mut w, &scroll(50.0, false, -5000.0), 0.0);
        assert_eq!(w.offset(), 0.0);
        assert!(!w.settling, "wheel input starts no settle animation");
    }

    /// A state that records every `ScrollInfo` its `on_scroll` observes.
    #[derive(Default)]
    struct ScrollLog {
        infos: Vec<ScrollInfo>,
        refreshes: u32,
    }

    /// A fixed-size content view generic over the state type (the shared `leaf`
    /// fixture is `View<()>` only), so a scroll view can wrap it over `ScrollLog`.
    struct Content(Size);
    struct ContentW(Size);
    impl<S: 'static> View<S> for Content {
        type Element = ContentW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> ContentW {
            ContentW(self.0)
        }
        fn rebuild(&self, _p: &Self, _e: &mut ContentW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for ContentW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
    }

    /// Build+lay out a scroll widget over `ScrollLog` state with the given
    /// callbacks installed.
    fn observed(with_refresh: bool) -> ScrollWidget {
        let mut view: ScrollView<ScrollLog> = scroll_view(Content(Size::new(200.0, 1000.0)))
            .on_scroll(|s: &mut ScrollLog, info| s.infos.push(info));
        if with_refresh {
            view = view.on_refresh_release(|s: &mut ScrollLog| s.refreshes += 1);
        }
        let mut counter = 0u64;
        let mut w = View::<ScrollLog>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));
        w
    }

    fn run_log(w: &mut ScrollWidget, state: &mut ScrollLog, e: &InputEvent, t: f64) {
        let sa: &mut dyn Any = state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, w.viewport);
        w.event_at(&mut ctx, e, t);
    }

    #[test]
    fn on_scroll_observes_drag_deltas() {
        let mut w = observed(false);
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 100.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 70.0), 16.0); // takeover
        // Two scrolling drags upward move the content down.
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 40.0), 32.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 10.0), 48.0);
        assert!(!state.infos.is_empty(), "on_scroll fired on the drag");
        let last = state.infos.last().unwrap();
        assert!(
            last.offset > 0.0,
            "the observed offset grew as content scrolled"
        );
        assert_eq!(last.overscroll, 0.0, "an in-range drag has no overscroll");
        assert_eq!(last.max_offset, 900.0);
    }

    #[test]
    fn on_refresh_release_fires_only_past_trigger_and_only_on_release() {
        // A small pull (under the trigger) does not fire on release.
        let mut w = observed(true);
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 50.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 90.0), 16.0); // takeover
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 180.0), 32.0); // raw -90 → -45
        assert_eq!(w.offset(), -45.0, "under the trigger (|-45| < 64)");
        run_log(&mut w, &mut state, &ev(PointerPhase::Up, 180.0), 48.0);
        assert_eq!(
            state.refreshes, 0,
            "release under the trigger does not refresh"
        );

        // A large pull past the trigger fires exactly once, on release.
        let mut w = observed(true);
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 50.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 90.0), 16.0); // takeover
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 290.0), 32.0); // raw -200 → -100
        assert_eq!(w.offset(), -100.0, "past the trigger (|-100| > 64)");
        assert_eq!(state.refreshes, 0, "no fire before release");
        run_log(&mut w, &mut state, &ev(PointerPhase::Up, 290.0), 48.0);
        assert_eq!(state.refreshes, 1, "release past the trigger fires once");
    }

    #[test]
    fn cancel_during_overscroll_never_fires_refresh_and_snaps_back() {
        let mut w = observed(true);
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 50.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 90.0), 16.0); // takeover
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 290.0), 32.0); // past trigger
        assert_eq!(w.offset(), -100.0);
        // A Cancel (gesture steal) must not fire on_refresh_release and snaps the
        // overscroll away with no settle animation.
        run_log(&mut w, &mut state, &ev(PointerPhase::Cancel, 290.0), 48.0);
        assert_eq!(
            state.refreshes, 0,
            "Cancel never fires the refresh callback"
        );
        assert_eq!(w.offset(), 0.0, "Cancel snaps the surface back into range");
        assert!(!w.settling);
    }
}
