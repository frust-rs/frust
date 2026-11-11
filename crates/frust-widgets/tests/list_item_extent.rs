//! `ListView::item_extent_of` — exact, caller-declared row extents with a
//! cached prefix sum (`list_view.rs`'s module docs, *Exact extents*).
//!
//! Every list here is built inside a `Component::build`, as app code builds
//! one, and observed from outside the way an app observes it: through the
//! attached `ScrollController` (offset, max offset) and through probe rows that
//! record where they painted and how tall they were laid out.
//!
//! Row extents are multiples of a quarter pixel, so every prefix sum is exact
//! in `f64` and the assertions compare positions with `==`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, Component, ComponentView, FrameTime, LayoutCtx,
    PaintCtx, PaintScene, RenderRoot, View, Widget, component,
};
use frust_widgets::scroll_controller::ItemAlignment;
use frust_widgets::{ChildKey, ListView, ScrollController};
use kurbo::{Point, Size};

/// The window every test lays the list out in.
const WINDOW: Size = Size::new(320.0, 640.0);

/// The constructors' required `item_extent`, deliberately unlike every declared
/// extent: exact mode overrides it, so a row laid out at this height would show
/// the override failing.
const IGNORED_ITEM_EXTENT: f64 = 1_000.0;

/// The extent a probe row asks for when unconstrained. Exact mode lays rows
/// out under a tight constraint, so no probe should ever end up this tall.
const PROBE_NATURAL_HEIGHT: f64 = 10.0;

/// A deterministic mix of row extents (quarter-pixel multiples), standing in
/// for a feed of text rows, media cards and dividers.
fn mixed_extent(id: u64) -> f64 {
    const KINDS: [f64; 7] = [24.0, 56.25, 88.5, 132.0, 40.75, 300.0, 1.25];
    KINDS[(id.wrapping_mul(2_654_435_761) >> 7) as usize % KINDS.len()]
}

/// The content-space top of each row and the total, summed exactly as the
/// widget sums them.
fn prefix_of(extents: &[f64]) -> Vec<f64> {
    let mut prefix = Vec::with_capacity(extents.len() + 1);
    let mut y = 0.0;
    prefix.push(y);
    for extent in extents {
        y += extent;
        prefix.push(y);
    }
    prefix
}

// --- Probe rows -------------------------------------------------------------

/// Where each row (by id) last painted: its window-space top and its laid-out
/// height.
#[derive(Default)]
struct PaintLog {
    rows: RefCell<HashMap<u64, (f64, f64)>>,
}

impl PaintLog {
    fn clear(&self) {
        self.rows.borrow_mut().clear();
    }

    fn row(&self, id: u64) -> Option<(f64, f64)> {
        self.rows.borrow().get(&id).copied()
    }
}

struct ProbeView {
    id: u64,
    log: Rc<PaintLog>,
}

struct ProbeWidget {
    id: u64,
    log: Rc<PaintLog>,
}

impl View<()> for ProbeView {
    type Element = ProbeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
        ProbeWidget {
            id: self.id,
            log: Rc::clone(&self.log),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut ProbeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if element.id != self.id {
            element.id = self.id;
            return ChangeFlags::PAINT;
        }
        ChangeFlags::NONE
    }
}

impl Widget for ProbeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(bc.max().width, PROBE_NATURAL_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.log
            .rows
            .borrow_mut()
            .insert(self.id, (ctx.origin().y, ctx.size().height));
    }
}

struct NullScene;

impl PaintScene for NullScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
}

// --- The screen under test ---------------------------------------------------

/// The data behind a feed: one stable id and one declared extent per row, and
/// the revision of those extents the app hands the list.
///
/// The rows live behind `Rc`s and are edited copy-on-write (`Rc::make_mut`), so
/// each frame's view keeps the snapshot it was built over — the purity the
/// `key_of` and `extent_of` contracts ask for, and what lets the previous
/// frame's view still name the previous frame's keys.
struct FeedData {
    ids: Rc<Vec<u64>>,
    extents: Rc<Vec<f64>>,
    generation: u64,
}

