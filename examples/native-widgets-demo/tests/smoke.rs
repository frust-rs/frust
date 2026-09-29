//! Headless smoke test: drive the real `RenderRoot` rebuild→layout→paint seam
//! (the desktop shell's own pipeline) against a recording paint target — no
//! GPU, no window — the same harness shape as `examples/playground`'s
//! `tests/smoke.rs`.
//!
//! These tests prove the shell and every section page mount, lay out, and
//! paint without panicking at two viewport sizes and both brightnesses — the
//! coverage gate. They assert structural paint output (text runs were shaped,
//! no panic), not pixel-exact content; what every page here exists to show
//! (real OS controls beside their frust-drawn peers) is a separate, human-run
//! device gate this suite cannot replace — see `README.md`.

use std::any::Any;

use frust::{AnyView, Brightness, Theme, any, component, provide_context};
use frust_core::FrameTime;
use frust_core::{PaintOutcome, PaintScene, RenderRoot, View};
use frust_reactive::ReactiveRuntime;
use frust_scene::GlyphRun;
use frust_text::TextContext;
use kurbo::{BezPath, Point, Rect, Size};
use peniko::{Brush, Color};
use reactive_graph::owner::Owner;

use native_widgets_demo::pages;
use native_widgets_demo::{NativeWidgetsDemoApp, NativeWidgetsDemoState, SECTION_LABELS};

const W: f64 = 900.0;
const H: f64 = 700.0;

/// Two viewport sizes the coverage sweep lays every page out at: a phone-ish
/// portrait size and a desktop-ish landscape size.
const SIZES: [(f64, f64); 2] = [(390.0, 844.0), (W, H)];

/// Both brightnesses the coverage sweep checks — every page's colors resolve
/// from the live theme, so sweeping both is what exercises that contract.
const BRIGHTNESSES: [Brightness; 2] = [Brightness::Dark, Brightness::Light];

/// A GPU-free paint target recording just enough of the workload to prove a
/// tree painted: shaped text runs (with their solid colors), rounded-rect
/// chrome, and solid/gradient fills.
#[derive(Default)]
struct RecScene {
    text_runs: usize,
    /// Every text run's resolved solid brush color — a hardcode tripwire: a
    /// page whose text colors are theme-resolved paints DIFFERENT color sets
    /// under Dark vs Light.
    text_colors: Vec<peniko::Color>,
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
            self.text_colors.push(c);
        }
        self.text_runs += 1;
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
/// [`Brightness`] — mirroring a shell's own theme push. The caller must ALSO
/// thread the theme into its `RenderRoot` (`root.set_theme(..)`): widget
/// internals resolve colors from the layout/paint theme, not the context.
fn setup_with_theme(brightness: Brightness) -> (Owner, Theme) {
    let owner = setup();
    let theme = Theme::builder(frust_glyph::baseline())
        .brightness(brightness)
        .build();
    provide_context(theme.clone());
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
) -> (RecScene, PaintOutcome) {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(size, tcx_any);
    let mut scene = RecScene::default();
    let outcome = root.paint(&mut scene, FrameTime::from_nanos(t_ms * 1_000_000));
    (scene, outcome)
}

/// One frame at the module's default `(W, H)` size.
fn frame_at<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    t_ms: u64,
) -> (RecScene, PaintOutcome) {
    frame_at_size(root, logic, state, tcx, Size::new(W, H), t_ms)
}

#[test]
fn every_page_mounts_and_paints() {
    let _owner = setup();
    let mut tcx = TextContext::new();

    for (section, label) in SECTION_LABELS.iter().enumerate() {
        let mut root: RenderRoot<NativeWidgetsDemoState, AnyView<NativeWidgetsDemoState>> =
            RenderRoot::new();
        let mut state = NativeWidgetsDemoState::new();
        let mut logic = |s: &mut NativeWidgetsDemoState| pages::current(section, s);
        let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
        assert!(
            scene.text_runs > 0,
            "section {section} ({label}) must paint some text",
        );
    }
}

