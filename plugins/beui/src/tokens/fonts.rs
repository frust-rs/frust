//! Bundled beUI fonts: the raw variable-TTF bytes for Geist and Geist Mono,
//! embedded via `include_bytes!` so [`install`](crate::install) can register
//! them with no filesystem or network access at runtime.
//!
//! - **Geist Variable** (upright) — vercel/geist-font, OFL-1.1. The sans face
//!   the whole catalog's text resolves to
//!   ([`type_scale`](super::theme::type_scale)); upstream binds it to
//!   `--font-sans` *and* `--font-display`, so it is the display face too.
//! - **Geist Mono Variable** (upright) — vercel/geist-font, OFL-1.1. The
//!   monospace face for code, key caps and tabular figures
//!   ([`mono_family`](super::theme::mono_family)); upstream's `--font-mono`.
//!
//! Both are *variable* fonts, so one file per family covers the whole weight
//! range the catalog names instead of one file per weight.
//!
//! The full OFL-1.1 license text ships beside each family
//! (`plugins/beui/fonts/geist/OFL.txt`, `fonts/geist-mono/OFL.txt`) — required
//! by the license's license-inclusion term. Both families are covered by the
//! one upstream license file, which the project publishes for the repository as
//! a whole; it is vendored once per family directory here so neither family's
//! bytes can ever travel without it. Its copyright line ("Copyright 2024 The
//! Geist Project Authors") carries **no Reserved Font Name** clause, so the
//! unmodified faces ship under their original names. Neither file's bytes are
//! modified — only the filenames are normalized to the `<Family>-Variable.ttf`
//! shape the other bundled catalogs use.
//!
//! The bytes are compiled in **unconditionally**: depending on this plugin at
//! all is the beUI opt-in, so there is no second feature to switch the faces
//! off with (the `frust-glyph` precedent, which `frust-shadcn` follows too).
//!
//! This module stays pure-data: [`font_data`] only returns bytes. Registering
//! them into a live `TextContext` (and forcing the relayout
//! `TextContext::register_fonts`'s contract requires) is a shell's job —
//! [`install`](crate::install) hands them to `frust::register_app_fonts`, and
//! each shell drains that queue into its own `TextContext`.

/// Geist Variable, upright (vercel/geist-font, OFL-1.1). See
/// `fonts/geist/OFL.txt`.
const GEIST_VARIABLE: &[u8] = include_bytes!("../../fonts/geist/Geist-Variable.ttf");
/// Geist Mono Variable, upright (vercel/geist-font, OFL-1.1). See
/// `fonts/geist-mono/OFL.txt`.
const GEIST_MONO_VARIABLE: &[u8] = include_bytes!("../../fonts/geist-mono/GeistMono-Variable.ttf");

/// Index of the Geist face in [`font_data`]'s array — the face
/// [`native_typefaces`](super::theme::native_typefaces) binds into both native
/// slots. Named rather than spelled `0` inline because a native host's publish
/// guard de-duplicates by byte *identity*: the face this index selects and the
/// one `font_data()` hands a shell must be the same `&'static [u8]`.
///
/// Public because a host that registers the faces itself (rather than through
/// [`install`](crate::install)) needs to name which is which.
pub const GEIST_VARIABLE_INDEX: usize = 0;
/// Index of the Geist Mono face in [`font_data`]'s array. See
/// [`GEIST_VARIABLE_INDEX`].
pub const GEIST_MONO_VARIABLE_INDEX: usize = 1;

/// Every bundled beUI font face's raw bytes (2 faces: Geist Variable, then
/// Geist Mono Variable), in no particular registration order — fontique groups
/// registered faces by the family name each face's own `name` table carries, so
/// a caller can register them in any order, one at a time or all at once.
///
/// The two `*_INDEX` constants above are the one place this order is
/// load-bearing; keep them in step with the array literal below.
pub fn font_data() -> &'static [&'static [u8]] {
    &[GEIST_VARIABLE, GEIST_MONO_VARIABLE]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::theme::{GEIST_FAMILY, GEIST_MONO_FAMILY};
    use frust::authoring::text::TextContext;

    #[test]
    fn font_data_registers_and_resolves_both_families_by_name() {
        let mut cx = TextContext::new();
        let mut seen_sans = false;
        let mut seen_mono = false;

        for bytes in font_data() {
            let families = cx
                .register_fonts(bytes.to_vec())
                .expect("bundled beUI font bytes must register as valid faces");
            for family in families {
                if family.name == GEIST_FAMILY {
                    seen_sans = true;
                }
                if family.name == GEIST_MONO_FAMILY {
                    seen_mono = true;
                }
            }
        }

        // The names here are the ones the type scale's font stacks ask for, so
        // a mismatch would mean text silently falling back to the system sans.
        assert!(
            seen_sans,
            "expected `font_data()` to register a \"{GEIST_FAMILY}\" family"
        );
        assert!(
            seen_mono,
            "expected `font_data()` to register a \"{GEIST_MONO_FAMILY}\" family"
        );
    }

    #[test]
    fn font_data_returns_both_variable_faces() {
        assert_eq!(
            font_data().len(),
            2,
            "expected Geist Variable + Geist Mono Variable"
        );
        assert_eq!(font_data()[GEIST_VARIABLE_INDEX], GEIST_VARIABLE);
        assert_eq!(font_data()[GEIST_MONO_VARIABLE_INDEX], GEIST_MONO_VARIABLE);
        // Two genuinely different faces, not the same file bundled twice.
        assert_ne!(GEIST_VARIABLE, GEIST_MONO_VARIABLE);
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