impl FeedData {
    fn mixed(count: u64) -> Self {
        let ids: Vec<u64> = (0..count).collect();
        let extents = ids.iter().map(|&id| mixed_extent(id)).collect();
        Self {
            ids: Rc::new(ids),
            extents: Rc::new(extents),
            generation: 0,
        }
    }

    fn ids_mut(&mut self) -> &mut Vec<u64> {
        Rc::make_mut(&mut self.ids)
    }

    fn extents_mut(&mut self) -> &mut Vec<f64> {
        Rc::make_mut(&mut self.extents)
    }
}

/// How the feed declares its extents. Only the misuse cases combine modes.
#[derive(Clone, Copy)]
enum Extents {
    /// No `item_extent_of`: every row is the constructor's uniform extent.
    Uniform,
    Exact,
    ExactThenEstimate,
    EstimateThenExact,
}

struct Feed {
    data: Rc<RefCell<FeedData>>,
    controller: ScrollController,
    log: Rc<PaintLog>,
    keyed: bool,
    extents: Extents,
}

impl Component for Feed {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        let data = self.data.borrow();
        let list = Self::rows(Rc::clone(&data.ids), &self.log, self.keyed);
        let extent_of = Self::extent_of(Rc::clone(&data.extents));
        let list = match self.extents {
            Extents::Uniform => list,
            Extents::Exact => list.item_extent_of(extent_of),
            Extents::ExactThenEstimate => {
                list.item_extent_of(extent_of).estimated_item_extent(48.0)
            }
            Extents::EstimateThenExact => {
                list.estimated_item_extent(48.0).item_extent_of(extent_of)
            }
        };
        list.extent_generation(data.generation)
            .controller(self.controller.clone())
    }
}

impl Feed {
    fn rows(ids: Rc<Vec<u64>>, log: &Rc<PaintLog>, keyed: bool) -> ListView<()> {
        let count = ids.len();
        let row_ids = Rc::clone(&ids);
        let log = Rc::clone(log);
        let row = move |i: usize| ProbeView {
            id: row_ids[i],
            log: Rc::clone(&log),
        };
        if keyed {
            let key_of = move |i: usize| ChildKey::new(ids[i]);
            ListView::builder_keyed(count, IGNORED_ITEM_EXTENT, key_of, row)
        } else {
            ListView::builder(count, IGNORED_ITEM_EXTENT, row)
        }
    }

    fn extent_of(extents: Rc<Vec<f64>>) -> impl Fn(usize) -> f64 + 'static {
        move |i| extents[i]
    }
}

/// A rendered [`Feed`] plus the handles a test drives and reads it through.
struct Harness {
    root: RenderRoot<(), ComponentView<Feed>>,
    data: Rc<RefCell<FeedData>>,
    controller: ScrollController,
    log: Rc<PaintLog>,
    keyed: bool,
    extents: Extents,
    ms: f64,
}

impl Harness {
    fn new(data: FeedData, keyed: bool, extents: Extents) -> Self {
        let mut harness = Self {
            root: RenderRoot::new(),
            data: Rc::new(RefCell::new(data)),
            controller: ScrollController::new(),
            log: Rc::new(PaintLog::default()),
            keyed,
            extents,
            ms: 0.0,
        };
        // The first frame plans against a zero viewport, the second converges
        // on the real one (the module docs' *Viewport staleness*).
        harness.frame();
        harness.frame();
        harness
    }

    fn frame(&mut self) {
        let data = Rc::clone(&self.data);
        let controller = self.controller.clone();
        let log = Rc::clone(&self.log);
        let (keyed, extents) = (self.keyed, self.extents);
        let mut logic = move |_: &mut ()| {
            component(Feed {
                data: Rc::clone(&data),
                controller: controller.clone(),
                log: Rc::clone(&log),
                keyed,
                extents,
            })
        };
        self.log.clear();
        self.root.rebuild(&mut logic, &mut ());
        self.root.layout(WINDOW);
        self.root.paint(
            &mut NullScene,
            FrameTime::from_nanos((self.ms * 1e6) as u64),
        );
        self.ms += 16.0;
    }

    /// Run frames until the list has drained a controller command and
    /// converged on its window (two are always enough; three is margin).
    fn settle(&mut self) {
        for _ in 0..3 {
            self.frame();
        }
    }

