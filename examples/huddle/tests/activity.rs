//! Integration tests for the Activity tab (Phase C task 14).
//!
//! The screen (`src/screens/activity.rs`) is a private module — `src/lib.rs`
//! declares `mod screens;`, not `pub mod screens;` (see
//! `src/README-phase-c.md`'s hub-file contract) — so, mirroring
//! `tests/shell.rs`, these tests drive the whole mounted [`HuddleApp`] and
//! navigate to `/activity` through its real [`forgekit::NavigatorController`],
//! rather than constructing the screen's `Component` directly. Paint output is
//! asserted the same black-box way `tests/shell.rs` does (`RecScene`'s glyph
//! and rounded-rect geometry — no GPU, no window).
//!
//! [`features::activity::ActivityController`]'s own constructor-injection
//! tests ("load renders items" at the data level, and the empty-feed shape)
//! live as unit tests inside `src/features/activity/mod.rs` itself — that is
//! the level at which item injection is reachable from outside the crate;
//! production always seeds the controller from the real `mock::activity()`
//! (see `src/screens/activity.rs`'s `Component::init`), so an integration test
//! exercises exactly that real, non-empty feed.
//!
//! # Settling the boot/push cross-fade before driving input
//!
//! The shell's navigator runs an `M3FadeThrough` cross-fade at boot and again
//! when `/activity` is pushed. The shared [`support::frame`] helper paints at
//! [`forgekit_core::FrameTime::ZERO`], which freezes that transition forever —
//! and while a transition is in flight the navigator suppresses ALL page
//! input, so an app-bar or row tap dispatched mid-transition never reaches the
//! Activity screen. [`mount_loaded_activity_tab`] therefore advances the paint
//! clock (via the local [`frame_at`]) as it pumps the loader, exactly like
//! `tests/home.rs`/`tests/search.rs`'s `settle_transitions`: by the time the
//! feed resolves past its mocked latency the entrance cross-fade has long
//! settled, the leaving Home page is torn down, and page input flows again.
//!
//! # Geometry
//!
//! Row taps are located from the paint recording (the first loaded row's
//! leading avatar circle, mirroring `tests/home.rs::first_row_center`) rather
//! than a hand-computed offset. The app-bar actions are icons that paint only
//! glyph runs (no locatable rect), so [`MARK_ALL_READ`] and [`BACK_ACTION`]
//! are derived from `AppBarView::layout`'s fixed geometry
//! (`crates/forgekit-widgets/src/material/appbar.rs`: 64dp bar, 4px edge
//! inset, a 24dp icon flush to the leading/trailing edge → a center 16px in
//! from that edge, at the bar's 32px vertical middle).

use std::any::Any;
use std::time::{Duration, Instant};

use forgekit::{AnyView, Component};
use forgekit_core::{FrameTime, RenderRoot};
use forgekit_reactive::ReactiveRuntime;
use forgekit_text::TextContext;
use kurbo::{Point, Size};

use huddle::{HuddleApp, HuddleState};

mod support;
use support::{RecScene, W, serial, setup, tap};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

/// How long [`mount_loaded_activity_tab`] pumps frames waiting for the feed to
/// resolve past its mocked `LOAD_LATENCY_MS` (~400ms) — the same 5s ceiling the
/// shared `support::pump_until` harness uses.
const LOAD_WAIT: Duration = Duration::from_secs(5);

/// The app bar's single "mark all read" action: a 24dp icon flush to the
/// trailing edge, centered 16px in from the 800px-wide bar's right edge, at the
/// 64dp bar's vertical middle (see the module docs' Geometry note).
const MARK_ALL_READ: Point = Point::new(W - 16.0, 32.0);

/// The detail view's back action: a 24dp icon flush to the leading edge,
/// centered 16px in from the left, at the bar's vertical middle.
const BACK_ACTION: Point = Point::new(16.0, 32.0);

/// The lower edge of the scrollable content region — above the 64dp bottom
/// navigation bar (which sits in `y ∈ [536, 600]` of the 600px window and
/// paints its own selection-indicator pill). Rounded rects below this are
/// persistent chrome, not feed rows.
const CONTENT_BOTTOM: f64 = 520.0;
/// The top of the scrollable content region — below the 64dp app bar.
const CONTENT_TOP: f64 = 64.0;

