//! Headless integration tests for the huddle screen (task 02's five gates).
//!
//! There is no GPU window: each test drives ForgeKit's [`RenderRoot`] directly
//! (the desktop shell's own rebuild/layout/paint seam) under an ambient reactive
//! [`Owner`] with the [`ReactiveRuntime`] installed, and pumps the UI-thread
//! local task queue frame by frame — the `examples/inbox` /
//! `clean-signals-forgekit` recipe (see `tests/support`). Text is shaped through
//! a real `TextContext` (CPU-only), so every assertion is deterministic and this
//! doubles as the permanent regression gate for the team-roster slice.
//!
//! Test seams (documented per the task): the async controller actions are driven
//! at the *controller seam* (`spawn_local(ctrl.rename(..))` / `.load()`) rather
//! than by synthesizing per-row `TextInput` key/IME events — deterministic, and
//! still exercising the full controller → signal → tracked-rebuild → paint path
//! plus the failure-listener → banner path. The banner Dismiss (test 5) is the
//! one exception: it is driven by a real synthetic pointer tap on the button, to
//! prove the wired button clears the banner signal end to end.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use clean_signals::AsyncState;
use forgekit::{ComponentView, GetUntracked, Set, component, spawn_local};
use forgekit_core::{InputEvent, PointerButton, PointerEvent, PointerPhase, RenderRoot};
use forgekit_text::TextContext;
use kurbo::{Point, Size};

use huddle::failure::TeamFailure;
use huddle::features::team::data::InMemoryTeamRepo;
use huddle::features::team::domain::entities::Member;
use huddle::features::team::domain::repositories::TeamRepository;
use huddle::{TeamController, TeamScreen, TeamSpy};

mod support;
use support::{RecScene, frame, pump_until, setup};

// ---------------------------------------------------------------------------
// Repos.
// ---------------------------------------------------------------------------

/// A repository that counts `list_members` attempts and fails the first
/// `fail_times` of them with a retryable `Network` failure — proves the
/// controller's `RetryPolicy` absorbs exactly that many failures (the twin's
/// `FlakyRepo`, exposed here so a test can read the attempt count).
struct CountingRepo {
    fail_times: u32,
    attempts: Arc<AtomicU32>,
    members: Vec<Member>,
}

fn member(id: &str, name: &str) -> Member {
    Member {
        id: id.to_string(),
        name: name.to_string(),
        role: "Mobile Engineer".to_string(),
        email: format!("{id}@team.dev"),
    }
}

impl CountingRepo {
    fn new(fail_times: u32) -> (Arc<Self>, Arc<AtomicU32>) {
        let attempts = Arc::new(AtomicU32::new(0));
        let repo = Arc::new(Self {
            fail_times,
            attempts: Arc::clone(&attempts),
            members: vec![
                member("u1", "Ava Chen"),
                member("u2", "Bruno Costa"),
                member("u3", "Chidi Okafor"),
            ],
        });
        (repo, attempts)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl TeamRepository for CountingRepo {
    async fn list_members(&self) -> Result<Vec<Member>, TeamFailure> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt <= self.fail_times {
            Err(TeamFailure::Network("temporary upstream error".to_string()))
        } else {
            Ok(self.members.clone())
        }
    }

    async fn update_member_name(&self, id: String, name: String) -> Result<Member, TeamFailure> {
        Ok(member(&id, &name))
    }
}

/// The demo's own in-memory repo, zero latency / zero seeded failures — the full
/// 6-member seed (incl. `u2` Bruno Costa) the filter/rename tests exercise.
fn instant_repo() -> Arc<dyn TeamRepository + Send + Sync> {
    Arc::new(InMemoryTeamRepo::new(
        std::time::Duration::from_millis(0),
        0,
    ))
}

// ---------------------------------------------------------------------------
// Mount helpers.
// ---------------------------------------------------------------------------

type Root = RenderRoot<(), ComponentView<TeamScreen>>;

/// Reads the controller + banner signal the mounted screen published to `spy`.
fn handles(spy: &TeamSpy) -> (Arc<TeamController>, forgekit::RwSignal<Option<String>>) {
    let guard = spy.lock().unwrap_or_else(|e| e.into_inner());
    let h = guard
        .as_ref()
        .expect("init published its handles to the spy");
    (Arc::clone(&h.controller), h.banner)
}

fn member_names(ctrl: &TeamController) -> Vec<String> {
    ctrl.members
        .get_untracked()
        .value()
        .map(|v| v.iter().map(|m| m.name.clone()).collect())
        .unwrap_or_default()
}

fn pointer(phase: PointerPhase, p: Point) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase,
        position: p,
        button: PointerButton::Primary,
    })
}

