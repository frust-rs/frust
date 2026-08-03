//! Integration test for the semantics seam (phase-6c D1): drive a small widget
//! tree through a real `RenderRoot` rebuild + layout, then assert the collected
//! accessibility tree — node count, roles, labels/values, absolute bounds, and
//! parent/child structure.
//!
//! Also proves a widget with **no** `Widget::semantics` override (the trait's
//! defaulted no-op) contributes no node while recursion into its siblings
//! continues.
//!
//! The second half pins **R23** — *navigator semantics forwarding follows input
//! routing, exactly* — as an executable invariant rather than as today's
//! behaviour: across an opaque cover, a transparent overlay, a
//! transparent-over-transparent stack and a running transition, the set of
//! pages contributing accessibility nodes equals the set `NavigatorWidget`
//! routes an input event to.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::{Role, Toggled};
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, Curve, EventCtx, EventResult, FrameTime,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase,
    RenderRoot, SemanticsCtx, SemanticsUpdate, View, Widget, any,
};
use frust_text::TextContext;
use frust_widgets::{
    Column, EdgeInsets, NavigatorController, NavigatorView, Padding, PaddingView, PageTransition,
    PushOptions, Timing, TransitionSpec, button, checkbox, navigator, text,
};
use kurbo::{Point, Size};

/// A leaf widget with a fixed intrinsic size and **no** semantics override — it
/// exercises the trait's defaulted no-op so a non-contributing sibling must be
/// skipped in the collected tree while its neighbours are still collected.
struct BlankWidget;

impl Widget for BlankWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(50.0, 10.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    // No `semantics` override — relies on the defaulted no-op.
}

struct BlankView;

