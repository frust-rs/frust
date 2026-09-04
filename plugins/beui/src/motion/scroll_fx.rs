//! [`ScrollFx`]: a scroll surface's own offsets folded into the three values
//! scroll-linked effects read.
//!
//! beUI's scroll family — the progress bar and ring, parallax drift, the
//! once-revealed section — all sit on Motion's `useScroll`, a continuously
//! readable `0..1` progress value any component may sample whenever it likes.
//! This module is that value's frust-shaped equivalent: [`ScrollFx::observe`]
//! folds each [`ScrollInfo`] into a progress fraction, a once-latched reveal
//! threshold, and (given a clock) a velocity estimate.
//!
//! # The framework contract this adapter is shaped around
//!
//! **Offsets arrive only through the callback.** `ScrollView::on_scroll` is the
//! sole delivery route for a scroll position — there is no mid-paint query, no
//! readable controller, nothing to sample on demand. So an effect cannot pull;
//! it is pushed, and this adapter is the retained thing in between.
//!
//! **Paint-driven settle lands one event late.** Drag and wheel motion notifies
//! within the pass that caused it, but fling and settle motion is integrated
//! during *paint*, which carries no `EventCtx` — so that notification is
//! recorded and delivered on the next event pass instead (the same
//! controlled-component convention every interactive widget in the framework
//! follows; `ScrollView::on_scroll`'s own contract states it, and a `Cancel`
//! drops a pending notification without firing it). Two consequences a
//! scroll-linked effect must be built to tolerate:
//!
//! * A fling that ends with no further input leaves this adapter holding the
//!   second-to-last offset until something else arrives. An effect must
//!   therefore be *positional* — derived from wherever the surface is — and
//!   never accumulate, or the residue is permanent rather than momentary.
//! * A velocity estimated from those samples under-reports through a fling for
//!   the same reason. It is an effect input (how hard to skew a parallax layer),
//!   never a physics input; the framework's own scroll physics run inside
//!   `ScrollView` and need nothing from here.
//!
//! # Time comes from the caller
//!
//! An event pass carries no clock, and framework-tier code never reads a wall
//! clock — time enters as the `FrameTime` a paint already holds. So the velocity
//! estimate is opt-in: [`observe`](ScrollFx::observe) tracks position only, and
//! [`observe_at`](ScrollFx::observe_at) additionally feeds the framework's own
//! [`VelocityTracker`] with a timestamp the caller differenced from two
//! `FrameTime`s. A consumer with no clock to offer simply gets no velocity,
//! rather than a fabricated one.

use frust::ScrollInfo;
use frust::input::VelocityTracker;
use frust::{RwSignal, Set};

/// The default fraction of the way down a surface at which
/// [`ScrollFx::revealed`] latches.
///
/// Source: `ScrollReveal`'s `amount = 0.3` default
/// (`components/motion/scroll-reveal.tsx`, beUI rev
/// `10c283e433a8f4f0ac0736684d4426ab612b9f55`) — upstream measures it as the
/// portion of the *element* that must be visible, which for a whole-surface
/// adapter is the same number read against the surface's own travel.
pub const DEFAULT_REVEAL_THRESHOLD: f64 = 0.3;

/// A scroll surface's state, folded from the [`ScrollInfo`] stream its
/// `on_scroll` callback delivers.
///
/// Retained by whatever owns the scroll surface — a widget, or a component's
/// state — and fed from the callback. Everything it exposes is positional, so a
/// dropped or late notification is a momentary staleness rather than an
/// accumulating error.
#[derive(Clone, Debug)]
pub struct ScrollFx {
    offset: f64,
    max_offset: f64,
    overscroll: f64,
    delta: f64,
    threshold: f64,
    revealed: bool,
    velocity: VelocityTracker,
    observations: usize,
}

impl ScrollFx {
    /// An unscrolled surface with the [`DEFAULT_REVEAL_THRESHOLD`].
    pub fn new() -> Self {
        Self::with_threshold(DEFAULT_REVEAL_THRESHOLD)
    }

