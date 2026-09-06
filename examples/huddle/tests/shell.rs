//! Shell + route + toast smoke tests for the Huddle skeleton.
//!
//! Every test that touches process-global reactive state (the shared background
//! executor's timing, `push_transparent_for_result`'s deep-owner interaction)
//! takes the [`support::serial`] lock first, so the trimmed suite is
//! deterministic under default parallel `cargo test` (the known `ported.rs`
//! flake — see the harness docs).
//!
//! `Tab` (the shell's selected-destination enum) is a private type (`mod
//! shell`), so the tab-switch test reads the `state.tab` signal and asserts it
//! *changed* across a tap rather than naming a specific variant.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use frust::{
    AnyView, Component, GetUntracked, PopResult, TransitionSpec, handles_back, push_back_press,
};
use frust_core::RenderRoot;
use frust_text::TextContext;
use kurbo::Point;

use huddle::ui::toast::ToastController;
use huddle::{HuddleApp, HuddleState};

mod support;
use support::{W, center, frame, serial, setup, tap};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

/// The 4 tabs switch via synthesized taps on the bottom navigation bar, and
/// each tab's placeholder renders.
#[test]
fn shell_builds_and_tabs_switch_via_taps() {
    let _g = serial();
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    let mut tcx = TextContext::new();

    // Initial frame: the Home tab renders.
    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(scene.glyph_runs > 0, "the Home tab renders text");

    // The bottom navigation bar is the 64px-tall inflexible row at the bottom
    // of the 800×600 window; its four item slots are 200px wide, centered at
    // x = 100/300/500/700, y ≈ 568 (the bar's vertical middle).
    let bar_y = 568.0;
    let slot_centers = [100.0, 300.0, 500.0, 700.0];

    let mut prev = state.tab.get_untracked();
    // Tap Search (slot 1), Activity (slot 2), You (slot 3) — each a different
    // tab — and assert the selection changes and the new placeholder renders.
    for &x in &slot_centers[1..] {
        tap(&mut root, &mut state, Point::new(x, bar_y));
        let now = state.tab.get_untracked();
        assert_ne!(
            prev, now,
            "tapping a different tab slot changes the selected tab"
        );
        prev = now;

        let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
        assert!(root.root_id().is_some(), "the shell rebuilds on tab switch");
        assert!(
            scene.glyph_runs > 0,
            "the switched-to tab renders its placeholder"
        );
    }

    // Tap back to Home (slot 0) and confirm it, too, switches + renders.
    tap(&mut root, &mut state, Point::new(slot_centers[0], bar_y));
    assert_ne!(
        prev,
        state.tab.get_untracked(),
        "tapping Home switches back"
    );
    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(scene.glyph_runs > 0);
}

/// Route smoke: a channel push and the settings stack navigate and pop without
/// panicking.
#[test]
fn routes_push_and_pop_without_panicking() {
    let _g = serial();
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);

    root.rebuild(&mut logic, &mut state);
    assert!(root.root_id().is_some(), "the shell builds at \"/\"");

    for route in [
        "/channel/general",
        "/you/settings",
        "/you/settings/appearance",
    ] {
        state.nav.router().push(route);
        root.rebuild(&mut logic, &mut state);
        assert!(root.root_id().is_some(), "pushing {route:?} builds");
    }

    // Pop the settings-appearance page back to the settings menu.
    state.nav.router().controller().pop();
    root.rebuild(&mut logic, &mut state);
    assert!(root.root_id().is_some(), "a pop rebuilds cleanly");
}

/// The Android/gesture back contract: a back
/// press pops the mounted navigator, and `frust::handles_back` mirrors
/// whether the framework will consume the *next* press (a single-page stack
/// bubbles to the platform instead). `state.back.track()` runs every
/// `HuddleApp::build` (see `lib.rs`), so a plain `rebuild()` — no layout/paint
/// needed, exactly like `routes_push_and_pop_without_panicking` above — is
/// enough to drive it.
///
/// `handles_back` refreshes one rebuild *after* the stack change that would
/// flip it (`BackHandler::track`'s documented timing-lag contract); every
/// assertion on the flag below settles with one extra `rebuild()` first,
/// mirroring `frust`'s own `back_glue` test.
#[test]
fn back_press_pops_the_navigator_and_root_bubbles_to_the_platform() {
    let _g = serial();
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);

    root.rebuild(&mut logic, &mut state);
    root.rebuild(&mut logic, &mut state); // settle the initial handles_back read
    assert_eq!(state.back.controller().depth(), 1, "starts at the tab root");
    assert!(
        !handles_back(),
        "a single-page stack bubbles back to the platform"
    );

    // Push a detail page: depth updates immediately; handles_back mirrors it
    // one rebuild later.
    state.nav.router().push("/channel/general");
    root.rebuild(&mut logic, &mut state);
    assert_eq!(state.back.controller().depth(), 2, "the push is published");
    root.rebuild(&mut logic, &mut state); // settle
    assert!(handles_back(), "a poppable stack sets handles_back true");

    // A back press pops one page.
    push_back_press();
    root.rebuild(&mut logic, &mut state);
    assert_eq!(
        state.back.controller().depth(),
        1,
        "one back press pops the pushed page"
    );
    root.rebuild(&mut logic, &mut state); // settle
    assert!(
        !handles_back(),
        "back at the root: handles_back reports false again"
    );

    // A back press at the root is a safe no-op (consumed but pops nothing —
    // the shell already bubbled it to the platform since handles_back was
    // false).
    push_back_press();
    root.rebuild(&mut logic, &mut state);
    assert_eq!(state.back.controller().depth(), 1, "pop-at-root is a no-op");
}

