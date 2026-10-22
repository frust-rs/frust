//! Headless smoke test: drive the real `RenderRoot` rebuild→layout→paint seam
//! (the desktop shell's own pipeline) against a recording paint target — no
//! GPU, no window — the same harness shape as `examples/playground`'s
//! `tests/smoke.rs`.
//!
//! These tests prove the four tab bodies and the full shell mount, lay out, and
//! paint without panicking at two viewport sizes and both brightnesses — the
//! coverage gate — and that each tab publishes its documented at-rest slot
//! count. They assert structural output (text runs were shaped, accessibility
//! labels are present, slot counts), not pixel-exact content; what every tab
//! exists to show (real OS controls and chrome beside their frust-drawn peers)
//! is a separate, human-run device gate this suite cannot replace — see
//! `README.md`.

use std::any::Any;

use std::sync::{Mutex, MutexGuard};

use frust::{
    AnyView, Brightness, Component, GetUntracked, ScrollController, Set, Theme, component,
    provide_context,
};
use frust_core::FrameTime;
use frust_core::accesskit::Role;
use frust_core::{PaintOutcome, PaintScene, RenderRoot, View, WindowEdgeInsets, WindowInsets};
use frust_reactive::ReactiveRuntime;
use frust_scene::GlyphRun;
use frust_text::TextContext;
use kurbo::{BezPath, Point, Rect, Size};
use native_widgets_demo::pages::common::{fresh_anchor_rect, viewport_key};
use native_widgets_demo::pages::controls_tab::{ANCHORS, AnchorStep, anchor_scroll_step};
use peniko::{Brush, Color};
use reactive_graph::owner::Owner;

use native_widgets_demo::pages::{self, detail};
use native_widgets_demo::{
    DetailRoute, NativeWidgetsDemoApp, NativeWidgetsDemoState, Tab, section_destination,
};

type S = NativeWidgetsDemoState;
type Page = fn(&S) -> AnyView<S>;

const W: f64 = 900.0;
const H: f64 = 700.0;

/// Two viewport sizes the coverage sweep lays every page out at: a phone-ish
/// portrait size and a desktop-ish landscape size.
const SIZES: [(f64, f64); 2] = [(390.0, 844.0), (W, H)];

/// A viewport tall enough to lay a whole tab body out visible: a
/// `platform_view` slot is created per painted frame, so a clipped body
/// publishes fewer than its documented at-rest count.
const TALL: Size = Size::new(390.0, 12000.0);

/// Every tab's root page, by [`Tab::index`].
const TAB_PAGES: [(Tab, Page); Tab::COUNT] = [
    (Tab::Home, pages::home::page),
    (Tab::Controls, pages::controls_tab::page),
    (Tab::Dialogs, pages::dialogs::page),
    (Tab::Lab, pages::lab::page),
];

/// Serialises the full-shell tests: the app chrome is one process-wide slot.
static SHELL: Mutex<()> = Mutex::new(());

fn shell_guard() -> MutexGuard<'static, ()> {
    SHELL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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

/// A fresh headless root on `theme`, under the ambient owner.
fn root_with<V: View<S>>(theme: Theme) -> RenderRoot<S, V> {
    let mut root = RenderRoot::new();
    root.set_theme(Box::new(theme));
    root
}

/// The coverage gate: every tab body builds, lays out and paints at two
/// viewport sizes and both brightnesses, then survives a second
/// (reconcile-in-place) frame.
#[test]
fn every_tab_mounts_and_paints_at_every_size_and_brightness() {
    for brightness in BRIGHTNESSES {
        for (tab, page) in TAB_PAGES {
            for (w, h) in SIZES {
                let (_owner, theme) = setup_with_theme(brightness);
                let mut tcx = TextContext::new();
                let mut root = root_with(theme);
                let mut state = S::new();
                let mut logic = |s: &mut S| page(s);
                let size = Size::new(w, h);
                for t_ms in [0, 16] {
                    let (scene, _outcome) =
                        frame_at_size(&mut root, &mut logic, &mut state, &mut tcx, size, t_ms);
                    assert!(
                        scene.text_runs > 0,
                        "{tab:?} must paint text at {w}x{h} under {brightness:?} (frame {t_ms} ms)",
                    );
                }
            }
        }
    }
}

