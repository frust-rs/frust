//! `NavOp::SetPages`: reconciling the stack against a keyed page list, the
//! flight it chooses, and what it does to removed and retained pages.

use super::super::*;
use super::support::*;
use crate::column;
use crate::nav::transition::{PageTransition, Timing, TransitionSpec};
use crate::test_support::RecordingScene;
use frust_core::{Curve, FrameTime, RenderRoot, any};
use kurbo::Rect;
use std::cell::Cell;
use std::time::Duration;

type Log = Rc<RefCell<Vec<String>>>;

/// A page leaf that logs its build, teardown and every event it receives, and
/// fills a box of its own height so a paint assertion can tell pages apart.
struct Probe {
    name: &'static str,
    height: f64,
    log: Log,
}
struct ProbeWidget {
    name: &'static str,
    height: f64,
    log: Log,
}
impl View<()> for Probe {
    type Element = ProbeWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
        self.log.borrow_mut().push(format!("build {}", self.name));
        ProbeWidget {
            name: self.name,
            height: self.height,
            log: Rc::clone(&self.log),
        }
    }
    fn rebuild(&self, _: &Self, element: &mut ProbeWidget, _: &mut BuildCtx<'_>) -> ChangeFlags {
        element.height = self.height;
        ChangeFlags::NONE
    }
    fn teardown(&self, _element: &mut ProbeWidget, _ctx: &mut BuildCtx<'_>) {
        self.log
            .borrow_mut()
            .push(format!("teardown {}", self.name));
    }
}
impl Widget for ProbeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, self.height))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
    }
    fn event(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
        self.log.borrow_mut().push(format!("event {}", self.name));
        EventResult::Ignored
    }
}

fn probe(name: &'static str, height: f64, log: &Log) -> PageBuilder<()> {
    let log = Rc::clone(log);
    Rc::new(move || {
        any(Probe {
            name,
            height,
            log: Rc::clone(&log),
        })
    })
}

fn counter(observed: &Rc<Cell<u32>>) -> PageBuilder<()> {
    let observed = Rc::clone(observed);
    Rc::new(move || counter_page(&observed))
}

/// A page hosting a nested navigator driven by `nested`.
fn nested_page(nested: &NavigatorController<()>, root: PageBuilder<()>) -> PageBuilder<()> {
    let nested = nested.clone();
    Rc::new(move || {
        let root = Rc::clone(&root);
        any(navigator(&nested, move || root()))
    })
}

/// A page spec keyed and routed by `path`, opaque, on the navigator's default
/// transition.
fn page(path: &str, builder: PageBuilder<()>) -> PageSpec<()> {
    PageSpec {
        key: Some(path_key(path)),
        builder,
        opaque: true,
        transition: None,
        route: Some(route(path)),
    }
}

fn path_key(path: &str) -> PageKey {
    PageKey::Path(path.to_string())
}

fn flying(spec: PageSpec<()>) -> PageSpec<()> {
    PageSpec {
        transition: Some(slide()),
        ..spec
    }
}

fn transparent(spec: PageSpec<()>) -> PageSpec<()> {
    PageSpec {
        opaque: false,
        ..spec
    }
}

/// Linear over 100ms, with both layers at full alpha: on a push the entering page
/// sits at `(1 - p) * width`, on a pop the leaving page at `p * width`.
fn slide() -> TransitionSpec {
    TransitionSpec::new(
        PageTransition::IosPush,
        Timing::Duration(Duration::from_millis(100), Curve::Linear),
    )
}

/// A navigator over an unkeyed root page 100 tall, in a 100x100 window.
struct Harness {
    controller: NavigatorController<()>,
    root: RenderRoot<(), NavigatorView<()>>,
    app: AppLogic,
}

impl Harness {
    fn new() -> Self {
        let controller: NavigatorController<()> = NavigatorController::new();
        let app: AppLogic = {
            let ctrl = controller.clone();
            Box::new(move |_: &mut ()| {
                navigator(&ctrl, || sized_page(100.0, 100.0))
                    .pop_swipe(true)
                    .root_route(route("/root"))
            })
        };
        let mut harness = Self {
            controller,
            root: RenderRoot::new(),
            app,
        };
        harness.frame(0);
        harness
    }

    fn frame(&mut self, ms: u64) -> Vec<(Point, Size, f32)> {
        full_frame(&mut self.root, &mut self.app, &mut (), ft(ms)).0
    }

    /// Queue `pages` and run one frame: the op's own frame.
    fn set(&mut self, pages: Vec<PageSpec<()>>, ms: u64) -> Vec<(Point, Size, f32)> {
        self.controller.set_pages(pages);
        self.frame(ms)
    }

    fn settle(&mut self, ms: u64) -> Vec<(Point, Size, f32)> {
        run_until_settled(&mut self.root, &mut self.app, &mut (), ms)
    }

    fn nav(&self) -> &NavigatorWidget<()> {
        nav_widget(&self.root)
    }

    /// The published route stack's paths, bottom to top.
    fn routes(&self) -> Vec<String> {
        self.controller
            .route_stack()
            .entries()
            .iter()
            .map(|entry| entry.as_ref().map_or("-".into(), |l| l.path.clone()))
            .collect()
    }

    fn tap(&mut self) {
        self.root.event(&mut (), &down(50.0, 50.0));
        self.root.event(&mut (), &up(50.0, 50.0));
    }
}

fn count(log: &Log, entry: &str) -> usize {
    log.borrow().iter().filter(|e| *e == entry).count()
}

/// How many pods the navigator reports to a tree walker.
fn visited_pods(nav: &NavigatorWidget<()>) -> usize {
    let mut pods = 0;
    nav.visit_children(&mut |_| pods += 1);
    pods
}

// --- The stack commits on the op's frame, however many pages it drops. ---

#[test]
fn a_multi_page_drop_commits_on_the_ops_frame_even_mid_flight() {
    let log = Log::default();
    let nested: NavigatorController<()> = NavigatorController::new();
    let mut h = Harness::new();
    h.set(
        vec![
            page("/a", probe("a", 10.0, &log)),
            page("/b", nested_page(&nested, probe("b", 20.0, &log))),
            page("/c", probe("c", 30.0, &log)),
        ],
        0,
    );
    assert_eq!(h.routes(), ["/a", "/b", "/c"]);
    assert!(h.controller.back_interest());
    assert!(nested.is_mounted());

    h.set(vec![flying(page("/x", probe("x", 40.0, &log)))], 1000);

    assert!(h.nav().transition.is_some(), "an animated drop flies");
    assert_eq!(h.controller.depth(), 1);
    assert_eq!(h.routes(), ["/x"]);
    assert!(!h.controller.back_interest());
    let state = h.controller.transition();
    assert_eq!(
        (state.from_depth, state.to_depth),
        (3, 1),
        "from_depth is the true pre-op depth"
    );
    assert!(!state.is_pop);

    h.settle(1016);
    assert!(h.nav().transition.is_none());
    assert_eq!(h.controller.depth(), 1);
    for name in ["teardown a", "teardown b", "teardown c"] {
        assert_eq!(count(&log, name), 1, "{name}");
    }
    assert!(!nested.is_mounted(), "the removed page's navigator is gone");
    assert!(!nested.back_interest());
}

