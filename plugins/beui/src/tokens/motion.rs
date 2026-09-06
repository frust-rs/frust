//! beUI's motion tokens: the three shared easing curves and the six spring
//! configurations every component animates with.
//!
//! **Source:** `lib/ease.ts` of the beUI monorepo, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved **2026-09-01**. Its own
//! header states the intent this port keeps: *"Strong custom variants —
//! defaults like `ease-in`/`ease-out` feel weak."* The three curves are also
//! published as CSS custom properties (`--ease-out`, `--ease-in-out`,
//! `--ease-drawer` in `app/globals.css`'s `@theme inline` block) with identical
//! control points, so there is one source, not two.
//!
//! Everything here is a plain constant. Nothing in this module reads a clock,
//! advances anything, or allocates — a component picks a curve or a spring and
//! drives it with the framework's own animation machinery.
//!
//! # The two shapes, and why the split is upstream's
//!
//! beUI animates with two mechanisms and this module ports both as they are:
//!
//! - **Cubic-Bézier curves** ([`EASE_OUT`], [`EASE_IN_OUT`], [`EASE_DRAWER`]) —
//!   duration-based transitions, expressed as [`Curve::Cubic`] with the exact
//!   control points the source's tuples carry. A [`Timing`] pairs one with a
//!   duration; the `TIMING_*` helpers below are the ready-made pairings for the
//!   durations upstream repeats.
//! - **Springs** ([`SPRING_PRESS`], [`SPRING_SWAP`], [`SPRING_PANEL`],
//!   [`SPRING_LAYOUT`], [`SPRING_MOUSE`], [`SPRING_GLIDE`]) — physics-based
//!   transitions, expressed as [`SpringDescription`]. Upstream's
//!   `stiffness`/`damping`/`mass` map **1:1** onto frust's three fields; the
//!   numbers below are the source's, unscaled.
//!
//! # What frust's spring does not carry
//!
//! The source's spring objects are Motion (`motion/react`) transition configs,
//! and two of their keys have no [`SpringDescription`] counterpart:
//!
//! - `type: "spring"` — a discriminator selecting Motion's spring integrator
//!   over its tween one. Choosing a spring is expressed here by *naming a
//!   spring constant*, so the discriminator has nothing to encode. Note the
//!   source itself is inconsistent about it: [`SPRING_MOUSE`] and
//!   [`SPRING_GLIDE`] omit the key entirely (they are passed to `useSpring`,
//!   which is unconditionally a spring), while the other four carry it.
//! - Motion's implicit rest thresholds (`restDelta`/`restSpeed`), which it
//!   applies rather than the config stating them. frust's equivalent is the
//!   `Tolerance` a driver carries at the call site, not a property of the
//!   spring description, so a component that needs a non-default rest threshold
//!   states it where it drives the spring.
//!
//! Neither omission changes the physics: the mass-spring-damper the three ported
//! numbers define is the same system in both runtimes.
//!
//! # Durations
//!
//! `lib/ease.ts` publishes no durations — upstream writes them inline per
//! component (`duration-200`, `duration: 0.45`, …). The two [`Timing`] helpers
//! below therefore cover only the durations the *stylesheet* itself repeats
//! (the `.press` utility's 120ms, and the 200ms `transition-colors` step
//! controls share); a component that needs another duration builds its own
//! [`Timing`] from one of the curve constants rather than growing a table of
//! numbers this source does not author.

use std::time::Duration;

use frust::{Curve, SpringDescription, Timing};

// ---- Easing curves --------------------------------------------------------

/// beUI's default easing — a strong, late-settling ease-out. The curve nearly
/// every entrance, exit and color transition in the catalog uses.
///
/// Source: `EASE_OUT = [0.16, 1, 0.3, 1]` (`lib/ease.ts`), published as
/// `--ease-out: cubic-bezier(0.16, 1, 0.3, 1)`.
pub const EASE_OUT: Curve = Curve::Cubic(0.16, 1.0, 0.3, 1.0);

/// beUI's symmetric easing — a hard-in, hard-out curve for reversible motion.
///
/// Source: `EASE_IN_OUT = [0.77, 0, 0.175, 1]` (`lib/ease.ts`), published as
/// `--ease-in-out: cubic-bezier(0.77, 0, 0.175, 1)`.
pub const EASE_IN_OUT: Curve = Curve::Cubic(0.77, 0.0, 0.175, 1.0);

/// The drawer/sheet curve — the iOS-style sheet easing, flatter at the end than
/// [`EASE_OUT`] so a large surface glides to rest instead of snapping.
///
/// Source: `EASE_DRAWER = [0.32, 0.72, 0, 1]` (`lib/ease.ts`), published as
/// `--ease-drawer: cubic-bezier(0.32, 0.72, 0, 1)`.
pub const EASE_DRAWER: Curve = Curve::Cubic(0.32, 0.72, 0.0, 1.0);

// ---- Ready-made timings ---------------------------------------------------

/// The press feedback timing: `transition: transform 120ms var(--ease-out)` —
/// the `.press` utility in `app/globals.css`, the one duration that stylesheet
/// pairs with a curve directly.
pub const TIMING_PRESS: Timing = Timing::Duration(Duration::from_millis(120), EASE_OUT);

