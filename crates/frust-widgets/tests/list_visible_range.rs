//! `ListView::on_visible_range` — the first and last row intersecting the
//! viewport, delivered like the near-edge callbacks (one event late, only on a
//! change), in every extent mode.
//!
//! Every list is built inside a `Component::build`, as app code builds one, and
//! driven from outside through the attached `ScrollController`.

use std::cell::RefCell;
use std::rc::Rc;

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, Component, ComponentView, FrameTime, LayoutCtx,
    PaintCtx, PaintScene, RenderRoot, View, Widget, component,
};
use frust_widgets::{ChildKey, ListView, ScrollController};
use kurbo::{Point, Size};

const WINDOW: Size = Size::new(320.0, 640.0);
const ROW: f64 = 40.0;
/// Natural height of a probe row, which only the estimated mode lets decide.
const NATURAL: f64 = 10.0;

/// Quarter-pixel multiples, so prefix sums are exact in `f64`.
fn mixed_extent(i: usize) -> f64 {
    const KINDS: [f64; 5] = [24.0, 56.25, 88.5, 132.0, 40.75];
    KINDS[(i.wrapping_mul(2_654_435_761) >> 7) % KINDS.len()]
}

struct Probe;

struct ProbeWidget;

impl<S: 'static> View<S> for Probe {
    type Element = ProbeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
        ProbeWidget
    }

    fn rebuild(&self, _: &Self, _: &mut ProbeWidget, _: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for ProbeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(bc.max().width, NATURAL))
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

struct NullScene;

impl PaintScene for NullScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
}