/// A modal pushed transparently through the real, mounted `NavigatorController`
/// and popped with a result delivers that `PopResult` back into app state — the
/// same round trip the toast undo affordance and the sheet overlays build on.
#[test]
fn modal_round_trip_delivers_its_result() {
    let _g = serial();
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);

    frust::provide_context(frust_material::baseline());
    root.rebuild(&mut logic, &mut state);

    let delivered: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let sink = Arc::clone(&delivered);
    let controller = state.nav.router().controller().clone();
    controller.push_transparent_for_result(
        || frust::any(frust::text("probe dialog")),
        TransitionSpec::NONE,
        move |_s: &mut HuddleState, result: PopResult| {
            *sink.lock().unwrap() = result.take::<String>();
        },
    );
    root.rebuild(&mut logic, &mut state); // the pushed page's first build
    controller.pop_with_result(PopResult::of("confirmed".to_string()));

    // One REAL frame (rebuild + layout at W×H + paint), and NO input event of
    // any kind. The rebuild that applies the pop also dispatches
    // the `InputEvent::Housekeeping` broadcast that flushes the queued
    // `on_result`, so the result is in app state before this frame ends. The
    // whole point is that it no longer waits on a tap — which, in this shell,
    // chrome outside the navigator could swallow entirely.
    let mut tcx = TextContext::new();
    frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(
        delivered.lock().unwrap().clone(),
        Some("confirmed".to_string()),
        "a modal push/pop round trip delivers its PopResult back into app state \
         on the pop's own frame, with no input event"
    );

    // A later, unrelated event must not re-deliver: the queue was drained, not
    // copied.
    *delivered.lock().unwrap() = None;
    root.event(
        &mut state,
        &support::pointer(frust_core::PointerPhase::Move, Point::new(W / 2.0, 300.0)),
    );
    assert!(
        delivered.lock().unwrap().is_none(),
        "the flushed callback fires exactly once"
    );
}

/// A toast renders above the shell, and tapping its action button fires the
/// callback and dismisses the toast.
#[test]
fn toast_renders_and_action_callback_fires() {
    let _g = serial();
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);
    let mut tcx = TextContext::new();

    let fired = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&fired);
    state
        .toasts
        .show_with_action("Archived #random", "Undo", move || {
            flag.store(true, Ordering::SeqCst);
        });

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(scene.glyph_runs > 0, "the toast text renders");
    assert!(
        !scene.rounded.is_empty(),
        "the toast's action button paints a rounded rect"
    );
    assert_eq!(
        state.toasts.snapshot_untracked().len(),
        1,
        "one toast queued"
    );

    // The action button is the last-painted rounded rect: the toast overlay is
    // the top Stack layer, and the button paints after its card background.
    let button = *scene.rounded.last().expect("a rounded rect was recorded");
    tap(&mut root, &mut state, center(button));

    assert!(
        fired.load(Ordering::SeqCst),
        "tapping Undo fires the action callback"
    );
    assert!(
        state.toasts.snapshot_untracked().is_empty(),
        "the toast dismisses once its action fires"
    );
}

/// A toast auto-dismisses after its configured delay (driven by the background
/// runtime timer). Tested at the controller level with a short delay so the
/// round trip completes fast and deterministically.
#[test]
fn toast_auto_dismisses_after_its_delay() {
    let _g = serial();
    let _ambient = setup();

    let controller = ToastController::with_dismiss_after(Duration::from_millis(50));
    controller.show("saved");
    assert_eq!(
        controller.snapshot_untracked().len(),
        1,
        "the toast is queued"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while !controller.snapshot_untracked().is_empty() {
        assert!(
            Instant::now() < deadline,
            "the toast did not auto-dismiss in time"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
