//! Integration tests for `wrap` (`crates/frust-widgets/src/wrap.rs`): line
//! breaking, spacing, alignment, direction, oversized and unbounded cases,
//! keyed reorder and child visiting.

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use frust_widgets::{Axis, WrapAlignment, WrapCrossAlignment, WrapView, wrap};
use kurbo::{Point, Size};
use peniko::Color;

/// A leaf whose widget size is fixed at build time and never rebuilt, so the
/// size doubles as an identity handle.
struct Leaf(Size);

struct LeafWidget(Size);

fn leaf(w: f64, h: f64) -> Leaf {
    Leaf(Size::new(w, h))
}

impl View<()> for Leaf {
    type Element = LeafWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LeafWidget {
        LeafWidget(self.0)
    }

    fn rebuild(&self, _prev: &Self, _el: &mut LeafWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
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

fn run(view: &WrapView<()>, bc: BoxConstraints) -> (Size, Vec<(Point, Size)>) {
    let mut counter = 0u64;
    let mut w = view.build(&mut BuildCtx::new(&mut counter));
    let size = w.layout(&mut LayoutCtx::new(), &bc);
    let mut rec = Rects::default();
    w.paint(&mut PaintCtx::new(Point::ZERO, size), &mut rec);
    (size, rec.0)
}

fn loose(w: f64, h: f64) -> BoxConstraints {
    BoxConstraints::loose(Size::new(w, h))
}

fn origins(rects: &[(Point, Size)]) -> Vec<(f64, f64)> {
    rects.iter().map(|(p, _)| (p.x, p.y)).collect()
}

fn three(view: WrapView<()>) -> WrapView<()> {
    view.child(leaf(30.0, 10.0))
        .child(leaf(30.0, 10.0))
        .child(leaf(30.0, 10.0))
}

#[test]
fn breaks_lines_at_the_boundary() {
    // Exactly 60 wide fits two 30s; the third wraps.
    let (size, r) = run(&three(wrap()), loose(60.0, 100.0));
    assert_eq!(origins(&r), vec![(0.0, 0.0), (30.0, 0.0), (0.0, 10.0)]);
    assert_eq!(size, Size::new(60.0, 20.0));
    // One unit short: only one fits per run.
    let (size, r) = run(&three(wrap()), loose(59.0, 100.0));
    assert_eq!(origins(&r), vec![(0.0, 0.0), (0.0, 10.0), (0.0, 20.0)]);
    assert_eq!(size, Size::new(30.0, 30.0));
}

#[test]
fn spacing_and_run_spacing() {
    // 30 + 5 + 30 = 65 fits; a third would need 100.
    let (size, r) = run(
        &three(wrap().spacing(5.0).run_spacing(7.0)),
        loose(70.0, 100.0),
    );
    assert_eq!(origins(&r), vec![(0.0, 0.0), (35.0, 0.0), (0.0, 17.0)]);
    assert_eq!(size, Size::new(65.0, 27.0));
}

#[test]
fn main_alignments_distribute_each_run() {
    let at = |a| {
        origins(&run(&three(wrap().alignment(a)), loose(100.0, 100.0)).1)
            .iter()
            .map(|p| p.0)
            .collect::<Vec<_>>()
    };
    // One run of 90 in a 100-wide tight box: 10 free.
    let tight = |a| {
        let v = three(wrap().alignment(a));
        origins(&run(&v, BoxConstraints::tight(Size::new(100.0, 10.0))).1)
            .iter()
            .map(|p| p.0)
            .collect::<Vec<_>>()
    };
    assert_eq!(at(WrapAlignment::Start), vec![0.0, 30.0, 60.0]);
    assert_eq!(tight(WrapAlignment::End), vec![10.0, 40.0, 70.0]);
    assert_eq!(tight(WrapAlignment::Center), vec![5.0, 35.0, 65.0]);
    assert_eq!(tight(WrapAlignment::SpaceBetween), vec![0.0, 35.0, 70.0]);
    let around = tight(WrapAlignment::SpaceAround);
    assert!((around[0] - 10.0 / 6.0).abs() < 1e-9);
    assert!((around[1] - (30.0 + 10.0 / 6.0 + 10.0 / 3.0)).abs() < 1e-9);
    let evenly = tight(WrapAlignment::SpaceEvenly);
    assert!((evenly[0] - 2.5).abs() < 1e-9 && (evenly[1] - 35.0).abs() < 1e-9);
}

#[test]
fn run_and_cross_alignment() {
    let v = |ra, ca| {
        wrap::<()>()
            .run_alignment(ra)
            .cross_alignment(ca)
            .child(leaf(50.0, 10.0))
            .child(leaf(50.0, 20.0))
    };
    // Two runs (width 60): cross extents 10 and 20 in a tight 100-tall box.
    let bc = BoxConstraints::tight(Size::new(60.0, 100.0));
    let ys = |view: WrapView<()>| run(&view, bc).1.iter().map(|r| r.0.y).collect::<Vec<_>>();
    assert_eq!(
        ys(v(WrapAlignment::Start, WrapCrossAlignment::Start)),
        vec![0.0, 10.0]
    );
    assert_eq!(
        ys(v(WrapAlignment::End, WrapCrossAlignment::Start)),
        vec![70.0, 80.0]
    );
    assert_eq!(
        ys(v(WrapAlignment::Center, WrapCrossAlignment::Start)),
        vec![35.0, 45.0]
    );
    assert_eq!(
        ys(v(WrapAlignment::SpaceBetween, WrapCrossAlignment::Start)),
        vec![0.0, 80.0]
    );
    // Cross alignment inside a run: a short child sits at the run's end/center.
    let mixed = |ca| {
        wrap::<()>()
            .cross_alignment(ca)
            .child(leaf(20.0, 10.0))
            .child(leaf(20.0, 30.0))
    };
    let y = |ca| run(&mixed(ca), loose(100.0, 100.0)).1[0].0.y;
    assert_eq!(y(WrapCrossAlignment::Start), 0.0);
    assert_eq!(y(WrapCrossAlignment::Center), 10.0);
    assert_eq!(y(WrapCrossAlignment::End), 20.0);
}

#[test]
fn vertical_direction_flows_down_then_across() {
    let v = three(
        wrap()
            .direction(Axis::Vertical)
            .spacing(2.0)
            .run_spacing(4.0),
    );
    // Heights 10: 10+2+10 = 22 fits in 25; third starts a new column.
    let (size, r) = run(&v, loose(200.0, 25.0));
    assert_eq!(origins(&r), vec![(0.0, 0.0), (0.0, 12.0), (34.0, 0.0)]);
    assert_eq!(size, Size::new(64.0, 22.0));
}

#[test]
fn oversized_child_gets_its_own_run_and_is_clamped() {
    let v = wrap::<()>()
        .child(leaf(10.0, 10.0))
        .child(leaf(500.0, 10.0))
        .child(leaf(10.0, 10.0));
    let (size, r) = run(&v, loose(100.0, 100.0));
    assert_eq!(r[1].1.width, 100.0, "clamped to the available main extent");
    assert_eq!(origins(&r), vec![(0.0, 0.0), (0.0, 10.0), (0.0, 20.0)]);
    assert_eq!(size, Size::new(100.0, 30.0));
}

#[test]
fn unbounded_main_is_a_single_run() {
    let bc = BoxConstraints::new(Size::ZERO, Size::new(f64::INFINITY, 100.0));
    let (size, r) = run(&three(wrap().spacing(5.0)), bc);
    assert_eq!(origins(&r), vec![(0.0, 0.0), (35.0, 0.0), (70.0, 0.0)]);
    assert_eq!(size, Size::new(100.0, 10.0));
}

#[test]
fn keyed_reorder_preserves_child_state() {
    let mut counter = 0u64;
    let before: WrapView<()> = wrap().keyed(1, leaf(10.0, 5.0)).keyed(2, leaf(20.0, 5.0));
    let mut w = before.build(&mut BuildCtx::new(&mut counter));
    let after: WrapView<()> = wrap().keyed(2, leaf(20.0, 5.0)).keyed(1, leaf(10.0, 5.0));
    after.rebuild(&before, &mut w, &mut BuildCtx::new(&mut counter));
    let size = w.layout(&mut LayoutCtx::new(), &loose(100.0, 100.0));
    let mut rec = Rects::default();
    w.paint(&mut PaintCtx::new(Point::ZERO, size), &mut rec);
    let widths: Vec<f64> = rec.0.iter().map(|r| r.1.width).collect();
    assert_eq!(widths, vec![20.0, 10.0]);
}

#[test]
fn visits_children_in_order() {
    let mut counter = 0u64;
    let w = three(wrap()).build(&mut BuildCtx::new(&mut counter));
    let mut n = 0;
    w.visit_children(&mut |_| n += 1);
    assert_eq!(n, 3);
    assert!(!w.hit_test_self());
}
