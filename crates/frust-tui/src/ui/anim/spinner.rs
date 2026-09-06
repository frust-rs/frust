//! Pure frame-selection math for a braille throbber. A caller supplies a
//! monotonically advancing frame index (`AppState::animation_frame`, or that
//! counter pre-divided by [`SPINNER_TICKS_PER_FRAME`] for a calmer cadence)
//! and gets the glyph to draw this frame back. No `AppState`, rendering, or
//! I/O dependency, so this is trivially testable and reusable by any call
//! site that wants a spinner (a tab glyph, a modal's busy indicator, …).

/// Braille throbber frames, in sweep order.
pub const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Ticks per spinner frame for a caller driving the spinner directly off
/// `AppState::animation_frame` (which advances every ~50 ms / 20 fps tick —
/// see `crate::runner::TICK`). At 2 ticks/frame the throbber advances
/// ~every 100 ms (~10 fps): lively but calm. Not every call site need use
/// this divisor — a busier or calmer cadence just picks a different one (or
/// none) before calling [`spinner_char`], which stays agnostic.
pub const SPINNER_TICKS_PER_FRAME: u64 = 2;

/// The throbber glyph for the given frame index, via direct modulo over
/// [`SPINNER_FRAMES`]. `frame` is the already-cadence-adjusted index: the
/// caller decides how fast to advance (pass the raw frame for
/// one-glyph-per-tick, or `frame / SPINNER_TICKS_PER_FRAME` for the calmer
/// cadence above). Wraps cleanly via `%`, so `u64` wrap in the source frame
/// is harmless.
pub fn spinner_char(frame: u64) -> char {
    SPINNER_FRAMES[(frame % SPINNER_FRAMES.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_char_advances_deterministically() {
        let len = SPINNER_FRAMES.len() as u64;
        for i in 0u64..25 {
            assert_eq!(
                spinner_char(i),
                SPINNER_FRAMES[(i % len) as usize],
                "spinner_char({i}) should equal SPINNER_FRAMES[{} % {len}]",
                i
            );
        }
    }

    #[test]
    fn spinner_char_wraps_over_frame_set() {
        let len = SPINNER_FRAMES.len() as u64;
        assert_eq!(
            spinner_char(len),
            spinner_char(0),
            "spinner_char(len) should equal spinner_char(0)"
        );
    }

    #[test]
    fn spinner_char_no_panic_near_u64_max() {
        let _ = spinner_char(u64::MAX);
        let _ = spinner_char(u64::MAX - 1);
    }

    #[test]
    fn spinner_frames_are_ten_braille_glyphs() {
        assert_eq!(
            SPINNER_FRAMES.len(),
            10,
            "SPINNER_FRAMES must have exactly 10 glyphs"
        );
        let expected = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        assert_eq!(SPINNER_FRAMES, expected);
    }
}
