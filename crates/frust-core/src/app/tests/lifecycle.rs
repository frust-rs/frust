//! RenderRoot lifecycle tests: build, layout, paint, change flags, theme and insets.

use super::*;
use crate::insets::{EdgeInsets, WindowInsets};

#[test]
fn build_inserts_widget_into_arena() {
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    let mut state = AppState {
        label: "hello".to_string(),
    };
    let flags = root.rebuild(&mut build, &mut state);
    // First build dirties both passes.
    assert!(flags.needs_layout());
    assert!(flags.needs_paint());
    let id = root.root_id().expect("root built");
    let pod = root.tree().pod(id).expect("pod in arena");
    assert!(pod.widget().downcast_ref_is::<TextWidget>());
}

#[test]
fn rebuild_changed_data_yields_paint() {
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    let mut state = AppState {
        label: "a".to_string(),
    };
    root.rebuild(&mut build, &mut state);
    state.label = "b".to_string();
    let flags = root.rebuild(&mut build, &mut state);
    assert_eq!(flags, ChangeFlags::PAINT);
}

#[test]
fn rebuild_unchanged_data_yields_none() {
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    let mut state = AppState {
        label: "same".to_string(),
    };
    root.rebuild(&mut build, &mut state);
    let flags = root.rebuild(&mut build, &mut state);
    assert_eq!(flags, ChangeFlags::NONE);
}

#[test]
fn layout_stores_size_in_pod() {
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    let mut state = AppState {
        label: "hi".to_string(),
    };
    root.rebuild(&mut build, &mut state);
    let size = root.layout(Size::new(800.0, 600.0));
    // "hi" -> 2 * 8 = 16 wide, 16 tall, within the window.
    assert_eq!(size, Size::new(16.0, 16.0));
    let id = root.root_id().unwrap();
    let pod = root.tree().pod(id).unwrap();
    assert_eq!(pod.origin(), Point::ZERO);
    assert_eq!(pod.size(), Size::new(16.0, 16.0));
}

#[test]
fn inspect_reports_the_laid_out_root() {
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    let mut state = AppState {
        label: "hi".to_string(),
    };
    // Before the first build there is nothing to inspect.
    assert!(root.inspect().is_empty());

    root.rebuild(&mut build, &mut state);
    root.layout(Size::new(800.0, 600.0));

    let nodes = root.inspect();
    assert_eq!(nodes.len(), 1);
    let node = &nodes[0];
    assert_eq!(node.id, root.root_id().unwrap());
    assert_eq!(node.parent, None);
    assert_eq!(node.depth, 0);
    assert!(node.children.is_empty());
    // The concrete element type is captured, not the erased box.
    assert!(node.type_name.ends_with("TextWidget"), "{}", node.type_name);
    assert_eq!(node.debug_label, None);
    // Bounds match what the layout pass recorded on the pod.
    let pod = root.tree().pod(node.id).unwrap();
    assert_eq!(
        node.bounds,
        Rect::from_origin_size(pod.origin(), pod.size())
    );
    assert_eq!(node.bounds, Rect::new(0.0, 0.0, 16.0, 16.0));
}

#[test]
fn layout_clamps_to_window() {
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    let mut state = AppState {
        label: "wwwwwwwwwww".to_string(), // 10 chars -> 80 wide intrinsic
    };
    root.rebuild(&mut build, &mut state);
    let size = root.layout(Size::new(40.0, 40.0));
    // Intrinsic width 80 is clamped to the 40-wide window.
    assert_eq!(size.width, 40.0);
}

#[test]
fn paint_emits_current_text() {
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    let mut state = AppState {
        label: "one".to_string(),
    };
    root.rebuild(&mut build, &mut state);
    root.layout(Size::new(200.0, 200.0));

    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    assert_eq!(scene.texts, vec![(Point::ZERO, "one".to_string())]);

    // Change data, rebuild, repaint -> new text.
    state.label = "two".to_string();
    root.rebuild(&mut build, &mut state);
    let mut scene2 = RecordingScene::default();
    root.paint(&mut scene2, FrameTime::ZERO);
    assert_eq!(scene2.texts, vec![(Point::ZERO, "two".to_string())]);
}

/// A root widget that advances no state but requests a continuation frame on
/// every paint — stands in for an animating widget (e.g. a scroll fling).
struct FrameWidget;

impl crate::widget::Widget for FrameWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        ctx.request_frame();
    }
}

struct FrameView;

