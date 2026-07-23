//! [`Theme::glyph_baseline`]: the Glyph design language assembled into a
//! [`Theme`] — paired Glyph light/dark schemes, the Glyph type/shape/elevation/
//! motion/glass scales, and both Glyph extensions
//! ([`StatusPalette::glyph`] + [`GlyphInk`]).

use frust_text::TextStyle;

use crate::color::{Brightness, ColorScheme};
use crate::elevation::Elevation;
use crate::extensions::ThemeExtensions;
use crate::glass::GlassScale;
use crate::glyph::GlyphInk;
use crate::motion::MotionScheme;
use crate::shape::ShapeScale;
use crate::status::StatusPalette;
use crate::theme::{DesignLanguage, Theme};
use crate::typography::TypeScale;

impl Theme {
    /// The Glyph baseline theme (PLAN.md Phase 3; RESEARCH §1): the Glyph
    /// light/dark [`ColorScheme`]s, the Glyph [`TypeScale`]/[`ShapeScale`]/
    /// [`Elevation`]/[`MotionScheme`]/[`GlassScale`], tagged
    /// [`DesignLanguage::Glyph`], with **both** Glyph extensions attached
    /// ([`StatusPalette::glyph`] and [`GlyphInk::default_ink`]) so
    /// `extension::<StatusPalette>()`/`extension::<GlyphInk>()` are always
    /// `Some` on a Glyph theme.
    ///
    /// **Starts in [`Brightness::Dark`]** — a deliberate divergence from
    /// [`Theme::m3_baseline`]/[`Theme::cupertino_baseline`] (both
    /// [`Brightness::Light`]): Glyph is a **dark-first** system (its dark HTML
    /// build is the canonical brightness, RESEARCH §1). Call
    /// `.with_brightness(Brightness::Light)` to select the light scheme (e.g.
    /// to honor a live OS light-mode preference before handing the theme to
    /// `set_app_theme`).
    ///
    /// Not `const` (like the other baselines): [`ThemeExtensions`]' `HashMap`
    /// and the [`TypeScale`]'s per-slot `FontFamily` stacks aren't
    /// const-evaluable.
    pub fn glyph_baseline() -> Self {
        let mut extensions = ThemeExtensions::new();
        extensions.insert(StatusPalette::glyph());
        extensions.insert(GlyphInk::default_ink());
        Self {
            light: ColorScheme::glyph_light(),
            dark: ColorScheme::glyph_dark(),
            type_scale: TypeScale::glyph(&TextStyle::default()),
            shape: ShapeScale::glyph(),
            elevation: Elevation::glyph(),
            motion: MotionScheme::glyph(),
            glass: GlassScale::glyph(),
            brightness: Brightness::Dark,
            design_language: DesignLanguage::Glyph,
            extensions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_baseline_is_internally_consistent() {
        let t = Theme::glyph_baseline();
        assert_eq!(t.light, ColorScheme::glyph_light());
        assert_eq!(t.dark, ColorScheme::glyph_dark());
        assert_eq!(t.shape, ShapeScale::glyph());
        assert_eq!(t.elevation, Elevation::glyph());
        assert_eq!(t.motion, MotionScheme::glyph());
        assert_eq!(t.glass, GlassScale::glyph());
        assert_eq!(t.design_language, DesignLanguage::Glyph);
    }

    #[test]
    fn glyph_baseline_is_dark_first() {
        // Deliberate divergence: Glyph starts dark (see the doc comment).
        let t = Theme::glyph_baseline();
        assert_eq!(t.brightness, Brightness::Dark);
        assert_eq!(t.scheme(), &t.dark);
    }

    #[test]
    fn with_brightness_light_selects_the_light_scheme() {
        // Acceptance criterion 3.
        let t = Theme::glyph_baseline().with_brightness(Brightness::Light);
        assert_eq!(t.brightness, Brightness::Light);
        assert_eq!(t.scheme(), &t.light);
        // Design language is preserved across the brightness flip.
        assert_eq!(t.design_language, DesignLanguage::Glyph);
    }

    #[test]
    fn glyph_baseline_carries_both_extensions() {
        // Acceptance criterion 3: both extensions resolve.
        let t = Theme::glyph_baseline();
        assert_eq!(
            t.extension::<StatusPalette>(),
            Some(&StatusPalette::glyph())
        );
        assert_eq!(t.extension::<GlyphInk>(), Some(&GlyphInk::default_ink()));
    }

    #[test]
    fn glyph_baseline_survives_clone() {
        let t = Theme::glyph_baseline();
        let c = t.clone();
        assert_eq!(c.extension::<GlyphInk>(), Some(&GlyphInk::default_ink()));
        assert_eq!(
            c.extension::<StatusPalette>(),
            Some(&StatusPalette::glyph())
        );
    }
}
