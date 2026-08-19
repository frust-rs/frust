// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// whose `lib/components/buttons/` tree is itself vendored from m3e_buttons
// (MIT, © 2026 Mudit Purohit) — `styles/m3e_button_motion.dart` +
// `components/m3e_radius_and_padding_motion.dart`.
// Porting decision: a mid-flight retarget restarts the spring *from the
// current value* rather than flipping a 0/1 progress target (see `retarget`).

//! The button press morph: one spring progress driving the container's corner
//! radius **and** its content padding together.
//!
//! Ports `M3ERadiusAndPaddingMotion` — the reference's shared morph primitive —
//! down to what a retained widget needs: a pair of `(from, to)` endpoints per
//! animated channel plus one [`frust::AnimationController`] flung along
//! [`PRESS_SPRING`]. Every corner and every padding edge moves off that single
//! progress value, exactly as the Dart widget's one `SingleMotionBuilder` does.
//!
//! # The spring: `EXPRESSIVE_SPATIAL_PRESS` (380 / 0.55)
//!
//! `m3e_button_motion.dart:56` declares `expressiveSpatialPress` as stiffness
//! 380 / damping ratio 0.55 and `m3e_base_button_state.dart:183` makes it the
//! default motion for every button, so that is what [`PRESS_SPRING`] carries —
//! the same preset [`crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`]
//! names and [`mod@crate::fab`] flings its press scale along. It is a named
//! constant rather than a theme read for the reason every press spring in this
//! crate is: a press starts in the event pass, and event-pass code never reads
//! a theme (`docs/CODE_STANDARDS.md`'s Theming conventions). The tripwire test
//! `press_spring_matches_expressive_spatial_press` keeps it equal to the token.
//!
//! # Retarget continuity — a deliberate divergence from the reference
//!
//! The Dart widget retargets by copying its current values into `_src*` and
//! then **flipping** a `0.0`/`1.0` progress target
//! (`m3e_radius_and_padding_motion.dart:265`), reading the interpolation factor
//! back as `t` or `1 - t`. Flipping mid-flight is discontinuous: at a flip from
//! a forward run sitting at `t = 0.4` the factor jumps to `0.6`, so the painted
//! radius jumps with it.
//!
//! [`RadiusPaddingMotion::retarget`] instead pins `from` to the value being
//! painted *right now* and starts a fresh fling at `t = 0`, so the first frame
//! after a retarget paints exactly what the previous frame painted and the
//! spring runs on from there — press-then-release-mid-flight morphs
//! continuously. `retarget_mid_flight_is_continuous` samples that.
//!
//! # Tolerances
//!
//! [`RETARGET_TOLERANCE`] (0.1 logical px, the reference's
//! `kSpringRetargetTolerance`) is the dead band a repeated target must exceed
//! to start a new leg — without it, a paint pass re-resolving the same target
//! from float math could restart the spring every frame.
//! [`OVERSHOOT_LIMIT`] mirrors the reference's `clamp(0.0, 1.5)`: an
//! under-damped spring's overshoot past the target is real motion and is let
//! through, up to 50% past it.

use std::time::Duration;

use frust::{AnimationController, FrameTime, SpringDesc};

/// The press morph's spring: stiffness 380, damping ratio 0.55, mass 1 —
/// `m3e_button_motion.dart:56`'s `expressiveSpatialPress`, i.e.
/// [`crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`]. See the
/// [module docs](self).
pub(crate) const PRESS_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 380.0,
    damping_ratio: 0.55,
};

/// Nominal period seeding the morph [`AnimationController`]'s clock. The
/// motion is spring-driven ([`PRESS_SPRING`]) via `fling`, so this duration
/// only backs the controller's construction and is not itself a timing
/// (mirrors [`mod@crate::fab`]'s `PRESS_ANIM_PERIOD`).
const PRESS_ANIM_PERIOD: Duration = Duration::from_millis(300);

/// Launch velocity (progress-units/sec) handed to each leg's
/// [`AnimationController::fling`] — the same modest kick [`mod@crate::fab`]
/// and [`mod@crate::button_group`] use, so a press reads snappy rather than
/// creeping off zero.
const FLING_VELOCITY: f64 = 4.0;

