//! [`ActivityController`] — the mentions feed's async load + local read-state
//! view model (huddle clean-architecture refactor, task 04; moved verbatim
//! apart from repo threading from the former flat `features/activity/mod.rs`).
//!
//! Embeds a [`ControllerCore`] by composition (the clean-signals controller
//! idiom) and drives two use cases:
//!
//! - [`LoadActivity`] — the async mentions-feed loader (through the injected
//!   [`ActivityRepository`]), a mocked ~400ms round trip long enough that the
//!   screen's loading skeletons are actually visible before the feed
//!   resolves.
//! - [`MarkAllRead`] — a synchronous, infallible local transform (the
//!   "single-op" shape [`crate::features::settings::SetTheme`] establishes).
//!
//! Neither use case ever fails; both reuse [`HuddleFailure`] purely to
//! satisfy the trait bound.
//!
//! # Why a thread-local instance, not a `Component` (mirrors `features::search`)
//!
//! [`crate::features::activity::presentation::pages::activity`]'s module docs
//! spell out why the screen is a **plain** function rather than a
//! `Component`: `src/routes.rs`'s `/activity` entry (a frozen hub file, see
//! `src/README-phase-c.md`) gives the screen no `NavigatorController`, so a
//! row tap's `on_press` needs `&mut HuddleState` directly to reach
//! `state.nav.router().push(..)` — a `Component`'s inner event handlers only
//! ever see the component's own local `State`, never the outer `HuddleState`
//! (`docs/ARCHITECTURE.md`'s Component state boundary).
//! [`ActivityController::instance`] is the same `thread_local!`-backed
//! singleton [`crate::features::search::presentation::SearchController::instance`]
//! establishes: correctly scoped because this app's `RenderRoot`/reactive
//! runtime pump always runs on one thread (a real process only ever has one),
//! and each `#[test]` in `tests/activity.rs` gets its own fresh OS thread
//! (Rust's default test harness never reuses one for a later test), so the
//! thread-local never leaks across tests either.
//!
//! # Repository injection
//!
//! [`ActivityController::instance`] recovers the injected
//! `Arc<dyn ActivityRepository + Send + Sync>` via `use_context` the moment it
//! lazily constructs the controller (production's composition root,
//! `crate::HuddleApp::init`, publishes it once) — the same cleaner,
//! `cfg(test)`-free injection task 02 establishes for `ChannelsController`
//! (no external test crate constructs `ActivityController` directly — see the
//! completion summary). Internal unit tests below construct
//! [`ActivityController::new`] directly with a `#[cfg(test)]`-only
//! `FakeActivityRepository` instead (see that module's docs).

use std::cell::RefCell;
use std::sync::Arc;

use clean_signals::async_state::{AsyncState, async_state_signal};
use clean_signals::{ControllerCore, RunOptions};
use frust::{GetUntracked, RwSignal, use_context};

use crate::failure::HuddleFailure;
use crate::features::activity::domain::models::ActivityRow;
use crate::features::activity::domain::repositories::ActivityRepository;
use crate::features::activity::domain::use_cases::{LoadActivity, MarkAllRead};

/// View model for the Activity screen.
///
/// Holds the feed's async load state ([`Self::rows`]) and drives
/// [`LoadActivity`]/[`MarkAllRead`] through the embedded [`ControllerCore`]
/// (the `templates/AGENTS.md` controller rule).
pub struct ActivityController {
    core: ControllerCore<HuddleFailure>,
    load: LoadActivity,
    mark_all_read_uc: MarkAllRead,
    /// The feed's async load state — `Loading` drives the screen's loading
    /// skeletons, `Data(rows)` (empty or not) drives the list/empty-state
    /// split.
    pub rows: RwSignal<AsyncState<Vec<ActivityRow>, HuddleFailure>>,
}

impl ActivityController {
    /// A fresh controller wired to the injected [`ActivityRepository`] (the
    /// composition root builds it once and hands it in via context — see
    /// `crate::HuddleApp`'s `init`).
    pub fn new(repo: Arc<dyn ActivityRepository + Send + Sync>) -> Self {
        Self {
            core: ControllerCore::new(),
            load: LoadActivity::new(repo),
            mark_all_read_uc: MarkAllRead,
            rows: async_state_signal(),
        }
    }

    /// Runs [`LoadActivity`] into [`Self::rows`]. Call once from the hosting
    /// component's `init` via `spawn_local` (the
    /// `SettingsController::apply`/`spawn_apply` pattern
    /// [`crate::screens::settings_appearance`] establishes).
    pub async fn load(&self) {
        let _ = self
            .core
            .run_into(&self.load, (), self.rows, RunOptions::default())
            .await;
    }

    /// Runs [`MarkAllRead`] over the currently-loaded rows. A no-op if the
    /// feed hasn't resolved yet, or resolved empty (nothing to mark).
    pub async fn mark_all_read(&self) {
        let current = match self.rows.get_untracked() {
            AsyncState::Data(rows) | AsyncState::Reloading(rows) => rows,
            AsyncState::Loading | AsyncState::Error { .. } => return,
        };
        if current.is_empty() {
            return;
        }
        let _ = self
            .core
            .run_into(
                &self.mark_all_read_uc,
                current,
                self.rows,
                RunOptions::default(),
            )
            .await;
    }