#[test]
fn an_instant_multi_page_drop_tears_down_on_the_ops_frame() {
    let log = Log::default();
    let nested: NavigatorController<()> = NavigatorController::new();
    let mut h = Harness::new();
    h.set(
        vec![
            page("/a", probe("a", 10.0, &log)),
            page("/b", nested_page(&nested, probe("b", 20.0, &log))),
            page("/c", probe("c", 30.0, &log)),
        ],
        0,
    );

    let fills = h.set(vec![page("/x", probe("x", 40.0, &log))], 16);

    assert!(h.nav().transition.is_none());
    assert_eq!(h.routes(), ["/x"]);
    assert_eq!(count(&log, "teardown a"), 1);
    assert_eq!(count(&log, "teardown c"), 1);
    assert!(!nested.is_mounted());
    assert_eq!(fills.len(), 1, "only the new page paints");
    assert_eq!(fill_h(&fills, 40.0).0, Point::ZERO);
}

// --- Retention. ---

#[test]
fn a_matched_page_keeps_its_state_and_takes_the_new_builder_and_route() {
    let first = Rc::new(Cell::new(0));
    let second = Rc::new(Cell::new(0));
    let mut h = Harness::new();
    h.set(vec![page("/k", counter(&first))], 0);
    h.tap();
    h.frame(16);
    assert_eq!(first.get(), 1, "the tap reached the page");

    let new_spec = PageSpec {
        route: Some(route("/k?tab=2")),
        transition: Some(slide()),
        ..page("/k", counter(&second))
    };
    h.set(vec![new_spec], 32);

    assert_eq!(second.get(), 1, "the new builder ran over the kept count");
    assert_eq!(h.nav().pages[0].route, Some(route("/k?tab=2")));
    assert_eq!(h.nav().pages[0].transition.preset, PageTransition::IosPush);
    assert!(h.nav().transition.is_none(), "an unchanged top never flies");
}

#[test]
fn a_nested_navigator_on_a_retained_page_is_not_rebuilt() {
    let log = Log::default();
    let nested: NavigatorController<()> = NavigatorController::new();
    let mut h = Harness::new();
    let host = nested_page(&nested, probe("nested-root", 10.0, &log));
    h.set(vec![page("/k", Rc::clone(&host))], 0);
    nested.push(|| sized_page(100.0, 20.0));
    h.frame(16);
    assert_eq!(nested.depth(), 2);

    h.set(vec![page("/k", host)], 32);

    assert_eq!(nested.depth(), 2, "its stack survived");
    assert_eq!(count(&log, "build nested-root"), 1);
    assert_eq!(count(&log, "teardown nested-root"), 0);
    assert_eq!(nested.mounted.get(), 1);
}

#[test]
fn a_page_keyed_by_push_or_replace_is_retained_and_an_unkeyed_one_is_not() {
    let log = Log::default();
    let mut h = Harness::new();

    let pushed = probe("pushed", 10.0, &log);
    h.controller.push_with_options(
        move || pushed(),
        PushOptions::opaque().key(path_key("/pushed")),
    );
    h.frame(0);
    h.set(vec![page("/pushed", probe("pushed", 10.0, &log))], 16);
    assert_eq!(
        count(&log, "build pushed"),
        1,
        "the keyed push was retained"
    );
    assert_eq!(h.nav().pages.len(), 1);

    let replaced = probe("replaced", 20.0, &log);
    h.controller.replace_with_options(
        move || replaced(),
        ReplaceOptions::opaque().key(path_key("/replaced")),
    );
    h.frame(32);
    h.set(vec![page("/replaced", probe("replaced", 20.0, &log))], 48);
    assert_eq!(count(&log, "build replaced"), 1, "so was the keyed replace");

    let bare = probe("bare", 30.0, &log);
    h.controller.push(move || bare());
    h.frame(64);
    let unkeyed = PageSpec {
        key: None,
        ..page("/bare", probe("bare", 30.0, &log))
    };
    h.set(vec![unkeyed], 80);
    assert_eq!(count(&log, "build bare"), 2, "no key never matches");
    assert_eq!(count(&log, "teardown bare"), 1);
}

#[test]
fn a_key_repeated_in_the_list_builds_the_later_page_fresh() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(vec![page("/k", probe("first", 10.0, &log))], 0);

    h.set(
        vec![
            page("/k", probe("first", 10.0, &log)),
            page("/k", probe("second", 20.0, &log)),
        ],
        16,
    );

    assert_eq!(h.nav().pages.len(), 2);
    assert_eq!(
        count(&log, "build first"),
        1,
        "the first occurrence matched"
    );
    assert_eq!(count(&log, "teardown first"), 0);
    assert_eq!(count(&log, "build second"), 1, "the later one built fresh");
}

#[test]
fn new_pages_build_before_anything_is_torn_down() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(vec![page("/old", probe("old", 10.0, &log))], 0);

    h.set(vec![page("/new", probe("new", 20.0, &log))], 16);

    let log = log.borrow();
    let built = log.iter().position(|e| e == "build new").unwrap();
    let torn = log.iter().position(|e| e == "teardown old").unwrap();
    assert!(built < torn, "{log:?}");
}

#[test]
fn a_controller_shared_by_an_old_and_a_new_page_stays_mounted() {
    let log = Log::default();
    let shared: NavigatorController<()> = NavigatorController::new();
    let mut h = Harness::new();
    h.set(
        vec![page("/a", nested_page(&shared, probe("a", 10.0, &log)))],
        0,
    );
    assert_eq!(shared.mounted.get(), 1);

    let mounted_at_new_build = Rc::new(Cell::new(0));
    let new_page: PageBuilder<()> = {
        let shared = shared.clone();
        let mounted_at_new_build = Rc::clone(&mounted_at_new_build);
        let host = nested_page(&shared, probe("b", 20.0, &log));
        Rc::new(move || {
            if mounted_at_new_build.get() == 0 {
                mounted_at_new_build.set(shared.mounted.get());
            }
            host()
        })
    };
    h.set(vec![page("/b", new_page)], 16);

    assert_eq!(mounted_at_new_build.get(), 1, "the old page is still up");
    assert_eq!(count(&log, "teardown a"), 1);
    assert_eq!(shared.mounted.get(), 1, "one view left, never zero between");
    assert!(shared.is_mounted());
}

