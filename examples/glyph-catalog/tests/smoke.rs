//! Headless smoke test: drive the real `RenderRoot` rebuild→layout→paint seam
//! (the desktop shell's own pipeline) against a recording paint target — no
//! GPU, no window — mirroring `examples/huddle`'s own headless `tests/*.rs`
//! harness shape.
//!
//! These tests prove the shell scaffold and every section page (now all
//! filled in, not stubs) mount, lay out, and paint without
//! panicking at multiple viewport sizes and both Glyph brightnesses — the
//! coverage gate. They assert structural paint output (glyph runs were
//! shaped, no panic), not pixel-exact content — see `README.md`'s coverage
//! table for the manual/visual gate this test
//! suite cannot replace.

use std::any::Any;

use frust::{
    AnyView, Brightness, MotionScheme, NavigatorController, Set, Theme, TransitionSpec, any,
    component, navigator, provide_context,
};
use frust_core::FrameTime;
use frust_core::{InputEvent, PointerButton, PointerEvent, PointerPhase};
use frust_core::{PaintOutcome, PaintScene, RenderRoot, View};
use frust_core::{WindowEdgeInsets, WindowInsets};
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
    /// Every glyph run's resolved solid brush color — a
    /// hardcode tripwire: a page whose text colors are theme-resolved paints
    /// DIFFERENT color sets under Dark vs Light; a hardcoded-only page paints
    /// identical sets and fails `page_text_colors_track_brightness`.
    glyph_colors: Vec<peniko::Color>,
    /// Every glyph run's absolute Y translation, in paint order — the
    /// no-double-top-padding regression test
    /// (`root_appbar_consumes_inset_without_double_padding`) diffs this
    /// vector between a zero-inset and a nonzero-top-inset frame.
    glyph_ys: Vec<f64>,
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
        self.glyph_ys.push(run.transform.translation().y);
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

/// One frame at the module's default `(W, H)` size — the shape every
/// pre-existing test in this file used before the multi-size sweep below.
fn frame_at<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    t_ms: u64,
) -> (RecScene, PaintOutcome) {
    frame_at_size(root, logic, state, tcx, Size::new(W, H), t_ms)
}

/// Like [`frame_at`], but pushes a top-edge-only [`WindowInsets`] onto `root`
/// before the rebuild/layout/paint pass — the seam
/// `root_appbar_consumes_inset_without_double_padding` uses to prove the root
/// AppBar consumes the top inset exactly once (its own height
/// grows by `top_inset`; the body `SafeArea` below it has `.top(false)`, so it
/// must NOT pad by `top_inset` a second time).
fn frame_at_with_top_inset<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    top_inset: f64,
    t_ms: u64,
) -> (RecScene, PaintOutcome) {
    root.set_insets(WindowInsets::new(
        WindowEdgeInsets {
            top: top_inset,
            ..WindowEdgeInsets::default()
        },
        WindowEdgeInsets::default(),
    ));
    frame_at(root, logic, state, tcx, t_ms)
}

#[test]
fn every_page_mounts_and_paints() {
    let _owner = setup();
    let mut tcx = TextContext::new();

    for (section, label) in SECTION_LABELS.iter().enumerate() {
        let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
        let mut state = CatalogState::new();
        let mut logic = |s: &mut CatalogState| pages::current(section, s);
        let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
        assert!(
            scene.glyph_runs > 0,
            "section {section} ({label}) must paint some text",
        );
    }
}

/// The coverage gate: every REAL section page (all seven — no stubs remain)
/// builds/lays out/paints headless at 2
/// viewport sizes and both Glyph brightnesses, with no GPU. This is the
/// automated half of the coverage story `README.md`'s table documents; it
/// proves structural soundness (mounts, lays out, paints real content) at
/// every combination, not pixel-exact fidelity to the reference builds (that
/// remains the manual/visual gate — see README.md).
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
                let (scene, _outcome) = frame_at_size(
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
                let (scene2, _outcome2) = frame_at_size(
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
    let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
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

    let (scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);

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
    let (scene2, _outcome2) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 16);
    assert!(scene2.glyph_runs > 0);
}

