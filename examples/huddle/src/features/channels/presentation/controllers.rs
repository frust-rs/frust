//! [`ChannelsController`] — the Home tab's roster view model (huddle
//! clean-architecture refactor; moved verbatim apart from repo
//! threading from the former flat `features/channels/mod.rs`).
//!
//! Mirrors [`crate::features::settings`]'s clean-signals shape: it embeds a
//! [`ControllerCore`] by composition and owns the one signal the Home screen
//! renders — an [`AsyncState`] over a [`ChannelsData`] (the channel + DM lists,
//! each item carrying its unread count and archived/muted flags).
//!
//! - [`LoadChannels`] is an async [`UseCase`](clean_signals::UseCase) that loads
//!   the roster through the injected [`ChannelRepository`]; it **loads cleanly**
//!   (one attempt, never fails), so no retry vocabulary is exercised here.
//! - [`ArchiveChannel`]/[`MuteChannel`] are synchronous use cases (the
//!   `SetTheme` shape) that carry the requested flag change; the controller
//!   applies it to its own signal via [`set_archived_flag`]/[`set_muted_flag`]
//!   afterwards. The data layer stays immutable — every mutation lives in the
//!   controller's signal.
//! - Undo is the same state patch in reverse: the swipe's undo toast captures
//!   the [`RwSignal`] (cheap, `Send + Sync`) and calls
//!   [`set_archived_flag`]/[`set_muted_flag`] with the prior value.
//! - [`ChannelsController::create_channel`] is a plain synchronous method, not
//!   an async [`UseCase`](clean_signals::UseCase): the new row is a
//!   controller-level op with no data-layer counterpart to echo through a round
//!   trip, so a caller sees it in the roster on the very next rebuild with no
//!   `spawn_local` involved.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use clean_signals::{AsyncState, ControllerCore, NoParams, RunOptions, async_state_signal};
use frust::{RwSignal, Update};

use crate::failure::HuddleFailure;
use crate::features::channels::domain::models::{ChannelItem, ChannelsData, FlagParams};
use crate::features::channels::domain::repositories::ChannelRepository;
use crate::features::channels::domain::use_cases::{ArchiveChannel, LoadChannels, MuteChannel};

/// The roster type a mutation can reach inside the [`AsyncState`] — its live
/// value in `Data`/`Reloading`, or the stale value an `Error` still carries.
fn data_mut(state: &mut AsyncState<ChannelsData, HuddleFailure>) -> Option<&mut ChannelsData> {
    match state {
        AsyncState::Data(d) | AsyncState::Reloading(d) => Some(d),
        AsyncState::Error { stale: Some(d), .. } => Some(d),
        _ => None,
    }
}

/// Set (or restore) the archived flag on the row with `id`, in place. Used both
/// by [`ChannelsController::set_archived`] (after the use case) and by the undo
/// toast (which captures the `Send + Sync` [`RwSignal`] directly).
pub fn set_archived_flag(
    data: RwSignal<AsyncState<ChannelsData, HuddleFailure>>,
    id: &str,
    value: bool,
) {
    data.try_update(|state| {
        if let Some(d) = data_mut(state) {
            if let Some(c) = d.channels.iter_mut().find(|c| c.id == id) {
                c.archived = value;
            }
            if let Some(dm) = d.dms.iter_mut().find(|dm| dm.id == id) {
                dm.archived = value;
            }
        }
    });
}

/// Set (or restore) the muted flag on the row with `id`, in place.
pub fn set_muted_flag(
    data: RwSignal<AsyncState<ChannelsData, HuddleFailure>>,
    id: &str,
    value: bool,
) {
    data.try_update(|state| {
        if let Some(d) = data_mut(state) {
            if let Some(c) = d.channels.iter_mut().find(|c| c.id == id) {
                c.muted = value;
            }
            if let Some(dm) = d.dms.iter_mut().find(|dm| dm.id == id) {
                dm.muted = value;
            }
        }
    });
}

