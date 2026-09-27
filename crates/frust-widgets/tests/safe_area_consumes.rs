//! Integration tests for [`frust_widgets::safe_area`]'s consumption contract:
//! a safe area pads its child by the enabled edges' window insets **and**
//! removes that padding from its subtree, so a descendant reading
//! `ctx.window_insets()` sees zero safe-area padding on every consumed edge
//! (Flutter's `SafeArea` / `MediaQuery.removePadding` parity).
//!
//! Driven through a real [`RenderRoot`] rebuild + [`RenderRoot::set_insets`] +
//! layout (+ paint), mirroring `safe_area.rs`'s own `#[cfg(test)]` ordering and
//! `scaffold_layout.rs`'s hand-written fixtures. The one fixture is
//! [`Probe`]: a leaf that records the insets it reads at layout and at paint
//! time, and self-sizes by its top/bottom padding the way a self-insetting
//! chrome bar does — so a double inset would show up in its size as well as in
//! the recorded value.

use std::cell::RefCell;
use std::rc::Rc;

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, DiscardScene, FrameTime, InspectNode, LayoutCtx,
    PaintCtx, PaintScene, RenderRoot, View, Widget, WindowEdgeInsets, WindowInsets,
};
use frust_widgets::{EdgeInsets, safe_area};
use kurbo::{Point, Size};

// -- Fixture ---------------------------------------------------------------

/// What a [`Probe`] observed on its most recent layout and paint.
#[derive(Default)]
struct Seen {
    layout: Option<WindowInsets>,
    paint: Option<WindowInsets>,
}

type Record = Rc<RefCell<Seen>>;

/// The probe's content size before it adds any inset.
const CONTENT: Size = Size::new(40.0, 20.0);

/// A leaf that records `ctx.window_insets()` at layout and paint time and
/// grows vertically by the top + bottom safe-area padding it reads — the
/// self-sizing chrome shape (`scaffold_layout.rs`'s `SelfInsetBar`).
struct Probe {
    record: Record,
}
fn probe(record: &Record) -> Probe {
    Probe {
        record: Rc::clone(record),
    }
}
struct ProbeWidget {
    record: Record,
}
impl View<()> for Probe {
    type Element = ProbeWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
        ProbeWidget {
            record: Rc::clone(&self.record),
        }
    }
    fn rebuild(&self, _prev: &Self, el: &mut ProbeWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        el.record = Rc::clone(&self.record);
        ChangeFlags::NONE
    }
}
impl Widget for ProbeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let insets = ctx.window_insets();
        self.record.borrow_mut().layout = Some(insets);
        let padding = insets.padding();
        bc.constrain(Size::new(
            CONTENT.width,
            CONTENT.height + padding.top + padding.bottom,
        ))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.record.borrow_mut().paint = Some(ctx.window_insets());
    }
}

// -- Harness ---------------------------------------------------------------

/// The insets every test pushes unless it needs the IME: system bars of
/// (l10, t20, r30, b40), no keyboard.
fn bars() -> WindowInsets {
    WindowInsets::new(
        WindowEdgeInsets::new(10.0, 20.0, 30.0, 40.0),
        WindowEdgeInsets::ZERO,
    )
}

/// Rebuild, push `insets`, lay out against a 500x500 window, paint once, and
/// return the inspect snapshot.
fn run<V: View<()>>(
    app_logic: &mut impl FnMut(&mut ()) -> V,
    insets: WindowInsets,
) -> Vec<InspectNode> {
    let mut root: RenderRoot<(), V> = RenderRoot::new();
    let mut state = ();
    root.rebuild(app_logic, &mut state);
    root.set_insets(insets);
    root.layout(Size::new(500.0, 500.0));
    root.paint(&mut DiscardScene, FrameTime::from_nanos(0));
    root.inspect()
}

/// The probe's inspect node (unique by type name in every tree here).
fn probe_node(nodes: &[InspectNode]) -> &InspectNode {
    nodes
        .iter()
        .find(|n| n.type_name.ends_with("ProbeWidget"))
        .expect("probe node present")
}

fn layout_seen(record: &Record) -> WindowInsets {
    record.borrow().layout.expect("probe was laid out")
}

fn paint_seen(record: &Record) -> WindowInsets {
    record.borrow().paint.expect("probe was painted")
}

// -- (a) all edges consumed --------------------------------------------------

#[test]
fn child_of_a_safe_area_sees_zero_padding_on_every_consumed_edge() {
    let record = Record::default();
    let nodes = run(&mut |_: &mut ()| safe_area(probe(&record)), bars());

    let seen = layout_seen(&record);
    assert_eq!(seen.padding(), WindowEdgeInsets::ZERO);
    assert_eq!(
        seen.view_padding,
        WindowEdgeInsets::ZERO,
        "view_padding is reduced by exactly the consumed padding"
    );
    assert_eq!(
        seen.view_insets,
        WindowEdgeInsets::ZERO,
        "view_insets untouched"
    );

    // The safe area itself still pads: the probe sits at (left, top) and, since
    // it read zero top/bottom padding, did not grow by the bars a second time.
    let node = probe_node(&nodes);
    assert_eq!(node.bounds.origin(), Point::new(10.0, 20.0));
    assert_eq!(node.bounds.size(), CONTENT, "no double inset");
}

// -- (b) a disabled edge stays visible to the child ---------------------------

