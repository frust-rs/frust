//! [`Stagger`]: one clock, `n` offset copies of the same ramp.
//!
//! A staggered reveal is the catalog's most reused motion: letters cascading
//! into a label, rows dropping into a list, toasts settling onto a stack, dock
//! icons swelling around the pointer. Upstream expresses it as a Motion parent
//! variant with a `delay` per child; expressed here it is one shared elapsed
//! time and a per-index offset into it — **no per-item controller**, which is
//! what keeps a fifty-letter cascade a single [`Ramp`] evaluated fifty times
//! rather than fifty animations to advance and reconcile.
//!
//! # Where this idiom comes from, and why it is re-stated here
//!
//! `frust-core` carries the same staggered-animation idiom internally, but it
//! is not reachable through the `frust` facade, which is this crate's only
//! production dependency — `frust_glyph::motion`'s own stagger states the same
//! constraint and takes the same route. `frust-widgets` publishes no stagger
//! primitive at all. So the idiom is re-stated here, widened where the catalog
//! needs it: a spring shape as well as an eased one, a reversible order, and an
//! enter/exit direction carrying upstream's asymmetric exit timing.
//!
//! # Direction is timing, not sign
//!
//! [`progress`](Stagger::progress) always runs `0 → 1` — it is *this item's own
//! sub-animation*, not its visibility. What [`StaggerDirection`] changes is the
//! timing: an exit cascade offsets its items by **half** the enter delay, so the
//! tail of a departing label lingers briefly instead of unwinding as slowly as
//! it arrived (`components/motion/action-swap.tsx`'s cascade variants, beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`: `delay: delay * 0.5` on exit).
//! [`revealed`](Stagger::revealed) is the read that *does* fold direction into a
//! sign, for a caller driving opacity straight from it.

use std::time::Duration;

use frust::{Curve, SpringDescription};

use super::Ramp;

/// The factor an exit run's per-item delay is scaled by, relative to the same
/// stagger's enter run.
///
/// Source: `CASCADE_LETTER_VARIANTS`' exit transition
/// (`components/motion/action-swap.tsx`) — `delay: delay * 0.5` against the
/// enter arm's undivided `delay`, with the stated intent that "exits cascade at
/// half the enter stagger so the tail of the old label lingers briefly".
pub const EXIT_DELAY_FACTOR: f64 = 0.5;

/// Which way a staggered run is travelling — see the [module docs](self) on why
/// this is a timing choice rather than a sign.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StaggerDirection {
    /// Items arriving. Per-item delays are used as authored.
    #[default]
    Enter,
    /// Items leaving. Per-item delays are scaled by [`EXIT_DELAY_FACTOR`].
    Exit,
}

/// Which end of the list leads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StaggerOrder {
    /// Index `0` leads; the last item is the most delayed. Left-to-right for a
    /// text cascade, top-down for a list.
    #[default]
    Forward,
    /// The last index leads; index `0` is the most delayed. Bottom-up for a
    /// toast stack, right-to-left for a cascade unwinding.
    Reverse,
}

/// A staggered run: `n` copies of one [`Ramp`], each offset by its index.
///
/// Cheap and `Copy` — a widget stores one and evaluates it per item per paint;
/// nothing here allocates or retains state, so two widgets may share the same
/// value and a test may construct one inline.
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use frust_beui::motion::{Ramp, Stagger};
/// use frust_beui::tokens::motion::SPRING_SWAP;
///
/// let cascade = Stagger::new(Duration::from_millis(25), Ramp::spring(SPRING_SWAP));
/// // Item 0 is already moving while item 3 has not started.
/// assert!(cascade.progress(Duration::from_millis(10), 0, 4) > 0.0);
/// assert_eq!(cascade.progress(Duration::from_millis(10), 3, 4), 0.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stagger {
    per_item_delay: Duration,
    ramp: Ramp,
    direction: StaggerDirection,
    order: StaggerOrder,
}

impl Stagger {
    /// A forward-ordered enter run: consecutive items start `per_item_delay`
    /// apart and each plays `ramp`.
    pub const fn new(per_item_delay: Duration, ramp: Ramp) -> Self {
        Stagger {
            per_item_delay,
            ramp,
            direction: StaggerDirection::Enter,
            order: StaggerOrder::Forward,
        }
    }

