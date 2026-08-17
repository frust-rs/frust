//! [`baseline`]: the Glyph design language assembled into a [`Theme`] —
//! paired Glyph light/dark schemes, the Glyph type/shape/elevation/motion/
//! glass scales, and the Glyph theme extensions (the status palette,
//! [`GlyphInk`], and the [`NativeTypefaces`] binding for native controls).

use frust::{
    Brightness, DesignLanguage, FontFace, NativeTypefaces, Theme, ThemeExtensions,
    authoring::text::TextStyle,
};

use super::color::{self, GlyphInk};
use super::fonts::{self, IBM_PLEX_MONO_REGULAR_INDEX, SPACE_MONO_REGULAR_INDEX};
use super::scales;

/// The family name the button/display native slot reports for its face —
/// diagnostics/de-duplication only (see [`FontFace`]), matching the name Space
/// Mono's own `name` table carries.
const SPACE_MONO_FAMILY: &str = "Space Mono";
/// The family name the body native slot reports for its face. See
/// [`SPACE_MONO_FAMILY`].
const IBM_PLEX_MONO_FAMILY: &str = "IBM Plex Mono";

/// The Glyph baseline theme: the Glyph light/dark [`frust::ColorScheme`]s, the
/// Glyph type/shape/elevation/motion/glass scales, tagged
/// [`DesignLanguage::Glyph`], with every Glyph extension attached
/// ([`color::status`], [`GlyphInk::default_ink`], and [`native_typefaces`]) so
/// `extension::<StatusPalette>()`/`extension::<GlyphInk>()`/
/// `extension::<NativeTypefaces>()` are always `Some` on a Glyph theme.
///
/// **Starts in [`Brightness::Dark`]** — a deliberate divergence from
/// `Theme::neutral` and the other design systems' baselines (all
/// [`Brightness::Light`]): Glyph is a **dark-first** system (its dark HTML
/// build is the canonical brightness). Call
/// `.with_brightness(Brightness::Light)` to select the light scheme (e.g.
/// to honor a live OS light-mode preference before handing the theme to
/// `set_app_theme`).
///
/// Not `const` (like `Theme::neutral`): [`ThemeExtensions`]' `HashMap` and
/// the type scale's per-slot `FontFamily` stacks aren't const-evaluable.
pub fn baseline() -> Theme {
    let mut extensions = ThemeExtensions::new();
    extensions.insert(color::status());
    extensions.insert(GlyphInk::default_ink());
    extensions.insert(native_typefaces());
    Theme {
        light: color::light(),
        dark: color::dark(),
        type_scale: scales::type_scale(&TextStyle::default()),
        shape: scales::shape(),
        elevation: scales::elevation(),
        motion: scales::motion(),
        glass: scales::glass(),
        brightness: Brightness::Dark,
        design_language: DesignLanguage::Glyph,
        extensions,
    }
}

/// The native-control typeface binding [`baseline`] attaches: Space Mono
/// Regular in the button/display slot, IBM Plex Mono Regular in the body slot
/// — the same pairing the native-widgets theme ladder resolves for a Glyph
/// theme today (its button slot is `Typeface::GlyphMono`, its body slot
/// `Typeface::GlyphPlex`), lifted here so the binding travels with the theme
/// instead of being inferred from `design_language`.
///
/// Both faces are taken **out of [`fonts::font_data`]'s own array** rather than
/// re-referenced from the underlying constants: a native host de-duplicates
/// published payloads by byte identity (address + length), so a slot's face and
/// the bytes a shell registers through
/// [`install`](crate::install) must be the *same* `&'static [u8]`, not merely
/// equal ones.
pub fn native_typefaces() -> NativeTypefaces {
    let faces = fonts::font_data();
    NativeTypefaces {
        button: faces
            .get(SPACE_MONO_REGULAR_INDEX)
            .copied()
            .map(|bytes| FontFace::new(SPACE_MONO_FAMILY, bytes)),
        body: faces
            .get(IBM_PLEX_MONO_REGULAR_INDEX)
            .copied()
            .map(|bytes| FontFace::new(IBM_PLEX_MONO_FAMILY, bytes)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::StatusPalette;

    #[test]
    fn glyph_baseline_is_internally_consistent() {
        let t = baseline();
        assert_eq!(t.light, color::light());
        assert_eq!(t.dark, color::dark());
        assert_eq!(t.shape, scales::shape());
        assert_eq!(t.elevation, scales::elevation());
        assert_eq!(t.motion, scales::motion());
        assert_eq!(t.glass, scales::glass());
        assert_eq!(t.design_language, DesignLanguage::Glyph);
    }

    #[test]
    fn glyph_baseline_is_dark_first() {
        // Deliberate divergence: Glyph starts dark (see the doc comment).
        let t = baseline();
        assert_eq!(t.brightness, Brightness::Dark);
        assert_eq!(t.scheme(), &t.dark);
    }

    #[test]
    fn with_brightness_light_selects_the_light_scheme() {
        let t = baseline().with_brightness(Brightness::Light);
        assert_eq!(t.brightness, Brightness::Light);
        assert_eq!(t.scheme(), &t.light);
        // Design language is preserved across the brightness flip.
        assert_eq!(t.design_language, DesignLanguage::Glyph);
    }

    #[test]
    fn glyph_baseline_carries_both_extensions() {
        let t = baseline();
        assert_eq!(t.extension::<StatusPalette>(), Some(&color::status()));
        assert_eq!(t.extension::<GlyphInk>(), Some(&GlyphInk::default_ink()));
    }

    #[test]
    fn glyph_baseline_survives_clone() {
        let t = baseline();
        let c = t.clone();
        assert_eq!(c.extension::<GlyphInk>(), Some(&GlyphInk::default_ink()));
        assert_eq!(c.extension::<StatusPalette>(), Some(&color::status()));
        assert_eq!(
            c.extension::<NativeTypefaces>(),
            Some(&native_typefaces()),
            "the native-typeface binding survives a theme clone too"
        );
    }

    #[test]
    fn glyph_baseline_attaches_the_bundled_faces_by_byte_identity() {
        // The native publish guard de-duplicates by pointer/length, so the
        // attached faces must BE `font_data()`'s entries, not copies of them.
        let t = baseline();
        let faces = t
            .extension::<NativeTypefaces>()
            .expect("the Glyph baseline attaches NativeTypefaces");
        let bundled = fonts::font_data();

        let button = faces.button.expect("button slot is bound");
        let body = faces.body.expect("body slot is bound");
        assert_eq!(button.family, SPACE_MONO_FAMILY);
        assert_eq!(body.family, IBM_PLEX_MONO_FAMILY);
        assert!(
            std::ptr::eq(button.bytes, bundled[SPACE_MONO_REGULAR_INDEX]),
            "button face must be the bundled Space Mono Regular bytes themselves"
        );
        assert!(
            std::ptr::eq(body.bytes, bundled[IBM_PLEX_MONO_REGULAR_INDEX]),
            "body face must be the bundled IBM Plex Mono Regular bytes themselves"
        );
    }
}
