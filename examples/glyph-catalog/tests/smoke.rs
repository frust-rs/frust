//! Headless smoke test: drive the real `RenderRoot` rebuild→layout→paint seam
//! (the desktop shell's own pipeline) against a recording paint target — no
//! GPU, no window — mirroring `examples/bubblebench/tests/bench.rs`'s harness
//! shape.
//!
//! These tests prove the shell scaffold and every section page (now all
//! filled by `c02`-`c08`, not stubs) mount, lay out, and paint without
//! panicking at multiple viewport sizes and both Glyph brightnesses — the
//! `c09` coverage gate. They assert structural paint output (glyph runs were
//! shaped, no panic), not pixel-exact content — see `README.md`'s coverage
//! table and `research/INVENTORY.md` for the manual/visual gate this test
//! suite cannot replace.

use std::any::Any;

use frust::{AnyView, Brightness, Theme, any, component, provide_context};
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

/// Two viewport sizes the coverage sweep below lays every real page out at: a
/// phone-ish portrait size and a desktop-ish landscape size (matching the
/// module-level `W`/`H` constant the other tests already use).
const SIZES: [(f64, f64); 2] = [(390.0, 844.0), (W, H)];

/// Both Glyph brightnesses the coverage sweep checks — every page's live
/// swatches/specimens re-resolve from `use_context::<Theme>()`, so a page
/// that only reads a hardcoded color would still "pass" a single-brightness
/// smoke test; sweeping both is what actually exercises that contract.
const BRIGHTNESSES: [Brightness; 2] = [Brightness::Dark, Brightness::Light];

/// A GPU-free paint target recording just enough of the workload to prove a
/// tree painted: shaped glyph runs, rounded-rect chrome (button/tab
/// backgrounds), and solid/gradient path fills.
#[derive(Default)]
struct RecScene {
    glyph_runs: usize,
    /// Every glyph run's resolved solid brush color — the round-1 review's
    /// hardcode tripwire: a page whose text colors are theme-resolved paints
    /// DIFFERENT color sets under Dark vs Light; a hardcoded-only page paints
    /// identical sets and fails `page_text_colors_track_brightness`.
    glyph_colors: Vec<peniko::Color>,
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
    fn draw_glyph_run(&mut self, run: GlyphRun) {
        if let peniko::Brush::Solid(c) = run.brush {
            self.glyph_colors.push(c);
        }
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

/// Like [`setup`], but also `provide_context`s a Glyph [`Theme`] at the given
/// [`Brightness`] under the fresh ambient owner — mirroring a shell's own
/// `provide_context(theme.clone())` push (`docs/ARCHITECTURE.md`'s Theme
/// delivery, the app-code half). Every page reads `use_context::<Theme>()`
/// (falling back to `Theme::glyph_baseline()` with no context), so this is
/// what lets the multi-brightness sweep below actually exercise each page's
/// live-token re-resolution rather than only ever hitting the fallback.
fn setup_with_theme(brightness: Brightness) -> (Owner, Theme) {
    let owner = setup();
    let theme = Theme::builder(Theme::glyph_baseline())
        .brightness(brightness)
        .build();
    provide_context(theme.clone());
    // The caller must ALSO thread the theme into its RenderRoot
    // (`root.set_theme(Box::new(theme))`): `provide_context` only serves
    // app-code `use_context` reads; every widget-internal color resolves from
    // the LayoutCtx/PaintCtx theme the shell threads via `set_theme`
    // (docs/ARCHITECTURE.md's Theme delivery). Round-1 review: without this
    // the brightness sweep silently exercised unthemed dark fallbacks.
    (owner, theme)
}

/// One frame at an explicit viewport `size`: rebuild, layout (shaping real
/// text), paint into a fresh recorder.
fn frame_at_size<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    size: Size,
    t_ms: u64,
) -> RecScene {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(size, tcx_any);
    let mut scene = RecScene::default();
    root.paint(&mut scene, FrameTime::from_nanos(t_ms * 1_000_000));
    scene
}

/// One frame at the module's default `(W, H)` size — the shape every
/// pre-existing test in this file used before the multi-size sweep below.
fn frame_at<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    t_ms: u64,
) -> RecScene {
    frame_at_size(root, logic, state, tcx, Size::new(W, H), t_ms)
}