    /// A run whose items are shaped by a duration and one of
    /// [`crate::tokens::motion`]'s curves.
    pub const fn eased(per_item_delay: Duration, item: Duration, curve: Curve) -> Self {
        Self::new(per_item_delay, Ramp::eased(item, curve))
    }

    /// A run whose items are shaped by one of [`crate::tokens::motion`]'s
    /// springs.
    pub const fn sprung(per_item_delay: Duration, spring: SpringDescription) -> Self {
        Self::new(per_item_delay, Ramp::spring(spring))
    }

    /// This run, travelling in `direction`.
    pub const fn direction(mut self, direction: StaggerDirection) -> Self {
        self.direction = direction;
        self
    }

    /// This run, with `order`'s end leading.
    pub const fn order(mut self, order: StaggerOrder) -> Self {
        self.order = order;
        self
    }

    /// The exit twin of this run: same ramp and order, delays scaled by
    /// [`EXIT_DELAY_FACTOR`].
    pub const fn exiting(self) -> Self {
        self.direction(StaggerDirection::Exit)
    }

    /// This run with its per-item offsets removed, so every item moves together
    /// on one ramp.
    ///
    /// The `reduce_motion` collapse: a cascade becomes a single fade, with the
    /// ramp itself left alone so the motion still reads as the same component
    /// rather than a different one.
    pub const fn collapsed(mut self) -> Self {
        self.per_item_delay = Duration::ZERO;
        self
    }

    /// The authored gap between consecutive items, before
    /// [`EXIT_DELAY_FACTOR`].
    pub const fn per_item_delay(&self) -> Duration {
        self.per_item_delay
    }

    /// The ramp each item plays.
    pub const fn ramp(&self) -> Ramp {
        self.ramp
    }

    /// Which way this run travels.
    pub const fn stagger_direction(&self) -> StaggerDirection {
        self.direction
    }

    /// Which end of the list leads.
    pub const fn stagger_order(&self) -> StaggerOrder {
        self.order
    }

    /// The effective gap between consecutive items, with the exit scaling
    /// applied.
    pub fn effective_delay(&self) -> Duration {
        match self.direction {
            StaggerDirection::Enter => self.per_item_delay,
            StaggerDirection::Exit => self.per_item_delay.mul_f64(EXIT_DELAY_FACTOR),
        }
    }

    /// Item `index`'s position in the running order — the identity under
    /// [`StaggerOrder::Forward`], mirrored under [`StaggerOrder::Reverse`].
    ///
    /// Out-of-range indices report slot `0`; see [`progress`](Self::progress)
    /// for what an out-of-range read yields.
    pub fn slot(&self, index: usize, count: usize) -> usize {
        if index >= count {
            return 0;
        }
        match self.order {
            StaggerOrder::Forward => index,
            StaggerOrder::Reverse => count - 1 - index,
        }
    }

    /// How long after the run's start item `index` begins moving.
    pub fn delay_for(&self, index: usize, count: usize) -> Duration {
        self.effective_delay() * self.slot(index, count) as u32
    }

    /// The whole run's length for `count` items: the last item's own start plus
    /// one ramp. `count == 0` has no timeline and reports zero.
    pub fn total_duration(&self, count: usize) -> Duration {
        if count == 0 {
            return Duration::ZERO;
        }
        self.effective_delay() * (count as u32 - 1) + self.ramp.settle()
    }

    /// Item `index`'s own progress at `elapsed` into the run, **raw** — a spring
    /// ramp may overshoot past `1.0`, exactly as [`Ramp::progress`] describes.
    ///
    /// `0.0` before the item's slot opens. An index outside `0..count`, or a
    /// `count` of zero, reports `1.0`: there is no item left to move, which is
    /// the reading that keeps a list shrinking mid-run from flashing a stale
    /// cell back in.
    pub fn progress(&self, elapsed: Duration, index: usize, count: usize) -> f64 {
        if count == 0 || index >= count {
            return 1.0;
        }
        let delay = self.delay_for(index, count);
        if elapsed <= delay {
            return 0.0;
        }
        self.ramp.progress(elapsed - delay)
    }

