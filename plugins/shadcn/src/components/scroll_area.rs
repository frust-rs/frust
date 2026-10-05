//! `scroll_area`: the baseline scroll surface with shadcn's slim scrollbar
//! painted over it.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/scroll-area.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17): a Radix
//! `Root`/`Viewport` pair plus a `ScrollBar` — `flex touch-none p-px
//! h-full w-2.5 border-l border-l-transparent` — whose thumb is
//! `relative flex-1 rounded-full bg-border`. So: a transparent track, a
//! `border`-colored pill inside a 1px inset, and nothing else.
//!
//! # What rides the baseline
//!
//! Scrolling itself is the framework's own [`frust::scroll_view`]: drag,
//! wheel/trackpad, fling, overscroll rubber-band and the keep-content-clipped
//! viewport are all its behavior, and this widget composes it as its child rather
//! than reimplementing any of it. What it adds is the *indicator*.
//!
//! # A deliberate physics pin, not an inherited default
//!
//! [`scroll_area`] installs [`RubberBand`](frust::RubberBand) on the surface
//! it wraps, not the workspace's platform-adaptive default (bouncing
//! everywhere but Android) every other `frust::scroll_view` call picks up
//! automatically. shadcn/ui is a desktop-first, web-derived design language
//! whose scroll surfaces have never bounced; keeping the flat rubber-band
//! feel here is a deliberate choice for this catalog, not an oversight in
//! the platform-adaptive seam. [`ScrollAreaView::physics`] is the opt-out
//! for a consumer that wants a different physics installed instead.
//!
//! # The indicator, with and without a controller
//!
//! - **No controller: fed, paint-only.** Without [`ScrollAreaView::controller`],
//!   `ScrollWidget`'s position is readable only through its own `on_scroll`
//!   callback (its offset accessors are on a type the facade does not export),
//!   so this widget cannot ask the child where it is: the app feeds the
//!   [`ScrollInfo`] it receives back through
//!   [`position`](ScrollAreaView::position) — why that callback is a
//!   constructor argument rather than an optional builder, since without it the
//!   thumb could never move. Nothing in that path sets a scroll offset from
//!   outside either, so there is no seam a thumb *drag* could write to: dragging
//!   the thumb does nothing (a press there is an ordinary drag on the surface
//!   underneath). Wheel, trackpad and touch drag all work, because they are the
//!   child's.
//! - **A controller: read live, draggable.** [`ScrollAreaView::controller`]
//!   attaches a [`frust::ScrollController`] to the wrapped surface (the same
//!   handle an app can hold onto and drive from anywhere). With one set, the
//!   thumb reads `offset`/`max_offset` straight off the handle every frame
//!   instead — [`position`](ScrollAreaView::position) is accepted but ignored
//!   in that case — and a primary-button press that lands on the thumb drags
//!   it: pointer travel maps to the handle's `jump_to`, at the same
//!   track-to-content ratio the thumb is painted at, and the press is consumed
//!   rather than falling through to the surface underneath (the one behavioral
//!   difference the controller path makes). Wheel, trackpad and touch drag on
//!   the surface itself are unaffected either way.

use std::cell::{Cell, OnceCell};

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, SemanticsCtx, Size, View,
    Widget, any, build_child, rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{
    RubberBand, ScrollController, ScrollInfo, ScrollPhysics, ScrollView, Theme, scroll_view,
};

use crate::components::input::FALLBACK;
use crate::hit::{inside, presses};

/// Scrollbar gutter width: `w-2.5`.
pub const SCROLLBAR_WIDTH: f64 = 10.0;
/// Inset of the thumb inside the gutter: `p-px`.
const THUMB_INSET: f64 = 1.0;
/// Shortest thumb the indicator will paint, so a very long document still leaves
/// something grabbable-looking rather than a hairline.
///
/// Upstream has no floor of its own (the browser's own scrollbar layout provides
/// one); this is the port's stated choice.
const MIN_THUMB_HEIGHT: f64 = 16.0;

