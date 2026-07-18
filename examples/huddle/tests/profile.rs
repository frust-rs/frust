//! Profile + workspace-switcher screen tests (Phase C task 16).
//!
//! Every test that touches process-global reactive state (the shared
//! background executor's timing) takes the [`support::serial`] lock first,
//! mirroring `tests/shell.rs`'s convention.

use forgekit::{AnyView, Component};
use forgekit_core::RenderRoot;
use forgekit_text::TextContext;

use huddle::{HuddleApp, HuddleState};

mod support;
use support::{center, frame, serial, setup, tap};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

fn harness() -> (Root, HuddleState, TextContext) {
    let root: Root = RenderRoot::new();
    let state = HuddleApp.init();
    let tcx = TextContext::new();
    (root, state, tcx)
}

/// The profile card renders a known mock user's fields (name/handle/status/
/// local time/role·team) and its avatar tile.
///
/// Hero paints transparently outside a page transition (see
/// `forgekit_widgets::nav::hero`'s module docs) — there is no distinct "hero
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
    let (mut root, mut state, mut tcx) = harness();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);

    // User 2 (Grace Hopper) has a DM (dm-2) in the mock dataset — exercises
    // the Message action's "DM exists" branch too.
    state.nav.router().push("/user/2");
    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);

    assert!(
        scene.glyph_runs > 0,
        "the profile card renders name/handle/status/local-time/role·team text"
    );
    assert_eq!(
        scene.rounded.len(),
        3,
        "the avatar tile + the Message/Huddle-call buttons paint 3 rounded rects"
    );
}

/// An unknown `/user/:id` renders the not-found fallback instead of panicking.
#[test]
fn unknown_user_id_renders_a_fallback_without_panicking() {
    let _g = serial();
    let _ambient = setup();
    let (mut root, mut state, mut tcx) = harness();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);

    state.nav.router().push("/user/999");
    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(
        scene.glyph_runs > 0,
        "the not-found fallback still renders text"
    );
}

/// The workspace switcher renders exactly 3 workspaces: one panel background
/// rounded rect plus one initials-tile rounded rect per workspace row.
#[test]
fn drawer_renders_three_workspaces() {
    let _g = serial();
    let _ambient = setup();
    let (mut root, mut state, mut tcx) = harness();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);

    state.nav.router().push("/workspace-switcher");
    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);

    assert!(scene.glyph_runs > 0, "the workspace rows render text");
    assert_eq!(
        scene.rounded.len(),
        4,
        "1 panel background + 1 initials tile per workspace (3 workspaces)"
    );
}

/// Tapping a non-active workspace toasts "Switched to <name>" and pops the
/// drawer.
#[test]
fn tapping_a_non_active_workspace_toasts_and_pops() {
    let _g = serial();
    let _ambient = setup();
    let (mut root, mut state, mut tcx) = harness();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);

    state.nav.router().push("/workspace-switcher");
    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(scene.rounded.len(), 4, "sanity: the drawer is showing");

    // Rounded-rect paint order: [0] panel background, [1] Huddle HQ (active),
    // [2] Forge Labs, [3] Weekend Crew — see `workspace_drawer_screen`'s
    // `WORKSPACES` order. Forge Labs (index 2) is not active.
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

    let scene_after = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_ne!(
        scene_after.rounded.len(),
        4,
        "the drawer popped — its 4-rounded-rect signature no longer renders"
    );
}

/// Tapping the scrim (anywhere outside the panel) pops the drawer, with no
/// toast.
#[test]
fn tapping_the_scrim_pops_without_toasting() {
    let _g = serial();
    let _ambient = setup();
    let (mut root, mut state, mut tcx) = harness();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);

    state.nav.router().push("/workspace-switcher");
    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(scene.rounded.len(), 4, "sanity: the drawer is showing");

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

    let scene_after = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_ne!(
        scene_after.rounded.len(),
        4,
        "the drawer popped — its 4-rounded-rect signature no longer renders"
    );
}