impl View<AppState> for FrameView {
    type Element = FrameWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> FrameWidget {
        FrameWidget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut FrameWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

#[test]
fn paint_reports_needs_frame_from_animating_root() {
    // A still root reports no continuation frame.
    let mut still: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    let mut state = AppState {
        label: "x".to_string(),
    };
    still.rebuild(&mut build, &mut state);
    still.layout(Size::new(100.0, 100.0));
    let mut scene = RecordingScene::default();
    assert!(!still.paint(&mut scene, FrameTime::ZERO).needs_frame);

    // An animating root bubbles request_frame out as PaintOutcome::needs_frame.
    let mut anim: RenderRoot<AppState, FrameView> = RenderRoot::new();
    anim.rebuild(&mut |_s: &mut AppState| FrameView, &mut state);
    anim.layout(Size::new(100.0, 100.0));
    let mut scene2 = RecordingScene::default();
    assert!(anim.paint(&mut scene2, FrameTime::ZERO).needs_frame);
}

/// A root widget whose animation changes its layout: it requests a layout
/// re-run on every paint — stands in for an expanding accordion.
struct LayoutFrameWidget;

impl crate::widget::Widget for LayoutFrameWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        ctx.request_layout();
    }
}

struct LayoutFrameView;

impl View<AppState> for LayoutFrameView {
    type Element = LayoutFrameWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LayoutFrameWidget {
        LayoutFrameWidget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut LayoutFrameWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

#[test]
fn paint_folds_request_layout_into_pending_change_flags() {
    let mut state = AppState {
        label: "x".to_string(),
    };

    // `request_layout` in paint surfaces on the outcome AND folds LAYOUT into
    // `pending`, so the next `take_change_flags` reports `needs_layout()`.
    let mut anim: RenderRoot<AppState, LayoutFrameView> = RenderRoot::new();
    anim.rebuild(&mut |_s: &mut AppState| LayoutFrameView, &mut state);
    anim.layout(Size::new(100.0, 100.0));
    // Drain any rebuild/layout dirtiness so we observe only paint's fold.
    let _ = anim.take_change_flags();
    let mut scene = RecordingScene::default();
    let outcome = anim.paint(&mut scene, FrameTime::ZERO);
    assert!(outcome.needs_layout, "outcome reports needs_layout");
    // `request_layout` implies `request_frame`, so the animation still runs.
    assert!(outcome.needs_frame, "request_layout implies needs_frame");
    assert!(
        anim.has_pending_change_flags(),
        "the fold survives to the next frame"
    );
    assert!(
        anim.take_change_flags().needs_layout(),
        "next frame's take_change_flags reports needs_layout"
    );
}

#[test]
fn paint_request_frame_only_folds_layout_into_pending() {
    let mut state = AppState {
        label: "x".to_string(),
    };

    // A paint-only animation dirties its path conservatively, folding LAYOUT into
    // pending, but does not report `needs_layout` on the outcome.
    let mut anim: RenderRoot<AppState, FrameView> = RenderRoot::new();
    anim.rebuild(&mut |_s: &mut AppState| FrameView, &mut state);
    anim.layout(Size::new(100.0, 100.0));
    let _ = anim.take_change_flags();
    let mut scene = RecordingScene::default();
    let outcome = anim.paint(&mut scene, FrameTime::ZERO);
    assert!(outcome.needs_frame);
    assert!(
        !outcome.needs_layout,
        "request_frame alone: no needs_layout on the outcome"
    );
    assert!(
        anim.take_change_flags().needs_layout(),
        "request_frame alone folds LAYOUT into pending"
    );
}

/// A root whose paint requests a *pacable* cosmetic-loop frame — stands in
/// for a skeleton shimmer whose cadence the mobile frame gate may throttle.
struct PacedFrameWidget;

impl crate::widget::Widget for PacedFrameWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        ctx.request_frame_paced();
    }
}

struct PacedFrameView;

impl View<AppState> for PacedFrameView {
    type Element = PacedFrameWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> PacedFrameWidget {
        PacedFrameWidget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut PacedFrameWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

#[test]
fn paint_surfaces_paced_only_tick_class_on_outcome() {
    let mut state = AppState {
        label: "x".to_string(),
    };

    // A paced-only root surfaces `needs_frame_paced_only` on the outcome so
    // the mobile frame gate may throttle its cadence.
    let mut paced: RenderRoot<AppState, PacedFrameView> = RenderRoot::new();
    paced.rebuild(&mut |_s: &mut AppState| PacedFrameView, &mut state);
    paced.layout(Size::new(100.0, 100.0));
    let mut scene = RecordingScene::default();
    let outcome = paced.paint(&mut scene, FrameTime::ZERO);
    assert!(outcome.needs_frame);
    assert!(
        outcome.needs_frame_paced_only,
        "a purely-cosmetic frame surfaces as paced-only"
    );
    assert_eq!(
        outcome.paced_interval,
        Some(std::time::Duration::ZERO),
        "a bare `request_frame_paced` names no interval (the theme's own rate)"
    );

    // A Transition-class (`request_frame`) root is never paced-only, keeping
    // today's every-vsync behavior for existing callers.
    let mut anim: RenderRoot<AppState, FrameView> = RenderRoot::new();
    anim.rebuild(&mut |_s: &mut AppState| FrameView, &mut state);
    anim.layout(Size::new(100.0, 100.0));
    let mut scene2 = RecordingScene::default();
    let outcome2 = anim.paint(&mut scene2, FrameTime::ZERO);
    assert!(outcome2.needs_frame);
    assert!(
        !outcome2.needs_frame_paced_only,
        "request_frame stays unpaced (Transition)"
    );
    assert_eq!(
        outcome2.paced_interval, None,
        "an unpaced frame names no paced interval"
    );
}

/// A root widget whose decorative loop names its own slow cadence — the
/// `request_frame_paced_at` counterpart of [`PacedFrameWidget`].
struct SlowPacedFrameWidget;

impl crate::widget::Widget for SlowPacedFrameWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        ctx.request_frame_paced_at(std::time::Duration::from_millis(500));
    }
}

