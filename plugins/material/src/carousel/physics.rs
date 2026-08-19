//! The carousel's snap physics: which item a released drag lands on, and the
//! ramp that takes the content there. Pure math plus one controller factory —
//! nothing here touches a `PaintScene`, a `ChildPod`, or a callback.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/carousel/components/m3e_carousel_scroll_physics.dart` and
//! `m3e_carousel_position.dart` (retrieved 2026-08-19), themselves vendored from
//! `m3_carousel` (MIT, © 2024 Paa) and derived from Flutter's `CarouselView` —
//! **BSD-3-Clause, © 2014 The Flutter Authors**; see `plugins/material/NOTICE`.
//!
//! # Scroll velocity, not finger velocity
//!
//! Every velocity here is a **scroll** velocity in logical px/s, positive when
//! the content is moving forward through the list — the sign convention
//! upstream's `createBallisticSimulation` uses. A finger moving toward the
//! leading edge scrolls forward, so [`super`] negates the tracked finger
//! velocity before calling in.
//!
//! # Ramp shape: a duration, not a spring
//!
//! Upstream has two settle paths, and this port follows the one its own
//! top-level widget takes: `M3ECarousel` drives every gesture step through
//! `controller.animateTo(..., duration: 500ms, curve: Curves.ease)`, and only
//! its opt-in free-scroll mode reaches `M3ECarouselScrollPhysics`'
//! `ScrollSpringSimulation`. Free scroll is not ported (see [`super`]'s docs),
//! so the duration ramp is the only one here — [`SETTLE_DURATION`] and
//! [`SETTLE_CURVE`] reproduce that pair exactly.
//!
//! # Clamp discipline
//!
//! No `f64::clamp` against a computed pair (`slider/core.rs`'s *Clamp
//! discipline*): [`snap_offset`]'s only bound is applied as `.max(0.0)` then
//! `.min(max_offset.max(0.0))`, an ordering visible at the call site, and every
//! entry point returns early on a non-positive or non-finite stride rather than
//! dividing by it.

use std::time::Duration;

use frust::{AnimationController, Curve};

use super::layout::PRECISION_TOLERANCE;
use crate::tokens::MaterialMotion;

/// Scroll speed below which a release counts as a settle rather than a fling,
/// in logical px/s.
///
/// Flutter's `Tolerance.velocity` is `1 / (0.050 * devicePixelRatio)` px/s in
/// *physical* pixels, which is `20.0` logical px/s at any density once the
/// ratio cancels — and logical px is the only space a frust widget ever sees
/// (`docs/CODE_STANDARDS.md`'s logical-coordinate rule).
pub(crate) const SNAP_VELOCITY_TOLERANCE: f64 = 20.0;

/// How long the content takes to settle onto an item
/// (`M3ECarouselTheme.defaultScrollAnimationDuration`, 500ms — the same value
/// [`MaterialMotion::LONG_2`] carries).
pub(crate) const SETTLE_DURATION: Duration = MaterialMotion::LONG_2;

/// The settle ramp's easing — Flutter's `Curves.ease`, i.e.
/// `cubic-bezier(0.25, 0.1, 0.25, 1.0)`, which `M3ECarousel.scrollFrame` passes
/// to `animateTo`. Deliberately *not* one of this crate's own easing tokens:
/// the reference names this curve explicitly for this motion.
pub(crate) const SETTLE_CURVE: Curve = Curve::Cubic(0.25, 0.1, 0.25, 1.0);

/// A fresh, idle settle controller at [`SETTLE_DURATION`]/[`SETTLE_CURVE`].
pub(crate) fn settle_controller() -> AnimationController {
    AnimationController::new(SETTLE_DURATION).with_curve(SETTLE_CURVE)
}

/// The continuous item index a scroll offset sits at
/// (`_CarouselPosition.getItemFromPixels`): `offset / stride`, snapped to a
/// whole item when it lands within [`PRECISION_TOLERANCE`] of one.
///
/// `0.0` for a non-positive or non-finite `stride` — there is no item scale to
/// divide by.
pub(crate) fn continuous_item(offset: f64, stride: f64) -> f64 {
    if stride <= 0.0 || !stride.is_finite() || !offset.is_finite() {
        return 0.0;
    }
    let actual = offset.max(0.0) / stride;
    if !actual.is_finite() {
        return 0.0;
    }
    let round = actual.round();
    if (actual - round).abs() < PRECISION_TOLERANCE {
        round
    } else {
        actual
    }
}

/// The whole item index a scroll offset is nearest to — upstream's
/// `_reportedLeadingIndex`, which **rounds** the continuous item rather than
/// truncating it so a swipe in either direction reports its new leading item at
/// the halfway mark instead of only once the old one has nearly left.
pub(crate) fn nearest_item(offset: f64, stride: f64) -> usize {
    let rounded = continuous_item(offset, stride).round();
    if rounded <= 0.0 { 0 } else { rounded as usize }
}

