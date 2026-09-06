//! [`SampleReveal`]: the design system's own [`TransitionPattern`] — a short
//! lift-and-fade, fed to
//! [`pattern_switcher`](frust::motion::switcher::pattern_switcher).
//!
//! # Why this is here at all
//!
//! `frust::motion` is the framework's implicit-animation and
//! transition-pattern vocabulary, and it lives **outside** the three
//! feature-gated catalogs — the facade re-exports it wholesale and ungated. So
//! a design system built with `default-features = false` still gets the whole
//! motion seam, including the ability to define its own patterns. That is a
//! fact worth pinning rather than assuming: this module compiling at all is
//! the proof (see this workspace's README).
//!
//! # The pattern
//!
//! Incoming: rises [`LIFT_DISTANCE`] logical px into place while fading in.
//! Exiting: fades out and sinks by the same distance. Both directions flip
//! under `reverse`, so a "back" switch reads as a descent.
//!
//! Neither `resolve` nor this module resolves *timing* or handles
//! `reduce_motion`: a `TransitionPattern` is pure staging math over a progress
//! value, and `PatternSwitcher` owns both — it resolves duration/easing from
//! `Theme.motion` (an explicit `.timing(...)` always wins) and collapses **any**
//! pattern to a fast linear crossfade under `reduce_motion`. A pattern that
//! hand-rolled a reduced variant would be fighting that hard accessibility
//! rule.

use frust::authoring::Size;
use frust::motion::patterns::{PatternLayer, TransitionPattern};

/// How far the incoming child rises (and the exiting child sinks), in logical
/// px.
///
/// **Community-approximate**: this design system publishes no motion spec —
/// ~14px is where the framework's own slide patterns sit (16dp Glyph, 30dp
/// Material shared-axis) and reads as a lift rather than a slide at chip/panel
/// scale.
const LIFT_DISTANCE: f64 = 14.0;

/// The Sample design system's transition pattern: a lift-and-fade. See the
/// [module docs](self).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SampleReveal;

impl TransitionPattern for SampleReveal {
    fn resolve(&self, p: f64, reverse: bool, _size: Size) -> (PatternLayer, PatternLayer) {
        // A spring driver can overshoot, so the raw progress is clamped before
        // it reaches an opacity — a `push_layer` alpha outside `[0, 1]` is not
        // a valid composite. The offsets deliberately use the *raw* value, so
        // an overshooting driver still reads as a spring.
        let clamped = p.clamp(0.0, 1.0);
        let direction = if reverse { -1.0 } else { 1.0 };

        let incoming = PatternLayer {
            dx: 0.0,
            dy: (1.0 - p) * LIFT_DISTANCE * direction,
            alpha: clamped as f32,
            scale: 1.0,
        };
        let exiting = PatternLayer {
            dx: 0.0,
            dy: -p * LIFT_DISTANCE * direction,
            alpha: (1.0 - clamped) as f32,
            scale: 1.0,
        };
        (incoming, exiting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOX: Size = Size::new(200.0, 100.0);

    #[test]
    fn at_rest_the_incoming_child_is_in_place_and_opaque() {
        let (incoming, exiting) = SampleReveal.resolve(1.0, false, BOX);
        assert_eq!(incoming.dy, 0.0);
        assert_eq!(incoming.alpha, 1.0);
        assert_eq!(exiting.alpha, 0.0);
    }

    #[test]
    fn at_the_start_the_incoming_child_is_lifted_and_transparent() {
        let (incoming, exiting) = SampleReveal.resolve(0.0, false, BOX);
        assert_eq!(incoming.dy, LIFT_DISTANCE);
        assert_eq!(incoming.alpha, 0.0);
        assert_eq!(exiting.dy, 0.0);
        assert_eq!(exiting.alpha, 1.0);
    }

    #[test]
    fn reverse_flips_the_direction_but_not_the_opacities() {
        let (forward, _) = SampleReveal.resolve(0.0, false, BOX);
        let (backward, _) = SampleReveal.resolve(0.0, true, BOX);
        assert_eq!(backward.dy, -forward.dy);
        assert_eq!(backward.alpha, forward.alpha);
    }

    #[test]
    fn a_spring_overshoot_never_produces_an_out_of_range_alpha() {
        // A spring driver hands the pattern raw progress, which may exceed 1.
        let (incoming, exiting) = SampleReveal.resolve(1.12, false, BOX);
        assert_eq!(incoming.alpha, 1.0);
        assert_eq!(exiting.alpha, 0.0);
        // ...but the offset still tracks the overshoot.
        assert!(
            incoming.dy < 0.0,
            "overshoot carries past the resting offset"
        );
    }

    #[test]
    fn the_pattern_stays_object_safe() {
        // The trait is object-safe by design; a design system that boxes its
        // patterns (to store several in one collection) depends on that.
        let boxed: Box<dyn TransitionPattern> = Box::new(SampleReveal);
        let (incoming, _) = boxed.resolve(0.5, false, BOX);
        assert!(incoming.alpha > 0.0);
    }
}