#[test]
fn only_the_new_top_is_built_reachable() {
    let reach_at_build: Rc<RefCell<Vec<(&'static str, bool)>>> = Rc::default();
    let builder = |name: &'static str| -> PageBuilder<()> {
        let seen = Rc::clone(&reach_at_build);
        Rc::new(move || {
            let reach = ambient_page_reach().expect("a page installs its reach");
            seen.borrow_mut().push((name, reach.get()));
            any(sized_page::<()>(100.0, 100.0))
        })
    };
    let mut h = Harness::new();

    h.controller.set_pages(vec![
        page("/under", builder("under")),
        page("/top", builder("top")),
    ]);
    h.root.rebuild(&mut h.app, &mut ());

    let seen = reach_at_build.borrow();
    assert_eq!(seen[0], ("under", false));
    assert_eq!(seen[1], ("top", true));
    assert!(!h.nav().pages[0].reach.get());
    assert!(h.nav().pages[1].reach.get());
}

// --- No flight. ---

#[test]
fn removal_below_a_retained_top_starts_no_flight() {
    let log = Log::default();
    let observed = Rc::new(Cell::new(0));
    let mut h = Harness::new();
    h.set(vec![page("/s", counter(&observed))], 0);
    h.tap();
    h.frame(16);
    h.set(
        vec![
            flying(page("/u", probe("u", 10.0, &log))),
            flying(page("/s", counter(&observed))),
        ],
        32,
    );
    assert!(h.nav().transition.is_none());

    h.set(vec![flying(page("/s", counter(&observed)))], 48);

    assert!(h.nav().transition.is_none(), "the top did not change");
    assert_eq!(h.controller.depth(), 1);
    assert_eq!(count(&log, "teardown u"), 1, "torn down on the op's frame");
    assert_eq!(observed.get(), 1, "S kept its state");
}

#[test]
fn an_unchanged_top_starts_no_flight() {
    let log = Log::default();
    let mut h = Harness::new();
    let list = |log: &Log| {
        vec![
            flying(page("/a", probe("a", 10.0, log))),
            flying(page("/b", probe("b", 20.0, log))),
        ]
    };
    h.set(list(&log), 0);
    h.settle(16);
    let generation = h.controller.transition().generation;

    h.set(list(&log), 1000);

    assert!(h.nav().transition.is_none());
    assert!(!h.controller.transition().active);
    assert_eq!(h.controller.transition().generation, generation);
    assert_eq!(count(&log, "teardown a"), 0);
    assert_eq!(count(&log, "teardown b"), 0);
}

#[test]
fn a_new_top_with_no_animated_spec_starts_no_flight() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(vec![page("/a", probe("a", 10.0, &log))], 0);

    h.set(vec![page("/b", probe("b", 20.0, &log))], 16);

    assert!(h.nav().transition.is_none());
    assert_eq!(count(&log, "teardown a"), 1);
}

#[test]
fn reordering_retained_pages_starts_no_flight() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(
        vec![
            flying(page("/a", probe("a", 10.0, &log))),
            flying(page("/b", probe("b", 20.0, &log))),
        ],
        0,
    );
    h.settle(16);

    h.set(
        vec![
            flying(page("/b", probe("b", 20.0, &log))),
            flying(page("/a", probe("a", 10.0, &log))),
        ],
        1000,
    );

    assert!(h.nav().transition.is_none(), "the old top was not removed");
    assert_eq!(h.routes(), ["/b", "/a"]);
    assert_eq!(count(&log, "teardown a") + count(&log, "teardown b"), 0);
}

// --- The push flight. ---

#[test]
fn a_push_flight_keeps_removed_pages_until_settle_then_tears_each_down_once() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(
        vec![
            page("/a", probe("a", 10.0, &log)),
            page("/b", probe("b", 20.0, &log)),
            page("/c", probe("c", 30.0, &log)),
        ],
        0,
    );
    assert_eq!(visited_pods(h.nav()), 3);

    h.set(vec![flying(page("/x", probe("x", 40.0, &log)))], 1000);

    assert_eq!(count(&log, "teardown a"), 0, "held until settle");
    assert_eq!(count(&log, "teardown b"), 0, "held until settle");
    assert_eq!(count(&log, "teardown c"), 0, "the leaving layer");
    assert_eq!(
        visited_pods(h.nav()),
        4,
        "a tree walk reaches the new page, the leaving page and both held pages"
    );
    let flight = h.nav().held_flight.as_ref().expect("a push flight");
    assert_eq!(flight.held.len(), 2);
    assert!(flight.held.iter().all(|page| !page.reach.get()));
    assert!(
        !h.nav()
            .transition
            .as_ref()
            .unwrap()
            .stashed
            .as_ref()
            .unwrap()
            .reach
            .get()
    );

    // A held page takes no events: neither a broadcast nor anything routed.
    log.borrow_mut().clear();
    h.root.event(&mut (), &InputEvent::Housekeeping);
    assert_eq!(*log.borrow(), ["event x"]);
    log.borrow_mut().clear();

    h.settle(1016);

    assert!(h.nav().held_flight.is_none());
    assert_eq!(visited_pods(h.nav()), 1);
    let teardowns: Vec<_> = log
        .borrow()
        .iter()
        .filter(|e| e.starts_with("teardown"))
        .cloned()
        .collect();
    assert_eq!(
        teardowns,
        ["teardown c", "teardown b", "teardown a"],
        "each once, top to bottom"
    );
}

