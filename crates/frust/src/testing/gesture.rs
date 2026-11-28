//! Scripted pointer input for a [`Tester`]: single-pointer composites ([`Tester::tap`],
//! [`Tester::long_press`], [`Tester::drag`], ..) and raw gestures ([`Tester::start_gesture`],
//! [`Tester::start_touch`]) moved and released through a [`Gesture`] handle.
//!
//! Every helper sends one event per call through [`Tester::send`], positions in window
//! coordinates. Events carry no clock, and a widget learns elapsed time only from the paints
//! between them, so a helper pumps exactly where elapsed time is part of its meaning
//! ([`long_press`](Tester::long_press), [`drag`](Tester::drag)) and nowhere else:
//! [`tap`](Tester::tap) does not pump, and the change a tap makes shows from the next pump.
//!
//! # Several contacts
//!
//! [`start_gesture`](Tester::start_gesture) is the mouse (a bare `InputEvent::Pointer`);
//! [`start_touch`](Tester::start_touch) is a touch contact in a numbered slot
//! (`InputEvent::PointerContact`). The root delivers slot `0` like the mouse, but drops slot `1`
//! and above unless the widget that captured the gesture's first contact opted into its other
//! contacts (`EventCtx::capture_contacts`), as `pan_zoom` and `pinch_detector` do. A second finger
//! over any other widget reaches nothing.

use std::time::Duration;

use frust_core::event::PointerId;
use frust_core::{
    EventOutcome, InputEvent, PointerButton, PointerEvent, PointerPhase, ScrollDelta, TOUCH_SLOP,
    View,
};
use kurbo::{Point, Vec2};

use super::Tester;
use super::find::Finder;
use super::tester::FRAME;

/// The gesture detector's default long-press threshold (`frust_widgets::gesture`'s
/// `LONG_PRESS_MS`, which is crate-private there).
const LONG_PRESS_MS: u64 = 500;

/// How far [`Tester::drag`] moves before its stepped delta: just past a pan's slop (twice
/// [`TOUCH_SLOP`]), which also crosses an axis drag's slop in any direction.
const DRAG_LEAD_IN: f64 = 2.0 * TOUCH_SLOP + 1.0;

/// One pointer held down by [`Tester::start_gesture`], [`Tester::start_touch`] or
/// [`Tester::press`]: its contact identity and where it is now.
///
/// Move it with [`Tester::move_by`] / [`Tester::move_to`] and end it with [`Tester::up`] or
/// [`Tester::cancel`] (Flutter's `TestGesture`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gesture {
    pointer: PointerId,
    position: Point,
}

impl Gesture {
    pub(crate) fn new(pointer: PointerId, position: Point) -> Self {
        Self { pointer, position }
    }

    /// The contact: [`PointerId::MOUSE`] or a touch slot.
    pub fn pointer(&self) -> PointerId {
        self.pointer
    }

    /// Where the pointer is, in window coordinates.
    pub fn position(&self) -> Point {
        self.position
    }

    /// The event this gesture sends for `phase`: a bare `Pointer` for the mouse, a
    /// `PointerContact` for a touch slot.
    fn event(&self, phase: PointerPhase) -> InputEvent {
        let event = PointerEvent {
            phase,
            position: self.position,
            button: PointerButton::Primary,
        };
        if self.pointer == PointerId::MOUSE {
            InputEvent::Pointer(event)
        } else {
            InputEvent::PointerContact {
                pointer_id: self.pointer,
                event,
            }
        }
    }
}

/// Where a gesture lands: a window-space point, or the centre of the first node a [`Finder`]
/// matches as of the last layout.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// This point, in window coordinates.
    Point(Point),
    /// The centre of the first match's bounds.
    Finder(Finder),
}

impl From<Point> for Target {
    fn from(point: Point) -> Self {
        Target::Point(point)
    }
}

impl From<(f64, f64)> for Target {
    fn from((x, y): (f64, f64)) -> Self {
        Target::Point(Point::new(x, y))
    }
}

impl From<Finder> for Target {
    fn from(finder: Finder) -> Self {
        Target::Finder(finder)
    }
}

impl From<&Finder> for Target {
    fn from(finder: &Finder) -> Self {
        Target::Finder(finder.clone())
    }
}

