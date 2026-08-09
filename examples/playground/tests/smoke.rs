//! Headless smoke test: drive the real `RenderRoot` rebuild→layout→paint seam
//! (the desktop shell's own pipeline) against a recording paint target — no
//! GPU, no window — mirroring `examples/huddle`'s own headless `tests/*.rs`
//! harness shape.
//!
//! These tests prove the shell scaffold and every section page mount, lay out,
//! and paint without panicking at multiple viewport sizes and both
//! brightnesses — the coverage gate. They assert structural paint output (text
//! runs were shaped, no panic), not pixel-exact content; the on-device
//! behaviour every page here actually exists to exercise (a real
//! `platform_view` slot, a real camera session, real platform controls) is a
//! separate, human-run gate this suite cannot replace — see `README.md`.

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

use playground::pages::{self, SECTION_LABELS};
use playground::{PlaygroundApp, PlaygroundState};

const W: f64 = 900.0;
const H: f64 = 700.0;

/// Two viewport sizes the coverage sweep below lays every page out at: a
/// phone-ish portrait size and a desktop-ish landscape size (matching the
/// module-level `W`/`H` constants the other tests already use). The pair
/// straddles `pages::responsive`'s own 600px breakpoint, so the sweep
/// exercises both of that page's structural layouts.
const SIZES: [(f64, f64); 2] = [(390.0, 844.0), (W, H)];

/// Both brightnesses the coverage sweep checks — every page's live theme reads
/// re-resolve from `use_context::<Theme>()`, so a page that only read a
/// hardcoded color would still "pass" a single-brightness smoke test; sweeping
/// both is what actually exercises that contract.
const BRIGHTNESSES: [Brightness; 2] = [Brightness::Dark, Brightness::Light];

/// A GPU-free paint target recording just enough of the workload to prove a
/// tree painted: shaped text runs, rounded-rect chrome (button/nav-bar
/// backgrounds), and solid/gradient path fills.
#[derive(Default)]
struct RecScene {
    text_runs: usize,
    /// Every text run's resolved solid brush color — a hardcode tripwire: a
    /// page whose text colors are theme-resolved paints DIFFERENT color sets
    /// under Dark vs Light; a hardcoded-only page paints identical sets and
    /// fails `page_text_colors_track_brightness`.
    text_colors: Vec<peniko::Color>,
    rounded: usize,
    fills: usize,
    /// The absolute (canvas-space) block origin — [`GlyphRun::transform`]'s
    /// translation — of every painted glyph run, in paint order. Every line
    /// of one [`frust_text::TextLayout`] shares the SAME transform (the
    /// block's own origin; see `frust_text::convert`'s "glyphs on the same
    /// line share a y" contract and [`frust_text::TextLayout::to_scene_runs`]),
    /// and a wrapped label's lines each land in a SEPARATE
    /// `draw_glyph_run` call — so more than one recorded origin at the same
    /// (x, y) is the two-line-wrap tripwire
    /// `full_shell_nav_labels_fit_single_line_at_phone_width` checks for.
    text_run_origins: Vec<Point>,
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
        let t = run.transform.translation();
        self.text_run_origins.push(Point::new(t.x, t.y));
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

/// Like [`setup`], but also `provide_context`s a [`Theme`] at the given
/// [`Brightness`] under the fresh ambient owner — mirroring a shell's own
/// `provide_context(theme.clone())` push (`docs/ARCHITECTURE.md`'s Theme
/// delivery, the app-code half). Every page reads `use_context::<Theme>()`
/// (falling back to `Theme::m3_baseline()` with no context), so this is what
/// lets the multi-brightness sweep below actually exercise each page's
/// live-token re-resolution rather than only ever hitting the fallback.
fn setup_with_theme(brightness: Brightness) -> (Owner, Theme) {
    let owner = setup();
    let theme = Theme::builder(Theme::m3_baseline())
        .brightness(brightness)
        .build();
    provide_context(theme.clone());
    // The caller must ALSO thread the theme into its RenderRoot
    // (`root.set_theme(Box::new(theme))`): `provide_context` only serves
    // app-code `use_context` reads; every widget-internal color resolves from
    // the LayoutCtx/PaintCtx theme the shell threads via `set_theme`
    // (docs/ARCHITECTURE.md's Theme delivery).
    (owner, theme)
}

/// One frame at an explicit viewport `size`: rebuild, layout (shaping real
/// text), paint into a fresh recorder. Returns both the recorded scene and
/// the paint outcome (used for asserting frame-request behavior).
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
        let mut root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>> = RenderRoot::new();
        let mut state = PlaygroundState::new();
        let mut logic = |s: &mut PlaygroundState| pages::current(section, s);
        let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
        assert!(
            scene.text_runs > 0,
            "section {section} ({label}) must paint some text",
        );
    }
}

