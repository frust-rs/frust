//! The browser oracle backend (`wasm32`).
//!
//! Compiled only for `target_family = "wasm"` (the cfg gate is on this module's `mod` line in
//! `oracle/mod.rs`). A stub until o-06 fills it in: every question is a miss,
//! so the oracle falls through to the app-font registry's generic fallbacks.
//!
//! A browser exposes no system font files, so this backend may stay empty: a web host supplies its faces through `register_generic_fallback` and app fonts, which the oracle consults without a backend.

use super::{Backend, FaceSource, FallbackRequest};
use crate::style::GenericSlot;

/// Answers fallback questions on the web (nothing to ask).
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
        String::from("web-stub")
    }
}