/// View model for the Home screen's roster.
pub struct ChannelsController {
    core: ControllerCore<HuddleFailure>,
    load_channels: LoadChannels,
    archive: ArchiveChannel,
    mute: MuteChannel,
    /// The roster through its load lifecycle (loading / data / reloading).
    pub data: RwSignal<AsyncState<ChannelsData, HuddleFailure>>,
    /// Mints a unique id for a channel created via [`Self::create_channel`],
    /// mirroring `MessagesController::next_id`/`mint_id`'s shape.
    next_id: AtomicU32,
}

impl ChannelsController {
    /// A fresh controller with an empty (`Loading`) roster signal, wired to the
    /// injected [`ChannelRepository`] (the composition root builds it once and
    /// hands it in via context — see `crate::HuddleApp`'s `init`).
    pub fn new(repo: Arc<dyn ChannelRepository + Send + Sync>) -> Self {
        Self {
            core: ControllerCore::new(),
            load_channels: LoadChannels::new(repo),
            archive: ArchiveChannel,
            mute: MuteChannel,
            data: async_state_signal(),
            next_id: AtomicU32::new(1),
        }
    }

    /// Load (or reload) the roster. `run_into` flips the signal to `Loading`
    /// (first load) or `Reloading` (pull-to-refresh, keeping the stale roster
    /// visible), then to `Data` on success.
    pub async fn load(&self) {
        let _ = self
            .core
            .run_into(
                &self.load_channels,
                NoParams,
                self.data,
                RunOptions::default(),
            )
            .await;
    }

    /// Archive (or restore) a row, routing through the [`ArchiveChannel`] use
    /// case then patching the signal.
    pub async fn set_archived(&self, id: String, value: bool) {
        if let Ok(flag) = self
            .core
            .run(
                &self.archive,
                FlagParams { id, value },
                RunOptions::default(),
            )
            .await
        {
            set_archived_flag(self.data, &flag.id, flag.value);
        }
    }

    /// Mute (or restore) a row, mirror of [`set_archived`](Self::set_archived).
    pub async fn set_muted(&self, id: String, value: bool) {
        if let Ok(flag) = self
            .core
            .run(&self.mute, FlagParams { id, value }, RunOptions::default())
            .await
        {
            set_muted_flag(self.data, &flag.id, flag.value);
        }
    }

    /// Append a new channel to the live roster — the Home screen's
    /// create-channel sheet. A controller-level op: the immutable data layer is
    /// never touched, so the row exists only for this controller's lifetime
    /// (matching every other roster mutation here). Synchronous and immediate
    /// (see the module docs' note on why this isn't an async
    /// [`UseCase`](clean_signals::UseCase) like [`ArchiveChannel`]/[`MuteChannel`]).
    /// A no-op (returns `None`) while the roster hasn't loaded yet (`Loading`/
    /// `Error` with no stale value) — there is no list to append to.
    pub fn create_channel(&self, name: String, private: bool) -> Option<String> {
        let id = format!("c-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let item = ChannelItem {
            id: id.clone(),
            name,
            private,
            preview: "No messages yet".to_string(),
            unread: 0,
            archived: false,
            muted: false,
        };
        let mut appended = false;
        self.data.try_update(|state| {
            if let Some(d) = data_mut(state) {
                d.channels.push(item);
                appended = true;
            }
        });
        appended.then_some(id)
    }
}

