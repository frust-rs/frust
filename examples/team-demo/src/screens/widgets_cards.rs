//! Widgets · Cards screen — PLACEHOLDER.
//!
//! Filled in by wave-2 **task 03 (widgets-screens)** — the catalog cards +
//! 1000-row virtualized ListView port. The shell + route wiring (task 02) is
//! already final, so task 03 edits only this file.

use forgekit::{Align, Alignment, AnyView, any, text};

use crate::ShellState;

/// The Cards widgets exhibit (placeholder until task 03).
pub fn cards_screen() -> AnyView<ShellState> {
    any(Align(
        Alignment::CENTER,
        text("Widgets · Cards — coming in wave 2 (task 03)").size(20.0),
    ))
}
