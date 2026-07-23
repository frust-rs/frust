//! T0 scene-construction tests (per `docs/TESTING.md`): pure plan/geometry
//! assertions plus GPU-free command-count checks driving the real
//! `RenderRoot` rebuild->layout->paint seam against a recording paint target
//! — no GPU, no window — the same harness shape as
//! `examples/shadertoy/tests/showcase.rs`.
//!
//! These prove the bench *constructs* the scenes it claims to (scene A's
//! vector command counts, scene B's [`NUM_LAYERS`] image quads + live spinner,
//! scene C's one live shader layer). GPU cost is the conductor's on-device
//! measurement, not tested here.

use std::any::Any;

use frust::{AnyView, View, any};
use frust_core::{FrameTime, PaintScene, RenderRoot};
use frust_reactive::ReactiveRuntime;
use frust_scene::{GlyphRun, ShaderProgram};
use frust_text::TextContext;
use kurbo::{BezPath, Point, Rect, Size};
use peniko::{Brush, Color, ImageData};
use reactive_graph::owner::Owner;

use layerbench::bench_view::bench_view;
use layerbench::scene::{self, Mode, NUM_LAYERS};

const W: f64 = 400.0;
const H: f64 = 800.0;
// Small DPR in tests keeps the offscreen buffers tiny (no ~10MB allocs).
const DPR: f64 = 1.0;

/// A GPU-free paint target recording the bench's per-mode workload.
#[derive(Default)]
struct RecScene {
    fill_rects: usize,
    rounded_rects: usize,
    glyph_runs: usize,
    paths: usize,
    images: usize,
    shaders: usize,
}

impl PaintScene for RecScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {
        self.fill_rects += 1;
    }
    fn fill_rounded_rect(&mut self, _origin: Point, _size: Size, _radius: f64, _color: Color) {
        self.rounded_rects += 1;
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn draw_glyph_run(&mut self, _run: GlyphRun) {
        self.glyph_runs += 1;
    }
    fn fill_path(&mut self, _origin: Point, _path: &BezPath, _brush: &Brush) {
        self.paths += 1;
    }
    fn draw_image(&mut self, _data: &ImageData, _dest: Rect) {
        self.images += 1;
    }
    fn draw_shader(&mut self, _program: &ShaderProgram, _dest: Rect, _time: f32) {
        self.shaders += 1;
    }
}

fn setup() -> Owner {
    let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
    let ambient = Owner::new();
    ambient.set();
    ambient
}

/// One frame at `t_ms`: rebuild, layout (shaping real text), paint into a fresh
/// recorder. Returns the recorded scene and whether paint asked for another
/// frame.
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

/// Paint a single mode once and return the recorded command counts.
fn paint_mode(mode: Mode) -> (RecScene, bool) {
    let _owner = setup();
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut tcx = TextContext::new();
    let mut logic = move |_s: &mut ()| any(bench_view(mode, DPR));
    let mut state = ();
    frame_at(&mut root, &mut logic, &mut state, &mut tcx, 16)
}

// ---------------------------------------------------------------------
// Pure plan / geometry.
// ---------------------------------------------------------------------

#[test]
fn plan_command_counts_match_the_feedback_calibration() {
    let counts = scene::plan(Size::new(W, H)).counts();
    // Calibrated against examples/glyph-catalog/src/pages/feedback.rs:
    // 20 rounded rects (5 badges + 3 tags + 4 alert cards + 5 toast buttons +
    // progress track + progress fill + skeleton), 6 status/loader dots, 33
    // shaped labels (section titles + badge/tag/alert/toast/loader text), 4
    // alert icon paths.
    assert_eq!(counts.rounded_rects, 20, "rounded rects");
    assert_eq!(counts.dots, 6, "status/loader dots");
    assert_eq!(counts.glyph_labels, 33, "shaped labels");
    assert_eq!(counts.icon_paths, 4, "alert icon paths");
}

#[test]
fn plan_is_deterministic_for_a_given_size() {
    let a = scene::plan(Size::new(W, H)).counts();
    let b = scene::plan(Size::new(W, H)).counts();
    assert_eq!(a, b);
}

