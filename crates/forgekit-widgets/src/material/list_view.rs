//! The virtualized `ListView` (Phase 6c, PLAN.md D4/D5, task 12):
//! `ListView::builder(item_count, item_extent, |index| -> AnyView)` with a
//! uniform, required `item_extent` (variable-extent lazy layout is deferred).
//!
//! # Windowed materialization at rebuild time (the novel pattern)
//!
//! Child widgets in ForgeKit only ever materialize at [`View::rebuild`] time
//! (there is no lazy layout-time child building). But `rebuild` receives the
//! retained `&mut Self::Element`, so the view can read the widget's *own*
//! scroll offset and cached viewport and materialize only the visible window ±
//! a small buffer. This is unprecedented — no other `View` reads element state
//! during rebuild — hence the module's care.
//!
//! Each frame's [`ListView::rebuild`] recomputes the window
//! `[floor(offset/extent) - BUFFER, ceil((offset+viewport.h)/extent) + BUFFER]`
//! (clamped to `[0, item_count]`) and reconciles the live children to it,
//! **keyed by item index**: an index that stays in the window keeps its live
//! [`ChildPod`] (and therefore all of the child's retained state) — it is
//! *relocated* into the new order and rebuilt in place against its own previous
//! view — while indices leaving the window are torn down and indices entering
//! are built fresh. Because the builder is a pure function of the index, the
//! *previous* frame's view for a surviving index is reconstructed with
//! `prev.builder(i)` and the current one with `self.builder(i)`, so no separate
//! per-window view cache is retained.
//!
//! # Viewport staleness (plan-verify correction a)
//!
//! [`forgekit_core::BuildCtx`] carries no viewport size, so the widget caches
//! `viewport: Size` from the previous layout pass (the exact `ScrollWidget`
//! precedent, which already caches `offset` + `viewport`) and rebuild reads it
//! from the element. One frame of staleness on a constraint change is accepted;
//! the very first `build` materializes a conservative window from a zero
//! viewport. To converge, [`Widget::paint`] requests one more frame whenever the
//! materialized window does not yet cover the now-known viewport — so a
//! stationary list fills its screen within one extra frame and never idles
//! under-materialized.
//!
//! # Scroll machinery (reused, not reinvented)
//!
//! The widget owns its own vertical drag capture / wheel / fling, reusing the
//! `forgekit-core::input` constants + fling math and the spring-during-paint
//! pump exactly like [`crate::ScrollView`] (see `scroll.rs`). During a scroll
//! drag the ListView captures the pointer and, on takeover, cancels any armed
//! child (a `ListItem`'s press) via the structural-change contract — the
//! accepted, Flutter-like tradeoff (PLAN.md D4). Offset changes request a
//! redraw so the next frame's rebuild re-windows; the fling advances the offset
//! at paint and requests a continuation frame, so the shell's next
//! rebuild→layout→paint re-windows as the fling carries the list.
//!
//! Scroll extent is `item_count * item_extent` exactly (no estimation); the
//! offset is clamped to `[0, extent - viewport.height]` with no overscroll.

use std::collections::HashMap;
use std::rc::Rc;

use forgekit_core::accesskit::Role;
use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, FLING_STOP,
    FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent,
    PointerPhase, ScrollDelta, SemanticsCtx, TOUCH_SLOP, VelocityTracker, View, WHEEL_LINE_PX,
    Widget, fling_decay, fling_displacement,
};
use kurbo::{Point, Size};

/// Extra items materialized above and below the visible window, so a small
/// scroll (or a fling's per-frame advance) reveals already-built rows instead of
/// a blank edge before the next rebuild re-windows.
const BUFFER: isize = 2;

/// A declarative, virtualized vertical list. See the [module docs](self).
///
/// `builder` is a pure function of the item index; it is retained (an [`Rc`]) so
/// the previous frame's view for a surviving index can be reconstructed during
/// reconciliation. All rows share the uniform, required `item_extent`.
pub struct ListView<State: 'static> {
    item_count: usize,
    item_extent: f64,
    builder: Rc<dyn Fn(usize) -> AnyView<State>>,
}

