//! Kept-mounted enter/exit presence: [`Presence`], the headless driver, and
//! [`presence(open, child)`](presence), the single-child view built on it.
//!
//! # Why kept-mounted
//!
//! A view is gone the frame after the rebuild that stopped returning it, and
//! the framework keeps no absent view alive (a pod lives exactly as long as
//! its owner returns it). An exit ramp therefore needs the owner to keep the
//! child mounted and hand it an `open` flag instead: the child stays built and
//! laid out while closed, plays its exit when the flag drops, and reports
//! [`on_exited`](PresenceView::on_exited) once that exit has settled, so the
//! owner may then stop returning it. The owner always owns `open`; nothing
//! here flips it.
//!
//! # Timing
//!
//! [`Timing::ThemeDefault`] (the default) resolves on the first paint after a
//! change: the entrance runs the theme's `durations.base` with
//! `easing.effects`, the exit `durations.fast` with `easing.exit`. Under
//! `reduce_motion` both become the theme's reduced fade
//! ([`MotionExtras::reduced_fade_duration`]), linear, whatever timing was set;
//! geometry is pinned at `1.0` while visible and only opacity fades.
//!
//! # Reversal
//!
//! Flipping `open` mid-ramp starts the opposite ramp from the value painted
//! last. A duration timing is value-continuous (an `animate_to` from there);
//! a spring timing is also velocity-continuous (a `spring_to` carrying the
//! current velocity), so a quick open-close-open reads as one motion.
//!
//! # Completion
//!
//! The exit settles during paint, which has no `&mut State`. The settling
//! paint raises a drain-once latch and [`frust_core::mark_pending_result_flush`];
//! the widget's next event (that frame's `Housekeeping` flush, with no input
//! required) drains it and fires `on_exited`.
//!
//! `frust_beui::motion::Presence` is the catalog-local predecessor of this
//! driver; it keeps its own ramp vocabulary and is not replaced by this one.

use std::rc::Rc;
use std::time::Duration;

use frust_core::event::{FileDropEvent, FileDropPhase};
use frust_core::{
    AnimationController, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve,
    EditingState, EventCtx, EventResult, FrameTime, ImeState, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, SemanticsCtx, SpringDesc, View, Widget, any,
};
use frust_theme::{MotionExtras, MotionScheme, Theme};
use kurbo::{Point, Size};

use crate::Timing;
use crate::authoring::ErasedCallback;

/// Where a [`Presence`] is in its lifecycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresencePhase {
    /// Closed and settled: nothing painted, no frames requested. The starting
    /// phase, and where an exit ends.
    #[default]
    Absent,
    /// Open and playing its entrance.
    Entering,
    /// Open and settled at full presence.
    Present,
    /// Closed but still visible while its exit plays.
    Exiting,
}

impl PresencePhase {
    /// Whether anything is painted: every phase but [`Absent`](Self::Absent).
    pub const fn is_visible(self) -> bool {
        !matches!(self, PresencePhase::Absent)
    }

    /// Whether a ramp is running, so the stepping widget owes another frame.
    pub const fn is_animating(self) -> bool {
        matches!(self, PresencePhase::Entering | PresencePhase::Exiting)
    }

    /// Whether the owner's flag is open ([`Entering`](Self::Entering) or
    /// [`Present`](Self::Present)): the phases that take input.
    pub const fn is_open(self) -> bool {
        matches!(self, PresencePhase::Entering | PresencePhase::Present)
    }
}

/// One paint's reading of a [`Presence`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PresenceSample {
    /// How present the content is, clamped to `0.0..=1.0`: the alpha to paint at.
    pub opacity: f64,
    /// Raw progress for position or scale, unclamped (a spring may overshoot).
    /// Pinned at `1.0` while visible under `reduce_motion`, `0.0` once absent.
    pub geometry: f64,
    /// The phase after this step.
    pub phase: PresencePhase,
}

