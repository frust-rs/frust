//! Activity feature domain — the mentions feed's async load + local
//! read-state controller (see [`crate::screens::activity`]).
//!
//! [`ActivityController`] embeds a [`ControllerCore`] by composition (the
//! clean-signals controller idiom) and drives two use cases:
//!
//! - [`LoadActivity`] — a mocked ~400ms round trip populating the feed, long
//!   enough that the screen's loading skeletons are actually visible before
//!   the feed resolves.
//! - [`MarkAllRead`] — a synchronous, infallible local transform (the
//!   "single-op" shape [`crate::features::settings::SetTheme`] establishes:
//!   a pure composition fn plus a thin `UseCase` wrapper, unit-tested
//!   independently of the controller).
//!
//! Neither use case ever fails; both reuse [`HuddleFailure`] purely to satisfy
//! the trait bound, mirroring `SetTheme`'s documented infallible shape.
//!
//! [`LoadActivity`]'s seed items are supplied at [`ActivityController::new`]
//! construction time rather than the use case calling
//! [`crate::mock::activity`] itself — production seeds it from that accessor
//! (see [`ActivityController::instance`]), a test seeds its own list
//! (including an empty one, to exercise the empty-feed shape without a screen
//! mount) — the same seeding convention `SettingsController::new` uses for
//! its selectors.
//!
//! # Why a thread-local instance, not a `Component` (mirrors `features::search`)
//!
//! [`crate::screens::activity`]'s module docs spell out why the screen is a
//! **plain** function rather than a `Component`: `src/routes.rs`'s `/activity`
//! entry (a frozen hub file, see `src/README-phase-c.md`) gives the screen no
//! `NavigatorController`, so a row tap's `on_press` needs `&mut HuddleState`
//! directly to reach `state.nav.router().push(..)` — a `Component`'s inner
//! event handlers only ever see the component's own local `State`, never the
//! outer `HuddleState` (`docs/ARCHITECTURE.md`'s Component state boundary).
//! [`ActivityController::instance`] is the same `thread_local!`-backed
//! singleton [`crate::features::search::SearchController::instance`]
//! establishes: correctly scoped because this app's `RenderRoot`/reactive
//! runtime pump always runs on one thread (a real process only ever has one),
//! and each `#[test]` in `tests/activity.rs` gets its own fresh OS thread
//! (Rust's default test harness never reuses one for a later test), so the
//! thread-local never leaks across tests either.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use clean_signals::async_state::{AsyncState, async_state_signal};
use clean_signals::{ControllerCore, RunOptions, UseCase};
use forgekit::{GetUntracked, RwSignal};

use crate::failure::HuddleFailure;
use crate::mock::{self, ActivityItem};

/// Mock load latency ("~400ms → skeletons" per the task spec) — long enough
/// that the screen's loading skeletons are actually visible before the feed
/// resolves.
const LOAD_LATENCY_MS: u64 = 400;

/// One activity-feed row: the underlying mention plus its locally-tracked
/// read/unread flag. `mock::ActivityItem` carries no read/unread field of its
/// own (`src/mock/**` is a frozen hub file — see `src/README-phase-c.md`), so
/// every freshly loaded item starts unread until [`MarkAllRead`] clears it.
#[derive(Clone, Debug)]
pub struct ActivityRow {
    /// The underlying mention.
    pub item: ActivityItem,
    /// Whether this row is still unread.
    pub unread: bool,
}

/// Id-based equality: `mock::ActivityItem` (a frozen hub type) derives no
/// `PartialEq`, and mock rows are static data keyed by `message_id`, so
/// message-id + unread-flag equality is exact row equality.
impl PartialEq for ActivityRow {
    fn eq(&self, other: &Self) -> bool {
        self.item.message_id == other.item.message_id && self.unread == other.unread
    }
}

/// Loads the activity feed after a mocked network delay. `items` is supplied
/// at construction (see the module docs' seeding note).
pub struct LoadActivity {
    items: Vec<ActivityItem>,
}

impl LoadActivity {
    /// Seed the use case with `items` (the exact list [`ActivityController`]
    /// hands it — see the module docs).
    pub fn new(items: Vec<ActivityItem>) -> Self {
        Self { items }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for LoadActivity {
    type Params = ();
    type Output = Vec<ActivityRow>;
    type Failure = HuddleFailure;

    async fn execute(&self, _params: ()) -> Result<Vec<ActivityRow>, HuddleFailure> {
        clean_signals::sleep(Duration::from_millis(LOAD_LATENCY_MS)).await;
        Ok(self
            .items
            .iter()
            .copied()
            .map(|item| ActivityRow { item, unread: true })
            .collect())
    }
}

/// Pure composition: every row marked read. Unit-tested in isolation, mirroring
/// [`crate::features::settings::compose`].
pub fn compose_mark_all_read(rows: Vec<ActivityRow>) -> Vec<ActivityRow> {
    rows.into_iter()
        .map(|row| ActivityRow {
            unread: false,
            ..row
        })
        .collect()
}

/// The synchronous "mark all read" op (top-bar action) — the single place the
/// feed's unread flags are cleared, mirroring
/// [`crate::features::settings::SetTheme`]'s synchronous, infallible,
/// single-call-site shape (composition function + thin `UseCase` wrapper).
pub struct MarkAllRead;

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for MarkAllRead {
    type Params = Vec<ActivityRow>;
    type Output = Vec<ActivityRow>;
    type Failure = HuddleFailure;

    async fn execute(&self, rows: Vec<ActivityRow>) -> Result<Vec<ActivityRow>, HuddleFailure> {
        Ok(compose_mark_all_read(rows))
    }
}

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
    /// `items` seeds [`LoadActivity`] — see the module docs' seeding note.
    pub fn new(items: Vec<ActivityItem>) -> Self {
        Self {
            core: ControllerCore::new(),
            load: LoadActivity::new(items),
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
    /// Seeds from the real [`crate::mock::activity`] (production's seed —
    /// see the module docs' seeding note) and kicks off [`Self::load`] once,
    /// the moment the instance is first created — mirroring
    /// `screens::activity`'s former `Component::init`, which spawned the
    /// same load exactly once per mount.
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
            let controller = Arc::new(ActivityController::new(mock::activity()));
            {
                let handle = Arc::clone(&controller);
                forgekit::spawn_local(async move {
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
        let controller = ActivityController::new(vec![item(1), item(2)]);
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
        let controller = ActivityController::new(Vec::new());
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
        let controller = ActivityController::new(vec![item(1), item(2)]);
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
        let controller = ActivityController::new(vec![item(1)]);
        controller.mark_all_read().await; // feed is still Loading
        assert!(
            matches!(controller.rows.get_untracked(), AsyncState::Loading),
            "mark_all_read before the feed resolves leaves it untouched"
        );
    }
}
