//! The `channels` feature slice (huddle clean-architecture refactor, task 02):
//! `domain` ← `data` / `domain` ← `presentation`, per the inward-only
//! dependency rule (PLAN Design Decisions 1–3). The Home tab's channel + DM
//! roster.
//!
//! The former flat `features/channels/mod.rs` (controller + use cases + derived
//! models) split across the layers:
//! - [`domain`] — entities, derived roster models, the [`ChannelRepository`]
//!   contract, and the `LoadChannels`/`ArchiveChannel`/`MuteChannel` use cases.
//! - [`data`] — `StoreChannelRepository` (the trait impl over the shared store;
//!   the feature's single failure-mapping site + the simulated load latency).
//! - [`presentation`] — [`ChannelsController`] and the `home`/`workspace_drawer`
//!   pages.
//!
//! The re-exports below are the feature's public facade: consumers keep using
//! `channels::ChannelsController`, `channels::ChannelsData`, etc. without
//! reaching into layer paths.

pub mod data;
pub mod domain;
pub mod presentation;

pub use domain::{ChannelItem, ChannelsData, DmItem};
pub use presentation::controllers::{ChannelsController, set_archived_flag, set_muted_flag};