    fn prefix(&self) -> Vec<f64> {
        prefix_of(&self.data.borrow().extents)
    }

    fn content_extent(&self) -> f64 {
        self.controller.max_offset() + WINDOW.height
    }
}

// --- Tests -------------------------------------------------------------------

#[test]
fn jump_to_lands_on_the_exact_row_among_100k_mixed_rows() {
    const COUNT: u64 = 100_000;
    let mut feed = Harness::new(FeedData::mixed(COUNT), false, Extents::Exact);
    let prefix = feed.prefix();

    for target in [73_421_u64, 99_000, 12, 50_000] {
        let top = prefix[target as usize];
        feed.controller.jump_to(top);
        feed.settle();
        assert_eq!(feed.controller.offset(), top, "row {target}'s top, exactly");
        let (y, height) = feed.log.row(target).expect("the jumped-to row painted");
        assert_eq!(y, 0.0, "row {target} sits at the viewport's top edge");
        assert_eq!(
            height,
            mixed_extent(target),
            "row {target} is laid out at its declared extent, not its natural \
             {PROBE_NATURAL_HEIGHT} px nor the constructor's {IGNORED_ITEM_EXTENT}"
        );
        // Its neighbours sit exactly where the prefix sum puts them.
        let next = target + 1;
        if let Some((next_y, _)) = feed.log.row(next) {
            assert_eq!(next_y, prefix[next as usize] - top);
        }
        if target > 0
            && let Some((prev_y, prev_height)) = feed.log.row(target - 1)
        {
            assert_eq!(prev_y + prev_height, 0.0);
        }
    }

    // A jump into the middle of a row: that row straddles the top edge by
    // exactly the distance into it.
    let target = 31_337_u64;
    let into = mixed_extent(target) / 2.0;
    feed.controller.jump_to(prefix[target as usize] + into);
    feed.settle();
    let (y, _) = feed.log.row(target).expect("the straddling row painted");
    assert_eq!(y, -into);
}

#[test]
fn content_extent_equals_the_sum_of_declared_extents() {
    for keyed in [false, true] {
        let feed = Harness::new(FeedData::mixed(100_000), keyed, Extents::Exact);
        let total = *feed.prefix().last().expect("non-empty prefix");
        assert_eq!(
            feed.content_extent(),
            total,
            "keyed: {keyed} — exact from the first frames, no row visited"
        );
    }

    // The end of the list: max_offset puts the last row flush with the bottom.
    let mut feed = Harness::new(FeedData::mixed(1_000), false, Extents::Exact);
    let prefix = feed.prefix();
    feed.controller.jump_to(1.0e12);
    feed.settle();
    assert_eq!(feed.controller.offset(), prefix[1_000] - WINDOW.height);
    let (y, height) = feed.log.row(999).expect("the last row painted");
    assert_eq!(y + height, WINDOW.height);
}

#[test]
fn a_generation_bump_reflects_new_extents() {
    let mut feed = Harness::new(FeedData::mixed(5_000), false, Extents::Exact);
    let before = feed.content_extent();
    feed.controller.jump_to(feed.prefix()[100]);
    feed.settle();

    // Every row doubles. The callback alone cannot say so: with
    // the generation unchanged the list keeps the extents it summed.
    {
        let mut data = feed.data.borrow_mut();
        for extent in data.extents_mut() {
            *extent *= 2.0;
        }
    }
    feed.settle();
    assert_eq!(feed.content_extent(), before, "no bump, no new prefix sum");

    // Bumping the generation re-reads every extent.
    feed.data.borrow_mut().generation += 1;
    feed.settle();
    let prefix = feed.prefix();
    assert_eq!(feed.content_extent(), prefix[5_000]);
    assert_eq!(feed.content_extent(), before * 2.0);

    // Rows are placed and laid out against the new extents.
    feed.controller.jump_to(prefix[2_000]);
    feed.settle();
    let (y, height) = feed.log.row(2_000).expect("row 2000 painted");
    assert_eq!((y, height), (0.0, mixed_extent(2_000) * 2.0));
    if let Some((next_y, _)) = feed.log.row(2_001) {
        assert_eq!(next_y, height);
    }

    // An item-count change rebuilds the sum without a bump.
    {
        let mut data = feed.data.borrow_mut();
        data.ids_mut().push(9_999_999);
        data.extents_mut().push(77.0);
    }
    feed.settle();
    assert_eq!(feed.content_extent(), feed.prefix()[5_001]);
}

