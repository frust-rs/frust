//! Profile + workspace-switcher screen tests.
//!
//! Every test that touches process-global reactive state (the shared
//! background executor's timing) takes the [`support::serial`] lock first,
//! mirroring `tests/shell.rs`'s convention.
//!
//! # Settling the boot/push cross-fade before counting or tapping
//!
//! The shell's navigator runs an `M3FadeThrough` cross-fade at boot (two Home
//! pages — the seeded root and the pushed `"/"` route) and again when a detail
//! route (`/user/:id`, `/workspace-switcher`) is pushed. The shared
//! [`support::frame`] helper paints at [`frust_core::FrameTime::ZERO`],
//! which freezes that transition forever — so the *leaving* Home page keeps
//! painting alongside the entering page, and every rounded-rect count (and the
//! `rounded[2]` empirical index the drawer taps rely on) is contaminated by
//! Home's chrome. Worse, while a transition is in flight the navigator
//! suppresses ALL page input, so a tap dispatched mid-transition never reaches
//! the drawer. [`push_and_settle`] therefore advances the paint clock past the
//! cross-fade ([`settle_transitions`], the same fix `tests/home.rs` and
//! `tests/search.rs` use) before any test counts rects or drives a tap, by
//! which point the leaving Home page is torn down and only the destination
//! page paints.

use std::any::Any;

use frust::{AnyView, Component};
use frust_core::{FrameTime, RenderRoot};
use frust_text::TextContext;
use kurbo::Size;

use huddle::{HuddleApp, HuddleState};

mod support;
use support::{H, RecScene, W, center, frame, serial, setup, tap};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

fn harness() -> (Root, HuddleState, TextContext) {
    let root: Root = RenderRoot::new();
    let state = HuddleApp.init();
    let tcx = TextContext::new();
    (root, state, tcx)
}

/// Rebuild + layout + paint like [`support::frame`], but painting at `t_ms` on a
/// caller-advanced clock instead of the shared harness's pinned
/// [`FrameTime::ZERO`]. Returns the recorded scene and whether the paint asked
/// for another frame (an animation — such as a page transition — is still
/// running). Mirrors `tests/search.rs`'s helper of the same name.
fn frame_at(
    root: &mut Root,
    state: &mut HuddleState,
    tcx: &mut TextContext,
    t_ms: u64,
) -> (RecScene, bool) {
    let mut logic = |s: &mut HuddleState| AnyView::new(HuddleApp.build(s));
    root.rebuild(&mut logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, H), tcx_any);
    let mut scene = RecScene::default();
    let outcome = root.paint(&mut scene, FrameTime::from_nanos(t_ms * 1_000_000));
    (scene, outcome.needs_frame)
}

/// Advance the paint clock until no animation asks for another frame, settling
/// the boot/push cross-fade so page input flows again and only the destination
/// page renders. Returns the settled scene (see the module docs, and
/// `tests/search.rs`'s helper of the same name).
fn settle_transitions(root: &mut Root, state: &mut HuddleState, tcx: &mut TextContext) -> RecScene {
    let mut t_ms = 50u64;
    loop {
        t_ms += 16;
        let (scene, needs_frame) = frame_at(root, state, tcx, t_ms);
        if !needs_frame {
            return scene;
        }
        assert!(
            t_ms < 50 + 16 * 300,
            "the boot/push entrance transition never settled"
        );
    }
}

/// The drawer page's settled paint signature: 4 page rounded rects (panel
/// background + 3 workspace tiles) plus the persistent bottom bar's selection
/// pill.
const DRAWER_SIGNATURE: usize = 5;

