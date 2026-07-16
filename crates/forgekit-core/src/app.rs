//! The render root: the object the shell (task 08) drives each frame.
//!
//! It owns the widget [`WidgetTree`] and the previous [`View`], and exposes the
//! three framework passes in Masonry order (the subset relevant to v0):
//!
//! * [`RenderRoot::rebuild`] — run `app_logic`, diff against the previous view,
//!   producing/mutating the retained widget.
//! * [`RenderRoot::layout`] — hand the root widget window-sized constraints and
//!   record the size it returns.
//! * [`RenderRoot::paint`] — emit the root widget's draw commands into a scene.
//!
//! v0 is single-root: `app_logic` returns one `impl View<State>` whose concrete
//! type is fixed, so the root's previous view and element are stored typed.
//! ViewSequence / multiple children are explicitly out of scope (task 08+).

use std::any::Any;

use kurbo::{Point, Size};

use crate::event::{EventCtx, EventOutcome, EventResult, ImeState, InputEvent, PointerPhase};
use crate::layout::BoxConstraints;
use crate::tree::{WidgetPod, WidgetTree};
use crate::view::{BuildCtx, ChangeFlags, View, WidgetId};
use crate::widget::{LayoutCtx, PaintCtx, PaintOutcome, PaintScene};

/// Owns the retained tree and drives the rebuild/layout/paint passes for a
/// single-root application.
///
/// Generic over the application `State` and the concrete root view type `V`
/// returned by `app_logic`.
pub struct RenderRoot<State: 'static, V: View<State>> {
    tree: WidgetTree,
    root_id: Option<WidgetId>,
    /// The previous view, retained to diff against on the next rebuild.
    prev_view: Option<V>,
    /// Monotonic widget-id counter, borrowed by each `BuildCtx`.
    next_id: u64,
    window_size: Size,
    /// Whether a pointer is currently down-and-captured somewhere in the tree.
    /// Set on a `Down` whose dispatch requested capture, cleared on `Up`/`Cancel`.
    /// Root-level mirror of the per-container `active` path bookkeeping.
    pointer_captured: bool,
    /// Whether some widget in the tree currently holds focus. Root-level mirror of
    /// the per-container `focused` path bookkeeping (the focus analog of
    /// `pointer_captured`): set when a dispatch requested focus, cleared on a
    /// release or a blur-on-outside-tap `Down`.
    focus_active: bool,
    /// The IME surface the focused widget last published (via
    /// [`EventCtx::publish_ime_state`]), surfaced to the shell by
    /// [`RenderRoot::ime_state`]. Persists across rebuilds/events until refreshed
    /// by a new publish or cleared on blur.
    ime_state: Option<ImeState>,
    /// Dirtiness accumulated since the last [`RenderRoot::take_change_flags`] —
    /// merged from each rebuild so a shell can decide, in one place, whether a
    /// frame needs layout/paint at all.
    pending: ChangeFlags,
    _state: core::marker::PhantomData<fn(&mut State)>,
}

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Create an empty render root with no widget yet built.
    pub fn new() -> Self {
        Self {
            tree: WidgetTree::new(),
            root_id: None,
            prev_view: None,
            next_id: 0,
            window_size: Size::ZERO,
            pointer_captured: false,
            focus_active: false,
            ime_state: None,
            pending: ChangeFlags::NONE,
            _state: core::marker::PhantomData,
        }
    }

    /// Whether a captured pointer gesture is currently in flight.
    pub fn is_pointer_captured(&self) -> bool {
        self.pointer_captured
    }

    /// Whether some widget in the tree currently holds keyboard/IME focus.
    pub fn is_focus_active(&self) -> bool {
        self.focus_active
    }

    /// The IME surface the focused widget published, for the shell to drive the
    /// platform input method (winit `set_ime_cursor_area`, Android
    /// `updateSelection`, iOS `inputDelegate`). `None` when nothing is focused or
    /// the focused widget publishes no IME surface.
    ///
    /// Written by the focused widget through [`EventCtx::publish_ime_state`] during
    /// the event pass and refreshed on every event; it survives a rebuild (so the
    /// shell can query it between frames) and is cleared when focus is lost.
    pub fn ime_state(&self) -> Option<ImeState> {
        self.ime_state.clone()
    }

    /// Take (and clear) the dirtiness accumulated since the last call.
    ///
    /// A shell can consult this to skip the layout/paint passes when nothing has
    /// changed and no redraw was requested (a desktop optimisation; the mobile
    /// continuous-loop shells may ignore it and repaint every tick). Each
    /// [`RenderRoot::rebuild`] merges its result here; this drains it.
    pub fn take_change_flags(&mut self) -> ChangeFlags {
        let flags = self.pending;
        self.pending = ChangeFlags::NONE;
        flags
    }

    /// The root widget id, once built.
    pub fn root_id(&self) -> Option<WidgetId> {
        self.root_id
    }

    /// Shared access to the retained tree (for the shell / tests).
    pub fn tree(&self) -> &WidgetTree {
        &self.tree
    }

    /// Run `app_logic`, then build (first call) or rebuild (subsequent calls)
    /// the root widget, returning what changed.
    ///
    /// `app_logic` is expected to be cheap and re-entrant (spec §5): it is
    /// re-run in full every rebuild.
    pub fn rebuild(
        &mut self,
        app_logic: &mut impl FnMut(&mut State) -> V,
        state: &mut State,
    ) -> ChangeFlags {
        let view = app_logic(state);

        let flags = self.rebuild_view(view);
        self.pending |= flags;
        flags
    }

    /// The rebuild body, split out so [`RenderRoot::rebuild`] can accumulate the
    /// result into [`RenderRoot::pending`] in one place.
    fn rebuild_view(&mut self, view: V) -> ChangeFlags {
        match (self.root_id, self.prev_view.take()) {
            // Reconcile against the previous view of the same type.
            (Some(root_id), Some(prev)) => {
                let mut ctx = BuildCtx::new(&mut self.next_id);
                let flags = {
                    let pod = self
                        .tree
                        .pod_mut(root_id)
                        .expect("root pod present when root_id is set");
                    let element = pod
                        .widget_mut()
                        .downcast_mut::<V::Element>()
                        .expect("root widget type matches its originating view");
                    view.rebuild(&prev, element, &mut ctx)
                };
                if let Some(pod) = self.tree.pod_mut(root_id) {
                    pod.merge_flags(flags);
                }
                self.prev_view = Some(view);
                flags
            }
            // First build: materialise the widget and insert it as the root.
            _ => {
                let mut ctx = BuildCtx::new(&mut self.next_id);
                let id = ctx.alloc_id();
                let element = view.build(&mut ctx);
                let pod = WidgetPod::new(id, Box::new(element));
                let root_id = self.tree.insert_root(pod);
                self.root_id = Some(root_id);
                self.prev_view = Some(view);
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
        }
    }

    /// Lay out the root widget against `window_size` and record its geometry.
    ///
    /// The root receives loose constraints (zero up to the window size) and is
    /// placed at the origin. Returns the size the root chose. No shared
    /// resources are threaded in; use [`RenderRoot::layout_with_text`] when the
    /// tree contains text widgets.
    pub fn layout(&mut self, window_size: Size) -> Size {
        let mut ctx = LayoutCtx::new();
        self.layout_with_ctx(window_size, &mut ctx)
    }

    /// Lay out the root widget, threading a shared text-shaping context down to
    /// text widgets (spec §10.3).
    ///
    /// `text_ctx` is the shell-owned `forgekit_text::TextContext`, passed
    /// type-erased so this crate needs no `forgekit-text` dependency. Text
    /// widgets recover it via [`crate::widget::LayoutCtx::text_context`].
    pub fn layout_with_text(&mut self, window_size: Size, text_ctx: &mut dyn Any) -> Size {
        let mut ctx = LayoutCtx::with_text_context(text_ctx);
        self.layout_with_ctx(window_size, &mut ctx)
    }

    /// Shared layout body: hands the root loose window constraints and records
    /// the size it returns.
    fn layout_with_ctx(&mut self, window_size: Size, ctx: &mut LayoutCtx<'_>) -> Size {
        self.window_size = window_size;
        let Some(root_id) = self.root_id else {
            return Size::ZERO;
        };
        let bc = BoxConstraints::loose(window_size);
        let Some(pod) = self.tree.pod_mut(root_id) else {
            return Size::ZERO;
        };
        let size = pod.widget_mut().layout(ctx, &bc);
        pod.set_layout(Point::ZERO, size);
        size
    }

    /// Paint the root widget into `scene`, returning whether the tree wants
    /// another frame to continue an animation.
    ///
    /// A widget whose paint advances animation state (e.g. a scroll fling) signals
    /// [`PaintCtx::request_frame`]; that flag bubbles up through the container
    /// [`ChildPod`](crate::widget::ChildPod)s and out here as
    /// [`PaintOutcome::needs_frame`], which the shell honors by scheduling the next
    /// frame (desktop `window.request_redraw()`; the mobile continuous loops
    /// already do so). Mirrors how [`RenderRoot::event`] surfaces `needs_redraw`.
    pub fn paint(&mut self, scene: &mut dyn PaintScene) -> PaintOutcome {
        let Some(root_id) = self.root_id else {
            return PaintOutcome::default();
        };
        if let Some(pod) = self.tree.pod_mut(root_id) {
            let mut ctx = PaintCtx::new(pod.origin(), pod.size());
            // Seed the root widget's paint-time focus from the cached focus path
            // so a leaf-root editable observes its own focus; deeper focus is
            // threaded per-pod by `ChildPod::paint_child`.
            ctx.set_has_focus(self.focus_active);
            pod.widget_mut().paint(&mut ctx, scene);
            pod.clear_flags();
            // A focused editable republishes its IME surface during paint (which
            // runs after every rebuild), so a controlled change applied by the
            // rebuild — e.g. a submit clearing the field — refreshes the
            // shell-facing `ime_state` that the event pass alone would leave
            // stale. Defense-in-depth against F1: only accept a bubbled publish
            // while focus is actually active. A widget whose pod focus was just
            // cleared by a container-routed blur (but whose internal flag lags
            // one frame) can then never resurrect the `ime_state` the blur
            // cleared — even before it observes the blur via `PaintCtx::has_focus`.
            if self.focus_active
                && let Some(ime) = ctx.take_ime_state()
            {
                self.ime_state = Some(ime);
            }
            PaintOutcome {
                needs_frame: ctx.needs_frame(),
            }
        } else {
            PaintOutcome::default()
        }
    }

    /// Deliver an input event to the widget tree, returning what happened.
    ///
    /// Builds a root [`EventCtx`] over the (type-erased) `state`, dispatches to
    /// the root widget — which routes the event down through its container
    /// children — and folds the result into an [`EventOutcome`]. The outcome's
    /// `needs_redraw` is set whenever a widget consumed the event or explicitly
    /// requested a redraw; the shell turns that into a `window.request_redraw()`.
    ///
    /// Root capture bookkeeping mirrors the per-container `active`-child model: a
    /// `Down` whose dispatch requested capture marks a gesture in flight; `Up`
    /// and `Cancel` release it (never a window-leave — see `research/RESEARCH.md`).
    ///
    /// # Reentrancy
    ///
    /// This pass **never rebuilds or repaints**. Event handlers mutate `state`
    /// synchronously through the context; the shell is expected to run a single
    /// [`RenderRoot::rebuild`] (then layout/paint) *after* the event pass returns,
    /// driven by the outcome. Rebuilding re-entrantly here would invalidate the
    /// widget references the dispatch still holds and turn the event→state→view
    /// feedback into recursion.
    pub fn event(&mut self, state: &mut State, event: &InputEvent) -> EventOutcome {
        let Some(root_id) = self.root_id else {
            return EventOutcome::default();
        };
        let Some(pod) = self.tree.pod_mut(root_id) else {
            return EventOutcome::default();
        };

        let (handled, needs_redraw, captured, focus_req, focus_rel, ime) = {
            let state_any: &mut dyn Any = state;
            let mut ctx = EventCtx::new(state_any, pod.origin(), pod.size());
            // Seed the root widget's focus flag so a leaf-root editable that holds
            // focus can observe `has_focus()`; deeper focus is threaded per-pod.
            ctx.set_has_focus(self.focus_active);
            let result = pod.widget_mut().event(&mut ctx, event);
            let handled = matches!(result, EventResult::Handled);
            (
                handled,
                ctx.needs_redraw() || handled,
                ctx.is_pointer_captured(),
                ctx.is_focus_requested(),
                ctx.is_focus_released(),
                ctx.take_ime_state(),
            )
        };

        // A published IME surface refreshes the stored one (persists past this
        // event, survives rebuild) until a blur clears it below.
        if ime.is_some() {
            self.ime_state = ime;
        }

        // Root-level capture path: a captured `Down` opens a gesture; `Up`/`Cancel`
        // close it. `Move` leaves the flag untouched so it survives the drag.
        //
        // Root-level focus path (the capture mirror): a `Down` that requested
        // focus opens the focus session; a `Down` that did not is a
        // blur-on-outside-tap and closes it (the per-container `focused` flags are
        // cleared by the routing helpers). Key/Ime/Scroll only adjust focus if the
        // dispatch explicitly requested or released it.
        match event {
            InputEvent::Pointer(pointer) => match pointer.phase {
                PointerPhase::Down => {
                    if captured {
                        self.pointer_captured = true;
                    }
                    if focus_req {
                        self.focus_active = true;
                    } else {
                        // Blur: no widget on the tapped path took focus.
                        self.focus_active = false;
                        self.ime_state = None;
                    }
                }
                PointerPhase::Up | PointerPhase::Cancel => self.pointer_captured = false,
                PointerPhase::Move => {}
            },
            InputEvent::Scroll { .. } | InputEvent::Key(_) | InputEvent::Ime(_) => {
                if focus_req {
                    self.focus_active = true;
                }
                if focus_rel {
                    self.focus_active = false;
                    self.ime_state = None;
                }
            }
        }

        EventOutcome {
            handled,
            needs_redraw,
        }
    }
}

