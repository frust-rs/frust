//! The shared playground kit: the preview/controls scaffolding every ported
//! playground is assembled from (the reference's `widgets/playground/`).
//!
//! Ported from `material_3_expressive`'s example app —
//! `widgets/playground/playground_body.dart`, `play_preview_card.dart`,
//! `play_code_snippet.dart`, `control_panel.dart`, and `controls/`
//! (`play_enum_segmented.dart`, `play_slider.dart`, `play_switch.dart`,
//! `play_enum_menu.dart`, `play_text_field.dart`). The route-level chrome
//! (app bar, back button, brightness toggle) is a *separate* port —
//! [`crate::pages::playground_scaffold`] — this module is only the in-page
//! body arrangement.
//!
//! # Generic over the page's own knob state, not [`crate::AppState`]
//!
//! Per the page contract ([`crate::pages::playground`]), a page owns its
//! knobs in a nested `frust::Component`, so every kit function here is
//! generic over `State: 'static` — the page's own knob-component state, not
//! [`crate::AppState`] — the same shape `frust_material`'s own controlled
//! widgets (`slider`, `switch`, `segmented_button`, ...) already take.
//!
//! # Divergences from the reference
//!
//! - **[`play_enum_menu_field`]/[`play_enum_menu_panel`] are two pieces, not
//!   one.** `frust_material::dropdown` is itself split into a trigger and a
//!   panel because "a widget paints inside its parent's box, so the panel
//!   cannot live inside the field" (that module's own doc) — this kit
//!   inherits that split rather than papering over it with a widget this
//!   catalog has no seam for. A page using this control mounts the field in
//!   its control-panel row list and the panel at its own outer `Stack`,
//!   sharing one `frust_material::OverlayAnchor` in its knob state — the
//!   pattern `OverlayAnchor`'s own doc names ("An app keeps one per anchored
//!   overlay in its `Component::State`, hands a clone to the trigger and
//!   another to the host").
//! - **No snackbar confirmation on copy.** The reference shows an
//!   `M3ESnackbar` after a successful copy; that needs a
//!   `frust_material::SnackbarController` mounted somewhere an app holds,
//!   which [`crate::AppState`] does not yet carry — out of scope for the kit
//!   itself. [`play_code_snippet`]'s copy action still wires
//!   `frust_clipboard::Clipboard::set_text` directly (see
//!   [`copy_to_clipboard`]).
//! - **`kPlaySnippetImport`/`playDartString` are not ported.** Both are
//!   Dart-string-building helpers for assembling *Dart* snippet source; a
//!   page here writes its Rust snippet text as a plain string literal
//!   instead, so there is nothing left for them to help with.
//!
//! # `#![allow(dead_code)]`: landed ahead of its callers
//!
//! `material3-demo` is a `bin` crate (no `lib.rs`), so `dead_code` tracks
//! reachability from `fn main` — a `pub` item earns no exemption the way a
//! library crate's public API does. Every one of the 39 pages still returns
//! [`crate::widgets::coming_soon`] (p5-01's placeholder), so nothing outside
//! this module's own unit tests calls into the kit yet; wiring a page to it
//! is the next task's job, not this one's (`docs/DEVELOPMENT.md`'s standard
//! verify gate — `cargo clippy --all-targets -- -D warnings` — would
//! otherwise fail on every symbol here). The re-export lines below trip the
//! same reachability check as plain `unused_imports`, so both lints are
//! allowed here. Drop this once the first real playground page lands.
#![allow(dead_code, unused_imports)]

mod body;
mod code_snippet;
mod control_panel;
mod controls;
mod preview_card;

pub use body::playground_body;
pub use code_snippet::{PlaySnippet, copy_to_clipboard, play_code_snippet, play_snippet};
pub use control_panel::control_panel;
pub use controls::{
    play_enum_menu_field, play_enum_menu_panel, play_enum_segmented, play_slider, play_switch,
    play_text_field,
};
pub use preview_card::play_preview_card;

use frust::{Theme, use_context};

/// The ambient [`Theme`], or `frust_material::baseline` before one is
/// provided — the fallback every kit widget resolves its type/color roles
/// through, mirroring [`crate::pages::playground_scaffold`]'s identical
/// fallback.
pub(crate) fn ambient_theme() -> Theme {
    use_context::<Theme>().unwrap_or_else(frust_material::baseline)
}