/// The value one ramp drives: `from + span * controller.value()`.
///
/// A duration ramp runs its controller `0 → 1` over a tween of `span = target -
/// from`; a spring ramp runs it from `0` to `target - from` with `span = 1`, so
/// the spring sees displacement and velocity in value units.
#[derive(Clone, Copy, Debug)]
struct Motion {
    from: f64,
    span: f64,
    controller: AnimationController,
}

impl Motion {
    /// A motion settled at `value`.
    fn rest(value: f64) -> Self {
        Motion {
            from: value,
            span: 0.0,
            controller: AnimationController::new(Duration::ZERO),
        }
    }

    /// A ramp from `from` (moving at `velocity`) to `target` under a resolved
    /// `timing`. A duration ramp keeps the value continuous; a spring keeps the
    /// velocity continuous too.
    fn start(from: f64, velocity: f64, target: f64, timing: Timing) -> Self {
        match timing {
            Timing::Spring(spring) => {
                let mut controller = AnimationController::new(Duration::ZERO);
                controller.spring_to_with_velocity(
                    target - from,
                    velocity,
                    SpringDesc::from(spring),
                );
                Motion {
                    from,
                    span: 1.0,
                    controller,
                }
            }
            Timing::Duration(duration, curve) => Self::eased(from, target, duration, curve),
            // Resolved before a ramp starts; unthemed tokens if one slips through.
            Timing::ThemeDefault => {
                Self::start(from, velocity, target, theme_timing(None, target >= from))
            }
        }
    }

    fn eased(from: f64, target: f64, duration: Duration, curve: Curve) -> Self {
        let mut controller = AnimationController::new(duration).with_curve(curve);
        controller.forward();
        Motion {
            from,
            span: target - from,
            controller,
        }
    }

    fn value(&self) -> f64 {
        self.from + self.span * self.controller.value()
    }

    fn velocity(&self) -> f64 {
        self.span * self.controller.velocity()
    }

    fn is_moving(&self) -> bool {
        self.controller.is_animating()
    }
}

/// The headless enter/exit driver described in the [module docs](self).
///
/// A consumer calls [`set_open`](Self::set_open) when the owner's flag may have
/// changed, [`advance`](Self::advance) once per paint on
/// [`PaintCtx::frame_time`], and [`take_exited`](Self::take_exited) from its
/// event pass.
#[derive(Clone, Debug)]
pub struct Presence {
    enter: Timing,
    exit: Timing,
    phase: PresencePhase,
    motion: Motion,
    /// A phase change waits for the next `advance` to resolve its timing.
    pending: bool,
    /// Whether the current ramp was started under `reduce_motion`.
    reduced: bool,
    exited: bool,
}

impl Default for Presence {
    fn default() -> Self {
        Self::new()
    }
}

impl Presence {
    /// A closed presence timed from the theme both ways.
    pub fn new() -> Self {
        Self::with_timing(Timing::ThemeDefault, Timing::ThemeDefault)
    }

    /// A closed presence using `enter` on the way in and `exit` on the way out.
    pub fn with_timing(enter: Timing, exit: Timing) -> Self {
        Presence {
            enter,
            exit,
            phase: PresencePhase::Absent,
            motion: Motion::rest(0.0),
            pending: false,
            reduced: false,
            exited: false,
        }
    }

    /// Replace both timings; a running ramp keeps its own, the next one uses these.
    pub fn set_timing(&mut self, enter: Timing, exit: Timing) {
        self.enter = enter;
        self.exit = exit;
    }

    /// The current phase.
    pub fn phase(&self) -> PresencePhase {
        self.phase
    }

    /// Whether anything is painted (true through the whole exit).
    pub fn is_visible(&self) -> bool {
        self.phase.is_visible()
    }

    /// Whether a ramp is running, so the stepping widget owes another frame.
    pub fn is_animating(&self) -> bool {
        self.phase.is_animating()
    }

    /// The driven value's rate of change, in progress units per second of
    /// animation time, as of the last [`advance`](Self::advance).
    pub fn velocity(&self) -> f64 {
        self.motion.velocity()
    }

