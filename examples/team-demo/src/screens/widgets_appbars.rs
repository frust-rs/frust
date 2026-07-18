//! Widgets · AppBars screen — PLACEHOLDER.
//!
//! Filled in by wave-2 **task 03 (widgets-screens)** — the catalog app-bar
//! port (Material `app_bar` + Cupertino `cupertino_nav_bar` side by side). The
//! shell + route wiring (task 02) is already final, so task 03 edits only this
//! file.

use forgekit::{Align, Alignment, AnyView, any, text};

use crate::ShellState;

/// The AppBars widgets exhibit (placeholder until task 03).
pub fn appbars_screen() -> AnyView<ShellState> {
    any(Align(
        Alignment::CENTER,
        text("Widgets · AppBars — coming in wave 2 (task 03)").size(20.0),
    ))
}
