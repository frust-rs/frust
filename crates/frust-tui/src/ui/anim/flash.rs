//! The one-shot success flash a session's tab (and its `⟳ watch` status
//! segment) shows when a hot patch completes: the background tints toward
//! the theme's success colour and fades linearly back to idle.
//!
//! Adapted from fdemon's reload flash (`Session::reload_flash_alpha` and the
//! header's `lerp_color(CARD_BG, STATUS_GREEN, alpha * 0.35)`), re-based on
//! frust-tui's clock-free frame counter: the stamp is the
//! `AppState::animation_frame` the engine recorded at the `Patched` outcome
//! (`SessionView::hot_patch_flash`), and "now" is the frame being rendered.
//! The duration is the engine's [`SessionView::HOT_PATCH_FLASH_FRAMES`] — the
//! engine decides when the tick may stop, so it owns the number. Colours come
//! from the caller's `Theme`; a non-RGB palette degrades to "no tint" through
//! [`lerp_color`]'s base fallback.

use ratatui::style::Color;

use super::shimmer::lerp_color;
use crate::engine::SessionView;

/// Frames a flash lasts: 10 × the runner's 50 ms tick = 500 ms.
pub const FLASH_FRAMES: u64 = SessionView::HOT_PATCH_FLASH_FRAMES;

/// The strongest blend toward the success colour, at the flash's first frame
/// — a tint, never a full repaint.
pub const FLASH_BLEND_CAP: f32 = 0.35;

/// The flash's strength at `frame` for a flash stamped at `stamp`: `1.0` on
/// the stamp frame, falling linearly to `0.0` at [`FLASH_FRAMES`] elapsed and
/// staying there. `0.0` with no stamp. Elapsed is `frame.wrapping_sub(stamp)`,
/// so a frame counter that wrapped mid-flash still fades correctly.
pub fn flash_alpha(stamp: Option<u64>, frame: u64) -> f32 {
    let Some(stamp) = stamp else {
        return 0.0;
    };
    let elapsed = frame.wrapping_sub(stamp);
    if elapsed >= FLASH_FRAMES {
        return 0.0;
    }
    (1.0 - elapsed as f32 / FLASH_FRAMES as f32).clamp(0.0, 1.0)
}

/// `base` tinted toward `success` by `alpha` (capped at [`FLASH_BLEND_CAP`]).
/// Returns `base` unchanged at `alpha == 0.0` or when either colour is not
/// RGB.
pub fn flash_bg(base: Color, success: Color, alpha: f32) -> Color {
    lerp_color(base, success, alpha * FLASH_BLEND_CAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_is_full_on_the_stamp_frame_and_half_way_through() {
        assert_eq!(flash_alpha(Some(20), 20), 1.0);
        assert_eq!(flash_alpha(Some(20), 25), 0.5);
    }

    #[test]
    fn alpha_is_zero_at_and_beyond_the_flash_length() {
        assert_eq!(flash_alpha(Some(20), 20 + FLASH_FRAMES), 0.0);
        assert_eq!(flash_alpha(Some(20), 20 + FLASH_FRAMES + 1), 0.0);
        assert_eq!(flash_alpha(Some(20), 10_000), 0.0);
    }

    #[test]
    fn alpha_is_zero_without_a_stamp() {
        assert_eq!(flash_alpha(None, 0), 0.0);
        assert_eq!(flash_alpha(None, 7), 0.0);
    }

    #[test]
    fn alpha_survives_the_frame_counter_wrapping() {
        let stamp = u64::MAX - 2;
        assert_eq!(flash_alpha(Some(stamp), stamp), 1.0);
        // Five frames later the counter has wrapped past zero.
        assert_eq!(flash_alpha(Some(stamp), 2), 0.5);
        assert_eq!(flash_alpha(Some(stamp), FLASH_FRAMES), 0.0);
    }

    #[test]
    fn bg_equals_base_at_alpha_zero_and_differs_at_full_alpha() {
        let base = Color::Rgb(30, 30, 46);
        let success = Color::Rgb(16, 185, 129);
        assert_eq!(flash_bg(base, success, 0.0), base);
        let tinted = flash_bg(base, success, 1.0);
        assert_ne!(tinted, base);
        assert_eq!(tinted, lerp_color(base, success, FLASH_BLEND_CAP));
    }

    #[test]
    fn non_rgb_colours_return_the_base_unchanged() {
        assert_eq!(
            flash_bg(Color::Indexed(236), Color::Indexed(42), 1.0),
            Color::Indexed(236)
        );
        assert_eq!(
            flash_bg(Color::DarkGray, Color::Green, 1.0),
            Color::DarkGray
        );
        assert_eq!(
            flash_bg(Color::Rgb(30, 30, 46), Color::Green, 1.0),
            Color::Rgb(30, 30, 46)
        );
    }
}
