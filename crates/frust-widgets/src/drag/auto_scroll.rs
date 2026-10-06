//! [`auto_scroll_zone()`]: edge auto-scroll for a scroll surface while a drag
//! is in flight — a transparent wrapper that, when the dragged pointer sits
//! near its top or bottom edge, drives a [`ScrollController`] toward that edge
//! every frame.
//!
//! # The zone
//!
//! The wrapper sits around a scroll surface (a `ScrollView`/`ListView` holding
//! the same [`ScrollController`]) and reads its own window-space bounds at
//! paint — the same way a drop target reports its bounds, since layout carries
//! no window origin. Each band of [`AutoScroll::edge_px`] inside the top and
//! bottom edges is an edge zone. While the coordinator is `Dragging` and the
//! session's pointer is inside one, the controller is jumped toward that edge
//! at a speed proportional to the pointer's depth into the band: zero at the
//! band's inner boundary, [`AutoScroll::max_px_per_s`] at the edge itself (see
//! [`AutoScroll::velocity`]). A pointer outside the wrapper's bounds — above
//! the surface, say — scrolls nothing.
//!
//! # Frame cadence
//!
//! Each paint that finds the pointer in a zone advances the controller by
//! `velocity × dt` with [`ScrollController::jump_to`], clamped to
//! `[0, max_offset]`, and calls [`PaintCtx::request_frame`] for the next step.
//! `dt` is the difference of two [`PaintCtx::frame_time`]s; the first frame in
//! a zone only seeds the clock (a zero-delta frame, the convention the scroll
//! surface's own ballistic pump uses), so time spent outside a zone is never
//! paid back as one large jump. The cadence stops — no further frame request,
//! the clock unseeded — the first paint the pointer is outside every zone, the
//! session is no longer `Dragging`, the controller is detached, or the offset
//! already sits at the edge being scrolled toward.
//!
//! The jump is recorded before the child paints, so the attached surface
//! drains it in that same paint and the drop targets inside it report their
//! moved bounds; after the child has painted, the wrapper calls
//! [`DragCoordinator::resolve_hover`] so a target that scrolled under a
//! stationary pointer becomes the hovered one without waiting for the pointer
//! to move. Both guarantees — the same-frame drain and the band geometry
//! itself — hold only when the zone encloses the scroll surface directly, as
//! [`auto_scroll_zone()`] is documented to be used: a zone wrapping only the
//! surface's *content* (rather than a bound `ScrollView`/`ListView` whose own
//! layout reports the viewport, not the content's full extent) reads that
//! content's full, usually taller, extent at paint instead of the viewport,
//! so its edge bands sit at the content's own edges rather than the
//! viewport's.
//!
//! # Limits
//!
//! * Vertical only: the controller drives a single scroll axis, and the
//!   shipped scroll surfaces scroll vertically.
//! * Window-space bounds are the wrapper's paint origin, so a zone under a
//!   transformed ancestor (a `pan_zoom`) measures an untransformed position,
//!   like every other window-space consumer in this module.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, FrameTime,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Rect, Size};

use super::coordinator::{DragCoordinator, DragState};
use crate::ScrollController;

/// The default depth, in logical px, of each edge zone
/// ([`AutoScroll::edge_px`]).
///
/// **Community-approximate**: wide enough to hit deliberately with a finger
/// and narrow enough not to trigger while hovering the first or last visible
/// row; no platform publishes a drag auto-scroll band.
pub const AUTO_SCROLL_EDGE_PX: f64 = 32.0;

/// The default top speed, in logical px per second, reached at the very edge
/// ([`AutoScroll::max_px_per_s`]).
///
/// **Community-approximate**: fast enough to cross a long list in a few
/// seconds, slow enough that a target can still be aimed at while it moves.
pub const AUTO_SCROLL_MAX_PX_PER_S: f64 = 600.0;

