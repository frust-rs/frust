//! The Android matcher's synthesis decision, as a pure function.
//!
//! `android.rs` asks `AFontMatcher` for a face and then decides what the face needs to look like
//! the request (variation coordinates, faux bold, faux oblique). The decision reads only plain
//! numbers and flags, so it lives here, compiled on every target: its tests run on every host and
//! in CI, not only on a device. `android.rs` fills [`Requested`] and [`Actual`] from the match and
//! attaches the result as the answer's `synthesis_hint`.

use super::{Synthesis, VariationSetting};

/// Faux-oblique angle (degrees) for an upright face asked for italic.
pub(crate) const FAUX_OBLIQUE_DEGREES: f32 = 14.0;

/// What the caller asked of the matcher.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Requested {
    pub(crate) weight: f32,
    pub(crate) italic: bool,
}

/// What the matcher answered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Actual {
    pub(crate) weight: f32,
    pub(crate) has_wght_axis: bool,
    pub(crate) has_ital_axis: bool,
    pub(crate) file_is_italic: bool,
}

/// The pure synthesis decision: independent of slant for weight, so upright and italic asks at
/// the same weight embolden identically.
///
/// - a face with a `wght` axis takes the requested weight on the axis;
/// - otherwise a request of 600 or more on a face at least 100 lighter is faux bold (no bold
///   file for this script, as for CJK, or this family, as for mono);
/// - an italic request takes `ital=1` on a face with that axis, else a faux oblique unless the
///   matched file is itself an italic design.
pub(crate) fn decide_synthesis(req: &Requested, actual: &Actual) -> Synthesis {
    let mut synth = Synthesis::default();
    if actual.has_wght_axis {
        synth.variations.push(VariationSetting {
            tag: *b"wght",
            value: req.weight,
        });
    } else if req.weight >= 600.0 && actual.weight + 100.0 <= req.weight {
        synth.embolden = true;
    }
    if req.italic {
        if actual.has_ital_axis {
            synth.variations.push(VariationSetting {
                tag: *b"ital",
                value: 1.0,
            });
        } else if !actual.file_is_italic {
            synth.skew = Some(FAUX_OBLIQUE_DEGREES);
        }
    }
    synth
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actual(weight: f32, wght: bool) -> Actual {
        Actual {
            weight,
            has_wght_axis: wght,
            has_ital_axis: false,
            file_is_italic: false,
        }
    }

    fn decide(weight: f32, italic: bool, a: &Actual) -> Synthesis {
        decide_synthesis(&Requested { weight, italic }, a)
    }

    #[test]
    fn upright_and_italic_embolden_alike() {
        for (w, face_w) in [(500.0, 400.0), (700.0, 400.0), (600.0, 500.0)] {
            let a = actual(face_w, false);
            assert_eq!(
                decide(w, false, &a).embolden,
                decide(w, true, &a).embolden,
                "weight {w} on {face_w}"
            );
        }
    }

    #[test]
    fn variable_face_takes_axis() {
        let s = decide(700.0, false, &actual(400.0, true));
        assert!(!s.embolden);
        assert_eq!(s.variation(*b"wght"), Some(700.0));
    }

    #[test]
    fn static_lighter_face_emboldens_from_600() {
        assert!(decide(700.0, false, &actual(400.0, false)).embolden);
        assert!(decide(600.0, false, &actual(400.0, false)).embolden);
    }

    #[test]
    fn five_hundred_on_regular_is_nothing_needed() {
        assert_eq!(
            decide(500.0, false, &actual(400.0, false)),
            Synthesis::default()
        );
    }
}
