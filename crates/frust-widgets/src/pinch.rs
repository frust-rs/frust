//! Touch pinch recognition: [`PinchRecognizer`], a pure two-contact state
//! machine, and [`pinch_detector`], a transparent wrapper view that feeds it.
//!
//! The recogniser reduces a gesture's contacts to the same
//! [`ScaleEvent`]/[`ScalePhase`] stream a desktop shell's ctrl/⌘+wheel and
//! trackpad-pinch mapping produce, so a consumer reacts to every scale source
//! through one type.
//!
//! # Recognition
//!
//! Contacts are fed as `(PointerId, PointerEvent, time_ms)` triples. The
//! **pair** is the first two tracked contacts, in `Down` order; any further
//! contact is tracked but contributes nothing until a pair member lifts.
//!
//! - **One contact is never a pinch.** Every event of a single-contact gesture
//!   returns `None`, so the owner's drag/scroll logic proceeds untouched.
//! - **Begin** once a pair exists and the inter-contact distance has changed by
//!   more than [`PINCH_SLOP`] from where it stood when the pair formed. Begin
//!   carries `scale_delta == 1.0`: the slop is consumed, so the content does not
//!   jump by the slop's worth at recognition.
//! - **Update** on every pair-member move after that: `scale_delta` is the
//!   multiplicative change in inter-contact distance since the previous event,
//!   `focal` the pair's midpoint, `velocity` the smoothed rate (below).
//! - **End** when a pair member's `Up`/`Cancel` arrives. End carries
//!   `scale_delta == 1.0`; an `Up` reports the release velocity (or `0.0` when the
//!   last sample is older than [`VELOCITY_WINDOW_MS`]), a `Cancel` always `0.0`.
//!   With one contact still down nothing further is a pinch, so the owner may
//!   carry on with a pan; a fresh second contact can begin a new pinch.
//!
//! The **first** contact of a gesture (the one that went down with nothing
//! tracked) mirrors the root's claimant: its `Up`/`Cancel` ends the pinch *and*
//! drops every other tracked contact, because once the claimant's capture ends
//! the root stops delivering the others (`InputEvent::PointerContact`'s
//! multi-contact contract, `frust_core::event`).
//!
//! # Velocity
//!
//! The rate is logarithmic: an instantaneous sample is
//! `ln(scale change) / elapsed seconds` (positive while spreading, negative while
//! pinching, and symmetric — doubling then halving sums to zero), smoothed by an
//! exponential moving average over roughly the last [`PINCH_VELOCITY_SAMPLES`]
//! samples. A consumer extrapolating with inertia scales by `exp(velocity · t)`.
//! Several events sharing one timestamp (the event pass carries no clock; the
//! wrapper stamps events with the last painted frame time) accumulate into the
//! next sample rather than dividing by zero.
//!
//! # The wrapper
//!
//! [`pinch_detector`] captures the pointer **and** opts into the gesture's other
//! contacts ([`EventCtx::capture_contacts`]) on the first primary `Down`, which is
//! what makes the root route a second finger to it at all. The first contact's
//! events are forwarded to the child unchanged, so a single-finger drag or scroll
//! inside it works as if the wrapper were absent; the other contacts are consumed
//! by the recogniser and never reach the child. Once a pinch begins the child's
//! gesture is **stolen**: on the first-contact event at or after Begin the child
//! receives a synthesized `Cancel` (exactly what a platform gesture steal is) and
//! nothing more of that gesture. The steal waits for a first-contact event
//! because only that pass may clear the child's recorded capture path.
//!
//! `on_scale` never runs from a `Cancel` (the Cancel-never-mutates-state
//! convention, `docs/CODE_STANDARDS.md`), so a cancelled pinch delivers no End;
//! a consumer simply sees the next Begin. An [`InputEvent::Scale`] from a
//! desktop source is offered to the child first and reaches `on_scale` only when
//! the child ignores it, so one callback covers touch and desktop alike. Nesting
//! two detectors is unsupported: the outer one consumes the extra contacts.
//!
//! # Surviving an enclosing scroll surface
//!
//! The detector always captures contacts on the claimant's `Down`, whether or
//! not that `Down` sits inside a `ScrollView`/`ListView` — so a second finger
//! can start a pinch over content the claimant's own finger is dragging. The
//! claimant's own `Move`s still take the ordinary captured path up through
//! every container on it, which means the enclosing surface's slop/takeover
//! logic still runs on them. To stop that surface stealing the claimant's
//! finger out from under a live pinch, the detector captures
//! [`crate::scroll::ambient_scroll_veto`]'s cell on the claiming `Down` (while
//! it is still ambient) and raises it for as long as a second contact is
//! tracked, clearing it the moment the pair breaks back down to one
//! (`PinchDetectorWidget::sync_scroll_veto`) — see `crate::scroll`'s module
//! docs' *Multi-contact veto*. Outside a scroll surface this is a no-op: the
//! ambient cell is simply absent.