/// The dead band (logical px) a new target must exceed before it starts a new
/// spring leg — the reference's `kSpringRetargetTolerance`
/// (`m3e_radius_and_padding_motion.dart:18`).
pub(crate) const RETARGET_TOLERANCE: f64 = 0.1;

/// How far past its target the spring's progress is allowed to read, mirroring
/// the reference's `rawFactor.clamp(0.0, 1.5)`
/// (`m3e_radius_and_padding_motion.dart:403`).
const OVERSHOOT_LIMIT: f64 = 1.5;

/// The button's internal content padding — the reference's `internalLeft`/
/// `internalRight`/`internalTop`/`internalBottom` quartet, which
/// [`RadiusPaddingMotion`] animates alongside the corner radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ContentPadding {
    pub(crate) left: f64,
    pub(crate) right: f64,
    pub(crate) top: f64,
    pub(crate) bottom: f64,
}

impl ContentPadding {
    /// Horizontal-only padding (`EdgeInsets.symmetric(horizontal: h)`) — the
    /// only shape a plain button ever builds, since its height is fixed by its
    /// size token rather than by vertical padding
    /// (`m3e_button_content.dart:7`).
    pub(crate) const fn symmetric(horizontal: f64) -> Self {
        Self {
            left: horizontal,
            right: horizontal,
            top: 0.0,
            bottom: 0.0,
        }
    }

    /// Linear interpolation of every edge at `t`.
    fn lerp(self, to: Self, t: f64) -> Self {
        Self {
            left: lerp(self.left, to.left, t),
            right: lerp(self.right, to.right, t),
            top: lerp(self.top, to.top, t),
            bottom: lerp(self.bottom, to.bottom, t),
        }
    }

    /// Whether every edge is within [`RETARGET_TOLERANCE`] of `other`'s.
    fn matches(self, other: Self) -> bool {
        (self.left - other.left).abs() <= RETARGET_TOLERANCE
            && (self.right - other.right).abs() <= RETARGET_TOLERANCE
            && (self.top - other.top).abs() <= RETARGET_TOLERANCE
            && (self.bottom - other.bottom).abs() <= RETARGET_TOLERANCE
    }
}

/// Linear interpolation between `from` and `to` at `t` (unclamped — the
/// spring's overshoot rides past `t = 1`).
fn lerp(from: f64, to: f64, t: f64) -> f64 {
    from + (to - from) * t
}

/// The radius+padding press morph a [`super::ButtonWidget`] owns. See the
/// [module docs](self).
#[derive(Clone, Copy, Debug)]
pub(crate) struct RadiusPaddingMotion {
    from_radius: f64,
    to_radius: f64,
    from_padding: ContentPadding,
    to_padding: ContentPadding,
    anim: AnimationController,
    /// Whether a first target has been seeded. The mount frame snaps to
    /// whatever it resolves (an `AnimatedContainer`'s first build is never
    /// itself animated); only a later target change springs.
    seeded: bool,
}

impl RadiusPaddingMotion {
    /// An unseeded morph resting at radius `0` and zero padding — the first
    /// [`Self::retarget`] snaps to its argument instead of springing to it.
    pub(crate) fn new() -> Self {
        let zero = ContentPadding {
            left: 0.0,
            right: 0.0,
            top: 0.0,
            bottom: 0.0,
        };
        Self {
            from_radius: 0.0,
            to_radius: 0.0,
            from_padding: zero,
            to_padding: zero,
            anim: AnimationController::new(PRESS_ANIM_PERIOD),
            seeded: false,
        }
    }