/// The brightness sweep must verify COLOR values, not just paint counts — a
/// tab that hardcoded its text colors would paint an identical color set
/// under both brightnesses.
#[test]
fn tab_text_colors_track_brightness() {
    use std::collections::BTreeSet;
    let (w, h) = SIZES[0];
    for (tab, page) in TAB_PAGES {
        let mut palettes: Vec<BTreeSet<[u8; 4]>> = Vec::new();
        for brightness in BRIGHTNESSES {
            let (_owner, theme) = setup_with_theme(brightness);
            let mut tcx = TextContext::new();
            let mut root = root_with(theme);
            let mut state = S::new();
            let mut logic = |s: &mut S| page(s);
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
            "{tab:?}: painted text-color set is identical under Dark and Light — at least one \
             theme-resolved role must differ (hardcoded-color regression)",
        );
    }
}

/// Every tab publishes exactly its documented at-rest number of native
/// `platform_view` slots ([`pages::AT_REST_SLOTS`], resolved for the target
/// this test runs on) when laid out with its whole body visible, on a first
/// and a second frame — the structural half of each tab's "expected at rest"
/// readout. A builder that renders a refusal banner on this target publishes
/// no slot, so the count also pins which families refuse here.
#[test]
fn every_tab_publishes_its_documented_at_rest_slot_count() {
    for (tab, page) in TAB_PAGES {
        let (_owner, theme) = setup_with_theme(Brightness::Dark);
        let mut tcx = TextContext::new();
        let mut root = root_with(theme);
        let mut state = S::new();
        let mut logic = |s: &mut S| page(s);
        for t_ms in [0, 16] {
            let (scene, _outcome) =
                frame_at_size(&mut root, &mut logic, &mut state, &mut tcx, TALL, t_ms);
            assert_eq!(
                root.platform_view_frames().len(),
                pages::at_rest_slots(tab),
                "{tab:?}, frame at {t_ms} ms: published native slots must equal the documented \
                 at-rest count",
            );
            assert!(
                scene.text_runs > 0 && scene.rounded + scene.fills > 0,
                "{tab:?} paints text and chrome",
            );
        }
    }
}

/// [`pages::common::anchor_probe`] records the window-space rect its child
/// was painted at — the value the Alerts page hands an action sheet as its
/// anchor — accumulated through ancestors' offsets, and transparent to the
/// child's own size.
#[test]
fn anchor_probe_records_the_painted_window_rect() {
    use frust::{EdgeInsets, Padding, SizedBox, column};
    use native_widgets_demo::pages::common::{anchor_probe, anchor_rect};

    let _owner = setup();
    let mut tcx = TextContext::new();
    let mut root = RenderRoot::new();
    let mut state = NativeWidgetsDemoState::new();
    let mut logic = |_: &mut NativeWidgetsDemoState| {
        Padding(
            EdgeInsets::all(20.0),
            column()
                .child(SizedBox(Some(10.0), Some(40.0)))
                .child(anchor_probe(
                    "smoke.anchor",
                    SizedBox(Some(120.0), Some(44.0)),
                )),
        )
    };
    frame_at(&mut root, &mut logic, &mut state, &mut tcx, 0);
    let anchor = anchor_rect("smoke.anchor").expect("the probe painted");
    assert_eq!(
        (anchor.x, anchor.y, anchor.width, anchor.height),
        (20.0, 60.0, 120.0, 44.0),
        "origin = padding + the sibling above; size = the child's own",
    );
}

/// The accessibility labels of every node of `role` in the last-painted tree.
fn labels<St: 'static, V: View<St>>(root: &RenderRoot<St, V>, role: Role) -> Vec<String> {
    root.semantics()
        .nodes
        .into_iter()
        .filter(|(_, node)| node.role() == role)
        .filter_map(|(_, node)| node.label().map(str::to_string))
        .collect()
}

/// The ambient owner, a theme at `brightness`, a fresh app state and a text
/// context: what every shell-over-state test starts from.
fn shell_parts(brightness: Brightness) -> (Owner, Theme, S, TextContext) {
    let (owner, theme) = setup_with_theme(brightness);
    (owner, theme, S::new(), TextContext::new())
}