#[test]
fn layer_bands_tile_the_full_height_without_gaps() {
    let bands = scene::layer_bands(Size::new(W, H));
    assert_eq!(bands.len(), NUM_LAYERS);
    assert_eq!(bands[0].y0, 0.0);
    assert!(
        (bands[NUM_LAYERS - 1].y1 - H).abs() < 1e-9,
        "last band reaches the bottom"
    );
    for pair in bands.windows(2) {
        assert!(
            (pair[0].y1 - pair[1].y0).abs() < 1e-9,
            "bands are contiguous"
        );
        assert_eq!(pair[0].x0, 0.0);
        assert_eq!(pair[0].x1, W);
    }
}

#[test]
fn layer_memory_is_the_sum_of_the_physical_band_textures() {
    let dpr = 3.0;
    let bytes = scene::layer_memory_bytes(Size::new(W, H), dpr);
    // NUM_LAYERS bands tiling one screen at 3x ≈ one full-screen RGBA8 layer.
    let expected: u64 = scene::layer_bands(Size::new(W, H))
        .into_iter()
        .map(|b| {
            let (w, h) = scene::layer_texture_dims(b, dpr);
            w as u64 * h as u64 * 4
        })
        .sum();
    assert_eq!(bytes, expected);
    assert!(bytes > 0);
}

#[test]
fn layer_texture_dims_clamp_to_the_vello_atlas_cap() {
    // A pathologically tall band at a large dpr must clamp to 8192, mirroring
    // shader_effects::clamp_size.
    let band = Rect::new(0.0, 0.0, 5000.0, 5000.0);
    let (w, h) = scene::layer_texture_dims(band, 4.0);
    assert_eq!(w, 8192);
    assert_eq!(h, 8192);
}

// ---------------------------------------------------------------------
// Mode parsing / cycle.
// ---------------------------------------------------------------------

#[test]
fn mode_parse_maps_scene_letters() {
    assert_eq!(Mode::parse("a"), Mode::Vector);
    assert_eq!(Mode::parse("B"), Mode::Composite);
    assert_eq!(Mode::parse("c"), Mode::CompositeRelayer);
    assert_eq!(Mode::parse("relayer"), Mode::CompositeRelayer);
    assert_eq!(Mode::parse("garbage"), Mode::Vector);
}

#[test]
fn mode_cycle_is_a_to_b_to_relayer_to_a() {
    assert_eq!(Mode::Vector.next(), Mode::Composite);
    assert_eq!(Mode::Composite.next(), Mode::CompositeRelayer);
    assert_eq!(Mode::CompositeRelayer.next(), Mode::Vector);
}

// ---------------------------------------------------------------------
// Painted command counts per mode (the real RenderRoot paint seam).
// ---------------------------------------------------------------------

#[test]
fn scene_a_paints_the_full_vector_content_and_no_composited_quads() {
    let (rec, needs_frame) = paint_mode(Mode::Vector);
    // Rounded rects: 20 plan rects + 6 dots (dots render as rounded rects).
    assert_eq!(rec.rounded_rects, 26, "vector rounded rects (rects + dots)");
    assert_eq!(rec.paths, 4, "alert icon paths");
    assert!(
        rec.glyph_runs >= 33,
        "at least one glyph run per shaped label"
    );
    assert_eq!(rec.images, 0, "scene A composites no textures");
    assert_eq!(rec.shaders, 0, "scene A renders no shader quads");
    assert_eq!(rec.fill_rects, 1, "one full-screen background fill");
    assert!(needs_frame, "the bench is a perpetual animator");
}

#[test]
fn scene_b_composites_num_layers_image_quads_plus_a_live_spinner() {
    let (rec, needs_frame) = paint_mode(Mode::Composite);
    assert_eq!(rec.images, NUM_LAYERS, "one image quad per cached layer");
    assert_eq!(rec.shaders, 0, "scene B re-renders nothing");
    assert_eq!(rec.glyph_runs, 0, "no vector text in the composite scene");
    // The live spinner is 8 orbiting dots (rounded rects).
    assert_eq!(rec.rounded_rects, 8, "the live spinner's 8 dots");
    assert_eq!(rec.fill_rects, 1, "background fill");
    assert!(needs_frame, "continuous frames in every mode");
}

#[test]
fn scene_c_swaps_one_image_quad_for_a_live_shader_layer() {
    let (rec, needs_frame) = paint_mode(Mode::CompositeRelayer);
    assert_eq!(rec.shaders, 1, "exactly one live re-rendered layer");
    assert_eq!(
        rec.images,
        NUM_LAYERS - 1,
        "the rest stay cached image quads"
    );
    assert_eq!(rec.rounded_rects, 8, "the live spinner still paints");
    assert!(needs_frame, "continuous frames in every mode");
}
