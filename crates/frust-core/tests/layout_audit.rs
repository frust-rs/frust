//! The layout audit (`RenderRoot::set_layout_audit`): after a skipping layout
//! pass it runs a forced full pass and reports every pod whose geometry
//! differs, which is how a widget that changes layout state without signalling
//! dirty is found.

use std::cell::{Cell, RefCell};
use std::sync::Once;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, DiscardScene, FrameTime, LayoutCtx,
    PaintCtx, PaintScene, RenderRoot, View, Widget, any,
};
use kurbo::{Point, Size};
use reactive_graph::owner::Owner;

// ---------------------------------------------------------------------------
// A log sink the tests can read: the audit reports through the `log` crate.
// ---------------------------------------------------------------------------

thread_local! {
    // Per test thread: the audit logs on the thread that lays out.
    static LOGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        LOGS.with(|logs| {
            logs.borrow_mut()
                .push(format!("{}|{}", record.level(), record.args()));
        });
    }
    fn flush(&self) {}
}

fn install_logger() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        log::set_logger(&Capture).expect("one logger per test binary");
        log::set_max_level(log::LevelFilter::Warn);
    });
}

fn audit_logs_naming(needle: &str) -> usize {
    LOGS.with(|logs| {
        logs.borrow()
            .iter()
            .filter(|l| l.starts_with("WARN|layout-audit:") && l.contains(needle))
            .count()
    })
}

thread_local! {
    static LAYOUT_CALLS: Cell<u32> = const { Cell::new(0) };
}

fn layout_calls() -> u32 {
    LAYOUT_CALLS.with(Cell::get)
}

// ---------------------------------------------------------------------------
// Widgets.
// ---------------------------------------------------------------------------

/// A well-behaved leaf: its size changes only through `rebuild`, which signals.
struct SteadyWidget {
    size: Size,
}

impl Widget for SteadyWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        LAYOUT_CALLS.with(|c| c.set(c.get() + 1));
        bc.constrain(self.size)
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

struct SteadyView {
    width: f64,
}

impl<S: 'static> View<S> for SteadyView {
    type Element = SteadyWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SteadyWidget {
        SteadyWidget {
            size: Size::new(self.width, 10.0),
        }
    }
    fn rebuild(&self, prev: &Self, el: &mut SteadyWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        if self.width == prev.width {
            return ChangeFlags::NONE;
        }
        el.size = Size::new(self.width, 10.0);
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }
}

/// A lying leaf: reports a different size on every layout call, with no dirty
/// signal of any kind.
struct LiarWidget {
    calls: f64,
}

impl Widget for LiarWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        LAYOUT_CALLS.with(|c| c.set(c.get() + 1));
        self.calls += 1.0;
        bc.constrain(Size::new(10.0 + self.calls, 10.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

struct LiarView;

impl<S: 'static> View<S> for LiarView {
    type Element = LiarWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LiarWidget {
        LiarWidget { calls: 0.0 }
    }
    fn rebuild(&self, _: &Self, _: &mut LiarWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

struct ColWidget {
    children: Vec<ChildPod>,
}

impl Widget for ColWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let child_bc = BoxConstraints::loose(Size::new(200.0, 200.0));
        let (mut width, mut y) = (0.0f64, 0.0);
        for pod in &mut self.children {
            let size = pod.layout_child(ctx, &child_bc);
            pod.set_origin(Point::new(0.0, y));
            y += size.height;
            width = width.max(size.width);
        }
        bc.constrain(Size::new(width, y))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }
    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        for pod in &self.children {
            visitor(pod);
        }
    }
}

struct ColView<S: 'static> {
    children: Vec<AnyView<S>>,
}

