//! Drives `RenderRoot::paint` with a ledger: a boundary-like widget nested one
//! pod deep (so the ledger must be threaded to children) reads
//! `PaintCtx::retained_available` and records the answers it saw.

use std::cell::{Cell, RefCell};

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, DiscardScene, FrameTime, LayoutCtx,
    PaintCtx, PaintScene, RenderRoot, RetainedKey, RetainedLedger, View, Widget, any,
};
use kurbo::{Point, Size};
use reactive_graph::owner::Owner;

thread_local! {
    static ANSWERS: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    /// The recording serial the boundary hands `retained_available`.
    static RECORDED: Cell<u64> = const { Cell::new(0) };
    /// The paint serials the boundary saw.
    static SERIALS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

const KEY: RetainedKey = RetainedKey(42);
/// The page the boundary asks about: 100x50 logical at scale 2 -> 200x100 px.
const PAGE: Size = Size::new(200.0, 100.0);

struct Boundary;

impl Widget for Boundary {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, 50.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        let available = ctx.retained_available(KEY, PAGE, 2.0, RECORDED.with(Cell::get));
        ANSWERS.with(|a| a.borrow_mut().push(available));
        SERIALS.with(|s| s.borrow_mut().push(ctx.paint_serial()));
    }
}

struct BoundaryView;

impl<S: 'static> View<S> for BoundaryView {
    type Element = Boundary;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> Boundary {
        Boundary
    }
    fn rebuild(&self, _: &Self, _: &mut Boundary, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

struct Host {
    child: ChildPod,
}

impl Widget for Host {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self
            .child
            .layout_child(ctx, &BoxConstraints::loose(bc.max()));
        self.child.set_origin(Point::ZERO);
        bc.constrain(size)
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }
    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        visitor(&self.child);
    }
}

struct HostView;

impl<S: 'static> View<S> for HostView {
    type Element = Host;
    fn build(&self, ctx: &mut BuildCtx<'_>) -> Host {
        let child: AnyView<S> = any(BoundaryView);
        Host {
            child: ChildPod::new(Box::new(child.build(ctx))),
        }
    }
    fn rebuild(&self, _: &Self, _: &mut Host, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn paint_once(root: &mut RenderRoot<(), HostView>) -> bool {
    ANSWERS.with(|a| a.borrow_mut().clear());
    root.layout(Size::new(300.0, 300.0));
    root.paint(&mut DiscardScene, FrameTime::ZERO);
    ANSWERS.with(|a| *a.borrow().last().expect("the boundary painted"))
}

fn root() -> (RenderRoot<(), HostView>, Owner) {
    RECORDED.with(|r| r.set(0));
    SERIALS.with(|s| s.borrow_mut().clear());
    let owner = Owner::new();
    owner.set();
    let mut root = RenderRoot::new();
    root.rebuild(&mut |_: &mut ()| HostView, &mut ());
    (root, owner)
}

fn resident() -> RetainedLedger {
    RetainedLedger::from_entries(1, [(KEY.get(), (200, 100), 2.0)])
}

#[test]
fn default_empty_ledger_reports_nothing_available() {
    let (mut root, _owner) = root();
    assert!(!paint_once(&mut root));
}

#[test]
fn a_boundary_nested_under_a_host_sees_the_published_ledger() {
    let (mut root, _owner) = root();
    root.set_retained_ledger(&resident());
    assert!(paint_once(&mut root));
    // Replaced by a ledger without the key: no longer available.
    root.set_retained_ledger(&RetainedLedger::empty());
    assert!(!paint_once(&mut root));
}

#[test]
fn extent_or_scale_mismatch_is_not_available() {
    let (mut root, _owner) = root();
    root.set_retained_ledger(&RetainedLedger::from_entries(
        1,
        [(KEY.get(), (199, 100), 2.0)],
    ));
    assert!(!paint_once(&mut root));
    root.set_retained_ledger(&RetainedLedger::from_entries(
        1,
        [(KEY.get(), (200, 100), 3.0)],
    ));
    assert!(!paint_once(&mut root));
}

#[test]
fn the_kill_switch_makes_retained_available_false() {
    let (mut root, _owner) = root();
    root.set_retained_ledger(&resident());
    root.set_retained_layers(false);
    assert!(!paint_once(&mut root));
    root.set_retained_layers(true);
    assert!(paint_once(&mut root));
}

#[test]
fn each_paint_mints_the_next_serial_and_reports_it() {
    let (mut root, _owner) = root();
    root.layout(Size::new(300.0, 300.0));
    let first = root.paint(&mut DiscardScene, FrameTime::ZERO);
    let second = root.paint(&mut DiscardScene, FrameTime::ZERO);
    assert_eq!((first.paint_serial, second.paint_serial), (1, 2));
    assert_eq!(
        SERIALS.with(|s| s.borrow().clone()),
        vec![1, 2],
        "the pod below the root painted under the same serials"
    );
}

#[test]
fn a_ledger_that_predates_the_recording_does_not_vouch_for_it() {
    let (mut root, _owner) = root();
    // The page was recorded in paint 3; the ledger describes the store
    // after scene 2 was committed.
    RECORDED.with(|r| r.set(3));
    root.set_retained_ledger(&RetainedLedger::from_entries(
        2,
        [(KEY.get(), (200, 100), 2.0)],
    ));
    assert!(!paint_once(&mut root));
    root.set_retained_ledger(&RetainedLedger::from_entries(
        3,
        [(KEY.get(), (200, 100), 2.0)],
    ));
    assert!(paint_once(&mut root));
}
