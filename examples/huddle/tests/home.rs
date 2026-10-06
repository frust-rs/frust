//! Home screen tests: the loading→loaded transition, the
//! swipe-to-archive undo toast, unread badges, and pull-to-refresh — driven
//! headlessly through the full `HuddleApp` shell (the same harness `shell.rs`
//! uses). Every test takes the [`support::serial`] lock first, since the roster
//! load runs on the shared background reactive runtime (its timer is a
//! process-global the parallel `cargo test` default would otherwise race).
//! Every frame paints on an advancing [`PaintClock`] — see its docs for why a
//! pinned clock can never show the loaded roster.

use std::any::Any;
use std::time::{Duration, Instant};

use frust::{AnyView, Component};
use frust_core::{FrameTime, PointerPhase, RenderRoot};
use frust_reactive::ReactiveRuntime;
use frust_text::TextContext;
use kurbo::{Point, Size};

use huddle::{HuddleApp, HuddleState};

mod support;
use support::{pointer, serial, setup};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

/// The paint clock's step per frame (one 60 Hz frame).
const FRAME_MS: u64 = 16;

/// How many frames an animation gets to settle before a test gives up: ~4.8 s
/// of paint clock, an order of magnitude past the boot entrance transition.
const SETTLE_FRAMES: usize = 300;

/// A monotonic paint clock: [`frame`](Self::frame) paints at the current time
/// and then advances it by [`FRAME_MS`], the way a real shell's clock moves.
///
/// The app boots mid-transition: the router's start location replaces the
/// navigator's seeded Home page under an `M3FadeThrough` cross-fade. Until that
/// cross-fade settles, the replaced page is the one on screen — and the
/// navigator does not rebuild a replaced page, so it keeps its loading
/// skeleton. The live Home page the roster loads into sits at alpha 0 on the
/// cross-fade's first stretch, and the navigator paints an alpha-0 page into a
/// discard sink, so none of its text reaches the recorded scene. A clock pinned
/// at [`FrameTime::ZERO`] — the time [`support::frame`] paints at — holds the
/// cross-fade at progress 0 forever, so no frame could ever show the loaded
/// roster.
struct PaintClock {
    t_ms: u64,
}

impl PaintClock {
    fn new() -> Self {
        Self { t_ms: 0 }
    }

    /// Rebuild, lay out and paint one frame at the current time, then advance
    /// the clock. Returns the recorded scene together with whether the paint
    /// asked for another frame (an animation — such as a page transition — is
    /// still running).
    fn frame(
        &mut self,
        root: &mut Root,
        logic: &mut impl FnMut(&mut HuddleState) -> AnyView<HuddleState>,
        state: &mut HuddleState,
        tcx: &mut TextContext,
    ) -> (support::RecScene, bool) {
        root.rebuild(logic, state);
        let tcx_any: &mut dyn Any = tcx;
        root.layout_with_text(Size::new(support::W, support::H), tcx_any);
        let mut scene = support::RecScene::default();
        let outcome = root.paint(&mut scene, FrameTime::from_nanos(self.t_ms * 1_000_000));
        self.t_ms += FRAME_MS;
        (scene, outcome.needs_frame)
    }
}