    /// [`progress`](Self::progress) clamped into `[0, 1]` — the opacity read.
    pub fn progress_clamped(&self, elapsed: Duration, index: usize, count: usize) -> f64 {
        self.progress(elapsed, index, count).clamp(0.0, 1.0)
    }

    /// How *present* item `index` is at `elapsed`, in `[0, 1]`: its clamped
    /// progress on an enter run, one minus it on an exit run.
    ///
    /// The read a caller drives opacity from without branching on direction
    /// itself.
    pub fn revealed(&self, elapsed: Duration, index: usize, count: usize) -> f64 {
        let progress = self.progress_clamped(elapsed, index, count);
        match self.direction {
            StaggerDirection::Enter => progress,
            StaggerDirection::Exit => 1.0 - progress,
        }
    }

    /// Whether the whole run has finished by `elapsed` — the stepping widget's
    /// signal to stop requesting frames.
    pub fn is_settled(&self, elapsed: Duration, count: usize) -> bool {
        elapsed >= self.total_duration(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::motion::{EASE_OUT, SPRING_SWAP};

    /// The eased fixture: 20ms apart, 100ms each, so item `i` occupies
    /// `[20i, 20i + 100]` and the arithmetic in each assertion is checkable by
    /// hand.
    fn eased() -> Stagger {
        Stagger::eased(
            Duration::from_millis(20),
            Duration::from_millis(100),
            EASE_OUT,
        )
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// The defining property: at any instant mid-run, an earlier item is
    /// strictly further along than a later one, and the last item has not
    /// started while the first already has.
    #[test]
    fn a_forward_run_is_ordered_first_to_last() {
        let stagger = eased();
        let at = ms(50);
        let progresses: Vec<f64> = (0..5).map(|i| stagger.progress(at, i, 5)).collect();
        for pair in progresses.windows(2) {
            assert!(
                pair[0] > pair[1] || (pair[0] == 0.0 && pair[1] == 0.0),
                "forward run out of order: {progresses:?}"
            );
        }
        assert!(progresses[0] > 0.0, "the leading item has started");
        assert_eq!(
            progresses[4], 0.0,
            "the last item has not (slot opens at 80ms)"
        );
    }

    /// Reverse order mirrors the run: the same instant leaves the *last* item
    /// leading and the first not yet started, and the two orders are exact
    /// mirrors of each other index for index.
    #[test]
    fn a_reverse_run_is_the_forward_run_mirrored() {
        let forward = eased();
        let reverse = eased().order(StaggerOrder::Reverse);
        let at = ms(50);
        let n = 5;

        let reversed: Vec<f64> = (0..n).map(|i| reverse.progress(at, i, n)).collect();
        for pair in reversed.windows(2) {
            assert!(
                pair[0] < pair[1] || (pair[0] == 0.0 && pair[1] == 0.0),
                "reverse run out of order: {reversed:?}"
            );
        }
        assert_eq!(reversed[n - 1], forward.progress(at, 0, n));
        assert_eq!(reversed[0], forward.progress(at, n - 1, n));
        assert_eq!(reversed[0], 0.0, "the trailing item has not started");
        assert!(reversed[n - 1] > 0.0, "the leading item has");
    }

    /// The exit asymmetry: the same authored delay produces a run that is
    /// tighter than its enter twin, item for item and overall.
    #[test]
    fn an_exit_run_staggers_at_half_the_enter_spacing() {
        let enter = eased();
        let exit = eased().exiting();
        assert_eq!(exit.effective_delay(), enter.effective_delay().mul_f64(0.5));
        assert_eq!(exit.delay_for(3, 5), enter.delay_for(3, 5).mul_f64(0.5));
        assert!(exit.total_duration(5) < enter.total_duration(5));
    }

    /// Direction folds into `revealed`, not into `progress`: an exiting item's
    /// own sub-animation still climbs while its presence falls away.
    #[test]
    fn revealed_inverts_on_an_exit_run_while_progress_does_not() {
        let exit = eased().exiting();
        let early = exit.progress(ms(20), 0, 3);
        let late = exit.progress(ms(60), 0, 3);
        assert!(late > early, "the item's own ramp still runs forward");
        assert!(
            exit.revealed(ms(60), 0, 3) < exit.revealed(ms(20), 0, 3),
            "but it is becoming less present"
        );

        let enter = eased();
        assert_eq!(
            enter.revealed(ms(60), 0, 3),
            enter.progress_clamped(ms(60), 0, 3)
        );
    }

    /// The timeline spans every item's own ramp: nothing has moved at zero,
    /// everything has finished at the total, and the run reports settled there
    /// and not before.
    #[test]
    fn the_timeline_spans_the_whole_run() {
        let stagger = eased();
        let n = 6;
        assert_eq!(stagger.total_duration(n), ms(20) * 5 + ms(100));

        for i in 0..n {
            assert_eq!(stagger.progress(Duration::ZERO, i, n), 0.0);
            assert_eq!(stagger.progress(stagger.total_duration(n), i, n), 1.0);
        }
        assert!(!stagger.is_settled(stagger.total_duration(n) - ms(1), n));
        assert!(stagger.is_settled(stagger.total_duration(n), n));
    }

    /// The `reduce_motion` collapse removes the offsets and nothing else: every
    /// item now reads identically, and the run is exactly one ramp long.
    #[test]
    fn a_collapsed_run_moves_every_item_together() {
        let stagger = eased().collapsed();
        let at = ms(40);
        let first = stagger.progress(at, 0, 8);
        assert!(first > 0.0);
        for i in 1..8 {
            assert_eq!(stagger.progress(at, i, 8), first);
        }
        assert_eq!(stagger.total_duration(8), ms(100));
    }

    /// A spring-shaped run staggers the same way an eased one does, and each
    /// item inherits the ramp's own shape — overshoot included, which is what a
    /// cascade's per-letter bounce is made of.
    #[test]
    fn a_sprung_run_staggers_and_each_item_keeps_the_ramp_shape() {
        let stagger = Stagger::sprung(ms(30), SPRING_SWAP);
        let at = ms(45);
        assert!(stagger.progress(at, 0, 3) > stagger.progress(at, 1, 3));
        assert_eq!(stagger.progress(at, 2, 3), 0.0, "slot opens at 60ms");

        // Item 1's curve is item 0's, shifted by exactly one slot.
        let shifted = Stagger::sprung(ms(30), SPRING_SWAP);
        for step in 0..20u32 {
            let t = ms(30) + ms(10) * step;
            assert_eq!(
                shifted.progress(t + ms(30), 1, 3),
                shifted.progress(t, 0, 3)
            );
        }

        // A bouncier spring than the catalog ships, to show the overshoot rides
        // through the stagger rather than being clamped away by it.
        let bouncy = Stagger::sprung(
            ms(30),
            SpringDescription {
                mass: 1.0,
                stiffness: 400.0,
                damping: 8.0,
            },
        );
        let settle = bouncy.ramp().settle();
        let peak = (0..=200)
            .map(|s| bouncy.progress(settle.mul_f64(s as f64 / 200.0), 0, 3))
            .fold(0.0_f64, f64::max);
        assert!(
            peak > 1.05,
            "the per-item overshoot survives the stagger: {peak}"
        );
    }

    /// Empty and out-of-range reads are defined rather than panicking: nothing
    /// is left to move, so both report fully progressed.
    #[test]
    fn an_empty_or_out_of_range_read_reports_nothing_left_to_move() {
        let stagger = eased();
        assert_eq!(stagger.total_duration(0), Duration::ZERO);
        assert_eq!(stagger.progress(ms(10), 0, 0), 1.0);
        assert_eq!(stagger.progress(ms(10), 7, 3), 1.0);
        assert!(stagger.is_settled(Duration::ZERO, 0));
    }

    /// A single-item run has no stagger to speak of: it is exactly its ramp.
    #[test]
    fn a_single_item_run_is_just_its_ramp() {
        let stagger = eased();
        assert_eq!(stagger.total_duration(1), ms(100));
        assert_eq!(stagger.delay_for(0, 1), Duration::ZERO);
        assert_eq!(
            stagger.progress(ms(50), 0, 1),
            stagger.ramp().progress(ms(50))
        );
    }
}