use frust_core::event::{PointerId, ScaleEvent, ScalePhase};
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerEvent, PointerPhase, SemanticsCtx, TOUCH_SLOP,
    VELOCITY_WINDOW_MS, View, Widget, any,
};
use kurbo::{Point, Size};
use std::cell::Cell;
use std::rc::Rc;

use crate::authoring::presses;
use crate::scroll::ambient_scroll_veto;

/// How far (logical px) the distance between the two pinch contacts must change
/// from its value when the pair formed before the pinch begins.
///
/// **Community-approximate**: neither platform publishes one number for touch.
/// Android's `ScaleGestureDetector` uses a span slop of twice its touch slop
/// (≈16dp) and Flutter's scale recogniser a span slop of `kPanSlop` (36 logical
/// px); this reuses [`TOUCH_SLOP`] (18 logical px), inside that range, so the
/// input thresholds keep their single source in `frust_core::input`.
pub const PINCH_SLOP: f64 = TOUCH_SLOP;

/// The window, in samples, of the exponential moving average smoothing a pinch's
/// velocity: the smoothing factor is `2 / (N + 1)` (0.4 for `N = 4`), the usual
/// N-sample EMA weighting.
///
/// **Community-approximate**: a hand-tuned value — a few frames of history damps
/// a jittery two-finger sample without lagging a deliberate flick.
pub const PINCH_VELOCITY_SAMPLES: f64 = 4.0;

/// The most contacts one [`PinchRecognizer`] tracks; further `Down`s are ignored
/// until a tracked contact lifts.
pub const MAX_PINCH_CONTACTS: usize = 10;

/// The smallest inter-contact distance (logical px) a scale ratio is computed
/// from. Two contacts reported at the same point would otherwise produce a
/// `0.0` delta, which a consumer multiplying its running scale could never
/// recover from.
const MIN_SPAN: f64 = 1.0;

/// Where the recogniser stands with its current pair.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Fewer than two contacts are tracked.
    Idle,
    /// A pair exists; waiting for its distance to leave the slop.
    Armed { start_distance: f64 },
    /// A pinch is in progress.
    Active { last_distance: f64 },
}

/// A pure two-contact pinch recogniser. See the [module docs](self).
///
/// Feed every contact's event through [`handle`](Self::handle); positions must
/// share one coordinate space (every contact routed down one captured path
/// does).
#[derive(Debug)]
pub struct PinchRecognizer {
    /// Tracked contacts in `Down` order; the first two are the pair.
    contacts: Vec<(PointerId, Point)>,
    phase: Phase,
    /// Smoothed logarithmic scale rate, per second.
    velocity: f64,
    /// Samples folded into `velocity` since the pinch began (`0` seeds the
    /// average with the first sample instead of decaying it from zero).
    samples: u32,
    /// Timestamp of the last velocity sample.
    last_sample_ms: f64,
    /// Log scale accumulated since `last_sample_ms` by events sharing it.
    pending_log: f64,
}

impl Default for PinchRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl PinchRecognizer {
    /// A recogniser tracking no contacts.
    pub fn new() -> Self {
        Self {
            contacts: Vec::with_capacity(MAX_PINCH_CONTACTS),
            phase: Phase::Idle,
            velocity: 0.0,
            samples: 0,
            last_sample_ms: 0.0,
            pending_log: 0.0,
        }
    }

    /// Whether a pinch is in progress (between its Begin and its End).
    pub fn is_pinching(&self) -> bool {
        matches!(self.phase, Phase::Active { .. })
    }

    /// How many contacts are currently tracked.
    pub fn contact_count(&self) -> usize {
        self.contacts.len()
    }

    /// Forget every contact without emitting anything — for a gesture whose end
    /// the owner learns of some other way (a teardown, a fresh claimant `Down`).
    pub fn reset(&mut self) {
        self.contacts.clear();
        self.phase = Phase::Idle;
        self.reset_velocity(0.0);
    }

    /// Feed one contact's event, stamped `time_ms` (any monotonic millisecond
    /// clock). Returns the scale event it produces, if any — always `None` while
    /// fewer than two contacts are down.
    pub fn handle(
        &mut self,
        id: PointerId,
        event: &PointerEvent,
        time_ms: f64,
    ) -> Option<ScaleEvent> {
        let index = self.contacts.iter().position(|(c, _)| *c == id);
        match event.phase {
            PointerPhase::Down => match index {
                // A repeated `Down` for a tracked contact is a move.
                Some(i) => self.moved(i, event.position, time_ms),
                None => {
                    if self.contacts.len() < MAX_PINCH_CONTACTS {
                        self.contacts.push((id, event.position));
                        if self.contacts.len() == 2 {
                            self.arm();
                        }
                    }
                    None
                }
            },
            PointerPhase::Move => self.moved(index?, event.position, time_ms),
            PointerPhase::Up | PointerPhase::Cancel => {
                let i = index?;
                self.contacts[i].1 = event.position;
                let cancelled = event.phase == PointerPhase::Cancel;
                let end = if i < 2 {
                    self.end(time_ms, cancelled)
                } else {
                    None
                };
                if i == 0 {
                    // The claimant left: the others are no longer delivered.
                    self.contacts.clear();
                    self.phase = Phase::Idle;
                } else {
                    self.contacts.remove(i);
                    if i < 2 {
                        if self.contacts.len() >= 2 {
                            self.arm();
                        } else {
                            self.phase = Phase::Idle;
                        }
                    }
                }
                end
            }
        }
    }

