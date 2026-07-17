//! Imperative navigation (Phase 6b): a retained page stack with push/pop/replace
//! and per-page result callbacks, opaque-page paint culling, and test-pinned
//! capture/focus/IME page-switch semantics.
//!
//! # Module map
//!
//! Every navigation module is pre-declared here so wave-2 tasks add their code to
//! their own file and never edit this module list:
//!
//! * [`navigator`] — the page stack, [`NavigatorController`](navigator::NavigatorController),
//!   opaque-page paint culling, and the page-switch capture/focus/IME contract.
//!   Instant page switches only — transitions arrive in task 03.
//! * [`transition`] — page-transition animations (task 03). A doc-only stub today.
//! * [`router`] — declarative route table (a later task). A doc-only stub today.
//! * [`path`] — path/URL parsing for the router (a later task). A doc-only stub today.

pub mod navigator;
pub mod path;
pub mod router;
pub mod transition;