/// Drive the pop cross-fade forward on an advancing clock until the drawer's
/// [`DRAWER_SIGNATURE`] is gone (the drawer torn down), returning the resulting
/// scene. Bounded rather than settled-to-quiescence: unlike the entering
/// drawer/profile pages, the *restored* Home page's loading-skeleton shimmer
/// keeps requesting frames forever, so [`settle_transitions`] would never return
/// after a pop — this stops at "drawer gone" instead. Mirrors
/// `tests/activity.rs`'s loop-until-observable pattern.
fn drive_until_drawer_gone(
    root: &mut Root,
    state: &mut HuddleState,
    tcx: &mut TextContext,
) -> RecScene {
    let mut t_ms = 50u64;
    loop {
        t_ms += 16;
        let (scene, _needs_frame) = frame_at(root, state, tcx, t_ms);
        if scene.rounded.len() != DRAWER_SIGNATURE {
            return scene;
        }
        assert!(t_ms < 50 + 16 * 300, "the drawer never popped");
    }
}

/// Boot the app, render its first frame (so the navigator widget exists and its
/// op queue has somewhere to drain — see `tests/activity.rs`'s note), push
/// `route`, and settle the ensuing cross-fade. Returns the root/state/tcx and
/// the settled destination scene, ready for counting or tapping.
fn push_and_settle(route: &str) -> (Root, HuddleState, TextContext, RecScene) {
    let (mut root, mut state, mut tcx) = harness();
    let mut logic = |s: &mut HuddleState| AnyView::new(HuddleApp.build(s));

    // First frame: mounts the navigator so the queued push has somewhere to
    // drain (mirrors `tests/activity.rs::mount_loaded_activity_tab`).
    frame(&mut root, &mut logic, &mut state, &mut tcx);
    state.nav.router().push(route);
    let scene = settle_transitions(&mut root, &mut state, &mut tcx);
    (root, state, tcx, scene)
}

/// The profile card renders a known mock user's fields (name/handle/status/
/// local time/role·team) and its avatar tile.
///
/// Hero paints transparently outside a page transition (see
/// `frust_widgets::nav::hero`'s module docs) — there is no distinct "hero
/// frame" the paint-recording harness can observe on a page with no
/// transition in flight, so per this task's spec this asserts the wrapped
/// avatar's own rounded-rect chrome renders in the expected structural
/// position instead (the documented fallback): `Column` paints its children
/// top-to-bottom and the avatar is the first child, so the *first* recorded
/// rounded rect (of the 3 this page paints: the avatar tile, then the
/// Message and Huddle-call buttons) is exactly the hero-wrapped avatar.
#[test]
fn profile_renders_known_mock_user_fields() {
    let _g = serial();
    let _ambient = setup();

    // User 2 (Grace Hopper) has a DM (dm-2) in the mock dataset — exercises
    // the Message action's "DM exists" branch too. Settle the boot/push
    // cross-fade so only the profile page paints (see the module docs).
    let (_root, _state, _tcx, scene) = push_and_settle("/user/2");

    assert!(
        scene.glyph_runs > 0,
        "the profile card renders name/handle/status/local-time/role·team text"
    );
    // The profile page paints 3 rounded rects (the avatar tile, then the
    // Message and Huddle-call buttons), and the persistent shell's bottom
    // navigation bar paints its active-tab selection pill (one
    // `fill_rounded_rect`, appended after the navigator's page content — see
    // `frust-widgets`' `navbar.rs`), for 4 total.
    assert_eq!(
        scene.rounded.len(),
        4,
        "avatar tile + Message/Huddle-call buttons (3) + the bottom bar's selection pill"
    );
}

/// An unknown `/user/:id` renders the not-found fallback instead of panicking.
#[test]
fn unknown_user_id_renders_a_fallback_without_panicking() {
    let _g = serial();
    let _ambient = setup();
    let (mut root, mut state, mut tcx) = harness();
    let mut logic = |s: &mut HuddleState| AnyView::new(HuddleApp.build(s));

    state.nav.router().push("/user/999");
    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(
        scene.glyph_runs > 0,
        "the not-found fallback still renders text"
    );
}

