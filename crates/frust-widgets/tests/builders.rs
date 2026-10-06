//! Fluent builders (`column()`/`row()`/`stack()` + `.child`/`.flex`/`.keyed`/
//! `.children`/`.when`/`.when_some`) must lay out exactly like the legacy
//! explicit-`FlexChild` / `Stack(vec![any(..)])` forms.

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget, any,
};
use frust_widgets::{Axis, FlexView, Stack, column, flexible, inflexible, keyed, row, stack};
use kurbo::{Point, Size};
use peniko::Color;

struct Leaf(Size);

fn leaf(w: f64, h: f64) -> Leaf {
    Leaf(Size::new(w, h))
}

struct LeafWidget(Size);

impl View<()> for Leaf {
    type Element = LeafWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LeafWidget {
        LeafWidget(self.0)
    }

    fn rebuild(&self, _prev: &Self, el: &mut LeafWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        el.0 = self.0;
        ChangeFlags::LAYOUT
    }
}

impl Widget for LeafWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.0)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
    }
}

#[derive(Default)]
struct Rects(Vec<(Point, Size)>);

impl PaintScene for Rects {
    fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
        self.0.push((origin, size));
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
}

/// Lay out + paint `view` under tight 300x200 constraints; return the root size
/// and every leaf rect in paint order.
fn rects<V: View<()>>(view: &V) -> (Size, Vec<(Point, Size)>) {
    let mut counter = 0u64;
    let mut w = view.build(&mut BuildCtx::new(&mut counter));
    let size = w.layout(
        &mut LayoutCtx::new(),
        &BoxConstraints::tight(Size::new(300.0, 200.0)),
    );
    let mut rec = Rects::default();
    w.paint(&mut PaintCtx::new(Point::ZERO, size), &mut rec);
    (size, rec.0)
}

#[test]
fn builder_chain_matches_explicit_flex_children() {
    let built: FlexView<()> = column()
        .child(leaf(30.0, 10.0))
        .flex(2, leaf(20.0, 10.0))
        .keyed(7, leaf(10.0, 25.0));
    let legacy = FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(leaf(30.0, 10.0)),
            flexible(2, leaf(20.0, 10.0)),
            keyed(7, leaf(10.0, 25.0)),
        ],
    );
    let (bs, br) = rects(&built);
    let (ls, lr) = rects(&legacy);
    assert_eq!(bs, ls);
    assert_eq!(br.len(), 3);
    assert_eq!(br, lr);

    let built_row: FlexView<()> = row().child(leaf(30.0, 10.0)).flex(1, leaf(5.0, 5.0));
    let legacy_row = FlexView::new(
        Axis::Horizontal,
        vec![inflexible(leaf(30.0, 10.0)), flexible(1, leaf(5.0, 5.0))],
    );
    assert_eq!(rects(&built_row), rects(&legacy_row));
}

#[test]
fn push_accepts_prebuilt_children() {
    let built: FlexView<()> = column().push(flexible(1, leaf(5.0, 5.0)));
    let legacy = FlexView::new(Axis::Vertical, vec![flexible(1, leaf(5.0, 5.0))]);
    assert_eq!(rects(&built), rects(&legacy));
}

#[test]
fn when_and_when_some_apply_conditionally() {
    let base = || column::<()>().child(leaf(10.0, 10.0));
    assert_eq!(
        rects(&base().when(false, |c| c.child(leaf(10.0, 10.0))))
            .1
            .len(),
        1
    );
    assert_eq!(
        rects(&base().when(true, |c| c.child(leaf(10.0, 10.0))))
            .1
            .len(),
        2
    );
    assert_eq!(
        rects(&base().when_some(None::<u32>, |c, _| c.child(leaf(1.0, 1.0))))
            .1
            .len(),
        1
    );
    assert_eq!(
        rects(&base().when_some(Some(3u32), |c, n| c.child(leaf(n as f64, 1.0))))
            .1
            .len(),
        2
    );

    let s = || stack::<()>().child(leaf(10.0, 10.0));
    assert_eq!(
        rects(&s().when(false, |s| s.child(leaf(5.0, 5.0)))).1.len(),
        1
    );
    assert_eq!(
        rects(&s().when(true, |s| s.child(leaf(5.0, 5.0)))).1.len(),
        2
    );
    assert_eq!(
        rects(&s().when_some(None::<u32>, |s, _| s.child(leaf(5.0, 5.0))))
            .1
            .len(),
        1
    );
    assert_eq!(
        rects(&s().when_some(Some(1u32), |s, _| s.child(leaf(5.0, 5.0))))
            .1
            .len(),
        2
    );
}

#[test]
fn children_accepts_a_concrete_iterator_without_any() {
    let built: FlexView<()> = column().children((1..=3).map(|i| leaf(10.0, i as f64 * 10.0)));
    let legacy = FlexView::new(
        Axis::Vertical,
        (1..=3)
            .map(|i| inflexible(leaf(10.0, i as f64 * 10.0)))
            .collect(),
    );
    assert_eq!(rects(&built), rects(&legacy));
    let st = stack::<()>().children((1..=2).map(|i| leaf(i as f64, i as f64)));
    assert_eq!(rects(&st).1.len(), 2);
}

#[test]
fn stack_builder_matches_legacy_stack() {
    let built = stack::<()>()
        .child(leaf(40.0, 20.0))
        .child(leaf(10.0, 30.0));
    let legacy = Stack(vec![any(leaf(40.0, 20.0)), any(leaf(10.0, 30.0))]);
    assert_eq!(rects(&built), rects(&legacy));
}

#[test]
fn already_erased_child_is_not_double_erased() {
    let erased = any::<(), _>(leaf(40.0, 20.0));
    let built = column::<()>().child(erased);
    let legacy = FlexView::new(Axis::Vertical, vec![inflexible(leaf(40.0, 20.0))]);
    assert_eq!(rects(&built), rects(&legacy));
    let st = stack::<()>().child(any::<(), _>(leaf(4.0, 4.0)));
    assert_eq!(rects(&st).1.len(), 1);
}