impl<State: 'static> ListView<State> {
    /// Create a virtualized list of `item_count` rows, each `item_extent`
    /// logical pixels tall, whose row at `index` is produced by `builder`.
    ///
    /// The builder returns an [`AnyView`] (rows may differ in concrete view
    /// type); spell each row with [`forgekit_core::any`]. Panics if
    /// `item_extent` is not positive (the uniform extent is the virtualization
    /// fast path; a zero/negative extent has no well-defined window).
    pub fn builder(
        item_count: usize,
        item_extent: f64,
        builder: impl Fn(usize) -> AnyView<State> + 'static,
    ) -> Self {
        assert!(
            item_extent > 0.0,
            "ListView item_extent must be positive (uniform extent, PLAN.md D4)"
        );
        Self {
            item_count,
            item_extent,
            builder: Rc::new(builder),
        }
    }
}

/// Create a virtualized [`ListView`] — the free-function spelling of
/// [`ListView::builder`].
pub fn list_view<State: 'static>(
    item_count: usize,
    item_extent: f64,
    builder: impl Fn(usize) -> AnyView<State> + 'static,
) -> ListView<State> {
    ListView::builder(item_count, item_extent, builder)
}

/// The retained widget for a [`ListView`]: the materialized window of children
/// (`children[j]` renders item `keys[j]`), the scroll offset + cached viewport,
/// and the same fling bookkeeping as [`crate::ScrollWidget`].
pub struct ListViewWidget {
    /// The currently materialized rows, parallel to [`ListViewWidget::keys`].
    children: Vec<ChildPod>,
    /// The item index each entry of [`ListViewWidget::children`] renders. Always
    /// a contiguous ascending run (uniform extent → the window is a range).
    keys: Vec<usize>,
    item_count: usize,
    item_extent: f64,
    /// Current scroll offset in `[0, max_offset]` (px scrolled down).
    offset: f64,
    /// Resolved viewport size (this widget's own size), cached from the previous
    /// layout so rebuild can window against it (BuildCtx carries no viewport).
    viewport: Size,
    /// Whether a scroll drag has taken the gesture over (past the slop).
    scrolling: bool,
    /// Whether a `Down` armed an active gesture (mirrors `ScrollWidget`).
    down_active: bool,
    down_start: Point,
    last_drag: Point,
    tracker: VelocityTracker,
    /// Active fling velocity (px/s of offset), or `None` when not flinging.
    fling: Option<f64>,
    /// Last painted frame time, reused as the event-pass timestamp for velocity
    /// tracking (the event pass carries no clock — spec §8).
    last_frame_time: FrameTime,
    /// Last animation frame time for the paint-time fling pump; `None` seeds the
    /// clock (zero-delta) on the first paint after a release.
    last_anim: Option<FrameTime>,
}

impl ListViewWidget {
    fn new(item_count: usize, item_extent: f64) -> Self {
        Self {
            children: Vec::new(),
            keys: Vec::new(),
            item_count,
            item_extent,
            offset: 0.0,
            viewport: Size::ZERO,
            scrolling: false,
            down_active: false,
            down_start: Point::ZERO,
            last_drag: Point::ZERO,
            tracker: VelocityTracker::new(),
            fling: None,
            last_frame_time: FrameTime::ZERO,
            last_anim: None,
        }
    }

    /// The current scroll offset.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// The item indices currently materialized (the visible window ± buffer).
    pub fn window(&self) -> &[usize] {
        &self.keys
    }

    /// The maximum scroll offset (`extent − viewport`, never negative).
    pub fn max_offset(&self) -> f64 {
        (self.item_count as f64 * self.item_extent - self.viewport.height).max(0.0)
    }

