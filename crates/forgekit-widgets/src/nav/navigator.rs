//! The navigator core (Phase 6b, task 02): a retained page stack with imperative
//! push/pop/replace, per-page result callbacks, opaque-page paint culling, and
//! test-pinned capture/focus/IME page-switch semantics. Instant switches only —
//! transitions are task 03.
//!
//! # Shape
//!
//! [`navigator`] is the app-facing view fn (the snake_case spelling, like
//! [`scroll_view`](crate::scroll_view)/[`text_input`](crate::text_input)):
//! `navigator(controller, initial_page_builder)` produces a [`NavigatorView`]
//! whose retained [`NavigatorWidget`] owns a `Vec` of page entries. The
//! **[`NavigatorController`]** is the app-state handle — a cloneable
//! `Rc<RefCell<…>>` the app keeps in its `Component::State`; it *records requested
//! ops* (`push`/`pop`/`replace`), which the widget *applies at rebuild* (never
//! self-mutating mid-event). This mirrors the controlled-component philosophy the
//! interactive widgets follow (see `docs/CODE_STANDARDS.md`, "Controlled
//! components never self-mutate").
//!
//! # Op application is view-driven (at rebuild), not event-driven
//!
//! Structural ops are drained and applied in [`NavigatorView::rebuild`] (a
//! `BuildCtx` pass), *not* inside `NavigatorWidget::event`: building a new page
//! pod ([`crate::build_child`]) and tearing a popped one down
//! ([`crate::teardown_child`]) both need a `BuildCtx`, and a rebuild always runs
//! every frame so a *programmatic* push/pop (from a background task, with no
//! triggering event) still lands. On every stack mutation the widget then applies
//! the explicit page-switch contract the structural-rebuild machinery does not
//! cover for a hand-managed stack: (a) it cancels an in-flight capture on the
//! outgoing top ([`crate::cancel_pod`]'s synthetic-`Cancel`), (b) clears its focus
//! flag, and (c) publishes a *cleared* IME surface on the next paint so the
//! platform keyboard hides deterministically rather than waiting for the lazy
//! event-pass convergence `RenderRoot` otherwise relies on.
//!
//! A [`pop`](NavigatorController::pop_with_result) result destined for a
//! pusher-registered `on_result` callback needs `&mut State` — which a rebuild
//! (`BuildCtx`) does not carry — so the callback is queued at rebuild and flushed
//! at the start of the next [`NavigatorWidget::event`] pass, where the erased
//! app state is in scope. See [`NavigatorController::push_for_result`].
//!
//! # Paint culling (Flutter opaque-route parity)
//!
//! Only the topmost **settled opaque** page (and any transparent pages stacked
//! above it) is laid out and painted; pages fully covered by an opaque page keep
//! their retained widgets (so their state survives) but are neither laid out nor
//! painted while covered. Layout runs unconditionally every frame, so a page
//! revealed by a pop is re-laid-out and correct on the very next frame — the same
//! relayout-every-frame invariant the theme path leans on.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EditingState, EventCtx, EventResult,
    ImeState, InputEvent, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use kurbo::{Point, Size};

/// A page builder: a cheap closure that produces the page's view, re-run every
/// rebuild so a retained page's content still reconciles against live app state
/// (the pod, and thus the page's internal widget state, is preserved across the
/// rebuild — only the view descriptor is rebuilt).
pub type PageBuilder<State> = Rc<dyn Fn() -> AnyView<State>>;

/// A pusher-registered result callback: invoked with `&mut State` when the page it
/// was registered against is popped, carrying the [`PopResult`] the pop supplied.
pub type ResultCallback<State> = Rc<dyn Fn(&mut State, PopResult)>;

/// The value a [`pop`](NavigatorController::pop_with_result) hands back to the
/// pusher's [`ResultCallback`], type-erased so a page can return any `'static`
/// payload (mirroring Flutter's `Navigator.pop(result)` → `push(...).then(...)`).
///
/// Empty by default ([`PopResult::empty`]); recover a typed payload with
/// [`PopResult::take`].
pub struct PopResult(Option<Box<dyn Any>>);

