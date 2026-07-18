//! Showcase · Notes screen — PLACEHOLDER.
//!
//! Filled in by wave-2 **task 05 (notes-screen)** — the notes port
//! (TextInput/IME, submit-to-keyed-list, embedded Image), refactored onto a
//! `NotesController` in [`crate::notes_domain`]. The shell + route wiring
//! (task 02) is already final, so task 05 edits only this file and
//! `notes_domain`.

use forgekit::{Align, Alignment, AnyView, any, text};

use crate::ShellState;

/// The notes exhibit (placeholder until task 05).
pub fn notes_screen() -> AnyView<ShellState> {
    any(Align(
        Alignment::CENTER,
        text("Showcase · Notes — coming in wave 2 (task 05)").size(20.0),
    ))
}