    /// Whether a fling animation is in flight.
    pub fn is_flinging(&self) -> bool {
        self.fling.is_some()
    }

    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    fn set_offset(&mut self, value: f64) {
        self.offset = value.clamp(0.0, self.max_offset());
    }

    fn clamp_offset(&mut self) {
        self.set_offset(self.offset);
    }

    /// The `[start, end)` item range that should be materialized for the current
    /// offset + cached viewport, clamped to `[0, item_count]`. With a zero
    /// viewport (first build) this is the conservative initial window.
    fn desired_window(&self) -> (usize, usize) {
        if self.item_count == 0 || self.item_extent <= 0.0 {
            return (0, 0);
        }
        let vh = self.viewport.height;
        let first = (self.offset / self.item_extent).floor() as isize - BUFFER;
        let last = ((self.offset + vh) / self.item_extent).ceil() as isize + BUFFER;
        let start = first.max(0) as usize;
        let end = (last.max(0) as usize).min(self.item_count);
        (start.min(end), end)
    }

    /// Whether the materialized window fully covers `[start, end)`.
    fn window_covers(&self, start: usize, end: usize) -> bool {
        if start >= end {
            return true;
        }
        match (self.keys.first(), self.keys.last()) {
            (Some(&f), Some(&l)) => f <= start && l + 1 >= end,
            _ => false,
        }
    }

    /// Place each materialized row at its content position minus the scroll
    /// offset (row `i` occupies `y ∈ [i*extent, (i+1)*extent)` in content space).
    fn sync_child_origins(&mut self) {
        let extent = self.item_extent;
        let offset = self.offset;
        for (index, pod) in self.keys.iter().zip(self.children.iter_mut()) {
            pod.set_origin(Point::new(0.0, *index as f64 * extent - offset));
        }
    }

    /// Advance an in-flight fling by `dt_ms`, returning whether it is still
    /// animating. Pure and deterministic — the paint-time pump and the tests
    /// both drive it (mirrors [`crate::ScrollWidget::tick`]).
    pub fn tick(&mut self, dt_ms: f64) -> bool {
        let Some(v) = self.fling else {
            return false;
        };
        self.set_offset(self.offset + fling_displacement(v, dt_ms));
        self.sync_child_origins();
        let next_v = fling_decay(v, dt_ms);
        let at_bound = self.offset <= 0.0 || self.offset >= self.max_offset();
        if next_v.abs() < FLING_STOP || at_bound {
            self.fling = None;
            false
        } else {
            self.fling = Some(next_v);
            true
        }
    }

    /// Advance the fling by the delta since the last paint and signal
    /// [`PaintCtx::request_frame`] while it is still running (mirrors
    /// [`crate::ScrollWidget`]'s pump). The continuation frame is what re-runs
    /// the shell's rebuild → the window re-materializes as the fling carries on.
    fn pump_fling(&mut self, ctx: &mut PaintCtx) {
        if self.fling.is_none() {
            self.last_anim = None;
            return;
        }
        let now = ctx.frame_time();
        let dt = match self.last_anim {
            Some(t) => now.saturating_sub(t).as_secs_f64() * 1000.0,
            None => 0.0,
        };
        self.last_anim = Some(now);
        if dt > 0.0 {
            self.tick(dt);
        }
        if self.fling.is_some() {
            ctx.request_frame();
        }
    }

