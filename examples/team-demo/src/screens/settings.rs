//! Settings screen — PLACEHOLDER.
//!
//! Filled in by wave-2 **task 06 (settings-profile-screens)** — the NEW
//! `SettingsController` driving design-language (Material3 ⇄ Cupertino) +
//! dark/light selection via `set_app_theme`, plus the transition-preset
//! default. Its domain lives in [`crate::settings_domain`]. The shell + route
//! wiring (task 02) is already final, so task 06 edits only this file and
//! `settings_domain`.

use forgekit::{Align, Alignment, AnyView, any, text};

use crate::ShellState;

/// The settings screen (placeholder until task 06).
pub fn settings_screen() -> AnyView<ShellState> {
    any(Align(
        Alignment::CENTER,
        text("Settings — coming in wave 2 (task 06)").size(20.0),
    ))
}
