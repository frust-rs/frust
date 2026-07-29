//! App-wide UI services mounted by the shell — hub files shared across
//! features.
//!
//! Feature presentation code CONSUMES these through their cloneable handles
//! (obtained from [`crate::HuddleState`]) rather than reimplementing them (see
//! `src/README-phase-c.md` for the feature-slice convention).
//!
//! - [`toast`] — the overlay toast/snackbar service ([`toast::ToastController`])
//!   mounted in the shell's reserved overlay slot.
//! - [`sheet`] — the in-screen bottom-sheet overlay (scrim + slide-in +
//!   drag-to-dismiss) plus the emoji-picker / action-row content helpers the
//!   message actions mount.
//! - [`fill_box`] — the arbitrary-color rounded-rect escape-hatch primitives
//!   (`fill_box` leaf + `filled_box` single-child container), promoted here
//!   from `screens::home` so the feed restyle and Home share them.
//! - [`scaffold`] — the shared titled-screen shell (`scaffold`/
//!   `placeholder_body`), promoted here from the `screens` hub so the
//!   profile/search/thread pages can share it.
//! - [`solid_source`] — the arbitrary-color 1×1 `ImageSource` fill helper
//!   (`solid_source`/`solid_source_alpha`), promoted here from
//!   `features::settings` so the You tab's avatar block can share it with
//!   the settings pages.

pub mod fill_box;
pub mod scaffold;
pub mod sheet;
pub mod solid_source;
pub mod swipeable;
pub mod toast;
