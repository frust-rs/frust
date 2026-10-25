//! The Android NDK oracle backend (`AFontMatcher`, API 29).
//!
//! Compiled only for `target_os = "android"` (the cfg gate is on this module's `mod` line in
//! `oracle/mod.rs`). A stub until o-03 fills it in: every question is a miss,
//! so the oracle falls through to the app-font registry's generic fallbacks.
//!
//! Planned shape (s-03): one reused `AFontMatcher` per (family, weight, italic, locales) key; `AFont_getFontFilePath`/`AFont_getCollectionIndex` give (file, face index), and the matched weight/italic become a synthesis hint (variable Roboto takes `wght`, CJK bold needs faux bold).

use super::{Backend, FaceSource, FallbackRequest};
use crate::style::GenericSlot;

/// Answers fallback questions through the NDK font matcher.
#[derive(Debug, Default)]
pub(crate) struct NdkFontBackend {}

impl NdkFontBackend {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

impl Backend for NdkFontBackend {
    fn resolve(&mut self, _req: &FallbackRequest) -> Option<FaceSource> {
        None
    }

    fn default_family(&self, _generic: GenericSlot) -> Option<FaceSource> {
        None
    }

    fn os_build_id(&self) -> String {
        String::from("ndk-stub")
    }
}
