//! File-drop phase routing tests for RenderRoot.

use super::*;

/// Every `FileDrop` phase a leaf saw, in order, and whether the leaf
/// handles the hit-tested phases.
#[derive(Default)]
struct DropLog {
    handles: bool,
    seen: Vec<(char, FileDropPhase)>,
}

struct DropLeaf {
    tag: char,
}
impl crate::widget::Widget for DropLeaf {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(40.0, 40.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::FileDrop(drop) = event else {
            return EventResult::Ignored;
        };
        let log = ctx.state_mut::<DropLog>();
        log.seen.push((self.tag, drop.phase));
        if log.handles && !event.is_broadcast() {
            EventResult::Handled
        } else {
            EventResult::Ignored
        }
    }
}

/// One 40x40 leaf at the origin of a 200x200 root that routes like
/// `frust-widgets`' helpers: a broadcast to the child unconditionally,
/// anything else only on a hit.
struct DropRoot {
    leaf: crate::widget::ChildPod,
}
impl crate::widget::Widget for DropRoot {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.leaf.layout_child(ctx, bc);
        self.leaf.set_origin(Point::ZERO);
        bc.max()
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.leaf.paint_child(ctx, scene);
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.leaf.event_child(ctx, event);
            return EventResult::Ignored;
        }
        if self.leaf.contains(event.position()) {
            return self.leaf.event_child(ctx, event);
        }
        EventResult::Ignored
    }
}

struct DropRootView;
impl View<DropLog> for DropRootView {
    type Element = DropRoot;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> DropRoot {
        DropRoot {
            leaf: crate::widget::ChildPod::new(Box::new(DropLeaf { tag: 'L' })),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut DropRoot,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn drop_root(handles: bool) -> (RenderRoot<DropLog, DropRootView>, DropLog) {
    let mut root: RenderRoot<DropLog, DropRootView> = RenderRoot::new();
    let mut log = DropLog {
        handles,
        seen: Vec::new(),
    };
    root.rebuild(&mut |_| DropRootView, &mut log);
    root.layout(Size::new(200.0, 200.0));
    (root, log)
}

fn file_drop(phase: FileDropPhase, x: f64, y: f64) -> InputEvent {
    InputEvent::FileDrop(FileDropEvent {
        phase,
        position: Point::new(x, y),
        paths: Vec::new(),
    })
}

#[test]
fn an_unhandled_file_drop_end_is_re_broadcast_to_the_whole_tree() {
    for phase in [FileDropPhase::Drop, FileDropPhase::Cancel] {
        let (mut root, mut log) = drop_root(false);
        // Far outside the leaf: the hit-tested dispatch reaches nothing.
        let outcome = root.event(&mut log, &file_drop(phase, 150.0, 150.0));
        assert!(!outcome.handled, "a broadcast is never consumed");
        assert_eq!(
            log.seen,
            vec![('L', FileDropPhase::Ended)],
            "the {phase:?} that missed the leaf is followed by an Ended broadcast it hears"
        );

        // Over the leaf but ignored by it: the leaf hears the hit-tested
        // phase, then Ended.
        log.seen.clear();
        root.event(&mut log, &file_drop(phase, 10.0, 10.0));
        assert_eq!(log.seen, vec![('L', phase), ('L', FileDropPhase::Ended)]);
    }
}

#[test]
fn a_handled_file_drop_end_is_still_re_broadcast_to_the_whole_tree() {
    let (mut root, mut log) = drop_root(true);
    for phase in [FileDropPhase::Drop, FileDropPhase::Cancel] {
        log.seen.clear();
        let outcome = root.event(&mut log, &file_drop(phase, 10.0, 10.0));
        assert!(outcome.handled, "the hit-tested {phase:?} was handled");
        assert_eq!(
            log.seen,
            vec![('L', phase), ('L', FileDropPhase::Ended)],
            "and is still followed by the Ended broadcast"
        );
    }
}

#[test]
fn a_hover_and_a_direct_ended_get_no_follow_up() {
    let (mut root, mut log) = drop_root(false);
    root.event(&mut log, &file_drop(FileDropPhase::Hover, 150.0, 150.0));
    assert!(log.seen.is_empty(), "a Hover is never re-broadcast");

    root.event(&mut log, &file_drop(FileDropPhase::Ended, 0.0, 0.0));
    assert_eq!(
        log.seen,
        vec![('L', FileDropPhase::Ended)],
        "an Ended dispatched directly is delivered once, not followed again"
    );
}