impl View<()> for BlankView {
    type Element = BlankWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BlankWidget {
        BlankWidget
    }
    fn rebuild(&self, _prev: &Self, _el: &mut BlankWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

/// `Padding(all: 20)[ Column[ Text, Blank, Button, Checkbox ] ]`. The 20px inset
/// gives the child nodes a known absolute offset so bounds threading is testable
/// without depending on exact font metrics.
fn logic(_state: &mut ()) -> PaddingView<()> {
    Padding(
        EdgeInsets::all(20.0),
        Column(vec![
            any::<(), _>(text("Hello")),
            any::<(), _>(BlankView),
            any::<(), _>(button::<(), _>("Go", |_| {})),
            any::<(), _>(checkbox::<(), _>(true, "Agree", |_, _| {})),
        ]),
    )
}

#[test]
fn column_of_text_button_checkbox_yields_correct_semantics_tree() {
    let mut root: RenderRoot<(), PaddingView<()>> = RenderRoot::new();
    let mut state = ();
    root.rebuild(&mut logic, &mut state);

    let mut tcx = TextContext::new();
    let window = Size::new(400.0, 600.0);
    root.layout_with_text(window, &mut tcx as &mut dyn std::any::Any);

    let update = root.semantics();

    // --- node count: window root + Text + Button + Checkbox. The Blank leaf has
    // no override, so it contributes nothing even though recursion continued
    // past it to the Button/Checkbox below it. ---
    assert_eq!(
        update.nodes.len(),
        4,
        "window root + 3 contributing widgets (blank leaf contributes nothing)"
    );
    assert!(update.focus.is_none(), "nothing in the tree is focused");

    // Helper: fetch a node by id.
    let get = |id| {
        &update
            .nodes
            .iter()
            .find(|(nid, _)| *nid == id)
            .unwrap_or_else(|| panic!("node {id:?} present"))
            .1
    };

    // --- root window node & parent/child structure. ---
    let root_node = get(update.root);
    assert_eq!(root_node.role(), Role::Window);
    let children = root_node.children();
    assert_eq!(
        children.len(),
        3,
        "the transparent Padding/Column forward their descendants directly under \
         the window node"
    );

    let text_node = get(children[0]);
    let button_node = get(children[1]);
    let checkbox_node = get(children[2]);

    // --- roles ---
    assert_eq!(text_node.role(), Role::Label);
    assert_eq!(button_node.role(), Role::Button);
    assert_eq!(checkbox_node.role(), Role::CheckBox);

    // --- labels / values / state ---
    assert_eq!(text_node.value(), Some("Hello"));
    assert_eq!(button_node.label(), Some("Go"));
    assert_eq!(checkbox_node.label(), Some("Agree"));
    assert_eq!(checkbox_node.toggled(), Some(Toggled::True));

    // --- absolute bounds: the 20px padding offsets the Column start, and the
    // Column stacks children top-to-bottom with no gap. ---
    let tb = text_node.bounds().expect("text node has bounds");
    let bb = button_node.bounds().expect("button node has bounds");
    let cb = checkbox_node.bounds().expect("checkbox node has bounds");

    // First child sits exactly at the padding origin.
    assert_eq!(tb.x0, 20.0, "text x offset by the 20px left padding");
    assert_eq!(tb.y0, 20.0, "text y offset by the 20px top padding");
    // Everything is inside the window.
    for b in [tb, bb, cb] {
        assert!(b.x0 >= 20.0 && b.x1 <= window.width - 20.0 + f64::EPSILON);
        assert!(b.y0 >= 20.0 && b.y1 <= window.height);
        assert!(b.x1 > b.x0 && b.y1 > b.y0, "non-degenerate bounds");
    }
    // The Blank leaf (50x10, no node) still consumes vertical space: the Button
    // sits below the Text plus the blank's 10px height — proving recursion
    // continued through the non-contributing sibling and geometry threaded past
    // it.
    assert!(
        (bb.y0 - (tb.y1 + 10.0)).abs() < 1e-6,
        "button starts below text + the 10px blank spacer (y0={}, expected {})",
        bb.y0,
        tb.y1 + 10.0
    );
    // The Checkbox sits directly below the Button.
    assert!(
        (cb.y0 - bb.y1).abs() < 1e-6,
        "checkbox starts directly below button"
    );
}

#[test]
fn empty_tree_yields_a_bare_window_root() {
    // An unbuilt render root still returns a single window node with no children.
    let root: RenderRoot<(), PaddingView<()>> = RenderRoot::new();
    let update = root.semantics();
    assert_eq!(update.nodes.len(), 1);
    let (id, node) = &update.nodes[0];
    assert_eq!(*id, update.root);
    assert_eq!(node.role(), Role::Window);
    assert!(node.children().is_empty());
    assert!(update.focus.is_none());
}

// ---------------------------------------------------------------------------
// R23 — navigator semantics forwarding follows input routing, exactly.
// ---------------------------------------------------------------------------

/// A full-screen navigator page that is both **labelled** in the accessibility
/// tree and **observable** on the input path: it pushes one `Role::Button` node
/// carrying `label`, and counts the pointer `Down`s that actually reach it.
///
/// One probe per page therefore lets a test read both sides of R23's invariant
/// — "contributes a node" and "is routed input" — as two sets of labels. It also
/// counts paints, so a test can prove a page it expects to be *omitted* from the
/// tree is nonetheless still being drawn (the transparent-overlay case, where
/// semantics deliberately diverges from painting).
struct PageProbeWidget {
    label: &'static str,
    hits: Rc<Cell<u32>>,
    paints: Rc<Cell<u32>>,
}

impl Widget for PageProbeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // A full-screen page, so every page's pod contains the probe point and
        // only the navigator's routing decides who receives it.
        bc.max()
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.paints.set(self.paints.get() + 1);
    }

    fn event(&mut self, _ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Down) {
            self.hits.set(self.hits.get() + 1);
            return EventResult::Handled;
        }
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Button, |node| node.set_label(self.label));
    }
}

struct PageProbe {
    label: &'static str,
    hits: Rc<Cell<u32>>,
    paints: Rc<Cell<u32>>,
}

impl View<()> for PageProbe {
    type Element = PageProbeWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> PageProbeWidget {
        PageProbeWidget {
            label: self.label,
            hits: self.hits.clone(),
            paints: self.paints.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        el: &mut PageProbeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        el.label = self.label;
        el.hits = self.hits.clone();
        el.paints = self.paints.clone();
        ChangeFlags::NONE
    }
}

/// A page identity a test tracks through both seams.
#[derive(Clone)]
struct Page {
    label: &'static str,
    hits: Rc<Cell<u32>>,
    paints: Rc<Cell<u32>>,
}

impl Page {
    fn new(label: &'static str) -> Self {
        Self {
            label,
            hits: Rc::new(Cell::new(0)),
            paints: Rc::new(Cell::new(0)),
        }
    }

    /// A page builder for `push`/`navigator` — cloned per call, so a rebuilt
    /// page still reports into the same counter.
    fn builder(&self) -> impl Fn() -> AnyView<()> + 'static {
        let page = self.clone();
        move || {
            any(PageProbe {
                label: page.label,
                hits: page.hits.clone(),
                paints: page.paints.clone(),
            })
        }
    }
}

