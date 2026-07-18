//! Showcase · Theme screen — PLACEHOLDER.
//!
//! Filled in by wave-2 **task 04 (theme-motion-screens)** — the gallery theme
//! port (color-role grid, 15-token type scale, elevation shadows). The shell +
//! route wiring (task 02) is already final, so task 04 edits only this file.

use forgekit::{Align, Alignment, AnyView, any, text};

use crate::ShellState;

/// The theme exhibit (placeholder until task 04).
pub fn theme_screen() -> AnyView<ShellState> {
    any(Align(
        Alignment::CENTER,
        text("Showcase · Theme — coming in wave 2 (task 04)").size(20.0),
    ))
}