/// The coverage gate: every section page builds/lays out/paints headless at
/// two viewport sizes and both brightnesses, then survives a second
/// (reconcile-in-place) frame.
#[test]
fn every_page_mounts_at_every_size_and_brightness() {
    for brightness in BRIGHTNESSES {
        for (section, label) in SECTION_LABELS.iter().enumerate() {
            for (w, h) in SIZES {
                let (_owner, theme) = setup_with_theme(brightness);
                let mut tcx = TextContext::new();
                let mut root: RenderRoot<NativeWidgetsDemoState, AnyView<NativeWidgetsDemoState>> =
                    RenderRoot::new();
                root.set_theme(Box::new(theme));
                let mut state = NativeWidgetsDemoState::new();
                let mut logic = |s: &mut NativeWidgetsDemoState| pages::current(section, s);
                let (scene, _outcome) = frame_at_size(
                    &mut root,
                    &mut logic,
                    &mut state,
                    &mut tcx,
                    Size::new(w, h),
                    0,
                );
                assert!(
                    scene.text_runs > 0,
                    "section {section} ({label}) must paint text at {w}x{h} under {brightness:?}",
                );

                let (scene2, _outcome2) = frame_at_size(
                    &mut root,
                    &mut logic,
                    &mut state,
                    &mut tcx,
                    Size::new(w, h),
                    16,
                );
                assert!(
                    scene2.text_runs > 0,
                    "section {section} ({label}) must still paint on a second frame at {w}x{h} \
                     under {brightness:?}",
                );
            }
        }
    }
}

/// The brightness sweep must verify COLOR values, not just paint counts — a
/// page that hardcoded its text colors would paint an identical color set
/// under both brightnesses.
#[test]
fn page_text_colors_track_brightness() {
    use std::collections::BTreeSet;
    let (w, h) = SIZES[0];
    for (section, label) in SECTION_LABELS.iter().enumerate() {
        let mut palettes: Vec<BTreeSet<[u8; 4]>> = Vec::new();
        for brightness in BRIGHTNESSES {
            let (_owner, theme) = setup_with_theme(brightness);
            let mut tcx = TextContext::new();
            let mut root: RenderRoot<NativeWidgetsDemoState, AnyView<NativeWidgetsDemoState>> =
                RenderRoot::new();
            root.set_theme(Box::new(theme));
            let mut state = NativeWidgetsDemoState::new();
            let mut logic = |s: &mut NativeWidgetsDemoState| pages::current(section, s);
            let (scene, _outcome) = frame_at_size(
                &mut root,
                &mut logic,
                &mut state,
                &mut tcx,
                Size::new(w, h),
                0,
            );
            palettes.push(
                scene
                    .text_colors
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
    let mut root: RenderRoot<NativeWidgetsDemoState, AnyView<NativeWidgetsDemoState>> =
        RenderRoot::new();
    let mut state = NativeWidgetsDemoState::new();
    let mut logic = |s: &mut NativeWidgetsDemoState| pages::current(99, s);
    let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(scene.text_runs > 0, "the fallback page paints");
}

/// Mounts the whole app through its root Component — the same tree `app!`
/// binds on every platform (navigator → app bar + pattern-switched section
/// body + bottom section navigation) — at both viewport sizes.
#[test]
fn full_shell_mounts_with_app_bar_body_and_section_nav() {
    for (w, h) in SIZES {
        let _owner = setup();
        let mut tcx = TextContext::new();
        let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
        let mut logic = |_s: &mut ()| any(component(NativeWidgetsDemoApp));
        let mut state = ();

        let size = Size::new(w, h);
        let (scene, _outcome) = frame_at_size(&mut root, &mut logic, &mut state, &mut tcx, size, 0);

        // App-bar title, one label per section button, and the first page's
        // own text all shape runs.
        assert!(
            scene.text_runs > SECTION_LABELS.len() + 1,
            "the shell paints its app bar, every section button and the page body at {w}x{h} \
             (got {} text runs)",
            scene.text_runs,
        );
        // The active section button (ButtonStyle::Primary) paints a rounded
        // fill; the Ghost buttons and the resting icon button are transparent.
        assert!(
            scene.rounded >= 1,
            "the active section button paints its fill at {w}x{h} (got {} rounded rects)",
            scene.rounded,
        );

        // A second frame keeps mounting cleanly (reconcile-in-place, no panic).
        let (scene2, _outcome2) =
            frame_at_size(&mut root, &mut logic, &mut state, &mut tcx, size, 16);
        assert!(scene2.text_runs > 0);
    }
}