#[test]
fn a_dialog_over_a_page_stays_painted_under_the_flight() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(
        vec![
            page("/below", probe("below", 10.0, &log)),
            page("/page", probe("page", 20.0, &log)),
            transparent(page("/dialog", probe("dialog", 30.0, &log))),
        ],
        0,
    );

    let seed = h.set(vec![flying(page("/x", probe("x", 40.0, &log)))], 1000);
    let mid = h.frame(1050);

    for fills in [&seed, &mid] {
        let heights: Vec<f64> = fills.iter().map(|(_, size, _)| size.height).collect();
        assert_eq!(
            heights,
            [20.0, 30.0, 40.0],
            "backdrop, then leaving, then entering; the covered page stays culled"
        );
    }
    assert_eq!(fill_h(&mid, 20.0).0, Point::ZERO, "the backdrop is static");
    assert!(fill_h(&mid, 30.0).0.x < 0.0, "the old top leaves");
    assert_eq!(fill_h(&mid, 40.0).0.x, 50.0, "the new top enters");

    let settled = h.settle(1100);
    assert_eq!(settled.len(), 1);
    assert_eq!(count(&log, "teardown page"), 1);
    assert_eq!(count(&log, "teardown dialog"), 1);
    assert_eq!(count(&log, "teardown below"), 1);
}

#[test]
fn a_retained_page_below_a_removed_dialog_paints_as_the_backdrop() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(
        vec![
            page("/page", probe("page", 20.0, &log)),
            transparent(page("/dialog", probe("dialog", 30.0, &log))),
        ],
        0,
    );

    let new_list = vec![
        page("/page", probe("page", 20.0, &log)),
        flying(page("/x", probe("x", 40.0, &log))),
    ];
    let seed = h.set(new_list, 1000);

    let heights: Vec<f64> = seed.iter().map(|(_, size, _)| size.height).collect();
    assert_eq!(heights, [20.0, 30.0, 40.0]);
    assert_eq!(count(&log, "teardown page"), 0);

    let settled = h.settle(1016);
    assert_eq!(fill_h(&settled, 40.0).0, Point::ZERO);
    assert_eq!(count(&log, "teardown page"), 0, "retained throughout");
    assert_eq!(count(&log, "teardown dialog"), 1);
}

#[test]
fn a_retained_old_top_is_the_leaving_page_wherever_it_lands() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(
        vec![
            page("/a", probe("a", 10.0, &log)),
            page("/b", probe("b", 20.0, &log)),
        ],
        0,
    );

    h.set(
        vec![
            page("/b", probe("b", 20.0, &log)),
            page("/mid", probe("mid", 30.0, &log)),
            flying(page("/x", probe("x", 40.0, &log))),
        ],
        1000,
    );
    let mid = h.frame(1050);

    assert_eq!(h.routes(), ["/b", "/mid", "/x"]);
    let heights: Vec<f64> = mid.iter().map(|(_, size, _)| size.height).collect();
    assert_eq!(
        heights,
        [20.0, 40.0],
        "the old top leaves, not the new page below"
    );
    assert_eq!(count(&log, "teardown a"), 0, "held until settle");
    assert_eq!(count(&log, "teardown b"), 0);
    h.settle(1100);
    assert_eq!(count(&log, "teardown a"), 1);
    assert_eq!(count(&log, "teardown b"), 0);
}

// --- The pop flight. ---

#[test]
fn a_pop_flight_reverses_the_old_tops_spec_over_the_retained_page() {
    let log = Log::default();
    let observed = Rc::new(Cell::new(0));
    let mut h = Harness::new();
    h.set(vec![page("/a", counter(&observed))], 0);
    h.tap();
    h.frame(16);
    h.set(
        vec![
            page("/a", counter(&observed)),
            flying(page("/b", probe("b", 20.0, &log))),
        ],
        32,
    );
    h.settle(48);

    h.set(vec![page("/a", counter(&observed))], 1000);

    {
        let flight = h.nav().transition.as_ref().expect("a pop flight");
        assert!(flight.is_pop);
        assert_eq!(flight.preset, PageTransition::IosPush);
        assert!(flight.stashed.is_some());
    }
    assert!(h.controller.transition().is_pop);
    assert_eq!(h.routes(), ["/a"]);
    assert_eq!(count(&log, "teardown b"), 0, "animating out");

    let mid = h.frame(1050);
    assert_eq!(
        fill_h(&mid, 20.0).0.x,
        50.0,
        "the old top slides off reversed"
    );

    h.settle(1100);
    assert_eq!(count(&log, "teardown b"), 1);
    assert_eq!(observed.get(), 1, "A kept its state");
}

#[test]
fn a_pop_flight_tears_other_removed_pages_down_now_with_the_true_from_depth() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(
        vec![
            page("/a", probe("a", 10.0, &log)),
            page("/b", probe("b", 20.0, &log)),
            flying(page("/c", probe("c", 30.0, &log))),
        ],
        0,
    );
    h.settle(16);

    h.set(vec![page("/a", probe("a", 10.0, &log))], 1000);

    assert!(h.nav().transition.as_ref().unwrap().is_pop);
    assert!(h.nav().held_flight.is_none());
    assert_eq!(
        count(&log, "teardown b"),
        1,
        "not the leaving page: gone now"
    );
    assert_eq!(count(&log, "teardown c"), 0, "the leaving page");
    let state = h.controller.transition();
    assert_eq!((state.from_depth, state.to_depth), (3, 1));
    h.settle(1016);
    assert_eq!(count(&log, "teardown c"), 1);
}

#[test]
fn a_removed_top_with_no_animated_spec_starts_no_pop_flight() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(
        vec![
            page("/a", probe("a", 10.0, &log)),
            page("/b", probe("b", 20.0, &log)),
        ],
        0,
    );

    h.set(vec![page("/a", probe("a", 10.0, &log))], 16);

    assert!(h.nav().transition.is_none());
    assert_eq!(count(&log, "teardown b"), 1);
}

// --- Result callbacks. ---

/// Push a keyed page whose `on_result` logs its name and whether the result was
/// empty.
fn push_with_result(h: &Harness, name: &'static str, results: &Log, log: &Log) {
    let builder = probe(name, 10.0, log);
    let results = Rc::clone(results);
    h.controller.push_with_options(
        move || builder(),
        PushOptions::opaque()
            .key(path_key(&format!("/{name}")))
            .on_result(move |_, result| {
                results
                    .borrow_mut()
                    .push(format!("{name} empty={}", result.is_empty()));
            }),
    );
}

#[test]
fn each_removed_page_fires_on_result_once_with_an_empty_result_on_the_ops_frame() {
    let log = Log::default();
    let results = Log::default();
    let mut h = Harness::new();
    push_with_result(&h, "kept", &results, &log);
    push_with_result(&h, "a", &results, &log);
    push_with_result(&h, "b", &results, &log);
    h.frame(0);
    assert!(results.borrow().is_empty());

    h.set(
        vec![
            page("/kept", probe("kept", 10.0, &log)),
            flying(page("/x", probe("x", 40.0, &log))),
        ],
        1000,
    );

    assert!(h.nav().transition.is_some(), "still flying");
    assert_eq!(
        *results.borrow(),
        ["b empty=true", "a empty=true"],
        "at the op, top to bottom; the retained page's callback stays armed"
    );

    h.settle(1016);
    assert_eq!(results.borrow().len(), 2, "a settle fires nothing again");

    // The retained page still owns its callback.
    h.set(vec![page("/y", probe("y", 50.0, &log))], 2000);
    assert_eq!(results.borrow().last().unwrap(), "kept empty=true");
    assert_eq!(results.borrow().len(), 3);
}