impl<S: 'static, V: View<S>> Tester<S, V> {
    /// The window-space point `target` names.
    ///
    /// # Panics
    ///
    /// When a finder target matches nothing; the message lists what the tree holds.
    fn resolve_target(&self, target: Target) -> Point {
        match target {
            Target::Point(point) => point,
            Target::Finder(finder) => match self.find(&finder).first() {
                Some(found) => found.bounds.center(),
                None => panic!("{}", self.miss_message(&finder)),
            },
        }
    }

    /// Put the mouse's primary button down at `at` and return the held gesture (Flutter's
    /// `startGesture`). It sends a bare `InputEvent::Pointer`.
    pub fn start_gesture(&mut self, at: impl Into<Target>) -> Gesture {
        let gesture = Gesture::new(PointerId::MOUSE, self.resolve_target(at.into()));
        self.send(&gesture.event(PointerPhase::Down));
        gesture
    }

    /// Put a finger down in touch slot `slot` at `at` and return the held gesture. It sends an
    /// `InputEvent::PointerContact` with `PointerId::touch(slot)`.
    ///
    /// Slot `0` is hit-tested like the mouse. Slot `1` and above is dropped at the root unless
    /// the widget that captured slot `0`'s `Down` opted into the gesture's other contacts
    /// (`EventCtx::capture_contacts`), as `pan_zoom` and `pinch_detector` do; so a pinch is
    /// `start_touch(0, ..)` then `start_touch(1, ..)` over such a widget.
    pub fn start_touch(&mut self, slot: u32, at: impl Into<Target>) -> Gesture {
        let gesture = Gesture::new(PointerId::touch(slot), self.resolve_target(at.into()));
        self.send(&gesture.event(PointerPhase::Down));
        gesture
    }

    /// Send a wheel scroll of `delta` at `at`, with no pump.
    ///
    /// [`ScrollDelta::Lines`] is a wheel notch and [`ScrollDelta::Pixels`] a trackpad's pixel
    /// delta. A scroll view's offset shows from the next pump.
    pub fn scroll_at(&mut self, at: impl Into<Target>, delta: ScrollDelta) -> EventOutcome {
        let position = self.resolve_target(at.into());
        self.send(&InputEvent::Scroll { position, delta })
    }

    /// Move the mouse to `at` with no button down: a bare pointer `Move` (button primary), no
    /// `Down` and no pump.
    ///
    /// Hover chrome is read at paint, so pump once before asserting on it. While a [`Gesture`]
    /// is held the same event is a captured drag move rather than a hover. A later
    /// [`tap`](Self::tap) or `Down` ends the hover.
    pub fn hover(&mut self, at: impl Into<Target>) -> EventOutcome {
        let position = self.resolve_target(at.into());
        self.send(&InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position,
            button: PointerButton::Primary,
        }))
    }

    /// Move `gesture` by `delta` and send the `Move`.
    pub fn move_by(&mut self, gesture: &mut Gesture, delta: Vec2) -> EventOutcome {
        let to = gesture.position + delta;
        self.move_to(gesture, to)
    }

    /// Move `gesture` to window-space `to` and send the `Move`.
    pub fn move_to(&mut self, gesture: &mut Gesture, to: impl Into<Point>) -> EventOutcome {
        gesture.position = to.into();
        self.send(&gesture.event(PointerPhase::Move))
    }

    /// Release `gesture` where it is.
    pub fn up(&mut self, gesture: Gesture) -> EventOutcome {
        self.send(&gesture.event(PointerPhase::Up))
    }

    /// Release `gesture` at window-space `at`, with no `Move` first.
    ///
    /// A real pointer can lift at a point it never reported a position for, and the `Up` carries
    /// that point. Nothing else is sent, so no travel reaches a velocity tracker: a release far
    /// from the last `Move` stays a release and never becomes a fling, which a
    /// [`move_to`](Self::move_to) just before the [`up`](Self::up) would make it.
    ///
    /// A gesture detector learns that a finger left its bounds from `Move`s, not from the `Up`'s
    /// position, so an `up_at` outside the bounds with no `Move` before it still resolves as a
    /// tap — on a device an `Up` never lands away from the last `Move`. To model a finger that
    /// left, [`move_to`](Self::move_to) out of the bounds first, then [`up`](Self::up).
    pub fn up_at(&mut self, mut gesture: Gesture, at: impl Into<Point>) -> EventOutcome {
        gesture.position = at.into();
        self.send(&gesture.event(PointerPhase::Up))
    }

    /// Cancel `gesture` where it is, as a platform gesture steal does.
    ///
    /// A widget never mutates app state from a `Cancel`; a callback it owes (a gesture
    /// detector's `on_tap_cancel` or drag `cancel`) runs from the next rebuild, so pump once
    /// before asserting on it.
    pub fn cancel(&mut self, gesture: Gesture) -> EventOutcome {
        self.send(&gesture.event(PointerPhase::Cancel))
    }

    /// Press and release the mouse at `at`, with no pump in between or after (Flutter's `tap`,
    /// minus its implicit pump). The callback runs during the `Up`; what it changes shows from
    /// the next pump.
    pub fn tap(&mut self, at: impl Into<Target>) {
        let gesture = self.start_gesture(at);
        self.up(gesture);
    }

    /// Press the mouse at `at` and hold it (Flutter's `press`); end it with [`up`](Self::up) or
    /// [`cancel`](Self::cancel).
    pub fn press(&mut self, at: impl Into<Target>) -> Gesture {
        self.start_gesture(at)
    }

    /// Press at `at`, hold it past the gesture detector's default 500 ms long-press threshold,
    /// then release (Flutter's `longPress`).
    ///
    /// The detector times a hold across paints, starting at the first paint after the `Down`, so
    /// this pumps 16 ms frames until the clock has passed the threshold plus one frame; the paint
    /// that observes the threshold requests the deferred fire, and one more pump runs it (from
    /// the next rebuild's housekeeping pass) before the `Up`. A repaint-only effect of the
    /// callback may need one further pump to show. A detector with a longer
    /// `hold_threshold_ms` needs [`press`](Self::press), [`pump`](Self::pump) and
    /// [`up`](Self::up) instead.
    pub fn long_press(&mut self, at: impl Into<Target>) {
        let gesture = self.press(at);
        let threshold = Duration::from_millis(LONG_PRESS_MS) + FRAME;
        let start = self.frame_time();
        while self.frame_time().saturating_sub(start) < threshold {
            self.pump(FRAME);
        }
        self.pump(FRAME);
        self.up(gesture);
    }

    /// Press at `at`, hold past the default long-press threshold until it has fired, drag to
    /// `to` and release.
    ///
    /// The moves after the fire are spread in steps of at most [`TOUCH_SLOP`], 16 ms apart, and
    /// the last lands exactly on `to`. A press moved before the threshold is an ordinary drag,
    /// so this holds still until the long-press fires (see [`long_press`](Self::long_press)).
    pub fn long_press_then_drag(&mut self, from: impl Into<Target>, to: impl Into<Point>) {
        let mut gesture = self.press(from);
        let threshold = Duration::from_millis(LONG_PRESS_MS) + FRAME;
        let start = self.frame_time();
        while self.frame_time().saturating_sub(start) < threshold {
            self.pump(FRAME);
        }
        self.pump(FRAME);
        let origin = gesture.position;
        let delta = to.into() - origin;
        let steps = (delta.hypot() / TOUCH_SLOP).ceil().max(1.0) as usize;
        for step in 1..=steps {
            self.pump(FRAME);
            let at = origin + delta * (step as f64 / steps as f64);
            self.move_to(&mut gesture, at);
        }
        self.pump(FRAME);
        self.up(gesture);
    }

    /// Press at `at`, drag by `delta` and release (Flutter's `drag`).
    ///
    /// Events carry no clock, so a recognizer's slop and a release velocity need paints between
    /// the moves: this pumps 16 ms between every event. The first `Move` is a lead-in just past
    /// a pan's slop (37 px along `delta`'s direction), which starts any drag recognizer or
    /// scroll that accepts that direction; a recognizer measures from where it started and
    /// never replays the slop, so the remaining moves deliver `delta` itself, in steps of at
    /// most [`TOUCH_SLOP`]. The pointer therefore ends at `at + lead-in + delta`; script a
    /// [`start_gesture`](Self::start_gesture) and [`move_to`](Self::move_to) to land on an exact
    /// point. A zero `delta` is a tap.
    pub fn drag(&mut self, at: impl Into<Target>, delta: Vec2) {
        let steps = (delta.hypot() / TOUCH_SLOP).ceil().max(1.0) as usize;
        self.stepped_drag(at.into(), delta, steps);
    }

    /// [`drag`](Self::drag) with `delta` spread over `duration`: one step per 16 ms frame, at
    /// least one (Flutter's `timedDrag`).
    pub fn timed_drag(&mut self, at: impl Into<Target>, delta: Vec2, duration: Duration) {
        let steps = (duration.as_nanos() / FRAME.as_nanos()).max(1);
        let steps = usize::try_from(steps).unwrap_or(usize::MAX);
        self.stepped_drag(at.into(), delta, steps);
    }

    /// Down at `at`, the lead-in move, `steps` evenly spaced moves covering `delta`, Up; a
    /// 16 ms pump between every two events.
    fn stepped_drag(&mut self, at: Target, delta: Vec2, steps: usize) {
        let mut gesture = self.start_gesture(at);
        let length = delta.hypot();
        if length > 0.0 {
            self.pump(FRAME);
            self.move_by(&mut gesture, delta * (DRAG_LEAD_IN / length));
            let origin = gesture.position;
            for step in 1..=steps {
                self.pump(FRAME);
                let to = origin + delta * (step as f64 / steps as f64);
                self.move_to(&mut gesture, to);
            }
            self.pump(FRAME);
        }
        self.up(gesture);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use frust_core::event::PointerId;
    use frust_core::{
        BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, FrameTime, InputEvent,
        LayoutCtx, Modifiers, PaintCtx, PaintScene, PointerPhase, ScrollDelta, View, Widget,
    };
    use kurbo::{Point, Size, Vec2};

    use super::{FRAME, LONG_PRESS_MS};
    use crate::authoring::Role;
    use crate::testing::{Tester, find};
    use crate::{
        DoubleTapDetails, DragEndDetails, DragStartBehavior, DragStartDetails, DragUpdateDetails,
        GestureDetectorView, LongPressEndDetails, LongPressMoveDetails, NavigatorController,
        PanZoomTransform, ScrollController, TapDownDetails, TapUpDetails, scroll_view,
    };

    const SIZE: Size = Size::new(200.0, 200.0);

    #[derive(Default)]
    struct Log {
        log: Vec<&'static str>,
        deltas: Vec<Vec2>,
        up_at: Option<Point>,
        start: Option<Point>,
    }

    fn pad<S: 'static>(side: f64) -> impl View<S> {
        crate::colored_box()
            .fill(crate::Color::from_rgb8(20, 20, 200))
            .size(side, side)
    }

    fn tap_detector(_: &mut Log) -> GestureDetectorView<Log> {
        crate::GestureDetector(pad(200.0))
            .on_tap_down(|s: &mut Log, _: TapDownDetails| s.log.push("down"))
            .on_tap_up(|s: &mut Log, d: TapUpDetails| {
                s.log.push("up");
                s.up_at = Some(d.local_position);
            })
            .on_tap_cancel(|s: &mut Log| s.log.push("cancel"))
            .on_tap(|s: &mut Log| s.log.push("tap"))
            .on_long_press(|s: &mut Log| s.log.push("long"))
    }

    #[test]
    fn tap_fires_down_up_then_tap_once() {
        let mut tester = Tester::new(Log::default(), tap_detector, SIZE);
        tester.tap((50.0, 50.0));
        assert_eq!(tester.state().log, ["down", "up", "tap"]);
    }

    #[test]
    fn up_at_releases_at_the_given_point_without_a_move() {
        let mut tester = Tester::new(Log::default(), tap_detector, SIZE);
        let press = tester.press((50.0, 50.0));
        tester.up_at(press, (120.0, 130.0));
        assert_eq!(
            tester.state().log,
            ["down", "up", "tap"],
            "an in-bounds release is a tap"
        );
        assert_eq!(
            tester.state().up_at,
            Some(Point::new(120.0, 130.0)),
            "the Up carries the release point, not the press point"
        );
    }

    #[test]
    fn up_at_outside_the_bounds_without_a_move_is_still_a_tap() {
        // The detector notices a finger leaving from Moves, never from the Up's position; this
        // pins that an `up_at` far outside is not a substitute for `move_to` out of the bounds.
        let mut tester = Tester::new(Log::default(), tap_detector, SIZE);
        let press = tester.press((50.0, 50.0));
        tester.up_at(press, (50.0, 260.0));
        assert_eq!(tester.state().log, ["down", "up", "tap"]);
        assert_eq!(tester.state().up_at, Some(Point::new(50.0, 260.0)));
    }

    #[test]
    fn press_moved_out_of_bounds_then_up_cancels_without_a_tap() {
        let mut tester = Tester::new(Log::default(), tap_detector, SIZE);
        let mut press = tester.press((50.0, 50.0));
        tester.move_to(&mut press, (50.0, 260.0));
        tester.up(press);
        assert_eq!(tester.state().log, ["down", "cancel"]);
    }

    #[test]
    fn cancel_of_a_held_press_fires_on_tap_cancel_after_one_pump() {
        let mut tester = Tester::new(Log::default(), tap_detector, SIZE);
        let press = tester.press((50.0, 50.0));
        tester.cancel(press);
        assert_eq!(tester.state().log, ["down"], "a Cancel never touches state");
        tester.pump(Duration::ZERO);
        assert_eq!(tester.state().log, ["down", "cancel"]);
    }

    #[test]
    fn long_press_fires_once_and_only_past_the_threshold() {
        let mut tester = Tester::new(Log::default(), tap_detector, SIZE);
        let start = tester.frame_time();
        tester.long_press((50.0, 50.0));
        assert_eq!(tester.state().log, ["down", "long", "up"]);
        assert!(
            tester.frame_time().saturating_sub(start) > Duration::from_millis(LONG_PRESS_MS),
            "the clock passed the threshold"
        );

        let mut held = Tester::new(Log::default(), tap_detector, SIZE);
        let start = held.frame_time();
        let _press = held.press((50.0, 50.0));
        while held.frame_time().saturating_sub(start) < Duration::from_millis(LONG_PRESS_MS) {
            held.pump(FRAME);
            assert!(!held.state().log.contains(&"long"), "no fire below 500 ms");
        }
    }

    #[test]
    fn a_press_released_at_half_the_threshold_is_a_tap() {
        let mut tester = Tester::new(Log::default(), tap_detector, SIZE);
        let press = tester.press((50.0, 50.0));
        tester.pump(Duration::from_millis(LONG_PRESS_MS / 2));
        tester.up(press);
        assert_eq!(tester.state().log, ["down", "up", "tap"]);
    }

    /// The gesture detector's double-tap window (`frust_widgets::gesture`'s `DOUBLE_TAP_MS`,
    /// crate-private there), inclusive, on the raw input clock.
    const DOUBLE_TAP_MS: u64 = 300;

    fn double_tap_detector(log: &mut Log) -> GestureDetectorView<Log> {
        tap_detector(log).on_double_tap(|s: &mut Log, _: DoubleTapDetails| s.log.push("double"))
    }

    #[test]
    fn double_tap_within_the_window_fires_double_tap_and_no_tap() {
        let mut tester = Tester::new(Log::default(), double_tap_detector, SIZE);
        tester.tap((50.0, 50.0));
        tester.pump(Duration::from_millis(100));
        tester.tap((60.0, 55.0));
        assert_eq!(tester.state().log, ["down", "up", "down", "up", "double"]);
        tester.pump_and_settle();
        assert_eq!(
            tester.state().log,
            ["down", "up", "down", "up", "double"],
            "the window closing owes no tap"
        );
    }

    #[test]
    fn a_lone_tap_on_a_double_tap_detector_fires_tap_after_the_window() {
        let mut tester = Tester::new(Log::default(), double_tap_detector, SIZE);
        let released = tester.frame_time();
        tester.tap((50.0, 50.0));
        assert_eq!(tester.state().log, ["down", "up"], "the tap is held");
        let window = Duration::from_millis(DOUBLE_TAP_MS);
        loop {
            assert!(
                tester.pump(FRAME).needs_frame,
                "the open window keeps frames"
            );
            let elapsed = tester.frame_time().saturating_sub(released);
            if elapsed > window {
                break;
            }
            assert!(
                !tester.state().log.contains(&"tap"),
                "no tap {elapsed:?} after the release"
            );
        }
        // The paint that observes the lapse owes the tap to the next Housekeeping pass.
        for _ in 0..2 {
            if tester.state().log.contains(&"tap") {
                break;
            }
            tester.pump(FRAME);
        }
        assert_eq!(tester.state().log, ["down", "up", "tap"]);
        tester.pump_and_settle();
        assert_eq!(tester.state().log, ["down", "up", "tap"], "exactly once");
    }

    #[test]
    fn two_taps_apart_in_space_are_two_taps() {
        let mut tester = Tester::new(Log::default(), double_tap_detector, SIZE);
        tester.tap((50.0, 50.0));
        tester.pump(Duration::from_millis(100));
        // 170 px from the first release, past the 100 px double-tap slop.
        tester.tap((170.0, 170.0));
        assert_eq!(
            tester.state().log,
            ["down", "up", "tap", "down", "up"],
            "the second Down pays the first tap before its own down"
        );
        tester.pump_and_settle();
        assert_eq!(
            tester.state().log,
            ["down", "up", "tap", "down", "up", "tap"],
            "the second tap fires after its own window"
        );
    }

    #[test]
    fn a_tap_only_detector_is_unchanged() {
        let mut tester = Tester::new(Log::default(), tap_detector, SIZE);
        tester.tap((50.0, 50.0));
        assert_eq!(
            tester.state().log,
            ["down", "up", "tap"],
            "on_tap at the Up"
        );
        assert!(
            !tester.pump(Duration::ZERO).needs_frame,
            "no double-tap window keeps frames"
        );
        assert_eq!(tester.state().log, ["down", "up", "tap"]);
    }

    fn drag_detector(_: &mut Log) -> GestureDetectorView<Log> {
        crate::GestureDetector(pad(200.0))
            .on_vertical_drag_start(|s: &mut Log, _: DragStartDetails| s.log.push("start"))
            .on_vertical_drag_update(|s: &mut Log, d: DragUpdateDetails| {
                s.log.push("update");
                s.deltas.push(d.delta);
            })
            .on_vertical_drag_end(|s: &mut Log, _: DragEndDetails| s.log.push("end"))
            .on_vertical_drag_cancel(|s: &mut Log| s.log.push("cancel"))
    }

    fn assert_drag(log: &Log, updates: usize, delta: Vec2) {
        let mut expected = vec!["start"];
        expected.extend(std::iter::repeat_n("update", updates));
        expected.push("end");
        assert_eq!(log.log, expected);
        let sum = log.deltas.iter().fold(Vec2::ZERO, |sum, d| sum + *d);
        assert!((sum - delta).hypot() < 1e-6, "updates sum to {sum:?}");
    }

    #[test]
    fn drag_delivers_start_updates_summing_to_the_delta_and_end() {
        let mut tester = Tester::new(Log::default(), drag_detector, SIZE);
        tester.drag((100.0, 20.0), Vec2::new(0.0, 100.0));
        // 100 px in steps of at most 18 px: six updates.
        assert_drag(tester.state(), 6, Vec2::new(0.0, 100.0));
    }

    #[test]
    fn timed_drag_over_320_ms_delivers_20_updates() {
        let mut tester = Tester::new(Log::default(), drag_detector, SIZE);
        let start = tester.frame_time();
        let delta = Vec2::new(0.0, 100.0);
        tester.timed_drag((100.0, 20.0), delta, Duration::from_millis(320));
        assert_drag(tester.state(), 20, delta);
        assert!(tester.frame_time().saturating_sub(start) >= Duration::from_millis(320));
    }

    /// The start point and update deltas of a pan scripted Down (50, 50), Moves to (70, 50),
    /// (95, 50) (past the 36 px pan slop) and (105, 50), then Up, with `behavior` set, or left
    /// at the default when `None`.
    fn scripted_pan(behavior: Option<DragStartBehavior>) -> (Option<Point>, Vec<Vec2>) {
        let mut tester = Tester::new(
            Log::default(),
            move |_: &mut Log| {
                let detector = crate::GestureDetector(pad(200.0))
                    .on_pan_start(|s: &mut Log, d: DragStartDetails| s.start = Some(d.position))
                    .on_pan_update(|s: &mut Log, d: DragUpdateDetails| s.deltas.push(d.delta));
                match behavior {
                    Some(behavior) => detector.drag_start_behavior(behavior),
                    None => detector,
                }
            },
            SIZE,
        );
        let mut press = tester.press((50.0, 50.0));
        for x in [70.0, 95.0, 105.0] {
            tester.pump(FRAME);
            tester.move_to(&mut press, (x, 50.0));
        }
        tester.pump(FRAME);
        tester.up(press);
        let log = tester.state();
        (log.start, log.deltas.clone())
    }

    #[test]
    fn drag_start_behavior_down_replays_the_slop_in_the_first_update() {
        assert_eq!(
            scripted_pan(Some(DragStartBehavior::Down)),
            (
                Some(Point::new(50.0, 50.0)),
                vec![Vec2::new(45.0, 0.0), Vec2::new(10.0, 0.0)]
            ),
            "the start at the Down, the first update spanning the slop"
        );
        let crossed = (Some(Point::new(95.0, 50.0)), vec![Vec2::new(10.0, 0.0)]);
        assert_eq!(
            scripted_pan(None),
            crossed,
            "the default starts where the slop was crossed"
        );
        assert_eq!(scripted_pan(Some(DragStartBehavior::Start)), crossed);
    }

    struct Zoom {
        transform: PanZoomTransform,
    }

    #[test]
    fn two_touch_contacts_pinch_a_pan_zoom_view_to_twice_its_scale() {
        let mut tester = Tester::new(
            Zoom {
                transform: PanZoomTransform::IDENTITY,
            },
            |_: &mut Zoom| {
                crate::pan_zoom(pad(400.0))
                    .on_transform(|s: &mut Zoom, t: PanZoomTransform| s.transform = t)
            },
            Size::new(400.0, 400.0),
        );
        let mut a = tester.start_touch(0, (180.0, 200.0));
        let mut b = tester.start_touch(1, (220.0, 200.0));
        // Each finger moves 40 px outward. The pinch begins once the 40 px spread has changed
        // by more than the 18 px slop (at 60 px) and scales from there: 120 / 60 = 2.
        for (left, right) in [(170.0, 230.0), (140.0, 260.0)] {
            tester.pump(FRAME);
            tester.move_to(&mut a, (left, 200.0));
            tester.pump(FRAME);
            tester.move_to(&mut b, (right, 200.0));
        }
        tester.pump(FRAME);
        tester.up(b);
        tester.up(a);
        tester.pump(FRAME);
        let scale = tester.state().transform.scale;
        assert!((scale - 2.0).abs() < 0.05, "scale {scale}");
    }

    /// A leaf that takes no capture and records the contact and phase of each pointer event.
    struct Probe(Rc<RefCell<Vec<(PointerId, PointerPhase)>>>);

    impl<S: 'static> View<S> for Probe {
        type Element = Probe;

        fn build(&self, _ctx: &mut BuildCtx<'_>) -> Probe {
            Probe(Rc::clone(&self.0))
        }

        fn rebuild(&self, _prev: &Self, _el: &mut Probe, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for Probe {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(400.0, 400.0))
        }

        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}

        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                self.0.borrow_mut().push((ctx.pointer_id(), p.phase));
            }
            EventResult::Handled
        }
    }

    #[test]
    fn a_second_contact_over_a_widget_without_contact_capture_is_dropped() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let probe = Rc::clone(&seen);
        let mut tester = Tester::new(
            (),
            move |_: &mut ()| crate::container(Probe(Rc::clone(&probe))),
            Size::new(400.0, 400.0),
        );
        let mut a = tester.start_gesture((180.0, 200.0));
        let mut b = tester.start_touch(1, (220.0, 200.0));
        tester.pump(FRAME);
        tester.move_to(&mut a, (140.0, 200.0));
        let moved = tester.move_to(&mut b, (260.0, 200.0));
        assert!(!moved.handled, "slot 1 reaches no widget");
        tester.up(b);
        tester.up(a);
        assert_eq!(
            *seen.borrow(),
            [
                (PointerId::MOUSE, PointerPhase::Down),
                (PointerId::MOUSE, PointerPhase::Move),
                (PointerId::MOUSE, PointerPhase::Up),
            ]
        );
    }

    /// A long-press detector over a pan/zoom view, which opts into the gesture's other contacts.
    fn held_over_pan_zoom(_: &mut Log) -> GestureDetectorView<Log> {
        crate::GestureDetector(crate::pan_zoom(pad(400.0)))
            .on_long_press(|s: &mut Log| s.log.push("long"))
            .on_tap_cancel(|s: &mut Log| s.log.push("cancel"))
    }

    /// Pump 16 ms frames until the clock has passed `ms` since `start`.
    fn pump_past<S: 'static, V: View<S>>(tester: &mut Tester<S, V>, start: FrameTime, ms: u64) {
        while tester.frame_time().saturating_sub(start) < Duration::from_millis(ms) {
            tester.pump(FRAME);
        }
    }

    #[test]
    fn two_resting_fingers_never_long_press() {
        let mut tester = Tester::new(Log::default(), held_over_pan_zoom, Size::new(400.0, 400.0));
        let start = tester.frame_time();
        let a = tester.start_touch(0, (180.0, 200.0));
        let b = tester.start_touch(1, (220.0, 200.0));
        assert_eq!(
            tester.state().log,
            ["cancel"],
            "the second finger ends the press"
        );
        pump_past(&mut tester, start, 600);
        tester.up(b);
        tester.up(a);
        tester.pump(FRAME);
        assert_eq!(
            tester.state().log,
            ["cancel"],
            "no long press, and cancel exactly once"
        );
    }

    #[test]
    fn one_finger_hold_still_long_presses() {
        let mut tester = Tester::new(Log::default(), held_over_pan_zoom, Size::new(400.0, 400.0));
        let start = tester.frame_time();
        let a = tester.start_touch(0, (180.0, 200.0));
        pump_past(&mut tester, start, 600);
        assert_eq!(tester.state().log, ["long"]);
        tester.up(a);
        tester.pump(FRAME);
        assert_eq!(tester.state().log, ["long"], "the release cancels nothing");
    }

    #[test]
    fn a_bare_detector_never_hears_a_second_finger_so_the_press_runs_on() {
        // Nothing below the detector opts into the gesture's other contacts, so the root drops
        // the second finger: no callback for it, no cancel, and the first finger long-presses.
        let mut tester = Tester::new(Log::default(), tap_detector, SIZE);
        let start = tester.frame_time();
        let a = tester.start_touch(0, (50.0, 50.0));
        let mut b = tester.start_touch(1, (90.0, 50.0));
        assert_eq!(
            tester.state().log,
            ["down"],
            "the second Down reaches nothing"
        );
        tester.pump(FRAME);
        let moved = tester.move_to(&mut b, (100.0, 50.0));
        assert!(!moved.handled, "slot 1 reaches no widget");
        pump_past(&mut tester, start, 600);
        tester.up(b);
        assert_eq!(tester.state().log, ["down", "long"]);
        tester.up(a);
        assert_eq!(tester.state().log, ["down", "long", "up"]);
    }

    #[test]
    fn a_finder_target_resolves_to_the_centre_of_its_match() {
        #[derive(Default)]
        struct Pressed(Option<Point>);
        let mut tester = Tester::new(
            Pressed::default(),
            |_: &mut Pressed| {
                crate::GestureDetector(crate::button("Go", |_: &mut Pressed| {}))
                    .on_tap_down(|s: &mut Pressed, d: TapDownDetails| s.0 = Some(d.local_position))
            },
            SIZE,
        );
        let button = tester.bounds(&find::label(Role::Button, "Go"));
        let press = tester.press(find::label(Role::Button, "Go"));
        assert_eq!(press.position(), button.center());
        assert_eq!(press.pointer(), PointerId::MOUSE);
        tester.up(press);
        assert!(tester.state().0.is_some());
    }

    struct Nav {
        nav: NavigatorController<Nav>,
    }

    fn home() -> crate::ButtonView<Nav> {
        crate::button("Open", |s: &mut Nav| s.nav.push(|| crate::text("details")))
    }

    #[test]
    fn a_tap_pushes_a_page_and_request_back_returns_to_the_first() {
        let mut tester = Tester::new(
            Nav {
                nav: NavigatorController::new(),
            },
            |s: &mut Nav| frust_widgets::navigator(&s.nav, home),
            SIZE,
        );
        tester.tap(find::label(Role::Button, "Open"));
        tester.pump_and_settle();
        assert_eq!(tester.find(&find::text("details")).len(), 1);
        assert_eq!(tester.state().nav.depth(), 2);

        tester.state().nav.request_back();
        tester.pump_and_settle();
        assert!(tester.find(&find::text("details")).is_empty());
        assert_eq!(tester.find(&find::label(Role::Button, "Open")).len(), 1);
    }

    #[derive(Default)]
    struct Scrolled {
        events: Vec<String>,
    }

    /// A detector inside a vertical scroll view of content taller than the
    /// viewport, with the follow-up callbacks set when `follow` is.
    fn scrolled_detector(
        controller: ScrollController,
        follow: bool,
    ) -> impl FnMut(&mut Scrolled) -> crate::ScrollView<Scrolled> {
        move |_: &mut Scrolled| {
            let mut detector = crate::GestureDetector(pad(200.0))
                .on_long_press(|s: &mut Scrolled| s.events.push("long".into()));
            if follow {
                detector = detector
                    .on_long_press_move_update(|s: &mut Scrolled, d: LongPressMoveDetails| {
                        s.events
                            .push(format!("move {}", d.offset_from_origin.y as i32))
                    })
                    .on_long_press_end(|s: &mut Scrolled, _: LongPressEndDetails| {
                        s.events.push("end".into())
                    })
                    .on_long_press_cancel(|s: &mut Scrolled| s.events.push("cancel".into()));
            }
            scroll_view(
                crate::column()
                    .child(detector)
                    .child(pad(200.0))
                    .child(pad(200.0))
                    .child(pad(200.0)),
            )
            .controller(controller.clone())
        }
    }

    #[test]
    fn a_long_press_drag_reaches_the_detector_and_does_not_scroll() {
        let controller = ScrollController::new();
        let mut tester = Tester::new(
            Scrolled::default(),
            scrolled_detector(controller.clone(), true),
            SIZE,
        );
        tester.long_press_then_drag((100.0, 20.0), Point::new(100.0, 220.0));
        let events = &tester.state().events;
        assert_eq!(events.first().map(String::as_str), Some("long"));
        assert_eq!(events.last().map(String::as_str), Some("end"));
        let moves: Vec<_> = events.iter().filter(|e| e.starts_with("move")).collect();
        assert!(moves.len() >= 10, "moves arrive: {events:?}");
        assert_eq!(moves.last().map(|m| m.as_str()), Some("move 200"));
        assert!(!events.iter().any(|e| e == "cancel"), "{events:?}");
        assert_eq!(controller.offset(), 0.0, "the scroll view stayed put");
    }

    #[test]
    fn a_drag_before_the_threshold_scrolls_and_fires_no_long_press_callbacks() {
        let controller = ScrollController::new();
        let mut tester = Tester::new(
            Scrolled::default(),
            scrolled_detector(controller.clone(), true),
            SIZE,
        );
        tester.drag((100.0, 150.0), Vec2::new(0.0, -100.0));
        tester.pump(FRAME);
        assert!(controller.offset() > 0.0, "the scroll view scrolled");
        assert!(
            tester.state().events.is_empty(),
            "{:?}",
            tester.state().events
        );
    }

    #[test]
    fn a_long_press_drag_with_only_on_long_press_still_scrolls() {
        let controller = ScrollController::new();
        let mut tester = Tester::new(
            Scrolled::default(),
            scrolled_detector(controller.clone(), false),
            SIZE,
        );
        tester.long_press_then_drag((100.0, 150.0), Point::new(100.0, 20.0));
        assert_eq!(tester.state().events, ["long"]);
        assert!(controller.offset() > 0.0, "the scroll view took the drag");
    }

    #[test]
    fn a_long_press_in_a_scrolled_view_reports_the_window_position_it_was_sent() {
        #[derive(Default)]
        struct Globals {
            moves: Vec<Point>,
            end: Option<Point>,
        }
        let controller = ScrollController::new();
        let ctl = controller.clone();
        let mut tester = Tester::new(
            Globals::default(),
            move |_: &mut Globals| {
                let detector = crate::GestureDetector(pad(200.0))
                    .on_long_press_move_update(|s: &mut Globals, d: LongPressMoveDetails| {
                        s.moves.push(d.global_position)
                    })
                    .on_long_press_end(|s: &mut Globals, d: LongPressEndDetails| {
                        s.end = Some(d.global_position)
                    });
                scroll_view(
                    crate::column()
                        .child(pad(200.0))
                        .child(detector)
                        .child(pad(200.0)),
                )
                .controller(ctl.clone())
            },
            SIZE,
        );
        controller.jump_to(120.0);
        tester.pump(FRAME);
        assert_eq!(controller.offset(), 120.0);
        // The detector sits at window y = 200 - 120 = 80.
        let mut gesture = tester.start_gesture(Point::new(60.0, 100.0));
        let threshold = Duration::from_millis(LONG_PRESS_MS) + FRAME;
        let start = tester.frame_time();
        while tester.frame_time().saturating_sub(start) < threshold {
            tester.pump(FRAME);
        }
        tester.pump(FRAME);
        tester.move_to(&mut gesture, Point::new(70.0, 150.0));
        tester.move_to(&mut gesture, Point::new(90.0, 170.0));
        tester.up(gesture);
        assert_eq!(
            tester.state().moves,
            [Point::new(70.0, 150.0), Point::new(90.0, 170.0)]
        );
        assert_eq!(tester.state().end, Some(Point::new(90.0, 170.0)));
    }

    /// One event the [`Recorder`] saw: its kind, the held modifiers and the hover flag.
    type Seen = (String, Modifiers, bool);

    /// A leaf that records every event with the modifiers held and `is_hovered`, and claims
    /// hover on an uncaptured `Move`.
    struct Recorder(Rc<RefCell<Vec<Seen>>>);

    impl<S: 'static> View<S> for Recorder {
        type Element = Recorder;

        fn build(&self, _ctx: &mut BuildCtx<'_>) -> Recorder {
            Recorder(Rc::clone(&self.0))
        }

        fn rebuild(
            &self,
            _prev: &Self,
            _el: &mut Recorder,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for Recorder {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(200.0, 200.0))
        }

        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}

        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let kind = match event {
                InputEvent::Scroll { position, delta } => format!("scroll {position:?} {delta:?}"),
                InputEvent::Pointer(p) => format!("{:?}", p.phase),
                _ => return EventResult::Ignored,
            };
            self.0
                .borrow_mut()
                .push((kind, ctx.modifiers(), ctx.is_hovered()));
            if matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Move) {
                ctx.claim_hover();
            }
            EventResult::Handled
        }
    }

    fn recorder_tester() -> (Tester<(), impl View<()>>, Rc<RefCell<Vec<Seen>>>) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let probe = Rc::clone(&seen);
        let tester = Tester::new((), move |_: &mut ()| Recorder(Rc::clone(&probe)), SIZE);
        (tester, seen)
    }

    #[test]
    fn scroll_at_delivers_a_scroll_at_the_target_point() {
        let (mut tester, seen) = recorder_tester();
        let outcome = tester.scroll_at((30.0, 40.0), ScrollDelta::Lines(0.0, -3.0));
        assert!(outcome.handled);
        tester.scroll_at(Point::new(60.0, 70.0), ScrollDelta::Pixels(5.0, 12.5));
        let (point, pixels) = {
            let kinds: Vec<_> = seen.borrow().iter().map(|s| s.0.clone()).collect();
            (kinds[0].clone(), kinds[1].clone())
        };
        assert_eq!(point, "scroll (30.0, 40.0) Lines(0.0, -3.0)");
        assert_eq!(pixels, "scroll (60.0, 70.0) Pixels(5.0, 12.5)");

        let (mut tester, seen) = recorder_tester();
        let centre = tester.find(&find::by_type::<Recorder>());
        let at = centre.first().map(|n| n.bounds.center()).unwrap();
        tester.scroll_at(find::by_type::<Recorder>(), ScrollDelta::Lines(1.0, 0.0));
        assert_eq!(
            seen.borrow()[0].0,
            format!("scroll {at:?} Lines(1.0, 0.0)"),
            "a finder target scrolls at its centre"
        );
    }

    #[test]
    fn scroll_at_scrolls_a_scroll_view_after_a_pump() {
        let controller = ScrollController::new();
        let mut tester = Tester::new(
            Scrolled::default(),
            scrolled_detector(controller.clone(), false),
            SIZE,
        );
        tester.scroll_at((100.0, 100.0), ScrollDelta::Pixels(0.0, 60.0));
        tester.pump(FRAME);
        assert!(controller.offset() > 0.0, "offset {}", controller.offset());
    }

    #[test]
    fn hover_sends_an_uncaptured_move_that_can_claim_hover() {
        let (mut tester, seen) = recorder_tester();
        let outcome = tester.hover((50.0, 50.0));
        assert!(outcome.handled);
        tester.pump(FRAME);
        tester.hover((60.0, 50.0));
        let seen = seen.borrow();
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|s| s.0 == "Move"), "{seen:?}");
        assert!(!seen[0].2, "not hovered before the first claim");
        assert!(seen[1].2, "hovered after the claim painted");
    }

    #[test]
    fn set_modifiers_holds_for_every_following_event() {
        let (mut tester, seen) = recorder_tester();
        assert_eq!(tester.modifiers(), Modifiers::default());
        tester.set_modifiers(Modifiers::SHIFT);
        assert_eq!(tester.modifiers(), Modifiers::SHIFT);
        tester.hover((10.0, 10.0));
        tester.scroll_at((10.0, 10.0), ScrollDelta::Lines(0.0, 1.0));
        tester.tap((10.0, 10.0));
        assert!(
            seen.borrow().iter().all(|s| s.1 == Modifiers::SHIFT),
            "{:?}",
            seen.borrow()
        );
        tester.set_modifiers(Modifiers::default());
        tester.hover((11.0, 10.0));
        assert_eq!(seen.borrow().last().unwrap().1, Modifiers::default());
    }

    #[test]
    fn with_modifiers_restores_the_previous_chord() {
        let (mut tester, seen) = recorder_tester();
        tester.set_modifiers(Modifiers::SHIFT);
        let inner = tester.with_modifiers(Modifiers::ALT, |t| {
            t.hover((10.0, 10.0));
            t.modifiers()
        });
        assert_eq!(inner, Modifiers::ALT);
        assert_eq!(tester.modifiers(), Modifiers::SHIFT);
        tester.hover((12.0, 10.0));
        let seen = seen.borrow();
        assert_eq!(seen[0].1, Modifiers::ALT);
        assert_eq!(seen[1].1, Modifiers::SHIFT);
    }

    #[test]
    fn an_alt_drag_reports_alt_in_its_drag_details() {
        #[derive(Default)]
        struct Held(Vec<Modifiers>);
        let mut tester = Tester::new(
            Held::default(),
            |_: &mut Held| {
                crate::GestureDetector(pad(200.0))
                    .on_pan_update(|s: &mut Held, d: DragUpdateDetails| s.0.push(d.modifiers))
            },
            SIZE,
        );
        tester.with_modifiers(Modifiers::ALT, |t| {
            t.drag((100.0, 20.0), Vec2::new(0.0, 60.0))
        });
        let held = &tester.state().0;
        assert!(!held.is_empty());
        assert!(held.iter().all(|m| *m == Modifiers::ALT), "{held:?}");
    }
}
