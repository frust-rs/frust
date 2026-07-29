//! Feature slices — the clean-architecture spine each stateful screen plugs
//! into.
//!
//! Each feature owns its own `{domain,data,presentation}/` slice under
//! `features/<name>/`, with pages hosted inside `presentation/pages/` as a
//! `Component` via `clean_signals_frust::use_controller`. See
//! `src/README-phase-c.md` for the full feature-slice convention (imitate
//! the sibling's `team-demo`) and `tests/architecture.rs` for its automated
//! enforcement — adding a new feature slice needs a `pub mod <name>;` line
//! here.
//!
//! - [`settings`] — the design-language + brightness selection spine and the
//!   `SettingsController` → `set_app_theme` single-call-site mechanism.
//! - [`search`] — the live channel/people/message filter (`SearchController`,
//!   `signals + Memo`, no `ControllerCore`/`UseCase` — search is synchronous).
//! - [`profile`] — the synchronous mock-dataset lookup behind
//!   `profile::presentation::pages::profile` (no controller/use-case spine
//!   needed — see its module docs).
//! - [`messages`] — the channel/DM message-feed view model (load / send /
//!   react / thread-reply / pagination), hosted by
//!   [`channel_feed`](crate::features::messages::presentation::pages::channel_feed)
//!   and consumed later by the thread screen.
//! - [`activity`] — the mentions feed's async load + mark-all-read
//!   controller.

pub mod activity;
pub mod channels;
pub mod messages;
pub mod profile;
pub mod search;
pub mod settings;