// --- Superseding a running transition. ---

/// The list every superseding test ends on, asserted exactly.
fn assert_exactly_the_list(h: &mut Harness, log: &Log) {
    assert_eq!(h.controller.depth(), 1);
    assert_eq!(h.routes(), ["/x"]);
    assert_eq!(h.nav().pages.len(), 1);
    h.settle(5000);
    assert_eq!(
        h.controller.depth(),
        1,
        "nothing was restored above the list"
    );
    assert_eq!(h.routes(), ["/x"]);
    assert_eq!(h.nav().pages.len(), 1);
    let fills = h.frame(9000);
    assert_eq!(fills.len(), 1);
    assert_eq!(fill_h(&fills, 40.0).0, Point::ZERO);
    assert_eq!(count(log, "teardown x"), 0);
}

#[test]
fn set_pages_supersedes_an_in_flight_push() {
    let log = Log::default();
    let mut h = Harness::new();
    let pushed = probe("pushed", 20.0, &log);
    h.controller.push_with(move || pushed(), slide());
    h.frame(0);
    h.frame(50);
    assert!(h.nav().transition.is_some());

    h.set(vec![page("/x", probe("x", 40.0, &log))], 66);

    assert!(h.nav().transition.is_none());
    assert_eq!(count(&log, "teardown pushed"), 1);
    assert_exactly_the_list(&mut h, &log);
}

#[test]
fn set_pages_supersedes_an_in_flight_pop() {
    let log = Log::default();
    let mut h = Harness::new();
    let pushed = probe("pushed", 20.0, &log);
    h.controller.push_with(move || pushed(), slide());
    h.frame(0);
    h.settle(16);
    h.controller.pop();
    h.frame(1000);
    h.frame(1050);
    assert!(h.nav().transition.as_ref().unwrap().is_pop);

    h.set(vec![flying(page("/x", probe("x", 40.0, &log)))], 1066);

    assert_eq!(
        count(&log, "teardown pushed"),
        1,
        "the stashed page went once"
    );
    assert_eq!(h.controller.transition().from_depth, 1);
    assert_exactly_the_list(&mut h, &log);
    assert_eq!(count(&log, "teardown pushed"), 1);
}

#[test]
fn set_pages_supersedes_its_own_flight_and_disposes_its_held_pages_once() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(
        vec![
            page("/a", probe("a", 10.0, &log)),
            page("/b", probe("b", 20.0, &log)),
        ],
        0,
    );
    h.set(vec![flying(page("/m", probe("m", 30.0, &log)))], 1000);
    h.frame(1050);
    assert_eq!(count(&log, "teardown a"), 0);

    h.set(vec![flying(page("/x", probe("x", 40.0, &log)))], 1066);

    assert_eq!(count(&log, "teardown a"), 1);
    assert_eq!(count(&log, "teardown b"), 1);
    assert_exactly_the_list(&mut h, &log);
    assert_eq!(count(&log, "teardown a"), 1);
    assert_eq!(count(&log, "teardown b"), 1);
    assert_eq!(count(&log, "teardown m"), 1);
}

/// Open a swipe over a keyed page: the finger is still down, the pop uncommitted.
fn swipe_over(h: &mut Harness, name: &'static str, log: &Log, results: &Log) {
    push_with_result(h, name, results, log);
    h.frame(0);
    h.root.event(&mut (), &down(5.0, 50.0));
    h.frame(200);
    h.root.event(&mut (), &move_to(30.0, 50.0));
    assert!(h.nav().transition.as_ref().unwrap().interactive);
    assert_eq!(
        h.controller.depth(),
        2,
        "an uncommitted swipe publishes nothing"
    );
}

#[test]
fn set_pages_completes_an_unreleased_swipe() {
    let log = Log::default();
    let results = Log::default();
    let mut h = Harness::new();
    swipe_over(&mut h, "swiped", &log, &results);

    h.set(vec![page("/x", probe("x", 40.0, &log))], 400);

    assert_eq!(count(&log, "teardown swiped"), 1);
    assert_eq!(
        *results.borrow(),
        ["swiped empty=true"],
        "fired exactly once"
    );
    assert!(!h.nav().edge.active, "the swipe no longer drives anything");
    assert_exactly_the_list(&mut h, &log);
    assert_eq!(count(&log, "teardown swiped"), 1);
}

#[test]
fn set_pages_during_a_cancelled_swipes_settle_restores_nothing_above_the_list() {
    let log = Log::default();
    let results = Log::default();
    let mut h = Harness::new();
    swipe_over(&mut h, "swiped", &log, &results);
    h.frame(400);
    h.root.event(&mut (), &up(30.0, 50.0));
    h.frame(416);
    assert!(h.nav().transition.as_ref().unwrap().restore_on_finalize);

    h.set(vec![flying(page("/x", probe("x", 40.0, &log)))], 432);

    assert_eq!(
        h.controller.transition().from_depth,
        2,
        "the restored page counts towards the pre-op depth"
    );
    assert_eq!(
        *results.borrow(),
        ["swiped empty=true"],
        "fired exactly once"
    );
    assert_exactly_the_list(&mut h, &log);
    assert_eq!(count(&log, "teardown swiped"), 1);
    assert_eq!(results.borrow().len(), 1);
}

#[test]
fn a_cancelled_swipes_page_keyed_by_the_list_is_retained() {
    let log = Log::default();
    let results = Log::default();
    let mut h = Harness::new();
    swipe_over(&mut h, "swiped", &log, &results);
    h.frame(400);
    h.root.event(&mut (), &up(30.0, 50.0));
    h.frame(416);

    h.set(vec![page("/swiped", probe("swiped", 10.0, &log))], 432);

    assert_eq!(h.nav().pages.len(), 1);
    assert_eq!(count(&log, "teardown swiped"), 0, "restored, then retained");
    assert_eq!(count(&log, "build swiped"), 1);
    assert!(results.borrow().is_empty());
}