/// 40 px rows with two runs of three 0 px rows: rows 20..23 all start at
/// 800, rows 40..43 at 1480.
fn zero_run_extent(i: usize) -> f64 {
    if (20..23).contains(&i) || (40..43).contains(&i) {
        0.0
    } else {
        ROW
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Uniform,
    Estimated,
    Exact,
    ExactZeroRuns,
}

/// What the handler saw.
#[derive(Clone, Default)]
struct Seen {
    calls: Rc<RefCell<Vec<(usize, usize)>>>,
}

struct Screen {
    seen: Seen,
    count: usize,
    mode: Mode,
    controller: ScrollController,
}

impl Component for Screen {
    type State = Seen;

    fn init(&self) -> Seen {
        self.seen.clone()
    }

    fn build(&self, _state: &mut Seen) -> impl View<Seen> {
        let list: ListView<Seen> = match self.mode {
            Mode::Uniform => ListView::builder(self.count, ROW, |_| Probe),
            Mode::Estimated => {
                ListView::builder_keyed(self.count, ROW, |i| ChildKey::new(i as u64), |_| Probe)
                    .estimated_item_extent(48.0)
            }
            Mode::Exact => {
                ListView::builder(self.count, ROW, |_| Probe).item_extent_of(mixed_extent)
            }
            Mode::ExactZeroRuns => {
                ListView::builder(self.count, ROW, |_| Probe).item_extent_of(zero_run_extent)
            }
        };
        list.on_visible_range(|seen: &mut Seen, first, last| {
            seen.calls.borrow_mut().push((first, last))
        })
        .controller(self.controller.clone())
    }
}

struct Harness {
    root: RenderRoot<Seen, ComponentView<Screen>>,
    seen: Seen,
    controller: ScrollController,
    count: usize,
    mode: Mode,
    ms: f64,
}

impl Harness {
    fn new(count: usize, mode: Mode) -> Self {
        let mut h = Self {
            root: RenderRoot::new(),
            seen: Seen::default(),
            controller: ScrollController::new(),
            count,
            mode,
            ms: 0.0,
        };
        h.settle();
        h
    }

    fn frame(&mut self) -> ChangeFlags {
        let (count, mode) = (self.count, self.mode);
        let controller = self.controller.clone();
        let seen = self.seen.clone();
        let mut logic = move |_: &mut Seen| {
            component(Screen {
                seen: seen.clone(),
                count,
                mode,
                controller: controller.clone(),
            })
        };
        let flags = self.root.rebuild(&mut logic, &mut self.seen);
        self.root.layout(WINDOW);
        self.root.paint(
            &mut NullScene,
            FrameTime::from_nanos((self.ms * 1e6) as u64),
        );
        self.ms += 16.0;
        flags
    }

    fn settle(&mut self) {
        for _ in 0..40 {
            self.frame();
        }
    }

    fn jump(&mut self, y: f64) {
        self.controller.jump_to(y);
        self.settle();
    }

    fn calls(&self) -> Vec<(usize, usize)> {
        self.seen.calls.borrow().clone()
    }

    fn last(&self) -> Option<(usize, usize)> {
        self.calls().last().copied()
    }
}

/// The rows of `extents` (laid end to end) with positive overlap with
/// `[top, top + WINDOW.height)`.
fn expected_range(extents: impl Iterator<Item = f64>, top: f64) -> (usize, usize) {
    let bottom = top + WINDOW.height;
    let mut y = 0.0;
    let mut range: Option<(usize, usize)> = None;
    for (i, extent) in extents.enumerate() {
        if y + extent > top && y < bottom {
            range = Some(range.map_or((i, i), |(f, _)| (f, i)));
        }
        y += extent;
    }
    range.expect("some row is visible")
}

#[test]
fn the_first_layout_reports_the_rows_on_screen() {
    let h = Harness::new(100, Mode::Uniform);
    // 640 / 40 = 16 rows, the 16th ending exactly on the bottom edge.
    assert_eq!(h.calls(), vec![(0, 15)]);
}

#[test]
fn scrolling_changes_the_range() {
    let mut h = Harness::new(100, Mode::Uniform);
    h.jump(100.0);
    // Rows 2..=18: row 2 spans 80..120 (straddles the top), row 18 spans
    // 720..760 and crosses the bottom edge at 740.
    assert_eq!(h.last(), Some((2, 18)));
    // On a row boundary the row above only touches the top edge: not visible.
    h.jump(400.0);
    assert_eq!(h.last(), Some((10, 25)));
    assert_eq!(h.calls().first(), Some(&(0, 15)));
}

#[test]
fn an_unchanged_range_is_not_redelivered_and_causes_no_rebuild() {
    let mut h = Harness::new(100, Mode::Uniform);
    h.jump(100.0);
    let calls = h.calls().len();
    // Same range from a different offset (still rows 2..=18): no event.
    h.jump(110.0);
    assert_eq!(h.last(), Some((2, 18)));
    assert_eq!(h.calls().len(), calls, "same range, no delivery");
    for _ in 0..5 {
        let flags = h.frame();
        assert!(flags.is_empty(), "an idle frame changes nothing: {flags:?}");
        assert!(
            !frust_core::event::has_pending_result_flush(),
            "and owes no flush"
        );
    }
    assert_eq!(h.calls().len(), calls, "idle frames deliver nothing");
}

#[test]
fn an_empty_list_delivers_nothing() {
    let h = Harness::new(0, Mode::Uniform);
    assert!(h.calls().is_empty());
    let h = Harness::new(0, Mode::Exact);
    assert!(h.calls().is_empty());
}

#[test]
fn the_estimated_mode_reports_measured_rows() {
    let mut h = Harness::new(500, Mode::Estimated);
    // Every row measures NATURAL tall: 640 / 10 = 64 rows fill the window.
    assert_eq!(h.last(), Some((0, 63)));
    h.jump(100.0);
    let (first, last) = h.last().expect("delivered");
    assert!(first <= 10 && last >= 73, "got {first}..={last}");
    assert!(last - first >= 63);
}

#[test]
fn the_exact_mode_reports_rows_off_the_prefix_sum() {
    let mut h = Harness::new(1_000, Mode::Exact);
    assert_eq!(
        h.last(),
        Some(expected_range((0..1_000).map(mixed_extent), 0.0))
    );
    let mut top = 0.0;
    let tops: Vec<f64> = (0..1_000)
        .map(|i| {
            let t = top;
            top += mixed_extent(i);
            t
        })
        .collect();
    // On a row's top, mid-row, and past the end (clamped to the last offset).
    for y in [tops[300], tops[300] + 10.0, tops[700] + 0.25, 1e9] {
        h.jump(y);
        let offset = h.controller.offset();
        assert_eq!(
            h.last(),
            Some(expected_range((0..1_000).map(mixed_extent), offset)),
            "offset {offset}"
        );
    }
}

#[test]
fn a_jump_in_exact_mode_reports_the_landed_rows() {
    let mut h = Harness::new(100_000, Mode::Exact);
    let top: f64 = (0..73_421).map(mixed_extent).sum();
    h.jump(top);
    let (first, last) = h.last().expect("delivered");
    assert_eq!(first, 73_421, "the jumped-to row leads the range");
    assert_eq!(
        (first, last),
        expected_range((0..100_000).map(mixed_extent), top)
    );
}

#[test]
fn zero_px_rows_on_the_top_and_bottom_edges_are_not_visible() {
    let mut h = Harness::new(200, Mode::ExactZeroRuns);
    let expected = |top| expected_range((0..200).map(zero_run_extent), top);

    // The top edge on the first run (rows 20..23 at 800): row 23, the 40 px
    // row sharing that top, leads; the bottom edge (1440) is row 39's top.
    h.jump(800.0);
    assert_eq!(h.last(), Some((23, 38)));
    assert_eq!(h.last(), Some(expected(800.0)));

    // The bottom edge on the second run (rows 40..43 at 1480): row 39, ending
    // on that edge, closes the range; row 23 ends on the top edge (840).
    h.jump(840.0);
    assert_eq!(h.last(), Some((24, 39)));
    assert_eq!(h.last(), Some(expected(840.0)));

    // A run strictly inside the viewport is visible like any row.
    h.jump(790.0);
    assert_eq!(h.last(), Some(expected(790.0)));
}

#[test]
fn an_exact_list_shrinking_reports_the_rows_it_lands_on() {
    let mut h = Harness::new(1_000, Mode::Exact);
    h.jump(1e9);
    assert_eq!(h.last().map(|(_, last)| last), Some(999));

    h.count = 500;
    h.settle();
    let total: f64 = (0..500).map(mixed_extent).sum();
    let offset = h.controller.offset();
    assert_eq!(offset, total - WINDOW.height, "clamped to the new end");
    assert_eq!(
        h.last(),
        Some(expected_range((0..500).map(mixed_extent), offset))
    );
    assert_eq!(h.last().map(|(_, last)| last), Some(499));
}

#[test]
fn turning_exact_extents_off_falls_back_to_the_uniform_range() {
    let mut h = Harness::new(1_000, Mode::Exact);
    let top: f64 = (0..300).map(mixed_extent).sum();
    h.jump(top);

    h.mode = Mode::Uniform;
    h.settle();
    let offset = h.controller.offset();
    assert_eq!(
        h.last(),
        Some(expected_range(std::iter::repeat_n(ROW, 1_000), offset))
    );

    // And back again, on the prefix sum.
    h.mode = Mode::Exact;
    h.settle();
    let offset = h.controller.offset();
    assert_eq!(
        h.last(),
        Some(expected_range((0..1_000).map(mixed_extent), offset))
    );
}
