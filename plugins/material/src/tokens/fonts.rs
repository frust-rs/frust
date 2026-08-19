//! Bundled Material typography fonts: the raw variable-TTF bytes for Roboto
//! Flex and Roboto Mono, embedded via `include_bytes!` so [`install`](crate::install)
//! can register them with no filesystem or network access at runtime.
//!
//! - **Roboto Flex Variable** (upright) — Google Fonts, OFL-1.1. The sans face
//!   the whole catalog's display/heading/body text resolves to
//!   ([`type_scale`](super::type_scale)).
//! - **Roboto Mono Variable** (upright) — Google Fonts, OFL-1.1 (verified from
//!   the shipped font's own `name` table, nameID 13/14). The monospace face
//!   for `kbd`, code, and tabular figures (currently unused by the catalog;
//!   available for components that need mono text).
//!
//! Both are *variable* fonts, so one file per family covers the whole weight
//! range the catalog names instead of one file per weight.
//!
//! Provenance (exact upstream releases/URLs, retrieval date, per-face sizes,
//! SHA-256) is recorded in `plugins/material/FONTS-LICENSE`, alongside the
//! full license text for each family (`fonts/roboto-flex/OFL.txt`,
//! `fonts/roboto-mono/OFL.txt`) — required by the OFL-1.1 license-inclusion
//! term. Neither font declares a Reserved Font Name; this module ships the
//! upstream bytes unmodified under the upstream names.
//!
//! The bytes are compiled in **unconditionally**: depending on this plugin at
//! all is the Material opt-in, so there is no second feature to switch the faces
//! off with (the `frust-glyph` and `frust-shadcn` precedent).
//!
//! This module stays pure-data: [`font_data`] only returns bytes. Registering
//! them into a live `TextContext` (and forcing the relayout
//! `TextContext::register_fonts`'s contract requires) is a shell's job —
//! [`install`](crate::install) hands them to `frust::register_app_fonts`, and
//! each shell drains that queue into its own `TextContext`.

/// Roboto Flex Variable, upright (Google Fonts `v1.2.0`, OFL-1.1). See
/// `fonts/roboto-flex/OFL.txt`.
const ROBOTO_FLEX_VARIABLE: &[u8] = include_bytes!("../../fonts/roboto-flex/RobotoFlex.ttf");
/// Roboto Mono Variable, upright (Google Fonts, OFL-1.1). See
/// `fonts/roboto-mono/OFL.txt`.
const ROBOTO_MONO_VARIABLE: &[u8] = include_bytes!("../../fonts/roboto-mono/RobotoMono.ttf");

/// Index of the Roboto Flex face in [`font_data`]'s array — the face
/// [`native_typefaces`](super::native_typefaces) binds into both native
/// slots. Named rather than spelled `0` inline because a native host's publish
/// guard de-duplicates by byte *identity*: the face this index selects and the
/// one `font_data()` hands a shell must be the same `&'static [u8]`.
///
/// Public because a host that registers the faces itself (rather than through
/// [`install`](crate::install)) needs to name which is which.
pub const ROBOTO_FLEX_VARIABLE_INDEX: usize = 0;
/// Index of the Roboto Mono face in [`font_data`]'s array. See
/// [`ROBOTO_FLEX_VARIABLE_INDEX`].
#[allow(dead_code)]
pub const ROBOTO_MONO_VARIABLE_INDEX: usize = 1;

/// Every bundled Material font face's raw bytes (2 faces: Roboto Flex Variable,
/// then Roboto Mono Variable), in no particular registration order — fontique
/// groups registered faces by the family name each face's own `name` table
/// carries, so a caller can register them in any order, one at a time or all at
/// once.
///
/// The two `*_INDEX` constants above are the one place this order is
/// load-bearing; keep them in step with the array literal below.
pub fn font_data() -> &'static [&'static [u8]] {
    &[ROBOTO_FLEX_VARIABLE, ROBOTO_MONO_VARIABLE]
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;

    #[test]
    fn font_data_registers_and_resolves_both_families_by_name() {
        let mut cx = TextContext::new();
        let mut seen_flex = false;
        let mut seen_mono = false;

        for bytes in font_data() {
            let families = cx
                .register_fonts(bytes.to_vec())
                .expect("bundled material font bytes must register as valid faces");
            for family in families {
                if family.name == super::super::ROBOTO_FLEX_FAMILY {
                    seen_flex = true;
                }
                if family.name == super::super::ROBOTO_MONO_FAMILY {
                    seen_mono = true;
                }
            }
        }

        // The names here are the ones the type scale's font stacks ask for, so a
        // mismatch would mean text silently falling back to the system sans.
        assert!(
            seen_flex,
            "expected `font_data()` to register a \"{}\" family",
            super::super::ROBOTO_FLEX_FAMILY
        );
        assert!(
            seen_mono,
            "expected `font_data()` to register a \"{}\" family",
            super::super::ROBOTO_MONO_FAMILY
        );
    }

    #[test]
    fn font_data_returns_both_variable_faces() {
        assert_eq!(
            font_data().len(),
            2,
            "expected Roboto Flex Variable + Roboto Mono Variable"
        );
        assert_eq!(
            font_data()[ROBOTO_FLEX_VARIABLE_INDEX],
            ROBOTO_FLEX_VARIABLE
        );
        assert_eq!(
            font_data()[ROBOTO_MONO_VARIABLE_INDEX],
            ROBOTO_MONO_VARIABLE
        );
    }

    #[test]
    fn font_data_bytes_are_non_empty_and_valid_fonts() {
        // Sniff for TTF magic bytes: 0x00010000 (big-endian) or 'true' for Apple
        for (i, bytes) in font_data().iter().enumerate() {
            assert!(
                bytes.len() > 4,
                "font face {i} must have at least 4 bytes for magic number"
            );
            let magic = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            assert!(
                magic == 0x00010000 || magic == 0x74727565, // 0x74727565 = 'true'
                "font face {i} must start with TTF magic bytes (0x00010000 or 'true')"
            );
        }
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