/// A declarative shadcn scroll area. See the [module docs](self).
pub struct ScrollAreaView<State: 'static> {
    /// The wrapped [`frust::scroll_view`], not yet erased: [`ScrollAreaView::physics`]
    /// still needs to be able to reach in and replace the installed
    /// [`ScrollPhysics`] before this view is handed to the tree. Every other
    /// single-child wrapper in this catalog erases its child at construction
    /// (nothing downstream ever needs to reach back in); this one can't,
    /// because [`ScrollView::physics`] only takes effect through an owned
    /// `.physics(...)` call on the (still-concrete) [`ScrollView`] itself —
    /// once erased into an [`AnyView`], there is no way back in.
    ///
    /// `Cell`, not a plain field: [`View::build`]/[`View::rebuild`]/
    /// [`View::teardown`] only ever get `&self`, so the one-time move into
    /// [`ScrollAreaView::erased`]'s `any(...)` call has to happen through
    /// interior mutability rather than ownership.
    pending: Cell<Option<ScrollView<State>>>,
    /// The erased form, memoized the first time [`ScrollAreaView::erased`]
    /// runs and reused for the rest of this view's build/rebuild/teardown
    /// lifetime — `rebuild`'s `prev` argument needs its *own* already-erased
    /// form to still be around, which is why this can't just be a local built
    /// fresh inside `build`/`rebuild`.
    scroll: OnceCell<AnyView<State>>,
    offset: f64,
    max_offset: f64,
    /// Set by [`ScrollAreaView::controller`]. With one attached, the thumb
    /// reads its offset/max_offset live instead of the fields above, and
    /// becomes draggable — see the [module docs](self).
    controller: Option<ScrollController>,
}

/// Wrap `child` in a scroll area, reporting every scroll through
/// `on_scroll(state, info)`.
///
/// Defaults to [`RubberBand`] rather than the workspace's platform-adaptive
/// physics — see the [module docs](self)' *A deliberate physics pin* — and
/// feeds the reported [`ScrollInfo`] back through
/// [`position`](ScrollAreaView::position) to move the thumb — the indicator is
/// controlled (see the [module docs](self)).
pub fn scroll_area<State: 'static, V, F>(child: V, on_scroll: F) -> ScrollAreaView<State>
where
    V: View<State>,
    F: Fn(&mut State, ScrollInfo) + 'static,
{
    ScrollAreaView {
        pending: Cell::new(Some(
            scroll_view(child)
                .physics(RubberBand::new())
                .on_scroll(on_scroll),
        )),
        scroll: OnceCell::new(),
        offset: 0.0,
        max_offset: 0.0,
        controller: None,
    }
}

impl<State: 'static> ScrollAreaView<State> {
    /// The wrapped surface, erased and memoized on first access. Safe to call
    /// more than once (idempotent past the first call) and from a different
    /// instance's `rebuild` reading `prev`'s own already-erased form.
    fn erased(&self) -> &AnyView<State> {
        self.scroll.get_or_init(|| {
            let scroll = self.pending.take().expect(
                "a ScrollAreaView is erased at most once, by its own build/rebuild/teardown",
            );
            any(scroll)
        })
    }

    /// Set the thumb's position: the current `offset` and the surface's
    /// `max_offset`, both straight off the [`ScrollInfo`] the callback reported.
    /// A `max_offset` of zero (nothing to scroll) paints no scrollbar at all,
    /// which is also the default.
    pub fn position(mut self, offset: f64, max_offset: f64) -> Self {
        self.max_offset = max_offset.max(0.0);
        self.offset = offset.clamp(0.0, self.max_offset);
        self
    }

    /// Install a different [`ScrollPhysics`] on the wrapped surface, forwarding
    /// to the inner [`frust::scroll_view`] — the opt-out from this catalog's
    /// pinned default (see the [module docs](self)' *A deliberate physics
    /// pin*) for a consumer that wants app content to scroll with something
    /// other than the flat rubber-band feel: a platform-parity physics, a
    /// chained composition, or any other [`ScrollPhysics`] implementation.
    pub fn physics(self, physics: impl ScrollPhysics + 'static) -> Self {
        let scroll = self
            .pending
            .take()
            .expect("physics() runs before this view is ever erased for build/rebuild");
        self.pending.set(Some(scroll.physics(physics)));
        self
    }

    /// Attach `controller` to the wrapped surface, forwarding to
    /// [`ScrollView::controller`] — the same handle an app can hold onto and
    /// drive (`jump_to`/`animate_to`) from outside this view.
    ///
    /// With a controller set, the thumb reads its `offset`/`max_offset` live
    /// every frame instead of the [`position`](Self::position)-fed fields
    /// (which are then accepted but ignored), and a primary-button press on
    /// the thumb drags it through the handle's `jump_to` — see the
    /// [module docs](self).
    pub fn controller(mut self, controller: ScrollController) -> Self {
        let scroll = self
            .pending
            .take()
            .expect("controller() runs before this view is ever erased for build/rebuild");
        self.pending
            .set(Some(scroll.controller(controller.clone())));
        self.controller = Some(controller);
        self
    }
}