/// Render frames (letting the background loader's ~600ms timer fire) until the
/// roster's text glyph count jumps well past the loading skeleton's, or a
/// deadline trips. Returns the loaded scene.
fn render_until_loaded(
    clock: &mut PaintClock,
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
        let (scene, _) = clock.frame(root, logic, state, tcx);
        if scene.glyph_runs > loading_glyphs + 10 {
            return scene;
        }
        let transition = state.nav.router().controller().transition();
        assert!(
            Instant::now() < deadline,
            "the roster did not load within the deadline (entrance transition \
             active: {}, progress {:.3})",
            transition.active,
            transition.progress
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Paint frames until no animation asks for another, and return the settled
/// scene. While a page transition runs the navigator suppresses ALL page input
/// (its input-blocking contract), so a gesture dispatched mid-transition never
/// reaches the roster.
fn settle_transitions(
    clock: &mut PaintClock,
    root: &mut Root,
    logic: &mut impl FnMut(&mut HuddleState) -> AnyView<HuddleState>,
    state: &mut HuddleState,
    tcx: &mut TextContext,
) -> support::RecScene {
    for _ in 0..SETTLE_FRAMES {
        let (scene, needs_frame) = clock.frame(root, logic, state, tcx);
        if !needs_frame {
            return scene;
        }
    }
    panic!("the app was still animating after {SETTLE_FRAMES} frames");
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

/// The paint clock carries the app out of its boot entrance cross-fade within a
/// bounded number of frames, independently of the roster loader's timer. Until
/// the cross-fade settles, the page the roster loads into is painted into a
/// discard sink (see [`PaintClock`]), so a clock that cannot settle it makes
/// every roster assertion in this file unreachable.
#[test]
fn boot_entrance_transition_settles_on_the_paint_clock() {
    let _g = serial();
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| AnyView::new(HuddleApp.build(s));
    let mut tcx = TextContext::new();
    let mut clock = PaintClock::new();
    let nav = state.nav.router().controller().clone();

    clock.frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(
        nav.transition().active,
        "the app boots mid-way through its entrance cross-fade"
    );

    for _ in 0..SETTLE_FRAMES {
        if !nav.transition().active {
            break;
        }
        clock.frame(&mut root, &mut logic, &mut state, &mut tcx);
    }
    let transition = nav.transition();
    assert!(
        !transition.active,
        "the entrance cross-fade never settled on the paint clock (still at \
         progress {:.3} after {SETTLE_FRAMES} frames)",
        transition.progress
    );
}

#[test]
fn loading_transitions_to_a_loaded_roster() {
    let _g = serial();
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| AnyView::new(HuddleApp.build(s));
    let mut tcx = TextContext::new();
    let mut clock = PaintClock::new();

    // First frame: the loading skeleton (plus the shell chrome text).
    let (loading, _) = clock.frame(&mut root, &mut logic, &mut state, &mut tcx);
    let loading_glyphs = loading.glyph_runs;

    // The roster loads and renders many more text rows (channel/DM names,
    // previews, section headers, unread badge counts).
    let loaded = render_until_loaded(
        &mut clock,
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        loading_glyphs,
    );
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
    let mut logic = |s: &mut HuddleState| AnyView::new(HuddleApp.build(s));
    let mut tcx = TextContext::new();
    let mut clock = PaintClock::new();

    let (loading, _) = clock.frame(&mut root, &mut logic, &mut state, &mut tcx);
    render_until_loaded(
        &mut clock,
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        loading.glyph_runs,
    );

    // Settle every in-flight animation before driving the gesture: while a
    // transition is in flight the navigator blocks all page input, so a swipe
    // dispatched now would be swallowed (see `settle_transitions`). The settled
    // scene paints only the live roster, so its first row circle is a real
    // swipeable row (not a frozen leaving-page skeleton).
    let loaded = settle_transitions(&mut clock, &mut root, &mut logic, &mut state, &mut tcx);

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
    let (after, _) = clock.frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(after.glyph_runs > 0, "the app still renders after undo");
}

#[test]
fn pull_to_refresh_reloads_without_disrupting_the_roster() {
    let _g = serial();
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| AnyView::new(HuddleApp.build(s));
    let mut tcx = TextContext::new();
    let mut clock = PaintClock::new();

    let (loading, _) = clock.frame(&mut root, &mut logic, &mut state, &mut tcx);
    let loaded = render_until_loaded(
        &mut clock,
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
    let (after, _) = clock.frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(
        after.glyph_runs > loaded_glyphs / 2,
        "the roster stays visible while refreshing"
    );

    // Let the reload complete; the roster is still there.
    let reloaded = render_until_loaded(&mut clock, &mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(reloaded.glyph_runs > 10, "the reloaded roster renders");
}
