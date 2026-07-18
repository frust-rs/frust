//! Widgets · Modals screen — PLACEHOLDER.
//!
//! Filled in by wave-2 **task 03 (widgets-screens)** — the catalog modals port
//! (dialog / bottom sheet / Cupertino alert + action sheet, each landing a
//! `PopResult`). The shell + route wiring (task 02) is already final, so task
//! 03 edits only this file.

use forgekit::{Align, Alignment, AnyView, any, text};

use crate::ShellState;

/// The Modals widgets exhibit (placeholder until task 03).
pub fn modals_screen() -> AnyView<ShellState> {
    any(Align(
        Alignment::CENTER,
        text("Widgets · Modals — coming in wave 2 (task 03)").size(20.0),
    ))
}
