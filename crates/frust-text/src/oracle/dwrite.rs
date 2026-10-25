//! The DirectWrite oracle backend (Windows).
//!
//! Compiled only for `windows` (the cfg gate is on this module's `mod` line in
//! `oracle/mod.rs`). A stub until o-05 fills it in: every question is a miss,
//! so the oracle falls through to the app-font registry's generic fallbacks.
//!
//! Planned shape (s-04): `IDWriteFontFallback::MapCharacters` over the cluster, then the mapped font's local file loader gives the file path and `IDWriteFontFace::GetIndex` the face index (windows-sys + windows-link, no `windows` crate).

use super::{Backend, FaceSource, FallbackRequest};
use crate::style::GenericSlot;

/// Answers fallback questions through DirectWrite.
#[derive(Debug, Default)]
pub(crate) struct DirectWriteBackend {}

impl DirectWriteBackend {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

impl Backend for DirectWriteBackend {
    fn resolve(&mut self, _req: &FallbackRequest) -> Option<FaceSource> {
        None
    }

    fn default_family(&self, _generic: GenericSlot) -> Option<FaceSource> {
        None
    }

    fn os_build_id(&self) -> String {
        String::from("dwrite-stub")
    }
}
