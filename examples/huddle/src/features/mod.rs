//! Feature slices — the clean-architecture spine each stateful screen plugs
//! into (PLAN.md's "Clean-signals spine").
//!
//! Each feature owns its own controller + use cases, hosted inside its own
//! `screens/<screen>.rs` `Component` via `clean_signals_forgekit::use_controller`
//! (the same seam the old team roster used). A Phase C screen task edits ONLY
//! its own `screens/<name>.rs` + `features/<name>/**`, never the hub files
//! (`lib.rs`, `shell.rs`, `routes.rs`, `mock/`, `ui/`, this `mod.rs`) — see
//! `src/README-phase-c.md`.
//!
//! Shipped in the skeleton (task 10):
//! - [`settings`] — the design-language + brightness selection spine and the
//!   `SettingsController` → `set_app_theme` single-call-site mechanism.
//!
//! Phase C fills in the remaining feature slices (channels, messages, search,
//! activity, profile) alongside the screens that host them.

pub mod settings;
