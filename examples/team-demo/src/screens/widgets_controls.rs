//! Widgets · Controls screen — PLACEHOLDER.
//!
//! Filled in by wave-2 **task 03 (widgets-screens)** — the catalog controls
//! port (switch/chips/FAB/progress/loading indicator/button group/split
//! button/toolbars/slider). The shell + route wiring (task 02) is already
//! final, so task 03 edits only this file.

use forgekit::{Align, Alignment, AnyView, any, text};

use crate::ShellState;

/// The Controls widgets exhibit (placeholder until task 03).
pub fn controls_screen() -> AnyView<ShellState> {
    any(Align(
        Alignment::CENTER,
        text("Widgets · Controls — coming in wave 2 (task 03)").size(20.0),
    ))
}
