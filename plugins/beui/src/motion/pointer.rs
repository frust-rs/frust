//! [`PointerTracker`]: where the pointer is inside *this* widget, and how far
//! from its centre.
//!
//! beUI's decorative pointer effects — the magnetic button, the tilt card, the
//! spotlight wash, the dock's magnification neighbourhood — all read the same
//! two numbers off a `mousemove`: the cursor's position within the element's own
//! box, and that position expressed as a signed fraction of the half-box
//! (`components/motion/magnetic.tsx`, beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`:
//! `(e.clientX − rect.left − rect.width / 2) * strength`). frust hands a widget
//! the first for free — [`PointerEvent::position`] is already widget-local
//! logical px, on every phase, translated by the container chain that routed it
//! — so this module is the second number plus the state machine around it.
//!
//! # There is no leave event; there are three ways to lose the pointer
//!
//! The web effects above reset on `onMouseLeave`. frust publishes **no leave
//! phase at all** — `PointerPhase` is `Down`/`Move`/`Up`/`Cancel` — because
//! hover here is a per-pass *claim* rather than a sticky state: a widget
//! hit-tests each uncaptured `Move`, calls `EventCtx::claim_hover` while inside,
//! and reads `PaintCtx::is_hovered` at paint as the authoritative answer (see
//! `docs/CODE_STANDARDS.md`'s Interaction Semantics). A pointer that leaves
//! routes its next move to whatever it moved *onto*, so the widget it left never
//! hears about it.
//!
//! This tracker therefore implements the leave reset three ways, and a consumer
//! wires all three because each catches what the others cannot:
//!
//! 1. [`on_pointer`](PointerTracker::on_pointer) from the widget's event arm —
//!    a move landing outside the box, or an `Up`/`Cancel`, resets immediately.
//! 2. [`sync_hovered`](PointerTracker::sync_hovered) from the widget's paint,
//!    fed `PaintCtx::is_hovered` — the authoritative correction, and the only
//!    thing that catches a pointer that left without another event.
//! 3. [`reset`](PointerTracker::reset) for a consumer that knows on its own that
//!    it has stopped being pointed at (an overlay closing under the cursor).
//!
//! Residual: with the pointer outside the window entirely and no further move
//! arriving, frust's own hover link can stand until the next in-window move —
//! `docs/LIMITATIONS.md`'s `hover-window-leave-standing`. Nothing this tracker
//! can do resolves it; the paint-time sync inherits whatever the framework
//! reports.
//!
//! # Touch
//!
//! Upstream skips these effects entirely on a touch device (`useHoverCapable`)
//! because a tap manufactures a phantom hover. frust draws no
//! touch-versus-mouse distinction in `PointerEvent`, so the equivalent guard is
//! the framework's own: a **captured** pointer never creates hover, and a `Down`
//! ends the hover link outright. A tracker driven only from uncaptured moves is
//! therefore quiet under a drag, and a decorative consumer should additionally
//! collapse itself under the theme's `reduce_motion` — which is the same switch
//! upstream's `useReducedMotion` throws.

use frust::authoring::{Point, PointerEvent, PointerPhase, Size, Vec2};
use frust::{RwSignal, Set};

/// Widget-local pointer state: whether the pointer is over this widget, where it
/// is, and how far that is from the centre.
///
/// Plain retained state, deliberately not signal-backed: a decorative effect
/// repaints from the tracked value, and writing a reactive signal on every
/// pointer move would rebuild the view tree at pointer-sample rate. A consumer
/// that genuinely needs cross-tree reactivity publishes into [`PointerSignals`]
/// instead, and pays that cost knowingly.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointerTracker {
    hovered: bool,
    position: Point,
    size: Size,
}

impl PointerTracker {
    /// An idle tracker: not hovered, centred, zero-sized until the first event
    /// or [`set_size`](Self::set_size) tells it otherwise.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the pointer is currently over this widget.
    pub fn hovered(&self) -> bool {
        self.hovered
    }

    /// The pointer's last known position in this widget's own logical-px space.
    ///
    /// While not hovered this is the box's centre, which is what makes
    /// [`offset`](Self::offset) read zero at rest — the same resting value
    /// upstream's `onMouseLeave` writes into its motion values.
    pub fn position(&self) -> Point {
        self.position
    }

    /// The widget box the position is interpreted against.
    pub fn size(&self) -> Size {
        self.size
    }