#[test]
fn set_pages_disarms_a_pending_edge_swipe() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(
        vec![
            page("/a", probe("a", 10.0, &log)),
            page("/b", probe("b", 20.0, &log)),
        ],
        0,
    );
    h.root.event(&mut (), &down(5.0, 50.0));
    assert!(h.nav().edge.armed);

    h.set(
        vec![
            page("/a", probe("a", 10.0, &log)),
            page("/b", probe("b", 20.0, &log)),
        ],
        16,
    );

    assert!(!h.nav().edge.armed);
    assert!(!h.nav().edge.inner_claimed);
}

// --- First build. ---

#[test]
fn a_set_pages_queued_before_the_first_build_replaces_the_root_with_no_flight() {
    let log = Log::default();
    let controller: NavigatorController<()> = NavigatorController::new();
    controller.set_pages(vec![
        flying(page("/a", probe("a", 10.0, &log))),
        flying(page("/x", probe("x", 40.0, &log))),
    ]);
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let root_builder = probe("root", 100.0, &log);
    let mut app = {
        let ctrl = controller.clone();
        move |_: &mut ()| {
            let root_builder = Rc::clone(&root_builder);
            navigator(&ctrl, move || root_builder())
        }
    };

    let (fills, needs_frame) = full_frame(&mut root, &mut app, &mut (), ft(0));

    assert!(
        nav_widget(&root).transition.is_none(),
        "nothing has painted"
    );
    assert!(!controller.transition().active);
    assert!(!needs_frame);
    assert_eq!(controller.depth(), 2);
    assert_eq!(count(&log, "teardown root"), 1);
    assert_eq!(fills.len(), 1);
    assert_eq!(
        fill_h(&fills, 40.0).0,
        Point::ZERO,
        "at rest on the first paint"
    );
}

// --- Queue. ---

#[test]
fn discard_queued_drops_every_unapplied_op() {
    let log = Log::default();
    let mut h = Harness::new();
    let dropped = probe("dropped", 20.0, &log);
    h.controller.push(move || dropped());
    h.controller
        .set_pages(vec![page("/x", probe("x", 40.0, &log))]);

    h.controller.discard_queued();
    h.frame(16);

    assert_eq!(h.routes(), ["/root"]);
    assert_eq!(count(&log, "build dropped"), 0);
    assert_eq!(count(&log, "build x"), 0);
}

// --- Visibility. ---

#[test]
fn a_revealed_retained_page_reports_current() {
    let log = Log::default();
    let seen: Rc<RefCell<Vec<PageVisibility>>> = Rc::default();
    let mut h = Harness::new();
    {
        let seen = Rc::clone(&seen);
        let builder = probe("k", 10.0, &log);
        h.controller.push_with_options(
            move || builder(),
            PushOptions::opaque()
                .key(path_key("/k"))
                .on_visibility(move |v| seen.borrow_mut().push(v)),
        );
    }
    h.frame(0);
    h.controller.push(|| sized_page(100.0, 60.0));
    h.frame(16);
    assert_eq!(
        *seen.borrow(),
        [PageVisibility::Current, PageVisibility::Covered]
    );

    h.set(vec![page("/k", probe("k", 10.0, &log))], 32);

    assert_eq!(seen.borrow().last(), Some(&PageVisibility::Current));
    assert_eq!(seen.borrow().len(), 3, "fired once for the reveal");
}

// --- The IME gate. ---

fn field_surface() -> ImeState {
    ImeState {
        active: true,
        editing: EditingState {
            text: "query".to_string(),
            selection_base: 5,
            selection_extent: 5,
            composing_base: -1,
            composing_extent: -1,
        },
        caret: Some(Rect::new(0.0, 0.0, 1.0, 12.0)),
        content_type: Default::default(),
        suppress_soft_keyboard: false,
    }
}

/// A field-shaped leaf: claims focus on `Down` and republishes its surface on every
/// paint while it holds focus, as a real text input does.
struct Field {
    height: f64,
    cancels: Option<Rc<Cell<u32>>>,
}
struct FieldWidget {
    height: f64,
    cancels: Option<Rc<Cell<u32>>>,
}
impl View<()> for Field {
    type Element = FieldWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> FieldWidget {
        FieldWidget {
            height: self.height,
            cancels: self.cancels.clone(),
        }
    }
    fn rebuild(&self, _: &Self, _: &mut FieldWidget, _: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}
impl Widget for FieldWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, self.height))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        if ctx.has_focus() {
            ctx.publish_ime_state(field_surface());
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Cancel
            && let Some(cancels) = &self.cancels
        {
            cancels.set(cancels.get() + 1);
        }
        if matches!(event, InputEvent::Key(_)) && ctx.has_focus() {
            return EventResult::Handled;
        }
        if let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Down
        {
            ctx.request_focus();
            if self.cancels.is_some() {
                ctx.capture_pointer();
            }
            ctx.publish_ime_state(field_surface());
            return EventResult::Handled;
        }
        EventResult::Ignored
    }
}

fn field(height: f64) -> PageBuilder<()> {
    Rc::new(move || {
        any(Field {
            height,
            cancels: None,
        })
    })
}

/// A [`field`] that also captures the pointer on `Down` and counts the
/// synthetic `Cancel`s it receives.
fn field_counting(height: f64, cancels: &Rc<Cell<u32>>) -> PageBuilder<()> {
    let cancels = Rc::clone(cancels);
    Rc::new(move || {
        any(Field {
            height,
            cancels: Some(Rc::clone(&cancels)),
        })
    })
}

#[test]
fn removing_a_focused_top_clears_the_ime_session() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(vec![page("/f", field(100.0))], 0);
    h.root.event(&mut (), &down(50.0, 50.0));
    h.frame(16);
    assert!(h.root.is_focus_active());

    h.controller
        .set_pages(vec![page("/x", probe("x", 40.0, &log))]);
    h.root.rebuild(&mut h.app, &mut ());
    assert!(h.nav().needs_ime_clear);
    h.root.layout(Size::new(100.0, 100.0));
    h.root
        .paint(&mut RecordingScene::default(), FrameTime::ZERO);

    assert!(!h.root.is_focus_active(), "the session died with its page");
    assert!(h.root.ime_state().is_none());
}

#[test]
fn removing_an_unfocused_top_raises_no_ime_clear() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(vec![page("/a", probe("a", 10.0, &log))], 0);

    h.controller
        .set_pages(vec![page("/x", probe("x", 40.0, &log))]);
    h.root.rebuild(&mut h.app, &mut ());

    assert!(!h.nav().needs_ime_clear);
}

