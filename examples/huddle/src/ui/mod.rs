//! App-wide UI services mounted by the shell — **hub files finalized in the
//! skeleton (task 10)**.
//!
//! Phase C screens CONSUME these through their cloneable handles (obtained from
//! [`crate::HuddleState`]) but never edit this module tree (see
//! `src/README-phase-c.md`).
//!
//! - [`toast`] — the overlay toast/snackbar service ([`toast::ToastController`])
//!   mounted in the shell's reserved overlay slot.

pub mod toast;
