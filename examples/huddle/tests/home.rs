//! Home screen tests (Phase C, task 11): the loading→loaded transition, the
//! swipe-to-archive undo toast, unread badges, and pull-to-refresh — driven
//! headlessly through the full `HuddleApp` shell (the same harness `shell.rs`
//! uses). Every test takes the [`support::serial`] lock first, since the roster
//! load runs on the shared background reactive runtime (its timer is a
//! process-global the parallel `cargo test` default would otherwise race).

use std::time::{Duration, Instant};

use forgekit::{AnyView, Component};
use forgekit_core::{PointerPhase, RenderRoot};
use forgekit_reactive::ReactiveRuntime;
use forgekit_text::TextContext;
use kurbo::Point;

use huddle::{HuddleApp, HuddleState};

mod support;
use support::{frame, pointer, serial, setup};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

/// Render frames (letting the background loader's ~600ms timer fire) until the
/// roster's text glyph count jumps well past the loading skeleton's, or a
/// deadline trips. Returns the loaded scene.
fn render_until_loaded(
    root: &mut Root,
    logic: &mut impl FnMut(&mut HuddleState) -> AnyView<HuddleState>,
    state: &mut HuddleState,
    tcx: &mut TextContext,
    loading_glyphs: usize,
) -> support::RecScene {
    let runtime = ReactiveRuntime::get().expect("setup() installed the reactive runtime");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // Drain the UI-thread task queue (the loader is a `spawn_local` task) and
        // let its ~600ms background timer advance, then re-render.
        runtime.pump_local();
        let scene = frame(root, logic, state, tcx);
        if scene.glyph_runs > loading_glyphs + 10 {
            return scene;
        }
        assert!(
            Instant::now() < deadline,
            "the roster did not load within the deadline"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Find the first ~40×40 rounded rect in the list region (a roster row's leading
/// avatar/`#` circle), returning its center point.
fn first_row_center(scene: &support::RecScene) -> Point {
    let (origin, size) = scene
        .rounded
        .iter()
        .copied()
        .find(|(origin, size)| {
            (size.width - 40.0).abs() < 1.5
                && (size.height - 40.0).abs() < 1.5
                && origin.y > 64.0
                && origin.y < 500.0
        })
        .expect("a roster row leading circle was painted");
    Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
}

#[test]
fn loading_transitions_to_a_loaded_roster() {
    let _g = serial();
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    let mut tcx = TextContext::new();

    // First frame: the loading skeleton (plus the shell chrome text).
    let loading = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let loading_glyphs = loading.glyph_runs;

    // The roster loads and renders many more text rows (channel/DM names,
    // previews, section headers, unread badge counts).
    let loaded = render_until_loaded(&mut root, &mut logic, &mut state, &mut tcx, loading_glyphs);
    assert!(
        loaded.glyph_runs > loading_glyphs,
        "the loaded roster renders more text than the loading skeleton"
    );
    // Unread badges are rounded rects painted with their count text — a loaded
    // roster paints many rounded rects (avatars, badges).
    assert!(
        loaded.rounded.len() > 4,
        "the loaded roster paints avatar circles and unread badges"
    );
}

#[test]
fn swipe_right_archives_with_an_undo_toast() {
    let _g = serial();
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    let mut tcx = TextContext::new();

    let loading = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let loaded = render_until_loaded(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        loading.glyph_runs,
    );

    let center = first_row_center(&loaded);
    let y = center.y;

    // Swipe right: Down, a Move past the slop (takeover), a Move well past the
    // commit fraction, then Up — commits the archive action.
    root.event(
        &mut state,
        &pointer(PointerPhase::Down, Point::new(160.0, y)),
    );
    root.event(
        &mut state,
        &pointer(PointerPhase::Move, Point::new(230.0, y)),
    );
    root.event(
        &mut state,
        &pointer(PointerPhase::Move, Point::new(700.0, y)),
    );
    root.event(&mut state, &pointer(PointerPhase::Up, Point::new(700.0, y)));

    // The swipe raised an "Archived …" undo toast (the observable that the
    // controller archive op fired).
    let snap = state.toasts.snapshot_untracked();
    let entry = snap
        .iter()
        .find(|e| e.text.starts_with("Archived"))
        .expect("swipe-right raises an archive toast");
    let action = entry
        .action
        .as_ref()
        .expect("the archive toast has an action");
    assert_eq!(action.label, "Undo", "the action is an Undo affordance");

    // Undo restores (runs the restore state-patch) without panicking, and the
    // app keeps rendering.
    (action.callback)();
    let after = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(after.glyph_runs > 0, "the app still renders after undo");
}

#[test]
fn pull_to_refresh_reloads_without_disrupting_the_roster() {
    let _g = serial();
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    let mut tcx = TextContext::new();

    let loading = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let loaded = render_until_loaded(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        loading.glyph_runs,
    );
    let loaded_glyphs = loaded.glyph_runs;

    // A pull-to-refresh gesture: at the top of the list, drag down well past the
    // refresh trigger, then release. This re-runs the loader (Reloading keeps
    // the stale roster visible).
    let x = support::W / 2.0;
    root.event(
        &mut state,
        &pointer(PointerPhase::Down, Point::new(x, 120.0)),
    );
    root.event(
        &mut state,
        &pointer(PointerPhase::Move, Point::new(x, 180.0)),
    );
    root.event(
        &mut state,
        &pointer(PointerPhase::Move, Point::new(x, 460.0)),
    );
    root.event(&mut state, &pointer(PointerPhase::Up, Point::new(x, 460.0)));

    // The stale roster stays visible during the reload (Reloading), so the app
    // keeps rendering rows.
    let after = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(
        after.glyph_runs > loaded_glyphs / 2,
        "the roster stays visible while refreshing"
    );

    // Let the reload complete; the roster is still there.
    let reloaded = render_until_loaded(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(reloaded.glyph_runs > 10, "the reloaded roster renders");
}
