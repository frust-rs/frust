//! Integration test for the semantics seam (phase-6c D1): drive a small widget
//! tree through a real `RenderRoot` rebuild + layout, then assert the collected
//! accessibility tree — node count, roles, labels/values, absolute bounds, and
//! parent/child structure.
//!
//! Also proves a widget with **no** `Widget::semantics` override (the trait's
//! defaulted no-op) contributes no node while recursion into its siblings
//! continues.

use frust_core::accesskit::{Role, Toggled};
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, RenderRoot, View,
    Widget, any,
};
use frust_text::TextContext;
use frust_widgets::{Column, EdgeInsets, Padding, PaddingView, button, checkbox, text};
use kurbo::Size;

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
