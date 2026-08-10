//! The TUI's animation substrate: pure frame-selection math for the loading
//! spinner and the shimmer sweep, driven off `AppState::animation_frame`
//! (`crate::engine::state`).
//!
//! Everything here is intentionally **pure**: no `AppState`, no rendering
//! side effects, no I/O — only frame/phase math and (for [`shimmer`]) the
//! `ratatui::text::Span`s a caller drops straight into a `Line`. A caller
//! passes `state.animation_frame` in and gets a glyph or spans back; the
//! runner's 50 ms tick (`crate::runner`) is what advances that counter, only
//! while `AppState::animating()` is `true`.
//!
//! Ported from fdemon's `widgets::spinner`/`widgets::shimmer` (a pattern
//! source, not a copy source — see `docs/TUI_CODE_STANDARDS.md`): the frame
//! math and unit tests carry over, but the shimmer color inputs are the
//! TUI's own [`crate::ui::theme::Theme`] tokens, never fdemon's hardcoded
//! RGB palette.

pub mod shimmer;
pub mod spinner;

pub use shimmer::{lerp_color, shimmer_phase, shimmer_spans, themed_shimmer_spans};
pub use spinner::{SPINNER_TICKS_PER_FRAME, spinner_char};