#[test]
fn a_retained_focused_top_raises_no_ime_clear() {
    let mut h = Harness::new();
    h.set(vec![page("/f", field(100.0))], 0);
    h.root.event(&mut (), &down(50.0, 50.0));
    h.frame(16);
    assert!(h.root.is_focus_active());

    h.controller.set_pages(vec![page("/f", field(100.0))]);
    h.root.rebuild(&mut h.app, &mut ());

    assert!(!h.nav().needs_ime_clear, "the old top was not removed");
}

/// A field page on top, tapped so it holds focus, capture and the IME session.
fn focused_field_top(cancels: &Rc<Cell<u32>>, below: &[&str]) -> Harness {
    let log = Log::default();
    let mut h = Harness::new();
    let mut pages: Vec<_> = below
        .iter()
        .map(|p| page(p, probe("u", 40.0, &log)))
        .collect();
    pages.push(page("/f", field_counting(100.0, cancels)));
    h.set(pages, 0);
    h.root.event(&mut (), &down(50.0, 50.0));
    h.frame(16);
    assert!(h.root.is_focus_active());
    assert!(h.nav().pages.last().unwrap().pod.is_focused());
    h
}

fn assert_top_keeps_focus_and_capture(h: &mut Harness, cancels: &Rc<Cell<u32>>) {
    assert!(h.nav().pages.last().unwrap().pod.is_focused());
    assert!(
        h.nav().pages.last().unwrap().pod.is_active(),
        "capture kept"
    );
    assert_eq!(cancels.get(), 0, "no synthetic Cancel reached the top");
    assert!(!h.nav().needs_ime_clear);
    h.root.layout(Size::new(100.0, 100.0));
    h.root
        .paint(&mut RecordingScene::default(), FrameTime::ZERO);
    assert!(h.root.is_focus_active(), "the IME session survived");
    assert!(h.root.ime_state().is_some());
    let key = InputEvent::Key(frust_core::KeyEvent {
        key: frust_core::Key::Character("a".to_string()),
        modifiers: frust_core::Modifiers::default(),
        repeat: false,
    });
    assert!(
        h.root.event(&mut (), &key).handled,
        "a key event still reaches the field"
    );
}

#[test]
fn an_unchanged_focused_top_keeps_focus_capture_and_session() {
    let cancels = Rc::new(Cell::new(0));
    let mut h = focused_field_top(&cancels, &[]);
    h.controller.set_pages(vec![page("/f", field(100.0))]);
    h.root.rebuild(&mut h.app, &mut ());
    assert_top_keeps_focus_and_capture(&mut h, &cancels);
}

#[test]
fn removing_pages_below_a_focused_retained_top_keeps_its_session() {
    let cancels = Rc::new(Cell::new(0));
    let mut h = focused_field_top(&cancels, &["/u"]);
    h.controller.set_pages(vec![page("/f", field(100.0))]);
    h.root.rebuild(&mut h.app, &mut ());
    assert_eq!(h.routes(), ["/f"]);
    assert_top_keeps_focus_and_capture(&mut h, &cancels);
}

#[test]
fn a_fresh_page_over_a_focused_retained_top_covers_it_and_clears() {
    let log = Log::default();
    let cancels = Rc::new(Cell::new(0));
    let mut h = focused_field_top(&cancels, &[]);
    h.controller.set_pages(vec![
        page("/f", field(100.0)),
        page("/x", probe("x", 40.0, &log)),
    ]);
    h.root.rebuild(&mut h.app, &mut ());
    assert!(!h.nav().pages[0].pod.is_focused());
    assert!(!h.nav().pages[0].pod.is_active());
    assert_eq!(cancels.get(), 1, "the capture was cancelled");
    assert!(h.nav().needs_ime_clear);
    h.root.layout(Size::new(100.0, 100.0));
    h.root
        .paint(&mut RecordingScene::default(), FrameTime::ZERO);
    assert!(!h.root.is_focus_active());
}