    /// The pointer's offset from the box centre, normalised to `-1..=1` on each
    /// axis: `-1` at the left/top edge, `0` at the centre, `1` at the
    /// right/bottom.
    ///
    /// Clamped, so a captured drag that wanders outside the widget still yields
    /// a bounded value rather than an unbounded tilt. A zero-extent axis has no
    /// centre to be offset from and reports `0.0`.
    pub fn offset(&self) -> Vec2 {
        Vec2::new(
            normalize(self.position.x, self.size.width),
            normalize(self.position.y, self.size.height),
        )
    }

    /// The pointer's displacement from the centre in **logical px**, scaled by
    /// `strength` — the magnetic pull upstream applies to its wrapper's
    /// transform (`strength` defaults to `0.35` there).
    ///
    /// Unlike [`offset`](Self::offset) this is not normalised: a wider control
    /// pulls further, which is the effect's whole character.
    pub fn magnetic(&self, strength: f64) -> Vec2 {
        Vec2::new(
            (self.position.x - self.size.width / 2.0) * strength,
            (self.position.y - self.size.height / 2.0) * strength,
        )
    }

    /// Tell the tracker what box to interpret positions against — the widget's
    /// laid-out size, from `layout` or from `PaintCtx::size`.
    ///
    /// Re-centres a resting tracker so a resize does not leave it reporting a
    /// stale offset. Returns whether anything changed.
    pub fn set_size(&mut self, size: Size) -> bool {
        if self.size == size {
            return false;
        }
        self.size = size;
        if !self.hovered {
            self.position = centre(size);
        }
        true
    }

    /// Feed one pointer event, interpreted against a box of `size`.
    ///
    /// Returns whether the tracked state changed — the widget gates its
    /// `request_redraw` on this, per the latch half of the framework's hover
    /// contract, so a pointer wandering inside a widget does not repaint it on
    /// every sample it does not move for.
    ///
    /// `Down`/`Move` inside the box track; anything outside it, and every
    /// `Up`/`Cancel`, resets — an `Up` or `Cancel` ends the framework's hover
    /// link outright, so a tracker that held its value across one would
    /// disagree with [`sync_hovered`](Self::sync_hovered) on the very next
    /// paint.
    ///
    /// **Claiming hover is still the caller's job.** This tracker sees no
    /// `EventCtx` and cannot claim; a consumer calls `EventCtx::claim_hover`
    /// from its own uncaptured `Move` arm — after routing to any children — for
    /// the framework to record the link at all.
    pub fn on_pointer(&mut self, event: &PointerEvent, size: Size) -> bool {
        let changed = self.set_size(size);
        match event.phase {
            PointerPhase::Down | PointerPhase::Move => {
                if inside(event.position, size) {
                    let was = (self.hovered, self.position);
                    self.hovered = true;
                    self.position = event.position;
                    changed || was != (self.hovered, self.position)
                } else {
                    self.reset() || changed
                }
            }
            PointerPhase::Up | PointerPhase::Cancel => self.reset() || changed,
        }
    }

    /// Reconcile against the framework's authoritative hover answer —
    /// `PaintCtx::is_hovered`, read from the consumer's `paint`.
    ///
    /// The self-correcting third of the hover contract. Losing hover resets the
    /// position with it; *gaining* it here only raises the flag, because
    /// `is_hovered` reports path membership and carries no position — the next
    /// `Move` supplies that.
    ///
    /// Returns whether anything changed.
    pub fn sync_hovered(&mut self, hovered: bool) -> bool {
        if self.hovered == hovered {
            return false;
        }
        if hovered {
            self.hovered = true;
            true
        } else {
            self.reset()
        }
    }

    /// Drop back to rest: not hovered, centred, offset zero. Returns whether
    /// anything changed.
    pub fn reset(&mut self) -> bool {
        let resting = centre(self.size);
        if !self.hovered && self.position == resting {
            return false;
        }
        self.hovered = false;
        self.position = resting;
        true
    }
}

/// A [`PointerTracker`]'s state republished as signals, for a consumer that
/// needs it **outside** the widget observing it — a sibling reading the same
/// cursor, a component driving app state from a hover.
///
/// Deliberately separate from the tracker: a signal write wakes every tracked
/// reader, so a per-sample publish is a real cost and belongs at a call site
/// that has decided to pay it. A widget animating only its own chrome should
/// read the tracker directly and repaint, which costs nothing outside itself.
///
/// [`publish`](Self::publish) is a no-op when nothing moved, so wiring it behind
/// [`PointerTracker::on_pointer`]'s changed-return keeps the wake rate at the
/// rate the state actually changes.
#[derive(Clone, Copy, Debug)]
pub struct PointerSignals {
    /// Whether the pointer is over the tracked widget.
    pub hovered: RwSignal<bool>,
    /// The pointer's widget-local position.
    pub position: RwSignal<Point>,
    /// The centre-normalised offset, `-1..=1` per axis.
    pub offset: RwSignal<Vec2>,
}