    /// An unscrolled surface whose reveal latches at `threshold` progress.
    ///
    /// The threshold is clamped into `[0, 1]`; a threshold of `0.0` latches on
    /// the first observation, which is a legitimate "reveal as soon as this
    /// surface is scrollable at all".
    pub fn with_threshold(threshold: f64) -> Self {
        ScrollFx {
            offset: 0.0,
            max_offset: 0.0,
            overscroll: 0.0,
            delta: 0.0,
            threshold: threshold.clamp(0.0, 1.0),
            revealed: false,
            velocity: VelocityTracker::new(),
            observations: 0,
        }
    }

    /// Fold one notification in, without a clock: position, progress, delta and
    /// the reveal latch all update; the velocity estimate does not.
    ///
    /// Returns whether anything an effect reads changed — the caller's redraw
    /// gate.
    pub fn observe(&mut self, info: ScrollInfo) -> bool {
        let before = (self.offset, self.max_offset, self.overscroll, self.revealed);
        self.delta = info.offset - self.offset;
        self.offset = info.offset;
        self.max_offset = info.max_offset.max(0.0);
        self.overscroll = info.overscroll;
        self.observations += 1;
        if !self.revealed && self.progress() >= self.threshold {
            self.revealed = true;
        }
        before != (self.offset, self.max_offset, self.overscroll, self.revealed)
    }

    /// [`observe`](Self::observe), additionally timestamped so the velocity
    /// estimate advances.
    ///
    /// `time_ms` is a monotonic millisecond reading on the caller's own
    /// timeline — in practice `FrameTime::saturating_sub` against the pass that
    /// started the gesture. Consistency between successive calls is all that
    /// matters; the origin is arbitrary.
    pub fn observe_at(&mut self, info: ScrollInfo, time_ms: f64) -> bool {
        let changed = self.observe(info);
        self.velocity.record(time_ms, self.offset);
        changed
    }

    /// The clamped scroll offset most recently reported, in px.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// The surface's maximum offset (content minus viewport), never negative.
    pub fn max_offset(&self) -> f64 {
        self.max_offset
    }

    /// Signed past-edge displacement: negative past the top, positive past the
    /// bottom, zero while in range.
    ///
    /// Passed through unchanged — a pull-to-refresh or rubber-band effect wants
    /// the surface's own resisted number, not a second interpretation of it.
    pub fn overscroll(&self) -> f64 {
        self.overscroll
    }

    /// How far the offset moved on the most recent observation, in px. Positive
    /// scrolling down.
    ///
    /// Per *notification*, not per unit time — the direction cue, with
    /// [`velocity`](Self::velocity) the timed reading.
    pub fn delta(&self) -> f64 {
        self.delta
    }

    /// Scroll position as a `0..=1` fraction of the surface's travel.
    ///
    /// A surface with nothing to scroll (`max_offset == 0`) has no meaningful
    /// fraction and reports `0.0` — an unscrollable page's progress bar reads
    /// empty rather than full, which is what upstream's `useScroll` does for a
    /// document shorter than its viewport.
    pub fn progress(&self) -> f64 {
        if self.max_offset <= 0.0 {
            return 0.0;
        }
        (self.offset / self.max_offset).clamp(0.0, 1.0)
    }

    /// Whether progress has *ever* reached the reveal threshold.
    ///
    /// Latched, matching upstream's `once` default: a section that has revealed
    /// stays revealed when it scrolls back out, so its content does not flicker
    /// on a scroll-up. [`rearm`](Self::rearm) is the every-time variant.
    pub fn revealed(&self) -> bool {
        self.revealed
    }

    /// The threshold progress must reach for [`revealed`](Self::revealed) to
    /// latch.
    pub fn threshold(&self) -> f64 {
        self.threshold
    }

    /// Clear the reveal latch, so the threshold can be crossed again — the
    /// `once: false` behaviour, driven by the consumer at whatever point it
    /// considers the section to have left.
    pub fn rearm(&mut self) {
        self.revealed = false;
    }

