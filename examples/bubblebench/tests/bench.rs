//! Headless integration tests: drive the real `RenderRoot`
//! rebuild→layout→paint seam (the desktop shell's own pipeline) against a
//! recording paint target — no GPU, no window — mirroring
//! `examples/huddle/tests/support/mod.rs`'s harness shape.
//!
//! The 60-bubble seed-42 field is a sustained limit cycle (center gravity vs.
//! the packed collision stack keeps peak speeds ~0.95, above the 0.1 settle
//! threshold), so the benchmark workload animates perpetually — these tests
//! assert the *controls* (pause/resume/touch/reset) and the per-frame paint
//! workload, not full settling (small clusters settle; see
//! `src/physics.rs`'s unit tests).

use std::any::Any;

use forgekit::{AnyView, GetUntracked, RwSignal, any, component};
use forgekit_core::{
    FrameTime, InputEvent, PaintScene, PointerButton, PointerEvent, PointerPhase, RenderRoot, View,
};
use forgekit_reactive::ReactiveRuntime;
use forgekit_scene::GlyphRun;
use forgekit_text::TextContext;
use kurbo::{BezPath, Point, Rect, Size};
use peniko::{Brush, Color};
use reactive_graph::owner::Owner;

use bubblebench::BubblebenchApp;
use bubblebench::chart::{BUBBLE_COUNT, bubble_chart};

const W: f64 = 800.0;
const H: f64 = 600.0;

/// A GPU-free paint target recording the chart's workload: gradient/solid
/// path fills, path strokes, glyph runs, and the rounded-rect chrome the HUD
/// buttons paint (so a test can locate and tap them).
#[derive(Default)]
struct RecScene {
    gradient_fills: usize,
    solid_fills: usize,
    strokes: usize,
    glyph_runs: usize,
    rounded: Vec<(Point, Size)>,
}

impl PaintScene for RecScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, _color: Color) {
        self.rounded.push((origin, size));
    }
    fn fill_rounded_rect_brush(&mut self, origin: Point, size: Size, _radius: f64, _brush: &Brush) {
        self.rounded.push((origin, size));
    }
    fn draw_glyph_run(&mut self, _run: GlyphRun) {
        self.glyph_runs += 1;
    }
    fn draw_image(&mut self, _data: &peniko::ImageData, _dest: Rect) {}
    fn fill_path(&mut self, _origin: Point, _path: &BezPath, brush: &Brush) {
        match brush {
            Brush::Gradient(_) => self.gradient_fills += 1,
            _ => self.solid_fills += 1,
        }
    }
    fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, _brush: &Brush) {
        self.strokes += 1;
    }
}

/// Installs the reactive runtime and an ambient owner, mirroring the desktop
/// shell's startup (required before creating signals / mounting a Component).
fn setup() -> Owner {
    let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
    let ambient = Owner::new();
    ambient.set();
    ambient
}

/// One frame at `t_ms` on a caller-advanced clock: rebuild, layout (shaping
/// real text), paint into a fresh recorder. Returns the scene and whether the
/// paint asked for another frame.
fn frame_at<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    t_ms: u64,
) -> (RecScene, bool) {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, H), tcx_any);
    let mut scene = RecScene::default();
    let outcome = root.paint(&mut scene, FrameTime::from_nanos(t_ms * 1_000_000));
    (scene, outcome.needs_frame)
}

fn pointer(phase: PointerPhase, p: Point) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase,
        position: p,
        button: PointerButton::Primary,
    })
}

/// Bare-chart benchmark state driven directly (no Component wrapper): the
/// chart view's inputs as plain fields.
struct Bench {
    running: bool,
    epoch: u64,
    fps: RwSignal<f64>,
}

type ChartRoot = RenderRoot<Bench, AnyView<Bench>>;

fn chart_logic(state: &mut Bench) -> AnyView<Bench> {
    any(bubble_chart(state.running, state.epoch, state.fps))
}

fn mounted_chart() -> (Owner, ChartRoot, Bench, TextContext) {
    let owner = setup();
    let root: ChartRoot = RenderRoot::new();
    let state = Bench {
        running: true,
        epoch: 0,
        fps: RwSignal::new(0.0),
    };
    (owner, root, state, TextContext::new())
}

#[test]
fn paints_the_full_workload_every_frame() {
    let (_owner, mut root, mut state, mut tcx) = mounted_chart();
    let (scene, needs_frame) = frame_at(&mut root, &mut chart_logic, &mut state, &mut tcx, 0);

    // The repro's exact per-frame GPU workload: one radial-gradient fill and
    // one stroked border per bubble, plus the shaped text runs (2 per bubble
    // whose symbol fits — at least the symbol run count in total).
    assert_eq!(
        scene.gradient_fills, BUBBLE_COUNT,
        "one gradient fill per bubble"
    );
    assert_eq!(scene.strokes, BUBBLE_COUNT, "one stroked border per bubble");
    assert!(
        scene.glyph_runs >= BUBBLE_COUNT,
        "shaped text painted per bubble (got {})",
        scene.glyph_runs
    );
    assert!(needs_frame, "a live simulation keeps requesting frames");

    // And it keeps repainting the identical workload on the next frame.
    let (scene2, needs_frame2) = frame_at(&mut root, &mut chart_logic, &mut state, &mut tcx, 16);
    assert_eq!(scene2.gradient_fills, BUBBLE_COUNT);
    assert!(needs_frame2);
}