#[test]
fn a_covered_focused_top_raises_no_clear_when_the_navigator_lacks_focus() {
    let log = Log::default();
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), crate::FlexView<()>> = RenderRoot::new();
    let mut app = field_above_navigator(&controller, field(40.0));
    root.rebuild(&mut app, &mut ());
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    // Focus the navigator's page, then the sibling field above it.
    root.event(&mut (), &down(5.0, 50.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    root.event(&mut (), &down(5.0, 5.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    let generation = root.focus_ime_generation();

    controller.set_pages(vec![
        page("/root", field(40.0)),
        page("/x", probe("x", 40.0, &log)),
    ]);
    root.rebuild(&mut app, &mut ());
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    assert!(root.is_focus_active(), "the sibling keeps its session");
    assert_eq!(root.focus_ime_generation(), generation);
}

#[test]
fn reordering_so_the_old_top_is_covered_cancels_and_clears_it() {
    let log = Log::default();
    let cancels = Rc::new(Cell::new(0));
    let mut h = focused_field_top(&cancels, &["/g"]);
    h.controller.set_pages(vec![
        page("/f", field_counting(100.0, &cancels)),
        page("/g", probe("g", 40.0, &log)),
    ]);
    h.root.rebuild(&mut h.app, &mut ());
    assert_eq!(h.routes(), ["/f", "/g"]);
    assert_eq!(cancels.get(), 1);
    assert!(h.nav().needs_ime_clear);
}

/// `Column[field, navigator]`, 40 tall each: a tap at `y < 40` lands in the field,
/// at `y >= 40` in the navigator's page.
fn field_above_navigator(
    controller: &NavigatorController<()>,
    page_builder: PageBuilder<()>,
) -> impl FnMut(&mut ()) -> crate::FlexView<()> + use<> {
    let ctrl = controller.clone();
    move |_: &mut ()| {
        let page_builder = Rc::clone(&page_builder);
        column()
            .child(Field {
                height: 40.0,
                cancels: None,
            })
            .child(navigator(&ctrl, move || page_builder()))
    }
}

#[test]
fn set_pages_in_an_unfocused_navigator_leaves_a_sibling_fields_session_alive() {
    let log = Log::default();
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), crate::FlexView<()>> = RenderRoot::new();
    let mut app = field_above_navigator(&controller, probe("root", 40.0, &log));
    root.rebuild(&mut app, &mut ());
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    root.event(&mut (), &down(5.0, 5.0));
    assert!(root.is_focus_active());
    let generation = root.focus_ime_generation();

    controller.set_pages(vec![page("/x", probe("x", 40.0, &log))]);
    root.rebuild(&mut app, &mut ());
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

    assert!(root.is_focus_active());
    assert_eq!(root.focus_ime_generation(), generation);
    assert_eq!(
        root.ime_state().map(|s| s.editing.text).as_deref(),
        Some("query")
    );
}

#[test]
fn a_stale_page_focus_flag_under_a_blurred_navigator_raises_no_ime_clear() {
    let log = Log::default();
    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), crate::FlexView<()>> = RenderRoot::new();
    let mut app = field_above_navigator(&controller, field(40.0));
    root.rebuild(&mut app, &mut ());
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    // Focus the navigator's page, then move focus to the sibling field: the page
    // pod's own flag stays set under a navigator link the column cleared.
    root.event(&mut (), &down(5.0, 50.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    root.event(&mut (), &down(5.0, 5.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    assert!(root.is_focus_active());
    let generation = root.focus_ime_generation();

    controller.set_pages(vec![page("/x", probe("x", 40.0, &log))]);
    root.rebuild(&mut app, &mut ());
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

    assert!(
        root.is_focus_active(),
        "the sibling field keeps its session"
    );
    assert_eq!(root.focus_ime_generation(), generation);
}

// --- Instant ops end a live held flight; a superseded push cancels its pop. ---

/// Start a push flight whose old top is retained as the leaving page, over a
/// plain page or, with `dialog`, over a dialog backdrop. Returns the harness
/// mid-flight.
fn held_flight_over(dialog: bool, log: &Log) -> Harness {
    let mut h = Harness::new();
    let list = |top: bool| {
        let mut pages = vec![page("/a", probe("a", 10.0, log))];
        let b = page("/b", probe("b", 20.0, log));
        pages.push(if dialog { transparent(b) } else { b });
        if top {
            pages.push(flying(page("/c", probe("c", 30.0, log))));
        }
        pages
    };
    h.set(list(false), 0);
    h.set(list(true), 1000);
    h.frame(1050);
    let flight = h.nav().held_flight.as_ref().expect("a held flight");
    assert_eq!(flight.leaving_index, Some(1), "the old top is retained");
    h
}

fn assert_flight_ended(h: &mut Harness, depth: usize) {
    // A pop runs its own flight by the popped page's stored spec, so only the
    // held flight is asserted ended before the settle.
    assert!(h.nav().held_flight.is_none());
    h.settle(1100);
    assert!(h.nav().held_flight.is_none());
    assert!(h.nav().transition.is_none());
    assert_eq!(h.nav().pages.len(), depth);
    assert_eq!(h.controller.depth(), depth);
}

#[test]
fn instant_replace_then_two_pops_during_a_held_flight_reach_depth_one() {
    let log = Log::default();
    let mut h = held_flight_over(false, &log);
    h.controller
        .replace_with(|| sized_page(100.0, 40.0), TransitionSpec::NONE);
    h.controller.pop();
    h.controller.pop();
    h.frame(1060);
    assert_flight_ended(&mut h, 1);
}

#[test]
fn each_instant_op_during_a_held_flight_ends_it_before_the_stack_changes() {
    type Op = fn(&NavigatorController<()>);
    let ops: [(Op, usize); 3] = [
        (|c| c.push(|| sized_page(100.0, 40.0)), 4),
        (|c| c.pop(), 2),
        (
            |c| c.replace_with(|| sized_page(100.0, 40.0), TransitionSpec::NONE),
            3,
        ),
    ];
    for dialog in [false, true] {
        for (op, depth) in ops {
            let log = Log::default();
            let mut h = held_flight_over(dialog, &log);
            op(&h.controller);
            h.frame(1060);
            assert_flight_ended(&mut h, depth);
        }
    }
}

#[test]
fn push_then_pop_then_set_pages_in_one_frame_keeps_the_keyed_page() {
    let log = Log::default();
    let mut h = Harness::new();
    let list = || {
        vec![
            page("/a", probe("a", 10.0, &log)),
            page("/b", probe("b", 20.0, &log)),
        ]
    };
    h.set(list(), 0);

    h.controller.push(|| sized_page(100.0, 40.0));
    h.controller.pop();
    h.set(list(), 16);

    assert_eq!(h.routes(), ["/a", "/b"]);
    assert_eq!(count(&log, "build b"), 1);
    assert_eq!(count(&log, "teardown b"), 0);
}

#[test]
fn two_pushes_and_one_pop_then_set_pages_leave_no_pop() {
    let log = Log::default();
    let mut h = Harness::new();
    let list = || {
        vec![
            page("/a", probe("a", 10.0, &log)),
            page("/b", probe("b", 20.0, &log)),
        ]
    };
    h.set(list(), 0);

    h.controller.push(|| sized_page(100.0, 40.0));
    h.controller.push(|| sized_page(100.0, 50.0));
    h.controller.pop();
    h.set(list(), 16);

    assert_eq!(h.routes(), ["/a", "/b"]);
    assert_eq!(count(&log, "teardown b"), 0);
    assert!(!h.controller.has_queued());
}

#[test]
fn pop_with_result_then_set_pages_in_one_frame_delivers_the_payload_once() {
    let log = Log::default();
    let received = Rc::new(RefCell::new(Vec::<i32>::new()));
    let mut h = Harness::new();
    let sink = Rc::clone(&received);
    let builder = probe("b", 20.0, &log);
    h.controller.push_with_options(
        move || builder(),
        PushOptions::opaque()
            .key(path_key("/b"))
            .on_result(move |_, result| {
                sink.borrow_mut().extend(result.take::<i32>());
            }),
    );
    h.frame(0);

    h.controller.pop_with_result(PopResult::of(7i32));
    h.set(vec![page("/x", probe("x", 30.0, &log))], 16);
    h.frame(32);

    assert_eq!(*received.borrow(), [7]);
    assert_eq!(h.routes(), ["/x"]);
}

#[test]
fn an_empty_set_pages_leaves_the_queue_and_the_stack_alone() {
    let log = Log::default();
    let mut h = Harness::new();
    h.set(vec![page("/a", probe("a", 10.0, &log))], 0);

    h.controller.push(|| sized_page(100.0, 40.0));
    h.controller.set_pages(Vec::new());
    assert!(h.controller.has_queued(), "the queued push survives");
    h.frame(16);

    assert_eq!(h.controller.depth(), 2);
    assert_eq!(count(&log, "teardown a"), 0);

    h.controller.set_pages(Vec::new());
    assert!(!h.controller.has_queued(), "nothing was enqueued");
    h.frame(32);
    assert_eq!(h.controller.depth(), 2);
}
