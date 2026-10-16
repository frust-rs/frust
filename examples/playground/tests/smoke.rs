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
use std::sync::Mutex;

use frust::authoring::WindowInsets;
use frust::{
    AnyView, Brightness, Component, GetUntracked, Set, Theme, Update, WindowMetrics, any,
    component, handles_back, provide_context, push_back_press, text,
};
use frust_core::FrameTime;
use frust_core::{PaintOutcome, PaintScene, RenderRoot, View};
use frust_reactive::{ReactiveRuntime, clear_can_pop_provider};
use frust_scene::GlyphRun;
use frust_text::TextContext;
use kurbo::{BezPath, Point, Rect, Size};
use peniko::{Brush, Color};
use reactive_graph::owner::Owner;

use playground::pages::{self, PRIMARY_NAV_COUNT, SECTION_LABELS};
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
    /// Straight-line strokes — the "Graph" section's edges
    /// (`pages::graph_canvas`'s `CanvasView` paints each with `stroke_line`).
    lines: usize,
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
    /// `draw_scene_texture` commands — an in-scene texture (`texture_view`).
    scene_textures: usize,
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
    fn stroke_line(&mut self, _p0: Point, _p1: Point, _width: f64, _color: Color) {
        self.lines += 1;
    }
    fn draw_scene_texture(&mut self, _id: u64, _dest: Rect) {
        self.scene_textures += 1;
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

/// Like [`setup`], but also `provide_context`s a [`Theme`] at the given
/// [`Brightness`] under the fresh ambient owner — mirroring a shell's own
/// `provide_context(theme.clone())` push (`docs/ARCHITECTURE.md`'s Theme
/// delivery, the app-code half). Every page reads `use_context::<Theme>()`
/// (falling back to `frust_material::baseline()` with no context), so this is
/// what lets the multi-brightness sweep below actually exercise each page's
/// live-token re-resolution rather than only ever hitting the fallback.
fn setup_with_theme(brightness: Brightness) -> (Owner, Theme) {
    let owner = setup();
    let theme = Theme::builder(frust_material::baseline())
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

/// The Video and Camera pages' texture modes: each page mounts and paints with
/// its toggle off (no scene texture — no session exists on this host, and the
/// native slot is a platform view) and on (a `SceneTexture` command: the
/// in-scene picture, fed here by the pages' synthetic pattern since no player
/// or camera exists headless), over two frames each.
#[test]
fn texture_pages_mount_in_both_modes() {
    type PageFn = fn(&PlaygroundState) -> AnyView<PlaygroundState>;
    let pages: [(&str, PageFn, bool); 4] = [
        ("video/native", |s| any(pages::video_player::page(s)), false),
        (
            "video/texture",
            |s| any(pages::video_player::texture_page(s)),
            true,
        ),
        ("camera/native", |s| any(pages::camera::page(s)), false),
        (
            "camera/texture",
            |s| any(pages::camera::texture_page(s)),
            true,
        ),
    ];
    for (name, page, texture) in pages {
        let _owner = setup();
        let mut tcx = TextContext::new();
        let mut root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>> = RenderRoot::new();
        let mut state = PlaygroundState::new();
        let mut logic = |s: &mut PlaygroundState| page(s);
        for t_ms in [0, 16] {
            let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, t_ms);
            assert!(scene.text_runs > 0, "{name} must paint text at {t_ms} ms");
            if texture {
                assert!(
                    scene.scene_textures > 0,
                    "{name} must draw its picture as a scene texture at {t_ms} ms"
                );
            } else {
                assert_eq!(
                    scene.scene_textures, 0,
                    "{name} draws no scene texture with the toggle off"
                );
            }
        }
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

/// The regression test for nav-label wrapping: at phone portrait width (no
/// `WindowMetrics` context published — the headless harness's own
/// unpublished-context state, which `playground::home_page` treats as narrow,
/// same as `pages::responsive`'s convention) the bottom `navigation_bar`
/// divides its width evenly across its VISIBLE destinations — the first
/// [`PRIMARY_NAV_COUNT`] of [`SECTION_LABELS`] plus a trailing "More" slot,
/// never the full (now eleven-long) label list — 390px / `VISIBLE_LABELS.len()`
/// per slot, and a label whose shaped text wraps to two lines overflows the
/// bar's declared 64dp height (`label_y` = 42 inside a 64px box leaves only
/// 22px, but two `labelMediumEmphasized` lines need 32px). Mounts the WHOLE
/// [`PlaygroundApp`] shell (not a bare page, and not at the suite's other
/// 900x700 desktop-ish size where a wider slot never wrapped) and inspects
/// the actual painted glyph runs: a single-line label's block lands at
/// exactly `LABEL_TOP_OFFSET` below the bar's top edge (see that local
/// constant's own comment, inside this test's body); a wrapped label's block
/// does NOT — post `p4-03-navbar-rework` (merge `cacaaf21`) the item's
/// icon+label column is vertically centred by its own content height, so a
/// taller (two-line) label pulls its shared block origin to a different Y
/// than a one-line label's — so a slot painting anything other than exactly
/// one glyph run at the single-line Y (zero, because the wrap moved it
/// elsewhere, or more, were a future layout to ever repaint in place) is the
/// tripwire.
///
/// Verified locally (not committed) to actually catch the regression: with
/// `SECTION_LABELS[0]` temporarily restored to `"Platform Views"`, this test
/// fails with `nav label 0 ("Platform Views") painted 0 glyph run(s) ...`
/// (the wrapped block's shared origin shifts down to a different Y, out of
/// the single-line band entirely — see `LABEL_TOP_OFFSET`'s comment).
#[test]
fn full_shell_nav_labels_fit_single_line_at_phone_width() {
    let _owner = setup();
    let mut tcx = TextContext::new();

    // The narrow-width bottom nav bar's own visible set (`playground`'s
    // `bottom_nav_bar`): the first `PRIMARY_NAV_COUNT` section labels plus a
    // trailing "More" slot — never the full `SECTION_LABELS` list, which is
    // exactly the overflow this nav restructure exists to avoid wrapping.
    let visible_labels: Vec<&str> = SECTION_LABELS[..PRIMARY_NAV_COUNT]
        .iter()
        .copied()
        .chain(std::iter::once("More"))
        .collect();

    const PHONE_W: f64 = 390.0;
    const PHONE_H: f64 = 844.0;
    const NAV_HEIGHT: f64 = 80.0;
    let slot_w: f64 = PHONE_W / visible_labels.len() as f64;
    // Mirrors `frust_material::navbar`'s current (post p4-03-navbar-rework,
    // merge `cacaaf21`) item layout — the label's top-edge offset from its
    // item's own origin. Before that rework `NavItemWidget::layout` placed
    // icon/label under a fixed `PAD_TOP` (8.0), giving a label offset of
    // `PAD_TOP + INDICATOR_H + LABEL_GAP` = 8 + 32 + 4 = 44. The rework
    // dropped `PAD_TOP` and instead centres the icon+label column inside the
    // item's full height — matching upstream's `Column(mainAxisAlignment:
    // center)` — so the offset is now `((HEIGHT_MEDIUM - content_h) / 2.0) +
    // INDICATOR_H + LABEL_GAP`, where `content_h = INDICATOR_H + LABEL_GAP +
    // <single-line label height>` (32 + 4 + 16 = 52 at the default
    // `NavBarSize::Medium`, 80dp). That's `((80.0 - 52.0) / 2.0) + 32.0 + 4.0`
    // = 14.0 + 36.0 = 50.0. A SINGLE-LINE item's label lands at exactly this Y
    // (the label block's TOP, not its baseline — see `NavItemWidget::layout`'s
    // `self.label.set_origin`) — but `top` (and so this offset) is itself a
    // function of the label's own content height, so a WRAPPED (two-line)
    // label's block lands at a DIFFERENT, smaller Y instead (both lines of a
    // wrapped label still share that one shifted block origin; see
    // [`RecScene::text_run_origins`]'s doc comment). That makes counting runs
    // at exactly this Y a tight, exact identifier for "a nav-bar label
    // painted as a single line in its slot": a wrapped or missing label
    // shows up as zero here (not two), unlike a loose "anywhere in the bar's
    // Y band" filter, which would also catch unclipped off-screen page
    // filler content that happens to paint in that band.
    const LABEL_TOP_OFFSET: f64 = 50.0;
    let nav_top = PHONE_H - NAV_HEIGHT;
    let expected_label_y = nav_top + LABEL_TOP_OFFSET;

    let mut root: RenderRoot<(), _> = RenderRoot::new();
    let mut logic = |_s: &mut ()| component(PlaygroundApp);
    let mut state = ();
    let (scene, _outcome) = frame_at_size(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        Size::new(PHONE_W, PHONE_H),
        0,
    );

    let mut runs_per_slot = vec![0usize; visible_labels.len()];
    for origin in &scene.text_run_origins {
        if (origin.y - expected_label_y).abs() > 0.5 {
            continue;
        }
        let slot = ((origin.x / slot_w) as usize).min(visible_labels.len() - 1);
        runs_per_slot[slot] += 1;
    }

    for (slot, label) in visible_labels.iter().enumerate() {
        assert_eq!(
            runs_per_slot[slot], 1,
            "nav label {slot} ({label:?}) painted {} glyph run(s) inside its {slot_w}px slot at \
             {PHONE_W}px width — expected exactly 1 (a single line painted at the item's \
             single-line label Y); zero means the label wrapped (its shared block origin shifted \
             to a different Y, since the item column is centred by content height — see \
             `LABEL_TOP_OFFSET`'s comment above) and overflowed the nav bar's declared \
             {NAV_HEIGHT}dp box",
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
    let mut root: RenderRoot<(), _> = RenderRoot::new();
    let mut logic = |_s: &mut ()| component(PlaygroundApp);
    let mut state = ();

    let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);

    // The app-bar title + three toggle labels + the six nav-bar labels (the
    // first `PRIMARY_NAV_COUNT` sections plus "More") + the platform-views
    // page's own text all shape text runs.
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

/// The "Graph" section's own coverage gate: its `CanvasView` paints every
/// edge (`stroke_line`, recorded as [`RecScene::lines`]) and every node (a
/// `fill_rounded_rect` circle, recorded as [`RecScene::rounded`]), and its
/// per-node labels plus its title/caption/readout all shape real text —
/// proving the page builds, lays out, and paints headless (see the module
/// docs), beyond the generic per-section sweep above.
#[test]
fn graph_page_paints_nodes_edges_and_labels() {
    let _owner = setup();
    let mut tcx = TextContext::new();
    let graph_index = pages::section_index_for("graph").expect("graph is a known section");

    let mut root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>> = RenderRoot::new();
    let mut state = PlaygroundState::new();
    let mut logic = |s: &mut PlaygroundState| pages::current(graph_index, s);
    let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);

    assert!(
        scene.lines >= 13,
        "the graph's 13 edges must each paint a stroke_line (got {})",
        scene.lines,
    );
    assert!(
        scene.rounded >= 12,
        "all 12 nodes must each paint a fill_rounded_rect circle (got {})",
        scene.rounded,
    );
    assert!(
        scene.text_runs > 12,
        "the title/caption/readout plus every node's own label must shape text (got {})",
        scene.text_runs,
    );

    // A second frame (reconcile-in-place) keeps painting cleanly, no panic.
    let (scene2, _outcome2) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 16);
    assert!(scene2.lines >= 13);
}

/// At or past the shell's wide-nav breakpoint, `playground`'s home page shows
/// its side rail's full fourteen-destination column instead of the narrow
/// bottom nav bar's six-item (five primary + "More") bar — a `WindowMetrics`
/// context is how an app publishes window shape (`pages::responsive`'s own
/// convention), so providing one at a wide size before mounting is what flips
/// the home page onto that path. Asserted by nav-destination TEXT RUN COUNT:
/// fourteen side-rail labels paint strictly more nav-destination text than the
/// narrow bar's six.
#[test]
fn wide_width_shows_every_section_in_the_side_rail() {
    let _owner = setup();
    let mut tcx = TextContext::new();

    let narrow_runs = {
        let mut root: RenderRoot<(), _> = RenderRoot::new();
        let mut logic = |_s: &mut ()| component(PlaygroundApp);
        let mut state = ();
        let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
        scene.text_runs
    };

    provide_context(WindowMetrics::new(
        Size::new(1024.0, 768.0),
        1.0,
        WindowInsets::default(),
    ));
    let wide_runs = {
        let mut root: RenderRoot<(), _> = RenderRoot::new();
        let mut logic = |_s: &mut ()| component(PlaygroundApp);
        let mut state = ();
        let (scene, _outcome) = frame_at_size(
            &mut root,
            &mut logic,
            &mut state,
            &mut tcx,
            Size::new(1024.0, 768.0),
            0,
        );
        scene.text_runs
    };

    assert!(
        wide_runs > narrow_runs,
        "the wide-width side rail (12 destinations) must paint more text runs than the narrow \
         bottom bar (6 destinations): narrow {narrow_runs}, wide {wide_runs}",
    );
}

/// The "Scroll" section's `ListView`/`ScrollController` pairing actually
/// drives the attached surface end to end, headless: mounts the page, issues
/// a `jump_to` straight through [`PlaygroundState::scroll_controller`] (the
/// same handle the page's own buttons drive), runs a frame to apply it, and
/// asserts the controller's published offset moved off zero — proving the
/// list attached, laid out, and drained the command, beyond the generic
/// per-section paint sweep above.
#[test]
fn scroll_page_jump_moves_the_controller_offset() {
    let _owner = setup();
    let mut tcx = TextContext::new();
    let scroll_index = pages::section_index_for("scroll").expect("scroll is a known section");

    let mut root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>> = RenderRoot::new();
    let mut state = PlaygroundState::new();
    let mut logic = |s: &mut PlaygroundState| pages::current(scroll_index, s);
    let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(scene.text_runs > 0, "the Scroll page must paint text");
    assert_eq!(
        state.scroll_controller.offset(),
        0.0,
        "the list starts at the top"
    );

    state.scroll_controller.jump_to(400.0);
    let (scene2, _outcome2) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 16);
    assert!(
        scene2.text_runs > 0,
        "the Scroll page must still paint text after the jump"
    );
    assert!(
        state.scroll_controller.offset() > 0.0,
        "jump_to(400.0) must move the controller's published offset, got {}",
        state.scroll_controller.offset(),
    );
}

/// The "Drag" section's kanban board actually reads live state end to end,
/// headless: mounts the page, moves a card between columns straight through
/// [`PlaygroundState::kanban_columns`] (the same public field the board's own
/// drop callback mutates), runs a frame to pick the change up, and asserts
/// the move landed — proving the board attached to that field and the page
/// re-paints after it changes, beyond the generic per-section paint sweep
/// above. The reorder list and the file-drop zone are covered by that
/// generic sweep only — see `pages::drag_drop`'s own module docs for why
/// (the file-drop zone is "untestable headlessly beyond construction").
#[test]
fn drag_page_paints_and_kanban_columns_move_cards() {
    let _owner = setup();
    let mut tcx = TextContext::new();
    let drag_index = pages::section_index_for("drag").expect("drag is a known section");

    let mut root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>> = RenderRoot::new();
    let mut state = PlaygroundState::new();
    let mut logic = |s: &mut PlaygroundState| pages::current(drag_index, s);
    let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(
        scene.text_runs > 0,
        "the Drag page must paint the kanban board, reorder list, and file-drop zone's text"
    );
    assert!(
        scene.rounded > 0,
        "the kanban cards and the file-drop zone must each paint a themed fill"
    );

    let before = state.kanban_columns.get_untracked();
    assert!(
        before[0].iter().any(|card| card.id == 0),
        "card 0 starts in column 0"
    );

    // Move card 0 into column 2 — the same mutation the kanban target's own
    // `on_drop` callback (`pages::drag_drop::move_card`) performs on a real
    // drop.
    state.kanban_columns.update(|columns| {
        let card = columns[0].remove(0);
        columns[2].push(card);
    });
    let (scene2, _outcome2) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 16);
    assert!(
        scene2.text_runs > 0,
        "the Drag page must still paint text after the card moves"
    );
    let after = state.kanban_columns.get_untracked();
    assert!(
        !after[0].iter().any(|card| card.id == 0),
        "card 0 must have left column 0"
    );
    assert!(
        after[2].iter().any(|card| card.id == 0),
        "card 0 must have landed in column 2"
    );
}

/// Serializes the "Tabs" back-routing tests: back arbitration sits behind
/// process-wide statics (`handles_back`), so two of them racing would read
/// each other's registrants.
static TABS_BACK_LOCK: Mutex<()> = Mutex::new(());

/// A pre-built harness over the "Tabs" page (and any other section the test
/// switches to through `state.section`).
struct TabsHarness {
    root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>>,
    state: PlaygroundState,
    tcx: TextContext,
    t_ms: u64,
}

impl TabsHarness {
    fn new() -> Self {
        clear_can_pop_provider();
        frust::set_handles_back(false);
        let state = PlaygroundState::new();
        state
            .section
            .set(pages::section_index_for("tabs").expect("tabs is a known section"));
        TabsHarness {
            root: RenderRoot::new(),
            state,
            tcx: TextContext::new(),
            t_ms: 0,
        }
    }

    /// `count` frames: a build-time back press or a pending push/pop lands on
    /// the first, and the route observers settle on the next.
    fn frames(&mut self, count: usize) {
        let mut logic = |s: &mut PlaygroundState| pages::current(s.section.get_untracked(), s);
        for _ in 0..count {
            self.t_ms += 16;
            frame_at(
                &mut self.root,
                &mut logic,
                &mut self.state,
                &mut self.tcx,
                self.t_ms,
            );
        }
    }

    fn select(&mut self, tab: usize) {
        self.state.tabs.selected.set(tab);
        self.frames(3);
    }

    /// Push `count` pages on tab `tab`'s navigator (a counted loop: `depth()`
    /// only updates on rebuild, so it can never be polled in place).
    fn push_pages(&mut self, tab: usize, count: usize) {
        for _ in 0..count {
            self.state.tabs.navs[tab].push(|| text("detail"));
            self.frames(3);
        }
    }

    fn depths(&self) -> [usize; 3] {
        [0, 1, 2].map(|tab| self.state.tabs.navs[tab].depth())
    }

    fn selected(&self) -> usize {
        self.state.tabs.selected.get_untracked()
    }
}

/// Back with Home selected leaves a hidden tab's stack alone: only the
/// selected tab's navigator may pop, and Home sits at its root.
#[test]
fn tabs_back_on_home_leaves_a_hidden_tab_untouched() {
    let _guard = TABS_BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _owner = setup();
    let mut h = TabsHarness::new();
    h.frames(3);

    h.select(1);
    h.push_pages(1, 2);
    assert_eq!(h.depths(), [1, 3, 1], "tab B holds three pages");

    h.select(0);
    push_back_press();
    h.frames(3);
    assert_eq!(h.depths(), [1, 3, 1], "Home unchanged, B still depth 3");
    assert_eq!(h.selected(), 0, "the selection stays on Home");
}

/// Back at a non-home tab root returns to Home (the fallback claims it).
#[test]
fn tabs_back_at_a_tab_root_selects_home() {
    let _guard = TABS_BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _owner = setup();
    let mut h = TabsHarness::new();
    h.frames(3);

    h.select(1);
    assert_eq!(h.depths()[1], 1, "tab B is at its root");
    assert!(handles_back(), "the fallback claims back off Home");

    push_back_press();
    h.frames(3);
    assert_eq!(h.selected(), 0, "back at B's root selected Home");
}

/// Narrow check only: this drives `pages::current` as the ROOT, where a
/// section change is an `AnyView` type swap that always tears down. It does
/// NOT exercise the shell's `pattern_switcher`; the `shell_*` tests below do.
///
/// Leaving the Tabs section unmounts the `back_fallback` wrapper: once pruned,
/// the fallback stops claiming and its `on_back` never fires.
#[test]
fn tabs_fallback_stops_claiming_after_the_section_unmounts() {
    let _guard = TABS_BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _owner = setup();
    let mut h = TabsHarness::new();
    h.frames(3);

    h.select(1);
    assert!(handles_back(), "mounted fallback claims back at B's root");

    // Switch the shell to another section and rebuild (a counted number of
    // frames) until the wrapper has unmounted and the fallback is pruned.
    h.state
        .section
        .set(pages::section_index_for("layout").expect("layout is a known section"));
    h.frames(4);
    assert!(
        !handles_back(),
        "an unmounted fallback claims nothing (no root navigator in this harness)"
    );

    push_back_press();
    h.frames(3);
    assert_eq!(h.selected(), 1, "on_back did not fire: B stays selected");
    assert!(!handles_back());
}

/// A root component that is `PlaygroundApp` verbatim (same `init`, same
/// `build`) but hands a clone of its state out, so a test can drive the real
/// shell (`pattern_switcher` hosting the sections) and read the Tabs state.
/// Every `PlaygroundState` field is a shared handle, so the clone is live.
struct ShellProbe(std::rc::Rc<std::cell::RefCell<Option<PlaygroundState>>>);

impl Component for ShellProbe {
    type State = PlaygroundState;

    fn init(&self) -> PlaygroundState {
        let state = PlaygroundApp.init();
        *self.0.borrow_mut() = Some(state.clone());
        state
    }

    fn build(&self, state: &mut PlaygroundState) -> impl View<PlaygroundState> {
        PlaygroundApp.build(state)
    }
}

/// Harness over the whole shell, rooted at the real navigator + switcher.
struct ShellHarness {
    root: RenderRoot<(), frust::ComponentView<ShellProbe>>,
    handle: std::rc::Rc<std::cell::RefCell<Option<PlaygroundState>>>,
    tcx: TextContext,
    t_ms: u64,
}

impl ShellHarness {
    fn new() -> Self {
        clear_can_pop_provider();
        frust::set_handles_back(false);
        let mut h = ShellHarness {
            root: RenderRoot::new(),
            handle: Default::default(),
            tcx: TextContext::new(),
            t_ms: 0,
        };
        h.frames(3);
        h
    }

    fn state(&self) -> PlaygroundState {
        self.handle.borrow().clone().expect("the shell initialised")
    }

    fn frames(&mut self, count: usize) {
        let handle = self.handle.clone();
        let mut logic = move |_s: &mut ()| component(ShellProbe(handle.clone()));
        let mut unit = ();
        for _ in 0..count {
            self.t_ms += 50;
            frame_at(
                &mut self.root,
                &mut logic,
                &mut unit,
                &mut self.tcx,
                self.t_ms,
            );
        }
    }

    /// Switch section the way the nav bar does, then run past the switcher's
    /// settle (a counted number of 50 ms frames, well over its duration).
    fn go_to(&mut self, label: &str) {
        let state = self.state();
        let index = pages::section_index_for(label).expect("known section");
        let current = state.section.get_untracked();
        state.reverse.set(index < current);
        state.section.set(index);
        self.frames(30);
    }

    fn select(&mut self, tab: usize) {
        self.state().tabs.selected.set(tab);
        self.frames(3);
    }

    fn push_pages(&mut self, tab: usize, count: usize) {
        let state = self.state();
        for _ in 0..count {
            state.tabs.navs[tab].push(|| text("detail"));
            self.frames(3);
        }
    }

    fn depth(&self, tab: usize) -> usize {
        self.state().tabs.navs[tab].depth()
    }

    fn selected(&self) -> usize {
        self.state().tabs.selected.get_untracked()
    }
}

/// Through the real shell: leaving Tabs never swallows back. B holds three
/// pages, the section is switched away, and the outgoing section's navigator
/// no longer claims the press; the shell's root navigator is at its root, so
/// nothing handles back and a press leaves B alone.
#[test]
fn shell_leaving_tabs_does_not_swallow_back() {
    let _guard = TABS_BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _owner = setup();
    let mut h = ShellHarness::new();

    h.go_to("tabs");
    h.select(1);
    h.push_pages(1, 2);
    assert_eq!(h.depth(1), 3, "tab B holds three pages");
    assert!(handles_back(), "mounted Tabs claims back");

    h.go_to("layout");
    assert!(
        !handles_back(),
        "after the switch only the root navigator (at its root) is left: nothing claims back"
    );
    push_back_press();
    h.frames(3);
    assert_eq!(h.depth(1), 3, "back did not pop B while Tabs is off screen");
    assert_eq!(h.selected(), 1, "the selection is untouched");

    // `selected` lives in PlaygroundState and survives the round trip, but
    // the navigator's page stack is torn down with the section (the pages are
    // closures mounted by the navigator widget), so B returns at its root.
    h.go_to("tabs");
    assert_eq!(h.depth(1), 1, "B's stack was torn down with the section");
    assert_eq!(h.selected(), 1);
}

/// Through the real shell: B at its root, section switched away, back must not
/// fire the Home fallback.
#[test]
fn shell_fallback_does_not_fire_after_leaving_tabs() {
    let _guard = TABS_BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _owner = setup();
    let mut h = ShellHarness::new();

    h.go_to("tabs");
    h.select(1);
    assert_eq!(h.depth(1), 1, "B is at its root");

    h.go_to("layout");
    push_back_press();
    h.frames(3);
    assert_eq!(h.selected(), 1, "on_back did not fire: B stays selected");
}

/// Through the real shell, no switch: back at B's root selects Home.
#[test]
fn shell_back_at_tab_root_selects_home() {
    let _guard = TABS_BACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _owner = setup();
    let mut h = ShellHarness::new();

    h.go_to("tabs");
    h.select(1);
    assert_eq!(h.depth(1), 1);
    assert!(handles_back(), "the fallback claims back at B's root");

    push_back_press();
    h.frames(3);
    assert_eq!(h.selected(), 0, "back at B's root selected Home");
}

/// The window bounds of the first laid-out widget whose type name contains
/// `name`.
fn bounds_of<S: 'static, V: View<S>>(root: &RenderRoot<S, V>, name: &str) -> Rect {
    root.inspect()
        .iter()
        .find(|node| node.type_name.contains(name))
        .map(|node| node.bounds)
        .unwrap_or_else(|| panic!("no laid-out {name}"))
}