/// Whether the last-painted tree has a node of `role` labelled `label`.
fn has_label<St: 'static, V: View<St>>(root: &RenderRoot<St, V>, role: Role, label: &str) -> bool {
    labels(root, role).iter().any(|seen| seen == label)
}

/// Mounts the whole app through its root Component — the same tree `app!`
/// binds on every platform (a `native_scaffold` over the tab stack) — at both
/// viewport sizes: the fallback chrome draws its title and four tab labels,
/// and publishes no native slot.
#[test]
fn full_shell_mounts_with_fallback_chrome() {
    let _guard = shell_guard();
    for (w, h) in SIZES {
        let _owner = setup();
        let mut tcx = TextContext::new();
        let mut root = RenderRoot::new();
        let mut logic = |_s: &mut ()| component(NativeWidgetsDemoApp);
        let mut state = ();

        let size = Size::new(w, h);
        let (scene, _outcome) = frame_at_size(&mut root, &mut logic, &mut state, &mut tcx, size, 0);
        assert!(
            scene.text_runs > Tab::COUNT + 1,
            "the shell paints its title, every tab label and the body at {w}x{h} (got {} runs)",
            scene.text_runs,
        );
        assert_eq!(
            labels(&root, Role::Heading),
            [Tab::Home.title()],
            "the top bar's title is the selected tab's at {w}x{h}",
        );
        let tab_labels = labels(&root, Role::Tab);
        for tab in Tab::ALL {
            assert!(
                tab_labels.iter().any(|label| label == tab.title()),
                "the tab row names {:?} at {w}x{h}; saw {tab_labels:?}",
                tab.title(),
            );
        }

        let (scene2, _outcome2) =
            frame_at_size(&mut root, &mut logic, &mut state, &mut tcx, size, 16);
        assert!(scene2.text_runs > 0, "a second frame reconciles in place");
    }
}

/// The chrome is never a slot: the full shell, tall enough to show the whole
/// Home body, publishes exactly the Home body's own count.
#[test]
fn the_chrome_publishes_zero_native_slots() {
    let _guard = shell_guard();
    let (_owner, theme, mut state, mut tcx) = shell_parts(Brightness::Dark);
    let mut root = root_with(theme);
    let held = state.clone();
    let mut logic = |_: &mut S| component(ShellOver(held.clone()));
    for t_ms in [0, 16] {
        frame_at_size(&mut root, &mut logic, &mut state, &mut tcx, TALL, t_ms);
        assert_eq!(
            root.platform_view_frames().len(),
            pages::at_rest_slots(Tab::Home),
            "frame at {t_ms} ms: the bars add no slot to the body's",
        );
    }
}

/// [`NativeWidgetsDemoApp`] over a state the test holds, so it can push pages
/// and flip signals the shell's own `init` would keep private. The state's
/// handles are shared, as the tab components' are.
struct ShellOver(S);

impl Component for ShellOver {
    type State = S;

    fn init(&self) -> S {
        self.0.clone()
    }

    fn build(&self, state: &mut S) -> impl View<S> {
        NativeWidgetsDemoApp.build(state)
    }
}

/// Push the Home detail page the way a work row's tap does.
fn push_detail(state: &S) {
    let route = DetailRoute::new("Pull request", "frust/frust");
    let (handles, shown) = (state.clone(), route.clone());
    route.push(state.nav(Tab::Home), move || detail::page(&handles, &shown));
}

/// The toolbar row is the detail page's and no other's: absent on Home's
/// root, present once a detail page is pushed.
#[test]
fn the_toolbar_row_shows_on_a_home_detail_page_only() {
    let _guard = shell_guard();
    let (_owner, theme, mut state, mut tcx) = shell_parts(Brightness::Light);
    let mut root = root_with(theme);
    let held = state.clone();
    let mut logic = |_: &mut S| component(ShellOver(held.clone()));
    let size = Size::new(390.0, 844.0);
    for t_ms in [0, 16] {
        frame_at_size(&mut root, &mut logic, &mut state, &mut tcx, size, t_ms);
    }
    assert!(
        !has_label(&root, Role::Button, "Comment"),
        "Home's root has no toolbar row"
    );
    assert_eq!(labels(&root, Role::Heading), [Tab::Home.title()]);

    push_detail(&state);
    for t_ms in [32, 48, 64] {
        frame_at_size(&mut root, &mut logic, &mut state, &mut tcx, size, t_ms);
    }
    assert!(
        has_label(&root, Role::Button, "Comment"),
        "a Home detail page shows the toolbar row"
    );
    assert_eq!(
        labels(&root, Role::Heading),
        ["Pull request"],
        "the bar carries the pushed page's title",
    );
}

