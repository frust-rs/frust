//! Scale (pinch) event routing tests for RenderRoot.

use super::*;
use crate::event::{ScaleEvent, ScalePhase};

/// What every leaf saw: which leaf, which event.
#[derive(Default)]
struct Log {
    seen: Vec<(char, ScaleEvent)>,
}

/// Logs every [`InputEvent::Scale`] it receives, in its own local space,
/// and reports `Handled` only when `handles` is set — everything else
/// (including a non-`Scale` event) is ignored.
struct ScaleLeaf {
    tag: char,
    handles: bool,
}
impl crate::widget::Widget for ScaleLeaf {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(40.0, 40.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Scale(scale) = event else {
            return EventResult::Ignored;
        };
        ctx.state_mut::<Log>().seen.push((self.tag, *scale));
        if self.handles {
            EventResult::Handled
        } else {
            EventResult::Ignored
        }
    }
}

/// Two leaves at identical bounds `(0, 0)..(40, 40)` — `top` painted (and
/// hit-tested) before `bottom`, mirroring `frust-widgets::route_event`'s
/// topmost-first, fall-through-on-`Ignored` hit test: a `Scale` landing in
/// the shared rect reaches `top` first, and only reaches `bottom` if `top`
/// ignores it.
struct Overlapping {
    top: crate::widget::ChildPod,
    bottom: crate::widget::ChildPod,
}
impl crate::widget::Widget for Overlapping {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.top.layout_child(ctx, bc);
        self.top.set_origin(Point::ZERO);
        self.bottom.layout_child(ctx, bc);
        self.bottom.set_origin(Point::ZERO);
        bc.max()
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.bottom.paint_child(ctx, scene);
        self.top.paint_child(ctx, scene);
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let pos = event.position();
        for pod in [&mut self.top, &mut self.bottom] {
            if pod.contains(pos) && pod.event_child(ctx, event) == EventResult::Handled {
                return EventResult::Handled;
            }
        }
        EventResult::Ignored
    }
}

/// Wraps [`Overlapping`] one container deeper, offset by `(10, 20)` — so
/// the fixture also proves [`InputEvent::translated`] shifts a `Scale`
/// event's focal point correctly across a container boundary.
struct Offset {
    inner: crate::widget::ChildPod,
}
impl crate::widget::Widget for Offset {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.inner.layout_child(ctx, bc);
        self.inner.set_origin(Point::new(10.0, 20.0));
        bc.max()
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.inner.paint_child(ctx, scene);
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let pos = event.position();
        if self.inner.contains(pos) {
            return self.inner.event_child(ctx, event);
        }
        EventResult::Ignored
    }
}

struct OffsetView {
    top_handles: bool,
}
impl View<Log> for OffsetView {
    type Element = Offset;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> Offset {
        Offset {
            inner: crate::widget::ChildPod::new(Box::new(Overlapping {
                top: crate::widget::ChildPod::new(Box::new(ScaleLeaf {
                    tag: 'T',
                    handles: self.top_handles,
                })),
                bottom: crate::widget::ChildPod::new(Box::new(ScaleLeaf {
                    tag: 'B',
                    handles: true,
                })),
            })),
        }
    }
    fn rebuild(&self, _prev: &Self, _element: &mut Offset, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn root_with(top_handles: bool) -> (RenderRoot<Log, OffsetView>, Log) {
    let mut root: RenderRoot<Log, OffsetView> = RenderRoot::new();
    let mut log = Log::default();
    root.rebuild(&mut |_| OffsetView { top_handles }, &mut log);
    root.layout(Size::new(200.0, 200.0));
    (root, log)
}

fn scale(phase: ScalePhase, scale_delta: f64, x: f64, y: f64) -> InputEvent {
    InputEvent::Scale(ScaleEvent {
        phase,
        scale_delta,
        focal: Point::new(x, y),
        velocity: 0.0,
    })
}

#[test]
fn scale_hit_tests_by_focal_point_and_translates_through_a_container() {
    let (mut root, mut log) = root_with(true);

    // Window (5, 5): outside the offset container entirely (it starts at
    // (10, 20)) — nothing is hit, nothing logged.
    let outside = root.event(&mut log, &scale(ScalePhase::Begin, 1.1, 5.0, 5.0));
    assert!(
        log.seen.is_empty(),
        "a focal point outside every pod hits nothing"
    );
    assert!(!outside.handled);

    // Window (30, 40) is local (20, 20) after the Offset origin is subtracted: inside
    // both overlapping leaves, so the topmost (`top`) sees it.
    let inside = root.event(&mut log, &scale(ScalePhase::Update, 1.2, 30.0, 40.0));
    assert!(inside.handled);
    assert_eq!(log.seen.len(), 1, "the topmost leaf alone handled it");
    let (tag, seen) = log.seen[0];
    assert_eq!(tag, 'T');
    assert_eq!(
        seen.focal,
        Point::new(20.0, 20.0),
        "translated() shifted the focal point into the container's local space"
    );
    assert_eq!(seen.scale_delta, 1.2);
    assert_eq!(seen.phase, ScalePhase::Update);
}

#[test]
fn an_ignored_scale_bubbles_to_the_next_hit_widget() {
    // `top` ignores every `Scale`; `bottom` at identical bounds still handles it, so
    // the hit test falls through instead of swallowing at the first hit.
    let (mut root, mut log) = root_with(false);
    let outcome = root.event(&mut log, &scale(ScalePhase::Begin, 0.9, 30.0, 40.0));
    assert!(outcome.handled, "the bottom leaf still handled it");
    assert_eq!(
        log.seen.iter().map(|(tag, _)| *tag).collect::<Vec<_>>(),
        vec!['T', 'B'],
        "the ignoring top leaf saw it first, and bottom is what it bubbled to"
    );
}