impl PointerSignals {
    /// Three signals at rest.
    pub fn new() -> Self {
        Self {
            hovered: RwSignal::new(false),
            position: RwSignal::new(Point::ORIGIN),
            offset: RwSignal::new(Vec2::ZERO),
        }
    }

    /// Copy `tracker`'s current state into the three signals.
    pub fn publish(&self, tracker: &PointerTracker) {
        self.hovered.set(tracker.hovered());
        self.position.set(tracker.position());
        self.offset.set(tracker.offset());
    }
}

impl Default for PointerSignals {
    fn default() -> Self {
        Self::new()
    }
}

/// The centre of a box of `size`, in that box's own space.
fn centre(size: Size) -> Point {
    Point::new(size.width / 2.0, size.height / 2.0)
}

/// One axis of [`PointerTracker::offset`]: `value`'s signed distance from the
/// centre of `extent`, as a fraction of the half-extent, clamped to `-1..=1`.
fn normalize(value: f64, extent: f64) -> f64 {
    if extent <= 0.0 {
        return 0.0;
    }
    let half = extent / 2.0;
    ((value - half) / half).clamp(-1.0, 1.0)
}

/// Whether `point` lies within a box of `size` at the origin — the same
/// half-open hit test the baseline widgets use for a press.
fn inside(point: Point, size: Size) -> bool {
    point.x >= 0.0 && point.y >= 0.0 && point.x < size.width && point.y < size.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::PointerButton;
    use frust::{Get, GetUntracked};

    const BOX: Size = Size::new(100.0, 40.0);

    fn at(phase: PointerPhase, x: f64, y: f64) -> PointerEvent {
        PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        }
    }

    /// The centre reads zero and each edge reads its own unit, per axis — the
    /// normalisation a tilt or spotlight is built on.
    #[test]
    fn the_offset_runs_minus_one_to_one_across_the_box() {
        let mut tracker = PointerTracker::new();

        tracker.on_pointer(&at(PointerPhase::Move, 50.0, 20.0), BOX);
        assert_eq!(tracker.offset(), Vec2::new(0.0, 0.0));

        tracker.on_pointer(&at(PointerPhase::Move, 0.0, 0.0), BOX);
        assert_eq!(tracker.offset(), Vec2::new(-1.0, -1.0));

        tracker.on_pointer(&at(PointerPhase::Move, 99.0, 39.0), BOX);
        let corner = tracker.offset();
        assert!(
            corner.x > 0.9 && corner.y > 0.9,
            "near the far corner: {corner:?}"
        );

        // Each axis normalises against its own extent, not a shared one.
        tracker.on_pointer(&at(PointerPhase::Move, 75.0, 20.0), BOX);
        assert_eq!(tracker.offset(), Vec2::new(0.5, 0.0));
        tracker.on_pointer(&at(PointerPhase::Move, 50.0, 30.0), BOX);
        assert_eq!(tracker.offset(), Vec2::new(0.0, 0.5));
    }

    /// A degenerate box has no centre to be offset from, and must not divide by
    /// zero.
    #[test]
    fn a_zero_sized_box_reports_no_offset() {
        let mut tracker = PointerTracker::new();
        tracker.on_pointer(&at(PointerPhase::Move, 5.0, 5.0), Size::ZERO);
        assert_eq!(tracker.offset(), Vec2::ZERO);
        assert!(!tracker.hovered(), "nothing can be inside a zero-sized box");
    }

    /// The three leave routes all land on the same resting state, and a move
    /// outside the box is one of them.
    #[test]
    fn every_leave_route_resets_to_the_resting_centre() {
        let hovered = |tracker: &PointerTracker| tracker.hovered();

        // 1. a move that lands outside.
        let mut tracker = PointerTracker::new();
        tracker.on_pointer(&at(PointerPhase::Move, 10.0, 10.0), BOX);
        assert!(hovered(&tracker));
        assert!(tracker.on_pointer(&at(PointerPhase::Move, 150.0, 10.0), BOX));
        assert!(!hovered(&tracker));
        assert_eq!(tracker.offset(), Vec2::ZERO);
        assert_eq!(tracker.position(), Point::new(50.0, 20.0));

        // 2. the paint-time correction, the only route that catches a pointer
        //    which left without another event reaching this widget.
        let mut tracker = PointerTracker::new();
        tracker.on_pointer(&at(PointerPhase::Move, 10.0, 10.0), BOX);
        assert!(tracker.sync_hovered(false));
        assert!(!hovered(&tracker));
        assert_eq!(tracker.offset(), Vec2::ZERO);

        // 3. the explicit reset.
        let mut tracker = PointerTracker::new();
        tracker.on_pointer(&at(PointerPhase::Move, 10.0, 10.0), BOX);
        assert!(tracker.reset());
        assert!(!hovered(&tracker));
    }

    /// An `Up` or a `Cancel` ends the framework's hover link, so the tracker
    /// must let go too rather than disagree with the next paint.
    #[test]
    fn a_release_or_cancel_ends_the_hover() {
        for phase in [PointerPhase::Up, PointerPhase::Cancel] {
            let mut tracker = PointerTracker::new();
            tracker.on_pointer(&at(PointerPhase::Move, 10.0, 10.0), BOX);
            assert!(tracker.hovered());
            assert!(tracker.on_pointer(&at(phase, 10.0, 10.0), BOX));
            assert!(!tracker.hovered(), "{phase:?} must end the hover");
        }
    }

    /// The changed-return is what gates a consumer's redraw: a repeated sample
    /// at the same spot reports no change, and a resting tracker fed another
    /// leave reports none either.
    #[test]
    fn an_unchanged_sample_reports_no_change() {
        let mut tracker = PointerTracker::new();
        assert!(tracker.on_pointer(&at(PointerPhase::Move, 10.0, 10.0), BOX));
        assert!(!tracker.on_pointer(&at(PointerPhase::Move, 10.0, 10.0), BOX));
        assert!(tracker.on_pointer(&at(PointerPhase::Move, 11.0, 10.0), BOX));

        tracker.reset();
        assert!(!tracker.reset());
        assert!(!tracker.sync_hovered(false));
    }

    /// Gaining hover from the paint-time answer raises the flag without
    /// inventing a position — `is_hovered` carries none.
    #[test]
    fn syncing_hover_on_does_not_invent_a_position() {
        let mut tracker = PointerTracker::new();
        tracker.set_size(BOX);
        assert!(tracker.sync_hovered(true));
        assert!(tracker.hovered());
        assert_eq!(tracker.position(), Point::new(50.0, 20.0));
        assert_eq!(tracker.offset(), Vec2::ZERO);
    }

    /// A resize re-centres a resting tracker instead of leaving it reporting an
    /// offset against the box it no longer has.
    #[test]
    fn a_resize_recentres_a_resting_tracker() {
        let mut tracker = PointerTracker::new();
        assert!(tracker.set_size(BOX));
        assert_eq!(tracker.position(), Point::new(50.0, 20.0));
        assert!(tracker.set_size(Size::new(200.0, 200.0)));
        assert_eq!(tracker.position(), Point::new(100.0, 100.0));
        assert_eq!(tracker.offset(), Vec2::ZERO);
        assert!(!tracker.set_size(Size::new(200.0, 200.0)));
    }

    /// The magnetic pull is un-normalised and scaled: displacement in px times
    /// the caller's strength.
    #[test]
    fn the_magnetic_pull_scales_the_raw_displacement() {
        let mut tracker = PointerTracker::new();
        tracker.on_pointer(&at(PointerPhase::Move, 90.0, 20.0), BOX);
        assert_eq!(tracker.magnetic(1.0), Vec2::new(40.0, 0.0));
        assert_eq!(tracker.magnetic(0.35), Vec2::new(14.0, 0.0));
        tracker.reset();
        assert_eq!(tracker.magnetic(0.35), Vec2::ZERO);
    }

    /// The signal mirror carries the tracker's state across a publish.
    #[test]
    fn publishing_mirrors_the_tracker_into_signals() {
        let signals = PointerSignals::new();
        assert!(!signals.hovered.get_untracked());

        let mut tracker = PointerTracker::new();
        tracker.on_pointer(&at(PointerPhase::Move, 75.0, 20.0), BOX);
        signals.publish(&tracker);
        assert!(signals.hovered.get());
        assert_eq!(signals.position.get(), Point::new(75.0, 20.0));
        assert_eq!(signals.offset.get(), Vec2::new(0.5, 0.0));

        tracker.reset();
        signals.publish(&tracker);
        assert!(!signals.hovered.get());
        assert_eq!(signals.offset.get(), Vec2::ZERO);
    }
}