impl PopResult {
    /// A result carrying no payload (a plain back-navigation).
    pub fn empty() -> Self {
        PopResult(None)
    }

    /// A result carrying `value`, recoverable by the pusher with
    /// [`PopResult::take`].
    pub fn of<T: Any>(value: T) -> Self {
        PopResult(Some(Box::new(value)))
    }

    /// Whether this result carries no payload.
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Recover the payload as `T`, consuming the result. `None` if the result was
    /// empty or carries a different concrete type.
    pub fn take<T: Any>(self) -> Option<T> {
        self.0
            .and_then(|boxed| boxed.downcast::<T>().ok())
            .map(|boxed| *boxed)
    }
}

impl std::fmt::Debug for PopResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PopResult")
            .field("has_payload", &self.0.is_some())
            .finish()
    }
}

/// One queued navigation op, recorded by the [`NavigatorController`] and drained
/// (in order) by [`NavigatorView::rebuild`].
enum NavOp<State: 'static> {
    /// Push a new page on top of the stack.
    Push {
        builder: PageBuilder<State>,
        opaque: bool,
        on_result: Option<ResultCallback<State>>,
    },
    /// Pop the top page (never the last/root page), delivering `result` to the
    /// popped page's pusher-registered callback.
    Pop { result: PopResult },
    /// Replace the top page in place.
    Replace {
        builder: PageBuilder<State>,
        opaque: bool,
    },
}

/// The app-state handle to a [`navigator`]: a cloneable op queue an app keeps in
/// its `Component::State` and drives with [`push`](Self::push)/[`pop`](Self::pop)/
/// [`replace`](Self::replace). Every clone shares one queue (`Rc`), so the handle
/// the view carries and the handle event handlers call are the same.
///
/// Ops are *recorded*, not applied — the [`NavigatorWidget`] drains and applies
/// them at its next rebuild (see the [module docs](self)).
pub struct NavigatorController<State: 'static> {
    ops: Rc<RefCell<Vec<NavOp<State>>>>,
}

impl<State: 'static> Clone for NavigatorController<State> {
    fn clone(&self) -> Self {
        Self {
            ops: Rc::clone(&self.ops),
        }
    }
}

impl<State: 'static> Default for NavigatorController<State> {
    fn default() -> Self {
        Self::new()
    }
}