/// The "Slivers" section's one `custom_scroll_view`, headless: the Material
/// bar is a real sliver (the viewport's own offset collapses it, and it stays
/// pinned at the viewport's top at its collapsed band), a wheel step scrolls
/// the surface, the shared [`PlaygroundState::slivers_controller`] jumps it to
/// a row, and the layout audit finds no pod whose geometry changed without
/// signalling dirty — beyond the generic per-section paint sweep above.
#[test]
fn slivers_page_collapses_its_pinned_bar_and_jumps_through_the_controller() {
    use frust_core::{InputEvent, PointerButton, PointerEvent, PointerPhase, ScrollDelta};
    use playground::pages::slivers::{BAR_COLLAPSED, BAR_EXPANDED, item_offset};

    let _owner = setup();
    let mut tcx = TextContext::new();
    let slivers_index = pages::section_index_for("slivers").expect("slivers is a known section");

    let mut root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>> = RenderRoot::new();
    root.set_layout_audit(true);
    let mut state = PlaygroundState::new();
    let mut logic = |s: &mut PlaygroundState| pages::current(slivers_index, s);
    let mut t_ms = 0;
    let mut frames = |root: &mut RenderRoot<PlaygroundState, AnyView<PlaygroundState>>,
                      state: &mut PlaygroundState,
                      count: usize| {
        let mut text_runs = 0;
        for _ in 0..count {
            t_ms += 16;
            let (scene, _outcome) = frame_at(root, &mut logic, state, &mut tcx, t_ms);
            text_runs = scene.text_runs;
        }
        text_runs
    };
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;

    assert!(
        frames(&mut root, &mut state, 3) > 0,
        "the Slivers page must paint text"
    );
    let viewport = bounds_of(&root, "ViewportWidget");
    let bar = bounds_of(&root, "SliverAppBarWidget");
    assert_eq!(bar.height(), BAR_EXPANDED, "the bar starts expanded");
    assert_eq!(bar.y0, viewport.y0, "the bar heads the viewport");

    // A 44 px wheel step over the surface collapses the bar by as much.
    let over = viewport.center();
    root.event(
        &mut state,
        &InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: over,
            button: PointerButton::Primary,
        }),
    );
    root.event(
        &mut state,
        &InputEvent::Scroll {
            delta: ScrollDelta::Pixels(0.0, 44.0),
            position: over,
        },
    );
    frames(&mut root, &mut state, 2);
    assert!(
        close(state.slivers_controller.offset(), 44.0),
        "the wheel step scrolls the surface, got {}",
        state.slivers_controller.offset(),
    );
    assert!(close(
        bounds_of(&root, "SliverAppBarWidget").height(),
        BAR_EXPANDED - 44.0
    ));

    // A jump to row 40 collapses the bar fully; it stays pinned at the top.
    state.slivers_controller.jump_to(item_offset(40));
    frames(&mut root, &mut state, 2);
    assert!(close(state.slivers_controller.offset(), item_offset(40)));
    let bar = bounds_of(&root, "SliverAppBarWidget");
    assert_eq!(bar.height(), BAR_COLLAPSED, "pinned at its collapsed band");
    assert_eq!(bar.y0, viewport.y0, "pinned at the viewport's top");

    // A short reverse drag reveals the floating header; it sits under the
    // pinned bar, so only the extent past the bar's band is visible.
    let at = Point::new(viewport.center().x, viewport.y0 + 300.0);
    let pointer = |phase,
                   y: f64,
                   root: &mut RenderRoot<PlaygroundState, AnyView<PlaygroundState>>,
                   state: &mut PlaygroundState| {
        root.event(
            state,
            &InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(at.x, at.y + y),
                button: PointerButton::Primary,
            }),
        );
    };
    pointer(PointerPhase::Down, 0.0, &mut root, &mut state);
    for step in 1..=14 {
        pointer(
            PointerPhase::Move,
            10.0 * step as f64,
            &mut root,
            &mut state,
        );
        frames(&mut root, &mut state, 1);
    }
    let header = bounds_of(&root, "SliverPersistentHeaderWidget");
    let bar = bounds_of(&root, "SliverAppBarWidget");
    let visible = header.y1 - bar.y1;
    assert!(
        visible > 8.0 && close(visible, header.height() - BAR_COLLAPSED),
        "the revealed header shows its extent past the bar's band, got {visible} of {}",
        header.height()
    );
    pointer(PointerPhase::Up, 140.0, &mut root, &mut state);
    frames(&mut root, &mut state, 1);

    // The far end, then back to the top, which re-expands it.
    state.slivers_controller.jump_to(f64::INFINITY);
    frames(&mut root, &mut state, 2);
    assert!(state.slivers_controller.offset() > item_offset(40));
    assert_eq!(
        bounds_of(&root, "SliverAppBarWidget").height(),
        BAR_COLLAPSED
    );
    state.slivers_controller.jump_to(0.0);
    assert!(frames(&mut root, &mut state, 2) > 0);
    assert_eq!(
        bounds_of(&root, "SliverAppBarWidget").height(),
        BAR_EXPANDED
    );

    assert_eq!(
        root.layout_audit_mismatches(),
        0,
        "the layout audit found pods laid out stale on the Slivers page"
    );
}
