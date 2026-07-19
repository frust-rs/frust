//! Shared headless-test harness for clean-signals-frust's integration
//! tests: a GPU-free paint target, a one-frame helper, a recording waker, an
//! ambient-owner setup, and an async pump-poll loop — the inbox recipe
//! (`frust:examples/inbox/tests/async.rs`), factored so every `tests/*.rs`
//! file (each its own crate root — see `mod support;` below) can share it
//! without duplicating it. Not every test file uses every helper.
#![allow(dead_code)]

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use frust_core::{FrameTime, PaintScene, RenderRoot, View};
use frust_reactive::{FrameWaker, ReactiveRuntime};
use frust_scene::GlyphRun;
use frust_text::TextContext;
use kurbo::{Point, Rect, Size};
use peniko::Color;
use reactive_graph::owner::Owner;

pub const W: f64 = 800.0;
pub const H: f64 = 600.0;

/// A GPU-free paint target that just counts glyph runs — enough to observe
/// which arm of a rendered `AsyncState` painted (one row per `Text`/message)
/// without any GPU or `frust-render` dependency.
#[derive(Default)]
pub struct RecScene {
    pub glyph_runs: usize,
}

impl PaintScene for RecScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
    fn fill_rounded_rect(&mut self, _origin: Point, _size: Size, _radius: f64, _color: Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn draw_glyph_run(&mut self, _run: GlyphRun) {
        self.glyph_runs += 1;
    }
    fn draw_image(&mut self, _data: &peniko::ImageData, _dest: Rect) {}
}

/// One frame: rebuild the tree from `state`, lay it out shaping real text, and
/// record the paint output. Paints at `FrameTime::ZERO` — none of this crate's
/// tests drive an animation/transition, so a fixed clock is fine here (see
/// `examples/huddle/tests/support/mod.rs`'s identical `frame()` for the same
/// non-advancing-clock precedent; a test that *does* need to observe
/// clock-dependent (settled) rendering must roll its own advancing-clock
/// helper instead of reusing this one with repeated `FrameTime::ZERO` calls —
/// see `examples/huddle/tests/feed.rs`'s local `frame_at`).
pub fn frame<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
) -> RecScene {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, H), tcx_any);
    let mut scene = RecScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    scene
}

/// A no-op-but-recording frame waker (see [`ReactiveRuntime::init`]). The
/// global waker is swapped by whichever test inits last, so the count is
/// never asserted on — installing a recording waker just exercises the real
/// init path.
pub fn recording_waker() -> FrameWaker {
    let counter = Arc::new(AtomicUsize::new(0));
    Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })
}

/// Installs the reactive runtime and an ambient owner for a test, mirroring
/// the desktop shell's startup. Returns the ambient owner (held for the
/// test's duration so component owners stay valid children of it). Required
/// before any test that mounts a `Component` (`on_cleanup`/`spawn_local`/
/// context all need an ambient owner + the installed executor).
pub fn setup() -> Owner {
    let _runtime = ReactiveRuntime::init(recording_waker());
    let ambient = Owner::new();
    ambient.set();
    ambient
}

/// Drives frames — pump the UI-thread local task queue, sleep 1ms, rebuild —
/// until `done` returns `true`, starting from an already-rendered `scene`
/// (the caller's own first `frame()` call, so it can assert on the
/// pre-pump state first). This is the `examples/inbox`
/// (`tests/async.rs:119-133`) pump-poll pattern: the only sleep in this
/// harness, bounded by a 5-second deadline so a stuck async task fails the
/// test instead of hanging it.
///
/// Requires [`setup`] to have run (needs the installed [`ReactiveRuntime`]).
pub fn pump_until<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    mut scene: RecScene,
    mut done: impl FnMut() -> bool,
) -> RecScene {
    let runtime = ReactiveRuntime::get()
        .expect("pump_until: ReactiveRuntime::init must have run (see setup())");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(
            Instant::now() < deadline,
            "pump_until: timed out waiting for the condition"
        );
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(1));
        scene = frame(root, logic, state, tcx);
    }
    scene
}

/// Pumps and rebuilds `count` more frames unconditionally — for tests that
/// need to drive time/frames forward without waiting on a specific condition
/// (e.g. proving a stopped `use_interval` loop's tick count stays flat across
/// further frames).
///
/// Requires [`setup`] to have run.
pub fn pump_frames<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    count: usize,
) {
    let runtime = ReactiveRuntime::get()
        .expect("pump_frames: ReactiveRuntime::init must have run (see setup())");
    for _ in 0..count {
        runtime.pump_local();
        std::thread::sleep(Duration::from_millis(1));
        frame(root, logic, state, tcx);
    }
}
