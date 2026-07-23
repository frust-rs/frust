//! Motion section — **filled by `c08`**: the duration/easing token table, an
//! `AnimatedOpacity`/`AnimatedScale` playground, a `PatternSwitcher` pattern
//! picker (FadeThrough / SharedAxis / FadeScale / GlyphSlide) on a demo card,
//! the staggered log-reveal + boot-sequence replays, a screen-transition
//! next/back demo, and a reduced-motion note tied to `state.reduce_motion`.
//!
//! Stub scaffold (`c01`): a placeholder honoring the page-fn contract in
//! `pages/mod.rs`.

use frust::{AnyView, any, text};

use crate::CatalogState;

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    any(text("Motion — filled by c08"))
}
