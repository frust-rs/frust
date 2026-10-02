//! The browser tier's bundled default face.
//!
//! `frust_text::TextContext::new` builds parley's `FontContext` from
//! fontique's system-font source, which on `target_arch = "wasm32"` is a stub
//! backend carrying an empty generic-family map (see that function's own
//! docs). With nothing mapped, `frust_text::FontFamily::SystemUi` — the
//! framework's default family — and any bare `frust_text::GenericSlot`
//! resolve to no font at all and shape zero glyph runs, so default-family
//! text is invisible until an app registers one itself.
//!
//! [`install_default_fonts`] is this tier's fix: it bundles one face and
//! registers it as the `SystemUi`/`SansSerif` generic-family fallback through
//! `frust_text::register_generic_fallback`, so the shell's own `TextContext`
//! (and any widget's private one, e.g. `TextInput`'s) resolves default-family
//! text to it. `Monospace`, `Serif`, and `Emoji` are deliberately left
//! unmapped: a proportional face substituted for `Monospace` would silently
//! regress `TextInput`/code-display layout, and this crate ships neither a
//! serif nor an emoji face. An app's own named-family registration
//! (`frust_text::TextContext::register_fonts`) still wins over this fallback,
//! since a named lookup always resolves before a generic one.
//!
//! # Bundled face: size and licence
//!
//! `fonts/InterVariable.ttf` — Inter, the variable TTF build, 879,708 bytes —
//! under the SIL Open Font License 1.1 (`fonts/OFL.txt`, bundled alongside
//! it). Every `frust-shell-web` wasm binary carries these bytes via
//! `include_bytes!` regardless of whether an app registers its own fonts;
//! there is no opt-out today. This is the same face `examples/web-gallery`
//! already ships and hand-registers for the identical purpose — once an app
//! (or a shell) calls this seam, that hand registration is redundant and can
//! be dropped in favor of this default.

use frust_text::{GenericSlot, register_generic_fallback};

/// The bundled default face: Inter Variable (SIL OFL 1.1). Copied from
/// `plugins/shadcn/fonts/inter/InterVariable.ttf` — this crate does not
/// depend on `plugins/shadcn` (a plugin crate sits above SHELLS; see
/// `docs/ARCHITECTURE.md`'s layer dependencies), so the bytes are a plain
/// copy, not a shared path. See this module's docs for size/licence.
const DEFAULT_FACE: &[u8] = include_bytes!("../fonts/InterVariable.ttf");

/// Whether [`install_default_fonts`] has already queued [`DEFAULT_FACE`] this
/// process — the flag that makes repeat calls a no-op (see that function's
/// docs).
static INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Registers [`DEFAULT_FACE`] as the `SystemUi`/`SansSerif` generic-family
/// fallback (see this module's docs), so every `TextContext` built afterward
/// resolves default-family text to it when nothing better — a real system
/// font, or an app-registered named family — is available.
///
/// Idempotent: a second (or later) call is a no-op, guarded by
/// [`INSTALLED`] — `frust_text::register_generic_fallback` itself only ever
/// queues a pending entry onto a process-wide, never-drained record, so
/// without this guard every extra call here would queue one more (redundant,
/// not incorrect, but wasteful) copy of the same face.
///
/// Called once, before the shell constructs its own `TextContext`
/// (`app_handler`'s browser event-loop start-up) — the seam a shell must
/// call ahead of that construction for the mapping to be live in the
/// context it hands to widget layout, per
/// `frust_text::register_generic_fallback`'s own contract.
pub fn install_default_fonts() {
    if INSTALLED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    register_generic_fallback(
        DEFAULT_FACE.to_vec(),
        &[GenericSlot::SystemUi, GenericSlot::SansSerif],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cheap parse-ability smoke check on the bundled bytes — not a full
    /// shape, since that needs a `TextContext`/collection round trip this
    /// module has no reason to own; `frust-text`'s own tests cover the
    /// registration/generic-family-append mechanics this function drives.
    #[test]
    fn default_face_bytes_look_like_a_font() {
        // TrueType/OpenType files open with one of a small set of 4-byte
        // sniffs; a bundling mistake (an empty file, a text README swapped
        // in by accident) would fail this cheaply, before any font parser
        // ever sees it.
        const TRUETYPE: &[u8; 4] = &[0x00, 0x01, 0x00, 0x00];
        const OPENTYPE: &[u8; 4] = b"OTTO";
        const TRUETYPE_TAG: &[u8; 4] = b"true";
        assert!(
            DEFAULT_FACE.len() > 4
                && (DEFAULT_FACE.starts_with(TRUETYPE)
                    || DEFAULT_FACE.starts_with(OPENTYPE)
                    || DEFAULT_FACE.starts_with(TRUETYPE_TAG)),
            "the bundled default face must sniff as TrueType/OpenType"
        );
    }

    #[test]
    fn install_default_fonts_does_not_panic_when_called_more_than_once() {
        install_default_fonts();
        install_default_fonts();
    }
}