/// The color-transition timing controls share: `transition-colors duration-200`
/// on the input field and its siblings, resolved against beUI's own
/// [`EASE_OUT`] rather than Tailwind's default `ease-in-out` (the stylesheet's
/// stated position is that the CSS defaults "feel weak").
pub const TIMING_COLORS: Timing = Timing::Duration(Duration::from_millis(200), EASE_OUT);

// ---- Springs --------------------------------------------------------------

/// Press feedback on buttons and other tappable surfaces — the stiffest, most
/// immediate spring in the set.
///
/// Source: `SPRING_PRESS = { type: "spring", stiffness: 500, damping: 30, mass:
/// 0.6 }` (`lib/ease.ts`).
pub const SPRING_PRESS: SpringDescription = SpringDescription {
    mass: 0.6,
    stiffness: 500.0,
    damping: 30.0,
};

/// Content swaps — label/icon slots trading places inside a control.
///
/// Source: `SPRING_SWAP = { type: "spring", stiffness: 460, damping: 30, mass:
/// 0.55 }` (`lib/ease.ts`).
pub const SPRING_SWAP: SpringDescription = SpringDescription {
    mass: 0.55,
    stiffness: 460.0,
    damping: 30.0,
};

/// Overlay panel entrances — modals and sheets summoned by pointer.
///
/// Source: `SPRING_PANEL = { type: "spring", stiffness: 420, damping: 40, mass:
/// 0.5 }` (`lib/ease.ts`).
pub const SPRING_PANEL: SpringDescription = SpringDescription {
    mass: 0.5,
    stiffness: 420.0,
    damping: 40.0,
};

/// Shared-layout glides — pills, indicators and panels morphing between
/// positions.
///
/// Source: `SPRING_LAYOUT = { type: "spring", stiffness: 360, damping: 32,
/// mass: 0.6 }` (`lib/ease.ts`).
pub const SPRING_LAYOUT: SpringDescription = SpringDescription {
    mass: 0.6,
    stiffness: 360.0,
    damping: 32.0,
};

/// Cursor-follow physics for decorative pointer tracking (magnetic pulls, tilt,
/// dock magnification) — by far the softest spring in the set (stiffness 200
/// against the next-lowest 360), which is what gives a tracked element its
/// visible lag behind the pointer.
///
/// Source: `SPRING_MOUSE = { stiffness: 200, damping: 15, mass: 0.3 }`
/// (`lib/ease.ts` — no `type` key; it is a `useSpring` config).
pub const SPRING_MOUSE: SpringDescription = SpringDescription {
    mass: 0.3,
    stiffness: 200.0,
    damping: 15.0,
};

/// Dragged handles and fills (sliders) — the source calls this "critically
/// damped … so the value follows the pointer butterily and never rebounds off
/// an end".
///
/// Source: `SPRING_GLIDE = { stiffness: 700, damping: 50, mass: 0.5 }`
/// (`lib/ease.ts` — no `type` key; it is a `useSpring` config).
///
/// The numbers are **over**-damped rather than critically damped: critical
/// damping for this mass and stiffness is `2·√(0.5 · 700) ≈ 37.4`, and the
/// source authors 50 (a damping ratio of ≈ 1.34). "Critically damped" is
/// upstream's description of the *intent* — no rebound off an end — which the
/// numbers do satisfy. The port keeps the numbers, not the description.
pub const SPRING_GLIDE: SpringDescription = SpringDescription {
    mass: 0.5,
    stiffness: 700.0,
    damping: 50.0,
};

/// Every spring this module publishes, in `lib/ease.ts`'s own declaration
/// order — the seam a test or a debug overlay enumerates the set through.
pub const ALL_SPRINGS: [SpringDescription; 6] = [
    SPRING_PRESS,
    SPRING_SWAP,
    SPRING_PANEL,
    SPRING_LAYOUT,
    SPRING_MOUSE,
    SPRING_GLIDE,
];

/// Every easing curve this module publishes, in `lib/ease.ts`'s own declaration
/// order.
pub const ALL_CURVES: [Curve; 3] = [EASE_OUT, EASE_IN_OUT, EASE_DRAWER];

#[cfg(test)]
mod tests {
    use super::*;

    /// The curves' control points, pinned against the source tuples verbatim.
    /// A curve is invisible in a unit test otherwise — `Curve::Cubic` evaluates
    /// to plausible-looking numbers for *any* control points — so the pin is
    /// the transcription check.
    #[test]
    fn the_curves_carry_the_source_control_points() {
        assert_eq!(EASE_OUT, Curve::Cubic(0.16, 1.0, 0.3, 1.0));
        assert_eq!(EASE_IN_OUT, Curve::Cubic(0.77, 0.0, 0.175, 1.0));
        assert_eq!(EASE_DRAWER, Curve::Cubic(0.32, 0.72, 0.0, 1.0));
        // And they are genuinely beUI's own, not one of the CSS defaults the
        // source explicitly rejects as "weak".
        for curve in ALL_CURVES {
            assert_ne!(curve, Curve::EaseOut);
            assert_ne!(curve, Curve::EaseInOut);
            assert_ne!(curve, Curve::Linear);
        }
    }