    /// Deliver a synthetic `Cancel` to whichever child holds the capture path,
    /// disarming an armed `ListItem` press when the scroll drag takes over.
    fn cancel_children(&mut self, ctx: &mut EventCtx, pos: Point) {
        let cancel = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Cancel,
            position: pos,
            button: PointerButton::Primary,
        });
        crate::route_event(&mut self.children, ctx, &cancel);
    }

    /// The event body, parameterised on an explicit timestamp so velocity math
    /// is deterministic in tests; [`Widget::event`] supplies the real clock.
    /// Adapted from [`crate::ScrollWidget`], routing to the *window* of children
    /// via [`crate::route_event`] rather than a single child.
    fn event_at(&mut self, ctx: &mut EventCtx, event: &InputEvent, t_ms: f64) -> EventResult {
        match event {
            InputEvent::Key(_) | InputEvent::Ime(_) => {
                crate::route_event(&mut self.children, ctx, event)
            }
            InputEvent::Scroll { delta, .. } => {
                let dy = match delta {
                    ScrollDelta::Lines(_, y) => y * WHEEL_LINE_PX,
                    ScrollDelta::Pixels(_, y) => *y,
                };
                self.fling = None;
                self.set_offset(self.offset + dy);
                self.sync_child_origins();
                ctx.request_redraw();
                EventResult::Handled
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    self.scrolling = false;
                    self.down_active = true;
                    self.fling = None;
                    self.last_anim = None;
                    self.down_start = p.position;
                    self.last_drag = p.position;
                    self.tracker.clear();
                    self.tracker.record(t_ms, p.position.y);
                    ctx.capture_pointer();
                    crate::route_event(&mut self.children, ctx, event);
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if !self.down_active {
                        return crate::route_event(&mut self.children, ctx, event);
                    }
                    self.tracker.record(t_ms, p.position.y);
                    if self.scrolling {
                        let dy = p.position.y - self.last_drag.y;
                        self.last_drag = p.position;
                        self.set_offset(self.offset - dy);
                        self.sync_child_origins();
                        ctx.request_redraw();
                    } else if (p.position.y - self.down_start.y).abs() > TOUCH_SLOP {
                        // Take the gesture over: cancel the armed child, stop
                        // forwarding — the documented window-shift capture-loss
                        // tradeoff's sibling (PLAN.md D4).
                        self.scrolling = true;
                        self.last_drag = p.position;
                        self.cancel_children(ctx, p.position);
                        ctx.request_redraw();
                    } else {
                        crate::route_event(&mut self.children, ctx, event);
                    }
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    if self.scrolling {
                        let finger_v = self.tracker.velocity();
                        if finger_v.abs() > FLING_STOP {
                            self.fling = Some(-finger_v);
                            self.last_anim = None;
                        }
                    } else {
                        crate::route_event(&mut self.children, ctx, event);
                    }
                    self.scrolling = false;
                    self.down_active = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    crate::route_event(&mut self.children, ctx, event);
                    self.scrolling = false;
                    self.down_active = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
        }
    }
}