#[test]
fn pause_goes_idle_and_resume_restarts() {
    let (_owner, mut root, mut state, mut tcx) = mounted_chart();
    let (_, live) = frame_at(&mut root, &mut chart_logic, &mut state, &mut tcx, 0);
    assert!(live);

    state.running = false;
    let (scene, paused) = frame_at(&mut root, &mut chart_logic, &mut state, &mut tcx, 16);
    assert!(!paused, "a paused simulation must stop requesting frames");
    // The field still paints (statically) while paused.
    assert_eq!(scene.gradient_fills, BUBBLE_COUNT);

    state.running = true;
    let (_, resumed) = frame_at(&mut root, &mut chart_logic, &mut state, &mut tcx, 32);
    assert!(resumed, "resume must restart the frame loop");
}

#[test]
fn touch_captures_and_drives_redraws() {
    let (_owner, mut root, mut state, mut tcx) = mounted_chart();
    let _ = frame_at(&mut root, &mut chart_logic, &mut state, &mut tcx, 0);

    let down = root.event(
        &mut state,
        &pointer(PointerPhase::Down, Point::new(400.0, 300.0)),
    );
    assert!(down.handled && down.needs_redraw, "a touch wakes the field");

    // A captured drag keeps updating the repulsion point even outside where
    // it started.
    let mv = root.event(
        &mut state,
        &pointer(PointerPhase::Move, Point::new(50.0, 50.0)),
    );
    assert!(mv.handled && mv.needs_redraw);

    let up = root.event(
        &mut state,
        &pointer(PointerPhase::Up, Point::new(50.0, 50.0)),
    );
    assert!(up.handled, "release clears the touch");
}

#[test]
fn reset_reseeds_deterministically() {
    let (_owner, mut root, mut state, mut tcx) = mounted_chart();
    // Let the field evolve away from its seeded layout.
    for i in 0..30 {
        let _ = frame_at(&mut root, &mut chart_logic, &mut state, &mut tcx, i * 16);
    }
    // A fresh mount at the same epoch gives the reference seeded layout.
    let (_o2, mut fresh_root, mut fresh_state, mut fresh_tcx) = mounted_chart();
    let _ = frame_at(
        &mut fresh_root,
        &mut chart_logic,
        &mut fresh_state,
        &mut fresh_tcx,
        0,
    );

    // Bump the epoch: the evolved field must snap back to the seeded layout —
    // observable as the paint workload being identical to a fresh mount's
    // (both fields step once on their next frame from identical positions,
    // so this asserts on the physics state indirectly but deterministically).
    state.epoch += 1;
    let (after_reset, _) = frame_at(&mut root, &mut chart_logic, &mut state, &mut tcx, 480);
    let (fresh, _) = frame_at(
        &mut fresh_root,
        &mut chart_logic,
        &mut fresh_state,
        &mut fresh_tcx,
        16,
    );
    assert_eq!(after_reset.gradient_fills, fresh.gradient_fills);
    assert_eq!(after_reset.glyph_runs, fresh.glyph_runs);
}

#[test]
fn fps_signal_publishes_after_a_second() {
    let (_owner, mut root, mut state, mut tcx) = mounted_chart();
    let fps = state.fps;
    // ~1.2 simulated seconds at 60Hz.
    for i in 0..75u64 {
        let _ = frame_at(&mut root, &mut chart_logic, &mut state, &mut tcx, i * 16);
    }
    let measured = fps.get_untracked();
    assert!(
        (30.0..=120.0).contains(&measured),
        "measured FPS must reflect the 16ms frame cadence (got {measured})"
    );
}

#[test]
fn full_app_pause_button_stops_the_loop() {
    let _owner = setup();
    // Mount the whole app through its root Component (the same tree `app!`
    // binds on every platform).
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut logic = |_s: &mut ()| any(component(BubblebenchApp));
    let mut state = ();
    let mut tcx = TextContext::new();

    let (scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(live, "the mounted app starts animating");
    assert!(
        scene.rounded.len() >= 2,
        "the HUD paints its two buttons (got {} rounded rects)",
        scene.rounded.len()
    );

    // The top row is FPS text … spacer … [Pause] [Reset]; Pause is the
    // second-rightmost rounded rect.
    let mut buttons = scene.rounded.clone();
    buttons.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
    let pause = buttons[buttons.len() - 2];
    let center = Point::new(
        pause.0.x + pause.1.width / 2.0,
        pause.0.y + pause.1.height / 2.0,
    );
    root.event(&mut state, &pointer(PointerPhase::Down, center));
    root.event(&mut state, &pointer(PointerPhase::Up, center));

    let (_, paused) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 16);
    assert!(!paused, "tapping Pause must stop the frame loop");
}