/// A no-op scene: these tests care about transition *progress*, never draws.
struct NullScene;

impl PaintScene for NullScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
}

fn nav_down() -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Down,
        position: Point::new(5.0, 5.0),
        button: PointerButton::Primary,
    })
}

fn nav_frame_time(ms: u64) -> FrameTime {
    FrameTime::from_nanos(ms * 1_000_000)
}

/// The labels present anywhere in the collected accessibility tree.
fn tree_labels(update: &SemanticsUpdate) -> Vec<String> {
    update
        .nodes
        .iter()
        .filter_map(|(_, node)| node.label().map(str::to_string))
        .collect()
}

/// The subset of `pages` whose label appears in the collected tree.
fn contributing(
    root: &RenderRoot<(), NavigatorView<()>>,
    pages: &[Page],
) -> BTreeSet<&'static str> {
    let present = tree_labels(&root.semantics());
    pages
        .iter()
        .filter(|p| present.iter().any(|l| l == p.label))
        .map(|p| p.label)
        .collect()
}

/// The subset of `pages` a pointer `Down` actually reaches.
fn routed(
    root: &mut RenderRoot<(), NavigatorView<()>>,
    state: &mut (),
    pages: &[Page],
) -> BTreeSet<&'static str> {
    for p in pages {
        p.hits.set(0);
    }
    root.event(state, &nav_down());
    pages
        .iter()
        .filter(|p| p.hits.get() > 0)
        .map(|p| p.label)
        .collect()
}

/// **The R23 invariant, as an assertion:** `semantics` forwards page *i* ⟺
/// `event` would route to page *i*. Both sides are measured, never assumed.
fn assert_input_parity(
    root: &mut RenderRoot<(), NavigatorView<()>>,
    state: &mut (),
    pages: &[Page],
    expected: &[&'static str],
) {
    let semantic = contributing(root, pages);
    let routed = routed(root, state, pages);
    let expected: BTreeSet<&'static str> = expected.iter().copied().collect();
    assert_eq!(
        semantic, routed,
        "R23: the pages contributing accessibility nodes must be exactly the \
         pages input is routed to"
    );
    assert_eq!(semantic, expected, "and that set is the top page only");
}

/// The #23 regression guard: before this fix `NavigatorWidget` had **no**
/// `semantics` impl at all, so the whole routed surface — every page of every
/// routed app — was absent from the accessibility tree. A pushed page's label
/// must be present.
#[test]
fn navigator_forwards_the_top_pages_semantics_regression_23() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let root_page = Page::new("root-page");
    let pushed = Page::new("pushed-page");

    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        let page = root_page.clone();
        move |_: &mut ()| navigator(&ctrl, page.builder())
    };
    let mut state = ();

    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));
    assert!(
        contributing(&root, std::slice::from_ref(&root_page)).contains("root-page"),
        "the root page is in the accessibility tree"
    );

    controller.push(pushed.builder());
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));

    let present = contributing(&root, &[root_page.clone(), pushed.clone()]);
    assert!(
        present.contains("pushed-page"),
        "the pushed page contributes a node (this is #23: it did not before)"
    );
    assert!(
        !present.contains("root-page"),
        "the covered page beneath it does not"
    );
}

/// Opaque over opaque: the covered page is not laid out and not painted, so its
/// bounds are stale and it is unreachable by input — it is omitted, and the
/// omitted set is exactly the unrouted set.
#[test]
fn opaque_over_opaque_semantics_set_equals_the_input_routed_set() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let bottom = Page::new("opaque-bottom");
    let top = Page::new("opaque-top");
    let pages = [bottom.clone(), top.clone()];

    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        let page = bottom.clone();
        move |_: &mut ()| navigator(&ctrl, page.builder())
    };
    let mut state = ();

    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));
    assert_input_parity(&mut root, &mut state, &pages, &["opaque-bottom"]);

    controller.push(top.builder());
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));
    assert_input_parity(&mut root, &mut state, &pages, &["opaque-top"]);

    // …and a pop restores the bottom page to both reaches together.
    controller.pop();
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));
    assert_input_parity(&mut root, &mut state, &pages, &["opaque-bottom"]);
}

