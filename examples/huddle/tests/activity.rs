//! Integration tests for the Activity tab (Phase C task 14).
//!
//! The screen (`src/screens/activity.rs`) is a private module — `src/lib.rs`
//! declares `mod screens;`, not `pub mod screens;` (see
//! `src/README-phase-c.md`'s hub-file contract) — so, mirroring
//! `tests/shell.rs`, these tests drive the whole mounted [`HuddleApp`] and
//! navigate to `/activity` through its real [`forgekit::NavigatorController`],
//! rather than constructing the screen's `Component` directly. Paint output is
//! asserted the same black-box way `tests/shell.rs` does (`RecScene`'s glyph
//! and rounded-rect counts — no GPU, no window).
//!
//! [`features::activity::ActivityController`]'s own constructor-injection
//! tests ("load renders items" at the data level, and the empty-feed shape)
//! live as unit tests inside `src/features/activity/mod.rs` itself (mirroring
//! `features::settings`'s `compose` tests) — that is the level at which item
//! injection is actually reachable from outside the crate; production always
//! seeds the controller from the real `mock::activity()` (see
//! `src/screens/activity.rs`'s `Component::init`), so an integration test
//! exercises exactly that real, non-empty feed.
//!
//! Row-tap navigation is the screen's own local in-tab drill-down (see
//! `src/screens/activity.rs`'s module docs) rather than a shared
//! `/channel/:id` route push — `routes.rs` passes this screen no
//! `NavigatorController` (a documented, deliberate simplification; see that
//! file). The tap-position geometry below is derived from `AppBarView`'s fixed
//! layout (`crates/forgekit-widgets/src/material/appbar.rs`: 64dp bar, 4px
//! edge inset, actions flush to the trailing edge) and `ListItem`'s
//! `THREE_LINE_HEIGHT` (88dp, since every loaded row calls `.three_line()`).

use std::time::{Duration, Instant};

use forgekit::AnyView;
use forgekit_core::RenderRoot;
use forgekit_text::TextContext;
use kurbo::Point;

use huddle::{HuddleApp, HuddleState};

mod support;
use support::{pump_frames, pump_until, serial, setup, tap};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

/// The mocked `LoadActivity` latency (`src/features/activity/mod.rs`'s
/// `LOAD_LATENCY_MS`) plus slack — how long these tests pump frames waiting
/// for the feed to resolve.
const LOAD_WAIT: Duration = Duration::from_millis(700);

/// The app bar's single "mark all read" action, per `AppBarView::layout`'s
/// fixed geometry (see the module docs).
const MARK_ALL_READ: Point = Point::new(784.0, 32.0);

/// The center of the first loaded feed row (every row is the `.three_line()`
/// 88dp variant, starting right below the 64dp app bar).
const FIRST_ROW: Point = Point::new(400.0, 108.0);

/// The detail view's back action (the leading slot, per the same app-bar
/// geometry as [`MARK_ALL_READ`]).
const BACK_ACTION: Point = Point::new(16.0, 32.0);

/// Navigate a fresh app straight to `/activity` and pump past the mocked load
/// latency, returning the root/state/logic/tcx plus the loaded scene.
fn mount_loaded_activity_tab() -> (
    Root,
    HuddleState,
    impl FnMut(&mut HuddleState) -> AnyView<HuddleState>,
    TextContext,
    support::RecScene,
) {
    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    let mut tcx = TextContext::new();

    // Build once at "/" before queuing the push, mirroring
    // `tests/shell.rs::routes_push_and_pop_without_panicking` — the
    // `NavigatorController`'s op queue is drained at the navigator's *next*
    // rebuild, so a push queued before the navigator widget exists has
    // nothing to drain into yet.
    root.rebuild(&mut logic, &mut state);
    state.nav.router().push("/activity");

    let loading_scene = support::frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(
        loading_scene.glyph_runs > 0,
        "the loading skeletons render text"
    );
    assert!(
        loading_scene.rounded.is_empty(),
        "a skeleton row has no avatar/chip chrome"
    );

    let deadline = Instant::now() + LOAD_WAIT;
    let loaded_scene = pump_until(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        loading_scene,
        || Instant::now() >= deadline,
    );

    (root, state, logic, tcx, loaded_scene)
}

/// The feed loads past its mocked latency and renders a row per mention —
/// every mention starts unread, so at least one avatar + "Unread" chip paints.
#[test]
fn activity_tab_loads_and_renders_items() {
    let _g = serial();
    let _ambient = setup();

    let (_root, _state, _logic, _tcx, loaded_scene) = mount_loaded_activity_tab();

    assert!(
        loaded_scene.glyph_runs > 0,
        "the loaded feed renders actor/action/excerpt text"
    );
    assert!(
        !loaded_scene.rounded.is_empty(),
        "at least one row's avatar + unread chip paint once the feed loads"
    );
}

/// Tapping the app bar's "mark all read" action clears every row's unread
/// chip — the painted rounded-rect count drops (only the avatar cards remain).
#[test]
fn mark_all_read_clears_the_unread_chips() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, loaded_scene) = mount_loaded_activity_tab();
    let before = loaded_scene.rounded.len();
    assert!(
        before > 0,
        "the freshly loaded feed has unread chips to clear"
    );

    tap(&mut root, &mut state, MARK_ALL_READ);
    // `mark_all_read` runs on the UI-thread local task queue (`spawn_local`);
    // pump a few frames so it drains and the rebuild observes the cleared
    // rows.
    pump_frames(&mut root, &mut logic, &mut state, &mut tcx, 10);
    let after = support::frame(&mut root, &mut logic, &mut state, &mut tcx);
    let after_count = after.rounded.len();

    assert!(
        after_count < before,
        "marking all read removes every unread chip's rounded-rect paint \
         (before: {before}, after: {after_count})"
    );
}

/// Tapping a row opens the local channel detail view (see
/// `src/screens/activity.rs`'s Navigation note): the detail rows carry no
/// avatar/chip chrome, so the painted rounded-rect count drops to zero: and
/// the back action returns to the feed list.
#[test]
fn tapping_a_row_opens_and_closes_the_channel_detail_view() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, loaded_scene) = mount_loaded_activity_tab();
    assert!(
        !loaded_scene.rounded.is_empty(),
        "the feed list paints avatar/chip chrome before the tap"
    );

    tap(&mut root, &mut state, FIRST_ROW);
    let detail_scene = support::frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(
        detail_scene.rounded.is_empty(),
        "the detail view's plain message rows have no avatar/chip chrome"
    );
    assert!(
        detail_scene.glyph_runs > 0,
        "the channel's messages render as text"
    );

    tap(&mut root, &mut state, BACK_ACTION);
    let back_scene = support::frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(
        !back_scene.rounded.is_empty(),
        "the back action returns to the feed list"
    );
}