    /// Estimated scroll velocity in px/s, positive scrolling down.
    ///
    /// Zero until [`observe_at`](Self::observe_at) has supplied at least two
    /// timestamped samples. Estimated by the framework's own
    /// [`VelocityTracker`] over its trailing window — the same estimator the
    /// baseline scroll surface flings from, rather than a second one that could
    /// disagree with it. An effect input; see the [module docs](self) on why it
    /// under-reports through a paint-driven fling.
    pub fn velocity(&self) -> f64 {
        self.velocity.velocity()
    }

    /// How many notifications have been folded in — a consumer's way of telling
    /// "at the top" from "has never been told anything".
    pub fn observations(&self) -> usize {
        self.observations
    }
}

impl Default for ScrollFx {
    fn default() -> Self {
        Self::new()
    }
}

/// A [`ScrollFx`]'s reads republished as signals, for effects that live outside
/// the widget holding the scroll surface — a progress bar pinned to the window,
/// a parallax layer in a sibling subtree.
///
/// The same trade [`super::pointer::PointerSignals`] documents: a publish wakes
/// every tracked reader, so it belongs behind
/// [`ScrollFx::observe`]'s changed-return rather than on every notification.
#[derive(Clone, Copy, Debug)]
pub struct ScrollFxSignals {
    /// Scroll position as a `0..=1` fraction of travel.
    pub progress: RwSignal<f64>,
    /// Whether the reveal threshold has been crossed.
    pub revealed: RwSignal<bool>,
    /// Estimated velocity in px/s.
    pub velocity: RwSignal<f64>,
    /// Signed past-edge displacement in px.
    pub overscroll: RwSignal<f64>,
}

impl ScrollFxSignals {
    /// Four signals at rest.
    pub fn new() -> Self {
        Self {
            progress: RwSignal::new(0.0),
            revealed: RwSignal::new(false),
            velocity: RwSignal::new(0.0),
            overscroll: RwSignal::new(0.0),
        }
    }

    /// Copy `fx`'s current reads into the four signals.
    pub fn publish(&self, fx: &ScrollFx) {
        self.progress.set(fx.progress());
        self.revealed.set(fx.revealed());
        self.velocity.set(fx.velocity());
        self.overscroll.set(fx.overscroll());
    }
}

impl Default for ScrollFxSignals {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Get;

    /// A synthetic notification, as `ScrollView` would deliver it.
    fn info(offset: f64, max_offset: f64) -> ScrollInfo {
        ScrollInfo {
            offset,
            max_offset,
            overscroll: 0.0,
        }
    }

    /// Progress tracks the fraction of travel and is clamped at both ends.
    #[test]
    fn progress_is_the_fraction_of_travel() {
        let mut fx = ScrollFx::new();
        assert_eq!(fx.progress(), 0.0);

        for (offset, expected) in [(0.0, 0.0), (100.0, 0.25), (200.0, 0.5), (400.0, 1.0)] {
            fx.observe(info(offset, 400.0));
            assert_eq!(fx.progress(), expected, "at offset {offset}");
        }

        // A surface reporting an offset past its own maximum still reads full.
        fx.observe(info(999.0, 400.0));
        assert_eq!(fx.progress(), 1.0);
    }

    /// A surface with nothing to scroll has no fraction to report.
    #[test]
    fn an_unscrollable_surface_reads_empty_not_full() {
        let mut fx = ScrollFx::new();
        fx.observe(info(0.0, 0.0));
        assert_eq!(fx.progress(), 0.0);
        assert!(!fx.revealed(), "and never crosses a non-zero threshold");
    }

    /// The reveal latches when the threshold is crossed and stays latched on the
    /// way back — upstream's `once` default.
    #[test]
    fn the_reveal_latches_once_and_survives_scrolling_back() {
        let mut fx = ScrollFx::with_threshold(0.5);
        for offset in [0.0, 100.0, 199.0] {
            fx.observe(info(offset, 400.0));
            assert!(!fx.revealed(), "not yet at half travel (offset {offset})");
        }
        fx.observe(info(200.0, 400.0));
        assert!(fx.revealed(), "crossing the threshold latches it");

        fx.observe(info(0.0, 400.0));
        assert!(fx.revealed(), "and scrolling back does not unlatch it");

        fx.rearm();
        assert!(!fx.revealed());
        fx.observe(info(400.0, 400.0));
        assert!(fx.revealed(), "re-armed, it latches again");
    }

