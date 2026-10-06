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

use frust::authoring::WindowInsets;
use frust::{
    AnyView, Brightness, GetUntracked, Theme, Update, WindowMetrics, any, component,
    provide_context,
};
use frust_core::FrameTime;
use frust_core::{PaintOutcome, PaintScene, RenderRoot, View};
use frust_reactive::ReactiveRuntime;
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
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut logic = |_s: &mut ()| any(component(PlaygroundApp));
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
/// its side rail's full thirteen-destination column instead of the narrow
/// bottom nav bar's six-item (five primary + "More") bar — a `WindowMetrics`
/// context is how an app publishes window shape (`pages::responsive`'s own
/// convention), so providing one at a wide size before mounting is what flips
/// the home page onto that path. Asserted by nav-destination TEXT RUN COUNT:
/// thirteen side-rail labels paint strictly more nav-destination text than the
/// narrow bar's six.
#[test]
fn wide_width_shows_every_section_in_the_side_rail() {
    let _owner = setup();
    let mut tcx = TextContext::new();

    let narrow_runs = {
        let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
        let mut logic = |_s: &mut ()| any(component(PlaygroundApp));
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
        let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
        let mut logic = |_s: &mut ()| any(component(PlaygroundApp));
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