/// How an [`auto_scroll_zone()`] scrolls: the edge band's depth and the speed
/// at the edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoScroll {
    /// Depth of the band inside the top and bottom edges that scrolls, in
    /// logical px (default [`AUTO_SCROLL_EDGE_PX`]). Zero, negative or
    /// non-finite disables auto-scroll.
    pub edge_px: f64,
    /// Speed at the edge itself, in logical px per second (default
    /// [`AUTO_SCROLL_MAX_PX_PER_S`]); it falls off linearly to zero at the
    /// band's inner boundary. Zero, negative or non-finite disables
    /// auto-scroll.
    pub max_px_per_s: f64,
}

impl Default for AutoScroll {
    fn default() -> Self {
        Self {
            edge_px: AUTO_SCROLL_EDGE_PX,
            max_px_per_s: AUTO_SCROLL_MAX_PX_PER_S,
        }
    }
}

impl AutoScroll {
    /// The scroll velocity, in px per second (negative toward the top), for a
    /// pointer at `pointer` over a zone whose window-space bounds are `zone`.
    ///
    /// Zero outside `zone` and outside both edge bands. Inside a band the speed
    /// is `max_px_per_s × depth / edge_px`, `depth` being how far past the
    /// band's inner boundary the pointer is. A zone shorter than two bands has
    /// them overlap; the nearer edge wins.
    pub fn velocity(&self, zone: Rect, pointer: Point) -> f64 {
        let usable = |v: f64| v.is_finite() && v > 0.0;
        if !usable(self.edge_px) || !usable(self.max_px_per_s) || !zone.contains(pointer) {
            return 0.0;
        }
        let from_top = pointer.y - zone.y0;
        let from_bottom = zone.y1 - pointer.y;
        let (distance, direction) = if from_top <= from_bottom {
            (from_top, -1.0)
        } else {
            (from_bottom, 1.0)
        };
        if distance >= self.edge_px {
            return 0.0;
        }
        direction * self.max_px_per_s * (self.edge_px - distance) / self.edge_px
    }
}

/// A declarative auto-scroll wrapper. See the [module docs](self).
pub struct AutoScrollZone<State: 'static> {
    child: AnyView<State>,
    coordinator: DragCoordinator,
    controller: ScrollController,
    config: AutoScroll,
}

/// Wrap `child` — a scroll surface holding `controller` — in an edge
/// auto-scroll zone for sessions on `coordinator`, with the default
/// [`AutoScroll`]. See the [module docs](self).
///
/// ```
/// use frust_widgets::drag::{AutoScroll, DragCoordinator, auto_scroll_zone, drag_target};
/// use frust_widgets::{ScrollController, SizedBox, scroll_view};
///
/// struct App;
///
/// let drag = DragCoordinator::new();
/// let scroll = ScrollController::new();
/// let column = scroll_view(SizedBox::<App>(None, Some(2_000.0))).controller(scroll.clone());
/// let _zone = auto_scroll_zone(
///     drag_target::<u32, App, _>(column, drag.clone()),
///     drag.clone(),
///     scroll.clone(),
/// )
/// .config(AutoScroll {
///     edge_px: 48.0,
///     ..AutoScroll::default()
/// });
/// ```
pub fn auto_scroll_zone<State, V>(
    child: V,
    coordinator: DragCoordinator,
    controller: ScrollController,
) -> AutoScrollZone<State>
where
    State: 'static,
    V: View<State>,
{
    AutoScrollZone {
        child: any(child),
        coordinator,
        controller,
        config: AutoScroll::default(),
    }
}

impl<State: 'static> AutoScrollZone<State> {
    /// The band depth and edge speed (default [`AutoScroll::default`]).
    pub fn config(mut self, config: AutoScroll) -> Self {
        self.config = config;
        self
    }
}

/// The retained widget for an [`AutoScrollZone`].
pub struct AutoScrollZoneWidget {
    child: ChildPod,
    coordinator: DragCoordinator,
    controller: ScrollController,
    config: AutoScroll,
    /// The frame time of the last auto-scroll step, or `None` while not
    /// scrolling — the next step then only seeds it.
    last_step: Option<FrameTime>,
}