/// Transparent over opaque — the modal case, and the deliberate divergence from
/// paint culling. The page below a dialog is `PageVisibility::Visible`: still
/// **painted**, with live geometry. It is omitted anyway, because R23 tracks
/// **input routing**, not painting, and `route_top` routes only to the top
/// page. That is what makes a modal modal to assistive technology for free —
/// do not "fix" this to follow the painted range.
#[test]
fn transparent_over_opaque_omits_the_still_painted_page_below_because_input_does() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let base = Page::new("opaque-base");
    let dialog = Page::new("transparent-dialog");
    let pages = [base.clone(), dialog.clone()];

    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        let page = base.clone();
        move |_: &mut ()| navigator(&ctrl, page.builder())
    };
    let mut state = ();

    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));

    controller.push_with_options(dialog.builder(), PushOptions::transparent());
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));
    base.paints.set(0);
    root.paint(&mut NullScene, nav_frame_time(0));
    assert!(
        base.paints.get() > 0,
        "the page under a transparent overlay is still painted \
         (PageVisibility::Visible) — the divergence this test exists to pin"
    );

    // The dialog is forwarded (it IS `pages.last()`); the painted page under it
    // is not — exactly as input behaves.
    assert_input_parity(&mut root, &mut state, &pages, &["transparent-dialog"]);
}

/// Transparent over transparent: two stacked overlays. Only the topmost is
/// forwarded, matching input, even though all three pages paint.
#[test]
fn transparent_over_transparent_forwards_only_the_topmost_overlay() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let base = Page::new("opaque-base");
    let sheet = Page::new("transparent-sheet");
    let alert = Page::new("transparent-alert");
    let pages = [base.clone(), sheet.clone(), alert.clone()];

    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        let page = base.clone();
        move |_: &mut ()| navigator(&ctrl, page.builder())
    };
    let mut state = ();

    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));

    controller.push_with_options(sheet.builder(), PushOptions::transparent());
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));
    assert_input_parity(&mut root, &mut state, &pages, &["transparent-sheet"]);

    controller.push_with_options(alert.builder(), PushOptions::transparent());
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));
    base.paints.set(0);
    sheet.paints.set(0);
    root.paint(&mut NullScene, nav_frame_time(0));
    assert!(
        base.paints.get() > 0 && sheet.paints.get() > 0,
        "all three pages paint; only the topmost is forwarded"
    );
    assert_input_parity(&mut root, &mut state, &pages, &["transparent-alert"]);
}

/// Mid-transition, R23 needs no special case: `pages.last()` is the destination
/// page, so it alone is forwarded while both pages are still painting.
///
/// The routed set is measured differently here **on purpose**: a running
/// transition suppresses *all* page routing (`event_at`'s input block), a gate
/// that sits above routing rather than changing which page is the target. So a
/// screen reader activating the forwarded node during those ≤340ms hits exactly
/// the same suppression a finger does — the two reaches still agree.
#[test]
fn mid_transition_forwards_the_destination_page_and_input_is_globally_suppressed() {
    let controller: NavigatorController<()> = NavigatorController::new();
    let from = Page::new("from-page");
    let to = Page::new("to-page");
    let pages = [from.clone(), to.clone()];

    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app = {
        let ctrl = controller.clone();
        let page = from.clone();
        move |_: &mut ()| navigator(&ctrl, page.builder())
    };
    let mut state = ();

    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));
    root.paint(&mut NullScene, nav_frame_time(0));

    // Push with a long transition and advance only part-way into it.
    let spec = TransitionSpec::new(
        PageTransition::M3FadeThrough,
        Timing::Duration(Duration::from_millis(1000), Curve::Linear),
    );
    controller.push_with(to.builder(), spec);
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));
    root.paint(&mut NullScene, nav_frame_time(0));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));
    root.paint(&mut NullScene, nav_frame_time(200));
    assert!(
        controller.transition().active,
        "the transition is still running at 200ms of 1000ms"
    );

    // Only the destination page contributes, though BOTH pages are painting.
    assert_eq!(
        contributing(&root, &pages),
        ["to-page"].into_iter().collect::<BTreeSet<_>>(),
        "mid-transition, only the destination page is forwarded"
    );
    // And input reaches no page at all — the suppression a screen-reader
    // activation hits identically.
    assert!(
        routed(&mut root, &mut state, &pages).is_empty(),
        "input is globally suppressed mid-transition"
    );

    // Once it settles, the destination page is on both reaches again.
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));
    root.paint(&mut NullScene, nav_frame_time(1200));
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(200.0, 400.0));
    root.paint(&mut NullScene, nav_frame_time(1300));
    assert!(
        !controller.transition().active,
        "the transition settled by 1300ms"
    );
    assert_input_parity(&mut root, &mut state, &pages, &["to-page"]);
}
