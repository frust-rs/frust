//! The catalog's animation substrate: the five mechanisms beUI components
//! animate *with*, built here because the framework publishes none of them.
//!
//! beUI is a Motion (`motion/react`) catalog. Its components lean on four
//! runtime facilities the web gives them for free — a shared timeline that
//! offsets each item of a list, per-letter text spans, a live cursor position,
//! and a scroll-linked progress value — plus one React facility, the ability to
//! keep an unmounting subtree alive while it plays an exit. frust has none of
//! those as primitives: a widget gets a frame clock, a widget-local pointer
//! event, a scroll callback, and a tree that is gone the frame after a rebuild
//! drops it. This module is where each gap is closed **once**, so thirty
//! components do not each close it differently.
//!
//! # Module map
//!
//! * [`stagger`] — [`Stagger`], one shared clock offset per list index. The
//!   substrate for text cascades, list reveals, toast stacks and dock
//!   magnification neighbourhoods.
//! * [`chars`] — [`CharCells`]/[`char_cascade`], a string split into per-grapheme
//!   cells laid out on one line, each animatable on its own [`Stagger`] slot.
//! * [`pointer`] — [`PointerTracker`], widget-local pointer position plus the
//!   centre-normalised offset magnetic/tilt/spotlight effects are driven from.
//! * [`scroll_fx`] — [`ScrollFx`], a [`ScrollInfo`](frust::ScrollInfo) stream
//!   folded into progress, a once-latched reveal threshold, and a velocity
//!   estimate.
//! * [`presence`] — [`Presence`], the enter/exit state machine that keeps a
//!   child mounted for the length of its exit ramp.
//!
//! # Naming
//!
//! The catalog's three component families prefix their public symbols
//! (`ButtonVariant`, never a bare `Variant`) because they cover overlapping
//! ground. This module deliberately does not: it names *mechanisms*, not
//! per-component variants, and there is exactly one stagger driver and one
//! presence machine in the crate.
//!
//! # The shared timing contract
//!
//! Nothing here reads a wall clock. Time enters as a
//! [`FrameTime`](frust::FrameTime) a widget's `paint` already has, or as an
//! elapsed [`Duration`] a caller differenced from two of them — the framework's
//! no-`Instant::now()` rule, which is also why the two event-fed helpers
//! ([`ScrollFx`]'s velocity estimate, [`PointerTracker`]) take a timestamp from
//! their caller rather than sampling one: an `EventCtx` carries no clock.
//!
//! Every driver here is a **pure state machine over that clock** — no widget,
//! no scene, no pods — following `frust_widgets::motion::patterns`' staging-math
//! precedent, so the timing is table-testable in isolation. [`chars`] is the one
//! exception, because per-letter cells have to *be* widgets.
//!
//! # Frame driving, and who calls it
//!
//! A driver never requests a frame; the widget stepping it does, from its own
//! `paint`:
//!
//! * A ramp with a visible endpoint — every enter, exit and stagger in this
//!   module — calls `PaintCtx::request_frame()` (implicitly
//!   [`TickClass::Transition`](frust::authoring::TickClass), unpaced) while the
//!   driver still reports motion, and stops the frame it settles.
//! * A perpetual decorative loop (a shimmer, a marquee) is *not* one of those:
//!   it calls `request_frame_class(TickClass::CosmeticLoop)`, or
//!   `request_frame_paced_at(interval)` when its own cadence is far slower than
//!   the frame gate's cosmetic cap.
//! * An animation that changes *layout* calls `request_layout()` instead, which
//!   implies a frame.
//!
//! # `reduce_motion`
//!
//! Honouring the theme's `motion.reduce_motion` is the stepping widget's job,
//! and every driver here gives it a one-call collapse rather than a second code
//! path: [`Stagger::collapsed`] drops the per-item offsets so a list reveals in
//! one beat, [`Presence::collapsed`] makes enter/exit instantaneous, and
//! [`CharCells`] paints every cell at rest and requests no frames.

use std::time::Duration;

use frust::{Curve, SpringDescription};

pub mod chars;
pub mod pointer;
pub mod presence;
pub mod scroll_fx;
pub mod stagger;

pub use chars::{CellEffect, CharCell, CharCells, CharCellsView, CharCellsWidget, char_cascade};
pub use pointer::{PointerSignals, PointerTracker};
pub use presence::{Presence, PresencePhase};
pub use scroll_fx::{ScrollFx, ScrollFxSignals};
pub use stagger::{Stagger, StaggerDirection, StaggerOrder};

/// The displacement fraction a spring's envelope must decay below before
/// [`Ramp::settle`] calls it finished — one part in a thousand of the unit
/// displacement it started from.
///
/// A spring is asymptotic: it never *reaches* equilibrium, so a timeline built
/// from one needs a stated cutoff rather than a solved end time. This is the
/// same order as the framework's own default animation rest tolerance; it is a
/// visual threshold (a thousandth of a control's travel is well under a
/// physical pixel), not a physical constant.
const SPRING_SETTLE_EPSILON: f64 = 0.001;

