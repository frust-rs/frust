//! Bundled shadcn fonts: the raw variable-TTF bytes for Inter and JetBrains
//! Mono, embedded via `include_bytes!` so [`install`](crate::install) can
//! register them with no filesystem or network access at runtime.
//!
//! - **Inter Variable** (upright) — rsms/inter, OFL-1.1. The sans face the whole
//!   catalog's text resolves to ([`type_scale`](super::theme::type_scale)).
//! - **JetBrains Mono Variable** (upright) — JetBrains, OFL-1.1. The monospace
//!   face for `kbd`, code, and tabular figures
//!   ([`mono_family`](super::theme::mono_family)).
//!
//! Both are *variable* fonts, so one file per family covers the whole weight
//! range the catalog names instead of one file per weight.
//!
//! Provenance (exact upstream releases/URLs/SHA-256s, retrieval date, per-face
//! sizes) is recorded in `plugins/shadcn/fonts/README.md`, alongside the full
//! OFL-1.1 license text for each family (`fonts/inter/OFL.txt`,
//! `fonts/jetbrains-mono/OFL.txt`) — required by the license's
//! license-inclusion term. Neither family declares a Reserved Font Name.
//!
//! JetBrains Mono ships the upstream bytes unmodified under its upstream
//! name. **Inter does not**: it is a `wght`-only static-axis instance of the
//! upstream `opsz`+`wght` variable font, produced with
//! `scripts/fonts/instance_variable_font.py` (fontTools `varLib.instancer`,
//! fontTools 4.63.0). `crates/frust-text` only ever drives
//! `StyleProperty::FontWeight`/`FontStyle` on a shaped run — no component
//! anywhere requests `opsz` — so Inter's optical-size axis always rendered at
//! its own default position (`opsz` = 14, its `fvar`-declared minimum and
//! default) anyway. Pinning `opsz` there and keeping only `wght` variable
//! drops its `gvar` deltas with no visual change and no glyph dropped (all
//! 2,937 glyphs kept; no subsetting). The vendored OFL-1.1 license text
//! (`fonts/inter/OFL.txt`) ships unchanged, and this instance declares no
//! Reserved Font Name, the same as upstream.
//!
//! ### Modification record — Inter Variable
//!
//! - **Upstream SHA-256** (pre-instancing, `v4.1` release asset, 879,708 B):
//!   `4989b125924991b90d05b2d16e0e388c48f7d5bb8b30539bbf9c755278d0ccaf`
//! - **Instanced SHA-256** (the bytes actually vendored, 636,684 B):
//!   `67ca63c5f8c1f2d04fe111e39501b4ae0783ee78a92246f2b210f32d9448dd2e`
//! - **Command**: `python3 scripts/fonts/instance_variable_font.py
//!   plugins/shadcn/fonts/inter/InterVariable.ttf
//!   plugins/shadcn/fonts/inter/InterVariable.ttf --keep wght`
//! - **Date**: 2026-09-17
//! - **Remaining axis**: `wght` (100–900), default 400 — unchanged from
//!   upstream
//! - **Regenerate when**: the text stack starts driving `opsz` (or any other
//!   axis) — re-download the upstream `v4.1` variable font and re-run the
//!   command above with an additional `--keep <axis>` per axis that must stay
//!   variable.
//!
//! The bytes are compiled in **unconditionally**: depending on this plugin at
//! all is the shadcn opt-in, so there is no second feature to switch the faces
//! off with (the `frust-glyph` precedent).
//!
//! This module stays pure-data: [`font_data`] only returns bytes. Registering
//! them into a live `TextContext` (and forcing the relayout
//! `TextContext::register_fonts`'s contract requires) is a shell's job —
//! [`install`](crate::install) hands them to `frust::register_app_fonts`, and
//! each shell drains that queue into its own `TextContext`.

/// Inter Variable, upright (rsms/inter `v4.1`, OFL-1.1). See
/// `fonts/inter/OFL.txt`.
const INTER_VARIABLE: &[u8] = include_bytes!("../../fonts/inter/InterVariable.ttf");
/// JetBrains Mono Variable, upright (JetBrains `v2.304`, OFL-1.1). See
/// `fonts/jetbrains-mono/OFL.txt`.
const JETBRAINS_MONO_VARIABLE: &[u8] =
    include_bytes!("../../fonts/jetbrains-mono/JetBrainsMono-Variable.ttf");

/// Index of the Inter face in [`font_data`]'s array — the face
/// [`native_typefaces`](super::theme::native_typefaces) binds into both native
/// slots. Named rather than spelled `0` inline because a native host's publish
/// guard de-duplicates by byte *identity*: the face this index selects and the
/// one `font_data()` hands a shell must be the same `&'static [u8]`.
///
/// Public because a host that registers the faces itself (rather than through
/// [`install`](crate::install)) needs to name which is which.
pub const INTER_VARIABLE_INDEX: usize = 0;
/// Index of the JetBrains Mono face in [`font_data`]'s array. See
/// [`INTER_VARIABLE_INDEX`].
pub const JETBRAINS_MONO_VARIABLE_INDEX: usize = 1;

/// Every bundled shadcn font face's raw bytes (2 faces: Inter Variable, then
/// JetBrains Mono Variable), in no particular registration order — fontique
/// groups registered faces by the family name each face's own `name` table
/// carries, so a caller can register them in any order, one at a time or all at
/// once.
///
/// The two `*_INDEX` constants above are the one place this order is
/// load-bearing; keep them in step with the array literal below.
pub fn font_data() -> &'static [&'static [u8]] {
    &[INTER_VARIABLE, JETBRAINS_MONO_VARIABLE]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::theme::{INTER_FAMILY, JETBRAINS_MONO_FAMILY};
    use frust::authoring::text::TextContext;

    #[test]
    fn font_data_registers_and_resolves_both_families_by_name() {
        let mut cx = TextContext::new();
        let mut seen_inter = false;
        let mut seen_mono = false;

        for bytes in font_data() {
            let families = cx
                .register_fonts(bytes.to_vec())
                .expect("bundled shadcn font bytes must register as valid faces");
            for family in families {
                if family.name == INTER_FAMILY {
                    seen_inter = true;
                }
                if family.name == JETBRAINS_MONO_FAMILY {
                    seen_mono = true;
                }
            }
        }

        // The names here are the ones the type scale's font stacks ask for, so a
        // mismatch would mean text silently falling back to the system sans.
        assert!(
            seen_inter,
            "expected `font_data()` to register an \"{INTER_FAMILY}\" family"
        );
        assert!(
            seen_mono,
            "expected `font_data()` to register a \"{JETBRAINS_MONO_FAMILY}\" family"
        );
    }

    #[test]
    fn font_data_returns_both_variable_faces() {
        assert_eq!(
            font_data().len(),
            2,
            "expected Inter Variable + JetBrains Mono Variable"
        );
        assert_eq!(font_data()[INTER_VARIABLE_INDEX], INTER_VARIABLE);
        assert_eq!(
            font_data()[JETBRAINS_MONO_VARIABLE_INDEX],
            JETBRAINS_MONO_VARIABLE
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
