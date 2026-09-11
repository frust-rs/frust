//! The app-facing half of this crate, behind the default-on `frust-api`
//! feature: the declarative view that pairs a [`crate::PlayerSession`] with
//! its native platform-view slot, so an app writes a video the way it writes
//! any other widget rather than wiring `view_type`/`params_json` by hand.
//!
//! # Charter: why this module exists at all
//!
//! This is what makes the crate's `--no-default-features` build the real
//! platform-plugin charter line rather than an aspirational one (crate root
//! doc's *Charter* section): the [`handle`]/[`view`] modules below are the
//! *only* place in this crate that names `frust`, so disabling `frust-api`
//! genuinely drops the facade dependency from the graph, and enabling it
//! genuinely adds it back — there is nothing else here riding on the
//! feature for show.
//!
//! # What's here
//!
//! [`VideoPlayerHandle`]/[`use_video_player`] ([`handle`]) pair a
//! [`crate::PlayerSession`] with five `RwSignal`s a component's `build`
//! reads, kept current by the session's one listener registration —
//! `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions' "build reads,
//! handlers write" rule, applied to a platform callback instead of a
//! `Widget::event` handler. [`video_view`] ([`view`]) is the one-line
//! `frust::platform_view` builder over a handle's session.

mod handle;
mod view;

pub use handle::{VideoPlayerHandle, use_video_player};
pub use view::video_view;