impl<State: 'static> View<State> for ListView<State> {
    type Element = ListViewWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ListViewWidget {
        let mut widget = ListViewWidget::new(self.item_count, self.item_extent);
        // Conservative initial window from a zero viewport (converges within one
        // extra frame via paint's continuation request — see the module docs).
        let (start, end) = widget.desired_window();
        for index in start..end {
            widget
                .children
                .push(crate::build_child(&(self.builder)(index), ctx));
            widget.keys.push(index);
        }
        widget.sync_child_origins();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ListViewWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.item_count != self.item_count {
            element.item_count = self.item_count;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.item_extent != self.item_extent {
            element.item_extent = self.item_extent;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.clamp_offset();

        let (start, end) = element.desired_window();
        // Move the live window out so surviving indices can be relocated by key.
        let old_keys = std::mem::take(&mut element.keys);
        let old_children = std::mem::take(&mut element.children);
        let mut old: HashMap<usize, ChildPod> = old_keys.into_iter().zip(old_children).collect();

        let mut new_children = Vec::with_capacity(end.saturating_sub(start));
        let mut new_keys = Vec::with_capacity(end.saturating_sub(start));
        let mut structural = false;

        for index in start..end {
            if let Some(mut pod) = old.remove(&index) {
                // Survivor: rebuild in place against its own previous view
                // (reconstructed from the pure builder) — state preserved.
                let prev_view = (prev.builder)(index);
                let next_view = (self.builder)(index);
                flags |= crate::rebuild_child(&prev_view, &next_view, &mut pod, ctx);
                new_children.push(pod);
            } else {
                new_children.push(crate::build_child(&(self.builder)(index), ctx));
                structural = true;
            }
            new_keys.push(index);
        }

        // Indices that left the window are torn down (cancel-if-active inside
        // teardown_child unwinds an armed child).
        for (index, mut pod) in old.drain() {
            crate::teardown_child(&(prev.builder)(index), &mut pod, ctx);
            structural = true;
        }

        element.children = new_children;
        element.keys = new_keys;
        element.sync_child_origins();

        if structural {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ListViewWidget, ctx: &mut BuildCtx<'_>) {
        for (index, pod) in element.keys.iter().zip(element.children.iter_mut()) {
            crate::teardown_child(&(self.builder)(*index), pod, ctx);
        }
    }
}

impl Widget for ListViewWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let vw = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let vh = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            // An unbounded height context: the list is as tall as its content.
            self.item_count as f64 * self.item_extent
        };
        self.viewport = Size::new(vw, vh);
        self.clamp_offset();
        // Uniform extent: every materialized row is exactly `item_extent` tall.
        let child_bc = BoxConstraints::tight(Size::new(vw, self.item_extent));
        for pod in &mut self.children {
            pod.layout_child(ctx, &child_bc);
        }
        self.sync_child_origins();
        bc.constrain(self.viewport)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.last_frame_time = ctx.frame_time();
        self.pump_fling(ctx);
        scene.push_clip(ctx.origin(), ctx.size());
        self.sync_child_origins();
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
        scene.pop_clip();
        // If the (now-known) viewport needs rows the materialized window does not
        // yet hold — first build, a constraint change, or a fling that advanced
        // the offset past the buffer — ask the shell for one more frame so the
        // next rebuild re-windows. Converges without idling under-materialized.
        let (start, end) = self.desired_window();
        if self.item_count > 0 && !self.window_covers(start, end) {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let t = self.event_time_ms();
        self.event_at(ctx, event, t)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A List container exposing its total item count and vertical scroll
        // range; the materialized rows contribute their own child nodes. Per the
        // pull-based seam, only the windowed rows are present — the accessible
        // set matches what is rendered, which is the documented v1 behavior.
        // `position_in_set` is not set on children: the semantics seam threads no
        // item index into `semantics_child`, and the generic builder need not
        // produce `ListItem`s, so the container's `size_of_set` carries the count.
        let max_offset = self.max_offset();
        let count = self.item_count;
        let offset = self.offset;
        ctx.push_container(
            Role::List,
            move |node| {
                node.set_size_of_set(count);
                node.set_scroll_y(offset);
                node.set_scroll_y_min(0.0);
                node.set_scroll_y_max(max_offset);
            },
            |ctx| {
                for pod in &self.children {
                    pod.semantics_child(ctx);
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit_core::{RenderRoot, any};
    use std::any::Any;
    use std::cell::Cell;
    use std::rc::Rc;

    // --- A stateful row fixture: each built widget is stamped with a monotonic
    //     "generation" from a shared counter, so a relocated (state-preserving)
    //     row keeps its stamp while a rebuilt-fresh row gets a new one. ---

    struct GenView {
        gens: Rc<Cell<u64>>,
        seen: Rc<GenLog>,
        index: usize,
    }

    #[derive(Default)]
    struct GenLog {
        // index -> generation, last write wins (proves which pod rendered it).
        entries: std::cell::RefCell<std::collections::HashMap<usize, u64>>,
    }

    struct GenWidget {
        generation: u64,
        seen: Rc<GenLog>,
        index: usize,
    }

    impl View<()> for GenView {
        type Element = GenWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> GenWidget {
            let generation = self.gens.get();
            self.gens.set(generation + 1);
            GenWidget {
                generation,
                seen: self.seen.clone(),
                index: self.index,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut GenWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            // A same-type in-place rebuild keeps the generation (state preserved).
            element.index = self.index;
            element.seen = self.seen.clone();
            ChangeFlags::NONE
        }
    }

    impl Widget for GenWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(bc.max())
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.seen
                .entries
                .borrow_mut()
                .insert(self.index, self.generation);
        }
    }

    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: peniko::Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    fn list_widget(root: &RenderRoot<(), ListView<()>>) -> &ListViewWidget {
        let id = root.root_id().expect("root built");
        (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
            .downcast_ref::<ListViewWidget>()
            .expect("root is a ListViewWidget")
    }

    /// Drive a full rebuild → layout → paint frame at `ms`.
    fn frame(
        root: &mut RenderRoot<(), ListView<()>>,
        logic: &mut impl FnMut(&mut ()) -> ListView<()>,
        state: &mut (),
        window: Size,
        ms: f64,
    ) {
        root.rebuild(logic, state);
        root.layout(window);
        let mut sink = NullScene;
        root.paint(&mut sink, FrameTime::from_nanos((ms * 1_000_000.0) as u64));
    }

    fn ev(phase: PointerPhase, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(10.0, y),
            button: PointerButton::Primary,
        })
    }

    // --- (1) Only the visible window materializes, at multiple offsets. ---

    #[test]
    fn only_the_visible_window_materializes() {
        fn logic(_: &mut ()) -> ListView<()> {
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
        }
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);

        // First frame builds a conservative window; a second converges to the
        // real viewport (200 / 50 = 4 rows + 2*BUFFER).
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        let w = list_widget(&root);
        // window = floor(0/50)-2 .. ceil(200/50)+2 = 0 .. 6
        assert_eq!(w.window(), &[0, 1, 2, 3, 4, 5]);
        assert!(
            w.children.len() < 20,
            "only a handful of the 1000 rows are materialized"
        );
    }

    #[test]
    fn window_shifts_to_the_scrolled_offset() {
        fn logic(_: &mut ()) -> ListView<()> {
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
        }
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // Wheel to offset 500 (10 lines * 40px/line = 400... use a pixel scroll).
        root.event(
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(10.0, 50.0),
                delta: ScrollDelta::Pixels(0.0, 500.0),
            },
        );
        frame(&mut root, &mut logic, &mut state, window, 32.0);

        let w = list_widget(&root);
        assert_eq!(w.offset(), 500.0);
        // window = floor(500/50)-2 .. ceil(700/50)+2 = 8 .. 16
        assert_eq!(w.window(), &[8, 9, 10, 11, 12, 13, 14, 15]);
    }

    // --- (2) A window shift relocates survivors (state preserved). ---

    #[test]
    fn window_shift_relocates_survivors_preserving_state() {
        let gens = Rc::new(Cell::new(0u64));
        let seen = Rc::new(GenLog::default());
        let gens_l = gens.clone();
        let seen_l = seen.clone();
        let mut logic = move |_: &mut ()| -> ListView<()> {
            let gens = gens_l.clone();
            let seen = seen_l.clone();
            list_view(1000, 50.0, move |i| {
                any::<(), _>(GenView {
                    gens: gens.clone(),
                    seen: seen.clone(),
                    index: i,
                })
            })
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // Record the generation stamped on a row that will survive the shift.
        let survivor_gen = seen.entries.borrow()[&3];

        // Nudge the offset down by one row (50px) so the window shifts by one but
        // index 3 stays inside it — via wheel so no child cancel is involved.
        root.event(
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(10.0, 50.0),
                delta: ScrollDelta::Pixels(0.0, 50.0),
            },
        );
        frame(&mut root, &mut logic, &mut state, window, 32.0);

        let entries = seen.entries.borrow();
        assert_eq!(
            entries[&3], survivor_gen,
            "a surviving row keeps its widget (and state): relocated, not rebuilt"
        );
        // window shifted to 0..7; index 6 newly entered → a strictly newer stamp.
        assert!(
            entries[&6] > survivor_gen,
            "a freshly-entered row is built anew (higher generation)"
        );
    }

    // --- (3) A fling advances the window across frames. ---

    #[test]
    fn fling_advances_the_window_across_frames() {
        fn logic(_: &mut ()) -> ListView<()> {
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
        }
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // Drag up past the slop, build velocity, release → a fling downward.
        root.event(&mut state, &ev(PointerPhase::Down, 180.0));
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        root.event(&mut state, &ev(PointerPhase::Move, 120.0)); // takeover
        frame(&mut root, &mut logic, &mut state, window, 48.0);
        root.event(&mut state, &ev(PointerPhase::Move, 60.0)); // build velocity
        root.event(&mut state, &ev(PointerPhase::Up, 60.0)); // release

        assert!(list_widget(&root).is_flinging(), "release starts a fling");
        let start_first = list_widget(&root).window()[0];

        // Pump several frames: paint advances the fling offset, the next rebuild
        // re-windows against it.
        for k in 0..8 {
            frame(
                &mut root,
                &mut logic,
                &mut state,
                window,
                64.0 + 16.0 * k as f64,
            );
        }

        assert!(
            list_widget(&root).window()[0] > start_first,
            "the fling carried the window to higher indices across frames"
        );
    }

    // --- (4) Extent / clamp math. ---

    #[test]
    fn extent_is_exact_and_offset_clamps_with_no_overscroll() {
        let mut w = ListViewWidget::new(100, 40.0);
        w.viewport = Size::new(200.0, 300.0);
        // extent = 100 * 40 = 4000; max_offset = 4000 - 300 = 3700.
        assert_eq!(w.max_offset(), 3700.0);
        w.set_offset(10_000.0);
        assert_eq!(w.offset(), 3700.0, "clamped to the end, no overscroll");
        w.set_offset(-50.0);
        assert_eq!(w.offset(), 0.0, "clamped to the top");

        // A viewport taller than the content pins the offset at zero.
        let mut short = ListViewWidget::new(2, 40.0);
        short.viewport = Size::new(200.0, 300.0);
        assert_eq!(short.max_offset(), 0.0);
    }

    #[test]
    fn empty_list_materializes_nothing() {
        fn logic(_: &mut ()) -> ListView<()> {
            list_view(0, 50.0, |i| any::<(), _>(gen_stub(i)))
        }
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);
        assert!(list_widget(&root).window().is_empty());
    }

    // A stateless stand-in row used by the window/fling/extent tests (no shared
    // generation bookkeeping needed there).
    pub(super) fn gen_stub(_index: usize) -> impl View<()> {
        crate::test_support::leaf(200.0, 50.0)
    }

    // --- (6) Semantics tree over the materialized window. ---

    #[test]
    fn semantics_is_a_list_container_over_the_windowed_rows() {
        use forgekit_core::accesskit::Role;
        use forgekit_text::TextContext;

        fn logic(_s: &mut ()) -> ListView<()> {
            list_view(1000, 56.0, |i| {
                any::<(), _>(super::super::list_item::list_item(format!("Row {i}")))
            })
        }
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(300.0, 200.0);

        // Converge the window (build → real-viewport rebuild), then collect.
        let mut tcx = TextContext::new();
        for _ in 0..2 {
            root.rebuild(&mut logic, &mut state);
            root.layout_with_text(window, &mut tcx as &mut dyn Any);
            let mut sink = NullScene;
            root.paint(&mut sink, FrameTime::ZERO);
        }

        let update = root.semantics();
        let materialized = list_widget(&root).window().len();

        let (_, list) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::List)
            .expect("the ListView contributes a Role::List container node");
        assert_eq!(
            list.size_of_set(),
            Some(1000),
            "the container advertises the full item count, not just the window"
        );
        assert_eq!(
            list.children().len(),
            materialized,
            "only the materialized rows are semantics children of the list"
        );

        // Every materialized row is a ListItem node with its headline announced.
        let list_items = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::ListItem)
            .count();
        assert_eq!(list_items, materialized);
    }
}
