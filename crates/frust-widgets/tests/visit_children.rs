//! Integration test for the child-visitation seam: build real widget trees
//! through a `RenderRoot`, then assert what `RenderRoot::inspect` reports —
//! i.e. that a container's retained `ChildPod`s actually surface, with the
//! right concrete type names and in paint order.
//!
//! The seam is implemented once in the authoring toolkit
//! (`frust_widgets::authoring::visit_children!` over
//! [`VisitPods`](frust_widgets::authoring::VisitPods)) and used one line per
//! container, so the coverage split here is:
//!
//! 1. **Toolkit-level**, one test per field shape the macro handles —
//!    `ChildPod` (Padding), `Vec<ChildPod>` (Column), `Option<ChildPod>`
//!    (SizedBox, present and absent), and a multi-slot site naming both shapes
//!    at once (`PatternSwitcher`'s `exiting`/`child`, empty and filled) — plus
//!    a catalog widget proving the same seam covers the design-system tier.
//! 2. **Per hand-written site**, one test each for the containers whose
//!    children live behind their own row/slot struct and therefore implement
//!    `VisitPods` by hand: `fab_menu` (item rows) and `navigator` (the page
//!    stack).

use std::any::Any;

use frust_core::{AnyView, InspectNode, RenderRoot, View, any};
use frust_text::TextContext;
use frust_widgets::motion::patterns::FadeThrough;
use frust_widgets::motion::switcher::{PatternSwitcherView, pattern_switcher};
use frust_widgets::{
    Column, EdgeInsets, NavigatorController, NavigatorView, Padding, SizedBox, navigator, text,
};
use kurbo::Size;

/// Build + lay out `app_logic`'s tree under a 400x400 window and return the
/// resulting inspect snapshot.
fn inspect<V: View<()>>(app_logic: &mut impl FnMut(&mut ()) -> V) -> Vec<InspectNode> {
    let mut root: RenderRoot<(), V> = RenderRoot::new();
    let mut state = ();
    root.rebuild(app_logic, &mut state);
    let mut text_ctx = TextContext::new();
    root.layout_with_text(Size::new(400.0, 400.0), &mut text_ctx as &mut dyn Any);
    root.inspect()
}

/// Each node's bare widget type name (no module path, no generic arguments),
/// in pre-order — `frust_widgets::nav::navigator::NavigatorWidget<()>` reads as
/// `NavigatorWidget`.
fn short_names(nodes: &[InspectNode]) -> Vec<&str> {
    nodes
        .iter()
        .map(|n| {
            let bare = n.type_name.split('<').next().unwrap_or(n.type_name);
            bare.rsplit("::").next().unwrap_or(bare)
        })
        .collect()
}

/// How many nodes report `name` as their short type name.
fn count(nodes: &[InspectNode], name: &str) -> usize {
    short_names(nodes).iter().filter(|n| **n == name).count()
}

#[test]
fn a_single_child_wrapper_publishes_its_child() {
    // The `child: ChildPod` shape. Padding contributes one node and its child
    // hangs under it, one depth deeper, inset by the padding's own origin.
    let nodes = inspect(&mut |_: &mut ()| Padding(EdgeInsets::all(10.0), text("hi")));
    assert_eq!(short_names(&nodes), vec!["PaddingWidget", "TextWidget"]);
    assert_eq!(nodes[0].children, vec![nodes[1].id]);
    assert_eq!(nodes[1].parent, Some(nodes[0].id));
    assert_eq!(nodes[1].depth, nodes[0].depth + 1);
    assert_eq!(
        nodes[1].bounds.origin().x,
        nodes[0].bounds.origin().x + 10.0
    );
    assert_eq!(
        nodes[1].bounds.origin().y,
        nodes[0].bounds.origin().y + 10.0
    );
    assert!(nodes[1].bounds.width() > 0.0, "the child was laid out");
}