/// A headless no-double-top-padding regression test: the root
/// [`glyph::app_bar`](frust::glyph::app_bar) consumes the top window inset
/// itself (grows its own height by it), and the body `safe_area(...).top(false)`
/// must NOT pad by that same inset a second time. Every painted glyph run's
/// absolute Y translation should shift by exactly `top_inset` between a
/// zero-inset and a `top_inset`-pushed frame — a run shifting by
/// `2 * top_inset` would mean the body is ALSO padding its top edge (a
/// double-padding bug), and a run shifting by `0` would mean the inset isn't
/// reaching the AppBar at all.
#[test]
fn root_appbar_consumes_inset_without_double_padding() {
    const TOP_INSET: f64 = 50.0;
    const EPS: f64 = 0.5;

    let _owner = setup();
    let mut tcx = TextContext::new();
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let mut logic = |_s: &mut ()| any(component(CatalogApp));
    let mut state = ();

    let (zero, _zero_outcome) =
        frame_at_with_top_inset(&mut root, &mut logic, &mut state, &mut tcx, 0.0, 0);
    let (pushed, _pushed_outcome) =
        frame_at_with_top_inset(&mut root, &mut logic, &mut state, &mut tcx, TOP_INSET, 16);

    // The AppBar consumes the inset, so the body ScrollView's viewport is
    // TOP_INSET shorter in the pushed frame — paint-time visible-rect culling
    // may therefore skip a few extra bottom-of-scroll runs there.
    // Painted runs keep traversal order, so the pushed frame's runs are a
    // prefix-aligned subset of the zero-inset frame's; compare that prefix.
    // (More runs after the push would still be a reshape bug.)
    assert!(
        pushed.glyph_ys.len() <= zero.glyph_ys.len(),
        "an inset push must not paint MORE glyph runs ({} -> {}) — the tree reshaped",
        zero.glyph_ys.len(),
        pushed.glyph_ys.len(),
    );
    assert!(!pushed.glyph_ys.is_empty(), "the shell paints some text");

    for (i, (y0, y1)) in zero.glyph_ys.iter().zip(pushed.glyph_ys.iter()).enumerate() {
        let delta = y1 - y0;
        assert!(
            (delta - TOP_INSET).abs() < EPS,
            "glyph run {i}: expected a single top-inset shift of {TOP_INSET}px, got {delta}px \
             (y0={y0}, y1={y1}) — a ~{double}px shift would mean the body SafeArea is ALSO \
             padding its top edge (double top-padding regression)",
            double = TOP_INSET * 2.0,
        );
    }
}

