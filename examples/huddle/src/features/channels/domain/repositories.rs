//! The channels feature's domain repository contract (huddle
//! clean-architecture refactor).
//!
//! [`ChannelRepository`] is the seam between the presentation controller / its
//! use cases and the data layer: the [`LoadChannels`](super::use_cases::LoadChannels)
//! use case reads the roster through this trait, and the data layer's
//! `StoreChannelRepository` implements it over the shared in-memory store,
//! owning the feature's single failure-mapping site. Consumed as
//! `Arc<dyn ChannelRepository + Send + Sync>` (the team-demo DI convention), so
//! neither the controller nor its use cases carry a repository type parameter.
//!
//! The signature is shaped by exactly what the controller/use cases read today
//! (no speculative methods): the roster loader is the only fallible read —
//! `Archive`/`Mute` are pure signal patches with no data access, so they take
//! no repository.

use crate::failure::HuddleFailure;
use crate::features::channels::domain::models::ChannelsData;

/// The same dual `cfg_attr` every async trait in this codebase uses (per
/// `docs/CODE_STANDARDS.md`) — `wasm32`'s single-threaded event loop can't
/// require `Send` futures.
#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
pub trait ChannelRepository {
    /// Load the channel + DM roster (each row carrying its unread count and
    /// archived/muted flags). Returns cleanly today — the simulated latency and
    /// the (currently branch-free) failure boundary live in the implementation.
    async fn load_channels(&self) -> Result<ChannelsData, HuddleFailure>;
}
