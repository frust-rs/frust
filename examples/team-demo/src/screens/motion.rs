//! Showcase · Motion screen — PLACEHOLDER.
//!
//! Filled in by wave-2 **task 04 (theme-motion-screens)** — the gallery motion
//! port (curves/tweens + M3/iOS spring presets on `AnimationController`, with
//! the raw-`Spring` escape hatch retired). The shell + route wiring (task 02)
//! is already final, so task 04 edits only this file.

use forgekit::{Align, Alignment, AnyView, any, text};

use crate::ShellState;

/// The motion exhibit (placeholder until task 04).
pub fn motion_screen() -> AnyView<ShellState> {
    any(Align(
        Alignment::CENTER,
        text("Showcase · Motion — coming in wave 2 (task 04)").size(20.0),
    ))
}