/// The retained widget for a [`ScrollAreaView`].
pub struct ScrollAreaWidget {
    scroll: ChildPod,
    offset: f64,
    max_offset: f64,
    controller: Option<ScrollController>,
    /// A thumb drag in progress: the pointer's `y` and the controller's
    /// offset, both as of the `Down` that started it. `None` when not
    /// dragging — always `None` with no controller attached, since that path
    /// never starts one.
    drag: Option<(f64, f64)>,
}

impl<State: 'static> View<State> for ScrollAreaView<State> {
    type Element = ScrollAreaWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ScrollAreaWidget {
        ScrollAreaWidget {
            scroll: build_child(self.erased(), ctx),
            offset: self.offset,
            max_offset: self.max_offset,
            controller: self.controller.clone(),
            drag: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ScrollAreaWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(prev.erased(), self.erased(), &mut element.scroll, ctx);
        if element.offset != self.offset || element.max_offset != self.max_offset {
            element.offset = self.offset;
            element.max_offset = self.max_offset;
            flags |= ChangeFlags::PAINT;
        }
        element.controller = self.controller.clone();
        flags
    }

    fn teardown(&self, element: &mut ScrollAreaWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(self.erased(), &mut element.scroll, ctx);
    }
}

/// `(thumb_height, travel)` for a `viewport_height`-tall gutter holding
/// `max_offset` worth of scrollable overflow: the thumb is as tall a
/// fraction of the track as the viewport is of the content (`content =
/// viewport_height + max_offset`), and `travel` is how far its top edge can
/// slide across the leftover track — the geometry a browser scrollbar has,
/// which is what upstream inherits by using one.
///
/// Shared by [`ScrollAreaWidget::thumb`] (paint) and its thumb-drag mapping
/// (event), so a drag's pointer-to-offset ratio always matches what is on
/// screen.
fn thumb_geometry(viewport_height: f64, max_offset: f64) -> (f64, f64) {
    let track_height = (viewport_height - 2.0 * THUMB_INSET).max(0.0);
    let content = viewport_height + max_offset;
    let thumb_height = (track_height * (viewport_height / content))
        .max(MIN_THUMB_HEIGHT.min(track_height))
        .min(track_height);
    let travel = (track_height - thumb_height).max(0.0);
    (thumb_height, travel)
}

impl ScrollAreaWidget {
    /// The `(offset, max_offset)` the thumb currently tracks: the attached
    /// controller's last published snapshot when
    /// [`ScrollAreaView::controller`] set one, otherwise whatever
    /// [`position`](ScrollAreaView::position) last fed in.
    fn live_offset_max(&self) -> (f64, f64) {
        match &self.controller {
            Some(controller) => (controller.offset(), controller.max_offset()),
            None => (self.offset, self.max_offset),
        }
    }

    /// The thumb's `(origin, size)` for a `viewport`-sized surface, or `None`
    /// when there is nothing to scroll.
    fn thumb(&self, viewport: Size) -> Option<(Point, Size)> {
        let (offset, max_offset) = self.live_offset_max();
        if max_offset <= 0.0 || viewport.height <= 0.0 {
            return None;
        }
        let (thumb_height, travel) = thumb_geometry(viewport.height, max_offset);
        let progress = (offset / max_offset).clamp(0.0, 1.0);
        let width = (SCROLLBAR_WIDTH - 2.0 * THUMB_INSET).max(0.0);
        Some((
            Point::new(
                viewport.width - SCROLLBAR_WIDTH + THUMB_INSET,
                THUMB_INSET + travel * progress,
            ),
            Size::new(width, thumb_height),
        ))
    }
}