    /// Apply the owner's flag, returning whether it changed the phase.
    ///
    /// Re-applying the flag already followed is a no-op, so a rebuild may pass
    /// the same value every frame. Opening from `Absent`/`Exiting` enters;
    /// closing from `Entering`/`Present` exits. The ramp itself starts on the
    /// next [`advance`](Self::advance), from the value painted last.
    pub fn set_open(&mut self, open: bool) -> bool {
        if open == self.phase.is_open() {
            return false;
        }
        self.phase = if open {
            PresencePhase::Entering
        } else {
            PresencePhase::Exiting
        };
        if open {
            // A reopen before the exit was drained means no exit happened.
            self.exited = false;
        }
        self.pending = true;
        true
    }

    /// Step to `now` (the scaled animation clock), resolving theme timings and
    /// `reduce_motion` against `theme`.
    ///
    /// A settling entrance becomes [`Present`](PresencePhase::Present); a
    /// settling exit becomes [`Absent`](PresencePhase::Absent) and raises the
    /// latch [`take_exited`](Self::take_exited) drains. A `reduce_motion` flip
    /// re-drives an in-flight ramp under the other timing.
    pub fn advance(&mut self, now: FrameTime, theme: Option<&Theme>) -> PresenceSample {
        let reduced = theme.is_some_and(|theme| theme.motion.reduce_motion);
        if reduced != self.reduced && self.motion.is_moving() {
            self.pending = true;
        }
        self.reduced = reduced;
        if self.pending {
            self.pending = false;
            let target = if self.phase.is_open() { 1.0 } else { 0.0 };
            let timing = self.resolve(theme, reduced);
            self.motion =
                Motion::start(self.motion.value(), self.motion.velocity(), target, timing);
        }
        let moving = self.motion.controller.advance(now);
        if !moving {
            match self.phase {
                PresencePhase::Entering => self.phase = PresencePhase::Present,
                PresencePhase::Exiting => {
                    self.phase = PresencePhase::Absent;
                    self.exited = true;
                }
                PresencePhase::Absent | PresencePhase::Present => {}
            }
        }
        self.sample()
    }

    /// The current reading without stepping the clock — for a second paint-time
    /// consumer of the same driver.
    pub fn sample(&self) -> PresenceSample {
        let value = self.motion.value();
        let visible = self.phase.is_visible();
        let geometry = match (visible, self.reduced) {
            (false, _) => 0.0,
            (true, true) => 1.0,
            (true, false) => value,
        };
        PresenceSample {
            opacity: if visible { value.clamp(0.0, 1.0) } else { 0.0 },
            geometry,
            phase: self.phase,
        }
    }

    /// Drain the "exit settled" latch: `true` exactly once per completed exit.
    pub fn take_exited(&mut self) -> bool {
        std::mem::replace(&mut self.exited, false)
    }

    /// Whether a completed exit waits to be drained, without draining it.
    pub fn has_exited(&self) -> bool {
        self.exited
    }

    /// The concrete timing for the ramp about to start.
    fn resolve(&self, theme: Option<&Theme>, reduced: bool) -> Timing {
        if reduced {
            let fade = MotionExtras::of(theme).reduced_fade_duration();
            return Timing::Duration(fade, Curve::Linear);
        }
        let entering = self.phase.is_open();
        match if entering { self.enter } else { self.exit } {
            Timing::ThemeDefault => theme_timing(theme, entering),
            explicit => explicit,
        }
    }
}

/// The theme's entrance (`base` + `effects`) or exit (`fast` + `exit`) timing,
/// falling back to [`MotionScheme::neutral`] unthemed or for an unusable value.
fn theme_timing(theme: Option<&Theme>, entering: bool) -> Timing {
    let neutral = MotionScheme::neutral();
    let scheme = theme.map_or(&neutral, |theme| &theme.motion);
    let (ms, fallback_ms, curve) = if entering {
        (
            scheme.durations.base,
            neutral.durations.base,
            scheme.easing.effects,
        )
    } else {
        (
            scheme.durations.fast,
            neutral.durations.fast,
            scheme.easing.exit,
        )
    };
    let ms = if ms.is_finite() && ms >= 0.0 {
        ms
    } else {
        fallback_ms
    };
    Timing::Duration(Duration::from_secs_f64(ms / 1000.0), curve)
}