/// `composer_open` renders the composer row as the detail column's last child:
/// its input sits below the scroller, at the bottom of the viewport.
#[test]
fn the_composer_row_is_the_detail_columns_last_child() {
    let _owner = setup();
    let mut tcx = TextContext::new();
    let mut root: RenderRoot<S, AnyView<S>> = RenderRoot::new();
    let mut state = S::new();
    let route = DetailRoute::new("Pull request", "frust/frust");
    let size = Size::new(390.0, 844.0);
    let mut logic = |s: &mut S| detail::page(s, &route);
    let input_bottom = |root: &RenderRoot<S, AnyView<S>>| {
        let update = root.semantics();
        update
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::TextInput)
            .filter_map(|(_, node)| node.bounds())
            .map(|bounds| bounds.y1)
            .reduce(f64::max)
    };

    frame_at_size(&mut root, &mut logic, &mut state, &mut tcx, size, 0);
    assert_eq!(input_bottom(&root), None, "no composer while it is closed");

    state.composer_open.set(true);
    frame_at_size(&mut root, &mut logic, &mut state, &mut tcx, size, 16);
    let bottom = input_bottom(&root).expect("the composer's input painted");
    assert!(
        bottom > size.height * 0.8 && bottom <= size.height,
        "the composer is pinned under the scroller (input bottom {bottom} of {})",
        size.height,
    );
}

/// A `section/sheet` deep link selects the Dialogs tab and asks it to scroll
/// to the Sheet section.
#[test]
fn a_section_deep_link_selects_its_tab_and_sets_the_anchor() {
    let _owner = setup();
    let state = S::new();
    assert_eq!(state.tab.get_untracked(), Tab::Home);
    assert_eq!(state.pending_anchor.get_untracked(), None);

    let destination = section_destination("sheet").expect("sheet is an old section slug");
    state.go(destination);

    assert_eq!(state.tab.get_untracked(), Tab::Dialogs);
    let request = state
        .pending_anchor
        .get_untracked()
        .expect("an anchor request");
    assert_eq!(request.anchor, pages::sheet::ANCHOR);
}

/// The Controls tab at a phone-sized viewport under a pushed top inset, with
/// frames pumped by hand in the order the UI task runs them.
struct AnchorRig {
    _owner: Owner,
    tcx: TextContext,
    root: RenderRoot<S, AnyView<S>>,
    state: S,
    t_ms: u64,
}

impl AnchorRig {
    const SIZE: Size = Size::new(390.0, 800.0);

    fn new(top_inset: f64) -> Self {
        let (owner, theme) = setup_with_theme(Brightness::Dark);
        let mut rig = Self {
            _owner: owner,
            tcx: TextContext::new(),
            root: root_with(theme),
            state: S::new(),
            t_ms: 0,
        };
        rig.root.rebuild(
            &mut |s: &mut S| pages::controls_tab::page(s),
            &mut rig.state,
        );
        rig.set_top_inset(top_inset);
        rig.pump(2);
        rig
    }

    /// Push a status-bar-like top inset, as a rotation or a shell would.
    fn set_top_inset(&mut self, top: f64) {
        self.root.set_insets(WindowInsets::new(
            WindowEdgeInsets::new(0.0, top, 0.0, 0.0),
            WindowEdgeInsets::ZERO,
        ));
    }

    fn pump(&mut self, frames: usize) {
        for _ in 0..frames {
            self.t_ms += 16;
            frame_at_size(
                &mut self.root,
                &mut |s: &mut S| pages::controls_tab::page(s),
                &mut self.state,
                &mut self.tcx,
                Self::SIZE,
                self.t_ms,
            );
        }
    }

