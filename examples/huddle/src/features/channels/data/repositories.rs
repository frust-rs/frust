//! [`StoreChannelRepository`] — the [`ChannelRepository`] implementation this
//! app wires up (huddle clean-architecture refactor).
//!
//! Adapts the shared in-memory [`crate::data::store`] to the channels domain
//! trait: it materializes the roster from the immutable dataset and owns this
//! feature's single failure-mapping boundary. The store reads are infallible
//! today, so the boundary is currently branch-free — the method preserves the
//! exact behavior of the pre-refactor `LoadChannels`/`seed_roster` pair (a
//! ~600ms simulated latency then a clean roster; no fail-once-then-retry). If a
//! future real source can fail, the `map_*_error -> HuddleFailure` conversion
//! lands here, at this one site, and nothing above the data layer changes.
//!
//! This is the **only** channels file that mentions [`crate::data::store`] (the
//! inward dependency rule: only the data layer may reach into the shared
//! store; domain/presentation never do).

use std::time::Duration;

use crate::failure::HuddleFailure;
use crate::features::channels::domain::models::{ChannelItem, ChannelsData, DmItem};
use crate::features::channels::domain::repositories::ChannelRepository;
use crate::features::profile::domain::UserStatus;

/// The mock load latency — long enough that the Home screen's loading skeletons
/// are actually seen before the roster appears.
pub const LOAD_LATENCY: Duration = Duration::from_millis(600);

/// The store-backed [`ChannelRepository`]: materializes the roster from the
/// shared immutable dataset.
#[derive(Clone, Copy, Default)]
pub struct StoreChannelRepository;

impl StoreChannelRepository {
    /// A fresh repository handle (the dataset it reads is process-global static
    /// state, so the handle carries no state of its own).
    pub fn new() -> Self {
        Self
    }
}

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl ChannelRepository for StoreChannelRepository {
    async fn load_channels(&self) -> Result<ChannelsData, HuddleFailure> {
        clean_signals::time::sleep(LOAD_LATENCY).await;
        Ok(seed_roster())
    }
}

/// A deterministic, varied unread count seeded from an id (0..=8) so the roster
/// shows a realistic mix of read and unread rows without any live message state.
fn unread_seed(id: &str) -> u32 {
    id.bytes().map(u32::from).sum::<u32>() % 9
}

/// Materialize the roster from the immutable [`crate::data::store`] dataset.
fn seed_roster() -> ChannelsData {
    let channels = crate::data::store::channels()
        .into_iter()
        .map(|c| ChannelItem {
            id: c.id.to_string(),
            name: c.name.to_string(),
            private: c.private,
            preview: c.topic.to_string(),
            unread: unread_seed(c.id),
            archived: false,
            muted: false,
        })
        .collect();

    let dms = crate::data::store::dms()
        .into_iter()
        .map(|d| {
            let user = crate::data::store::user(d.user_id);
            DmItem {
                id: d.id.to_string(),
                user_id: d.user_id,
                name: user.map(|u| u.name.to_string()).unwrap_or_default(),
                initials: user.map(|u| u.initials.to_string()).unwrap_or_default(),
                status: user.map(|u| u.status).unwrap_or(UserStatus::Away),
                preview: d.preview.to_string(),
                unread: unread_seed(d.id),
                archived: false,
                muted: false,
            }
        })
        .collect();

    ChannelsData { channels, dms }
}