    /// Aim the morph at `radius`/`padding`, returning whether this actually
    /// started a new spring leg.
    ///
    /// Three outcomes, in order: an unseeded morph **snaps** (mount frame); a
    /// target within [`RETARGET_TOLERANCE`] of the live one is **ignored**
    /// (the dead band that keeps a re-resolving paint pass from restarting the
    /// spring every frame); anything else **springs**, from the value being
    /// painted right now — see the [module docs](self)' continuity note.
    pub(crate) fn retarget(&mut self, radius: f64, padding: ContentPadding) -> bool {
        if !self.seeded {
            self.snap_to(radius, padding);
            return false;
        }
        if (self.to_radius - radius).abs() <= RETARGET_TOLERANCE && self.to_padding.matches(padding)
        {
            return false;
        }
        self.from_radius = self.radius();
        self.from_padding = self.padding();
        self.to_radius = radius;
        self.to_padding = padding;
        self.anim = AnimationController::new(PRESS_ANIM_PERIOD);
        self.anim.fling(FLING_VELOCITY, PRESS_SPRING);
        true
    }

    /// Pin both channels to `radius`/`padding` with no motion at all (the
    /// reference's `snapToTarget`), marking the morph seeded.
    pub(crate) fn snap_to(&mut self, radius: f64, padding: ContentPadding) {
        self.from_radius = radius;
        self.to_radius = radius;
        self.from_padding = padding;
        self.to_padding = padding;
        self.anim = AnimationController::new(PRESS_ANIM_PERIOD);
        self.seeded = true;
    }

    /// Advance the spring to frame time `now`, returning whether it is still
    /// animating (in which case the caller must request another frame).
    pub(crate) fn advance(&mut self, now: FrameTime) -> bool {
        self.anim.advance(now)
    }

    /// Whether a first target has been seeded — i.e. whether the values this
    /// morph reports mean anything yet.
    pub(crate) fn is_seeded(&self) -> bool {
        self.seeded
    }

    /// The progress factor this frame paints at: the spring's value, floored
    /// at `0` and capped at [`OVERSHOOT_LIMIT`].
    fn factor(&self) -> f64 {
        let raw = self.anim.value();
        if raw.is_finite() {
            raw.clamp(0.0, OVERSHOOT_LIMIT)
        } else {
            0.0
        }
    }

    /// The corner radius to paint this frame (never negative — the
    /// reference's `v < 0 ? 0.0 : v` corner guard, which an under-damped
    /// overshoot toward a small target can otherwise drive past zero).
    pub(crate) fn radius(&self) -> f64 {
        lerp(self.from_radius, self.to_radius, self.factor()).max(0.0)
    }

    /// The content padding to lay out with this frame.
    pub(crate) fn padding(&self) -> ContentPadding {
        self.from_padding.lerp(self.to_padding, self.factor())
    }