/// The inactive IME surface published when a close severs a focused child
/// (mirrors `motion::switcher`'s).
fn cleared_ime_state() -> ImeState {
    ImeState {
        active: false,
        editing: EditingState {
            text: String::new(),
            selection_base: -1,
            selection_extent: -1,
            composing_base: -1,
            composing_extent: -1,
        },
        caret: None,
        content_type: Default::default(),
        suppress_soft_keyboard: false,
    }
}

/// Events forwarded even while closed: tree bookkeeping, not user input.
fn is_bookkeeping(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Housekeeping
            | InputEvent::FileDrop(FileDropEvent {
                phase: FileDropPhase::Ended,
                ..
            })
    )
}

/// A view-held `on_exited` callback (erased to an `ErasedCallback` on build).
type OnExited<State> = Rc<dyn Fn(&mut State)>;

/// A kept-mounted enter/exit wrapper. See the [module docs](self).
pub struct PresenceView<State: 'static> {
    open: bool,
    enter: Timing,
    exit: Timing,
    on_exited: Option<OnExited<State>>,
    child: AnyView<State>,
}

/// Show `child` while `open`, fading it in and out — see the [module docs](self).
///
/// The child is always built and laid out (the wrapper reports its size). It
/// is painted only while visible, takes input and focus only while open, and
/// contributes semantics only while visible. Mounting with `open` set plays the
/// entrance.
pub fn presence<State: 'static, V: View<State>>(open: bool, child: V) -> PresenceView<State> {
    PresenceView {
        open,
        enter: Timing::ThemeDefault,
        exit: Timing::ThemeDefault,
        on_exited: None,
        child: any(child),
    }
}

impl<State: 'static> PresenceView<State> {
    /// Override the entrance and exit timings (default: both
    /// [`Timing::ThemeDefault`]). Ignored under `reduce_motion`.
    pub fn timing(mut self, enter: Timing, exit: Timing) -> Self {
        self.enter = enter;
        self.exit = exit;
        self
    }

    /// Run `f` once each exit has settled — the owner's cue to stop returning
    /// this view. Delivered on the next event pass after the settling paint.
    pub fn on_exited(mut self, f: impl Fn(&mut State) + 'static) -> Self {
        self.on_exited = Some(Rc::new(f));
        self
    }
}

/// The retained widget for a [`PresenceView`].
pub struct PresenceWidget {
    child: ChildPod,
    driver: Presence,
    on_exited: Option<ErasedCallback>,
    /// A close severed a focused child: publish a cleared IME surface next paint.
    needs_ime_clear: bool,
}

impl PresenceWidget {
    /// The driver, for inspection.
    pub fn driver(&self) -> &Presence {
        &self.driver
    }
}