fn tap(root: &mut Root, state: &mut (), p: Point) {
    root.event(state, &pointer(PointerPhase::Down, p));
    root.event(state, &pointer(PointerPhase::Up, p));
}

fn center((origin, size): (Point, Size)) -> Point {
    Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
}

// ===========================================================================
// 1. Retry load: Loading → Data, retry absorbed the seeded failure.
// ===========================================================================

#[test]
fn retry_load_transitions_to_data_after_absorbing_a_failure() {
    let _ambient = setup();

    let (repo, attempts) = CountingRepo::new(1);
    let spy: TeamSpy = Arc::new(Mutex::new(None));
    let spy_for_logic = spy.clone();
    let repo_dyn: Arc<dyn TeamRepository + Send + Sync> = repo;
    let mut logic = move |_: &mut ()| {
        component(TeamScreen::with_spy(
            Arc::clone(&repo_dyn),
            spy_for_logic.clone(),
        ))
    };
    let mut root: Root = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    // First frame: `init` runs (spawns the load); the signal is still Loading.
    let loading = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let (ctrl, _banner) = handles(&spy);
    assert!(
        matches!(ctrl.members.get_untracked(), AsyncState::Loading),
        "starts Loading before any frame is pumped"
    );

    // Pump until the load lands as Data (inherently exercises the retry: the
    // first attempt fails, so Data only appears after the retry succeeds).
    let ctrl_probe = Arc::clone(&ctrl);
    let data = pump_until(&mut root, &mut logic, &mut state, &mut tcx, loading, || {
        matches!(ctrl_probe.members.get_untracked(), AsyncState::Data(_))
    });

    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "attempt 1 failed (retryable), the RetryPolicy retried, attempt 2 succeeded"
    );
    assert_eq!(
        member_names(&ctrl),
        vec!["Ava Chen", "Bruno Costa", "Chidi Okafor"],
        "the seeded member names loaded"
    );
    assert!(
        data.glyph_runs > 3,
        "the Data arm paints the member rows (more glyph runs than the loading placeholder)"
    );
}

// ===========================================================================
// 2. Rename: painted row updates and `members` is patched in place.
// ===========================================================================

#[test]
fn rename_updates_the_row_and_patches_members_in_place() {
    let _ambient = setup();

    let spy: TeamSpy = Arc::new(Mutex::new(None));
    let spy_for_logic = spy.clone();
    let repo = instant_repo();
    let mut logic = move |_: &mut ()| {
        component(TeamScreen::with_spy(
            Arc::clone(&repo),
            spy_for_logic.clone(),
        ))
    };
    let mut root: Root = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    let first = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let (ctrl, _banner) = handles(&spy);
    let ctrl_probe = Arc::clone(&ctrl);
    pump_until(&mut root, &mut logic, &mut state, &mut tcx, first, || {
        matches!(ctrl_probe.members.get_untracked(), AsyncState::Data(_))
    });

    // Snapshot the loaded vector length so we can prove the patch is in place
    // (same length, one element mutated — not a reload).
    let before_len = ctrl.members.get_untracked().value().unwrap().len();
    assert_eq!(before_len, 6);

    // Drive the rename at the controller seam.
    let handle = Arc::clone(&ctrl);
    spawn_local(async move {
        handle
            .rename("u2".to_string(), "New Name".to_string())
            .await;
    });
    let ctrl_probe = Arc::clone(&ctrl);
    let after = pump_until(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        RecScene::default(),
        || {
            ctrl_probe
                .members
                .get_untracked()
                .value()
                .and_then(|v| v.iter().find(|m| m.id == "u2"))
                .is_some_and(|m| m.name == "New Name")
        },
    );

    let members = ctrl.members.get_untracked();
    let all = members.value().unwrap();
    assert_eq!(all.len(), before_len, "patched in place, not reloaded");
    assert_eq!(
        all.iter().find(|m| m.id == "u2").unwrap().name,
        "New Name",
        "the u2 member was renamed in the loaded list"
    );
    assert!(
        after.glyph_runs > 3,
        "the updated data arm still paints the member rows"
    );
}

// ===========================================================================
// 3. Validation failure: a too-short rename surfaces on the banner; members
//    unchanged.
// ===========================================================================