/// Upper bound on an estimated spring settle time. A nearly-undamped spring
/// (damping ratio approaching zero) rings for an unbounded time; clamping keeps
/// a mis-specified spring from producing a timeline no user would sit through.
const SPRING_SETTLE_CAP: Duration = Duration::from_secs(10);

/// Lower bound on an estimated spring settle time, so a degenerate spring still
/// yields a timeline a division can be taken against.
const SPRING_SETTLE_FLOOR: Duration = Duration::from_millis(1);

/// How one thing's own `0 → 1` sub-animation is shaped: a duration paired with
/// an easing curve, or a physics spring.
///
/// The catalog's two motion vocabularies, and the reason this is not
/// [`frust::Timing`]: that type's spring arm carries a theme
/// `MotionSpring` (damping *ratio* and stiffness), while beUI authors its six
/// springs as mass/stiffness/damping triples — [`crate::tokens::motion`]'s
/// [`SpringDescription`] constants. A `Ramp` takes the catalog's own springs
/// directly rather than round-tripping them through a lossier shape.
///
/// Both [`Stagger`] and [`Presence`] are built on this, which is what makes a
/// staggered list and a presence exit describable with the same
/// `SPRING_SWAP`/`TIMING_PRESS` constants a component already reaches for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ramp {
    /// A bounded, eased run: `progress` reaches exactly `1.0` at `duration` and
    /// never passes it.
    Eased {
        /// How long the run takes.
        duration: Duration,
        /// The easing applied across it.
        curve: Curve,
    },
    /// A physics spring released from a unit displacement toward rest. An
    /// under-damped one **overshoots past `1.0`** before settling — see
    /// [`Ramp::progress`].
    Spring(SpringDescription),
}

impl Ramp {
    /// A bounded eased ramp.
    pub const fn eased(duration: Duration, curve: Curve) -> Self {
        Ramp::Eased { duration, curve }
    }

    /// A spring ramp from one of [`crate::tokens::motion`]'s constants.
    pub const fn spring(spring: SpringDescription) -> Self {
        Ramp::Spring(spring)
    }

    /// Progress at `elapsed`, **raw**: an eased ramp stays inside `[0, 1]`, an
    /// under-damped spring overshoots past `1.0` before settling back.
    ///
    /// Raw is the useful form for *position* and scale — an overshoot is what
    /// makes a spring visibly spring — and wrong for opacity, which is only
    /// meaningful in `[0, 1]`. Use [`progress_clamped`](Self::progress_clamped)
    /// there. This is the same raw/clamped split
    /// `frust_widgets::motion::patterns` draws for the same reason.
    ///
    /// A zero-length eased ramp is an instantaneous step to `1.0`.
    pub fn progress(&self, elapsed: Duration) -> f64 {
        match self {
            Ramp::Eased { duration, curve } => {
                if duration.is_zero() {
                    return 1.0;
                }
                if elapsed >= *duration {
                    return 1.0;
                }
                curve.transform(elapsed.as_secs_f64() / duration.as_secs_f64())
            }
            Ramp::Spring(spring) => {
                // A spring is asymptotic, so past its own settle time it is
                // snapped exactly onto its target rather than left the last
                // thousandth short — the same snap-at-rest the framework's
                // animation controller performs, and what lets a settled cell
                // composite at a true alpha of 1.
                if elapsed >= spring_settle(*spring) {
                    return 1.0;
                }
                // The solver measures *remaining* displacement from equilibrium,
                // so a unit displacement decaying to zero is progress rising to
                // one.
                1.0 - solve_spring(*spring).position(elapsed.as_secs_f64())
            }
        }
    }

    /// [`progress`](Self::progress) clamped into `[0, 1]` — the form opacity and
    /// any other bounded quantity reads.
    pub fn progress_clamped(&self, elapsed: Duration) -> f64 {
        self.progress(elapsed).clamp(0.0, 1.0)
    }

    /// How long this ramp takes to finish: an eased ramp's own duration, or a
    /// spring's **estimated** settle time.
    ///
    /// The spring estimate is analytic rather than sampled. A damped spring's
    /// envelope decays as `e^(−r·t)`, so the time to fall below
    /// [`SPRING_SETTLE_EPSILON`] is `−ln(ε) / r` — with `r = ζ·ω₀` up to
    /// critical damping, and the *slower* of the two real roots
    /// (`ω₀·(ζ − √(ζ² − 1))`) beyond it, since an over-damped spring's return is
    /// governed by its slow root. Result clamped to
    /// `[SPRING_SETTLE_FLOOR, SPRING_SETTLE_CAP]`.
    ///
    /// It is an estimate, and deliberately cheap: a stagger asks for it once per
    /// frame to size its timeline, so a fixed-point search would be the wrong
    /// shape entirely.
    pub fn settle(&self) -> Duration {
        match self {
            Ramp::Eased { duration, .. } => *duration,
            Ramp::Spring(spring) => spring_settle(*spring),
        }
    }