    /// The single, lazily-created controller instance for this thread —
    /// see the [module docs](self)'s "Why a thread-local instance" section.
    /// Resolves the injected [`ActivityRepository`] via `use_context` (see
    /// the module docs' "Repository injection" section) and kicks off
    /// [`Self::load`] once, the moment the instance is first created —
    /// mirroring `screens::activity`'s former `Component::init`, which
    /// spawned the same load exactly once per mount.
    pub fn instance() -> Arc<Self> {
        thread_local! {
            static INSTANCE: RefCell<Option<Arc<ActivityController>>> =
                const { RefCell::new(None) };
        }
        INSTANCE.with(|cell| {
            // Self-heal (task 22 hardening): a cached controller whose `rows`
            // signal was disposed by a prior owner (reused test thread) returns
            // `None` from `try_get_untracked` and is rebuilt (load re-kicked)
            // rather than handed back to panic on the next read — the same guard
            // `features::messages`'s registry and the screen composer caches carry.
            if let Some(existing) = cell.borrow().as_ref().cloned()
                && existing.rows.try_get_untracked().is_some()
            {
                return existing;
            }
            let repo = use_context::<Arc<dyn ActivityRepository + Send + Sync>>()
                .expect("the composition root provides an ActivityRepository");
            let controller = Arc::new(ActivityController::new(repo));
            {
                let handle = Arc::clone(&controller);
                frust::spawn_local(async move {
                    handle.load().await;
                });
            }
            *cell.borrow_mut() = Some(Arc::clone(&controller));
            controller
        })
    }
}

impl AsRef<ControllerCore<HuddleFailure>> for ActivityController {
    fn as_ref(&self) -> &ControllerCore<HuddleFailure> {
        &self.core
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::activity::domain::entities::ActivityItem;
    use crate::features::activity::domain::use_cases::compose_mark_all_read;
    use crate::features::channels::domain::Channel;
    use crate::features::profile::domain::User;

    /// A test-only fake `ActivityRepository` seeded with a fixed item list —
    /// the `Fake<Trait>` naming convention `docs/CODE_STANDARDS.md`
    /// prescribes for test-only fakes (`FakeProcessRunner`/`FakeEnv`). Lets
    /// the moved unit tests below keep seeding their own item list (including
    /// an empty one, to exercise the empty-feed shape) exactly as the
    /// pre-refactor `ActivityController::new(items)` did — a real
    /// `StoreActivityRepository` can't do that (its `activity_items()` always
    /// returns the real, non-empty, mock-derived feed).
    struct FakeActivityRepository {
        items: Vec<ActivityItem>,
    }

    #[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
    #[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
    impl ActivityRepository for FakeActivityRepository {
        async fn activity_items(&self) -> Result<Vec<ActivityItem>, HuddleFailure> {
            Ok(self.items.clone())
        }

        fn user(&self, _id: u32) -> Option<User> {
            None
        }

        fn channel(&self, _id: &str) -> Option<Channel> {
            None
        }
    }

    fn repo(items: Vec<ActivityItem>) -> Arc<dyn ActivityRepository + Send + Sync> {
        Arc::new(FakeActivityRepository { items })
    }

    fn item(message_id: u32) -> ActivityItem {
        ActivityItem {
            message_id,
            channel_id: "general",
            author_id: 2,
            text: "@Ada Lovelace can you review the release notes?",
        }
    }

    #[test]
    fn mark_all_read_clears_every_row() {
        let rows = vec![
            ActivityRow {
                item: item(1),
                unread: true,
            },
            ActivityRow {
                item: item(2),
                unread: true,
            },
        ];
        let cleared = compose_mark_all_read(rows);
        assert_eq!(cleared.len(), 2);
        assert!(
            cleared.iter().all(|r| !r.unread),
            "every row is marked read"
        );
    }

    #[test]
    fn mark_all_read_is_a_no_op_on_an_empty_feed() {
        assert_eq!(compose_mark_all_read(Vec::new()), Vec::new());
    }

    #[tokio::test]
    async fn load_populates_rows_all_unread() {
        let controller = ActivityController::new(repo(vec![item(1), item(2)]));
        controller.load().await;
        match controller.rows.get_untracked() {
            AsyncState::Data(rows) => {
                assert_eq!(rows.len(), 2, "both seeded items load");
                assert!(
                    rows.iter().all(|r| r.unread),
                    "freshly loaded rows are unread"
                );
            }
            other => panic!("expected AsyncState::Data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn load_with_no_items_yields_empty_data_not_loading() {
        let controller = ActivityController::new(repo(Vec::new()));
        controller.load().await;
        match controller.rows.get_untracked() {
            AsyncState::Data(rows) => {
                assert!(rows.is_empty(), "an empty seed loads to an empty feed")
            }
            other => panic!("expected AsyncState::Data(empty), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mark_all_read_flips_the_loaded_rows_unread_flag() {
        let controller = ActivityController::new(repo(vec![item(1), item(2)]));
        controller.load().await;
        controller.mark_all_read().await;
        match controller.rows.get_untracked() {
            AsyncState::Data(rows) => {
                assert_eq!(rows.len(), 2);
                assert!(
                    rows.iter().all(|r| !r.unread),
                    "mark_all_read clears every row"
                );
            }
            other => panic!("expected AsyncState::Data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mark_all_read_before_load_is_a_no_op() {
        let controller = ActivityController::new(repo(vec![item(1)]));
        controller.mark_all_read().await; // feed is still Loading
        assert!(
            matches!(controller.rows.get_untracked(), AsyncState::Loading),
            "mark_all_read before the feed resolves leaves it untouched"
        );
    }
}