/// The scroll offset a release at `scroll_velocity` settles onto
/// (`M3ECarouselScrollPhysics._getTargetPixels`).
///
/// A release slower than [`SNAP_VELOCITY_TOLERANCE`] lands on the nearest item
/// boundary; a faster one biases the continuous item by half a step in the
/// fling's own direction first, so any recognised fling advances by exactly one
/// item however short the drag was. The result is bounded to
/// `0.0..=max_offset`.
pub(crate) fn snap_offset(offset: f64, stride: f64, scroll_velocity: f64, max_offset: f64) -> f64 {
    if stride <= 0.0 || !stride.is_finite() {
        return 0.0;
    }
    let mut item = continuous_item(offset, stride);
    if scroll_velocity.is_finite() {
        if scroll_velocity < -SNAP_VELOCITY_TOLERANCE {
            item -= 0.5;
        } else if scroll_velocity > SNAP_VELOCITY_TOLERANCE {
            item += 0.5;
        }
    }
    let target = item.round() * stride;
    // Ordered explicitly rather than `clamp` (module docs): the lower bound is
    // the literal `0.0` and the upper is floored to it right here.
    let upper = if max_offset.is_finite() {
        max_offset.max(0.0)
    } else {
        0.0
    };
    target.max(0.0).min(upper)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRIDE: f64 = 200.0;
    const MAX: f64 = 1200.0;

    #[test]
    fn a_slow_release_settles_onto_the_nearest_boundary() {
        // Just past the first third of an item: back to where it came from.
        assert_eq!(snap_offset(70.0, STRIDE, 0.0, MAX), 0.0);
        // Just past halfway: on to the next one.
        assert_eq!(snap_offset(130.0, STRIDE, 0.0, MAX), STRIDE);
        // Exactly on a boundary: nothing moves.
        assert_eq!(snap_offset(400.0, STRIDE, 0.0, MAX), 400.0);
    }

    #[test]
    fn a_release_below_the_velocity_tolerance_is_still_a_settle() {
        let creeping = SNAP_VELOCITY_TOLERANCE - 1.0;
        assert_eq!(snap_offset(70.0, STRIDE, creeping, MAX), 0.0);
        assert_eq!(snap_offset(70.0, STRIDE, -creeping, MAX), 0.0);
    }

    #[test]
    fn a_forward_fling_advances_one_item_however_short_the_drag() {
        // 10px into the item — nowhere near halfway — but flung forward.
        let fast = SNAP_VELOCITY_TOLERANCE * 40.0;
        assert_eq!(snap_offset(10.0, STRIDE, fast, MAX), STRIDE);
        assert_eq!(snap_offset(210.0, STRIDE, fast, MAX), STRIDE * 2.0);
    }

    #[test]
    fn a_backward_fling_biases_toward_the_previous_boundary() {
        let fast = -SNAP_VELOCITY_TOLERANCE * 40.0;
        // Nearly onto item 2, but flung back: it returns to item 1.
        assert_eq!(snap_offset(390.0, STRIDE, fast, MAX), STRIDE);
        // Just past item 2: the same fling settles back onto item 2 rather
        // than carrying on to item 3.
        assert_eq!(snap_offset(410.0, STRIDE, fast, MAX), STRIDE * 2.0);
    }

    #[test]
    fn a_fling_never_leaves_the_scrollable_range() {
        let fast = SNAP_VELOCITY_TOLERANCE * 40.0;
        assert_eq!(snap_offset(MAX, STRIDE, fast, MAX), MAX);
        assert_eq!(snap_offset(0.0, STRIDE, -fast, MAX), 0.0);
        assert_eq!(snap_offset(500.0, STRIDE, fast, 0.0), 0.0);
    }

    #[test]
    fn a_degenerate_stride_or_bound_degrades_to_zero_instead_of_dividing() {
        assert_eq!(snap_offset(300.0, 0.0, 500.0, MAX), 0.0);
        assert_eq!(snap_offset(300.0, -5.0, 500.0, MAX), 0.0);
        assert_eq!(snap_offset(300.0, f64::NAN, 500.0, MAX), 0.0);
        assert_eq!(snap_offset(300.0, STRIDE, f64::NAN, MAX), 400.0);
        assert_eq!(snap_offset(300.0, STRIDE, 0.0, f64::NAN), 0.0);
    }

    #[test]
    fn the_continuous_item_pins_to_a_boundary_within_tolerance() {
        assert_eq!(continuous_item(STRIDE, STRIDE), 1.0);
        assert_eq!(continuous_item(STRIDE - 1e-12, STRIDE), 1.0);
        assert!((continuous_item(STRIDE * 1.5, STRIDE) - 1.5).abs() < 1e-12);
        assert_eq!(continuous_item(-50.0, STRIDE), 0.0, "never negative");
        assert_eq!(continuous_item(50.0, 0.0), 0.0);
    }

    #[test]
    fn the_nearest_item_crosses_at_the_halfway_mark_in_both_directions() {
        assert_eq!(nearest_item(0.0, STRIDE), 0);
        assert_eq!(nearest_item(99.0, STRIDE), 0);
        assert_eq!(nearest_item(101.0, STRIDE), 1);
        assert_eq!(nearest_item(299.0, STRIDE), 1);
        assert_eq!(nearest_item(301.0, STRIDE), 2);
        assert_eq!(nearest_item(-500.0, STRIDE), 0);
    }

    #[test]
    fn the_settle_ramp_is_the_references_own_duration_and_curve() {
        assert_eq!(SETTLE_DURATION, Duration::from_millis(500));
        assert_eq!(SETTLE_CURVE, Curve::Cubic(0.25, 0.1, 0.25, 1.0));
        let anim = settle_controller();
        assert!(!anim.is_animating(), "a fresh controller is idle");
        assert_eq!(anim.value(), 0.0);
    }
}
