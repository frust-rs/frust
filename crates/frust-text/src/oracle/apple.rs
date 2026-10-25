//! Sanctioned unsafe zone (decision D-6): FFI to CoreText, Apple's font API.
//! Every `unsafe` block carries a SAFETY note, and the FFI stays behind safe
//! wrappers that never leak past the `Backend` trait. No other oracle module
//! may contain `unsafe`; the guard tests in `oracle/mod.rs` enforce it.
//!
//! The CoreText oracle backend (macOS, iOS).
//!
//! Compiled only for `target_vendor = "apple"` (the cfg gate is on this module's `mod` line in
//! `oracle/mod.rs`). A stub until o-02 fills it in: every question is a miss,
//! so the oracle falls through to the app-font registry's generic fallbacks.
//!
//! Planned shape (s-02): `CTFontCreateForStringWithLanguage` per cluster, then the returned font's PostScript name and `kCTFontURLAttribute` map to (file, face index) by name ID 6 or an `fvar` named instance, with `CTFontCopyVariation` as the face's variation coordinates.

#![allow(unsafe_code)]

use super::{Backend, FaceSource, FallbackRequest};
use crate::style::GenericSlot;

/// Answers fallback questions through CoreText.
#[derive(Debug, Default)]
pub(crate) struct CoreTextBackend {}

impl CoreTextBackend {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

impl Backend for CoreTextBackend {
    fn resolve(&mut self, _req: &FallbackRequest) -> Option<FaceSource> {
        None
    }

    fn default_family(&self, _generic: GenericSlot) -> Option<FaceSource> {
        None
    }

    fn os_build_id(&self) -> String {
        String::from("coretext-stub")
    }
}