    /// The pair's distance and midpoint. Only meaningful with two contacts.
    fn pair(&self) -> (f64, Point) {
        let (a, b) = (self.contacts[0].1, self.contacts[1].1);
        ((b - a).hypot(), a.midpoint(b))
    }

    /// Start a fresh slop measurement for the current pair.
    fn arm(&mut self) {
        let (distance, _) = self.pair();
        self.phase = Phase::Armed {
            start_distance: distance,
        };
    }

    fn reset_velocity(&mut self, time_ms: f64) {
        self.velocity = 0.0;
        self.samples = 0;
        self.last_sample_ms = time_ms;
        self.pending_log = 0.0;
    }

    /// Fold `log_delta` into the smoothed rate at `time_ms`.
    fn sample_velocity(&mut self, log_delta: f64, time_ms: f64) {
        self.pending_log += log_delta;
        let dt_ms = time_ms - self.last_sample_ms;
        if dt_ms <= 0.0 {
            // Same timestamp as the last sample: accumulate until time moves.
            return;
        }
        if dt_ms > VELOCITY_WINDOW_MS {
            // A pause longer than the window says nothing about the rate now.
            self.samples = 0;
        }
        let instant = self.pending_log / (dt_ms / 1000.0);
        self.velocity = if self.samples == 0 {
            instant
        } else {
            let alpha = 2.0 / (PINCH_VELOCITY_SAMPLES + 1.0);
            alpha * instant + (1.0 - alpha) * self.velocity
        };
        self.samples = self.samples.saturating_add(1);
        self.last_sample_ms = time_ms;
        self.pending_log = 0.0;
    }

    /// Contact `i` moved to `position`.
    fn moved(&mut self, i: usize, position: Point, time_ms: f64) -> Option<ScaleEvent> {
        self.contacts[i].1 = position;
        if i >= 2 || self.contacts.len() < 2 {
            return None;
        }
        let (distance, focal) = self.pair();
        match self.phase {
            Phase::Idle => None,
            Phase::Armed { start_distance } => {
                if (distance - start_distance).abs() <= PINCH_SLOP {
                    return None;
                }
                self.phase = Phase::Active {
                    last_distance: distance,
                };
                self.reset_velocity(time_ms);
                Some(ScaleEvent {
                    phase: ScalePhase::Begin,
                    scale_delta: 1.0,
                    focal,
                    velocity: 0.0,
                })
            }
            Phase::Active { last_distance } => {
                let scale_delta = distance.max(MIN_SPAN) / last_distance.max(MIN_SPAN);
                self.phase = Phase::Active {
                    last_distance: distance,
                };
                self.sample_velocity(scale_delta.ln(), time_ms);
                Some(ScaleEvent {
                    phase: ScalePhase::Update,
                    scale_delta,
                    focal,
                    velocity: self.velocity,
                })
            }
        }
    }

    /// A pair member lifted (or was cancelled): End an active pinch.
    fn end(&mut self, time_ms: f64, cancelled: bool) -> Option<ScaleEvent> {
        if !self.is_pinching() || self.contacts.len() < 2 {
            return None;
        }
        let (_, focal) = self.pair();
        let stale = time_ms - self.last_sample_ms > VELOCITY_WINDOW_MS;
        let velocity = if cancelled || stale || self.samples == 0 {
            0.0
        } else {
            self.velocity
        };
        self.phase = Phase::Idle;
        Some(ScaleEvent {
            phase: ScalePhase::End,
            scale_delta: 1.0,
            focal,
            velocity,
        })
    }
}

/// A declarative pinch wrapper. See the [module docs](self#the-wrapper).
pub struct PinchDetectorView<State: 'static> {
    child: AnyView<State>,
    on_scale: Option<crate::authoring::TypedArgCallback<State, ScaleEvent>>,
}

/// Wrap `child` in a pinch detector (inert until
/// [`on_scale`](PinchDetectorView::on_scale) is attached, apart from capturing
/// the gesture it would recognise).
pub fn pinch_detector<State: 'static, V: View<State>>(child: V) -> PinchDetectorView<State> {
    PinchDetectorView {
        child: any(child),
        on_scale: None,
    }
}

impl<State: 'static> PinchDetectorView<State> {
    /// Receive every Begin/Update/End of a two-finger pinch over the child, and
    /// every desktop [`InputEvent::Scale`] the child leaves unhandled.
    pub fn on_scale<F: Fn(&mut State, ScaleEvent) + 'static>(mut self, on_scale: F) -> Self {
        self.on_scale = Some(std::rc::Rc::new(on_scale));
        self
    }
}

/// Where the child stands in the current gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChildRoute {
    /// The first contact's events reach the child.
    Forwarding,
    /// A pinch began; the child is cancelled on the next first-contact event.
    StealPending,
    /// The child was cancelled; the rest of the gesture is withheld from it.
    Stolen,
}