    /// Every curve is a well-formed timing function: anchored at both ends and
    /// monotonically non-decreasing across the timeline. `EASE_OUT`'s second
    /// control point sits at `y = 1`, which is exactly the shape that would
    /// overshoot if a control point were transposed.
    #[test]
    fn every_curve_runs_from_zero_to_one_without_going_backwards() {
        for curve in ALL_CURVES {
            assert_eq!(curve.transform(0.0), 0.0);
            assert_eq!(curve.transform(1.0), 1.0);
            let mut previous = 0.0;
            for step in 0..=100 {
                let progress = curve.transform(f64::from(step) / 100.0);
                assert!(
                    progress >= previous - 1e-9,
                    "curve went backwards at t = {step}/100"
                );
                assert!((-1e-9..=1.0 + 1e-9).contains(&progress));
                previous = progress;
            }
        }
    }

    /// `EASE_OUT` front-loads its motion — that is what "strong ease-out" means
    /// and what distinguishes it from the symmetric curve beside it.
    #[test]
    fn ease_out_is_front_loaded_and_ease_in_out_is_not() {
        assert!(
            EASE_OUT.transform(0.25) > 0.5,
            "quarter-way, past half-done"
        );
        assert!(EASE_IN_OUT.transform(0.25) < 0.25, "still winding up");
        // The drawer curve is gentler off the mark than EASE_OUT, which is the
        // reason it exists as a separate token.
        assert!(EASE_DRAWER.transform(0.25) < EASE_OUT.transform(0.25));
    }

    /// The spring numbers, pinned field-by-field against `lib/ease.ts`.
    #[test]
    fn the_springs_carry_the_source_numbers() {
        let expected = [
            ("SPRING_PRESS", SPRING_PRESS, 0.6, 500.0, 30.0),
            ("SPRING_SWAP", SPRING_SWAP, 0.55, 460.0, 30.0),
            ("SPRING_PANEL", SPRING_PANEL, 0.5, 420.0, 40.0),
            ("SPRING_LAYOUT", SPRING_LAYOUT, 0.6, 360.0, 32.0),
            ("SPRING_MOUSE", SPRING_MOUSE, 0.3, 200.0, 15.0),
            ("SPRING_GLIDE", SPRING_GLIDE, 0.5, 700.0, 50.0),
        ];
        for (name, spring, mass, stiffness, damping) in expected {
            assert_eq!(spring.mass, mass, "{name} mass");
            assert_eq!(spring.stiffness, stiffness, "{name} stiffness");
            assert_eq!(spring.damping, damping, "{name} damping");
        }
        assert_eq!(ALL_SPRINGS.len(), expected.len());
    }

    /// The damping regime each spring lands in, derived from the ported
    /// numbers rather than asserted from the source's prose.
    ///
    /// This is the check that would catch a transposed `damping`/`stiffness`
    /// pair — a swap the field-by-field pin above cannot see if both numbers
    /// move together — and it records a finding worth knowing before a
    /// component picks a spring: the three *direct-manipulation* springs
    /// (press, swap, cursor-follow) are underdamped and overshoot, while the
    /// three *settling* springs (panel, layout, slider glide) are overdamped
    /// and do not. `SPRING_GLIDE` in particular is over-, not critically,
    /// damped, whatever its source comment says.
    #[test]
    fn each_spring_lands_in_its_expected_damping_regime() {
        let ratio = |s: SpringDescription| s.damping / (2.0 * (s.mass * s.stiffness).sqrt());
        for (name, spring, underdamped) in [
            ("SPRING_PRESS", SPRING_PRESS, true),
            ("SPRING_SWAP", SPRING_SWAP, true),
            ("SPRING_MOUSE", SPRING_MOUSE, true),
            ("SPRING_PANEL", SPRING_PANEL, false),
            ("SPRING_LAYOUT", SPRING_LAYOUT, false),
            ("SPRING_GLIDE", SPRING_GLIDE, false),
        ] {
            let zeta = ratio(spring);
            assert_eq!(
                zeta < 1.0,
                underdamped,
                "{name} damping ratio {zeta} contradicts its documented regime"
            );
        }
        // The cursor-follow spring is the softest in the set — the property
        // that produces its lag behind a moving pointer.
        for spring in ALL_SPRINGS {
            assert!(SPRING_MOUSE.stiffness <= spring.stiffness);
        }
    }

    /// The ready-made timings pair the source's durations with beUI's own
    /// curve, never a framework default.
    #[test]
    fn the_ready_made_timings_carry_beui_s_own_curve() {
        assert_eq!(
            TIMING_PRESS,
            Timing::Duration(Duration::from_millis(120), EASE_OUT)
        );
        assert_eq!(
            TIMING_COLORS,
            Timing::Duration(Duration::from_millis(200), EASE_OUT)
        );
        assert_ne!(TIMING_PRESS, Timing::ThemeDefault);
    }
}