/// The coverage gate: every section page builds/lays out/paints headless at
/// two viewport sizes and both brightnesses, with no GPU. This proves
/// structural soundness (mounts, lays out, paints real content) at every
/// combination — the on-device behaviour stays a manual gate (see README.md).
#[test]
fn every_page_mounts_at_every_size_and_brightness() {
    for brightness in BRIGHTNESSES {
        for (section, label) in SECTION_LABELS.iter().enumerate() {
            for (w, h) in SIZES {
                // Fresh owner + theme context per case: a page's own nested
                // Component (the camera page mounts one) must tolerate being
                // mounted fresh under any size/brightness combination,
                // matching how the section pattern_switcher actually tears
                // down and rebuilds a page's subtree on every section change.
                let (_owner, theme) = setup_with_theme(brightness);
                let mut tcx = TextContext::new();
                let mut root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>> =
                    RenderRoot::new();
                root.set_theme(Box::new(theme));
                let mut state = PlaygroundState::new();
                let mut logic = |s: &mut PlaygroundState| pages::current(section, s);
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

                // A second frame proves reconcile-in-place also survives at
                // this size/brightness (not just first mount).
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

/// The brightness sweep must actually verify COLOR values, not just paint
/// counts — a page that hardcoded its dark-mode text colors would paint an
/// identical color set under both brightnesses. Every page carries at least
/// one theme-resolved text role (headings resolve `primary`, captions
/// `on_surface_variant`), so the per-page painted color SETS must differ
/// between Dark and Light.
#[test]
fn page_text_colors_track_brightness() {
    use std::collections::BTreeSet;
    let (w, h) = SIZES[0];
    for (section, label) in SECTION_LABELS.iter().enumerate() {
        let mut palettes: Vec<BTreeSet<[u8; 4]>> = Vec::new();
        for brightness in BRIGHTNESSES {
            let (_owner, theme) = setup_with_theme(brightness);
            let mut tcx = TextContext::new();
            let mut root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>> = RenderRoot::new();
            root.set_theme(Box::new(theme));
            let mut state = PlaygroundState::new();
            let mut logic = |s: &mut PlaygroundState| pages::current(section, s);
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
    let mut root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>> = RenderRoot::new();
    let mut state = PlaygroundState::new();
    // A defensive out-of-range index must still mount (platform-views fallback).
    let mut logic = |s: &mut PlaygroundState| pages::current(99, s);
    let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(scene.text_runs > 0, "the fallback page paints");
}

/// The regression test for nav-label wrapping: at phone
/// portrait width the bottom `navigation_bar` divides its width evenly across
/// all six [`SECTION_LABELS`] destinations — 390px / 6 = 65px per slot — and
/// a label whose shaped text wraps to two lines overflows the bar's declared
/// 64dp height (`label_y` = 44 inside a 64px box leaves only 20px, but two
/// `labelMediumEmphasized` lines need 32px). Mounts the WHOLE
/// [`PlaygroundApp`] shell (not a bare page, and not at the suite's other
/// 900x700 desktop-ish size where a 150px slot never wrapped) and inspects
/// the actual painted glyph runs: a wrapped label shapes to two separate
/// `draw_glyph_run` calls sharing one block origin (see
/// [`RecScene::text_run_origins`]'s doc comment), so more than one recorded
/// origin per slot is the tripwire.
///
/// Verified locally (not committed) to actually catch the regression: with
/// `SECTION_LABELS[0]` temporarily restored to `"Platform Views"`, this test
/// fails with `nav label 0 ("Platform") painted 2 glyph run(s) ...`.
#[test]
fn full_shell_nav_labels_fit_single_line_at_phone_width() {
    let _owner = setup();
    let mut tcx = TextContext::new();

    const PHONE_W: f64 = 390.0;
    const PHONE_H: f64 = 844.0;
    const NAV_HEIGHT: f64 = 64.0;
    const SLOT_W: f64 = PHONE_W / SECTION_LABELS.len() as f64;
    // Mirrors `frust_widgets::material::navbar`'s private `PAD_TOP` (8.0) +
    // `INDICATOR_H` (32.0) + `LABEL_GAP` (4.0) — the label's top-edge offset
    // from its item's own origin. Every item's label lands at exactly this Y
    // (the label block's TOP, not its baseline — see
    // `NavItemWidget::layout`'s `self.label.set_origin`), single-line or
    // wrapped alike (both lines of a wrapped label share one block origin;
    // see [`RecScene::text_run_origins`]'s doc comment) — so this is a tight,
    // exact identifier for "a nav-bar label's glyph run", unlike a loose
    // "anywhere in the bar's Y band" filter, which also catches unclipped
    // off-screen page filler content that happens to paint in that band.
    const LABEL_TOP_OFFSET: f64 = 44.0;
    let nav_top = PHONE_H - NAV_HEIGHT;
    let expected_label_y = nav_top + LABEL_TOP_OFFSET;

    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut logic = |_s: &mut ()| any(component(PlaygroundApp));
    let mut state = ();
    let (scene, _outcome) = frame_at_size(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        Size::new(PHONE_W, PHONE_H),
        0,
    );

    let mut runs_per_slot = [0usize; 6];
    for origin in &scene.text_run_origins {
        if (origin.y - expected_label_y).abs() > 0.5 {
            continue;
        }
        let slot = ((origin.x / SLOT_W) as usize).min(SECTION_LABELS.len() - 1);
        runs_per_slot[slot] += 1;
    }

    for (slot, label) in SECTION_LABELS.iter().enumerate() {
        assert_eq!(
            runs_per_slot[slot], 1,
            "nav label {slot} ({label:?}) painted {} glyph run(s) inside its {SLOT_W}px slot at \
             {PHONE_W}px width — expected exactly 1 (a single line); more than 1 means the label \
             wrapped and overflowed the nav bar's declared {NAV_HEIGHT}dp box",
            runs_per_slot[slot],
        );
    }
}

#[test]
fn full_shell_mounts_with_app_bar_body_and_nav_bar() {
    let _owner = setup();
    let mut tcx = TextContext::new();

    // Mount the whole app through its root Component — the same tree `app!`
    // binds on every platform: navigator → app bar + pattern-switched section
    // body + bottom navigation bar.
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut logic = |_s: &mut ()| any(component(PlaygroundApp));
    let mut state = ();

    let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);

    // The app-bar title + three toggle labels + the four nav-bar labels + the
    // platform-views page's own text all shape text runs.
    assert!(
        scene.text_runs > 0,
        "the shell paints its app-bar/nav-bar/body text (got {})",
        scene.text_runs,
    );
    // The three app-bar toggle buttons paint rounded-rect backgrounds.
    assert!(
        scene.rounded >= 3,
        "the app bar paints its three toggle buttons (got {} rounded rects)",
        scene.rounded,
    );

    // A second frame keeps mounting cleanly (reconcile-in-place, no panic).
    let (scene2, _outcome2) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 16);
    assert!(scene2.text_runs > 0);
}
