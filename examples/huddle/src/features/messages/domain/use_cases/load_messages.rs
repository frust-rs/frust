//! [`LoadMessages`] — the async message-load use case.
//!
//! Takes the [`MessageRepository`] as `Arc<dyn MessageRepository + Send + Sync>`
//! (the team-demo `LoadTeam` shape) and materializes the channel's
//! [`FeedMessage`]s (the newest `limit` of them). The deliberate mock latency
//! (so the feed's loading skeletons are visible) stays here in the use case
//! rather than the data layer, because it is per-controller and test-tunable
//! (`MessagesController::with_latency`) — see the module docs on
//! [`repositories`](super::super::repositories). Never fails today — it reuses
//! [`HuddleFailure`] only to satisfy the [`UseCase`] bound.
//!
//! Never call [`UseCase::execute`] directly from a controller — always drive it
//! through `ControllerCore::run`/`run_into`.

use std::sync::Arc;

use clean_signals::UseCase;

use crate::failure::HuddleFailure;
use crate::features::messages::domain::models::{FeedMessage, LoadMessagesParams};
use crate::features::messages::domain::repositories::MessageRepository;

/// The message-load use case: sleep the mock latency, then materialize the
/// channel's [`FeedMessage`]s (the newest `limit` of them) through the injected
/// [`MessageRepository`].
pub struct LoadMessages {
    repo: Arc<dyn MessageRepository + Send + Sync>,
}

impl LoadMessages {
    /// Construct with the injected repository.
    pub fn new(repo: Arc<dyn MessageRepository + Send + Sync>) -> Self {
        Self { repo }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for LoadMessages {
    type Params = LoadMessagesParams;
    type Output = Vec<FeedMessage>;
    type Failure = HuddleFailure;

    async fn execute(&self, params: LoadMessagesParams) -> Result<Vec<FeedMessage>, HuddleFailure> {
        clean_signals::time::sleep(params.latency).await;
        let all: Vec<FeedMessage> = self
            .repo
            .messages_for(&params.channel_id)
            .await?
            .into_iter()
            .map(FeedMessage::from_mock)
            .collect();
        let out = match params.limit {
            Some(n) if all.len() > n => all[all.len() - n..].to_vec(),
            _ => all,
        };
        Ok(out)
    }
}
