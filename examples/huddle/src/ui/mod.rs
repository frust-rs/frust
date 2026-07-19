//! App-wide UI services mounted by the shell — **hub files finalized in the
//! skeleton (task 10)**.
//!
//! Phase C screens CONSUME these through their cloneable handles (obtained from
//! [`crate::HuddleState`]) but never edit this module tree (see
//! `src/README-phase-c.md`).
//!
//! - [`toast`] — the overlay toast/snackbar service ([`toast::ToastController`])
//!   mounted in the shell's reserved overlay slot.
//! - [`sheet`] — the in-screen bottom-sheet overlay (scrim + slide-in +
//!   drag-to-dismiss) plus the emoji-picker / action-row content helpers Phase
//!   D's message actions mount (task 20); the hub freeze is lifted for Phase D.

pub mod sheet;
pub mod swipeable;
pub mod toast;
