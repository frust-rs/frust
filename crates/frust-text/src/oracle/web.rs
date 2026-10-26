//! The browser oracle backend (`wasm32`).
//!
//! Compiled only for `target_family = "wasm"` (the cfg gate is on this module's `mod` line in
//! `oracle/mod.rs`).
//!
//! # No system fonts in a browser
//!
//! A browser exposes no font files and no installed-family list, so this backend never answers a
//! question: [`Backend::resolve`] and [`Backend::default_family`] both return `None`, and every
//! answer comes from the app-font registry. The oracle's resolve pipeline reaches it in this order
//! (see `selector`):
//!
//! 1. app faces named by the family chain (`register_app_fonts`);
//! 2. the backend miss, cached as such;
//! 3. the app generic fallbacks for the chain's generic slots, which is where a web shell's
//!    `register_generic_fallback` faces land — the designated face for `SystemUi` and `SansSerif`.
//!
//! [`Backend::default_family`] therefore returns `None` rather than copying the registry's
//! generic table: [`Oracle::default_face`] already falls through to the registry's
//! `generic_faces` for the slot, and the registry is the one owner of which bytes a generic slot
//! designates. The trait has no registry handle, so this backend cannot name that face itself.
//!
//! # Gaps kept on purpose
//!
//! The shell designates faces for `SystemUi` and `SansSerif` only. `Monospace`, `Serif` and
//! `Emoji` have no designated face on the web, so a `FontFamily` that names only those slots
//! resolves nothing (`LIMITATIONS.md` entry `web-generic-family-partial-fallback`). This is a
//! scoped gap, not a bug to patch here: a follow-up may bundle subsets for those slots (Flutter
//! downloads Noto on demand instead).
//!
//! # Persistence
//!
//! The OS build id is the crate version only: a browser has no OS identity to stamp. No
//! [`FaceSource`] is ever a [`FaceLocation::Path`] on this target, so the face store never reads a
//! file.

use super::{Backend, FaceSource, FallbackRequest};
use crate::style::GenericSlot;

/// Answers fallback questions on the web: nothing from the system, so every question is a miss
/// and the app-font registry answers instead.
#[derive(Debug, Default)]
pub(crate) struct WebBackend {}

impl WebBackend {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

impl Backend for WebBackend {
    fn resolve(&mut self, _req: &FallbackRequest) -> Option<FaceSource> {
        None
    }

    fn default_family(&self, _generic: GenericSlot) -> Option<FaceSource> {
        None
    }

    fn os_build_id(&self) -> String {
        String::from(env!("CARGO_PKG_VERSION"))
    }
}
