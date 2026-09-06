//! Bundled Glyph fonts: the raw TTF bytes for Space Mono and IBM Plex Mono,
//! embedded via `include_bytes!` so [`install`](crate::install) can register
//! them with no filesystem or network access at runtime.
//!
//! - **Space Mono** (Regular/Bold/Italic) — Google Fonts, OFL-1.1.
//! - **IBM Plex Mono** (Regular/Medium/SemiBold/Italic) — IBM, OFL-1.1.
//!
//! Provenance (exact upstream commits/URLs, retrieval date, per-face sizes)
//! is recorded in `plugins/glyph/fonts/README.md`, alongside the full
//! OFL-1.1 license text for each family (`fonts/space-mono/OFL.txt`,
//! `fonts/ibm-plex-mono/OFL.txt`) — required by the license's reserved-font-name
//! and license-inclusion terms. Neither font's reserved name ("Space Mono",
//! "Plex") is modified here; this module ships the upstream bytes unmodified.
//!
//! The bytes are compiled in **unconditionally**: depending on this plugin at
//! all is the Glyph opt-in, so there is no second feature to switch the faces
//! off with (the in-tree catalog's `glyph-fonts` gate has no counterpart here).
//!
//! This module stays pure-data: [`font_data`] only returns bytes. Registering
//! them into a live `TextContext` (and forcing the relayout
//! `TextContext::register_fonts`'s contract requires) is a shell's job —
//! [`install`](crate::install) hands them to `frust::register_app_fonts`, and
//! each shell drains that queue into its own `TextContext`.

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

/// Index of the Space Mono **Regular** face in [`font_data`]'s array — the
/// button/display face [`native_typefaces`](super::baseline::native_typefaces)
/// binds. Named rather than spelled `0` inline because a native host's publish
/// guard de-duplicates by byte *identity*: the face this index selects and the
/// one `font_data()` hands a shell must be the same `&'static [u8]`.
pub(crate) const SPACE_MONO_REGULAR_INDEX: usize = 0;
/// Index of the IBM Plex Mono **Regular** face in [`font_data`]'s array — the
/// body face. See [`SPACE_MONO_REGULAR_INDEX`].
pub(crate) const IBM_PLEX_MONO_REGULAR_INDEX: usize = 3;

/// Every bundled Glyph font face's raw bytes (7 faces: 3 Space Mono + 4 IBM
/// Plex Mono), in no particular registration order — fontique groups
/// registered faces by the family name each face's own `name` table carries,
/// so a caller can register them in any order, one at a time or all at once.
///
/// The two `*_INDEX` constants above are the one place this order is load-
/// bearing; keep them in step with the array literal below.
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
    use frust::authoring::text::TextContext;

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

    #[test]
    fn named_face_indices_select_each_family_s_regular() {
        // The two indices the native-typeface binding rides on: Space Mono's
        // Regular first, IBM Plex Mono's Regular right after the three Space
        // Mono faces. Compared by content — a `const` of reference type is
        // inlined per use site, so the *pointer* a bare `SPACE_MONO_REGULAR`
        // read yields need not be the one inside the array below (verified: it
        // isn't). That is exactly why the native binding reads its faces out of
        // `font_data()` rather than off the constants.
        let faces = font_data();
        assert_eq!(
            faces[super::SPACE_MONO_REGULAR_INDEX],
            super::SPACE_MONO_REGULAR
        );
        assert_eq!(
            faces[super::IBM_PLEX_MONO_REGULAR_INDEX],
            super::IBM_PLEX_MONO_REGULAR
        );
    }

    #[test]
    fn font_data_hands_out_the_same_bytes_on_every_call() {
        // The identity guarantee the native-control publish guard depends on
        // (it de-duplicates published payloads by address + length).
        let (a, b) = (font_data(), font_data());
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert!(
                std::ptr::eq(*x, *y),
                "face {i} must be the same `&'static [u8]` on every call"
            );
        }
    }
}