struct SlowPacedFrameView;

impl View<AppState> for SlowPacedFrameView {
    type Element = SlowPacedFrameWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SlowPacedFrameWidget {
        SlowPacedFrameWidget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut SlowPacedFrameWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

#[test]
fn paint_surfaces_the_requested_paced_interval_on_outcome() {
    // End-to-end core half of per-request pacing: `request_frame_paced_at` reaches
    // the shell on `PaintOutcome`.
    let mut state = AppState {
        label: "x".to_string(),
    };
    let mut root: RenderRoot<AppState, SlowPacedFrameView> = RenderRoot::new();
    root.rebuild(&mut |_s: &mut AppState| SlowPacedFrameView, &mut state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene = RecordingScene::default();
    let outcome = root.paint(&mut scene, FrameTime::ZERO);
    assert!(outcome.needs_frame_paced_only);
    assert_eq!(
        outcome.paced_interval,
        Some(std::time::Duration::from_millis(500))
    );
}

/// A root widget that records the `frame_time` its paint observed, so a test
/// can prove the shell-injected clock reaches `PaintCtx::frame_time()`.
struct ClockWidget {
    seen: std::rc::Rc<std::cell::Cell<Option<FrameTime>>>,
}

impl crate::widget::Widget for ClockWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.seen.set(Some(ctx.frame_time()));
    }
}

struct ClockView {
    seen: std::rc::Rc<std::cell::Cell<Option<FrameTime>>>,
}

impl View<AppState> for ClockView {
    type Element = ClockWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ClockWidget {
        ClockWidget {
            seen: self.seen.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut ClockWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

#[test]
fn paint_threads_injected_frame_time_to_widget() {
    let seen = std::rc::Rc::new(std::cell::Cell::new(None));
    let mut root: RenderRoot<AppState, ClockView> = RenderRoot::new();
    let mut state = AppState::default();
    let seen_for_view = seen.clone();
    root.rebuild(
        &mut move |_s: &mut AppState| ClockView {
            seen: seen_for_view.clone(),
        },
        &mut state,
    );
    root.layout(Size::new(100.0, 100.0));

    // Two paints with distinct injected times: the widget observes each one,
    // proving the clock is shell-fed (not read from an ambient `Instant`).
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::from_nanos(1_000));
    assert_eq!(seen.get(), Some(FrameTime::from_nanos(1_000)));
    root.paint(&mut scene, FrameTime::from_nanos(17_000));
    assert_eq!(seen.get(), Some(FrameTime::from_nanos(17_000)));
}

// --- Theme threading: a dummy theme recovered during paint/layout. ---

/// A dummy theme type standing in for `frust_theme::Theme` — `frust-core`
/// never names the real one, so this proves the type-erased slot works for
/// any `'static` type.
#[derive(Debug, Clone, PartialEq)]
struct TestTheme {
    accent: u32,
}

/// A root widget recording the theme accent it recovered during paint (and
/// during layout), or `None` when no theme was threaded in.
struct ThemeWidget {
    seen_paint: std::rc::Rc<std::cell::Cell<Option<u32>>>,
    seen_layout: std::rc::Rc<std::cell::Cell<Option<u32>>>,
}

impl crate::widget::Widget for ThemeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.seen_layout
            .set(ctx.theme_as::<TestTheme>().map(|t| t.accent));
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.seen_paint
            .set(ctx.theme_as::<TestTheme>().map(|t| t.accent));
    }
}

struct ThemeView {
    seen_paint: std::rc::Rc<std::cell::Cell<Option<u32>>>,
    seen_layout: std::rc::Rc<std::cell::Cell<Option<u32>>>,
}

impl View<AppState> for ThemeView {
    type Element = ThemeWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ThemeWidget {
        ThemeWidget {
            seen_paint: self.seen_paint.clone(),
            seen_layout: self.seen_layout.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut ThemeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn drive_theme_root(theme: Option<TestTheme>) -> (Option<u32>, Option<u32>) {
    let seen_paint = std::rc::Rc::new(std::cell::Cell::new(None));
    let seen_layout = std::rc::Rc::new(std::cell::Cell::new(None));
    let mut root: RenderRoot<AppState, ThemeView> = RenderRoot::new();
    if let Some(theme) = theme {
        root.set_theme(Box::new(theme));
    }
    let mut state = AppState::default();
    let sp = seen_paint.clone();
    let sl = seen_layout.clone();
    root.rebuild(
        &mut move |_s: &mut AppState| ThemeView {
            seen_paint: sp.clone(),
            seen_layout: sl.clone(),
        },
        &mut state,
    );
    root.layout(Size::new(100.0, 100.0));
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    (seen_layout.get(), seen_paint.get())
}

#[test]
fn set_theme_threads_into_layout_and_paint() {
    let (layout, paint) = drive_theme_root(Some(TestTheme { accent: 5 }));
    assert_eq!(layout, Some(5));
    assert_eq!(paint, Some(5));
}

#[test]
fn no_theme_yields_none_in_layout_and_paint() {
    let (layout, paint) = drive_theme_root(None);
    assert_eq!(layout, None);
    assert_eq!(paint, None);
}

#[test]
fn set_theme_replaces_the_previous_theme() {
    // A second `set_theme` (a live dark-mode flip on desktop) wins on the
    // next paint.
    let seen_paint = std::rc::Rc::new(std::cell::Cell::new(None));
    let seen_layout = std::rc::Rc::new(std::cell::Cell::new(None));
    let mut root: RenderRoot<AppState, ThemeView> = RenderRoot::new();
    root.set_theme(Box::new(TestTheme { accent: 1 }));
    let mut state = AppState::default();
    let sp = seen_paint.clone();
    let sl = seen_layout.clone();
    root.rebuild(
        &mut move |_s: &mut AppState| ThemeView {
            seen_paint: sp.clone(),
            seen_layout: sl.clone(),
        },
        &mut state,
    );
    root.layout(Size::new(100.0, 100.0));
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    assert_eq!(seen_paint.get(), Some(1));

    // Flip the theme, repaint — the new accent is observed.
    root.set_theme(Box::new(TestTheme { accent: 2 }));
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut scene, FrameTime::ZERO);
    assert_eq!(seen_paint.get(), Some(2));
}

// --- Event-pass fixtures: a widget that mutates state on pointer-down. ---

struct ButtonWidget;

impl crate::widget::Widget for ButtonWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(40.0, 20.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut crate::event::EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event {
            match p.phase {
                PointerPhase::Down => {
                    ctx.state_mut::<ClickState>().clicks += 1;
                    ctx.request_redraw();
                    ctx.capture_pointer();
                    return EventResult::Handled;
                }
                PointerPhase::Up | PointerPhase::Cancel => return EventResult::Handled,
                PointerPhase::Move => {}
            }
        }
        EventResult::Ignored
    }
}

struct ButtonView;

impl View<ClickState> for ButtonView {
    type Element = ButtonWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ButtonWidget {
        ButtonWidget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut ButtonWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn button_logic(_state: &mut ClickState) -> ButtonView {
    ButtonView
}

#[test]
fn event_reaches_root_widget_and_mutates_state() {
    let mut root: RenderRoot<ClickState, ButtonView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut button_logic, &mut state);
    root.layout(Size::new(200.0, 200.0));

    let outcome = root.event(&mut state, &pointer(PointerPhase::Down, 5.0, 5.0));
    assert!(outcome.handled);
    assert!(outcome.needs_redraw);
    assert_eq!(state.clicks, 1);
    // A captured Down opens the root gesture.
    assert!(root.is_pointer_captured());
}

#[test]
fn event_before_build_is_a_benign_no_op() {
    let mut root: RenderRoot<ClickState, ButtonView> = RenderRoot::new();
    let mut state = ClickState::default();
    let outcome = root.event(&mut state, &pointer(PointerPhase::Down, 1.0, 1.0));
    assert_eq!(outcome, EventOutcome::default());
    assert_eq!(state.clicks, 0);
}

#[test]
fn capture_releases_on_pointer_up() {
    let mut root: RenderRoot<ClickState, ButtonView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut button_logic, &mut state);
    root.layout(Size::new(200.0, 200.0));

