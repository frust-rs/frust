//! Bundled Glyph fonts (feature `glyph-fonts`, default-on): the raw TTF bytes
//! for Space Mono and IBM Plex Mono, embedded via `include_bytes!` so a shell
//! can register them into a [`frust_text::TextContext`] with no filesystem or
//! network access at runtime.
//!
//! - **Space Mono** (Regular/Bold/Italic) — Google Fonts, OFL-1.1.
//! - **IBM Plex Mono** (Regular/Medium/SemiBold/Italic) — IBM, OFL-1.1.
//!
//! Provenance (exact upstream commits/URLs, retrieval date, per-face sizes)
//! is recorded in `crates/frust-theme/fonts/README.md`, alongside the full
//! OFL-1.1 license text for each family (`fonts/space-mono/OFL.txt`,
//! `fonts/ibm-plex-mono/OFL.txt`) — required by the license's reserved-font-name
//! and license-inclusion terms. Neither font's reserved name ("Space Mono",
//! "Plex") is modified here; this module ships the upstream bytes unmodified.
//!
//! This crate stays pure-data: [`font_data`] only returns bytes. Registering
//! them into a live [`frust_text::TextContext`] (and forcing the
//! `ChangeFlags::LAYOUT | PAINT` relayout `TextContext::register_fonts`'s
//! contract requires) is a shell's job — each shell seeds this accessor's
//! bytes through `frust-shell-common`'s font-registry pending-font queue at
//! construction, applying them via `TextContext::register_fonts`.

/// Space Mono Regular (Google Fonts, OFL-1.1). See `fonts/space-mono/OFL.txt`.
const SPACE_MONO_REGULAR: &[u8] = include_bytes!("../../fonts/space-mono/SpaceMono-Regular.ttf");
/// Space Mono Bold (Google Fonts, OFL-1.1). See `fonts/space-mono/OFL.txt`.
const SPACE_MONO_BOLD: &[u8] = include_bytes!("../../fonts/space-mono/SpaceMono-Bold.ttf");
/// Space Mono Italic (Google Fonts, OFL-1.1). See `fonts/space-mono/OFL.txt`.
const SPACE_MONO_ITALIC: &[u8] = include_bytes!("../../fonts/space-mono/SpaceMono-Italic.ttf");

/// IBM Plex Mono Regular (IBM, OFL-1.1). See `fonts/ibm-plex-mono/OFL.txt`.
const IBM_PLEX_MONO_REGULAR: &[u8] =
    include_bytes!("../../fonts/ibm-plex-mono/IBMPlexMono-Regular.ttf");
/// IBM Plex Mono Medium (IBM, OFL-1.1). See `fonts/ibm-plex-mono/OFL.txt`.
const IBM_PLEX_MONO_MEDIUM: &[u8] =
    include_bytes!("../../fonts/ibm-plex-mono/IBMPlexMono-Medium.ttf");
/// IBM Plex Mono SemiBold (IBM, OFL-1.1). See `fonts/ibm-plex-mono/OFL.txt`.
const IBM_PLEX_MONO_SEMIBOLD: &[u8] =
    include_bytes!("../../fonts/ibm-plex-mono/IBMPlexMono-SemiBold.ttf");
/// IBM Plex Mono Italic (IBM, OFL-1.1). See `fonts/ibm-plex-mono/OFL.txt`.
const IBM_PLEX_MONO_ITALIC: &[u8] =
    include_bytes!("../../fonts/ibm-plex-mono/IBMPlexMono-Italic.ttf");

/// Every bundled Glyph font face's raw bytes (7 faces: 3 Space Mono + 4 IBM
/// Plex Mono), in no particular registration order — fontique groups
/// registered faces by the family name each face's own `name` table carries,
/// so a caller can register them in any order, one at a time or all at once.
pub fn font_data() -> &'static [&'static [u8]] {
    &[
        SPACE_MONO_REGULAR,
        SPACE_MONO_BOLD,
        SPACE_MONO_ITALIC,
        IBM_PLEX_MONO_REGULAR,
        IBM_PLEX_MONO_MEDIUM,
        IBM_PLEX_MONO_SEMIBOLD,
        IBM_PLEX_MONO_ITALIC,
    ]
}

#[cfg(test)]
mod tests {
    use super::font_data;
    use frust_text::TextContext;

    #[test]
    fn font_data_registers_and_resolves_both_families_by_name() {
        let mut cx = TextContext::new();
        let mut seen_space_mono = false;
        let mut seen_ibm_plex_mono = false;

        for bytes in font_data() {
            let families = cx
                .register_fonts(bytes.to_vec())
                .expect("bundled Glyph font bytes must register as valid faces");
            for family in families {
                if family.name == "Space Mono" {
                    seen_space_mono = true;
                }
                if family.name == "IBM Plex Mono" {
                    seen_ibm_plex_mono = true;
                }
            }
        }

        assert!(
            seen_space_mono,
            "expected `font_data()` to register a \"Space Mono\" family"
        );
        assert!(
            seen_ibm_plex_mono,
            "expected `font_data()` to register an \"IBM Plex Mono\" family"
        );
    }

    #[test]
    fn font_data_returns_all_seven_faces() {
        assert_eq!(
            font_data().len(),
            7,
            "expected 3 Space Mono + 4 IBM Plex Mono faces"
        );
    }
}