#[test]
fn a_multi_child_container_publishes_every_child_in_order() {
    // The `children: Vec<ChildPod>` shape, nested two levels deep so the walk
    // has to recurse rather than stop at the first container.
    let nodes = inspect(&mut |_: &mut ()| {
        Column(vec![
            any(text("a")),
            any(Padding(EdgeInsets::all(1.0), text("b"))),
        ])
    });
    assert_eq!(
        short_names(&nodes),
        vec!["FlexWidget", "TextWidget", "PaddingWidget", "TextWidget"],
        "pre-order: a node precedes its descendants, siblings in paint order"
    );
    assert_eq!(nodes[0].children, vec![nodes[1].id, nodes[2].id]);
    assert_eq!(nodes[2].children, vec![nodes[3].id]);
    assert_eq!(
        nodes.iter().map(|n| n.depth).collect::<Vec<_>>(),
        [0, 1, 1, 2]
    );
}

#[test]
fn an_optional_slot_publishes_only_when_present() {
    // The `child: Option<ChildPod>` shape, both ways.
    let with_child =
        inspect(&mut |_: &mut ()| SizedBox::<()>(Some(20.0), Some(20.0)).child(text("x")));
    assert_eq!(
        short_names(&with_child),
        vec!["SizedBoxWidget", "TextWidget"]
    );

    let empty = inspect(&mut |_: &mut ()| SizedBox::<()>(Some(20.0), Some(20.0)));
    assert_eq!(short_names(&empty), vec!["SizedBoxWidget"]);
    assert!(empty[0].children.is_empty());
}

#[cfg(feature = "material")]
#[test]
fn a_catalog_widget_is_covered_by_the_same_toolkit_seam() {
    use frust_widgets::{CardVariant, card};

    let nodes = inspect(&mut |_: &mut ()| {
        card(
            CardVariant::Filled,
            Column(vec![any(text("title")), any(text("body"))]),
        )
    });
    assert_eq!(
        short_names(&nodes),
        vec!["CardWidget", "FlexWidget", "TextWidget", "TextWidget"]
    );
}

#[cfg(feature = "material")]
#[test]
fn a_catalog_widget_publishes_its_per_item_slots() {
    // `NavigationBarWidget { items: Vec<ChildPod> }` where each item is itself
    // a `NavItemWidget { icon, label }` — two macro sites composing.
    use frust_widgets::{nav_item, navigation_bar};

    let nodes = inspect(&mut |_: &mut ()| {
        navigation_bar(
            vec![
                nav_item("one").icon(any(text("i1"))),
                nav_item("two").icon(any(text("i2"))),
            ],
            0,
            |_: &mut (), _| {},
        )
    });
    assert_eq!(count(&nodes, "NavigationBarWidget"), 1);
    assert_eq!(count(&nodes, "NavItemWidget"), 2);
    // Each item publishes its icon and its label.
    assert_eq!(count(&nodes, "TextWidget"), 4);
}

#[test]
fn a_multi_slot_container_publishes_each_filled_slot() {
    // The multi-field macro shape: `visit_children!(exiting, child)` names one
    // `Option<ChildPod>` slot and one `ChildPod` slot on the same site. At rest
    // the transient `exiting` slot is empty, so only the live child appears.
    let nodes = inspect(&mut |_: &mut ()| {
        pattern_switcher(
            0u32,
            FadeThrough,
            Column(vec![any(text("t")), any(text("d"))]),
        )
    });
    assert_eq!(
        short_names(&nodes),
        vec![
            "PatternSwitcherWidget",
            "FlexWidget",
            "TextWidget",
            "TextWidget"
        ],
        "the empty exiting slot contributes nothing"
    );
    assert_eq!(nodes[0].children.len(), 1);
}

#[cfg(feature = "material")]
#[test]
fn fab_menu_publishes_its_item_rows_and_its_trigger() {
    // Hand-written site: `FabMenuItemPod { icon, label }` implements
    // `VisitPods`, so an open menu shows two pods per item plus the trigger.
    use frust_widgets::{fab_menu, fab_menu_item};

    let nodes = inspect(&mut |_: &mut ()| {
        fab_menu(
            any(text("+")),
            true,
            vec![
                fab_menu_item(any(text("a")), "alpha", |_: &mut ()| {}),
                fab_menu_item(any(text("b")), "beta", |_: &mut ()| {}),
            ],
            |_: &mut ()| {},
        )
    });
    assert_eq!(count(&nodes, "FabMenuWidget"), 1);
    // Two items x (icon + label), plus the trigger icon.
    assert_eq!(nodes[0].children.len(), 5);
    assert_eq!(count(&nodes, "TextWidget"), 5);
}