    /// The radius the morph is currently springing *toward* — what a resting
    /// morph already paints.
    #[cfg(test)]
    pub(crate) fn target_radius(&self) -> f64 {
        self.to_radius
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    /// Advance `motion` in 16ms steps until it settles (or `max_steps` is
    /// exhausted), returning the number of steps taken.
    fn settle(motion: &mut RadiusPaddingMotion, max_steps: usize) -> usize {
        let mut t = 0.0;
        for step in 0..max_steps {
            t += 1.0 / 60.0;
            if !motion.advance(ft_secs(t)) {
                return step;
            }
        }
        max_steps
    }

    const PAD: ContentPadding = ContentPadding::symmetric(16.0);

    #[test]
    fn press_spring_matches_expressive_spatial_press() {
        // m3e_button_motion.dart:56 — expressiveSpatialPress (380 / 0.55),
        // the default motion m3e_base_button_state.dart:183 applies.
        let token = crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS;
        assert_eq!(PRESS_SPRING.stiffness, token.stiffness);
        assert_eq!(PRESS_SPRING.damping_ratio, token.damping_ratio);
        assert_eq!(PRESS_SPRING.mass, 1.0);
        assert_eq!(PRESS_SPRING.stiffness, 380.0);
        assert_eq!(PRESS_SPRING.damping_ratio, 0.55);
    }

    #[test]
    fn the_first_target_snaps_rather_than_springing() {
        let mut motion = RadiusPaddingMotion::new();
        assert!(
            !motion.retarget(20.0, PAD),
            "the mount frame seeds, it does not animate"
        );
        assert_eq!(motion.radius(), 20.0);
        assert_eq!(motion.padding(), PAD);
        assert!(!motion.advance(ft_secs(0.0)), "nothing to advance");
    }

    #[test]
    fn a_target_inside_the_dead_band_is_ignored() {
        let mut motion = RadiusPaddingMotion::new();
        motion.retarget(20.0, PAD);
        assert!(
            !motion.retarget(20.0 + RETARGET_TOLERANCE / 2.0, PAD),
            "a sub-tolerance change must not restart the spring"
        );
        assert_eq!(motion.target_radius(), 20.0);
        assert!(
            motion.retarget(20.0 + RETARGET_TOLERANCE * 2.0, PAD),
            "a change past the tolerance starts a leg"
        );
    }

    #[test]
    fn a_press_springs_from_the_rest_radius_to_the_pressed_one() {
        let mut motion = RadiusPaddingMotion::new();
        motion.retarget(20.0, PAD); // sm round: height / 2
        assert!(motion.retarget(8.0, PAD), "the press starts a leg");
        assert_eq!(motion.radius(), 20.0, "the leg starts where it was");
        settle(&mut motion, 600);
        assert_eq!(motion.radius(), 8.0, "and settles exactly on target");
    }

    #[test]
    fn retarget_mid_flight_is_continuous() {
        let mut motion = RadiusPaddingMotion::new();
        motion.retarget(20.0, PAD);
        motion.retarget(8.0, PAD);
        // Two advances: the first only seeds the controller's clock.
        motion.advance(ft_secs(0.0));
        motion.advance(ft_secs(0.05));
        let mid = motion.radius();
        assert!(
            mid < 20.0 && mid > 8.0,
            "sampled mid-flight (got {mid}), not at either endpoint"
        );

        // Release while still in flight: the next painted value must be the
        // one just painted — no jump (the divergence from the reference's
        // progress-flip documented in the module docs).
        assert!(motion.retarget(20.0, PAD));
        assert_eq!(motion.radius(), mid);

        // ...and it converges on the rest radius from there.
        settle(&mut motion, 600);
        assert_eq!(motion.radius(), 20.0);
    }

    #[test]
    fn padding_rides_the_same_progress_as_the_radius() {
        let mut motion = RadiusPaddingMotion::new();
        motion.retarget(20.0, ContentPadding::symmetric(16.0));
        motion.retarget(8.0, ContentPadding::symmetric(24.0));
        motion.advance(ft_secs(0.0));
        motion.advance(ft_secs(0.05));

        let factor = motion.factor();
        assert_eq!(motion.radius(), lerp(20.0, 8.0, factor));
        let pad = motion.padding();
        assert_eq!(pad.left, lerp(16.0, 24.0, factor));
        assert_eq!(pad.right, lerp(16.0, 24.0, factor));
        assert_eq!(pad.top, 0.0, "a plain button has no vertical padding");
        assert_eq!(pad.bottom, 0.0);

        settle(&mut motion, 600);
        assert_eq!(motion.padding(), ContentPadding::symmetric(24.0));
    }

    #[test]
    fn overshoot_never_drives_a_corner_negative() {
        // 380/0.55 is under-damped, so a leg toward 0 overshoots past it; the
        // reference's corner guard clamps the painted radius at 0.
        let mut motion = RadiusPaddingMotion::new();
        motion.retarget(28.0, PAD);
        motion.retarget(0.0, PAD);
        let mut t = 0.0;
        for _ in 0..600 {
            t += 1.0 / 60.0;
            let animating = motion.advance(ft_secs(t));
            assert!(motion.radius() >= 0.0, "radius went negative at t={t}");
            if !animating {
                break;
            }
        }
        assert_eq!(motion.radius(), 0.0);
    }

    #[test]
    fn snap_to_moves_without_animating() {
        let mut motion = RadiusPaddingMotion::new();
        motion.retarget(20.0, PAD);
        motion.retarget(8.0, PAD);
        motion.snap_to(28.0, ContentPadding::symmetric(48.0));
        assert_eq!(motion.radius(), 28.0);
        assert_eq!(motion.padding(), ContentPadding::symmetric(48.0));
        assert!(
            !motion.advance(ft_secs(1.0)),
            "a snap leaves nothing running"
        );
    }
}