    root.event(&mut state, &pointer(PointerPhase::Down, 5.0, 5.0));
    assert!(root.is_pointer_captured());
    root.event(&mut state, &pointer(PointerPhase::Up, 5.0, 5.0));
    assert!(!root.is_pointer_captured());
}

#[test]
fn take_change_flags_drains_accumulated_dirtiness() {
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    let mut state = AppState {
        label: "x".to_string(),
    };
    root.rebuild(&mut build, &mut state);
    // First build accumulated LAYOUT|PAINT.
    let flags = root.take_change_flags();
    assert!(flags.needs_layout());
    // Draining leaves it empty until the next rebuild.
    assert!(root.take_change_flags().is_empty());
}

#[test]
fn has_pending_change_flags_peeks_without_draining() {
    // The frame-gate peek observes pending dirtiness without clearing it, so a
    // skipped frame preserves the flags.
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    assert!(
        !root.has_pending_change_flags(),
        "a fresh root has nothing pending"
    );
    let mut state = AppState {
        label: "x".to_string(),
    };
    root.rebuild(&mut build, &mut state);
    // First build accumulated LAYOUT|PAINT — the peek sees it...
    assert!(root.has_pending_change_flags());
    // ...and repeated peeks do NOT drain it.
    assert!(root.has_pending_change_flags());
    // Only `take_change_flags` drains.
    assert!(!root.take_change_flags().is_empty());
    assert!(!root.has_pending_change_flags());
}

#[test]
fn set_theme_marks_layout_and_paint_pending() {
    // `set_theme` alone (no rebuild) must dirty layout/paint so a gated shell does
    // not skip re-resolving theme-baked widget state.
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    root.set_theme(Box::new(TestTheme { accent: 1 }));
    let flags = root.take_change_flags();
    assert!(flags.needs_layout());
    assert!(flags.needs_paint());

    // Draining clears it until the next `set_theme`/rebuild.
    assert!(root.take_change_flags().is_empty());
    root.set_theme(Box::new(TestTheme { accent: 2 }));
    let flags = root.take_change_flags();
    assert!(flags.needs_layout());
    assert!(flags.needs_paint());
}

// --- Window insets: pushed value reaches layout/paint contexts. ---

/// A root widget recording the `WindowInsets` it observed during layout and
/// paint, proving the shell-pushed value threads through both contexts.
struct InsetsWidget {
    seen_layout: std::rc::Rc<std::cell::Cell<Option<WindowInsets>>>,
    seen_paint: std::rc::Rc<std::cell::Cell<Option<WindowInsets>>>,
}

impl crate::widget::Widget for InsetsWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.seen_layout.set(Some(ctx.window_insets()));
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.seen_paint.set(Some(ctx.window_insets()));
    }
}