impl<S: 'static> View<S> for ColView<S> {
    type Element = ColWidget;
    fn build(&self, ctx: &mut BuildCtx<'_>) -> ColWidget {
        ColWidget {
            children: self
                .children
                .iter()
                .map(|c| ChildPod::new(Box::new(c.build(ctx))))
                .collect(),
        }
    }
    fn rebuild(&self, prev: &Self, el: &mut ColWidget, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        for (i, pod) in el.children.iter_mut().enumerate() {
            let boxed = pod
                .widget_mut_for_rebuild()
                .downcast_mut::<Box<dyn Widget>>()
                .expect("children are built from AnyView");
            let child = self.children[i].rebuild(&prev.children[i], boxed, ctx);
            if child.contains(ChangeFlags::LAYOUT) {
                pod.mark_layout_dirty();
            }
            flags |= child;
        }
        flags
    }
}

// ---------------------------------------------------------------------------
// Harness.
// ---------------------------------------------------------------------------

const WINDOW: Size = Size::new(300.0, 300.0);

struct Harness {
    root: RenderRoot<(), ColView<()>>,
    _owner: Owner,
}

impl Harness {
    fn new() -> Self {
        install_logger();
        let owner = Owner::new();
        owner.set();
        Self {
            root: RenderRoot::new(),
            _owner: owner,
        }
    }

    fn frame(&mut self, steady_w: f64, with_liar: bool) {
        self.root.rebuild(
            &mut |_: &mut ()| {
                let mut children: Vec<AnyView<()>> = vec![any(SteadyView { width: steady_w })];
                if with_liar {
                    children.push(any(LiarView));
                }
                ColView { children }
            },
            &mut (),
        );
        self.root.layout(WINDOW);
        self.root.paint(&mut DiscardScene, FrameTime::ZERO);
    }
}

#[test]
fn a_lying_widget_is_reported_with_its_type_name() {
    let mut h = Harness::new();
    h.root.set_layout_audit(true);
    for _ in 0..5 {
        h.frame(40.0, true);
    }
    assert!(
        h.root.layout_audit_mismatches() >= 4,
        "the liar differs on every audited frame, got {}",
        h.root.layout_audit_mismatches()
    );
    assert!(audit_logs_naming("LiarWidget") >= 1, "the liar is named");
    // Rate limited: five frames, one log line for the one (type, label).
    assert_eq!(audit_logs_naming("LiarWidget"), 1);
    assert_eq!(audit_logs_naming("SteadyWidget"), 0);
}

#[test]
fn a_well_behaved_tree_reports_nothing_over_scripted_changes() {
    let mut h = Harness::new();
    h.root.set_layout_audit(true);
    for frame in 0..50u32 {
        // A real, signalled size change every 7th frame.
        let width = 40.0 + f64::from(frame / 7) * 3.0;
        h.frame(width, false);
    }
    assert_eq!(h.root.layout_audit_mismatches(), 0);
}

#[test]
fn audit_off_is_a_single_pass() {
    let mut h = Harness::new();
    h.frame(40.0, false);
    let before = layout_calls();
    h.root.layout(WINDOW);
    assert_eq!(layout_calls(), before, "a clean pod is skipped, audit off");

    h.root.set_layout_audit(true);
    h.root.layout(WINDOW);
    assert_eq!(
        layout_calls(),
        before + 1,
        "audit on adds exactly the forced pass"
    );
    assert_eq!(h.root.layout_audit_mismatches(), 0);

    // The audit never runs when skipping is disabled: that is already one
    // forced pass.
    h.root.set_layout_skip(false);
    let before = layout_calls();
    h.root.layout(WINDOW);
    assert_eq!(layout_calls(), before + 1);
}

#[test]
fn the_forced_geometry_stays() {
    let mut h = Harness::new();
    h.root.set_layout_audit(true);
    h.frame(40.0, true);
    h.frame(40.0, true);
    let sizes: Vec<_> = h.root.inspect().iter().map(|n| n.bounds.size()).collect();
    // The liar's size grows by one per layout call; two audited frames ran
    // three calls (the second frame's skipping pass skips the clean liar), and the forced (last) call's size is what is recorded.
    assert_eq!(sizes.last().copied(), Some(Size::new(13.0, 10.0)));
}