    /// Whether this ramp has finished by `elapsed`.
    pub fn is_settled(&self, elapsed: Duration) -> bool {
        elapsed >= self.settle()
    }
}

/// The undamped natural frequency `ω₀ = √(k/m)` and damping ratio
/// `ζ = c / (2·√(k·m))` of a [`SpringDescription`], with non-positive mass or
/// stiffness falling back to a unit spring — the same degenerate-input posture
/// the framework's own solver takes, so a mis-specified constant produces slow
/// motion rather than `NaN`.
fn spring_shape(spring: SpringDescription) -> (f64, f64) {
    let mass = if spring.mass > 0.0 { spring.mass } else { 1.0 };
    let stiffness = if spring.stiffness > 0.0 {
        spring.stiffness
    } else {
        1.0
    };
    let w0 = (stiffness / mass).sqrt();
    let zeta = spring.damping.max(0.0) / (2.0 * (mass * stiffness).sqrt());
    (w0, zeta)
}

/// Solve `spring` released from a unit displacement at rest — the shared
/// construction behind [`Ramp::progress`].
///
/// Bridges the catalog's [`SpringDescription`] (mass / stiffness / damping
/// *coefficient*) onto the framework solver's [`SpringDesc`](frust::SpringDesc)
/// (mass / stiffness / damping *ratio*); the two describe the same
/// mass-spring-damper, and the ratio is the coefficient over its critical value.
fn solve_spring(spring: SpringDescription) -> frust::Spring {
    let (_, zeta) = spring_shape(spring);
    let desc = frust::SpringDesc {
        mass: if spring.mass > 0.0 { spring.mass } else { 1.0 },
        stiffness: if spring.stiffness > 0.0 {
            spring.stiffness
        } else {
            1.0
        },
        damping_ratio: zeta,
    };
    frust::Spring::new(desc, 1.0, 0.0)
}

