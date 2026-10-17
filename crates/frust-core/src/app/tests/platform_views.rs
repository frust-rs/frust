//! Platform-view frame and z-shield channel tests for RenderRoot.

use super::*;

/// A leaf widget that publishes a fixed [`PlatformViewFrame`] — and reports
/// a z-shield rect over its own bounds — on every paint, unless
/// `should_publish` is false (the widget-level toggle that simulates a slot
/// no longer publishing between two rebuilds). Both channels ride the same
/// toggle so one fixture covers both replace-per-pass contracts.
struct PlatformViewProbeWidget {
    slot_id: u64,
    should_publish: bool,
}

impl crate::widget::Widget for PlatformViewProbeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        if self.should_publish {
            ctx.publish_platform_view(PlatformViewFrame {
                slot_id: self.slot_id,
                view_type: "dev.frust.Probe".to_string(),
                params_json: String::new(),
                params_generation: 0,
                rect: kurbo::Rect::from_origin_size(ctx.origin(), ctx.size()),
                clip: None,
                visible: true,
                interactive: false,
                shields: Vec::new(),
                command_index: 0,
            });
            ctx.report_input_shield(kurbo::Rect::from_origin_size(ctx.origin(), ctx.size()));
        }
    }
}

/// A root widget owning two independently toggleable [`ChildPod`]s (a
/// minimal two-slot container) so a rebuild can flip either slot's
/// `should_publish` — the fixture the "two slots in one pass" and
/// "empty-pass clears stale frames" tests below need.
struct PlatformViewRootWidget {
    a: crate::widget::ChildPod,
    b: crate::widget::ChildPod,
}

impl crate::widget::Widget for PlatformViewRootWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.a.layout_child(ctx, bc);
        self.b.layout_child(ctx, bc);
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.a.paint_child(ctx, scene);
        self.b.paint_child(ctx, scene);
    }
}

/// The `View` producing [`PlatformViewRootWidget`], reconciling each
/// slot's `should_publish` flag on rebuild like any controlled widget.
struct PlatformViewRootView {
    publish_a: bool,
    publish_b: bool,
}

impl View<PvState> for PlatformViewRootView {
    type Element = PlatformViewRootWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> Self::Element {
        PlatformViewRootWidget {
            a: crate::widget::ChildPod::new(Box::new(PlatformViewProbeWidget {
                slot_id: 1,
                should_publish: self.publish_a,
            })),
            b: crate::widget::ChildPod::new(Box::new(PlatformViewProbeWidget {
                slot_id: 2,
                should_publish: self.publish_b,
            })),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.publish_a != self.publish_a || prev.publish_b != self.publish_b {
            element
                .a
                .widget_mut()
                .downcast_mut::<PlatformViewProbeWidget>()
                .expect("slot a stays a PlatformViewProbeWidget")
                .should_publish = self.publish_a;
            element
                .b
                .widget_mut()
                .downcast_mut::<PlatformViewProbeWidget>()
                .expect("slot b stays a PlatformViewProbeWidget")
                .should_publish = self.publish_b;
            ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

/// App state for the platform-view frame-channel tests.
#[derive(Default)]
struct PvState {
    publish_a: bool,
    publish_b: bool,
}

fn platform_view_logic(state: &mut PvState) -> PlatformViewRootView {
    PlatformViewRootView {
        publish_a: state.publish_a,
        publish_b: state.publish_b,
    }
}

#[test]
fn platform_view_frames_arrive_in_order_and_clear_on_empty_pass() {
    let mut root: RenderRoot<PvState, PlatformViewRootView> = RenderRoot::new();
    let mut state = PvState {
        publish_a: true,
        publish_b: true,
    };
    root.rebuild(&mut platform_view_logic, &mut state);
    root.layout(Size::new(200.0, 200.0));

    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);

    // Two slots publishing in one pass both arrive, in paint order (an Option-based
    // shape would keep only the second).
    let frames = root.platform_view_frames();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].slot_id, 1);
    assert_eq!(frames[1].slot_id, 2);

    // Next pass neither slot publishes: the collection is REPLACED, so earlier
    // frames must not survive as stale entries.
    state.publish_a = false;
    state.publish_b = false;
    root.rebuild(&mut platform_view_logic, &mut state);
    root.layout(Size::new(200.0, 200.0));
    root.paint(&mut scene, FrameTime::ZERO);
    assert!(
        root.platform_view_frames().is_empty(),
        "a paint pass with no publishers must yield an empty slice"
    );
}

#[test]
fn input_shields_arrive_in_order_and_clear_on_empty_pass() {
    // Shield channel: two shields in one pass both survive, and a pass reporting none
    // replaces the collection, so a stale shield never steals input.
    let mut root: RenderRoot<PvState, PlatformViewRootView> = RenderRoot::new();
    let mut state = PvState {
        publish_a: true,
        publish_b: true,
    };
    root.rebuild(&mut platform_view_logic, &mut state);
    root.layout(Size::new(200.0, 200.0));

    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    assert_eq!(root.input_shields().len(), 2);
    assert_eq!(
        root.input_shields()[0],
        Rect::from_origin_size(Point::ZERO, Size::new(10.0, 10.0)),
        "a shield is reported in absolute paint coordinates"
    );

    state.publish_a = false;
    state.publish_b = false;
    root.rebuild(&mut platform_view_logic, &mut state);
    root.layout(Size::new(200.0, 200.0));
    root.paint(&mut scene, FrameTime::ZERO);
    assert!(
        root.input_shields().is_empty(),
        "a paint pass reporting no shields must yield an empty slice"
    );
}

#[test]
fn retired_platform_views_drain_exactly_once() {
    // Prompt teardown: a reported slot id is handed to the shell once, like
    // `take_change_flags`; serialized via `RETIRE_TEST_LOCK` (process-wide list).
    let _guard = RETIRE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut root: RenderRoot<PvState, PlatformViewRootView> = RenderRoot::new();
    let _ = root.take_retired_platform_views(); // clear anything a sibling left

    crate::widget::report_retired_slot(7);
    crate::widget::report_retired_slot(9);
    assert_eq!(root.take_retired_platform_views(), vec![7, 9]);
    assert!(
        root.take_retired_platform_views().is_empty(),
        "draining is destructive — a second drain reports nothing"
    );
}

#[test]
fn retired_platform_views_are_capped_dropping_the_oldest() {
    // A shell that never drains must not grow the list forever: past the cap the
    // OLDEST id is dropped.
    let _guard = RETIRE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut root: RenderRoot<PvState, PlatformViewRootView> = RenderRoot::new();
    let _ = root.take_retired_platform_views();

    for slot_id in 0..1_000u64 {
        crate::widget::report_retired_slot(slot_id);
    }
    let drained = root.take_retired_platform_views();
    assert!(drained.len() <= 256, "the pending list stays bounded");
    assert_eq!(
        *drained.last().expect("non-empty"),
        999,
        "the newest report always survives"
    );
    assert!(
        !drained.contains(&0),
        "the oldest reports are the ones dropped"
    );
}

/// Serializes the two tests that drive the process-wide retire list
/// (`crate::widget::report_retired_slot`), which `cargo test`'s parallel
/// threads would otherwise interleave — the same shape
/// `frust-shell-common::theme_override`'s tests use for its global slot.
static RETIRE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
