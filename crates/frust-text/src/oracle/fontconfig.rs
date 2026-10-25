//! Sanctioned unsafe zone (decision D-6): FFI to fontconfig, reached through `dlopen`.
//! Every `unsafe` block carries a SAFETY note, and the FFI stays behind safe
//! wrappers that never leak past the `Backend` trait. No other oracle module
//! may contain `unsafe`; the guard tests in `oracle/mod.rs` enforce it.
//!
//! The fontconfig oracle backend (Linux, FreeBSD).
//!
//! Compiled only for `target_os = "linux"` or `"freebsd"` (the cfg gate is on this module's `mod` line in
//! `oracle/mod.rs`). A stub until o-04 fills it in: every question is a miss,
//! so the oracle falls through to the app-font registry's generic fallbacks.
//!
//! Planned shape: an `FcFontSort`/`FcFontMatch` query per cluster (charset, lang, weight, slant) through the `dlopen`-loaded library, answering the matched pattern's `FC_FILE` and `FC_INDEX`.

#![allow(unsafe_code)]

use super::{Backend, FaceSource, FallbackRequest};
use crate::style::GenericSlot;

/// Answers fallback questions through fontconfig.
#[derive(Debug, Default)]
pub(crate) struct FontconfigBackend {}

impl FontconfigBackend {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

impl Backend for FontconfigBackend {
    fn resolve(&mut self, _req: &FallbackRequest) -> Option<FaceSource> {
        None
    }

    fn default_family(&self, _generic: GenericSlot) -> Option<FaceSource> {
        None
    }

    fn os_build_id(&self) -> String {
        String::from("fontconfig-stub")
    }
}