impl<State: 'static> View<State> for AutoScrollZone<State> {
    type Element = AutoScrollZoneWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        AutoScrollZoneWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            coordinator: self.coordinator.clone(),
            controller: self.controller.clone(),
            config: self.config,
            last_step: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Read every paint, so adopting the latest values needs no flag.
        element.coordinator = self.coordinator.clone();
        element.controller = self.controller.clone();
        element.config = self.config;
        crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl AutoScrollZoneWidget {
    /// Take this frame's auto-scroll step for a zone at window-space `zone`,
    /// returning whether a jump was recorded. See the [module
    /// docs](self#frame-cadence).
    fn step(&mut self, ctx: &mut PaintCtx, zone: Rect) -> bool {
        let velocity = match self.coordinator.state() {
            DragState::Dragging(session) if self.controller.is_attached() => {
                self.config.velocity(zone, session.pointer)
            }
            _ => 0.0,
        };
        let offset = self.controller.offset();
        let max_offset = self.controller.max_offset();
        let room = (velocity < 0.0 && offset > 0.0) || (velocity > 0.0 && offset < max_offset);
        if !room {
            self.last_step = None;
            return false;
        }
        let now = ctx.frame_time();
        let dt = self
            .last_step
            .map_or(0.0, |last| now.saturating_sub(last).as_secs_f64());
        self.last_step = Some(now);
        ctx.request_frame();
        if dt <= 0.0 {
            return false;
        }
        self.controller
            .jump_to((offset + velocity * dt).clamp(0.0, max_offset));
        true
    }
}

