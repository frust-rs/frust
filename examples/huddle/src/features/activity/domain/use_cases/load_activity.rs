//! [`LoadActivity`] — the async mentions-feed loader.
//!
//! Takes the [`ActivityRepository`] as `Arc<dyn ActivityRepository + Send +
//! Sync>` (the team-demo `LoadTeam` shape, mirroring `LoadChannels`) and
//! delegates the fallible read to it, then maps every returned
//! [`ActivityItem`] into a freshly-unread [`ActivityRow`]. The mocked ~400ms
//! latency (long enough that the screen's loading skeletons are actually
//! visible) and the failure boundary live in the data-layer implementation —
//! this use case just drives the trait then composes the result.
//!
//! Never call [`UseCase::execute`] directly from a controller — always drive
//! it through `ControllerCore::run`/`run_into`.

use std::sync::Arc;

use clean_signals::UseCase;

use crate::failure::HuddleFailure;
use crate::features::activity::domain::models::ActivityRow;
use crate::features::activity::domain::repositories::ActivityRepository;

/// Loads the mentions feed via the injected [`ActivityRepository`].
pub struct LoadActivity {
    repo: Arc<dyn ActivityRepository + Send + Sync>,
}

impl LoadActivity {
    /// Construct with the injected repository.
    pub fn new(repo: Arc<dyn ActivityRepository + Send + Sync>) -> Self {
        Self { repo }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for LoadActivity {
    type Params = ();
    type Output = Vec<ActivityRow>;
    type Failure = HuddleFailure;

    async fn execute(&self, _params: ()) -> Result<Vec<ActivityRow>, HuddleFailure> {
        let items = self.repo.activity_items().await?;
        Ok(items
            .into_iter()
            .map(|item| ActivityRow { item, unread: true })
            .collect())
    }
}