#[test]
fn every_page_mounts_and_paints() {
    let _owner = setup();
    let mut tcx = TextContext::new();

    for (section, label) in SECTION_LABELS.iter().enumerate() {
        let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
        let mut state = CatalogState::new();
        let mut logic = |s: &mut CatalogState| pages::current(section, s);
        let scene = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
        assert!(
            scene.glyph_runs > 0,
            "section {section} ({label}) must paint some text",
        );
    }
}

/// The `c09` coverage gate: every REAL section page (all seven, filled by
/// `c02`-`c08` — no stubs remain) builds/lays out/paints headless at 2
/// viewport sizes and both Glyph brightnesses, with no GPU. This is the
/// automated half of the coverage story `README.md`'s table documents; it
/// proves structural soundness (mounts, lays out, paints real content) at
/// every combination, not pixel-exact fidelity to the reference builds (that
/// remains the manual/visual gate — see README.md and this task's Testing
/// Performed notes).
#[test]
fn every_page_mounts_at_every_size_and_brightness() {
    for brightness in BRIGHTNESSES {
        for (section, label) in SECTION_LABELS.iter().enumerate() {
            for (w, h) in SIZES {
                // Fresh owner + theme context per case: a page's Component
                // (buttons_forms/feedback/content/overlays/motion each nest
                // one for local demo state) must tolerate being mounted fresh
                // under any size/brightness combination, matching how the
                // section pattern_switcher actually tears down and rebuilds
                // a page's subtree on every section change.
                let (_owner, theme) = setup_with_theme(brightness);
                let mut tcx = TextContext::new();
                let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
                root.set_theme(Box::new(theme));
                let mut state = CatalogState::new();
                let mut logic = |s: &mut CatalogState| pages::current(section, s);
                let scene = frame_at_size(
                    &mut root,
                    &mut logic,
                    &mut state,
                    &mut tcx,
                    Size::new(w, h),
                    0,
                );
                assert!(
                    scene.glyph_runs > 0,
                    "section {section} ({label}) must paint text at {w}x{h} under {brightness:?}",
                );

                // A second frame proves reconcile-in-place also survives at
                // this size/brightness (not just first mount) — mirrors the
                // full-shell test's own second-frame check below.
                let scene2 = frame_at_size(
                    &mut root,
                    &mut logic,
                    &mut state,
                    &mut tcx,
                    Size::new(w, h),
                    16,
                );
                assert!(
                    scene2.glyph_runs > 0,
                    "section {section} ({label}) must still paint on a second frame at {w}x{h} under {brightness:?}",
                );
            }
        }
    }
}

/// Round-1 review Major 2: the brightness sweep must actually verify COLOR
/// values, not just paint counts — a page that hardcoded its dark-mode text
/// colors would paint an identical color set under both brightnesses. Every
/// page carries at least one theme-resolved text role (headings resolve
/// `primary`, captions `on_surface_variant`), so the per-page painted color
/// SETS must differ between Dark and Light. (Brightness-INVARIANT ink — the
/// term block/tooltip GlyphInk roles on the content page — is allowed to
/// repeat across both sets; the assertion is set inequality, not disjointness.)
#[test]
fn page_text_colors_track_brightness() {
    use std::collections::BTreeSet;
    let (w, h) = SIZES[0];
    for (section, label) in SECTION_LABELS.iter().enumerate() {
        let mut palettes: Vec<BTreeSet<[u8; 4]>> = Vec::new();
        for brightness in BRIGHTNESSES {
            let (_owner, theme) = setup_with_theme(brightness);
            let mut tcx = TextContext::new();
            let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
            root.set_theme(Box::new(theme));
            let mut state = CatalogState::new();
            let mut logic = |s: &mut CatalogState| pages::current(section, s);
            let scene = frame_at_size(
                &mut root,
                &mut logic,
                &mut state,
                &mut tcx,
                Size::new(w, h),
                0,
            );
            palettes.push(
                scene
                    .glyph_colors
                    .iter()
                    .map(|c| c.to_rgba8().to_u8_array())
                    .collect(),
            );
        }
        assert_ne!(
            palettes[0], palettes[1],
            "section {section} ({label}): painted text-color set is identical under Dark and \
             Light — at least one theme-resolved role must differ (hardcoded-color regression)",
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
    // foundations page's own text all shape glyph runs.
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