#[test]
fn a_prepend_keeps_the_anchor_row_in_place() {
    let mut feed = Harness::new(FeedData::mixed(2_000), true, Extents::Exact);
    let anchor_id = 500_u64;
    let top = feed.prefix()[anchor_id as usize] + 7.5;
    feed.controller.jump_to(top);
    feed.settle();
    let (anchor_y, _) = feed.log.row(anchor_id).expect("the anchor row painted");
    assert_eq!(anchor_y, -7.5);

    // "Load older": seven rows of mixed, very different extents land above.
    let prepended: Vec<f64> = vec![300.0, 1.25, 88.5, 24.0, 132.0, 56.25, 640.0];
    {
        let mut data = feed.data.borrow_mut();
        for (n, extent) in prepended.iter().enumerate().rev() {
            data.ids_mut().insert(0, 1_000_000 + n as u64);
            data.extents_mut().insert(0, *extent);
        }
    }
    feed.settle();

    let shifted: f64 = prepended.iter().sum();
    assert_eq!(
        feed.controller.offset(),
        top + shifted,
        "the offset absorbs exactly the prepended rows' summed extents"
    );
    let (y, height) = feed
        .log
        .row(anchor_id)
        .expect("the anchor row still painted");
    assert_eq!(y, anchor_y, "the anchor row has not moved on screen");
    assert_eq!(height, mixed_extent(anchor_id));

    // And a removal above the viewport shifts back by the removed extents.
    {
        let mut data = feed.data.borrow_mut();
        data.ids_mut().drain(0..3);
        data.extents_mut().drain(0..3);
    }
    feed.settle();
    let removed: f64 = prepended[..3].iter().sum();
    assert_eq!(feed.controller.offset(), top + shifted - removed);
    let (y, _) = feed
        .log
        .row(anchor_id)
        .expect("the anchor row still painted");
    assert_eq!(y, anchor_y);
}

/// Grow row `id`'s declared extent by `by` px under an unchanged item count,
/// bumping the generation as the contract asks.
fn reflow(feed: &Harness, id: usize, by: f64) {
    let mut data = feed.data.borrow_mut();
    data.extents_mut()[id] += by;
    data.generation += 1;
}

#[test]
fn a_reflow_above_the_viewport_keeps_the_anchor_row_in_place() {
    let mut feed = Harness::new(FeedData::mixed(2_000), true, Extents::Exact);
    let anchor_id = 500_u64;
    let top = feed.prefix()[anchor_id as usize] + 7.5;
    feed.controller.jump_to(top);
    feed.settle();
    let (anchor_y, _) = feed.log.row(anchor_id).expect("the anchor row painted");
    assert_eq!(anchor_y, -7.5);

    // A row far above the materialized window grows: same count, new
    // generation. The offset absorbs the growth.
    reflow(&feed, 100, 200.0);
    feed.settle();
    assert_eq!(feed.controller.offset(), top + 200.0);
    let (y, _) = feed.log.row(anchor_id).expect("the anchor row painted");
    assert_eq!(y, anchor_y, "a re-flow far above leaves the anchor row put");

    // The row just above the viewport, inside the window's leading buffer,
    // grows: the row the top edge sits in still holds its place.
    reflow(&feed, 499, 33.25);
    feed.settle();
    assert_eq!(feed.controller.offset(), top + 200.0 + 33.25);
    let (y, _) = feed.log.row(anchor_id).expect("the anchor row painted");
    assert_eq!(y, anchor_y, "a re-flow in the leading buffer leaves it put");

    // A re-flow below the viewport changes nothing on screen.
    let offset = feed.controller.offset();
    reflow(&feed, 1_500, 120.0);
    feed.settle();
    assert_eq!(feed.controller.offset(), offset);
    let (y, _) = feed.log.row(anchor_id).expect("the anchor row painted");
    assert_eq!(y, anchor_y);
}