    fn controller(&self) -> ScrollController {
        self.state.scroll(Tab::Controls).clone()
    }

    /// Scroll to `offset` and let the frames settle.
    fn scroll_to(&mut self, offset: f64) {
        let controller = self.controller();
        assert!(controller.max_offset() > offset, "the body scrolls");
        controller.jump_to(offset);
        self.pump(3);
        assert!((controller.offset() - offset).abs() < 1.0, "scrolled");
    }

    /// Issue a `section/<label>` request and alternate anchor-scroll steps with
    /// frames until it lands; asserts the section header then sits `inset`
    /// below the viewport top (where the body starts), or the scroll hit its
    /// bottom edge when the section is too near the end to get there.
    fn assert_lands(&mut self, label: &str, inset: f64) {
        let controller = self.controller();
        let destination = section_destination(label).expect("a section slug");
        self.state.go(destination);
        let anchor = destination.anchor.expect("an anchored destination");

        let mut landed = false;
        for _ in 0..40 {
            if anchor_scroll_step(&controller, Tab::Controls, anchor, ANCHORS) == AnchorStep::Landed
            {
                landed = true;
                break;
            }
            self.pump(2);
        }
        assert!(landed, "{label}: never landed");

        let viewport = fresh_anchor_rect(viewport_key(Tab::Controls)).expect("viewport painted");
        let header = fresh_anchor_rect(anchor)
            .expect("the target header is painted")
            .y
            - viewport.y;
        let at_bottom = (controller.max_offset() - controller.offset()).abs() < 2.0;
        assert!(
            (header - inset).abs() < 2.0 || at_bottom,
            "{label}: header at {header}, body starts at {inset}, offset {} of {}",
            controller.offset(),
            controller.max_offset(),
        );
    }
}

/// A fresh Controls tab under `inset`, scrolled to `start_offset`, then a
/// request for `label` must land it at the inset.
fn assert_anchor_lands_from(start_offset: f64, label: &str, inset: f64) {
    let mut rig = AnchorRig::new(inset);
    rig.scroll_to(start_offset);
    rig.assert_lands(label, inset);
}

/// After scrolling more than a viewport, the top probe is culled and its rect
/// stale: the landing must not be off by the scroll delta.
#[test]
fn an_anchor_lands_after_the_body_was_scrolled_past_a_viewport() {
    let _guard = shell_guard();
    assert_anchor_lands_from(2500.0, "New", 0.0);
    assert_anchor_lands_from(2500.0, "Controls", 0.0);
}

/// A section more than two viewports away never paints until the scroll gets
/// near it: the request scrolls toward it, then corrects.
#[test]
fn an_anchor_lands_on_the_far_section_from_the_top() {
    let _guard = shell_guard();
    assert_anchor_lands_from(0.0, "Composite", 0.0);
    assert_anchor_lands_from(0.0, "New", 0.0);
}

/// The header must end at the safe-area inset (below the floating bar), from
/// an offset where the tab header is culled and nothing was ever measured on
/// this fresh thread, for a mid and a far section.
#[test]
fn an_anchor_lands_at_a_nonzero_top_inset_from_a_culled_header() {
    let _guard = shell_guard();
    assert_anchor_lands_from(2500.0, "New", 59.0);
    assert_anchor_lands_from(2500.0, "Controls", 59.0);
    assert_anchor_lands_from(2500.0, "Composite", 59.0);
}

/// Same, from the top, where the far section is culled.
#[test]
fn an_anchor_lands_at_a_nonzero_top_inset_on_the_far_section() {
    let _guard = shell_guard();
    assert_anchor_lands_from(0.0, "Composite", 47.0);
}

/// A rotation changes the top inset between two requests: the second landing
/// uses the new inset, not the first one's.
#[test]
fn an_anchor_follows_a_changed_top_inset_between_requests() {
    let _guard = shell_guard();
    let mut rig = AnchorRig::new(59.0);
    rig.scroll_to(2500.0);
    rig.assert_lands("Controls", 59.0);

    rig.set_top_inset(20.0);
    rig.pump(2);
    rig.scroll_to(2500.0);
    rig.assert_lands("New", 20.0);
}