#[test]
fn a_disabled_edge_is_not_consumed() {
    let record = Record::default();
    let nodes = run(
        &mut |_: &mut ()| safe_area(probe(&record)).top(false),
        bars(),
    );

    let seen = layout_seen(&record);
    assert_eq!(
        seen.padding(),
        WindowEdgeInsets::new(0.0, 20.0, 0.0, 0.0),
        "the top edge was not consumed, so the child still sees it"
    );
    assert_eq!(
        seen.view_padding,
        WindowEdgeInsets::new(0.0, 20.0, 0.0, 0.0)
    );

    // Not padded at the top by the safe area; the probe self-insets instead.
    let node = probe_node(&nodes);
    assert_eq!(node.bounds.origin(), Point::new(10.0, 0.0));
    assert_eq!(node.bounds.size(), Size::new(40.0, 20.0 + 20.0));
}

#[test]
fn no_enabled_edges_pass_the_insets_through_unchanged() {
    let record = Record::default();
    run(
        &mut |_: &mut ()| {
            safe_area(probe(&record))
                .left(false)
                .top(false)
                .right(false)
                .bottom(false)
        },
        bars(),
    );
    assert_eq!(layout_seen(&record), bars());
    assert_eq!(paint_seen(&record), bars());
}

// -- (c) nested safe areas consume once ---------------------------------------

#[test]
fn nested_safe_areas_consume_once() {
    let single = Record::default();
    let single_nodes = run(&mut |_: &mut ()| safe_area(probe(&single)), bars());

    let nested = Record::default();
    let nested_nodes = run(
        &mut |_: &mut ()| safe_area(safe_area(probe(&nested))),
        bars(),
    );

    // The outer safe area pads by the bars (bottom 40); the inner one sees
    // zero padding and pads by nothing, so the probe lands exactly where a
    // single safe area puts it.
    assert_eq!(
        probe_node(&nested_nodes).bounds,
        probe_node(&single_nodes).bounds,
        "the inner safe area adds no second inset"
    );
    assert_eq!(
        probe_node(&nested_nodes).bounds.origin(),
        Point::new(10.0, 20.0)
    );
    assert_eq!(layout_seen(&nested), layout_seen(&single));
    assert_eq!(layout_seen(&nested).padding(), WindowEdgeInsets::ZERO);
}

#[test]
fn nested_inner_safe_area_pads_only_by_its_minimum() {
    let record = Record::default();
    let nodes = run(
        &mut |_: &mut ()| safe_area(safe_area(probe(&record)).minimum(EdgeInsets::all(8.0))),
        bars(),
    );

    // Outer pads (10, 20); the inner one sees zero window padding and falls
    // back to its 8px floor on every edge.
    assert_eq!(
        probe_node(&nodes).bounds.origin(),
        Point::new(10.0 + 8.0, 20.0 + 8.0)
    );
    // `minimum` is not window inset: it consumes nothing further.
    assert_eq!(layout_seen(&record).padding(), WindowEdgeInsets::ZERO);
    assert_eq!(layout_seen(&record).view_padding, WindowEdgeInsets::ZERO);
}

// -- (d) the IME still reaches the subtree ------------------------------------

#[test]
fn ime_view_insets_flow_through_a_consuming_safe_area() {
    let insets = WindowInsets::new(
        WindowEdgeInsets::new(0.0, 24.0, 0.0, 40.0),
        WindowEdgeInsets::new(0.0, 0.0, 0.0, 300.0),
    );
    // Before: the keyboard already covers the bottom bar, so the root-level
    // bottom padding is 0.
    assert_eq!(insets.padding().bottom, 0.0);

    let record = Record::default();
    run(&mut |_: &mut ()| safe_area(probe(&record)), insets);

    // After: still 0 at the bottom, and the keyboard is still visible to the
    // child for its own avoidance.
    let seen = layout_seen(&record);
    assert_eq!(seen.padding().bottom, 0.0);
    assert_eq!(seen.padding(), WindowEdgeInsets::ZERO);
    assert_eq!(seen.view_insets.bottom, 300.0);
    assert_eq!(seen.view_insets, insets.view_insets);
    // The IME-covered bottom edge had nothing to consume.
    assert_eq!(seen.view_padding.bottom, 40.0);
    assert_eq!(seen.view_padding.top, 0.0);
}

// -- (e) paint-time reads match layout-time reads -----------------------------

#[test]
fn paint_time_insets_match_layout_time_insets() {
    let all = Record::default();
    run(&mut |_: &mut ()| safe_area(probe(&all)), bars());
    assert_eq!(paint_seen(&all), layout_seen(&all));
    assert_eq!(paint_seen(&all).padding(), WindowEdgeInsets::ZERO);

    let partial = Record::default();
    run(
        &mut |_: &mut ()| safe_area(probe(&partial)).top(false),
        bars(),
    );
    assert_eq!(paint_seen(&partial), layout_seen(&partial));

    let nested = Record::default();
    run(
        &mut |_: &mut ()| safe_area(safe_area(probe(&nested)).bottom(false)).bottom(false),
        bars(),
    );
    assert_eq!(paint_seen(&nested), layout_seen(&nested));
    assert_eq!(
        paint_seen(&nested).padding(),
        WindowEdgeInsets::new(0.0, 0.0, 0.0, 40.0),
        "a bottom edge neither safe area consumed reaches the painted child"
    );
}