/// Count the loaded feed's per-row "Unread" indicator chips: the
/// `filter_chip`s the screen paints flush to each row's trailing edge
/// (`x ≈ 705`, well past the leading avatars at `x = 16`), within the
/// scrollable content region. Zero while the feed is loading (skeletons paint
/// no trailing chip) and zero once "mark all read" clears them — the direct
/// observable for both the load and the mark-all-read behaviors.
fn unread_chips(scene: &RecScene) -> usize {
    scene
        .rounded
        .iter()
        .filter(|(o, _)| o.x > 600.0 && o.y >= CONTENT_TOP && o.y < CONTENT_BOTTOM)
        .count()
}

/// Count the loaded feed's leading row avatars (`filled_card`s at the list's
/// leading edge, `x = 16`) within the content region. The feed list paints
/// one per row; the in-tab detail view's plain message rows paint none.
fn row_avatars(scene: &RecScene) -> usize {
    scene
        .rounded
        .iter()
        .filter(|(o, _)| o.x < 100.0 && o.y >= CONTENT_TOP && o.y < CONTENT_BOTTOM)
        .count()
}

/// The center of the first loaded feed row, derived from its leading avatar
/// circle (the top-most content-region rounded rect at the leading edge) —
/// mirroring `tests/home.rs::first_row_center`.
fn first_row_center(scene: &RecScene) -> Point {
    let (origin, size) = scene
        .rounded
        .iter()
        .copied()
        .filter(|(o, _)| o.x < 100.0 && o.y >= CONTENT_TOP && o.y < CONTENT_BOTTOM)
        .min_by(|a, b| a.0.y.partial_cmp(&b.0.y).unwrap())
        .expect("a loaded feed row paints a leading avatar circle");
    Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
}

/// Rebuild + layout + paint like [`support::frame`], but painting at `t_ms` on a
/// caller-advanced clock instead of the shared harness's pinned
/// [`FrameTime::ZERO`]. Returns the recorded scene together with whether the
/// paint asked for another frame (an animation — such as the boot/push
/// cross-fade — is still running). Mirrors `tests/home.rs`'s helper of the same
/// name (see the module docs).
fn frame_at(
    root: &mut Root,
    logic: &mut impl FnMut(&mut HuddleState) -> AnyView<HuddleState>,
    state: &mut HuddleState,
    tcx: &mut TextContext,
    t_ms: u64,
) -> (RecScene, bool) {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, support::H), tcx_any);
    let mut scene = RecScene::default();
    let outcome = root.paint(&mut scene, FrameTime::from_nanos(t_ms * 1_000_000));
    (scene, outcome.needs_frame)
}

/// Navigate a fresh app straight to `/activity` and pump past the mocked load
/// latency — advancing the paint clock so the boot/push cross-fade settles and
/// page input flows (see the module docs). Returns the root/state/logic/tcx,
/// the current paint-clock reading (so callers keep advancing it), and the
/// loaded scene.
fn mount_loaded_activity_tab() -> (
    Root,
    HuddleState,
    impl FnMut(&mut HuddleState) -> AnyView<HuddleState>,
    TextContext,
    u64,
    RecScene,
) {
    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    let mut tcx = TextContext::new();

    // Build once at "/" before queuing the push, mirroring
    // `tests/shell.rs::routes_push_and_pop_without_panicking` — the
    // `NavigatorController`'s op queue is drained at the navigator's *next*
    // rebuild, so a push queued before the navigator widget exists has nothing
    // to drain into yet.
    root.rebuild(&mut logic, &mut state);
    state.nav.router().push("/activity");

    // First frame: the entering Activity page's loading skeletons render text,
    // and no "Unread" chip is painted yet (the feed hasn't resolved).
    let (loading_scene, _) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 16);
    assert!(
        loading_scene.glyph_runs > 0,
        "the loading skeletons render text"
    );
    assert_eq!(
        unread_chips(&loading_scene),
        0,
        "no unread chip is painted before the feed resolves"
    );

    // Pump the spawned loader (`spawn_local`) while advancing the paint clock,
    // which settles the boot/push cross-fade so subsequent taps reach the page.
    // The feed resolves past its mocked ~400ms latency and starts painting one
    // "Unread" chip per (freshly unread) row.
    let runtime = ReactiveRuntime::get().expect("setup() installed the reactive runtime");
    let deadline = Instant::now() + LOAD_WAIT;
    let mut t_ms = 50u64;
    let loaded_scene = loop {
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(1));
        t_ms += 16;
        let (scene, _needs_frame) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
        if unread_chips(&scene) > 0 {
            break scene;
        }
        assert!(
            Instant::now() < deadline,
            "the activity feed did not load within the deadline"
        );
    };

    (root, state, logic, tcx, t_ms, loaded_scene)
}