    /// A zero threshold latches on the first notification; a threshold outside
    /// the unit range is clamped rather than made unreachable.
    #[test]
    fn the_threshold_is_clamped_into_the_unit_range() {
        let mut fx = ScrollFx::with_threshold(0.0);
        fx.observe(info(0.0, 400.0));
        assert!(fx.revealed());

        let mut fx = ScrollFx::with_threshold(4.0);
        assert_eq!(fx.threshold(), 1.0);
        fx.observe(info(400.0, 400.0));
        assert!(
            fx.revealed(),
            "a clamped threshold is still reachable at full travel"
        );

        let mut fx = ScrollFx::with_threshold(-1.0);
        assert_eq!(fx.threshold(), 0.0);
        fx.observe(info(0.0, 400.0));
        assert!(fx.revealed());
    }

    /// Delta is the per-notification movement, signed by direction.
    #[test]
    fn the_delta_reports_the_last_movement_and_its_direction() {
        let mut fx = ScrollFx::new();
        fx.observe(info(0.0, 400.0));
        assert_eq!(fx.delta(), 0.0);
        fx.observe(info(60.0, 400.0));
        assert_eq!(fx.delta(), 60.0);
        fx.observe(info(20.0, 400.0));
        assert_eq!(fx.delta(), -40.0);
    }

    /// Velocity needs timestamps, and reports px/s once it has two of them.
    #[test]
    fn velocity_is_zero_without_a_clock_and_estimated_with_one() {
        let mut fx = ScrollFx::new();
        fx.observe(info(0.0, 1000.0));
        fx.observe(info(100.0, 1000.0));
        assert_eq!(fx.velocity(), 0.0, "no timestamps, no velocity");

        let mut fx = ScrollFx::new();
        fx.observe_at(info(0.0, 1000.0), 0.0);
        assert_eq!(fx.velocity(), 0.0, "one sample is not a velocity");
        fx.observe_at(info(16.0, 1000.0), 16.0);
        assert!(
            (fx.velocity() - 1000.0).abs() < 1e-6,
            "16px in 16ms is 1000px/s, got {}",
            fx.velocity()
        );

        // Scrolling back up reads negative.
        let mut fx = ScrollFx::new();
        fx.observe_at(info(100.0, 1000.0), 0.0);
        fx.observe_at(info(50.0, 1000.0), 50.0);
        assert!(fx.velocity() < 0.0);
    }

    /// Overscroll passes through with its sign intact.
    #[test]
    fn overscroll_passes_through_unchanged() {
        let mut fx = ScrollFx::new();
        fx.observe(ScrollInfo {
            offset: 0.0,
            max_offset: 400.0,
            overscroll: -32.0,
        });
        assert_eq!(fx.overscroll(), -32.0);
        assert_eq!(fx.progress(), 0.0, "past the top is still zero progress");
    }

    /// The changed-return gates a consumer's redraw: a repeated notification
    /// with the same numbers reports nothing new.
    #[test]
    fn a_repeated_notification_reports_no_change() {
        let mut fx = ScrollFx::new();
        assert!(fx.observe(info(50.0, 400.0)));
        assert!(!fx.observe(info(50.0, 400.0)));
        assert!(fx.observe(info(51.0, 400.0)));
        assert_eq!(
            fx.observations(),
            3,
            "but every notification is still counted"
        );
    }

    /// The signal mirror carries the folded reads across a publish.
    #[test]
    fn publishing_mirrors_the_reads_into_signals() {
        let signals = ScrollFxSignals::new();
        let mut fx = ScrollFx::with_threshold(0.25);
        fx.observe_at(info(0.0, 400.0), 0.0);
        fx.observe_at(info(200.0, 400.0), 100.0);
        signals.publish(&fx);

        assert_eq!(signals.progress.get(), 0.5);
        assert!(signals.revealed.get());
        assert_eq!(signals.velocity.get(), 2000.0);
        assert_eq!(signals.overscroll.get(), 0.0);
    }
}