#[test]
fn too_short_rename_surfaces_the_validation_banner_and_leaves_members_unchanged() {
    let _ambient = setup();

    let spy: TeamSpy = Arc::new(Mutex::new(None));
    let spy_for_logic = spy.clone();
    let repo = instant_repo();
    let mut logic = move |_: &mut ()| {
        component(TeamScreen::with_spy(
            Arc::clone(&repo),
            spy_for_logic.clone(),
        ))
    };
    let mut root: Root = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    let first = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let (ctrl, banner) = handles(&spy);
    let ctrl_probe = Arc::clone(&ctrl);
    pump_until(&mut root, &mut logic, &mut state, &mut tcx, first, || {
        matches!(ctrl_probe.members.get_untracked(), AsyncState::Data(_))
    });
    let before = member_names(&ctrl);

    // A 1-char rename → domain Validation failure, routed to the failure sink →
    // the screen's failure listener sets the banner signal.
    let handle = Arc::clone(&ctrl);
    spawn_local(async move {
        handle.rename("u1".to_string(), "A".to_string()).await;
    });
    pump_until(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        RecScene::default(),
        || banner.get_untracked().is_some(),
    );

    assert_eq!(
        banner.get_untracked(),
        Some("Name must be at least 2 characters.".to_string()),
        "the Validation user_message is shown in the banner"
    );
    assert_eq!(
        member_names(&ctrl),
        before,
        "a rejected rename leaves the loaded members unchanged"
    );
}

// ===========================================================================
// 4. Filter: a query narrows the painted rows; clearing restores them.
// ===========================================================================

#[test]
fn query_narrows_the_painted_rows_and_clearing_restores_them() {
    let _ambient = setup();

    let spy: TeamSpy = Arc::new(Mutex::new(None));
    let spy_for_logic = spy.clone();
    let repo = instant_repo();
    let mut logic = move |_: &mut ()| {
        component(TeamScreen::with_spy(
            Arc::clone(&repo),
            spy_for_logic.clone(),
        ))
    };
    let mut root: Root = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    let first = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let (ctrl, _banner) = handles(&spy);
    let ctrl_probe = Arc::clone(&ctrl);
    let unfiltered = pump_until(&mut root, &mut logic, &mut state, &mut tcx, first, || {
        matches!(ctrl_probe.members.get_untracked(), AsyncState::Data(_))
    });
    assert_eq!(ctrl.filtered.get_untracked().len(), 6, "all members shown");

    // Set the query the `filtered` memo reads (what the search field's on_change
    // does) and re-render.
    ctrl.query.set("bruno".to_string());
    let filtered = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(
        ctrl.filtered.get_untracked().len(),
        1,
        "only Bruno Costa matches"
    );
    assert!(
        filtered.glyph_runs < unfiltered.glyph_runs,
        "fewer rows painted when filtered ({} < {})",
        filtered.glyph_runs,
        unfiltered.glyph_runs
    );

    // Clear the query → all rows return.
    ctrl.query.set(String::new());
    let cleared = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(ctrl.filtered.get_untracked().len(), 6, "all members again");
    assert_eq!(
        cleared.glyph_runs, unfiltered.glyph_runs,
        "clearing the query restores every row"
    );
}

// ===========================================================================
// 5. Banner dismiss: tapping Dismiss (after a validation failure) clears it.
// ===========================================================================

#[test]
fn dismiss_button_clears_the_banner() {
    let _ambient = setup();

    let spy: TeamSpy = Arc::new(Mutex::new(None));
    let spy_for_logic = spy.clone();
    let repo = instant_repo();
    let mut logic = move |_: &mut ()| {
        component(TeamScreen::with_spy(
            Arc::clone(&repo),
            spy_for_logic.clone(),
        ))
    };
    let mut root: Root = RenderRoot::new();
    let mut state = ();
    let mut tcx = TextContext::new();

    let first = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let (ctrl, banner) = handles(&spy);
    let ctrl_probe = Arc::clone(&ctrl);
    pump_until(&mut root, &mut logic, &mut state, &mut tcx, first, || {
        matches!(ctrl_probe.members.get_untracked(), AsyncState::Data(_))
    });

    // Raise the banner (as in test 3), keeping the scene that paints it.
    let handle = Arc::clone(&ctrl);
    spawn_local(async move {
        handle.rename("u1".to_string(), "A".to_string()).await;
    });
    let with_banner = pump_until(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        RecScene::default(),
        || banner.get_untracked().is_some(),
    );
    assert!(banner.get_untracked().is_some(), "banner is raised");

    // The Dismiss button is the topmost rounded-rect chrome (the banner row
    // sits above the search field and the list). Tap its center.
    let dismiss = with_banner
        .rounded
        .iter()
        .copied()
        .min_by(|a, b| a.0.y.partial_cmp(&b.0.y).unwrap())
        .expect("a rounded rect for the Dismiss button");
    tap(&mut root, &mut state, center(dismiss));

    // The button handler cleared the banner signal; the next frame drops the
    // banner arm.
    let after = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(banner.get_untracked(), None, "Dismiss cleared the banner");
    assert!(
        after.rounded.len() < with_banner.rounded.len(),
        "the banner (and its Dismiss button) is gone from the painted scene"
    );
}