impl<State: 'static> View<State> for PresenceView<State> {
    type Element = PresenceWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PresenceWidget {
        let mut driver = Presence::with_timing(self.enter, self.exit);
        driver.set_open(self.open);
        PresenceWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            driver,
            on_exited: self
                .on_exited
                .as_ref()
                .map(crate::authoring::erase_callback),
            needs_ime_clear: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PresenceWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.enter != self.enter || prev.exit != self.exit {
            element.driver.set_timing(self.enter, self.exit);
        }
        if element.driver.set_open(self.open) {
            flags |= ChangeFlags::PAINT;
            if !self.open {
                suppress_routing(element, ctx);
            }
        }
        element.on_exited = self
            .on_exited
            .as_ref()
            .map(crate::authoring::erase_callback);
        flags |= crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut PresenceWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

/// The container-suppresses-routing sequence for a child a close made inert:
/// cancel its capture, clear its focus, then (only if that severed the live
/// session) publish a cleared IME surface on the next paint.
fn suppress_routing(element: &mut PresenceWidget, ctx: &BuildCtx<'_>) {
    if element.child.is_active() {
        crate::authoring::cancel_pod(&mut element.child);
        element.child.set_active(false);
    }
    if element.child.is_focused() {
        element.child.set_focused(false);
        if ctx.has_focus() {
            element.needs_ime_clear = true;
        }
    }
}

impl Widget for PresenceWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let latched = self.driver.has_exited();
        let sample = self.driver.advance(ctx.frame_time(), theme);
        if std::mem::take(&mut self.needs_ime_clear) {
            ctx.publish_ime_state(cleared_ime_state());
        }
        if sample.phase.is_animating() {
            ctx.request_frame();
        }
        if !latched && self.driver.has_exited() && self.on_exited.is_some() {
            // Paint has no `&mut State`: owe `on_exited` to the next frame's
            // Housekeeping flush.
            frust_core::mark_pending_result_flush();
            ctx.request_frame();
        }
        if !sample.phase.is_visible() {
            return;
        }
        if sample.opacity >= 1.0 {
            self.child.paint_child(ctx, scene);
            return;
        }
        scene.push_layer(ctx.origin(), ctx.size(), sample.opacity as f32);
        self.child.paint_child(ctx, scene);
        scene.pop_layer();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if self.driver.take_exited()
            && let Some(on_exited) = &mut self.on_exited
        {
            on_exited(ctx);
            ctx.request_redraw();
        }
        if self.driver.phase().is_open() || is_bookkeeping(event) {
            return crate::authoring::route_event_single(&mut self.child, ctx, event);
        }
        EventResult::Ignored
    }

    fn visit_focus_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        if self.driver.phase().is_open() {
            visitor(&self.child);
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.driver.is_visible() {
            self.child.semantics_child(ctx);
        }
    }

    crate::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use frust_core::{PointerButton, PointerEvent, PointerPhase, RenderRoot};
    use frust_theme::{EasingSet, MotionDurations, MotionSpring};

    use super::*;
    use crate::test_support::RecordingScene;

    fn at(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn linear(ms: u64) -> Timing {
        Timing::Duration(Duration::from_millis(ms), Curve::Linear)
    }

    fn timed() -> Presence {
        Presence::with_timing(linear(200), linear(100))
    }

    /// Step `driver` from `start` in 16 ms frames until it stops animating.
    fn settle(driver: &mut Presence, start: f64, theme: Option<&Theme>) -> f64 {
        let mut now = start;
        driver.advance(at(now), theme);
        for _ in 0..1000 {
            if !driver.is_animating() {
                break;
            }
            now += 16.0;
            driver.advance(at(now), theme);
        }
        now
    }

    #[test]
    fn set_open_twice_is_a_no_op() {
        let mut driver = timed();
        assert!(!driver.set_open(false), "closing a closed presence");
        assert!(driver.set_open(true));
        assert!(!driver.set_open(true));
        driver.advance(at(0.0), None);
        let mid = driver.advance(at(100.0), None);
        assert!(
            !driver.set_open(true),
            "a rebuild re-passing true changes nothing"
        );
        assert_eq!(driver.advance(at(100.0), None), mid, "and does not restart");
        assert_eq!(driver.phase(), PresencePhase::Entering);
    }

    #[test]
    fn enter_then_exit_reach_present_and_absent() {
        let mut driver = timed();
        assert_eq!(driver.advance(at(0.0), None).phase, PresencePhase::Absent);
        driver.set_open(true);
        let seed = driver.advance(at(0.0), None);
        assert_eq!((seed.phase, seed.opacity), (PresencePhase::Entering, 0.0));
        let mid = driver.advance(at(100.0), None);
        assert!((mid.opacity - 0.5).abs() < 1e-9, "{mid:?}");
        let done = driver.advance(at(200.0), None);
        assert_eq!(done.phase, PresencePhase::Present);
        assert_eq!((done.opacity, done.geometry), (1.0, 1.0));
        assert!(!driver.is_animating());

        driver.set_open(false);
        driver.advance(at(200.0), None);
        let out = driver.advance(at(250.0), None);
        assert_eq!(out.phase, PresencePhase::Exiting);
        assert!(driver.is_visible() && (out.opacity - 0.5).abs() < 1e-9);
        let gone = driver.advance(at(300.0), None);
        assert_eq!(gone.phase, PresencePhase::Absent);
        assert_eq!((gone.opacity, gone.geometry), (0.0, 0.0));
        assert!(!driver.is_visible() && !driver.is_animating());
    }

    #[test]
    fn exit_completion_latches_once() {
        let mut driver = timed();
        driver.set_open(true);
        let now = settle(&mut driver, 0.0, None);
        assert!(!driver.has_exited(), "an entrance raises nothing");
        driver.set_open(false);
        driver.advance(at(now), None);
        driver.advance(at(now + 50.0), None);
        assert!(!driver.take_exited(), "not before the exit settles");
        settle(&mut driver, now + 50.0, None);
        assert!(driver.has_exited());
        assert!(driver.take_exited(), "reported once");
        assert!(!driver.take_exited(), "and not again");
        driver.advance(at(now + 500.0), None);
        assert!(
            !driver.has_exited(),
            "a settled absent paint raises nothing"
        );
    }

    #[test]
    fn an_interrupted_exit_reports_nothing() {
        let mut driver = timed();
        driver.set_open(true);
        let now = settle(&mut driver, 0.0, None);
        driver.set_open(false);
        driver.advance(at(now), None);
        driver.advance(at(now + 50.0), None);
        driver.set_open(true);
        settle(&mut driver, now + 50.0, None);
        assert_eq!(driver.phase(), PresencePhase::Present);
        assert!(!driver.take_exited());
    }

    #[test]
    fn duration_reversal_is_value_continuous() {
        let mut driver = timed();
        driver.set_open(true);
        driver.advance(at(0.0), None);
        let before = driver.advance(at(100.0), None).opacity;
        assert!((before - 0.5).abs() < 1e-9);

        assert!(driver.set_open(false));
        let after = driver.advance(at(100.0), None);
        assert_eq!(after.phase, PresencePhase::Exiting);
        assert!(
            (after.opacity - before).abs() < 1e-12,
            "no jump: {before} -> {after:?}"
        );
        // The exit runs from there to zero over its own 100 ms.
        let mid = driver.advance(at(150.0), None).opacity;
        assert!((mid - 0.25).abs() < 1e-9, "{mid}");
        assert_eq!(driver.advance(at(200.0), None).phase, PresencePhase::Absent);
    }

    #[test]
    fn spring_reversal_is_velocity_continuous() {
        let spring = Timing::Spring(MotionSpring {
            damping_ratio: 1.0,
            stiffness: 200.0,
        });
        let mut driver = Presence::with_timing(spring, spring);
        driver.set_open(true);
        for ms in [0.0, 16.0, 32.0, 48.0] {
            driver.advance(at(ms), None);
        }
        let value = driver.sample().geometry;
        let velocity = driver.velocity();
        assert!(
            value > 0.0 && value < 1.0 && velocity > 0.0,
            "{value} {velocity}"
        );

        assert!(driver.set_open(false));
        let seed = driver.advance(at(48.0), None);
        assert!((seed.geometry - value).abs() < 1e-12, "no value jump");
        assert!(
            (driver.velocity() - velocity).abs() < 1e-9,
            "no velocity jump"
        );

        // One millisecond on, the exit is still carried the entrance's way.
        let next = driver.advance(at(49.0), None);
        let carried = driver.velocity();
        assert!(
            next.geometry > value,
            "still moving up: {value} -> {}",
            next.geometry
        );
        assert!(carried > 0.0, "same sign: {carried}");
        assert!(
            (carried - velocity).abs() < 0.1 * velocity,
            "{velocity} -> {carried}"
        );

        settle(&mut driver, 49.0, None);
        assert_eq!(driver.phase(), PresencePhase::Absent);
        assert!(driver.take_exited());
    }

    #[test]
    fn reduce_motion_fades_over_the_reduced_fade_with_geometry_pinned() {
        let theme = Theme::builder(Theme::neutral())
            .map_motion(|motion| MotionScheme {
                reduce_motion: true,
                ..motion
            })
            .extension(MotionExtras {
                reduced_fade: 200.0,
                ..MotionExtras::neutral()
            })
            .build();
        let theme = Some(&theme);
        let spring = Timing::Spring(MotionSpring {
            damping_ratio: 0.5,
            stiffness: 300.0,
        });
        // An explicit spring is ignored while reduced: the fade is linear.
        let mut driver = Presence::with_timing(spring, spring);
        driver.set_open(true);
        let seed = driver.advance(at(0.0), theme);
        assert_eq!((seed.opacity, seed.geometry), (0.0, 1.0));
        let mid = driver.advance(at(100.0), theme);
        assert!((mid.opacity - 0.5).abs() < 1e-9, "{mid:?}");
        assert_eq!(mid.geometry, 1.0, "geometry is pinned while visible");
        assert_eq!(
            driver.advance(at(200.0), theme).phase,
            PresencePhase::Present
        );

        driver.set_open(false);
        driver.advance(at(200.0), theme);
        let out = driver.advance(at(300.0), theme);
        assert!(
            (out.opacity - 0.5).abs() < 1e-9 && out.geometry == 1.0,
            "{out:?}"
        );
        let gone = driver.advance(at(400.0), theme);
        assert_eq!((gone.phase, gone.geometry), (PresencePhase::Absent, 0.0));
        assert!(driver.take_exited(), "a reduced exit still reports itself");
    }

    #[test]
    fn theme_default_timings_resolve_from_the_motion_tokens() {
        let theme = Theme::builder(Theme::neutral())
            .map_motion(|motion| MotionScheme {
                durations: MotionDurations {
                    base: 100.0,
                    fast: 40.0,
                    ..motion.durations
                },
                easing: EasingSet {
                    effects: Curve::Linear,
                    exit: Curve::Linear,
                    ..motion.easing
                },
                ..motion
            })
            .build();
        let theme = Some(&theme);
        let mut driver = Presence::new();
        driver.set_open(true);
        driver.advance(at(0.0), theme);
        let enter = driver.advance(at(50.0), theme).opacity;
        assert!((enter - 0.5).abs() < 1e-9, "base 100 ms: {enter}");
        assert_eq!(
            driver.advance(at(100.0), theme).phase,
            PresencePhase::Present
        );
        driver.set_open(false);
        driver.advance(at(100.0), theme);
        let exit = driver.advance(at(120.0), theme).opacity;
        assert!((exit - 0.5).abs() < 1e-9, "fast 40 ms: {exit}");
    }

    #[test]
    fn a_zero_reduced_fade_snaps_without_a_frame() {
        let theme = Theme::builder(Theme::neutral())
            .map_motion(|motion| MotionScheme {
                reduce_motion: true,
                ..motion
            })
            .extension(MotionExtras {
                reduced_fade: 0.0,
                ..MotionExtras::neutral()
            })
            .build();
        let mut driver = Presence::new();
        driver.set_open(true);
        let sample = driver.advance(at(0.0), Some(&theme));
        assert_eq!(
            (sample.phase, sample.opacity),
            (PresencePhase::Present, 1.0)
        );
        assert!(!driver.is_animating());
    }

    // --- View ---

    #[derive(Default)]
    struct Owner {
        open: bool,
        exits: u32,
    }

    /// A full-box child that counts the pointer `Down`s reaching it and
    /// contributes one labelled semantics node.
    struct Target {
        hits: Rc<Cell<u32>>,
    }

    struct TargetWidget {
        hits: Rc<Cell<u32>>,
    }

    impl<S: 'static> View<S> for Target {
        type Element = TargetWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> TargetWidget {
            TargetWidget {
                hits: self.hits.clone(),
            }
        }
        fn rebuild(&self, _: &Self, _: &mut TargetWidget, _: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for TargetWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
        }
        fn event(&mut self, _ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                self.hits.set(self.hits.get() + 1);
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
        fn semantics(&self, ctx: &mut SemanticsCtx) {
            ctx.push_node(frust_core::accesskit::Role::Button, |node| {
                node.set_label("target")
            });
        }
    }

    type Root = RenderRoot<Owner, PresenceView<Owner>>;
    type Logic = Box<dyn FnMut(&mut Owner) -> PresenceView<Owner>>;

    const BOX: Size = Size::new(40.0, 40.0);

    struct Harness {
        root: Root,
        logic: Logic,
        owner: Owner,
        hits: Rc<Cell<u32>>,
        now: f64,
    }

    impl Harness {
        fn new(open: bool) -> Self {
            let hits = Rc::new(Cell::new(0));
            let child_hits = hits.clone();
            let logic = Box::new(move |owner: &mut Owner| {
                presence(
                    owner.open,
                    Target {
                        hits: child_hits.clone(),
                    },
                )
                .timing(linear(64), linear(64))
                .on_exited(|owner: &mut Owner| owner.exits += 1)
            });
            let mut harness = Harness {
                root: RenderRoot::new(),
                logic,
                owner: Owner { open, exits: 0 },
                hits,
                now: 0.0,
            };
            harness.frame();
            harness
        }

        /// One shell frame — rebuild, layout, paint — 16 ms after the last.
        fn frame(&mut self) -> RecordingScene {
            self.root.rebuild(&mut self.logic, &mut self.owner);
            self.root.layout(BOX);
            self.now += 16.0;
            let mut scene = RecordingScene::default();
            self.root.paint(&mut scene, at(self.now));
            scene
        }

        fn frames(&mut self, n: usize) -> RecordingScene {
            let mut scene = RecordingScene::default();
            for _ in 0..n {
                scene = self.frame();
            }
            scene
        }

        fn tap(&mut self) -> bool {
            let down = InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(20.0, 20.0),
                button: PointerButton::Primary,
            });
            self.root.event(&mut self.owner, &down).handled
        }

        fn labels(&self) -> Vec<String> {
            self.root
                .semantics()
                .nodes
                .iter()
                .filter_map(|(_, node)| node.label().map(str::to_string))
                .collect()
        }
    }

    #[test]
    fn closed_presence_paints_nothing_and_is_inert() {
        let mut h = Harness::new(false);
        let scene = h.frames(3);
        assert!(scene.rects.is_empty() && scene.layers.is_empty());
        assert!(!h.tap(), "a closed child takes no input");
        assert_eq!(h.hits.get(), 0);
        assert_eq!(h.owner.exits, 0, "never opened, never exited");

        h.owner.open = true;
        let scene = h.frames(8);
        assert_eq!(scene.rects.len(), 1, "an open child paints");
        assert!(h.tap(), "and takes input");
        assert_eq!(h.hits.get(), 1);
    }

    #[test]
    fn exiting_child_is_painted_but_not_hit() {
        let mut h = Harness::new(true);
        h.frames(8);
        h.owner.open = false;
        h.frames(2);
        let scene = h.frame();
        assert_eq!(scene.rects.len(), 1, "still painted mid-exit");
        let alpha = scene.layers[0].2;
        assert!(alpha > 0.0 && alpha < 1.0, "partway out: {alpha}");
        assert!(!h.tap(), "an exiting child is inert");
        assert_eq!(h.hits.get(), 0);
    }

    #[test]
    fn on_exited_fires_without_an_input_event() {
        let mut h = Harness::new(true);
        h.frames(8);
        h.owner.open = false;
        // Frames only: the settling paint owes the next frame's Housekeeping
        // flush, which delivers the callback.
        let scene = h.frames(10);
        assert_eq!(h.owner.exits, 1, "reported once the exit settled");
        assert!(scene.rects.is_empty(), "and nothing is painted once absent");
        h.frames(5);
        assert_eq!(h.owner.exits, 1, "and only once");
    }

    #[test]
    fn semantics_absent_while_closed() {
        let mut h = Harness::new(false);
        assert!(h.labels().is_empty(), "closed: {:?}", h.labels());
        h.owner.open = true;
        h.frames(8);
        assert_eq!(h.labels(), vec!["target".to_string()]);
        h.owner.open = false;
        h.frame();
        assert_eq!(
            h.labels(),
            vec!["target".to_string()],
            "visible while exiting"
        );
        h.frames(10);
        assert!(h.labels().is_empty(), "absent again: {:?}", h.labels());
    }
}