impl Widget for AutoScrollZoneWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let zone = Rect::from_origin_size(ctx.origin(), ctx.size());
        let jumped = self.step(ctx, zone);
        self.child.paint_child(ctx, scene);
        if jumped {
            self.coordinator.resolve_hover();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drag::{DragPhase, DragTargetId, drag_target, draggable};
    use crate::test_support::RecordingScene;
    use crate::{EdgeInsets, Padding, SizedBox, Stack, StackView, scroll_view, stack};
    use frust_core::{PointerButton, PointerEvent, PointerPhase, RenderRoot};

    const WINDOW: Size = Size::new(400.0, 600.0);
    /// The zone's window-space bounds: the scroll viewport, 300 px tall.
    const ZONE: Rect = Rect::new(0.0, 100.0, 400.0, 400.0);
    const FRAME_MS: f64 = 16.0;
    const ROW: f64 = 100.0;

    #[derive(Clone, Copy)]
    struct Cfg {
        content_height: f64,
        /// Fill the content with drop-target rows and a draggable.
        populated: bool,
    }

    struct App {
        cfg: Cfg,
        coordinator: DragCoordinator,
        controller: ScrollController,
    }

    fn logic(state: &mut App) -> StackView<App> {
        let cfg = state.cfg;
        let mut content = vec![any(SizedBox::<App>(
            Some(WINDOW.width),
            Some(cfg.content_height),
        ))];
        if cfg.populated {
            let rows = (cfg.content_height / ROW) as usize;
            for row in 0..rows {
                content.push(any(Padding(
                    EdgeInsets {
                        left: 0.0,
                        top: row as f64 * ROW,
                        right: 0.0,
                        bottom: 0.0,
                    },
                    drag_target::<u32, App, _>(
                        SizedBox::<App>(Some(WINDOW.width), Some(ROW)),
                        state.coordinator.clone(),
                    ),
                )));
            }
            content.push(any(Padding(
                EdgeInsets {
                    left: 10.0,
                    top: 10.0,
                    right: 0.0,
                    bottom: 0.0,
                },
                draggable(
                    SizedBox::<App>(Some(100.0), Some(40.0)),
                    state.coordinator.clone(),
                    |_: &App| 5_u32,
                ),
            )));
        }
        let surface = scroll_view(Stack(content)).controller(state.controller.clone());
        stack().child(Padding(
            EdgeInsets {
                left: 0.0,
                top: ZONE.y0,
                right: 0.0,
                bottom: WINDOW.height - ZONE.y1,
            },
            auto_scroll_zone(surface, state.coordinator.clone(), state.controller.clone()),
        ))
    }

    struct Harness {
        root: RenderRoot<App, StackView<App>>,
        state: App,
        clock_ms: f64,
    }

    impl Harness {
        fn new(cfg: Cfg) -> Self {
            let mut harness = Harness {
                root: RenderRoot::new(),
                state: App {
                    cfg,
                    coordinator: DragCoordinator::new(),
                    controller: ScrollController::new(),
                },
                clock_ms: 0.0,
            };
            harness.frame();
            harness
        }

        /// One whole frame — rebuild, layout, paint — returning whether it
        /// asked for another.
        fn frame(&mut self) -> bool {
            let mut build: fn(&mut App) -> StackView<App> = logic;
            self.root.rebuild(&mut build, &mut self.state);
            self.root.layout(WINDOW);
            self.clock_ms += FRAME_MS;
            let mut scene = RecordingScene::default();
            self.root
                .paint(
                    &mut scene,
                    FrameTime::from_nanos((self.clock_ms * 1_000_000.0) as u64),
                )
                .needs_frame
        }

        /// Start a session straight on the coordinator, pointer at `at`.
        fn drag_at(&mut self, at: Point) {
            let drag = &self.state.coordinator;
            drag.arm(drag.new_source_id(), at);
            drag.begin(0_u8);
            drag.update_pointer(at);
        }

        fn offset(&self) -> f64 {
            self.state.controller.offset()
        }

        fn mouse(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
        }

        fn hovered(&self) -> Option<DragTargetId> {
            self.state
                .coordinator
                .state()
                .session()
                .and_then(|s| s.hovered)
        }
    }

    fn plain(content_height: f64) -> Harness {
        Harness::new(Cfg {
            content_height,
            populated: false,
        })
    }

    fn assert_close(actual: f64, expected: f64, what: &str) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "{what}: {actual} != {expected}"
        );
    }

    #[test]
    fn velocity_is_proportional_to_the_depth_into_an_edge_band() {
        let config = AutoScroll::default();
        let at = |y: f64| config.velocity(ZONE, Point::new(200.0, y));
        assert_eq!(at(250.0), 0.0, "the middle scrolls nothing");
        assert_eq!(at(ZONE.y0 + 32.0), 0.0, "the band's inner boundary");
        assert_eq!(at(ZONE.y1 - 32.0), 0.0);
        assert_close(at(ZONE.y0), -600.0, "the top edge itself");
        assert_close(at(ZONE.y0 + 8.0), -450.0, "a quarter in from the top");
        assert_close(at(ZONE.y1 - 16.0), 300.0, "halfway into the bottom band");
        assert_eq!(at(ZONE.y0 - 1.0), 0.0, "above the zone");
        assert_eq!(at(ZONE.y1 + 1.0), 0.0, "below the zone");
        assert_eq!(
            config.velocity(ZONE, Point::new(-1.0, ZONE.y1 - 1.0)),
            0.0,
            "beside the zone"
        );
        let off = AutoScroll {
            edge_px: 0.0,
            ..config
        };
        assert_eq!(off.velocity(ZONE, Point::new(200.0, ZONE.y0)), 0.0);
        let off = AutoScroll {
            max_px_per_s: f64::NAN,
            ..config
        };
        assert_eq!(off.velocity(ZONE, Point::new(200.0, ZONE.y0)), 0.0);

        // A zone shorter than two bands: the nearer edge wins.
        let short = Rect::new(0.0, 0.0, 100.0, 40.0);
        assert!(config.velocity(short, Point::new(50.0, 10.0)) < 0.0);
        assert!(config.velocity(short, Point::new(50.0, 30.0)) > 0.0);
    }

    #[test]
    fn auto_scroll_advances_the_controller_at_the_expected_rate() {
        let mut h = plain(2_000.0);
        assert_close(h.state.controller.max_offset(), 1_700.0, "max offset");
        // 16 px into the 32 px bottom band: half of 600 px/s.
        h.drag_at(Point::new(200.0, ZONE.y1 - 16.0));
        assert!(h.frame(), "the seeding frame asks for the next");
        assert_eq!(h.offset(), 0.0, "the seeding frame moves nothing");
        for step in 1..=10 {
            assert!(h.frame());
            assert_close(
                h.offset(),
                300.0 * (FRAME_MS / 1_000.0) * step as f64,
                "offset after a 16 ms step",
            );
        }
        assert_eq!(h.state.coordinator.phase(), DragPhase::Dragging);
    }

    #[test]
    fn auto_scroll_stops_at_max_offset_and_at_zero() {
        let mut h = plain(400.0);
        assert_close(h.state.controller.max_offset(), 100.0, "max offset");
        // At the bottom edge: the full 600 px/s, 9.6 px a frame.
        h.drag_at(Point::new(200.0, ZONE.y1 - 0.5));
        let mut frames = 0;
        while h.frame() {
            frames += 1;
            assert!(frames < 40, "auto-scroll never stopped");
        }
        assert_eq!(h.offset(), 100.0, "stopped exactly at max_offset");
        assert!(!h.frame(), "and stays stopped");
        assert_eq!(h.offset(), 100.0);

        // Back up at the top edge, down to zero.
        h.state
            .coordinator
            .update_pointer(Point::new(200.0, ZONE.y0));
        let mut frames = 0;
        while h.frame() {
            frames += 1;
            assert!(frames < 40, "auto-scroll never stopped");
        }
        assert_eq!(h.offset(), 0.0);
    }

    #[test]
    fn auto_scroll_stops_when_the_pointer_leaves_the_band_or_the_session_ends() {
        let mut h = plain(2_000.0);
        h.drag_at(Point::new(200.0, ZONE.y1 - 16.0));
        for _ in 0..4 {
            h.frame();
        }
        let scrolled = h.offset();
        assert!(scrolled > 0.0);

        // Into the middle: no step, no frame request.
        h.state.coordinator.update_pointer(Point::new(200.0, 250.0));
        assert!(!h.frame());
        assert_eq!(h.offset(), scrolled);

        // Outside the zone entirely (below it): nothing either.
        h.state
            .coordinator
            .update_pointer(Point::new(200.0, ZONE.y1 + 20.0));
        assert!(!h.frame());
        assert_eq!(h.offset(), scrolled);

        // Back into the band after a long pause: the clock re-seeds rather
        // than paying the pause back as one jump.
        h.clock_ms += 5_000.0;
        h.state
            .coordinator
            .update_pointer(Point::new(200.0, ZONE.y1 - 16.0));
        assert!(h.frame());
        assert_eq!(h.offset(), scrolled, "the re-entry frame only seeds");
        h.frame();
        assert_close(h.offset(), scrolled + 4.8, "one 16 ms step at 300 px/s");

        // The session ends: the cadence stops with it.
        let at_cancel = h.offset();
        h.state.coordinator.cancel();
        assert!(!h.frame());
        assert_eq!(h.offset(), at_cancel);
    }

    #[test]
    fn a_drag_inside_the_surface_keeps_its_session_and_resolves_rows_that_scroll_under_it() {
        let mut h = Harness::new(Cfg {
            content_height: 1_000.0,
            populated: true,
        });
        let rows = h.state.coordinator.registered_targets();
        assert_eq!(rows.len(), 10, "one target per row, in tree order");

        // Press the draggable (content (10, 10)), begin the drag inside the
        // scroll surface's own slop, then carry it 16 px into the bottom band,
        // over row 2 (content y 284).
        h.mouse(PointerPhase::Down, 30.0, ZONE.y0 + 20.0);
        h.mouse(PointerPhase::Move, 30.0, ZONE.y0 + 28.0);
        assert_eq!(h.state.coordinator.phase(), DragPhase::Dragging);
        h.mouse(PointerPhase::Move, 30.0, ZONE.y1 - 16.0);
        assert_eq!(h.state.coordinator.phase(), DragPhase::Dragging);
        h.frame();
        assert_eq!(h.hovered(), Some(rows[2]));

        // 4.8 px a frame: row 3's top (content y 300) reaches the pointer
        // after 16 px of scroll, without the pointer moving.
        let mut frames = 0;
        while h.offset() < 16.0 {
            h.frame();
            frames += 1;
            assert!(frames < 10, "the surface never scrolled far enough");
        }
        assert_eq!(
            h.state.coordinator.phase(),
            DragPhase::Dragging,
            "scrolling the surface under the drag does not end it"
        );
        assert_eq!(
            h.hovered(),
            Some(rows[3]),
            "the row that scrolled under the pointer is hovered"
        );

        // Releasing there drops on that row.
        h.mouse(PointerPhase::Up, 30.0, ZONE.y1 - 16.0);
        match h.state.coordinator.state() {
            DragState::Dropping { target, .. } => assert_eq!(target, rows[3]),
            other => panic!("expected a drop, got {other:?}"),
        }
    }

    #[test]
    fn resolve_hover_after_the_zone_paints_does_not_flicker_a_later_sibling() {
        use std::cell::RefCell;
        use std::rc::Rc;

        use crate::drag::DragStateChange;

        // A later sibling sharing the coordinator, painted (and so
        // registered) after the zone: a second kanban column, a `Stack`
        // layer above, an overlay. Covers the whole window, including the
        // zone's own bottom edge band.
        struct App {
            coordinator: DragCoordinator,
            controller: ScrollController,
        }

        fn logic(state: &mut App) -> StackView<App> {
            let column = scroll_view(SizedBox::<App>(Some(WINDOW.width), Some(1_000.0)))
                .controller(state.controller.clone());
            stack()
                .child(Padding(
                    EdgeInsets {
                        left: 0.0,
                        top: ZONE.y0,
                        right: 0.0,
                        bottom: WINDOW.height - ZONE.y1,
                    },
                    auto_scroll_zone(
                        drag_target::<u32, App, _>(column, state.coordinator.clone()),
                        state.coordinator.clone(),
                        state.controller.clone(),
                    ),
                ))
                .child(drag_target::<u32, App, _>(
                    SizedBox::<App>(Some(WINDOW.width), Some(WINDOW.height)),
                    state.coordinator.clone(),
                ))
        }

        let mut root: RenderRoot<App, StackView<App>> = RenderRoot::new();
        let mut state = App {
            coordinator: DragCoordinator::new(),
            controller: ScrollController::new(),
        };
        let log: Rc<RefCell<Vec<DragStateChange>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&log);
        let _subscription = state
            .coordinator
            .subscribe(move |change| sink.borrow_mut().push(change));

        let mut build: fn(&mut App) -> StackView<App> = logic;
        let mut clock_ms = 0.0_f64;
        root.rebuild(&mut build, &mut state);
        root.layout(WINDOW);
        clock_ms += FRAME_MS;
        let mut scene = RecordingScene::default();
        root.paint(
            &mut scene,
            FrameTime::from_nanos((clock_ms * 1_000_000.0) as u64),
        );

        let targets = state.coordinator.registered_targets();
        assert_eq!(targets.len(), 2, "the zone's own target plus the sibling");
        let sibling = targets[1];

        // The sibling, registered after the zone's own target, wins the
        // overlap at a point both cover — the pointer sits in the zone's
        // bottom edge band, which the sibling also spans.
        let source = state.coordinator.new_source_id();
        let at = Point::new(200.0, ZONE.y1 - 16.0);
        state.coordinator.arm(source, at);
        state.coordinator.begin(0_u32);
        state.coordinator.update_pointer(at);
        assert_eq!(
            state.coordinator.state().session().and_then(|s| s.hovered),
            Some(sibling),
            "the later-registered sibling wins the overlap"
        );
        log.borrow_mut().clear();

        // Two more frames: the first only seeds the auto-scroll clock, the
        // second actually jumps the surface, which is when the zone's own
        // target repaints (and the zone calls `resolve_hover`) before the
        // sibling — the later Stack child — gets its own turn to repaint
        // this same frame.
        for _ in 0..2 {
            root.rebuild(&mut build, &mut state);
            root.layout(WINDOW);
            clock_ms += FRAME_MS;
            let mut scene = RecordingScene::default();
            root.paint(
                &mut scene,
                FrameTime::from_nanos((clock_ms * 1_000_000.0) as u64),
            );
        }

        assert!(
            log.borrow().is_empty(),
            "the sibling simply has not repainted yet this frame, not culled: {:?}",
            log.borrow()
        );
        assert_eq!(
            state.coordinator.state().session().and_then(|s| s.hovered),
            Some(sibling),
            "the hover must not flicker away from the sibling mid-frame"
        );
    }
}