struct InsetsView {
    seen_layout: std::rc::Rc<std::cell::Cell<Option<WindowInsets>>>,
    seen_paint: std::rc::Rc<std::cell::Cell<Option<WindowInsets>>>,
}

impl View<AppState> for InsetsView {
    type Element = InsetsWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> InsetsWidget {
        InsetsWidget {
            seen_layout: self.seen_layout.clone(),
            seen_paint: self.seen_paint.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut InsetsWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn drive_insets_root(insets: Option<WindowInsets>) -> (Option<WindowInsets>, Option<WindowInsets>) {
    let seen_layout = std::rc::Rc::new(std::cell::Cell::new(None));
    let seen_paint = std::rc::Rc::new(std::cell::Cell::new(None));
    let mut root: RenderRoot<AppState, InsetsView> = RenderRoot::new();
    if let Some(insets) = insets {
        root.set_insets(insets);
    }
    let mut state = AppState::default();
    let sl = seen_layout.clone();
    let sp = seen_paint.clone();
    root.rebuild(
        &mut move |_s: &mut AppState| InsetsView {
            seen_layout: sl.clone(),
            seen_paint: sp.clone(),
        },
        &mut state,
    );
    root.layout(Size::new(100.0, 100.0));
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    (seen_layout.get(), seen_paint.get())
}

#[test]
fn set_insets_threads_into_layout_and_paint() {
    let insets = WindowInsets::new(
        EdgeInsets::new(0.0, 24.0, 0.0, 34.0),
        EdgeInsets::new(0.0, 0.0, 0.0, 0.0),
    );
    let (layout, paint) = drive_insets_root(Some(insets));
    assert_eq!(layout, Some(insets));
    assert_eq!(paint, Some(insets));
}

#[test]
fn no_insets_yields_zero_in_layout_and_paint() {
    let (layout, paint) = drive_insets_root(None);
    assert_eq!(layout, Some(WindowInsets::default()));
    assert_eq!(paint, Some(WindowInsets::default()));
}

#[test]
fn set_insets_marks_layout_and_paint_pending() {
    // Mirrors `set_theme_marks_layout_and_paint_pending`: a bare inset push must
    // dirty layout/paint so a gated shell relayouts a `SafeArea`.
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    root.set_insets(WindowInsets::new(
        EdgeInsets::new(0.0, 24.0, 0.0, 0.0),
        EdgeInsets::ZERO,
    ));
    let flags = root.take_change_flags();
    assert!(flags.needs_layout());
    assert!(flags.needs_paint());
    // Drained until the next change.
    assert!(root.take_change_flags().is_empty());
}

#[test]
fn set_insets_no_op_when_unchanged_marks_nothing() {
    // The `PartialEq` no-op guard: re-pushing the current insets dirties nothing, so
    // forwarding platform insets every frame never forces a relayout.
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    let insets = WindowInsets::new(EdgeInsets::new(0.0, 24.0, 0.0, 34.0), EdgeInsets::ZERO);
    root.set_insets(insets);
    assert!(!root.take_change_flags().is_empty());
    // Same value again: no dirtiness.
    root.set_insets(insets);
    assert!(root.take_change_flags().is_empty());
    // A different value dirties again.
    root.set_insets(WindowInsets::default());
    assert!(!root.take_change_flags().is_empty());
}

#[test]
fn set_insets_round_trips_corner_insets() {
    use crate::insets::{CornerInset, CornerInsets};
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    let corners = CornerInsets::new(
        CornerInset::ZERO,
        CornerInset::new(72.0, 24.0),
        CornerInset::ZERO,
        CornerInset::ZERO,
    );
    let insets = WindowInsets::default().with_corner_insets(corners);
    root.set_insets(insets);
    let flags = root.take_change_flags();
    assert!(flags.needs_layout());
    assert!(flags.needs_paint());
    assert_eq!(root.insets().corner_insets, corners);
    // Identical re-push marks nothing.
    root.set_insets(insets);
    assert!(root.take_change_flags().is_empty());
    // A push differing only in corners dirties again.
    let moved = insets.with_corner_insets(CornerInsets::new(
        CornerInset::new(72.0, 24.0),
        CornerInset::ZERO,
        CornerInset::ZERO,
        CornerInset::ZERO,
    ));
    root.set_insets(moved);
    let flags = root.take_change_flags();
    assert!(flags.needs_layout());
    assert!(flags.needs_paint());
}

// --- Presented-frame count: pushed value reaches the paint context, unset
//     yields `None`, and — unlike theme/insets — the setter dirties nothing. ---

/// A root widget recording the `presented_frames` count it observed during
/// paint, proving the shell-pushed value threads through `PaintCtx`.
struct PresentedWidget {
    seen_paint: std::rc::Rc<std::cell::Cell<Option<Option<u64>>>>,
}

impl crate::widget::Widget for PresentedWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.seen_paint.set(Some(ctx.presented_frames()));
    }
}

struct PresentedView {
    seen_paint: std::rc::Rc<std::cell::Cell<Option<Option<u64>>>>,
}

impl View<AppState> for PresentedView {
    type Element = PresentedWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> PresentedWidget {
        PresentedWidget {
            seen_paint: self.seen_paint.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut PresentedWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn drive_presented_root(presented: Option<u64>) -> Option<u64> {
    let seen_paint = std::rc::Rc::new(std::cell::Cell::new(None));
    let mut root: RenderRoot<AppState, PresentedView> = RenderRoot::new();
    if let Some(presented) = presented {
        root.set_presented_frames(presented);
    }
    let mut state = AppState::default();
    let sp = seen_paint.clone();
    root.rebuild(
        &mut move |_s: &mut AppState| PresentedView {
            seen_paint: sp.clone(),
        },
        &mut state,
    );
    root.layout(Size::new(100.0, 100.0));
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    // Unwrap the "did paint run" outer Option; the inner is what the widget saw.
    seen_paint.get().expect("paint ran")
}

#[test]
fn set_presented_frames_threads_into_paint() {
    assert_eq!(drive_presented_root(Some(12)), Some(12));
}

#[test]
fn unset_presented_frames_yields_none_in_paint() {
    assert_eq!(drive_presented_root(None), None);
}

#[test]
fn set_presented_frames_marks_no_change_flags() {
    // A presented-count push is a paint-only observation and must dirty NOTHING, or
    // a ticking counter would force relayouts and keep the mobile frame gate on "Run".
    let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
    let gen_before = root.semantics_generation();
    root.set_presented_frames(1);
    assert!(root.take_change_flags().is_empty());
    assert!(!root.has_pending_change_flags());
    // A second, changed push still dirties nothing.
    root.set_presented_frames(2);
    assert!(root.take_change_flags().is_empty());
    // And bumps no semantics generation (mirrors the no-dirty contract).
    assert_eq!(root.semantics_generation(), gen_before);
}

// Small test helper: does the boxed widget downcast to `W`?
trait DowncastRefIs {
    fn downcast_ref_is<W: crate::widget::Widget>(&self) -> bool;
}

impl DowncastRefIs for dyn crate::widget::Widget {
    fn downcast_ref_is<W: crate::widget::Widget>(&self) -> bool {
        (self as &dyn std::any::Any).is::<W>()
    }
}

#[test]
fn orientation_from_size_is_portrait_when_taller_than_wide() {
    assert_eq!(
        Orientation::from_size(Size::new(400.0, 800.0)),
        Orientation::Portrait
    );
}

#[test]
fn orientation_from_size_is_landscape_when_wider_than_tall() {
    assert_eq!(
        Orientation::from_size(Size::new(800.0, 400.0)),
        Orientation::Landscape
    );
}

#[test]
fn orientation_from_size_square_reads_as_portrait() {
    // An exact square satisfies `>=` (height >= width is the rule) and must not
    // resolve ambiguously.
    assert_eq!(
        Orientation::from_size(Size::new(500.0, 500.0)),
        Orientation::Portrait
    );
}

#[test]
fn window_metrics_new_derives_orientation_from_size() {
    let insets = WindowInsets::default();
    let portrait = WindowMetrics::new(Size::new(390.0, 844.0), 3.0, insets);
    assert_eq!(portrait.orientation, Orientation::Portrait);
    assert_eq!(portrait.size, Size::new(390.0, 844.0));
    assert_eq!(portrait.scale, 3.0);
    assert_eq!(portrait.insets, insets);

    let landscape = WindowMetrics::new(Size::new(844.0, 390.0), 3.0, insets);
    assert_eq!(landscape.orientation, Orientation::Landscape);

    let square = WindowMetrics::new(Size::new(500.0, 500.0), 2.0, insets);
    assert_eq!(square.orientation, Orientation::Portrait);
}

// --- Deferred state-bearing callbacks: the `InputEvent::Housekeeping` flush
//     `RenderRoot::rebuild` dispatches. ---

/// App state for the flush tests.
#[derive(Default)]
struct FlushState {
    /// How many deferred callbacks have run.
    flushes: u32,
    /// How many more times a running callback re-queues itself — the knob the
    /// chained/capped tests turn.
    chain_left: u32,
    /// The `flushes` value each build-closure run observed, in order. This is
    /// what proves the rebuild re-runs the build closure *after* a flush rather than
    /// shipping the now-stale pre-flush view.
    observed: Vec<u32>,
}

/// The navigator's deferred-callback shape reduced to one leaf: a shared
/// `Rc<Cell<u32>>` op queue (the `NavigatorController` analog) is drained
/// during the state-free [`View::rebuild`], which can therefore only *queue*
/// the callback and raise the flush mark; [`crate::widget::Widget::event`]
/// runs it when the broadcast arrives, where `&mut State` finally exists.
struct FlushView {
    ops: std::rc::Rc<Cell<u32>>,
}

struct FlushWidget {
    ops: std::rc::Rc<Cell<u32>>,
    queued: u32,
}

impl FlushWidget {
    fn drain_ops(&mut self) {
        let ops = self.ops.replace(0);
        if ops > 0 {
            self.queued += ops;
            crate::event::mark_pending_result_flush();
        }
    }
}

impl View<FlushState> for FlushView {
    type Element = FlushWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> FlushWidget {
        let mut widget = FlushWidget {
            ops: self.ops.clone(),
            queued: 0,
        };
        widget.drain_ops();
        widget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut FlushWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.ops = self.ops.clone();
        element.drain_ops();
        ChangeFlags::NONE
    }
}

impl crate::widget::Widget for FlushWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !event.is_broadcast() || self.queued == 0 {
            return EventResult::Ignored;
        }
        let queued = std::mem::take(&mut self.queued);
        let state = ctx.state_mut::<FlushState>();
        for _ in 0..queued {
            state.flushes += 1;
            if state.chain_left > 0 {
                state.chain_left -= 1;
                self.ops.set(self.ops.get() + 1);
            }
        }
        EventResult::Ignored
    }
}

fn flush_logic(ops: std::rc::Rc<Cell<u32>>) -> impl FnMut(&mut FlushState) -> FlushView {
    move |state: &mut FlushState| {
        state.observed.push(state.flushes);
        FlushView { ops: ops.clone() }
    }
}

#[test]
fn a_queued_callback_flushes_and_re_diffs_inside_one_rebuild() {
    let ops = std::rc::Rc::new(Cell::new(0u32));
    let mut root: RenderRoot<FlushState, FlushView> = RenderRoot::new();
    let mut app = flush_logic(ops.clone());
    let mut state = FlushState::default();

    root.rebuild(&mut app, &mut state);
    assert_eq!(state.flushes, 0);
    assert_eq!(
        state.observed,
        vec![0],
        "nothing queued ⇒ exactly one build run, no broadcast"
    );

    // Queue one op — the `NavigatorController::pop_with_result` analog.
    state.observed.clear();
    ops.set(1);
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        state.flushes, 1,
        "the queued callback ran inside this rebuild — no event was dispatched \
         by anyone but the rebuild itself"
    );
    assert_eq!(
        state.observed,
        vec![0, 1],
        "the build closure re-ran after the flush and saw the post-callback state, so \
         the view this frame ships is not the stale pre-flush one"
    );
    assert!(
        !crate::event::take_pending_result_flush(),
        "the mark was consumed; nothing is owed to a later frame"
    );
}

#[test]
fn a_runaway_callback_chain_is_capped_and_deferred_to_the_next_frame() {
    let ops = std::rc::Rc::new(Cell::new(0u32));
    let mut root: RenderRoot<FlushState, FlushView> = RenderRoot::new();
    let mut app = flush_logic(ops.clone());
    // Far more chaining than the cap allows: unbounded, this rebuild would
    // never return. Reaching the assertions below at all is the no-spin proof.
    let mut state = FlushState {
        chain_left: 100,
        ..Default::default()
    };
    root.rebuild(&mut app, &mut state);

    ops.set(1);
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        state.flushes, MAX_PENDING_RESULT_FLUSH_PASSES as u32,
        "exactly the cap's worth of flush passes, then stop"
    );