impl AsRef<ControllerCore<HuddleFailure>> for ChannelsController {
    fn as_ref(&self) -> &ControllerCore<HuddleFailure> {
        &self.core
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::channels::data::repositories::StoreChannelRepository;
    use frust::GetUntracked;

    /// The test-wiring composition root: the real store-backed repository behind
    /// the domain trait object. `#[cfg(test)]`-only wiring, so it is the sole
    /// place the presentation layer touches `data/` — exactly as the production
    /// composition root (`crate::HuddleApp::init`) is (production presentation
    /// imports no `data/`).
    fn store_repo() -> Arc<dyn ChannelRepository + Send + Sync> {
        Arc::new(StoreChannelRepository::new())
    }

    #[tokio::test]
    async fn load_populates_channels_and_dms_cleanly() {
        let controller = ChannelsController::new(store_repo());
        controller.load().await;

        let state = controller.data.get_untracked();
        let data = state.value().expect("loaded cleanly on the first attempt");
        assert_eq!(data.channels.len(), 6, "6 channels");
        assert_eq!(data.dms.len(), 9, "9 DMs (one an empty DM — task 22)");
        assert!(
            data.channels.iter().filter(|c| c.private).count() == 1,
            "exactly one private channel",
        );
        let any_unread =
            data.channels.iter().any(|c| c.unread > 0) || data.dms.iter().any(|d| d.unread > 0);
        assert!(any_unread, "at least one row is unread");

        controller.core.dispose();
    }

    #[tokio::test]
    async fn archive_sets_then_restores_the_flag() {
        let controller = ChannelsController::new(store_repo());
        controller.load().await;

        controller.set_archived("general".to_string(), true).await;
        assert!(
            channel_flag(&controller, "general").0,
            "archive sets the flag"
        );

        // Undo restores it.
        set_archived_flag(controller.data, "general", false);
        assert!(
            !channel_flag(&controller, "general").0,
            "restore clears the flag"
        );

        controller.core.dispose();
    }

    #[tokio::test]
    async fn mute_sets_then_restores_a_dm_flag() {
        let controller = ChannelsController::new(store_repo());
        controller.load().await;

        controller.set_muted("dm-2".to_string(), true).await;
        assert!(dm_muted(&controller, "dm-2"), "mute sets the DM flag");

        set_muted_flag(controller.data, "dm-2", false);
        assert!(!dm_muted(&controller, "dm-2"), "restore clears the DM flag");

        controller.core.dispose();
    }

    #[tokio::test]
    async fn create_channel_appends_a_live_row_immediately() {
        let controller = ChannelsController::new(store_repo());
        controller.load().await;

        let before = controller
            .data
            .get_untracked()
            .value()
            .expect("loaded")
            .channels
            .len();
        let id = controller
            .create_channel("launch-planning".to_string(), true)
            .expect("the roster is loaded, so the append succeeds");

        let state = controller.data.get_untracked();
        let data = state.value().expect("loaded");
        assert_eq!(data.channels.len(), before + 1, "the new row is appended");
        let created = data
            .channels
            .iter()
            .find(|c| c.id == id)
            .expect("the minted id is in the roster");
        assert_eq!(created.name, "launch-planning");
        assert!(created.private, "the private flag carries through");
        assert_eq!(created.unread, 0, "a brand-new channel starts read");

        controller.core.dispose();
    }

    #[test]
    fn create_channel_before_load_is_a_no_op() {
        let controller = ChannelsController::new(store_repo());
        // No `load().await` — the signal is still `Loading`, so there is no
        // roster to append to.
        assert_eq!(
            controller.create_channel("too-early".to_string(), false),
            None
        );
        controller.core.dispose();
    }

    #[tokio::test]
    async fn reload_keeps_the_roster_shape() {
        let controller = ChannelsController::new(store_repo());
        controller.load().await;
        // A second load (pull-to-refresh) re-runs cleanly and keeps the roster.
        controller.load().await;
        let state = controller.data.get_untracked();
        assert_eq!(state.value().expect("reloaded").channels.len(), 6);
        controller.core.dispose();
    }

    fn channel_flag(controller: &ChannelsController, id: &str) -> (bool, bool) {
        let state = controller.data.get_untracked();
        let d = state.value().expect("loaded");
        let c = d
            .channels
            .iter()
            .find(|c| c.id == id)
            .expect("channel exists");
        (c.archived, c.muted)
    }

    fn dm_muted(controller: &ChannelsController, id: &str) -> bool {
        let state = controller.data.get_untracked();
        let d = state.value().expect("loaded");
        d.dms
            .iter()
            .find(|dm| dm.id == id)
            .expect("dm exists")
            .muted
    }
}