impl Widget for ScrollAreaWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // A transparent wrapper: the scroll surface takes the whole box, and the
        // scrollbar is painted *over* it (upstream's is absolutely positioned, so
        // it steals no width from the viewport either).
        let size = self.scroll.layout_child(ctx, bc);
        self.scroll.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (origin, size) = (ctx.origin(), ctx.size());
        let thumb = self.thumb(size);
        let border = {
            let theme = Theme::from_paint_ctx(ctx);
            theme.map_or(FALLBACK.border, |t| t.scheme().outline)
        };
        self.scroll.paint_child(ctx, scene);
        // The track is `border-l-transparent` with no background: only the thumb is
        // ever painted, `rounded-full` in `bg-border`.
        if let Some((thumb_origin, thumb_size)) = thumb {
            scene.fill_rounded_rect(
                origin + thumb_origin.to_vec2(),
                thumb_size,
                thumb_size.width / 2.0,
                border,
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // With a controller attached, a primary-button press/drag on the thumb
        // is this widget's own gesture — captured and consumed rather than
        // forwarded to the scroll surface underneath (see the module docs).
        // With no controller, the thumb is paint-only and this block never
        // starts a drag, so every event still falls straight through below,
        // byte-identical to the pre-controller behavior.
        if let Some(controller) = self.controller.clone()
            && let InputEvent::Pointer(p) = event
        {
            match p.phase {
                PointerPhase::Down => {
                    if presses(p) {
                        let size = ctx.size();
                        if let Some((thumb_origin, thumb_size)) = self.thumb(size) {
                            let local = Point::new(
                                p.position.x - thumb_origin.x,
                                p.position.y - thumb_origin.y,
                            );
                            if inside(local, thumb_size) {
                                let (offset, _) = self.live_offset_max();
                                self.drag = Some((p.position.y, offset));
                                ctx.capture_pointer();
                                ctx.request_redraw();
                                return EventResult::Handled;
                            }
                        }
                    }
                }
                PointerPhase::Move => {
                    if let Some((start_y, start_offset)) = self.drag {
                        let size = ctx.size();
                        let (_, max_offset) = self.live_offset_max();
                        let (_, travel) = thumb_geometry(size.height, max_offset);
                        let dy = p.position.y - start_y;
                        let next_offset = if travel > 0.0 {
                            start_offset + dy * max_offset / travel
                        } else {
                            start_offset
                        };
                        controller.jump_to(next_offset);
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                }
                PointerPhase::Up | PointerPhase::Cancel => {
                    if self.drag.take().is_some() {
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                }
            }
        }
        route_event_single(&mut self.scroll, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // The scroll surface contributes the node; the indicator is decoration.
        self.scroll.semantics_child(ctx);
    }

    visit_children!(scroll);
}

/// The thumb color a themed pass resolves (`bg-border`), exposed for a host that
/// wants to match the indicator elsewhere.
pub fn scrollbar_color(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.border, |t| t.scheme().outline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent, PointerPhase, Role, ScrollDelta};
    use frust::{Brightness, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        rects: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {
            self.rects += 1;
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn push_clip(&mut self, _origin: Point, _size: Size) {}
        fn pop_clip(&mut self) {}
    }

    impl Recorder {
        /// The painted thumb, if any: the only rounded rect this widget emits.
        fn thumb(&self) -> Option<(Point, Size, f64, Color)> {
            self.rrects.first().copied()
        }
    }

    const VIEWPORT: Size = Size::new(200.0, 100.0);
    const CONTENT_H: f64 = 300.0;

    /// A fixed-size content leaf generic over the app state.
    struct Block(Size);

    /// The retained half of [`Block`].
    struct BlockWidget(Size);

    impl<S: 'static> View<S> for Block {
        type Element = BlockWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> BlockWidget {
            BlockWidget(self.0)
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut BlockWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.0 = self.0;
            ChangeFlags::NONE
        }
    }

    impl Widget for BlockWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }

    #[derive(Default)]
    struct AppState {
        info: Option<ScrollInfo>,
    }

    fn view(offset: f64, max_offset: f64) -> ScrollAreaView<AppState> {
        scroll_area(
            Block(Size::new(VIEWPORT.width, CONTENT_H)),
            |s: &mut AppState, info| s.info = Some(info),
        )
        .position(offset, max_offset)
    }

    fn build(v: &ScrollAreaView<AppState>) -> ScrollAreaWidget {
        let mut counter = 0u64;
        View::<AppState>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut ScrollAreaWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(VIEWPORT))
    }

    fn paint(w: &mut ScrollAreaWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        if let Some(theme) = theme {
            let mut ctx = ctx.with_theme(theme as &dyn Any);
            w.paint(&mut ctx, &mut rec);
        } else {
            w.paint(&mut ctx, &mut rec);
        }
        rec
    }

    #[test]
    fn the_wrapper_hands_the_whole_box_to_the_scroll_surface() {
        let mut w = build(&view(0.0, 0.0));
        let size = layout(&mut w);
        assert_eq!(size, VIEWPORT);
        assert_eq!(w.scroll.size(), VIEWPORT, "the surface takes the full box");
        assert_eq!(w.scroll.origin(), Point::ORIGIN);
    }

    #[test]
    fn nothing_to_scroll_paints_no_scrollbar() {
        let mut w = build(&view(0.0, 0.0));
        let size = layout(&mut w);
        let rec = paint(&mut w, size, None);
        assert!(rec.thumb().is_none(), "no track, no thumb");
        assert!(rec.rects > 0, "the content still painted");
    }

    #[test]
    fn the_thumb_is_a_border_colored_pill_in_the_w2_5_gutter() {
        let max_offset = CONTENT_H - VIEWPORT.height;
        let mut w = build(&view(0.0, max_offset));
        let size = layout(&mut w);
        let theme = crate::theme().with_brightness(Brightness::Light);
        let rec = paint(&mut w, size, Some(&theme));
        let (origin, thumb_size, radius, color) = rec.thumb().expect("a thumb");
        assert_eq!(color, theme.scheme().outline, "bg-border");
        assert_eq!(thumb_size.width, SCROLLBAR_WIDTH - 2.0 * THUMB_INSET);
        assert_eq!(radius, thumb_size.width / 2.0, "rounded-full");
        assert_eq!(
            origin.x,
            VIEWPORT.width - SCROLLBAR_WIDTH + THUMB_INSET,
            "inside the p-px gutter at the trailing edge"
        );
        assert_eq!(origin.y, THUMB_INSET, "at the top of the track");
        // Its height is the viewport's share of the content.
        let track = VIEWPORT.height - 2.0 * THUMB_INSET;
        let expected = track * (VIEWPORT.height / CONTENT_H);
        assert!((thumb_size.height - expected).abs() < 1e-9);
    }

    #[test]
    fn the_thumb_slides_with_the_reported_offset() {
        let max_offset = CONTENT_H - VIEWPORT.height;
        let thumb_at = |offset: f64| {
            let mut w = build(&view(offset, max_offset));
            let size = layout(&mut w);
            let rec = paint(&mut w, size, None);
            rec.thumb().expect("thumb")
        };
        let (top, size) = {
            let (origin, size, _, _) = thumb_at(0.0);
            (origin.y, size)
        };
        let middle = thumb_at(max_offset / 2.0).0.y;
        let bottom = thumb_at(max_offset).0.y;
        assert!(top < middle && middle < bottom, "monotonic travel");
        // At the end of the range the thumb's bottom edge sits at the track's.
        assert!((bottom + size.height - (VIEWPORT.height - THUMB_INSET)).abs() < 1e-9);
        // An out-of-range offset is clamped by the builder rather than overshooting.
        let clamped = {
            let mut w = build(&view(max_offset * 4.0, max_offset));
            let size = layout(&mut w);
            paint(&mut w, size, None).thumb().expect("thumb").0.y
        };
        assert_eq!(clamped, bottom);
    }

    #[test]
    fn a_very_long_document_still_gets_a_grabbable_looking_thumb() {
        let mut w = build(&view(0.0, 100_000.0));
        let size = layout(&mut w);
        let rec = paint(&mut w, size, None);
        let (_, thumb_size, _, _) = rec.thumb().expect("thumb");
        assert_eq!(thumb_size.height, MIN_THUMB_HEIGHT);
    }

    #[test]
    fn scrolling_rides_the_baseline_and_reports_through_the_callback() {
        let mut root: RenderRoot<AppState, ScrollAreaView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let mut logic = |state: &mut AppState| {
            let (offset, max) = state
                .info
                .map_or((0.0, 0.0), |info| (info.offset, info.max_offset));
            view(offset, max)
        };
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(VIEWPORT, &mut tcx as &mut dyn Any);

        // A wheel notch goes straight through to the wrapped surface, which
        // reports the new position — the seam the indicator is fed from.
        root.event(
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(50.0, 50.0),
                // A positive line delta moves the offset down the content, which is
                // the baseline's own convention.
                delta: ScrollDelta::Lines(0.0, 3.0),
            },
        );
        let info = state.info.expect("the surface reported a scroll");
        assert!(info.offset > 0.0, "the baseline scrolled: {info:?}");
        assert_eq!(info.max_offset, CONTENT_H - VIEWPORT.height);

        // The next rebuild feeds it back, and the thumb moves off the top.
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(VIEWPORT, &mut tcx as &mut dyn Any);
        let mut rec = Recorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        let (origin, _, _, _) = rec.thumb().expect("thumb");
        assert!(origin.y > THUMB_INSET, "the thumb followed the offset");
    }

    #[test]
    fn a_press_on_the_thumb_is_an_ordinary_drag_on_the_surface_underneath() {
        // The documented limit: there is no seam to write the baseline's offset, so
        // the thumb is not draggable. A press over it still reaches the surface,
        // which is what keeps the gesture from being swallowed.
        let mut root: RenderRoot<AppState, ScrollAreaView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let mut logic = |_s: &mut AppState| view(0.0, CONTENT_H - VIEWPORT.height);
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(VIEWPORT, &mut tcx as &mut dyn Any);
        let x = VIEWPORT.width - SCROLLBAR_WIDTH / 2.0;
        let outcome = root.event(
            &mut state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(x, 10.0),
                button: PointerButton::Primary,
            }),
        );
        // The scroll surface captures on `Down` (that is how it takes a drag), so
        // the event was handled by the child, not by a thumb.
        assert!(outcome.needs_redraw || state.info.is_none());
        // The first move past the slop only takes the gesture over; the next one
        // scrolls.
        for y in [-60.0, -70.0] {
            root.event(
                &mut state,
                &InputEvent::Pointer(PointerEvent {
                    phase: PointerPhase::Move,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
        }
        let info = state.info.expect("the drag scrolled the surface");
        assert!(info.offset > 0.0, "{info:?}");
    }

    /// A view fed by a [`ScrollController`] instead of
    /// [`ScrollAreaView::position`] — the counterpart to [`view`] above for
    /// the controller-driven path.
    fn controlled_view(controller: &ScrollController) -> ScrollAreaView<AppState> {
        scroll_area(
            Block(Size::new(VIEWPORT.width, CONTENT_H)),
            |s: &mut AppState, info| s.info = Some(info),
        )
        .controller(controller.clone())
    }

    #[test]
    fn a_thumb_drag_moves_the_surface_through_the_controller() {
        let controller = ScrollController::new();
        let mut w = build(&controlled_view(&controller));
        let size = layout(&mut w);

        let (thumb_origin, thumb_size) = w.thumb(size).expect("a thumb before any drag");
        let x = VIEWPORT.width - SCROLLBAR_WIDTH / 2.0;
        let press_y = thumb_origin.y + thumb_size.height / 2.0;

        let mut app = ();
        let outcome = w.event(
            &mut EventCtx::new(&mut app as &mut dyn Any, Point::ZERO, size),
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(x, press_y),
                button: PointerButton::Primary,
            }),
        );
        assert_eq!(
            outcome,
            EventResult::Handled,
            "the press on the thumb is captured, not forwarded"
        );

        w.event(
            &mut EventCtx::new(&mut app as &mut dyn Any, Point::ZERO, size),
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Move,
                position: Point::new(x, press_y + 40.0),
                button: PointerButton::Primary,
            }),
        );

        // The drag only records a jump_to; a layout drains it against the
        // laid-out extent and publishes the result back onto the handle.
        layout(&mut w);
        assert!(
            controller.offset() > 0.0,
            "the drag moved the surface through the controller"
        );

        w.event(
            &mut EventCtx::new(&mut app as &mut dyn Any, Point::ZERO, size),
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Up,
                position: Point::new(x, press_y + 40.0),
                button: PointerButton::Primary,
            }),
        );
    }

    #[test]
    fn controller_driven_thumb_geometry_matches_the_position_fed_path_for_the_same_offsets() {
        let max_offset = CONTENT_H - VIEWPORT.height;
        let offset = max_offset / 3.0;

        let positioned = {
            let mut w = build(&view(offset, max_offset));
            let size = layout(&mut w);
            w.thumb(size)
        };

        let controller = ScrollController::new();
        controller.jump_to(offset);
        let via_controller = {
            let mut w = build(&controlled_view(&controller));
            let size = layout(&mut w);
            w.thumb(size)
        };

        assert_eq!(controller.offset(), offset);
        assert_eq!(controller.max_offset(), max_offset);
        assert_eq!(
            positioned, via_controller,
            "the same offset/max_offset paint the same thumb rect either way"
        );
    }

    #[test]
    fn visit_children_and_semantics_both_forward_the_surface() {
        let w = build(&view(0.0, 10.0));
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 1);

        let mut root: RenderRoot<AppState, ScrollAreaView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let mut logic = |_s: &mut AppState| view(0.0, 10.0);
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(VIEWPORT, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::ScrollView),
            "the wrapped surface's own node reaches the tree"
        );
    }

    #[test]
    fn the_thumb_color_helper_matches_what_the_widget_paints() {
        let theme = crate::theme().with_brightness(Brightness::Dark);
        assert_eq!(scrollbar_color(Some(&theme)), theme.scheme().outline);
        assert_eq!(scrollbar_color(None), FALLBACK.border);
    }

    #[test]
    fn scroll_area_defaults_to_the_flat_rubber_band() {
        // `ScrollWidget`'s installed physics is a private, `pub(crate)` field
        // inside `frust-widgets` — unreachable even from this crate's
        // `frust-widgets` dev-dependency, so this asserts the *documented*,
        // distinguishing behavior instead: `RubberBand`'s past-edge drag
        // mapping is a flat `raw * 0.5` scale (`OVERSCROLL_RESISTANCE`),
        // unlike the workspace's depth-aware `Bouncing` default.
        use frust::input::TOUCH_SLOP;

        let mut root: RenderRoot<AppState, ScrollAreaView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let mut logic = |_s: &mut AppState| view(0.0, CONTENT_H - VIEWPORT.height);
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(VIEWPORT, &mut tcx as &mut dyn Any);

        let x = 50.0;
        root.event(
            &mut state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(x, 0.0),
                button: PointerButton::Primary,
            }),
        );
        // Crosses the slop: takes the gesture over, no scroll effect of its
        // own (mirrors `a_press_on_the_thumb_is_an_ordinary_drag_on_the_surface_underneath`'s
        // two-move shape above).
        root.event(
            &mut state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Move,
                position: Point::new(x, TOUCH_SLOP + 1.0),
                button: PointerButton::Primary,
            }),
        );
        // A clean 40px pull further down, past the already-topmost edge.
        root.event(
            &mut state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Move,
                position: Point::new(x, TOUCH_SLOP + 1.0 + 40.0),
                button: PointerButton::Primary,
            }),
        );

        let info = state.info.expect("the drag reported a scroll");
        assert_eq!(
            info.overscroll, -20.0,
            "RubberBand's flat 0.5 resistance: -40px pulled past the top settles at -20px"
        );
    }

    #[test]
    fn scroll_area_physics_pass_through_reaches_the_inner_scroll() {
        use frust::NeverScrollable;
        use frust::input::TOUCH_SLOP;

        let mut root: RenderRoot<AppState, ScrollAreaView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let mut logic = |_s: &mut AppState| {
            scroll_area(
                Block(Size::new(VIEWPORT.width, CONTENT_H)),
                |s: &mut AppState, info| s.info = Some(info),
            )
            .physics(NeverScrollable::new())
            .position(0.0, CONTENT_H - VIEWPORT.height)
        };
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(VIEWPORT, &mut tcx as &mut dyn Any);

        let x = 50.0;
        root.event(
            &mut state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(x, 10.0),
                button: PointerButton::Primary,
            }),
        );
        // Same two-move shape as every other drag test here, well past the
        // slop — `NeverScrollable` refuses the takeover outright, so neither
        // move ever reaches `apply_drag_offset`.
        for y in [10.0 + TOUCH_SLOP + 1.0, 10.0 + TOUCH_SLOP + 41.0] {
            root.event(
                &mut state,
                &InputEvent::Pointer(PointerEvent {
                    phase: PointerPhase::Move,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
        }
        assert!(
            state.info.is_none(),
            "NeverScrollable, installed through the pass-through builder, refused the drag \
             outright: the inner scroll_view never reported a scroll"
        );
    }
}