/// The analytic settle estimate described on [`Ramp::settle`].
fn spring_settle(spring: SpringDescription) -> Duration {
    let (w0, zeta) = spring_shape(spring);
    let rate = if zeta <= 1.0 {
        zeta * w0
    } else {
        w0 * (zeta - (zeta * zeta - 1.0).sqrt())
    };
    if rate <= 0.0 || !rate.is_finite() {
        return SPRING_SETTLE_CAP;
    }
    let seconds = -SPRING_SETTLE_EPSILON.ln() / rate;
    if !seconds.is_finite() {
        return SPRING_SETTLE_CAP;
    }
    // `Duration::from_secs_f64` panics on a value that overflows `Duration`'s
    // own range — a damping ratio near zero drives `rate` near zero and
    // `seconds` past it — so the cap has to bound the `f64` *before*
    // conversion; clamping the constructed `Duration` afterward, as this used
    // to, is already too late to avoid the panic.
    let seconds = seconds.clamp(
        SPRING_SETTLE_FLOOR.as_secs_f64(),
        SPRING_SETTLE_CAP.as_secs_f64(),
    );
    Duration::from_secs_f64(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::motion::{ALL_SPRINGS, EASE_OUT, SPRING_GLIDE, SPRING_SWAP};

    /// An eased ramp is anchored at both ends and never leaves `[0, 1]`.
    #[test]
    fn an_eased_ramp_runs_zero_to_one_and_stops() {
        let ramp = Ramp::eased(Duration::from_millis(200), EASE_OUT);
        assert_eq!(ramp.progress(Duration::ZERO), 0.0);
        assert_eq!(ramp.progress(Duration::from_millis(200)), 1.0);
        // Past the end it stays there rather than continuing to climb.
        assert_eq!(ramp.progress(Duration::from_secs(5)), 1.0);
        let mid = ramp.progress(Duration::from_millis(100));
        assert!(
            (0.0..=1.0).contains(&mid),
            "eased midpoint {mid} left [0, 1]"
        );
        assert_eq!(ramp.settle(), Duration::from_millis(200));
    }

    /// A zero-length eased ramp is a step, not a division by zero.
    #[test]
    fn a_zero_length_eased_ramp_steps_straight_to_one() {
        let ramp = Ramp::eased(Duration::ZERO, EASE_OUT);
        assert_eq!(ramp.progress(Duration::ZERO), 1.0);
        assert!(ramp.is_settled(Duration::ZERO));
    }

    /// A spring starts at zero, and lands exactly on its target at its settle
    /// time rather than a thousandth short of it.
    #[test]
    fn a_spring_ramp_starts_at_zero_and_lands_exactly_on_target() {
        let ramp = Ramp::spring(SPRING_SWAP);
        assert!(ramp.progress(Duration::ZERO).abs() < 1e-9);

        // Just before the snap it is within the settle epsilon of the target...
        let nearly = ramp.progress(ramp.settle().mul_f64(0.999));
        assert!(
            (nearly - 1.0).abs() <= SPRING_SETTLE_EPSILON * 2.0,
            "not converged approaching its settle time: {nearly}"
        );
        // ...and at and after it, exactly on it.
        assert_eq!(ramp.progress(ramp.settle()), 1.0);
        assert_eq!(ramp.progress(Duration::from_secs(30)), 1.0);
    }

    /// A lightly-damped spring passes its target before settling back — the
    /// property that makes the raw/clamped split worth having. The spring here
    /// is deliberately bouncier than anything the catalog ships, because the
    /// catalog's own are near-critically damped (see the test below).
    #[test]
    fn an_under_damped_spring_overshoots_but_the_clamped_read_does_not() {
        let bouncy = SpringDescription {
            mass: 1.0,
            stiffness: 400.0,
            damping: 8.0,
        };
        let ramp = Ramp::spring(bouncy);
        let settle = ramp.settle();
        let steps = 400;
        let mut peak: f64 = 0.0;
        for step in 0..=steps {
            let at = settle.mul_f64(step as f64 / steps as f64);
            peak = peak.max(ramp.progress(at));
            assert!(ramp.progress_clamped(at) <= 1.0);
        }
        assert!(
            peak > 1.05,
            "a damping ratio of 0.2 should visibly overshoot: {peak}"
        );
    }

    /// beUI's own springs are authored close to critical damping, so none of
    /// them overshoots visibly through its settle window — a pinned property of
    /// the ported numbers, and the reason a component may drive opacity from a
    /// catalog spring without the clamp mattering.
    #[test]
    fn no_catalog_spring_overshoots_visibly() {
        for spring in ALL_SPRINGS {
            let ramp = Ramp::spring(spring);
            let settle = ramp.settle();
            let mut peak: f64 = 0.0;
            for step in 0..=200 {
                peak = peak.max(ramp.progress(settle.mul_f64(step as f64 / 200.0)));
            }
            assert!(
                peak <= 1.01,
                "{spring:?} overshot to {peak} within its settle window"
            );
        }
    }

    /// `SPRING_GLIDE` is over-damped — upstream's whole point is that a slider
    /// fill "never rebounds off an end" — so it must not pass its target at any
    /// point, not merely settle below it.
    #[test]
    fn the_over_damped_spring_never_passes_its_target() {
        let ramp = Ramp::spring(SPRING_GLIDE);
        let settle = ramp.settle();
        for step in 0..=200 {
            let at = settle.mul_f64(step as f64 / 200.0);
            assert!(
                ramp.progress(at) <= 1.0 + 1e-9,
                "SPRING_GLIDE overshot at {at:?}"
            );
        }
    }

    /// Every catalog spring settles inside a plausible interaction window —
    /// the estimate is what sizes a staggered timeline, so a wildly wrong one
    /// would show up as a list that reveals over several seconds.
    #[test]
    fn every_catalog_spring_settles_within_an_interaction_window() {
        for spring in ALL_SPRINGS {
            let settle = spring_settle(spring);
            assert!(
                settle >= SPRING_SETTLE_FLOOR && settle <= Duration::from_millis(800),
                "{spring:?} settles in {settle:?}"
            );
        }
    }

    /// A degenerate spring produces slow motion, never `NaN` and never a
    /// zero-length timeline something else divides by.
    #[test]
    fn a_degenerate_spring_falls_back_instead_of_producing_nan() {
        let dead = SpringDescription {
            mass: 0.0,
            stiffness: 0.0,
            damping: 0.0,
        };
        let ramp = Ramp::spring(dead);
        assert_eq!(ramp.settle(), SPRING_SETTLE_CAP);
        assert!(ramp.progress(Duration::from_millis(10)).is_finite());
    }

    /// A damping ratio near zero drives the analytic settle estimate's raw
    /// seconds count past what `Duration` can represent — `spring_settle`
    /// must cap it before ever building one, not after, or the conversion
    /// itself panics.
    #[test]
    fn a_near_undamped_spring_clamps_to_the_cap_instead_of_overflowing_duration() {
        let barely_damped = SpringDescription {
            mass: 1.0,
            stiffness: 100.0,
            damping: 1e-20,
        };
        let ramp = Ramp::spring(barely_damped);
        assert_eq!(ramp.settle(), SPRING_SETTLE_CAP);
        assert!(ramp.progress(Duration::from_millis(10)).is_finite());
    }
}
