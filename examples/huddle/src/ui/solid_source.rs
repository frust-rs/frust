//! `solid_source`/`solid_source_alpha` — the facade-only way to paint an
//! arbitrary (possibly translucent) filled rectangle: stretch a 1×1
//! [`ImageSource`] to fill with [`ImageFit::Fill`](frust::ImageFit).
//! Promoted here from `features::settings` (huddle clean-architecture
//! refactor, task 05) once the You tab's cross-feature use (the current
//! user's avatar block) made it clear this is a feature-generic fill helper,
//! not settings-specific — mirrors [`crate::ui::fill_box`]'s promotion
//! precedent (device-parity-round2 task R2). Used by the appearance/about
//! settings pages, the You tab's avatar block, and the theme-swap fade veil,
//! none of which map onto a themed widget's own surface fill.

use frust::{Color, ImageSource};

/// A 1×1 solid-color image source.
pub fn solid_source(color: Color) -> ImageSource {
    let c = color.components;
    let to_u8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    ImageSource::from_rgba8(
        vec![to_u8(c[0]), to_u8(c[1]), to_u8(c[2]), to_u8(c[3])],
        1,
        1,
    )
}

/// [`solid_source`] with an explicit `alpha` (`0.0..=1.0`) replacing the
/// color's own — the veil's translucent wash.
pub fn solid_source_alpha(color: Color, alpha: f64) -> ImageSource {
    let c = color.components;
    let to_u8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    ImageSource::from_rgba8(
        vec![
            to_u8(c[0]),
            to_u8(c[1]),
            to_u8(c[2]),
            to_u8(alpha.clamp(0.0, 1.0) as f32),
        ],
        1,
        1,
    )
}