/// The workspace switcher renders exactly 3 workspaces: one panel background
/// rounded rect plus one initials-tile rounded rect per workspace row — plus
/// the persistent shell's bottom-bar selection pill.
#[test]
fn drawer_renders_three_workspaces() {
    let _g = serial();
    let _ambient = setup();

    // Settle the boot/push cross-fade so only the drawer page paints (see the
    // module docs) — otherwise the leaving Home page's chrome inflates the count.
    let (_root, _state, _tcx, scene) = push_and_settle("/workspace-switcher");

    assert!(scene.glyph_runs > 0, "the workspace rows render text");
    // The drawer paints 4 rounded rects (1 panel background + 1 initials tile
    // per workspace, 3 workspaces), and the persistent bottom navigation bar
    // paints its active-tab selection pill (one `fill_rounded_rect`, appended
    // after the navigator's page content — see `frust-widgets`' `navbar.rs`),
    // for 5 total.
    assert_eq!(
        scene.rounded.len(),
        5,
        "panel bg + 3 workspace tiles (4) + the bottom bar's selection pill"
    );
}

/// Tapping a non-active workspace toasts "Switched to <name>" and pops the
/// drawer.
#[test]
fn tapping_a_non_active_workspace_toasts_and_pops() {
    let _g = serial();
    let _ambient = setup();

    // Settle the boot/push cross-fade first: while it runs the navigator blocks
    // all page input, so a tap now would be swallowed (see the module docs).
    let (mut root, mut state, mut tcx, scene) = push_and_settle("/workspace-switcher");
    assert_eq!(
        scene.rounded.len(),
        DRAWER_SIGNATURE,
        "sanity: the drawer is showing"
    );

    // Rounded-rect paint order: [0] panel background, [1] Huddle HQ (active),
    // [2] Forge Labs, [3] Weekend Crew — see `workspace_drawer_screen`'s
    // `WORKSPACES` order (the bottom bar's pill paints last, at [4], so this
    // page-content indexing is unaffected). Forge Labs (index 2) is not active.
    let forge_labs_tile = scene.rounded[2];
    tap(&mut root, &mut state, center(forge_labs_tile));

    assert_eq!(
        state.toasts.snapshot_untracked().len(),
        1,
        "tapping a non-active workspace queues a toast"
    );
    assert!(
        state.toasts.snapshot_untracked()[0]
            .text
            .contains("Forge Labs"),
        "the toast names the switched-to workspace"
    );

    // Drive the pop cross-fade forward: the drawer's rounded-rect signature is
    // gone once it pops (see `drive_until_drawer_gone`).
    let scene_after = drive_until_drawer_gone(&mut root, &mut state, &mut tcx);
    assert_ne!(
        scene_after.rounded.len(),
        DRAWER_SIGNATURE,
        "the drawer popped — its rounded-rect signature no longer renders"
    );
}

/// Tapping the scrim (anywhere outside the panel) pops the drawer, with no
/// toast.
#[test]
fn tapping_the_scrim_pops_without_toasting() {
    let _g = serial();
    let _ambient = setup();

    // Settle the boot/push cross-fade first (see the module docs) so page input
    // flows and only the drawer paints.
    let (mut root, mut state, mut tcx, scene) = push_and_settle("/workspace-switcher");
    assert_eq!(
        scene.rounded.len(),
        DRAWER_SIGNATURE,
        "sanity: the drawer is showing"
    );

    // The panel background (rect 0) spans the full width starting at the top;
    // a point well below it (near the bottom of the 800x536 navigator area)
    // lands on the scrim rather than the panel.
    let scrim_point = kurbo::Point::new(400.0, 500.0);
    tap(&mut root, &mut state, scrim_point);

    assert_eq!(
        state.toasts.snapshot_untracked().len(),
        0,
        "a scrim tap raises no toast"
    );

    // Drive the pop cross-fade forward: the drawer's signature is gone once it
    // pops (see `drive_until_drawer_gone`).
    let scene_after = drive_until_drawer_gone(&mut root, &mut state, &mut tcx);
    assert_ne!(
        scene_after.rounded.len(),
        DRAWER_SIGNATURE,
        "the drawer popped — its rounded-rect signature no longer renders"
    );
}
