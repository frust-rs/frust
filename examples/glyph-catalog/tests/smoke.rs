//! Headless smoke test: drive the real `RenderRoot` rebuild→layout→paint seam
//! (the desktop shell's own pipeline) against a recording paint target — no
//! GPU, no window — mirroring `examples/bubblebench/tests/bench.rs`'s harness
//! shape.
//!
//! These tests prove the shell scaffold and every stub section page mount,
//! lay out, and paint without panicking — the `c01` acceptance gate. They do
//! NOT assert any section's eventual content (that lands with each fill task);
//! a stub paints only its `"… filled by cXX"` placeholder text.

use std::any::Any;

use frust::{AnyView, any, component};
use frust_core::FrameTime;
use frust_core::{PaintScene, RenderRoot, View};
use frust_reactive::ReactiveRuntime;
use frust_scene::GlyphRun;
use frust_text::TextContext;
use kurbo::{BezPath, Point, Rect, Size};
use peniko::{Brush, Color};
use reactive_graph::owner::Owner;

use glyphcatalog::pages::{self, SECTION_LABELS};
use glyphcatalog::{CatalogApp, CatalogState};

const W: f64 = 900.0;
const H: f64 = 700.0;

/// A GPU-free paint target recording just enough of the workload to prove a
/// tree painted: shaped glyph runs, rounded-rect chrome (button/tab
/// backgrounds), and solid/gradient path fills.
#[derive(Default)]
struct RecScene {
    glyph_runs: usize,
    rounded: usize,
    fills: usize,
}

impl PaintScene for RecScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {
        self.fills += 1;
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn fill_rounded_rect(&mut self, _origin: Point, _size: Size, _radius: f64, _color: Color) {
        self.rounded += 1;
    }
    fn fill_rounded_rect_brush(
        &mut self,
        _origin: Point,
        _size: Size,
        _radius: f64,
        _brush: &Brush,
    ) {
        self.rounded += 1;
    }
    fn draw_glyph_run(&mut self, _run: GlyphRun) {
        self.glyph_runs += 1;
    }
    fn draw_image(&mut self, _data: &peniko::ImageData, _dest: Rect) {}
    fn fill_path(&mut self, _origin: Point, _path: &BezPath, _brush: &Brush) {
        self.fills += 1;
    }
    fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, _brush: &Brush) {}
}

/// Installs the reactive runtime and an ambient owner, mirroring the desktop
/// shell's startup (required before creating signals / mounting a Component).
fn setup() -> Owner {
    let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
    let ambient = Owner::new();
    ambient.set();
    ambient
}

/// One frame: rebuild, layout (shaping real text), paint into a fresh recorder.
fn frame_at<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    t_ms: u64,
) -> RecScene {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, H), tcx_any);
    let mut scene = RecScene::default();
    root.paint(&mut scene, FrameTime::from_nanos(t_ms * 1_000_000));
    scene
}

#[test]
fn every_stub_page_mounts_and_paints() {
    let _owner = setup();
    let mut tcx = TextContext::new();

    for (section, label) in SECTION_LABELS.iter().enumerate() {
        let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
        let mut state = CatalogState::new();
        let mut logic = |s: &mut CatalogState| pages::current(section, s);
        let scene = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
        assert!(
            scene.glyph_runs > 0,
            "section {section} ({label}) stub must paint its placeholder text",
        );
    }
}

#[test]
fn out_of_range_section_falls_back() {
    let _owner = setup();
    let mut tcx = TextContext::new();
    let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
    let mut state = CatalogState::new();
    // A defensive out-of-range index must still mount (foundations fallback).
    let mut logic = |s: &mut CatalogState| pages::current(99, s);
    let scene = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(scene.glyph_runs > 0, "the fallback page paints");
}

#[test]
fn full_shell_mounts_with_header_tabs_and_body() {
    let _owner = setup();
    let mut tcx = TextContext::new();

    // Mount the whole app through its root Component — the same tree `app!`
    // binds on every platform: navigator → header + tabs + pattern-switched
    // section body + toast host.
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut logic = |_s: &mut ()| any(component(CatalogApp));
    let mut state = ();

    let scene = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);

    // The header title + two toggle labels + the seven tab labels + the
    // foundations stub text all shape glyph runs.
    assert!(
        scene.glyph_runs > 0,
        "the shell paints its header/tabs/body text (got {})",
        scene.glyph_runs,
    );
    // The two header toggle buttons paint rounded-rect backgrounds.
    assert!(
        scene.rounded >= 2,
        "the header paints its two toggle buttons (got {} rounded rects)",
        scene.rounded,
    );

    // A second frame keeps mounting cleanly (reconcile-in-place, no panic).
    let scene2 = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 16);
    assert!(scene2.glyph_runs > 0);
}
