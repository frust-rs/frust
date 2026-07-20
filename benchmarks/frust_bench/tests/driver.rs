//! Headless tests for the scenario driver + the two implemented scenarios
//! (S1, S7), driving Frust's `RenderRoot` directly (no GPU, no window) — the
//! same harness shape as `examples/bubblebench/tests/bench.rs`.

use std::any::Any;

use frust::{AnyView, any, component};
use frust_core::{FrameTime, RenderRoot, View};
use frust_reactive::ReactiveRuntime;
use frust_scene::GlyphRun;
use frust_text::TextContext;
use frustbench::BenchApp;
use frustbench::scenarios::{self, BenchState, SCENARIOS, s1_animation::chart::BUBBLE_COUNT};
use kurbo::{BezPath, Point, Rect, Size};
use peniko::{Brush, Color};
use reactive_graph::owner::Owner;

const W: f64 = 800.0;
const H: f64 = 600.0;

/// A GPU-free paint target recording the workload: gradient/solid path fills,
/// strokes, glyph runs, and rounded-rect chrome (the HUD/switcher buttons).
#[derive(Default)]
struct RecScene {
    gradient_fills: usize,
    solid_fills: usize,
    strokes: usize,
    glyph_runs: usize,
    rounded: Vec<(Point, Size)>,
}

impl frust_core::PaintScene for RecScene {
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

/// Installs the reactive runtime + an ambient owner (required before creating
/// signals / mounting a Component).
fn setup() -> Owner {
    let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
    let ambient = Owner::new();
    ambient.set();
    ambient
}

/// One frame at `t_ms`: rebuild, layout (shaping real text), paint into a fresh
/// recorder. Returns the scene and whether paint asked for another frame.
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

#[test]
fn registry_has_eight_scenarios_in_id_order() {
    let ids: Vec<&str> = SCENARIOS.iter().map(|s| s.id()).collect();
    assert_eq!(ids, ["s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8"]);
}

#[test]
fn deep_link_and_id_parsing() {
    assert_eq!(scenarios::index_from_id("s1"), Some(0));
    assert_eq!(scenarios::index_from_id("s7"), Some(6));
    assert_eq!(scenarios::index_from_id("s9"), None);

    assert_eq!(scenarios::index_from_url("frustbench://s1"), Some(0));
    assert_eq!(scenarios::index_from_url("frustbench://s7/idle"), Some(6));
    assert_eq!(scenarios::index_from_url("frustbench://s3?x=1"), Some(2));
    assert_eq!(scenarios::index_from_url("frustbench://nope"), None);
    assert_eq!(scenarios::index_from_url("other://s1"), None);
}

/// Drive one scenario's view over a `BenchState`, at a fixed index.
fn scenario_logic(idx: usize) -> impl FnMut(&mut BenchState) -> AnyView<BenchState> {
    move |state: &mut BenchState| SCENARIOS[idx].build(state)
}

#[test]
fn s1_paints_the_full_bubble_workload() {
    let _owner = setup();
    let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
    let mut state = BenchState::new();
    let mut tcx = TextContext::new();
    let mut logic = scenario_logic(0);

    let (scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert_eq!(
        scene.gradient_fills, BUBBLE_COUNT,
        "one gradient fill per bubble"
    );
    assert_eq!(scene.strokes, BUBBLE_COUNT, "one stroked border per bubble");
    assert!(scene.glyph_runs >= BUBBLE_COUNT, "shaped text per bubble");
    assert!(live, "a live animation storm keeps requesting frames");
}

#[test]
fn s7_is_static_and_paints_text() {
    let _owner = setup();
    let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
    let mut state = BenchState::new();
    let mut tcx = TextContext::new();
    let mut logic = scenario_logic(6); // s7

    let (scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(
        scene.glyph_runs > 0,
        "S7 paints its informational text (frame-gate idle demo)"
    );
    assert!(
        !live,
        "S7 requests no frames after the first — the frame gate goes idle"
    );
}

#[test]
fn stub_scenario_renders_a_placeholder() {
    let _owner = setup();
    let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
    let mut state = BenchState::new();
    let mut tcx = TextContext::new();
    // s3 is still a compiling stub (task 04's scope); s2/s5 are implemented
    // by task 03 below.
    let mut logic = scenario_logic(2);

    let (scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(
        scene.glyph_runs > 0,
        "a stub paints its labeled placeholder"
    );
    assert!(!live, "a static placeholder requests no further frames");
}

#[test]
fn s2_long_list_scrolls_and_keeps_requesting_frames() {
    let _owner = setup();
    let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
    let mut state = BenchState::new();
    let mut tcx = TextContext::new();
    let mut logic = scenario_logic(1); // s2

    let (scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(
        scene.glyph_runs > 0,
        "S2 paints row title/subtitle text for the materialized window"
    );
    assert!(
        live,
        "S2's scripted auto-scroll keeps requesting frames (see s2_list's module doc)"
    );
}

#[test]
fn s5_image_pipeline_keeps_requesting_frames() {
    let _owner = setup();
    let mut root: RenderRoot<BenchState, AnyView<BenchState>> = RenderRoot::new();
    let mut state = BenchState::new();
    let mut tcx = TextContext::new();
    let mut logic = scenario_logic(4); // s5

    let (_scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(
        live,
        "S5's scripted auto-scroll keeps requesting frames while decodes stream in"
    );
}

#[test]
fn full_app_mounts_with_scenario_switcher() {
    let _owner = setup();
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut logic = |_s: &mut ()| any(component(BenchApp));
    let mut state = ();
    let mut tcx = TextContext::new();

    let (scene, live) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(live, "the mounted app starts on S1 (animating)");
    // S1's own HUD paints two buttons (Pause/Reset) plus the eight-scenario
    // switcher row: at least ten rounded-rect buttons in total.
    assert!(
        scene.rounded.len() >= SCENARIOS.len() + 2,
        "the switcher paints one button per scenario plus S1's HUD (got {})",
        scene.rounded.len()
    );
}
