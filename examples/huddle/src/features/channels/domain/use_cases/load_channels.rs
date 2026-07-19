//! [`LoadChannels`] — the async roster loader.
//!
//! Takes the [`ChannelRepository`] as `Arc<dyn ChannelRepository + Send + Sync>`
//! (the team-demo `LoadTeam` shape) and delegates to it. The deliberate ~600ms
//! mock latency (so the Home screen's loading skeletons are visible) and the
//! failure boundary now live in the data-layer implementation; the use case
//! itself just drives the trait. Loads cleanly — one attempt, never fails —
//! so no retry vocabulary is exercised here.
//!
//! Never call [`UseCase::execute`] directly from a controller — always drive it
//! through `ControllerCore::run`/`run_into`.

use std::sync::Arc;

use clean_signals::{NoParams, UseCase};

use crate::failure::HuddleFailure;
use crate::features::channels::domain::models::ChannelsData;
use crate::features::channels::domain::repositories::ChannelRepository;

/// Loads the channel + DM roster via the injected [`ChannelRepository`].
pub struct LoadChannels {
    repo: Arc<dyn ChannelRepository + Send + Sync>,
}

impl LoadChannels {
    /// Construct with the injected repository.
    pub fn new(repo: Arc<dyn ChannelRepository + Send + Sync>) -> Self {
        Self { repo }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for LoadChannels {
    type Params = NoParams;
    type Output = ChannelsData;
    type Failure = HuddleFailure;

    async fn execute(&self, _params: NoParams) -> Result<ChannelsData, HuddleFailure> {
        self.repo.load_channels().await
    }
}