#[test]
fn a_transient_slot_publishes_its_child_only_while_one_is_playing() {
    // The other half of the multi-slot shape above: `exiting` fills only while
    // a transition runs, so the inspector must pick it up mid-flight and drop
    // it again once the slot empties. Driven by an identity change (the
    // switcher freezes the outgoing child at rebuild), so no paint clock is
    // needed here.
    let mut root: RenderRoot<u32, _> = RenderRoot::new();
    let mut app_logic = |key: &mut u32| -> PatternSwitcherView<u32, FadeThrough> {
        pattern_switcher(*key, FadeThrough, text("page"))
    };
    let mut state = 0u32;
    let mut text_ctx = TextContext::new();

    root.rebuild(&mut app_logic, &mut state);
    root.layout_with_text(Size::new(400.0, 400.0), &mut text_ctx as &mut dyn Any);
    assert_eq!(
        short_names(&root.inspect()),
        vec!["PatternSwitcherWidget", "TextWidget"],
        "at rest the empty exiting slot contributes nothing"
    );

    state = 1;
    root.rebuild(&mut app_logic, &mut state);
    root.layout_with_text(Size::new(400.0, 400.0), &mut text_ctx as &mut dyn Any);
    let playing = root.inspect();
    assert_eq!(
        short_names(&playing),
        vec!["PatternSwitcherWidget", "TextWidget", "TextWidget"],
        "the frozen exiting child (and the incoming one) both hang under the switcher"
    );
    assert_eq!(playing[0].children.len(), 2);
}

#[test]
fn the_navigator_publishes_every_retained_page() {
    // Hand-written site: pages live in `PageEntry`, and an inspector shows the
    // whole retained stack — including a page covered by the one above it,
    // which input routing and semantics both (deliberately) omit.
    let controller: NavigatorController<()> = NavigatorController::new();
    controller.push(|| any(text("second")));
    let ctl = controller.clone();
    let nodes = inspect(&mut move |_: &mut ()| -> NavigatorView<()> {
        navigator(&ctl, || -> AnyView<()> { any(text("root")) })
    });
    assert_eq!(count(&nodes, "NavigatorWidget"), 1);
    assert_eq!(
        count(&nodes, "TextWidget"),
        2,
        "both the covered root page and the pushed page are retained"
    );
    assert_eq!(nodes[0].children.len(), 2);
}

#[test]
fn a_type_swapped_child_reports_its_new_widget_type() {
    // The pod's recorded name is refreshed on the one path that can change it:
    // an `AnyView` concrete-type swap through the shared child plumbing.
    let mut root: RenderRoot<bool, _> = RenderRoot::new();
    let mut app_logic = |editing: &mut bool| -> frust_widgets::PaddingView<bool> {
        Padding(
            EdgeInsets::all(1.0),
            if *editing {
                any(frust_widgets::text_input("x", |_: &mut bool, _| {}))
            } else {
                any(text("x"))
            },
        )
    };
    let mut state = false;
    root.rebuild(&mut app_logic, &mut state);
    let mut text_ctx = TextContext::new();
    root.layout_with_text(Size::new(400.0, 400.0), &mut text_ctx as &mut dyn Any);
    assert_eq!(
        short_names(&root.inspect()),
        vec!["PaddingWidget", "TextWidget"]
    );

    state = true;
    root.rebuild(&mut app_logic, &mut state);
    root.layout_with_text(Size::new(400.0, 400.0), &mut text_ctx as &mut dyn Any);
    assert_eq!(
        short_names(&root.inspect()),
        vec!["PaddingWidget", "TextInputWidget"],
        "the swapped-in widget's own type name replaces the dead one's"
    );
}