/// The retained widget for a [`PinchDetectorView`].
pub struct PinchDetectorWidget {
    child: ChildPod,
    recognizer: PinchRecognizer,
    on_scale: Option<crate::authoring::ErasedArgCallback<ScaleEvent>>,
    /// The contact whose `Down` this widget captured on, while its gesture lives.
    claimant: Option<PointerId>,
    route: ChildRoute,
    /// The last painted frame time in ms — the event pass's timestamp source.
    last_frame_ms: f64,
    /// The enclosing scroll surface's live multi-contact veto, captured from
    /// [`ambient_scroll_veto`] on the claiming `Down` while it is still
    /// reachable — `None` outside a scroll surface, which simply has nothing
    /// to raise (see the [module docs](self#surviving-an-enclosing-scroll-surface)).
    scroll_veto: Option<Rc<Cell<bool>>>,
}

impl<State: 'static> View<State> for PinchDetectorView<State> {
    type Element = PinchDetectorWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PinchDetectorWidget {
        PinchDetectorWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            recognizer: PinchRecognizer::new(),
            on_scale: self
                .on_scale
                .as_ref()
                .map(crate::authoring::erase_callback_arg),
            claimant: None,
            route: ChildRoute::Forwarding,
            last_frame_ms: 0.0,
            scroll_veto: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PinchDetectorWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_scale = self
            .on_scale
            .as_ref()
            .map(crate::authoring::erase_callback_arg);
        crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut PinchDetectorWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl PinchDetectorWidget {
    /// Hand a recognised scale event to `on_scale`. Never called for a `Cancel`.
    fn fire(&mut self, ctx: &mut EventCtx, scale: ScaleEvent) {
        if let Some(cb) = self.on_scale.as_mut() {
            cb(ctx, scale);
            ctx.request_redraw();
        }
    }

    /// Feed one contact to the recogniser, firing what it produces (except from
    /// a `Cancel`) and marking the child for a steal when a pinch begins.
    fn recognize(&mut self, ctx: &mut EventCtx, id: PointerId, p: &PointerEvent) {
        let Some(scale) = self.recognizer.handle(id, p, self.last_frame_ms) else {
            return;
        };
        if scale.phase == ScalePhase::Begin && self.route == ChildRoute::Forwarding {
            self.route = ChildRoute::StealPending;
        }
        if p.phase != PointerPhase::Cancel {
            self.fire(ctx, scale);
        }
    }

    /// Raise or clear the captured [`PinchDetectorWidget::scroll_veto`] to
    /// match whether the recogniser is tracking more than one contact right
    /// now — called after every [`PinchRecognizer::handle`], so an enclosing
    /// scroll surface sees the flip before its own next `Move` decides
    /// whether to take the claimant's finger over (`crate::scroll`'s module
    /// docs' *Multi-contact veto*).
    fn sync_scroll_veto(&self) {
        if let Some(veto) = &self.scroll_veto {
            veto.set(self.recognizer.contact_count() >= 2);
        }
    }

    /// Route a first-contact event to the child according to [`ChildRoute`].
    fn route_claimant(&mut self, ctx: &mut EventCtx, event: &InputEvent, p: &PointerEvent) {
        let ends = matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel);
        match self.route {
            ChildRoute::Forwarding => {
                self.child.event_child(ctx, event);
                if ends {
                    self.child.set_active(false);
                }
            }
            ChildRoute::StealPending => {
                let cancel = InputEvent::Pointer(PointerEvent {
                    phase: PointerPhase::Cancel,
                    ..*p
                });
                self.child.event_child(ctx, &cancel);
                self.child.set_active(false);
                self.route = ChildRoute::Stolen;
            }
            ChildRoute::Stolen => {}
        }
        if ends {
            self.claimant = None;
            self.route = ChildRoute::Forwarding;
            self.recognizer.reset();
            self.scroll_veto = None;
        }
    }
}