/// The foundations page must render
/// ZERO frames at rest — its Radius scale specimens are static rounded rects,
/// not animated skeletons. This test drives the foundations page through
/// rebuild→layout→paint and asserts `PaintOutcome::needs_frame == false`.
#[test]
fn foundations_page_requests_no_frames_at_rest() {
    let _owner = setup();
    let mut tcx = TextContext::new();
    let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
    let mut state = CatalogState::new();
    // Section 0 is the foundations page.
    let mut logic = |s: &mut CatalogState| pages::current(0, s);
    let (_scene, outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(
        !outcome.needs_frame,
        "foundations page must not request frames at rest (needs_frame must be false)",
    );
}

/// A later checkpoint (ms) the sweep below re-paints at, well past every
/// section's one-shot MOUNT reveal (`content`'s and `motion`'s staggered
/// `term_block` demos included — their `GlyphStagger` cascade plays out over
/// a fixed wall-clock span *regardless* of `reduce_motion`, which only
/// desyncs the per-line cascade into a single fast fade rather than
/// eliminating the reveal — see `term_block`'s module docs; the longest
/// instance here is 5 lines, ≈510ms). "Settled state" (Acceptance Criterion
/// #1) means at-rest AFTER any such mount-time reveal has finished, not on
/// the very first post-mount paint.
const SETTLE_MS: u64 = 5000;

/// The header's animations-off toggle:
/// the coverage-sweep half of Acceptance Criterion #1 — "toggle off: headless
/// paint of every section reports zero frame requests". Mirrors what the real
/// toggle handler does (`lib.rs`'s `apply_theme`/`effective_reduce_motion`):
/// force `MotionScheme::reduce_motion` through the threaded theme (the "every
/// convention-following widget collapses" half) AND flip
/// `CatalogState::animations_enabled` off (the half `pages::interactions`'
/// wall-clock demos gate on directly) — every section, both brightnesses,
/// must then paint its settled state ([`SETTLE_MS`] after mount) with zero
/// frame requests. Interactions' heartbeat (once
/// the catalog's one demo documented to auto-repeat without
/// input — now tap-to-play and stopped by default like every other demo, see
/// `pages::interactions`'s module docs' Tap-to-play note) was the
/// load-bearing case this regressed if the toggle was wired to only one of
/// the two halves; `interactions_heartbeat_resumes_with_animations_enabled`
/// below now covers that half explicitly against a STARTED heartbeat.
#[test]
fn every_section_requests_no_frames_with_animations_disabled() {
    for brightness in BRIGHTNESSES {
        let theme = Theme::builder(Theme::glyph_baseline())
            .brightness(brightness)
            .map_motion(|m: MotionScheme| MotionScheme {
                reduce_motion: true,
                ..m
            })
            .build();

        for (section, label) in SECTION_LABELS.iter().enumerate() {
            let _owner = setup();
            let mut tcx = TextContext::new();
            let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
            root.set_theme(Box::new(theme.clone()));
            let mut state = CatalogState::new();
            state.animations_enabled.set(false);
            let mut logic = |s: &mut CatalogState| pages::current(section, s);
            // First frame: mount (any one-shot entrance reveal starts here).
            let _ = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
            // Second frame, well past SETTLE_MS: the section's true at-rest
            // state, the one Acceptance Criterion #1 actually targets.
            let (_scene, outcome) =
                frame_at(&mut root, &mut logic, &mut state, &mut tcx, SETTLE_MS);
            assert!(
                !outcome.needs_frame,
                "section {section} ({label}) must request zero frames once settled with \
                 animations disabled under {brightness:?} (got needs_frame == true)",
            );
        }
    }
}

/// The toggle's other half (`pages::interactions`'s own module docs'
/// Tap-to-play note): the
/// heartbeat demo, like every other demo on the page, now starts STOPPED,
/// so a fresh Interactions mount must
/// request zero frames regardless of the toggle. Starting the heartbeat via
/// [`pages::interactions::start_heartbeat_for_test`] (the `pub` test seam
/// that fn's own doc comment describes, mirroring `appbar::open_*`'s
/// precedent) and re-painting proves the toggle's ON half (animations
/// enabled, the default; `reduce_motion` off) still lets a STARTED heartbeat
/// resume requesting frames — the toggle actually restores animation rather
/// than latching off.
#[test]
fn interactions_heartbeat_resumes_with_animations_enabled() {
    let _owner = setup();
    let mut tcx = TextContext::new();
    let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
    let mut state = CatalogState::new();
    let interactions_section = SECTION_LABELS
        .iter()
        .position(|s| *s == "Interactions")
        .expect("an Interactions section must exist");
    let mut logic = |s: &mut CatalogState| pages::current(interactions_section, s);

    let (_scene, outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(
        !outcome.needs_frame,
        "task 11: every demo (including the heartbeat) starts stopped — a fresh Interactions \
         mount must request zero frames",
    );

    pages::interactions::start_heartbeat_for_test();
    let (_scene, outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    assert!(
        outcome.needs_frame,
        "with animations enabled (the default) and reduce_motion off, a STARTED heartbeat demo \
         must request the next frame",
    );
}

/// A smoke sweep across
/// sizes/brightness in both stopped/playing states for at least two demos.
/// `every_page_mounts_at_every_size_and_brightness` above already sweeps
/// every section — including Interactions — in its default (now STOPPED)
/// state; this test sweeps the same size/brightness matrix again with two
/// demos ([`pages::interactions::start_heartbeat_for_test`] and
/// [`pages::interactions::start_waveform_for_test`], each in its own fresh
/// mount) started, proving the PLAYING half of the same contract.
#[test]
fn interactions_playing_demos_mount_at_every_size_and_brightness() {
    let starters: [fn(); 2] = [
        pages::interactions::start_heartbeat_for_test,
        pages::interactions::start_waveform_for_test,
    ];
    let interactions_section = SECTION_LABELS
        .iter()
        .position(|s| *s == "Interactions")
        .expect("an Interactions section must exist");

    for start in starters {
        for brightness in BRIGHTNESSES {
            for (w, h) in SIZES {
                let (_owner, theme) = setup_with_theme(brightness);
                let mut tcx = TextContext::new();
                let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
                root.set_theme(Box::new(theme));
                let mut state = CatalogState::new();
                start();
                let mut logic = |s: &mut CatalogState| pages::current(interactions_section, s);
                let (scene, outcome) = frame_at_size(
                    &mut root,
                    &mut logic,
                    &mut state,
                    &mut tcx,
                    Size::new(w, h),
                    0,
                );
                assert!(
                    scene.glyph_runs > 0,
                    "Interactions must paint text at {w}x{h} under {brightness:?} while playing",
                );
                assert!(
                    outcome.needs_frame,
                    "a started demo must still request its next frame at {w}x{h} under \
                     {brightness:?}",
                );
            }
        }
    }
}

/// The six `appbar::open_*` fns, in [`glyphcatalog::pages::appbar`]'s
/// `VARIATIONS` launcher-list order — shared by both regression tests below.
/// `pub` on the `open_*` fns exists exactly for this seam (see their own doc
/// comments): a headless test calls them directly, mirroring how
/// `crates/frust-widgets/src/nav/navigator.rs`'s own unit tests drive a
/// `NavigatorController` (`controller.push(..)`/`.pop()`) rather than
/// simulating a pixel-exact click through a composite list widget.
const APPBAR_VARIATION_OPENERS: [fn(&mut CatalogState); 6] = [
    pages::appbar::open_compact_elevate,
    pages::appbar::open_scroll_collapse,
    pages::appbar::open_overflow,
    pages::appbar::open_selection,
    pages::appbar::open_banner,
    pages::appbar::open_backnav,
];

/// Mount the AppBar section's launcher list over its own fresh
/// [`NavigatorController`] — mirrors `full_shell_mounts_with_header_tabs_and_body`'s
/// shape, but scoped to just this section's own navigator instead of the
/// whole [`CatalogApp`] shell (the launcher's pushed pages replace the whole
/// screen via the SAME shared navigator the real shell also drives — see
/// `glyphcatalog`'s module docs — so a section-scoped navigator here is a
/// faithful stand-in, not a divergent shape).
fn mount_appbar_launcher() -> (
    RenderRoot<CatalogState, AnyView<CatalogState>>,
    NavigatorController<CatalogState>,
    CatalogState,
) {
    let controller: NavigatorController<CatalogState> = NavigatorController::new();
    let mut state = CatalogState::new();
    state.nav = controller.clone();
    (RenderRoot::new(), controller, state)
}

/// Every one of the six variation pages
/// pushes onto the section's navigator (depth 1 → 2) and pops cleanly back
/// (depth 2 → 1), painting real content at every step — the launcher list
/// itself, the pushed variation page, and the launcher list again after back.
#[test]
fn appbar_variation_pages_push_and_pop() {
    for open in APPBAR_VARIATION_OPENERS {
        let _owner = setup();
        let mut tcx = TextContext::new();
        let (mut root, controller, mut state) = mount_appbar_launcher();
        let ctrl = controller.clone();
        let mut logic = move |_s: &mut CatalogState| {
            any(navigator(&ctrl, || {
                pages::appbar::page(&CatalogState::new())
            }))
        };

        let (launcher_scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
        assert!(launcher_scene.glyph_runs > 0, "the launcher list paints");
        assert_eq!(controller.depth(), 1, "the launcher starts at depth 1");

        // Push the variation page (mirrors the launcher row's `on_press`).
        open(&mut state);
        let (pushed_scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 16);
        assert!(
            pushed_scene.glyph_runs > 0,
            "the pushed variation page paints",
        );
        assert_eq!(controller.depth(), 2, "push must increase depth to 2");

        // Pop — the leading back button's own `nav.pop()` call.
        controller.pop();
        let (popped_scene, _outcome) = frame_at(&mut root, &mut logic, &mut state, &mut tcx, 32);
        assert!(
            popped_scene.glyph_runs > 0,
            "back pops to the launcher list, which paints again",
        );
        assert_eq!(controller.depth(), 1, "pop must return depth to 1");
    }
}

/// Extend the coverage sweep
/// (`every_page_mounts_at_every_size_and_brightness`'s shape) to every
/// variation page — each must mount, lay out, and paint with no panic across
/// both viewport sizes and both Glyph brightnesses once pushed.
#[test]
fn appbar_variation_pages_mount_at_every_size_and_brightness() {
    for brightness in BRIGHTNESSES {
        for (i, open) in APPBAR_VARIATION_OPENERS.iter().enumerate() {
            for (w, h) in SIZES {
                let (_owner, theme) = setup_with_theme(brightness);
                let mut tcx = TextContext::new();
                let (mut root, controller, mut state) = mount_appbar_launcher();
                root.set_theme(Box::new(theme));
                let ctrl = controller.clone();
                let mut logic = move |_s: &mut CatalogState| {
                    any(navigator(&ctrl, || {
                        pages::appbar::page(&CatalogState::new())
                    }))
                };

                let _ = frame_at_size(
                    &mut root,
                    &mut logic,
                    &mut state,
                    &mut tcx,
                    Size::new(w, h),
                    0,
                );
                open(&mut state);
                let (scene, _outcome) = frame_at_size(
                    &mut root,
                    &mut logic,
                    &mut state,
                    &mut tcx,
                    Size::new(w, h),
                    16,
                );
                assert!(
                    scene.glyph_runs > 0,
                    "variation {i} must paint text at {w}x{h} under {brightness:?}",
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Variation-page scroll — headless repro of the device 0px-scroll bug
// ---------------------------------------------------------------------------
//
// The device symptom (Xiaomi 12, build 67c334d): a pushed AppBar *variation*
// page reports 0px scroll-offset movement despite 26 input frames reaching the
// app, while the root Content tab scrolls fine and the back button works
// (ruling out a stuck never-finalized push transition). The layout hypothesis
// was REFUTED (the constraint chain is tight/finite end-to-end, mathematically
// equivalent to the WORKING root page), so these tests attack the *event* path
// instead: they push a real variation page through a real `RenderRoot` (the
// desktop-shell pipeline the whole file uses), settle its Glyph push
// transition, then dispatch a real pointer drag via `RenderRoot::event` and
// assert the pushed page's `ScrollView` offset actually moved.
//
// The offset is observed through the variation page's own `on_scroll` wiring:
// `pages::appbar::variation_scroll_collapse` feeds every offset change into the
// `collapse_progress_sig` (`offset / 60`, clamped), read back via
// `scroll_collapse_progress_for_test()`. A nonzero progress means the drag
// reached the scroll widget; a zero means it died somewhere in the
// navigator/routing chain — the exact device defect. This is the "downcast
// introspection" (scroll.rs's own drag tests read `w.offset()`) equivalent that
// is reachable through a `RenderRoot`, whose nested `ScrollWidget` no public
// traversal API exposes.

/// A device-realistic phone viewport (Xiaomi-12-ish portrait logical size) the
/// scroll-repro tests push a full-screen variation page at. Reuses `SIZES[0]`'s
/// shape but named here for the drag-coordinate math below (a `y` of 500 lands
/// well inside the body `ScrollView`, clear of the top AppBar).
const REPRO_SIZE: (f64, f64) = (390.0, 844.0);

/// A pointer [`InputEvent`] at an absolute `(x, y)` window coordinate — the
/// shape the desktop/Android shells build before crossing into `frust-core`
/// (already logical-pixel; mirrors `scroll.rs`'s own `ev` test helper).
fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase,
        position: Point::new(x, y),
        button: PointerButton::Primary,
    })
}

/// Drive a Glyph push transition to settle: paint several frames advancing the
/// clock far past any themed transition duration, so the navigator's
/// `paint_transition` reports `done`, the next `rebuild` finalizes it, and the
/// stack routes events normally again (the "settled pushed page" state the
/// device bug is observed in). Returns the clock (ms) reached.
fn settle_push<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    mut t_ms: u64,
) -> u64 {
    for _ in 0..6 {
        t_ms += 2_000;
        frame_at_size(
            root,
            logic,
            state,
            tcx,
            Size::new(REPRO_SIZE.0, REPRO_SIZE.1),
            t_ms,
        );
    }
    t_ms
}

/// Dispatch a top-to-bottom vertical drag (finger moving UP the screen, so the
/// content scrolls DOWN / offset increases) starting at `x`, through
/// `RenderRoot::event`. The first move crosses `TOUCH_SLOP` (18px) to arm the
/// scroll takeover; the following moves accumulate ~80px of offset, past the
/// `COLLAPSE_SPAN_PX` (60px) that saturates `collapse_progress` to 1.0.
fn drag_up<S: 'static, V: View<S>>(root: &mut RenderRoot<S, V>, state: &mut S, x: f64) {
    root.event(state, &pointer(PointerPhase::Down, x, 500.0));
    // First move: 20px > slop → takeover (no offset change yet, seeds the drag).
    root.event(state, &pointer(PointerPhase::Move, x, 480.0));
    // Subsequent moves accumulate offset (finger up ⇒ offset up).
    root.event(state, &pointer(PointerPhase::Move, x, 440.0));
    root.event(state, &pointer(PointerPhase::Move, x, 400.0));
    root.event(state, &pointer(PointerPhase::Up, x, 400.0));
}

/// The missing regression test: a real drag on a
/// **pushed** variation page — under the catalog's exact navigator config
/// (`TransitionSpec::glyph()`, edge-swipe therefore OFF, since `pop_swipe`
/// derives on only for the iOS-push preset) — must move the page's `ScrollView`
/// offset. Reproduces the device scenario headlessly: if the drag died in the
/// navigator/routing chain (the 0px defect), `collapse_progress` stays 0 and
/// this FAILS; if scroll works, `on_scroll` fires and progress goes nonzero.
///
/// **Result: this PASSES** — the headless drag scrolls. The event path is
/// sound: the settled navigator routes the whole Down/Move/Up stream to the top
/// page, and its `ScrollView` takes the gesture over past slop exactly as the
/// root page's does.
#[test]
fn pushed_variation_page_drag_scrolls_its_scrollview() {
    let (_owner, theme) = setup_with_theme(Brightness::Dark);
    let mut tcx = TextContext::new();
    let (mut root, controller, mut state) = mount_appbar_launcher();
    root.set_theme(Box::new(theme));
    let ctrl = controller.clone();
    // The catalog's real navigator config: Glyph page transition (lib.rs's
    // `.transition(TransitionSpec::glyph())`), which leaves the edge-swipe
    // pop gesture DISABLED (on only for the iOS-push preset).
    let mut logic = move |_s: &mut CatalogState| {
        any(
            navigator(&ctrl, || pages::appbar::page(&CatalogState::new()))
                .transition(TransitionSpec::glyph()),
        )
    };

    // Mount the launcher, push the scroll-collapse variation, settle its push.
    let _ = frame_at_size(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        Size::new(REPRO_SIZE.0, REPRO_SIZE.1),
        0,
    );
    pages::appbar::open_scroll_collapse(&mut state);
    let _ = settle_push(&mut root, &mut logic, &mut state, &mut tcx, 16);
    assert_eq!(
        controller.depth(),
        2,
        "the variation page must be pushed and settled"
    );

    // Deterministic baseline (the signals are thread-local and self-healing),
    // then a real drag through the whole RenderRoot event path.
    pages::appbar::reset_scroll_collapse_for_test();
    assert_eq!(
        pages::appbar::scroll_collapse_progress_for_test(),
        0.0,
        "baseline: the reset seam must zero the collapse progress",
    );
    drag_up(&mut root, &mut state, 200.0);

    assert!(
        pages::appbar::scroll_collapse_progress_for_test() > 0.0,
        "a drag on the settled pushed variation page must move its ScrollView offset \
         (on_scroll fires ⇒ collapse progress > 0); a 0.0 here is the device 0px-scroll defect \
         reproduced headlessly",
    );
}

/// Edge-swipe-zone vertical-drag guard: a
/// vertical drag whose `Down` lands INSIDE the left `EDGE_SWIPE_ZONE_DP` (20dp)
/// on a pushed page — with the interactive edge-swipe pop gesture ENABLED
/// (`.pop_swipe(true)`, the config the swipe's own investigation flagged as the
/// prime, UNTESTED suspect) — must STILL scroll: the navigator must disarm its
/// armed edge-swipe on the first vertical-dominant move and yield the gesture to
/// the page's `ScrollView`, never steal it as a back-pop. A 0.0 here would mean
/// the edge-swipe arm swallowed a vertical drag that should have scrolled.
#[test]
fn edge_zone_vertical_drag_still_scrolls_pushed_page() {
    let (_owner, theme) = setup_with_theme(Brightness::Dark);
    let mut tcx = TextContext::new();
    let (mut root, controller, mut state) = mount_appbar_launcher();
    root.set_theme(Box::new(theme));
    let ctrl = controller.clone();
    // Edge-swipe explicitly ENABLED over the Glyph transition — the arm/steal
    // machinery suspected for drags starting in the zone.
    let mut logic = move |_s: &mut CatalogState| {
        any(
            navigator(&ctrl, || pages::appbar::page(&CatalogState::new()))
                .transition(TransitionSpec::glyph())
                .pop_swipe(true),
        )
    };

    let _ = frame_at_size(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        Size::new(REPRO_SIZE.0, REPRO_SIZE.1),
        0,
    );
    pages::appbar::open_scroll_collapse(&mut state);
    let _ = settle_push(&mut root, &mut logic, &mut state, &mut tcx, 16);
    assert_eq!(
        controller.depth(),
        2,
        "the variation page must be pushed and settled"
    );

    pages::appbar::reset_scroll_collapse_for_test();
    // Down inside the left edge-swipe zone (x = 10 ≤ 20dp), then a purely
    // vertical drag: the navigator arms the edge-swipe on Down, then must
    // disarm on the first vertical-dominant move and let the page scroll.
    drag_up(&mut root, &mut state, 10.0);

    assert!(
        pages::appbar::scroll_collapse_progress_for_test() > 0.0,
        "a vertical drag starting inside the left edge-swipe zone must disarm the edge-swipe \
         and scroll the pushed page's ScrollView (collapse progress > 0), not be stolen as a \
         back-pop",
    );
    assert_eq!(
        controller.depth(),
        2,
        "the vertical edge-zone drag must NOT trigger a back-pop (depth stays 2)",
    );
}