    // The remainder is owed, not lost: the mark still stands and the next
    // paint asks for the follow-up frame that will finish it.
    root.layout(Size::new(50.0, 50.0));
    let mut scene = RecordingScene::default();
    let outcome = root.paint(&mut scene, FrameTime::ZERO);
    assert!(
        outcome.needs_frame,
        "hitting the cap requests one more frame, so a dirty-driven shell \
         wakes instead of waiting for input"
    );
    assert!(
        !outcome.needs_frame_paced_only,
        "a deferred flush is not a cosmetic loop — the mobile frame gate must \
         not throttle it"
    );

    // That next frame picks up exactly where the capped one left off.
    root.rebuild(&mut app, &mut state);
    assert_eq!(
        state.flushes,
        2 * MAX_PENDING_RESULT_FLUSH_PASSES as u32,
        "the deferred remainder resumed on the following frame"
    );

    // Leave this thread's flag clean for anything else in the binary.
    let _ = crate::event::take_pending_result_flush();
}

/// The redraw-only flush shape: queues a callback like [`FlushView`] above, but its
/// broadcast handler touches no state and only calls [`EventCtx::request_redraw`]
/// (the gesture long-press latch is the shipped instance).
struct RedrawOnlyView {
    ops: std::rc::Rc<Cell<u32>>,
}