#[test]
fn entering_exact_mode_on_a_prepend_frame_skips_the_anchor_shift() {
    let mut feed = Harness::new(FeedData::mixed(100), true, Extents::Uniform);
    let top = 5.0 * IGNORED_ITEM_EXTENT;
    feed.controller.jump_to(top);
    feed.settle();
    assert_eq!(feed.controller.offset(), top);

    // One frame prepends three rows and switches the list to exact extents:
    // there is no previous prefix to read the anchor row's old top off, so
    // the offset is left where it was rather than shifted by a guess.
    {
        let mut data = feed.data.borrow_mut();
        for n in 0..3 {
            data.ids_mut().insert(0, 1_000_000 + n);
            data.extents_mut().insert(0, 24.0);
        }
    }
    feed.extents = Extents::Exact;
    feed.settle();
    let max_offset = feed.prefix()[103] - WINDOW.height;
    assert_eq!(feed.controller.offset(), top.min(max_offset));
    assert_eq!(feed.content_extent(), feed.prefix()[103]);
}

/// Scroll a 100-row feed to its end, then cut it to its first 50 rows: the
/// materialized window (rows 9x) now lies wholly past the new item count.
fn feed_at_the_end_then_halved(keyed: bool) -> Harness {
    let mut feed = Harness::new(FeedData::mixed(100), keyed, Extents::Exact);
    feed.controller.jump_to(1.0e12);
    feed.settle();
    assert!(feed.log.row(99).is_some(), "the last row is on screen");
    let mut data = feed.data.borrow_mut();
    data.ids_mut().truncate(50);
    data.extents_mut().truncate(50);
    drop(data);
    feed
}

#[test]
fn a_shrink_past_the_window_with_a_queued_jump_lands_on_valid_rows() {
    let mut feed = feed_at_the_end_then_halved(false);
    // The jump drains in the same rebuild that sees the shrink, before the
    // window is reconciled to the new count.
    feed.controller.jump_to(0.0);
    feed.settle();
    assert_eq!(feed.controller.offset(), 0.0);
    assert_eq!(feed.content_extent(), feed.prefix()[50]);
    let (y, height) = feed.log.row(0).expect("row 0 painted");
    assert_eq!((y, height), (0.0, mixed_extent(0)));
    assert!(
        feed.log.row(60).is_none(),
        "no row past the new count paints"
    );
}

#[test]
fn a_shrink_past_the_window_with_a_queued_scroll_to_item_lands_on_the_row() {
    let mut feed = feed_at_the_end_then_halved(true);
    feed.controller
        .scroll_to_item(ChildKey::new(10_u64), ItemAlignment::Start, false);
    feed.settle();
    let prefix = feed.prefix();
    assert_eq!(feed.controller.offset(), prefix[10]);
    let (y, height) = feed.log.row(10).expect("row 10 painted");
    assert_eq!((y, height), (0.0, mixed_extent(10)));
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "mutually exclusive")]
fn exact_then_estimated_extents_debug_panic() {
    Harness::new(FeedData::mixed(10), true, Extents::ExactThenEstimate);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "mutually exclusive")]
fn estimated_then_exact_extents_debug_panic() {
    Harness::new(FeedData::mixed(10), true, Extents::EstimateThenExact);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "negative or non-finite")]
fn a_non_finite_extent_debug_panics() {
    let mut data = FeedData::mixed(10);
    data.extents_mut()[4] = f64::NAN;
    Harness::new(data, false, Extents::Exact);
}

#[test]
#[cfg(not(debug_assertions))]
fn a_negative_or_non_finite_extent_counts_as_zero_in_release() {
    let mut data = FeedData::mixed(1_000);
    data.extents_mut()[4] = f64::NAN;
    data.extents_mut()[6] = -40.0;
    let feed = Harness::new(data, false, Extents::Exact);
    let mut sane = feed.data.borrow().extents.to_vec();
    sane[4] = 0.0;
    sane[6] = 0.0;
    let total = *prefix_of(&sane).last().expect("non-empty prefix");
    assert_eq!(feed.content_extent(), total);
}