/// The feed loads past its mocked latency and renders a row per mention — every
/// mention starts unread, so the loaded scene paints leading avatars and
/// trailing "Unread" chips that the loading skeleton never did.
#[test]
fn activity_tab_loads_and_renders_items() {
    let _g = serial();
    let _ambient = setup();

    let (_root, _state, _logic, _tcx, _t_ms, loaded_scene) = mount_loaded_activity_tab();

    assert!(
        loaded_scene.glyph_runs > 0,
        "the loaded feed renders actor/action/excerpt text"
    );
    assert!(
        row_avatars(&loaded_scene) > 0,
        "the loaded feed paints a leading avatar per row"
    );
    assert!(
        unread_chips(&loaded_scene) > 0,
        "every freshly loaded row paints its \"Unread\" indicator chip"
    );
}

/// Tapping the app bar's "mark all read" action clears every row's unread
/// indicator: the trailing "Unread" chips stop painting, while the rows
/// themselves (their leading avatars) remain.
#[test]
fn mark_all_read_clears_the_unread_indicators() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, loaded_scene) =
        mount_loaded_activity_tab();
    let avatars_before = row_avatars(&loaded_scene);
    assert!(
        unread_chips(&loaded_scene) > 0,
        "the freshly loaded feed has unread indicators to clear"
    );

    tap(&mut root, &mut state, MARK_ALL_READ);

    // `mark_all_read` runs on the UI-thread local task queue (`spawn_local`);
    // pump frames (advancing the clock) so it drains and the rebuild observes
    // the cleared rows.
    let runtime = ReactiveRuntime::get().expect("setup() installed the reactive runtime");
    let deadline = Instant::now() + Duration::from_secs(2);
    let after = loop {
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(1));
        t_ms += 16;
        let (scene, _needs_frame) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
        if unread_chips(&scene) == 0 {
            break scene;
        }
        assert!(
            Instant::now() < deadline,
            "marking all read never cleared the unread indicators"
        );
    };

    assert_eq!(
        unread_chips(&after),
        0,
        "marking all read removes every row's unread indicator"
    );
    assert_eq!(
        row_avatars(&after),
        avatars_before,
        "the rows themselves remain — only the unread indicator is cleared"
    );
}

/// Tapping a row opens the local in-tab channel detail view (see
/// `src/screens/activity.rs`'s Navigation note): the detail rows carry no
/// avatar/chip chrome, so both drop to zero — and the back action returns to
/// the feed list (its unread indicators reappear).
#[test]
fn tapping_a_row_opens_and_closes_the_channel_detail_view() {
    let _g = serial();
    let _ambient = setup();

    let (mut root, mut state, mut logic, mut tcx, mut t_ms, loaded_scene) =
        mount_loaded_activity_tab();

    let row = first_row_center(&loaded_scene);
    tap(&mut root, &mut state, row);

    // The drill-down is a local `ActivityState::open` toggle (no navigator
    // push, so no cross-fade); one advanced-clock frame renders it.
    t_ms += 16;
    let (detail_scene, _) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
    assert!(
        detail_scene.glyph_runs > 0,
        "the channel's messages render as text"
    );
    assert_eq!(
        row_avatars(&detail_scene),
        0,
        "the detail view's plain message rows have no avatar chrome"
    );
    assert_eq!(
        unread_chips(&detail_scene),
        0,
        "nor any per-row unread chip"
    );

    tap(&mut root, &mut state, BACK_ACTION);
    t_ms += 16;
    let (back_scene, _) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
    assert!(
        unread_chips(&back_scene) > 0,
        "the back action returns to the feed list (its unread indicators reappear)"
    );
    assert!(
        row_avatars(&back_scene) > 0,
        "the feed list's rows are back"
    );
}
