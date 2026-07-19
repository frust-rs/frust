//! Feature slices — the clean-architecture spine each stateful screen plugs
//! into (PLAN.md's "Clean-signals spine").
//!
//! Each feature owns its own controller + use cases, hosted inside its own
//! `screens/<screen>.rs` `Component` via `clean_signals_frust::use_controller`
//! (the same seam the old team roster used). A Phase C screen task edits ONLY
//! its own `screens/<name>.rs` + `features/<name>/**`, never the hub files
//! (`lib.rs`, `shell.rs`, `routes.rs`, `mock/`, `ui/`, this `mod.rs`) — see
//! `src/README-phase-c.md`.
//!
//! Shipped in the skeleton (task 10):
//! - [`settings`] — the design-language + brightness selection spine and the
//!   `SettingsController` → `set_app_theme` single-call-site mechanism.
//!
//! Added by a Phase C screen task (per `src/README-phase-c.md`'s explicit
//! "adding a `pub mod <name>;` line ... only if its feature slice is new"
//! exception — the only edit that task makes to this otherwise-frozen file):
//! - [`search`] — the live channel/people/message filter (`SearchController`,
//!   `signals + Memo`, no `ControllerCore`/`UseCase` — search is synchronous).
//! - [`profile`] (task 16) — the synchronous mock-dataset lookup behind
//!   `profile::presentation::pages::profile` (no controller/use-case spine
//!   needed — see its module docs).
//!
//! Phase C additions (each a new feature slice registered by its own screen
//! task per `src/README-phase-c.md`'s "new feature slice" rule):
//! - [`messages`] — the channel/DM message-feed view model (load / send /
//!   react / thread-reply / pagination), hosted by
//!   [`channel_feed`](crate::features::messages::presentation::pages::channel_feed)
//!   and consumed later by the thread screen.
//! - [`activity`] (Phase C task 14) — the mentions feed's async load +
//!   mark-all-read controller.

pub mod activity;
pub mod channels;
pub mod messages;
pub mod profile;
pub mod search;
pub mod settings;