impl<State: 'static, V: View<State>> Default for RenderRoot<State, V> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Application state for the tests.
    #[derive(Default)]
    struct AppState {
        label: String,
    }

    /// The retained widget produced by `MockTextView`: stores the current text
    /// and records what it painted.
    struct TextWidget {
        text: String,
    }

    impl crate::widget::Widget for TextWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            // A crude intrinsic size: width proportional to text length.
            let intrinsic = Size::new(self.text.len() as f64 * 8.0, 16.0);
            bc.constrain(intrinsic)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.draw_text(ctx.origin(), &self.text);
        }
    }

    /// The task's `MockTextView`: a real `View` impl living in tests.
    struct MockTextView {
        text: String,
    }

    impl View<AppState> for MockTextView {
        type Element = TextWidget;

        fn build(&self, _ctx: &mut BuildCtx<'_>) -> Self::Element {
            TextWidget {
                text: self.text.clone(),
            }
        }

        fn rebuild(
            &self,
            prev: &Self,
            element: &mut Self::Element,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            if prev.text != self.text {
                element.text = self.text.clone();
                // Text change: same size model would relayout, but the intrinsic
                // width can change, so signal PAINT here and let callers decide.
                ChangeFlags::PAINT
            } else {
                ChangeFlags::NONE
            }
        }
    }

    /// A scene recorder for asserting paint output.
    #[derive(Default)]
    struct RecordingScene {
        texts: Vec<(Point, String)>,
    }
    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {}
        fn draw_text(&mut self, origin: Point, text: &str) {
            self.texts.push((origin, text.to_string()));
        }
    }

    fn app_logic(state: &mut AppState) -> MockTextView {
        MockTextView {
            text: state.label.clone(),
        }
    }

    #[test]
    fn build_inserts_widget_into_arena() {
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "hello".to_string(),
        };
        let flags = root.rebuild(&mut app_logic, &mut state);
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
        root.rebuild(&mut app_logic, &mut state);
        state.label = "b".to_string();
        let flags = root.rebuild(&mut app_logic, &mut state);
        assert_eq!(flags, ChangeFlags::PAINT);
    }

    #[test]
    fn rebuild_unchanged_data_yields_none() {
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "same".to_string(),
        };
        root.rebuild(&mut app_logic, &mut state);
        let flags = root.rebuild(&mut app_logic, &mut state);
        assert_eq!(flags, ChangeFlags::NONE);
    }

    #[test]
    fn layout_stores_size_in_pod() {
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "hi".to_string(),
        };
        root.rebuild(&mut app_logic, &mut state);
        let size = root.layout(Size::new(800.0, 600.0));
        // "hi" -> 2 * 8 = 16 wide, 16 tall, within the window.
        assert_eq!(size, Size::new(16.0, 16.0));
        let id = root.root_id().unwrap();
        let pod = root.tree().pod(id).unwrap();
        assert_eq!(pod.origin(), Point::ZERO);
        assert_eq!(pod.size(), Size::new(16.0, 16.0));
    }

    #[test]
    fn layout_clamps_to_window() {
        let mut root: RenderRoot<AppState, MockTextView> = RenderRoot::new();
        let mut state = AppState {
            label: "wwwwwwwwwww".to_string(), // 10 chars -> 80 wide intrinsic
        };
        root.rebuild(&mut app_logic, &mut state);
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
        root.rebuild(&mut app_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));

        let mut scene = RecordingScene::default();
        root.paint(&mut scene);
        assert_eq!(scene.texts, vec![(Point::ZERO, "one".to_string())]);

        // Change data, rebuild, repaint -> new text.
        state.label = "two".to_string();
        root.rebuild(&mut app_logic, &mut state);
        let mut scene2 = RecordingScene::default();
        root.paint(&mut scene2);
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
        still.rebuild(&mut app_logic, &mut state);
        still.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        assert!(!still.paint(&mut scene).needs_frame);

        // An animating root bubbles request_frame out as PaintOutcome::needs_frame.
        let mut anim: RenderRoot<AppState, FrameView> = RenderRoot::new();
        anim.rebuild(&mut |_s: &mut AppState| FrameView, &mut state);
        anim.layout(Size::new(100.0, 100.0));
        let mut scene2 = RecordingScene::default();
        assert!(anim.paint(&mut scene2).needs_frame);
    }

    // --- Event-pass fixtures: a widget that mutates state on pointer-down. ---

    #[derive(Default)]
    struct ClickState {
        clicks: u32,
    }

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

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(crate::event::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: crate::event::PointerButton::Primary,
        })
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
        root.rebuild(&mut app_logic, &mut state);
        // First build accumulated LAYOUT|PAINT.
        let flags = root.take_change_flags();
        assert!(flags.needs_layout());
        // Draining leaves it empty until the next rebuild.
        assert!(root.take_change_flags().is_empty());
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

    // --- Focus / IME surface fixtures: a root editable that focuses + publishes
    //     an IME surface on a `Down` in its left half, and blurs (no focus) on a
    //     `Down` in its right half. ---

    use crate::event::{EditingState, ImeState};

    struct ImeWidget;
    impl crate::widget::Widget for ImeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(100.0, 100.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut crate::event::EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                if p.phase == PointerPhase::Down && p.position.x < 50.0 {
                    ctx.request_focus();
                    ctx.publish_ime_state(ImeState {
                        active: true,
                        editing: EditingState {
                            text: "abc".to_string(),
                            selection_base: 3,
                            selection_extent: 3,
                            composing_base: -1,
                            composing_extent: -1,
                        },
                        caret: Some(kurbo::Rect::new(0.0, 0.0, 1.0, 12.0)),
                    });
                    return EventResult::Handled;
                }
                if p.phase == PointerPhase::Down {
                    // Right-half tap: a blur (no focus request).
                    return EventResult::Handled;
                }
            }
            EventResult::Ignored
        }
    }

    struct ImeView;
    impl View<ClickState> for ImeView {
        type Element = ImeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ImeWidget {
            ImeWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ImeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    fn ime_logic(_state: &mut ClickState) -> ImeView {
        ImeView
    }

    #[test]
    fn focus_and_ime_state_surface_and_clear_on_blur() {
        let mut root: RenderRoot<ClickState, ImeView> = RenderRoot::new();
        let mut state = ClickState::default();
        root.rebuild(&mut ime_logic, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // No focus / no IME surface initially.
        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());

        // A left-half Down focuses the widget and publishes an IME surface.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.is_focus_active());
        let ime = root
            .ime_state()
            .expect("focused widget published an IME surface");
        assert!(ime.active);
        assert_eq!(ime.editing.text, "abc");

        // The published surface survives a rebuild (shell can query it between
        // frames).
        root.rebuild(&mut ime_logic, &mut state);
        assert!(root.ime_state().is_some());

        // A right-half Down is a blur: focus and the IME surface both clear.
        root.event(&mut state, &pointer(PointerPhase::Down, 80.0, 10.0));
        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());
    }
}