impl<State: 'static> NavigatorController<State> {
    /// A fresh controller with an empty op queue.
    pub fn new() -> Self {
        Self {
            ops: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Push an **opaque** page built by `builder` on top of the stack.
    pub fn push(&self, builder: impl Fn() -> AnyView<State> + 'static) {
        self.enqueue(NavOp::Push {
            builder: Rc::new(builder),
            opaque: true,
            on_result: None,
        });
    }

    /// Push a **transparent** page (e.g. a dialog/overlay) — the page below it
    /// stays visible and painted (see [module docs](self)'s paint culling).
    pub fn push_transparent(&self, builder: impl Fn() -> AnyView<State> + 'static) {
        self.enqueue(NavOp::Push {
            builder: Rc::new(builder),
            opaque: false,
            on_result: None,
        });
    }

    /// Push an opaque page and register `on_result`, invoked with `&mut State`
    /// when *this* page is later popped (carrying the pop's [`PopResult`]).
    ///
    /// The callback is delivered at the start of the [`NavigatorWidget::event`]
    /// pass after the pop's rebuild — the first point after the pop where the
    /// erased app state is in scope (a rebuild carries only a `BuildCtx`).
    pub fn push_for_result(
        &self,
        builder: impl Fn() -> AnyView<State> + 'static,
        on_result: impl Fn(&mut State, PopResult) + 'static,
    ) {
        self.enqueue(NavOp::Push {
            builder: Rc::new(builder),
            opaque: true,
            on_result: Some(Rc::new(on_result)),
        });
    }

    /// Pop the top page with no result payload (a plain back-navigation). A pop
    /// of the last/root page is ignored (a navigator always keeps one page).
    pub fn pop(&self) {
        self.enqueue(NavOp::Pop {
            result: PopResult::empty(),
        });
    }

    /// Pop the top page, handing `result` to its pusher-registered
    /// [`push_for_result`](Self::push_for_result) callback.
    pub fn pop_with_result(&self, result: PopResult) {
        self.enqueue(NavOp::Pop { result });
    }

    /// Replace the top page in place with an opaque page built by `builder`.
    pub fn replace(&self, builder: impl Fn() -> AnyView<State> + 'static) {
        self.enqueue(NavOp::Replace {
            builder: Rc::new(builder),
            opaque: true,
        });
    }

    fn enqueue(&self, op: NavOp<State>) {
        self.ops.borrow_mut().push(op);
    }

    /// Take the queued ops (leaving the queue empty). Called by
    /// [`NavigatorView::rebuild`]/`build`.
    fn drain(&self) -> Vec<NavOp<State>> {
        std::mem::take(&mut *self.ops.borrow_mut())
    }
}

/// A declarative navigator. See the [module docs](self).
pub struct NavigatorView<State: 'static> {
    controller: NavigatorController<State>,
    initial: PageBuilder<State>,
}

/// Build a [`NavigatorView`] driven by `controller`, whose initial (root) page is
/// produced by `initial`. The app-facing entry point (see [module docs](self)).
pub fn navigator<State: 'static>(
    controller: &NavigatorController<State>,
    initial: impl Fn() -> AnyView<State> + 'static,
) -> NavigatorView<State> {
    NavigatorView {
        controller: controller.clone(),
        initial: Rc::new(initial),
    }
}

/// One retained page in the [`NavigatorWidget`]'s stack: its builder (re-run each
/// rebuild), the last view it produced (for reconciliation), the retained child
/// pod, its opacity, and the pusher's result callback (fired when this page pops).
struct PageEntry<State: 'static> {
    builder: PageBuilder<State>,
    view: AnyView<State>,
    pod: ChildPod,
    opaque: bool,
    on_result: Option<ResultCallback<State>>,
}

/// The reserved per-widget transition state (task 03).
///
/// Instant switches only in task 02; this zero-sized slot reserves the widget
/// field so task 03 extends paint/layout (per-page paint offset is already the
/// pod origin) without changing `NavigatorWidget`'s struct shape.
#[derive(Default)]
struct TransitionState;

/// The retained widget for a [`NavigatorView`]: owns the page stack and applies
/// the [`NavigatorController`]'s queued ops at rebuild. See the [module docs](self).
pub struct NavigatorWidget<State: 'static> {
    pages: Vec<PageEntry<State>>,
    /// Pop-result callbacks awaiting `&mut State` — flushed at the start of the
    /// next [`event`](NavigatorWidget::event) pass.
    pending_results: Vec<(ResultCallback<State>, PopResult)>,
    /// Set on every stack mutation; the next paint publishes a cleared IME surface
    /// and clears this, so the platform keyboard hides deterministically.
    needs_ime_clear: bool,
    /// Reserved for task 03; unused in task 02 (see [`TransitionState`]).
    #[allow(dead_code)]
    transition: TransitionState,
}

