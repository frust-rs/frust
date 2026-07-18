//! Profile screen — PLACEHOLDER.
//!
//! Filled in by wave-2 **task 06 (settings-profile-screens)** — the NEW
//! profile screen (avatar Image, editable fields via TextInput/IME with a
//! save/validate UseCase, failure path surfaced) driven by a
//! `ProfileController` in [`crate::profile_domain`]. Reached from the shell's
//! app-bar avatar button (`router.push("/profile")`). The shell + route wiring
//! (task 02) is already final, so task 06 edits only this file and
//! `profile_domain`.

use forgekit::{Align, Alignment, AnyView, any, text};

use crate::ShellState;

/// The profile screen (placeholder until task 06).
pub fn profile_screen() -> AnyView<ShellState> {
    any(Align(
        Alignment::CENTER,
        text("Profile — coming in wave 2 (task 06)").size(20.0),
    ))
}