impl Widget for PinchDetectorWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The event pass carries no clock; stamp velocity samples with the last
        // painted frame instead (the `ScrollView` precedent).
        self.last_frame_ms = ctx.frame_time().as_secs_f64() * 1000.0;
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.child.event_child(ctx, event);
            return EventResult::Ignored;
        }
        match event {
            InputEvent::Pointer(p) => {
                let id = ctx.pointer_id();
                let starts = p.phase == PointerPhase::Down && presses(p);
                if self.claimant.is_none() || (starts && self.claimant == Some(id)) {
                    if !starts {
                        // Hover moves, non-primary presses, stray releases.
                        return self.child.event_child(ctx, event);
                    }
                    // A fresh gesture: claim it and every contact that joins it.
                    self.recognizer.reset();
                    self.claimant = Some(id);
                    self.route = ChildRoute::Forwarding;
                    // Capture the enclosing scroll surface's live veto now,
                    // while the ambient cell from its `Down` forward is still
                    // reachable — a `None` snapshot here would be a surface's
                    // own `Down`-time claim read too early to see a second
                    // contact that has not arrived yet (see the module docs'
                    // *Surviving an enclosing scroll surface*).
                    self.scroll_veto = ambient_scroll_veto();
                    ctx.capture_pointer();
                    ctx.capture_contacts();
                    self.recognize(ctx, id, p);
                    self.sync_scroll_veto();
                    self.child.event_child(ctx, event);
                    return EventResult::Handled;
                }
                self.recognize(ctx, id, p);
                self.sync_scroll_veto();
                if self.claimant == Some(id) {
                    self.route_claimant(ctx, event, p);
                }
                // Another contact is the recogniser's alone.
                EventResult::Handled
            }
            InputEvent::Scale(scale) => {
                let result = self.child.event_child(ctx, event);
                if result == EventResult::Ignored && self.on_scale.is_some() {
                    self.fire(ctx, *scale);
                    return EventResult::Handled;
                }
                result
            }
            _ => self.child.event_child(ctx, event),
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent wrapper: forward to the single child.
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{FrameTime, PointerButton, RenderRoot};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn pe(phase: PointerPhase, x: f64, y: f64) -> PointerEvent {
        PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        }
    }

    const T0: PointerId = PointerId::touch(0);
    const T1: PointerId = PointerId::touch(1);
    const T2: PointerId = PointerId::touch(2);

    /// A recogniser with two contacts down 20 px apart on y = 50, at t = 0.
    fn paired() -> PinchRecognizer {
        let mut r = PinchRecognizer::new();
        assert_eq!(r.handle(T0, &pe(PointerPhase::Down, 40.0, 50.0), 0.0), None);
        assert_eq!(r.handle(T1, &pe(PointerPhase::Down, 60.0, 50.0), 0.0), None);
        r
    }

    /// `paired()` spread past the slop: begun at distance 40, focal (60, 50).
    fn begun() -> PinchRecognizer {
        let mut r = paired();
        let begin = r
            .handle(T1, &pe(PointerPhase::Move, 80.0, 50.0), 16.0)
            .expect("past the slop");
        assert_eq!(begin.phase, ScalePhase::Begin);
        r
    }

    #[test]
    fn two_finger_spread_begins_updates_and_ends_at_the_midpoint() {
        let mut r = paired();
        let begin = r
            .handle(T1, &pe(PointerPhase::Move, 80.0, 50.0), 16.0)
            .unwrap();
        assert_eq!(begin.phase, ScalePhase::Begin);
        assert_eq!(begin.scale_delta, 1.0, "the slop is consumed, not jumped");
        assert_eq!(begin.focal, Point::new(60.0, 50.0));
        assert!(r.is_pinching());

        let update = r
            .handle(T1, &pe(PointerPhase::Move, 100.0, 50.0), 32.0)
            .unwrap();
        assert_eq!(update.phase, ScalePhase::Update);
        assert!((update.scale_delta - 1.5).abs() < 1e-12, "40 → 60 px");
        assert_eq!(update.focal, Point::new(70.0, 50.0));

        let end = r
            .handle(T1, &pe(PointerPhase::Up, 100.0, 50.0), 48.0)
            .unwrap();
        assert_eq!(end.phase, ScalePhase::End);
        assert_eq!(end.scale_delta, 1.0);
        assert_eq!(end.focal, Point::new(70.0, 50.0));
        assert!(!r.is_pinching());
    }

    #[test]
    fn spreading_the_first_contact_counts_too() {
        let mut r = paired();
        let begin = r
            .handle(T0, &pe(PointerPhase::Move, 20.0, 50.0), 16.0)
            .unwrap();
        assert_eq!(begin.phase, ScalePhase::Begin);
        assert_eq!(begin.focal, Point::new(40.0, 50.0));
    }

    #[test]
    fn constant_distance_drag_reports_unit_scale_and_a_moving_focal() {
        let mut r = begun(); // T0 at 40, T1 at 80
        let mut product = 1.0;
        let mut last_focal_x = 60.0;
        let mut t = 16.0;
        for step in 1..=10 {
            t += 8.0;
            let x = 40.0 + step as f64;
            // Each finger steps 1 px to the right in turn.
            for (id, pos) in [(T1, x + 40.0), (T0, x)] {
                let u = r.handle(id, &pe(PointerPhase::Move, pos, 50.0), t).unwrap();
                assert_eq!(u.phase, ScalePhase::Update);
                assert!((u.scale_delta - 1.0).abs() < 0.03, "{u:?}");
                assert!(u.focal.x > last_focal_x, "focal moves with the drag");
                last_focal_x = u.focal.x;
                product *= u.scale_delta;
            }
        }
        assert!(
            (product - 1.0).abs() < 1e-9,
            "net scale is 1, got {product}"
        );
        assert_eq!(last_focal_x, 70.0);
    }

    #[test]
    fn a_single_finger_never_pinches() {
        let mut r = PinchRecognizer::new();
        assert_eq!(r.handle(T0, &pe(PointerPhase::Down, 10.0, 10.0), 0.0), None);
        for i in 1..20 {
            let x = 10.0 + 15.0 * i as f64;
            assert_eq!(
                r.handle(T0, &pe(PointerPhase::Move, x, 10.0), i as f64 * 16.0),
                None
            );
        }
        assert_eq!(
            r.handle(T0, &pe(PointerPhase::Up, 300.0, 10.0), 400.0),
            None
        );
        assert_eq!(r.contact_count(), 0);
    }

    #[test]
    fn lifting_the_second_finger_ends_and_the_first_pans_on_unrecognised() {
        let mut r = begun();
        let end = r.handle(T1, &pe(PointerPhase::Up, 80.0, 50.0), 32.0);
        assert_eq!(end.map(|e| e.phase), Some(ScalePhase::End));
        assert_eq!(r.contact_count(), 1);
        // The remaining finger is the owner's pan again.
        assert_eq!(
            r.handle(T0, &pe(PointerPhase::Move, 140.0, 90.0), 48.0),
            None
        );
        assert_eq!(r.handle(T0, &pe(PointerPhase::Up, 140.0, 90.0), 64.0), None);
    }

    #[test]
    fn lifting_the_first_finger_ends_and_drops_every_other_contact() {
        let mut r = begun();
        let end = r.handle(T0, &pe(PointerPhase::Up, 40.0, 50.0), 32.0);
        assert_eq!(end.map(|e| e.phase), Some(ScalePhase::End));
        assert_eq!(r.contact_count(), 0, "the claimant took the others with it");
        assert_eq!(
            r.handle(T1, &pe(PointerPhase::Move, 120.0, 50.0), 48.0),
            None
        );
    }

    #[test]
    fn cancel_ends_the_pinch_with_no_velocity() {
        for id in [T0, T1] {
            let mut r = begun();
            r.handle(T1, &pe(PointerPhase::Move, 100.0, 50.0), 24.0);
            let end = r
                .handle(id, &pe(PointerPhase::Cancel, 0.0, 0.0), 32.0)
                .unwrap();
            assert_eq!(end.phase, ScalePhase::End);
            assert_eq!(end.velocity, 0.0);
            assert!(!r.is_pinching());
        }
    }

    #[test]
    fn velocity_sign_follows_spread_and_pinch() {
        let mut spread = begun();
        let mut v = 0.0;
        for (i, x) in [90.0, 100.0, 110.0].into_iter().enumerate() {
            let t = 16.0 * (i + 2) as f64;
            v = spread
                .handle(T1, &pe(PointerPhase::Move, x, 50.0), t)
                .unwrap()
                .velocity;
        }
        assert!(v > 0.0, "spreading is positive, got {v}");
        let end = spread
            .handle(T1, &pe(PointerPhase::Up, 110.0, 50.0), 64.0)
            .unwrap();
        assert!(end.velocity > 0.0, "a prompt release keeps the rate");

        let mut pinch = begun();
        for (i, x) in [75.0, 70.0, 65.0].into_iter().enumerate() {
            let t = 16.0 * (i + 2) as f64;
            v = pinch
                .handle(T1, &pe(PointerPhase::Move, x, 50.0), t)
                .unwrap()
                .velocity;
        }
        assert!(v < 0.0, "pinching is negative, got {v}");
    }

    #[test]
    fn velocity_is_a_log_rate_and_survives_shared_timestamps() {
        let mut r = begun(); // distance 40 at t = 16
        // Two events on one frame time: no division by zero, no sample yet.
        let a = r
            .handle(T1, &pe(PointerPhase::Move, 90.0, 50.0), 16.0)
            .unwrap();
        assert_eq!(a.velocity, 0.0);
        // 100 ms later the distance has doubled since the last sample (40 → 80).
        let b = r
            .handle(T1, &pe(PointerPhase::Move, 120.0, 50.0), 116.0)
            .unwrap();
        let expected = 2.0_f64.ln() / 0.1;
        assert!((b.velocity - expected).abs() < 1e-9, "{b:?}");
    }

    #[test]
    fn a_stale_release_reports_no_velocity() {
        let mut r = begun();
        r.handle(T1, &pe(PointerPhase::Move, 100.0, 50.0), 32.0);
        let end = r
            .handle(
                T1,
                &pe(PointerPhase::Up, 100.0, 50.0),
                32.0 + VELOCITY_WINDOW_MS + 1.0,
            )
            .unwrap();
        assert_eq!(end.velocity, 0.0, "held still before lifting");
    }

    #[test]
    fn slop_gates_begin() {
        let mut r = paired(); // distance 20
        // 17 px of change: inside the slop.
        assert_eq!(
            r.handle(T1, &pe(PointerPhase::Move, 77.0, 50.0), 16.0),
            None
        );
        // Translating the pair without changing distance never begins either.
        assert_eq!(
            r.handle(T0, &pe(PointerPhase::Move, 57.0, 50.0), 32.0),
            None
        );
        assert_eq!(
            r.handle(T1, &pe(PointerPhase::Move, 77.0, 80.0), 48.0),
            None
        );
        assert!(!r.is_pinching());
        // Back to 20 apart at (57,50)/(77,50), then 19 px of change: begins.
        assert_eq!(
            r.handle(T1, &pe(PointerPhase::Move, 77.0, 50.0), 64.0),
            None
        );
        let begin = r.handle(T1, &pe(PointerPhase::Move, 96.0, 50.0), 80.0);
        assert_eq!(begin.map(|e| e.phase), Some(ScalePhase::Begin));
        const { assert!(PINCH_SLOP > 17.0 && PINCH_SLOP < 19.0) };
    }

    #[test]
    fn a_third_contact_waits_for_a_pair_member_to_lift() {
        let mut r = begun();
        assert_eq!(
            r.handle(T2, &pe(PointerPhase::Down, 200.0, 50.0), 20.0),
            None
        );
        assert_eq!(
            r.handle(T2, &pe(PointerPhase::Move, 300.0, 50.0), 24.0),
            None
        );
        let end = r.handle(T1, &pe(PointerPhase::Up, 80.0, 50.0), 32.0);
        assert_eq!(end.map(|e| e.phase), Some(ScalePhase::End));
        // T0 and T2 form a new pair, re-armed at their current distance.
        assert_eq!(r.contact_count(), 2);
        assert_eq!(
            r.handle(T2, &pe(PointerPhase::Move, 310.0, 50.0), 48.0),
            None
        );
        let begin = r.handle(T2, &pe(PointerPhase::Move, 340.0, 50.0), 64.0);
        assert_eq!(begin.map(|e| e.phase), Some(ScalePhase::Begin));
    }

    #[test]
    fn coincident_contacts_never_produce_a_zero_scale() {
        let mut r = begun();
        let collapse = r
            .handle(T1, &pe(PointerPhase::Move, 40.0, 50.0), 32.0)
            .unwrap();
        assert!(collapse.scale_delta > 0.0 && collapse.scale_delta.is_finite());
        let reopen = r
            .handle(T1, &pe(PointerPhase::Move, 80.0, 50.0), 48.0)
            .unwrap();
        assert!((collapse.scale_delta * reopen.scale_delta - 1.0).abs() < 1e-12);
    }

    // --- The wrapper, driven through a real `RenderRoot` -----------------------

    #[derive(Default)]
    struct PinchState {
        scales: Vec<ScaleEvent>,
    }

    /// A full-bleed child that captures on `Down` (like a scroll view) and logs
    /// every pointer phase it receives into a shared log — never into app state,
    /// so its `Cancel` arm stays state-free.
    struct Recorder {
        log: Rc<RefCell<Vec<PointerPhase>>>,
        handles_scale: bool,
    }
    struct RecorderWidget {
        log: Rc<RefCell<Vec<PointerPhase>>>,
        handles_scale: bool,
    }
    impl View<PinchState> for Recorder {
        type Element = RecorderWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> RecorderWidget {
            RecorderWidget {
                log: self.log.clone(),
                handles_scale: self.handles_scale,
            }
        }
        fn rebuild(
            &self,
            _p: &Self,
            _e: &mut RecorderWidget,
            _c: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for RecorderWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(p) => {
                    self.log.borrow_mut().push(p.phase);
                    if p.phase == PointerPhase::Down {
                        ctx.capture_pointer();
                    }
                    EventResult::Handled
                }
                InputEvent::Scale(_) if self.handles_scale => EventResult::Handled,
                _ => EventResult::Ignored,
            }
        }
    }

    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: peniko::Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    type Root = RenderRoot<PinchState, PinchDetectorView<PinchState>>;

    fn detector_root(handles_scale: bool) -> (Root, Rc<RefCell<Vec<PointerPhase>>>) {
        let log = Rc::new(RefCell::new(Vec::new()));
        let child_log = log.clone();
        let mut logic = move |_: &mut PinchState| {
            pinch_detector(Recorder {
                log: child_log.clone(),
                handles_scale,
            })
            .on_scale(|s: &mut PinchState, e| s.scales.push(e))
        };
        let mut root: Root = RenderRoot::new();
        root.rebuild(&mut logic, &mut PinchState::default());
        root.layout(Size::new(400.0, 400.0));
        (root, log)
    }

    fn touch(slot: u32, phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::PointerContact {
            pointer_id: PointerId::touch(slot),
            event: pe(phase, x, y),
        }
    }

    fn phases(state: &PinchState) -> Vec<ScalePhase> {
        state.scales.iter().map(|e| e.phase).collect()
    }

    #[test]
    fn wrapper_receives_the_second_finger_and_reports_the_pinch() {
        use PointerPhase::{Down, Move, Up};
        let (mut root, child) = detector_root(false);
        let mut state = PinchState::default();
        let mut sink = NullScene;
        root.paint(&mut sink, FrameTime::from_nanos(0));
        root.event(&mut state, &touch(0, Down, 100.0, 100.0));
        root.event(&mut state, &touch(1, Down, 120.0, 100.0));
        root.paint(&mut sink, FrameTime::from_nanos(16_000_000));
        root.event(&mut state, &touch(1, Move, 160.0, 100.0)); // 20 → 60: Begin
        root.paint(&mut sink, FrameTime::from_nanos(32_000_000));
        root.event(&mut state, &touch(1, Move, 220.0, 100.0)); // 60 → 120
        root.event(&mut state, &touch(1, Up, 220.0, 100.0));
        assert_eq!(
            phases(&state),
            [ScalePhase::Begin, ScalePhase::Update, ScalePhase::End]
        );
        let update = state.scales[1];
        assert!((update.scale_delta - 2.0).abs() < 1e-12);
        assert_eq!(update.focal, Point::new(160.0, 100.0));
        assert!(update.velocity > 0.0);
        assert!(
            root.is_pointer_captured(),
            "the first finger still holds it"
        );

        // The rest of the gesture: the first finger pans; nothing more scales,
        // and the stolen child hears nothing until a fresh gesture.
        root.event(&mut state, &touch(0, Move, 50.0, 50.0));
        root.event(&mut state, &touch(0, Up, 50.0, 50.0));
        assert_eq!(state.scales.len(), 3);
        assert!(!root.is_pointer_captured());
        assert_eq!(*child.borrow(), [Down, PointerPhase::Cancel]);

        // A fresh single-finger gesture reaches the child again.
        root.event(&mut state, &touch(0, Down, 10.0, 10.0));
        root.event(&mut state, &touch(0, Up, 10.0, 10.0));
        assert_eq!(
            *child.borrow(),
            [Down, PointerPhase::Cancel, Down, Up],
            "the steal lasted one gesture"
        );
    }

    #[test]
    fn wrapper_single_finger_drag_passes_through_to_the_child() {
        use PointerPhase::{Down, Move, Up};
        let (mut root, child) = detector_root(false);
        let mut state = PinchState::default();
        for event in [
            touch(0, Down, 10.0, 10.0),
            touch(0, Move, 200.0, 10.0),
            touch(0, Move, 300.0, 200.0),
            touch(0, Up, 300.0, 200.0),
        ] {
            root.event(&mut state, &event);
        }
        assert!(state.scales.is_empty());
        assert_eq!(*child.borrow(), [Down, Move, Move, Up]);
        assert!(!root.is_pointer_captured());
    }

    #[test]
    fn wrapper_second_finger_inside_the_slop_leaves_the_child_alone() {
        use PointerPhase::{Down, Move, Up};
        let (mut root, child) = detector_root(false);
        let mut state = PinchState::default();
        root.event(&mut state, &touch(0, Down, 100.0, 100.0));
        root.event(&mut state, &touch(1, Down, 140.0, 100.0));
        root.event(&mut state, &touch(1, Move, 145.0, 100.0));
        root.event(&mut state, &touch(1, Up, 145.0, 100.0));
        root.event(&mut state, &touch(0, Move, 110.0, 100.0));
        root.event(&mut state, &touch(0, Up, 110.0, 100.0));
        assert!(state.scales.is_empty());
        assert_eq!(
            *child.borrow(),
            [Down, Move, Up],
            "the extra finger never reaches the child"
        );
    }

    #[test]
    fn wrapper_cancel_ends_without_calling_on_scale() {
        use PointerPhase::{Cancel, Down, Move};
        let (mut root, child) = detector_root(false);
        let mut state = PinchState::default();
        root.event(&mut state, &touch(0, Down, 100.0, 100.0));
        root.event(&mut state, &touch(1, Down, 120.0, 100.0));
        root.event(&mut state, &touch(0, Move, 60.0, 100.0)); // claimant spreads: Begin
        root.event(&mut state, &touch(0, Cancel, 60.0, 100.0));
        assert_eq!(phases(&state), [ScalePhase::Begin]);
        assert!(!root.is_pointer_captured());
        // The claimant's own Move both began the pinch and carried the steal.
        assert_eq!(*child.borrow(), [Down, Cancel]);
    }

    #[test]
    fn wrapper_offers_a_desktop_scale_to_the_child_first() {
        let event = InputEvent::Scale(ScaleEvent {
            phase: ScalePhase::Update,
            scale_delta: 1.1,
            focal: Point::new(50.0, 50.0),
            velocity: 0.0,
        });
        let (mut root, _) = detector_root(false);
        let mut state = PinchState::default();
        root.event(&mut state, &event);
        assert_eq!(state.scales.len(), 1, "an ignored scale reaches on_scale");
        assert_eq!(state.scales[0].scale_delta, 1.1);

        let (mut root, _) = detector_root(true);
        let mut state = PinchState::default();
        root.event(&mut state, &event);
        assert!(state.scales.is_empty(), "the child handled it");
    }

    #[test]
    fn wrapper_mouse_press_is_an_ordinary_single_pointer_gesture() {
        use PointerPhase::{Down, Up};
        let (mut root, child) = detector_root(false);
        let mut state = PinchState::default();
        root.event(&mut state, &InputEvent::Pointer(pe(Down, 10.0, 10.0)));
        root.event(&mut state, &InputEvent::Pointer(pe(Up, 10.0, 10.0)));
        assert!(state.scales.is_empty());
        assert_eq!(*child.borrow(), [Down, Up]);
    }
}
