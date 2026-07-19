//! [`ArchiveChannel`] — the archive use case (synchronous, the `SetTheme`
//! shape). Echoes the requested flag change so the controller can apply it to
//! its own signal afterwards; the shared store stays immutable, so this reads
//! nothing and takes no repository.

use clean_signals::UseCase;

use crate::failure::HuddleFailure;
use crate::features::channels::domain::models::FlagParams;

/// The archive use case: echoes the request back to the controller.
pub struct ArchiveChannel;

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for ArchiveChannel {
    type Params = FlagParams;
    type Output = FlagParams;
    type Failure = HuddleFailure;

    async fn execute(&self, params: FlagParams) -> Result<FlagParams, HuddleFailure> {
        Ok(params)
    }
}
