//! Baseline widget set: Text, Button, Image, Column/Row, Stack, ScrollView, etc.
//! (spec §6.4).
//!
//! v0 ships only [`text`] — the first real widget, driving the desktop preview
//! shell's text-rendering exit criterion (spec Phase 0). Every other widget in
//! the baseline set is composition of these and lands in later phases.

mod text;

pub use text::{TextView, TextWidget, text};
