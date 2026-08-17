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
//! # Two honest limits
//!
//! - **The indicator is controlled.** `ScrollWidget`'s position is readable only
//!   through its own `on_scroll` callback (its offset accessors are on a type the
//!   facade does not export), so this widget cannot ask the child where it is:
//!   the app feeds the [`ScrollInfo`] it receives back through
//!   [`position`](ScrollAreaView::position). That is why the callback is a
//!   constructor argument rather than an optional builder — without it the thumb
//!   could never move.
//! - **The thumb is paint-only.** Nothing in the baseline's public surface sets a
//!   scroll offset from outside, so there is no seam a thumb *drag* could write
//!   to; dragging the thumb is therefore not implemented (a press there is an
//!   ordinary drag on the surface underneath). Wheel, trackpad and touch drag all
//!   work, because they are the child's.

use std::rc::Rc;

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, SemanticsCtx, Size, View, Widget, any,
    build_child, rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{ScrollInfo, Theme, scroll_view};

use crate::components::input::FALLBACK;

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

/// A view-held, typed scroll-observation callback.
type OnScroll<State> = Rc<dyn Fn(&mut State, ScrollInfo)>;

/// A declarative shadcn scroll area. See the [module docs](self).
pub struct ScrollAreaView<State: 'static> {
    /// The wrapped [`frust::scroll_view`], already carrying the app's
    /// observation callback (erased once, at construction).
    scroll: AnyView<State>,
    offset: f64,
    max_offset: f64,
}

/// Wrap `child` in a scroll area, reporting every scroll through
/// `on_scroll(state, info)`.
///
/// Feed the reported [`ScrollInfo`] back through
/// [`position`](ScrollAreaView::position) to move the thumb — the indicator is
/// controlled (see the [module docs](self)).
pub fn scroll_area<State: 'static, V, F>(child: V, on_scroll: F) -> ScrollAreaView<State>
where
    V: View<State>,
    F: Fn(&mut State, ScrollInfo) + 'static,
{
    let on_scroll: OnScroll<State> = Rc::new(on_scroll);
    ScrollAreaView {
        scroll: any(
            scroll_view(child).on_scroll(move |state: &mut State, info| on_scroll(state, info))
        ),
        offset: 0.0,
        max_offset: 0.0,
    }
}

impl<State: 'static> ScrollAreaView<State> {
    /// Set the thumb's position: the current `offset` and the surface's
    /// `max_offset`, both straight off the [`ScrollInfo`] the callback reported.
    /// A `max_offset` of zero (nothing to scroll) paints no scrollbar at all,
    /// which is also the default.
    pub fn position(mut self, offset: f64, max_offset: f64) -> Self {
        self.max_offset = max_offset.max(0.0);
        self.offset = offset.clamp(0.0, self.max_offset);
        self
    }
}

/// The retained widget for a [`ScrollAreaView`].
pub struct ScrollAreaWidget {
    scroll: ChildPod,
    offset: f64,
    max_offset: f64,
}

impl<State: 'static> View<State> for ScrollAreaView<State> {
    type Element = ScrollAreaWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ScrollAreaWidget {
        ScrollAreaWidget {
            scroll: build_child(&self.scroll, ctx),
            offset: self.offset,
            max_offset: self.max_offset,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ScrollAreaWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.scroll, &self.scroll, &mut element.scroll, ctx);
        if element.offset != self.offset || element.max_offset != self.max_offset {
            element.offset = self.offset;
            element.max_offset = self.max_offset;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ScrollAreaWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.scroll, &mut element.scroll, ctx);
    }
}

impl ScrollAreaWidget {
    /// The thumb's `(origin, size)` for a `viewport`-sized surface, or `None`
    /// when there is nothing to scroll.
    ///
    /// The thumb is as tall a fraction of the track as the viewport is of the
    /// content (`content = viewport + max_offset`), and slides across the leftover
    /// track in step with the offset — the geometry a browser scrollbar has, which
    /// is what upstream inherits by using one.
    fn thumb(&self, viewport: Size) -> Option<(Point, Size)> {
        if self.max_offset <= 0.0 || viewport.height <= 0.0 {
            return None;
        }
        let track_height = (viewport.height - 2.0 * THUMB_INSET).max(0.0);
        let content = viewport.height + self.max_offset;
        let thumb_height = (track_height * (viewport.height / content))
            .max(MIN_THUMB_HEIGHT.min(track_height))
            .min(track_height);
        let travel = (track_height - thumb_height).max(0.0);
        let progress = (self.offset / self.max_offset).clamp(0.0, 1.0);
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
        // Everything goes to the scroll surface: the thumb is paint-only (see the
        // module docs), so this widget consumes nothing of its own.
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
}