impl<State: 'static> NavigatorWidget<State> {
    /// The index of the topmost **opaque** page — the bottom of the visible
    /// (laid-out + painted) range. Pages below it are culled. With no opaque page
    /// at all (an all-transparent stack), everything is visible.
    fn base_visible_index(&self) -> usize {
        for i in (0..self.pages.len()).rev() {
            if self.pages[i].opaque {
                return i;
            }
        }
        0
    }

    /// Cancel any in-flight capture and clear the focus flag on the *current* top
    /// page — the page being covered/replaced/popped by a stack mutation.
    ///
    /// Capture unwinds via [`crate::cancel_pod`]'s synthetic `Cancel` (the outgoing
    /// widget's state machine must not fire on a later `Up`); focus is a reflected
    /// pod flag, so clearing it is enough (no widget-internal blur to drive) — the
    /// same asymmetry the container reconcilers document.
    fn cancel_top(&mut self) {
        if let Some(top) = self.pages.last_mut() {
            if top.pod.is_active() {
                crate::cancel_pod(&mut top.pod);
                top.pod.set_active(false);
            }
            if top.pod.is_focused() {
                top.pod.set_focused(false);
            }
        }
    }

    /// Drain and apply the controller's queued ops (structural changes only),
    /// building/tearing down pods through `ctx`. Returns the accumulated dirtiness.
    fn apply_ops(&mut self, ops: Vec<NavOp<State>>, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        for op in ops {
            match op {
                NavOp::Push {
                    builder,
                    opaque,
                    on_result,
                } => {
                    self.cancel_top();
                    let view = builder();
                    let pod = crate::build_child(&view, ctx);
                    self.pages.push(PageEntry {
                        builder,
                        view,
                        pod,
                        opaque,
                        on_result,
                    });
                    self.needs_ime_clear = true;
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                NavOp::Pop { result } => {
                    // A navigator always keeps its root page; a pop of the last
                    // page is a no-op (its result payload is dropped).
                    if self.pages.len() > 1 {
                        self.cancel_top();
                        let mut popped = self.pages.pop().expect("len checked > 1");
                        crate::teardown_child(&popped.view, &mut popped.pod, ctx);
                        if let Some(callback) = popped.on_result.take() {
                            self.pending_results.push((callback, result));
                        }
                        self.needs_ime_clear = true;
                        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                    }
                }
                NavOp::Replace { builder, opaque } => {
                    self.cancel_top();
                    let view = builder();
                    let pod = crate::build_child(&view, ctx);
                    if let Some(top) = self.pages.last_mut() {
                        crate::teardown_child(&top.view, &mut top.pod, ctx);
                        *top = PageEntry {
                            builder,
                            view,
                            pod,
                            opaque,
                            on_result: None,
                        };
                    } else {
                        // Defensive: an empty stack should not occur (build seeds
                        // the root page), but replace-into-empty pushes.
                        self.pages.push(PageEntry {
                            builder,
                            view,
                            pod,
                            opaque,
                            on_result: None,
                        });
                    }
                    self.needs_ime_clear = true;
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }
        flags
    }
}

impl<State: 'static> View<State> for NavigatorView<State> {
    type Element = NavigatorWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> NavigatorWidget<State> {
        let view = (self.initial)();
        let pod = crate::build_child(&view, ctx);
        let mut widget = NavigatorWidget {
            pages: vec![PageEntry {
                builder: self.initial.clone(),
                view,
                pod,
                opaque: true,
                on_result: None,
            }],
            pending_results: Vec::new(),
            needs_ime_clear: false,
            transition: TransitionState,
        };
        // Apply any ops the app queued before the first frame.
        let ops = self.controller.drain();
        if !ops.is_empty() {
            widget.apply_ops(ops, ctx);
        }
        widget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut NavigatorWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        // 1. Structural ops (view-driven): push/pop/replace the retained stack.
        let ops = self.controller.drain();
        if !ops.is_empty() {
            flags |= element.apply_ops(ops, ctx);
        }
        // 2. Reconcile every retained page (including culled ones) by re-running
        //    its builder against live state — the pod, and thus the page's own
        //    widget state, is preserved; only the view descriptor is rebuilt.
        for entry in &mut element.pages {
            let next_view = (entry.builder)();
            flags |= crate::rebuild_child(&entry.view, &next_view, &mut entry.pod, ctx);
            entry.view = next_view;
        }
        flags
    }

    fn teardown(&self, element: &mut NavigatorWidget<State>, ctx: &mut BuildCtx<'_>) {
        for entry in &mut element.pages {
            crate::teardown_child(&entry.view, &mut entry.pod, ctx);
        }
    }
}

impl<State: 'static> Widget for NavigatorWidget<State> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Lay out only the visible range (topmost opaque page + any transparent
        // pages above it); covered pages keep their retained widgets but are not
        // laid out while covered (re-laid-out on the next frame once revealed).
        // Constraints pass through unchanged — a full-screen page returns
        // `bc.max()`; the navigator sizes to the largest visible page.
        let start = self.base_visible_index();
        let mut size = Size::ZERO;
        for entry in &mut self.pages[start..] {
            let child = entry.pod.layout_child(ctx, bc);
            entry.pod.set_origin(Point::ZERO);
            size = Size::new(size.width.max(child.width), size.height.max(child.height));
        }
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Paint the visible range bottom-to-top: only the topmost opaque page (and
        // any transparent pages above it) — every fully-covered page is culled.
        let start = self.base_visible_index();
        for entry in &mut self.pages[start..] {
            entry.pod.paint_child(ctx, scene);
        }
        // Deterministic IME hide after a stack mutation: publish a cleared surface
        // so the platform keyboard drops immediately rather than waiting for the
        // lazy event-pass convergence. `RenderRoot::paint` only accepts this while
        // focus is (still) active — exactly the stale-focus window a switch opens.
        if self.needs_ime_clear {
            ctx.publish_ime_state(cleared_ime_state());
            self.needs_ime_clear = false;
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Flush pop-result callbacks queued at the previous rebuild — this is the
        // first point after a pop where the erased app state is in scope.
        if !self.pending_results.is_empty() {
            let pending = std::mem::take(&mut self.pending_results);
            let state = ctx.state_mut::<State>();
            for (callback, result) in pending {
                callback(state, result);
            }
        }
        // Route to the top page only (capture/focus/blur handled by the shared
        // single-child router).
        if let Some(top) = self.pages.last_mut() {
            crate::route_event_single(&mut top.pod, ctx, event)
        } else {
            EventResult::Ignored
        }
    }
}

/// The cleared/inactive IME surface the navigator publishes after a page switch so
/// the platform keyboard hides deterministically (see [`NavigatorWidget::paint`]).
fn cleared_ime_state() -> ImeState {
    ImeState {
        active: false,
        editing: EditingState {
            text: String::new(),
            selection_base: -1,
            selection_extent: -1,
            composing_base: -1,
            composing_extent: -1,
        },
        caret: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RecordingScene;
    use forgekit_core::{FrameTime, PointerButton, PointerEvent, PointerPhase, RenderRoot, any};
    use kurbo::Rect;
    use std::cell::Cell;

    fn down(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn move_to(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    // --- A leaf with retained internal state (a counter) that reports its value
    //     during paint, so a test can prove the page's widget state survives a
    //     push→pop round-trip (pod retention, not a rebuild-from-scratch). ---

    struct CounterView {
        observed: Rc<Cell<u32>>,
    }
    struct CounterWidget {
        count: u32,
        observed: Rc<Cell<u32>>,
    }
    impl View<()> for CounterView {
        type Element = CounterWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CounterWidget {
            CounterWidget {
                count: 0,
                observed: self.observed.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut CounterWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            // Refresh the observation handle but preserve the retained count.
            element.observed = self.observed.clone();
            ChangeFlags::NONE
        }
    }
    impl Widget for CounterWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.observed.set(self.count);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                self.count += 1;
                ctx.request_redraw();
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    fn counter_page(observed: &Rc<Cell<u32>>) -> AnyView<()> {
        any(CounterView {
            observed: observed.clone(),
        })
    }

    /// A leaf that fills a rect of a fixed size — RecordingScene captures its
    /// (origin, size) so a paint-culling test can tell pages apart by size.
    struct SizedLeaf {
        size: Size,
    }
    impl<S: 'static> View<S> for SizedLeaf {
        type Element = SizedLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> SizedLeafWidget {
            SizedLeafWidget { size: self.size }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut SizedLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    struct SizedLeafWidget {
        size: Size,
    }
    impl Widget for SizedLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
        }
    }
    fn sized_page<S: 'static>(w: f64, h: f64) -> AnyView<S> {
        any(SizedLeaf {
            size: Size::new(w, h),
        })
    }

    // --- Criterion 1: retained per-page widget state across push → pop. ---

    #[test]
    fn push_pop_preserves_page_widget_state() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let observed = Rc::new(Cell::new(0u32));

        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            let obs = observed.clone();
            move |_: &mut ()| {
                navigator(&ctrl, {
                    let obs = obs.clone();
                    move || counter_page(&obs)
                })
            }
        };
        let mut state = ();

        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 0, "fresh counter starts at zero");

        // Tap page A → its retained counter increments to 1.
        root.event(&mut state, &down(5.0, 5.0));
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 1);

        // Push B (opaque): A is culled — not painted, count untouched.
        controller.push(|| sized_page(20.0, 20.0));
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 1, "covered page A is not repainted");

        // Pop B → A is revealed and repainted; its retained count is still 1
        // (proving the pod survived rather than being rebuilt from scratch).
        controller.pop();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 1, "page A's widget state survived push→pop");
    }

    // --- Criterion 2: a pop result reaches the on_result callback with state. ---

    #[derive(Default)]
    struct ResultState {
        received: Option<i32>,
    }

    #[test]
    fn pop_result_reaches_callback_with_state() {
        let controller: NavigatorController<ResultState> = NavigatorController::new();

        let mut root: RenderRoot<ResultState, NavigatorView<ResultState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ResultState| {
                navigator(&ctrl, || {
                    any(SizedLeaf {
                        size: Size::new(10.0, 10.0),
                    })
                })
            }
        };
        let mut state = ResultState::default();

        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Push B, registering a result callback that records into app state.
        controller.push_for_result(
            || {
                any(SizedLeaf {
                    size: Size::new(10.0, 10.0),
                })
            },
            |state: &mut ResultState, result: PopResult| {
                state.received = result.take::<i32>();
            },
        );
        root.rebuild(&mut app, &mut state);

        // Pop B with a payload; the structural pop applies at rebuild and queues
        // the callback.
        controller.pop_with_result(PopResult::of(42i32));
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            state.received, None,
            "callback not yet flushed (no event pass)"
        );

        // The next event pass flushes the queued callback with `&mut State`.
        root.event(&mut state, &move_to(5.0, 5.0));
        assert_eq!(state.received, Some(42));
    }

    // --- Criterion 3: opaque-page paint culling. ---

    fn drive_paint(root: &mut RenderRoot<(), NavigatorView<()>>) -> Vec<(Point, Size)> {
        root.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);
        scene.rects
    }

    #[test]
    fn opaque_top_culls_pages_below() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);

        // Only the root page paints.
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(10.0, 10.0))]
        );

        // Push an opaque page B (20x20): only B paints, A is culled.
        controller.push(|| sized_page(20.0, 20.0));
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(20.0, 20.0))]
        );
    }

    #[test]
    fn transparent_top_paints_page_below() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);

        // A transparent top page B (20x20) over opaque A (10x10): both paint,
        // bottom-to-top (A then B).
        controller.push_transparent(|| sized_page(20.0, 20.0));
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![
                (Point::ZERO, Size::new(10.0, 10.0)),
                (Point::ZERO, Size::new(20.0, 20.0)),
            ]
        );
    }

    // --- Criterion 4a: a captured drag on the top page is cancelled on push. ---

    struct CaptureLeaf {
        cancelled: Rc<Cell<bool>>,
    }
    struct CaptureLeafWidget {
        cancelled: Rc<Cell<bool>>,
    }
    impl View<()> for CaptureLeaf {
        type Element = CaptureLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CaptureLeafWidget {
            CaptureLeafWidget {
                cancelled: self.cancelled.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut CaptureLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.cancelled = self.cancelled.clone();
            ChangeFlags::NONE
        }
    }
    impl Widget for CaptureLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                match p.phase {
                    PointerPhase::Down => {
                        ctx.capture_pointer();
                        return EventResult::Handled;
                    }
                    // A `Cancel` arm must never touch application state (the `()`
                    // tripwire): only record that it fired.
                    PointerPhase::Cancel => {
                        self.cancelled.set(true);
                        return EventResult::Handled;
                    }
                    _ => return EventResult::Handled,
                }
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn push_cancels_captured_drag_on_outgoing_top() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let cancelled = Rc::new(Cell::new(false));
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            let flag = cancelled.clone();
            move |_: &mut ()| {
                let flag = flag.clone();
                navigator(&ctrl, move || {
                    any(CaptureLeaf {
                        cancelled: flag.clone(),
                    })
                })
            }
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Capture a drag on page A.
        root.event(&mut state, &down(5.0, 5.0));
        assert!(root.is_pointer_captured());
        assert!(!cancelled.get());

        // Push B → A's in-flight capture is cancelled (synthetic Cancel, no state
        // access) as it becomes the covered page.
        controller.push(|| {
            any(SizedLeaf {
                size: Size::new(10.0, 10.0),
            })
        });
        root.rebuild(&mut app, &mut state);
        assert!(
            cancelled.get(),
            "the covered page received a synthetic Cancel"
        );
    }

    // --- Criterion 4b: a focused field's IME surface is cleared on push. ---

    struct EditableLeaf;
    struct EditableLeafWidget;
    impl View<()> for EditableLeaf {
        type Element = EditableLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> EditableLeafWidget {
            EditableLeafWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut EditableLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for EditableLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
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
                    caret: Some(Rect::new(0.0, 0.0, 1.0, 12.0)),
                });
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn push_clears_focused_field_ime_surface() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || any(EditableLeaf))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Focus the editable on page A: it publishes an active IME surface.
        root.event(&mut state, &down(5.0, 5.0));
        assert!(root.is_focus_active());
        let ime = root
            .ime_state()
            .expect("focused field published an IME surface");
        assert!(ime.active);
        assert_eq!(ime.editing.text, "abc");

        // Push B programmatically (no blurring tap): the navigator's own switch
        // handling must clear the stale IME surface deterministically at paint.
        controller.push(|| {
            any(SizedLeaf {
                size: Size::new(10.0, 10.0),
            })
        });
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

        let ime = root
            .ime_state()
            .expect("a cleared IME surface is still published");
        assert!(
            !ime.active,
            "the stale active IME surface was cleared, not left stale"
        );
        assert!(ime.editing.text.is_empty());
    }

    // --- Criterion 5 / replace: an example-style stack driven through the facade
    //     API (counter pattern), proving replace swaps the top in place. ---

    #[test]
    fn replace_swaps_top_in_place() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(10.0, 10.0))]
        );

        // Replace the root page with a 30x30 page: still a single page, new size.
        controller.replace(|| sized_page(30.0, 30.0));
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(30.0, 30.0))]
        );

        // Pop is a no-op on a single-page stack (root is never popped).
        controller.pop();
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(30.0, 30.0))]
        );
    }

    #[test]
    fn pop_result_take_recovers_typed_payload() {
        assert_eq!(PopResult::of(7u8).take::<u8>(), Some(7));
        assert_eq!(PopResult::of(7u8).take::<i64>(), None);
        assert!(PopResult::empty().is_empty());
        assert_eq!(PopResult::empty().take::<u8>(), None);
    }
}
