//! [`MuteChannel`] — the mute use case (synchronous, mirror of
//! [`ArchiveChannel`](super::archive_channel::ArchiveChannel)). Echoes the
//! requested flag change; reads nothing, so it takes no repository.

use clean_signals::UseCase;

use crate::failure::HuddleFailure;
use crate::features::channels::domain::models::FlagParams;

/// The mute use case: echoes the request back to the controller.
pub struct MuteChannel;

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for MuteChannel {
    type Params = FlagParams;
    type Output = FlagParams;
    type Failure = HuddleFailure;

    async fn execute(&self, params: FlagParams) -> Result<FlagParams, HuddleFailure> {
        Ok(params)
    }
}