struct RedrawOnlyWidget {
    ops: std::rc::Rc<Cell<u32>>,
    queued: bool,
}

impl RedrawOnlyWidget {
    fn drain_ops(&mut self) {
        if self.ops.replace(0) > 0 {
            self.queued = true;
            crate::event::mark_pending_result_flush();
        }
    }
}

impl View<()> for RedrawOnlyView {
    type Element = RedrawOnlyWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> RedrawOnlyWidget {
        let mut widget = RedrawOnlyWidget {
            ops: self.ops.clone(),
            queued: false,
        };
        widget.drain_ops();
        widget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut RedrawOnlyWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.ops = self.ops.clone();
        element.drain_ops();
        // The whole point: the re-diff after the flush reports nothing, so
        // the dispatch's own outcome is the only wake signal there is.
        ChangeFlags::NONE
    }
}

impl crate::widget::Widget for RedrawOnlyWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(10.0, 10.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() && std::mem::take(&mut self.queued) {
            // No `state_mut`, no signal, no view-visible change — a repaint
            // request and nothing else.
            ctx.request_redraw();
        }
        EventResult::Ignored
    }
}

#[test]
fn a_redraw_only_flushed_callback_wakes_both_loop_styles() {
    let ops = std::rc::Rc::new(Cell::new(0u32));
    let mut root: RenderRoot<(), RedrawOnlyView> = RenderRoot::new();
    let mut app = |_state: &mut ()| RedrawOnlyView { ops: ops.clone() };
    let mut state = ();

    // Settle the first build so the assertions below observe only the flush.
    root.rebuild(&mut app, &mut state);
    root.layout(Size::new(50.0, 50.0));
    let mut scene = RecordingScene::default();
    let settled = root.paint(&mut scene, FrameTime::ZERO);
    assert!(!settled.needs_frame, "nothing queued ⇒ the tree is at rest");
    let _ = root.take_change_flags();

    // Queue the redraw-only callback (the gesture long-press latch analog: a
    // prior pass marks, this rebuild flushes).
    ops.set(1);
    let flags = root.rebuild(&mut app, &mut state);
    assert!(
        flags.needs_paint(),
        "the broadcast's `needs_redraw` folds into the rebuild's flags even \
         though the re-diff saw no view change"
    );
    assert!(
        root.has_pending_change_flags(),
        "PAINT reached `pending`, which is the input the mobile frame gate \
         reads to decide the next tick runs at all"
    );

    let outcome = root.paint(&mut scene, FrameTime::ZERO);
    assert!(
        outcome.needs_frame,
        "the same wake surfaces as `needs_frame`, which is how the desktop \
         `ControlFlow::Wait` loop schedules a frame with no input pending"
    );
    assert!(
        !outcome.needs_frame_paced_only,
        "a flushed callback's repaint is not a cosmetic loop — the mobile \
         frame gate must not throttle it"
    );
    assert!(
        !crate::event::take_pending_result_flush(),
        "the mark was consumed; nothing is owed to a later frame"
    );

    // And it settles: the next frame asks for nothing, so neither loop spins.
    let _ = root.take_change_flags();
    root.rebuild(&mut app, &mut state);
    let settled = root.paint(&mut scene, FrameTime::ZERO);
    assert!(
        !settled.needs_frame,
        "one wake, not a perpetual one — the flush is over"
    );
    assert!(!root.has_pending_change_flags());
}
