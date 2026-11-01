//! [`page_dots`]/[`PageDots`]: a page indicator — one dot per page, the
//! current page's dot drawn as the accent pill and the rest muted.
//!
//! The shape is Glyph's own (Flutter's built-in `TabPageSelector` is bound to
//! a `TabController`, so it is not mirrored). The app passes the page `count`
//! and the current `index`; the widget owns no selection state.
//!
//! # Geometry
//!
//! Every dot keeps a fixed slot, so moving the highlight never shifts a
//! neighbour. The slot pitch is `dot_size + gap`. When `active_width` is wider
//! than `dot_size`, the row reserves the difference (`active_width − dot_size`)
//! once, split evenly across both ends, so the widest pill is never clipped.
//! Intrinsic width is therefore `count × dot_size + (count − 1) × gap` plus that
//! reserved extra; height is `dot_size`. An empty row (`count == 0`) lays out to
//! nothing. An `index` at or past `count` clamps to the last page.
//!
//! # Tap targets
//!
//! With [`PageDots::on_tap`] set, the row is `max(dot_size, 24)` tall so each
//! dot has a 24 px hit band. The paint stays `dot_size` and is centred within
//! that band. Each tap maps to the nearest slot horizontally. A press fires
//! `on_tap(state, index)` on release inside the same slot. Without `on_tap`
//! the dots are decorative and ignore pointer input.
//!
//! # Motion
//!
//! An index change slides the highlight from its painted position to the new
//! slot, using the same duration and overshoot easing as [`crate::tabs`]'s
//! indicator. The frame gate is requested only while the slide runs. Under
//! `theme.motion.reduce_motion` the highlight snaps to the target on the next
//! paint and no frame is requested.
//!
//! # Semantics
//!
//! The row reports a `Role::Status` labelled `"page {index+1} of {count}"`. With
//! `on_tap`, each dot is also a `Role::Button` child labelled `"page n"`,
//! whose bounds are that dot's own hit slot (not the whole row), so an
//! assistive Click activates `on_tap(n - 1)`.

use std::rc::Rc;

use frust::Theme;
use frust::authoring::{
    Action, BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, PointerPhase, Role, SemanticsCtx, View, Widget,
};
use frust::{AnimationController, FrameTime};
use kurbo::{Point, Size};
use peniko::Color;

use crate::press::presses;
use crate::tabs::{INDICATOR_CURVE, INDICATOR_DURATION};

/// Default dot diameter, in logical px.
const DEFAULT_DOT_SIZE: f64 = 6.0;
/// Default gap between adjacent dot slots, in logical px.
const DEFAULT_GAP: f64 = 8.0;
/// Minimum tap-target height when `on_tap` is set, in logical px.
const MIN_HIT_HEIGHT: f64 = 24.0;

/// Unthemed active-dot fallback — the Glyph dark accent (`#ffb627`).
const FALLBACK_ACTIVE: Color = Color::from_rgb8(0xff, 0xb6, 0x27);
/// Unthemed inactive-dot fallback — a muted warm grey.
const FALLBACK_INACTIVE: Color = Color::from_rgb8(0x8c, 0x84, 0x74);

/// Float tolerance for "the target moved".
const EPS: f64 = 1e-9;

/// A view-held, typed tap callback receiving the tapped page index.
type OnTap<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative page indicator. See the [module docs](self).
pub struct PageDots<State: 'static> {
    count: usize,
    index: usize,
    dot_size: f64,
    gap: f64,
    active_width: Option<f64>,
    on_tap: Option<OnTap<State>>,
}

/// Create a page indicator for `count` pages with page `index` current.
/// Decorative until [`PageDots::on_tap`] is set.
pub fn page_dots<State: 'static>(count: usize, index: usize) -> PageDots<State> {
    PageDots {
        count,
        index,
        dot_size: DEFAULT_DOT_SIZE,
        gap: DEFAULT_GAP,
        active_width: None,
        on_tap: None,
    }
}

impl<State: 'static> PageDots<State> {
    /// Fire `on_tap(state, index)` when a dot is tapped (press and release
    /// inside the same dot). Also makes the dots tappable and labels them as
    /// buttons for assistive technology.
    pub fn on_tap<F: Fn(&mut State, usize) + 'static>(mut self, on_tap: F) -> Self {
        self.on_tap = Some(Rc::new(on_tap));
        self
    }

    /// Set the inactive dot diameter (default 6 px).
    pub fn dot_size(mut self, dot_size: f64) -> Self {
        self.dot_size = dot_size;
        self
    }

    /// Set the gap between adjacent dot slots (default 8 px).
    pub fn gap(mut self, gap: f64) -> Self {
        self.gap = gap;
        self
    }

    /// Set the active pill's width (default: `dot_size`, a plain dot). Values
    /// below `dot_size` are raised to it.
    pub fn active_width(mut self, active_width: f64) -> Self {
        self.active_width = Some(active_width);
        self
    }
}

impl<State: 'static> View<State> for PageDots<State> {
    type Element = PageDotsWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> PageDotsWidget {
        let mut widget = PageDotsWidget {
            count: self.count,
            index: self.index,
            dot_size: self.dot_size,
            gap: self.gap,
            active_width: self.resolved_active_width(),
            on_tap: self
                .on_tap
                .as_ref()
                .map(|f| frust::authoring::erase_callback_arg(f)),
            captured: None,
            indicator: new_indicator(),
            anim_from: 0.0,
            anim_to: 0.0,
            displayed: 0.0,
        };
        let target = widget.target_centre();
        widget.displayed = target;
        widget.anim_from = target;
        widget.anim_to = target;
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PageDotsWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let geometry_changed = prev.count != self.count
            || prev.dot_size != self.dot_size
            || prev.gap != self.gap
            || prev.resolved_active_width() != self.resolved_active_width()
            || prev.on_tap.is_some() != self.on_tap.is_some();
        let index_changed = prev.index != self.index;

        element.count = self.count;
        element.index = self.index;
        element.dot_size = self.dot_size;
        element.gap = self.gap;
        element.active_width = self.resolved_active_width();
        element.on_tap = self
            .on_tap
            .as_ref()
            .map(|f| frust::authoring::erase_callback_arg(f));
        if element.on_tap.is_none() {
            element.captured = None;
        }

        let target = element.target_centre();
        let mut flags = ChangeFlags::NONE;
        if geometry_changed {
            // The geometry the highlight was painted in is gone: snap, no slide.
            element.displayed = target;
            element.anim_from = target;
            element.anim_to = target;
            element.indicator = new_indicator();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if (target - element.anim_to).abs() > EPS {
            // Slide from wherever the highlight is painted now.
            element.anim_from = element.displayed;
            element.anim_to = target;
            element.indicator = new_indicator();
            element.indicator.forward();
        }
        if index_changed || geometry_changed {
            // The active index drives the semantics label as well as paint.
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl<State: 'static> PageDots<State> {
    fn resolved_active_width(&self) -> f64 {
        self.active_width
            .unwrap_or(self.dot_size)
            .max(self.dot_size)
    }
}

fn new_indicator() -> AnimationController {
    AnimationController::new(INDICATOR_DURATION).with_curve(INDICATOR_CURVE)
}

/// The retained widget for a [`PageDots`]. See the [module docs](self).
pub struct PageDotsWidget {
    count: usize,
    index: usize,
    dot_size: f64,
    gap: f64,
    /// Resolved (`>= dot_size`) width of the active pill.
    active_width: f64,
    on_tap: Option<frust::authoring::ErasedArgCallback<usize>>,
    /// The slot a press began in, while the pointer is captured.
    captured: Option<usize>,
    /// The highlight slide controller (idle between index changes).
    indicator: AnimationController,
    /// The highlight centre (widget-local x) the current slide starts from.
    anim_from: f64,
    /// The highlight centre (widget-local x) the current slide heads to.
    anim_to: f64,
    /// The highlight centre painted this frame.
    displayed: f64,
}

impl PageDotsWidget {
    /// Extra width the active pill needs beyond a plain dot, reserved once
    /// across both ends.
    fn extra(&self) -> f64 {
        (self.active_width - self.dot_size).max(0.0)
    }

    /// The active page, clamped into `0..count`, or `None` when empty.
    fn active_index(&self) -> Option<usize> {
        (self.count > 0).then(|| self.index.min(self.count - 1))
    }

    /// Widget-local x of slot `i`'s centre.
    fn slot_centre(&self, i: usize) -> f64 {
        self.extra() / 2.0 + self.dot_size / 2.0 + i as f64 * (self.dot_size + self.gap)
    }

    /// Widget-local x of the active highlight's centre (`0.0` when empty).
    fn target_centre(&self) -> f64 {
        self.active_index().map_or(0.0, |i| self.slot_centre(i))
    }

    /// Total intrinsic width.
    fn intrinsic_width(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        self.count as f64 * self.dot_size + (self.count as f64 - 1.0) * self.gap + self.extra()
    }

    /// Row height: the dot, padded to the 24 px hit band when tappable.
    fn height(&self) -> f64 {
        if self.on_tap.is_some() {
            self.dot_size.max(MIN_HIT_HEIGHT)
        } else {
            self.dot_size
        }
    }

    /// The nearest slot under widget-local `x`, or `None` outside the row or
    /// when the pitch is degenerate.
    fn slot_at(&self, x: f64, width: f64) -> Option<usize> {
        let pitch = self.dot_size + self.gap;
        if self.count == 0 || pitch <= 0.0 || x < 0.0 || x > width {
            return None;
        }
        let rel = (x - self.extra() / 2.0 - self.dot_size / 2.0) / pitch;
        let i = rel.round().clamp(0.0, (self.count - 1) as f64);
        Some(i as usize)
    }

    /// Widget-local `(x0, x1)` of slot `i`'s hit span: one pitch wide, centred
    /// on the slot and clamped to the row, so neighbours abut without overlap
    /// (the span `slot_at` maps back to `i`).
    fn slot_span(&self, i: usize, width: f64) -> (f64, f64) {
        let half = (self.dot_size + self.gap) / 2.0;
        let c = self.slot_centre(i);
        let x0 = if i == 0 { 0.0 } else { c - half };
        let x1 = if i + 1 == self.count { width } else { c + half };
        // Clamp both ends so `0 <= x0 <= x1 <= width`; a slot pushed past a
        // row the parent narrowed below its intrinsic width collapses to
        // `x0 == x1`, and the caller omits it.
        let x0 = x0.clamp(0.0, width);
        (x0, x1.clamp(x0, width))
    }

    /// `(active, inactive)` for the current theme.
    fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
        match theme {
            Some(t) => (t.scheme().primary, t.scheme().on_surface_variant),
            None => (FALLBACK_ACTIVE, FALLBACK_INACTIVE),
        }
    }

    /// Advance the highlight slide; `true` while it still runs.
    fn advance_indicator(&mut self, now: FrameTime) -> bool {
        let animating = self.indicator.is_animating() && self.indicator.advance(now);
        self.displayed = if animating {
            self.anim_from + (self.anim_to - self.anim_from) * self.indicator.value()
        } else {
            self.anim_to
        };
        animating
    }

    /// Fill a dot (or pill) of `width` centred on widget-local `centre_x`.
    fn fill_dot(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        cy: f64,
        centre_x: f64,
        width: f64,
        color: Color,
    ) {
        let r = self.dot_size / 2.0;
        scene.fill_rounded_rect(
            Point::new(origin.x + centre_x - width / 2.0, cy - r),
            Size::new(width, self.dot_size),
            r,
            color,
        );
    }
}

impl Widget for PageDotsWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        if self.count == 0 {
            return bc.constrain(Size::ZERO);
        }
        bc.constrain(Size::new(self.intrinsic_width(), self.height()))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.count == 0 {
            return;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let (active, inactive) = Self::resolve_colors(theme);

        let moving = if reduce_motion {
            self.indicator.stop();
            self.displayed = self.anim_to;
            false
        } else {
            let running = self.advance_indicator(ctx.frame_time());
            if running {
                ctx.request_frame();
            }
            running
        };

        let origin = ctx.origin();
        let cy = origin.y + ctx.size().height / 2.0;
        let active_idx = self.active_index().unwrap_or(0);

        for i in 0..self.count {
            let centre = self.slot_centre(i);
            if !moving && i == active_idx {
                self.fill_dot(scene, origin, cy, centre, self.active_width, active);
            } else {
                self.fill_dot(scene, origin, cy, centre, self.dot_size, inactive);
            }
        }
        if moving {
            let centre = self.displayed;
            self.fill_dot(scene, origin, cy, centre, self.active_width, active);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if self.on_tap.is_none() {
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let width = ctx.size().width;
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(slot) = self.slot_at(p.position.x, width) else {
                    return EventResult::Ignored;
                };
                self.captured = Some(slot);
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(captured) = self.captured.take() else {
                    return EventResult::Ignored;
                };
                let fired = self.slot_at(p.position.x, width) == Some(captured);
                if fired && let Some(on_tap) = self.on_tap.as_mut() {
                    on_tap(ctx, captured);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.captured.take().is_none() {
                    return EventResult::Ignored;
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.captured.is_none() {
                    return EventResult::Ignored;
                }
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let Some(active) = self.active_index() else {
            return;
        };
        let status = format!("page {} of {}", active + 1, self.count);
        if self.on_tap.is_some() {
            ctx.push_container(
                Role::Status,
                |node| {
                    node.set_label(status.as_str());
                },
                |ctx| {
                    // `push_node` stamps the whole row; narrow each button to its
                    // own slot (absolute bounds) so a synthesised Click lands
                    // on that slot's centre.
                    let origin = ctx.origin();
                    let size = ctx.size();
                    for i in 0..self.count {
                        let name = format!("page {}", i + 1);
                        let (x0, x1) = self.slot_span(i, size.width);
                        if x1 <= x0 {
                            continue;
                        }
                        ctx.push_node(Role::Button, |node| {
                            node.set_label(name.as_str());
                            node.add_action(Action::Click);
                            node.set_bounds(frust::accesskit::Rect {
                                x0: origin.x + x0,
                                y0: origin.y,
                                x1: origin.x + x1,
                                y1: origin.y + size.height,
                            });
                        });
                    }
                },
            );
        } else {
            ctx.push_node(Role::Status, |node| {
                node.set_label(status.as_str());
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    use frust::authoring::text::TextContext;
    use frust::authoring::{BuildCtx, PointerButton};

    #[derive(Default)]
    struct Recorder {
        /// `(left-top origin, size, radius, colour)` per filled dot.
        dots: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.dots.push((o, s, radius, color));
        }
    }

    fn build<State: 'static>(view: &PageDots<State>) -> PageDotsWidget {
        let mut counter = 0u64;
        <PageDots<State> as View<State>>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut PageDotsWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)))
    }

    fn paint(w: &mut PageDotsWidget, size: Size, theme: Option<&Theme>) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn reduced_theme() -> Theme {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        theme
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn secondary_down(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(x, y),
            button: PointerButton::Secondary,
        })
    }

    /// Left edge of slot `i` for the default 6 px dot and 8 px gap.
    fn default_left(i: usize) -> f64 {
        i as f64 * 14.0
    }

    #[test]
    fn paints_five_dots_with_the_active_one_in_primary() {
        let mut w = build(&page_dots::<()>(5, 2));
        let size = layout(&mut w);
        assert_eq!(size, Size::new(62.0, 6.0));

        let theme = reduced_theme();
        let (rec, _) = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.dots.len(), 5);
        for (i, (o, s, _, color)) in rec.dots.iter().enumerate() {
            assert_eq!(o.x, default_left(i), "dot {i} left edge");
            assert_eq!(s.width, 6.0);
            let expected = if i == 2 {
                theme.scheme().primary
            } else {
                theme.scheme().on_surface_variant
            };
            assert_eq!(color.components[..3], expected.components[..3], "dot {i}");
        }
    }

    #[test]
    fn active_pill_widens_and_reserves_the_extra_width() {
        let mut w = build(&page_dots::<()>(3, 1).active_width(18.0));
        // 3 × 6 + 2 × 8 + (18 − 6) reserved once.
        let size = layout(&mut w);
        assert_eq!(size, Size::new(46.0, 6.0));
        let theme = reduced_theme();
        let (rec, _) = paint(&mut w, size, Some(&theme));
        let pill = &rec.dots[1];
        assert_eq!(pill.1.width, 18.0);
        assert_eq!(
            pill.3.components[..3],
            theme.scheme().primary.components[..3]
        );
        // Slot 1's centre is 6 px (half the reserved extra) + 3 + 14.
        assert_eq!(pill.0.x + pill.1.width / 2.0, 6.0 + 3.0 + 14.0);
    }

    #[test]
    fn index_change_slides_the_highlight_and_settles_without_frames() {
        let prev = page_dots::<()>(5, 0);
        let mut w = build(&prev);
        layout(&mut w);
        let start = w.displayed;
        assert_eq!(start, w.slot_centre(0));

        let next = page_dots::<()>(5, 3);
        let mut counter = 0u64;
        <PageDots<()> as View<()>>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        let target = w.slot_centre(3);
        assert!(w.indicator.is_animating());

        // Mid-slide: strictly between the two slots.
        assert!(w.advance_indicator(ft_ms(0.0)));
        assert!(w.advance_indicator(ft_ms(80.0)));
        let mid = w.displayed;
        assert!(mid > start && mid < target, "mid-slide at {mid}");

        // Past the duration: settled on the target, no longer animating.
        assert!(!w.advance_indicator(ft_ms(500.0)));
        assert_eq!(w.displayed, target);
        assert!(!w.indicator.is_animating());
    }

    #[test]
    fn reduce_motion_snaps_to_the_target_and_requests_no_frame() {
        let prev = page_dots::<()>(5, 0);
        let mut w = build(&prev);
        let size = layout(&mut w);
        let theme = reduced_theme();
        paint(&mut w, size, Some(&theme));

        let next = page_dots::<()>(5, 3);
        let mut counter = 0u64;
        <PageDots<()> as View<()>>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        let (rec, needs_frame) = paint(&mut w, size, Some(&theme));
        assert!(!needs_frame, "reduce_motion must not request a frame");
        assert_eq!(w.displayed, w.slot_centre(3));
        let active = rec
            .dots
            .iter()
            .find(|(_, _, _, c)| c.components[..3] == theme.scheme().primary.components[..3])
            .expect("the active dot is painted in primary");
        assert_eq!(active.0.x, default_left(3));
    }

    #[test]
    fn index_past_the_count_clamps_to_the_last_page() {
        let w = build(&page_dots::<()>(5, 9));
        assert_eq!(w.active_index(), Some(4));
        assert_eq!(w.target_centre(), w.slot_centre(4));
    }

    #[test]
    fn zero_pages_lay_out_empty_and_paint_nothing() {
        let mut w = build(&page_dots::<()>(0, 0));
        assert_eq!(layout(&mut w), Size::ZERO);
        let (rec, needs_frame) = paint(&mut w, Size::ZERO, None);
        assert!(rec.dots.is_empty());
        assert!(!needs_frame);
    }

    #[test]
    fn tap_on_dot_three_fires_on_tap_once() {
        let view = page_dots::<Vec<usize>>(5, 0).on_tap(|s: &mut Vec<usize>, i| s.push(i));
        let mut w = build(&view);
        let size = layout(&mut w);
        assert_eq!(size.height, MIN_HIT_HEIGHT, "tappable row pads to 24 px");

        let mut log: Vec<usize> = Vec::new();
        let any: &mut dyn Any = &mut log;
        let mut ctx = EventCtx::new(any, Point::ZERO, size);
        let x = default_left(3) + 3.0;
        let y = size.height / 2.0;
        assert_eq!(
            w.event(&mut ctx, &ev(PointerPhase::Down, x, y)),
            EventResult::Handled
        );
        w.event(&mut ctx, &ev(PointerPhase::Up, x, y));
        assert_eq!(log, vec![3]);
    }

    #[test]
    fn without_on_tap_pointer_events_are_ignored() {
        let mut w = build(&page_dots::<Vec<usize>>(5, 0));
        let size = layout(&mut w);
        assert_eq!(size.height, DEFAULT_DOT_SIZE);

        let mut log: Vec<usize> = Vec::new();
        let any: &mut dyn Any = &mut log;
        let mut ctx = EventCtx::new(any, Point::ZERO, size);
        let x = default_left(3) + 3.0;
        assert_eq!(
            w.event(&mut ctx, &ev(PointerPhase::Down, x, 3.0)),
            EventResult::Ignored
        );
        assert_eq!(
            w.event(&mut ctx, &ev(PointerPhase::Up, x, 3.0)),
            EventResult::Ignored
        );
        assert!(log.is_empty());
    }

    #[test]
    fn secondary_press_does_not_arm_a_tap() {
        let mut w = build(&page_dots::<Vec<usize>>(5, 0).on_tap(|s: &mut Vec<usize>, i| s.push(i)));
        let size = layout(&mut w);
        let mut log: Vec<usize> = Vec::new();
        let any: &mut dyn Any = &mut log;
        let mut ctx = EventCtx::new(any, Point::ZERO, size);
        let x = default_left(3) + 3.0;
        assert_eq!(
            w.event(&mut ctx, &secondary_down(x, 12.0)),
            EventResult::Ignored
        );
        assert!(w.captured.is_none());
    }

    fn logic_five(_s: &mut ()) -> PageDots<()> {
        page_dots(5, 2)
    }

    #[test]
    fn semantics_labels_the_current_page() {
        let mut root: frust_core::RenderRoot<(), PageDots<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic_five, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Status)
            .expect("page dots contribute a Role::Status node");
        assert_eq!(node.label(), Some("page 3 of 5"));
    }

    #[test]
    fn tappable_dots_expose_button_children() {
        fn logic(_s: &mut ()) -> PageDots<()> {
            page_dots(5, 2).on_tap(|_s: &mut (), _i| {})
        }
        let mut root: frust_core::RenderRoot<(), PageDots<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let buttons = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .count();
        assert_eq!(buttons, 5);
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.label() == Some("page 4"))
        );
    }

    #[derive(Default)]
    struct Taps(Vec<usize>);

    fn tap_logic(_s: &mut Taps) -> PageDots<Taps> {
        page_dots(5, 2).on_tap(|s: &mut Taps, i| s.0.push(i))
    }

    fn tap_root() -> (frust_core::RenderRoot<Taps, PageDots<Taps>>, Taps) {
        let mut root: frust_core::RenderRoot<Taps, PageDots<Taps>> = frust_core::RenderRoot::new();
        let mut state = Taps::default();
        root.rebuild(&mut tap_logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        (root, state)
    }

    #[test]
    fn accessibility_click_on_each_page_button_delivers_its_own_index() {
        let (mut root, mut state) = tap_root();
        let update = root.semantics();
        for n in 1..=5usize {
            let label = format!("page {n}");
            let id = update
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some(label.as_str()))
                .map(|(id, _)| *id)
                .expect("button exists");
            root.perform_accessibility_action(&mut state, id, Action::Click);
            assert_eq!(state.0.last(), Some(&(n - 1)), "{label}");
        }
        assert_eq!(state.0, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn button_bounds_are_disjoint_slots_within_the_row() {
        let (root, _state) = tap_root();
        let update = root.semantics();
        let status = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Status)
            .map(|(_, n)| n)
            .expect("status container");
        assert_eq!(status.label(), Some("page 3 of 5"));
        let row = status.bounds().expect("row bounds");
        let mut rects: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .map(|(_, n)| {
                assert!(n.supports_action(Action::Click));
                n.bounds().expect("button bounds")
            })
            .collect();
        assert_eq!(rects.len(), 5);
        rects.sort_by(|a, b| a.x0.total_cmp(&b.x0));
        for r in &rects {
            assert!(r.x0 >= row.x0 && r.x1 <= row.x1, "{r:?} within {row:?}");
            assert!(r.x1 > r.x0);
        }
        for pair in rects.windows(2) {
            assert!(pair[0].x1 <= pair[1].x0, "slots must not overlap");
        }
    }

    #[test]
    fn a_constrained_row_keeps_every_button_rect_well_formed_and_inside() {
        let mut root: frust_core::RenderRoot<Taps, PageDots<Taps>> = frust_core::RenderRoot::new();
        let mut state = Taps::default();
        let mut logic = |_s: &mut Taps| page_dots(7, 0).on_tap(|s: &mut Taps, i| s.0.push(i));
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(40.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let row = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Status)
            .and_then(|(_, n)| n.bounds())
            .expect("row bounds");
        let buttons: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert!(!buttons.is_empty(), "in-range dots keep their buttons");
        for (_, n) in &buttons {
            let r = n.bounds().expect("button bounds");
            assert!(
                row.x0 <= r.x0 && r.x0 <= r.x1 && r.x1 <= row.x1,
                "{r:?} well-formed within {row:?}"
            );
        }
        let (id, _) = buttons
            .iter()
            .find(|(_, n)| n.label() == Some("page 1"))
            .expect("page 1 button");
        root.perform_accessibility_action(&mut state, *id, Action::Click);
        assert_eq!(state.0, vec![0]);

        // Slots 0..=3 start inside the 40 px row (slot 3 is clipped); slots
        // 4..=6 collapse to zero width and are omitted, labels keeping the
        // original index.
        let mut labels: Vec<_> = buttons
            .iter()
            .filter_map(|(_, n)| n.label().map(str::to_owned))
            .collect();
        labels.sort();
        assert!(buttons.len() < 7, "collapsed slots are omitted");
        assert_eq!(labels, ["page 1", "page 2", "page 3", "page 4"]);

        // The highest surviving button delivers its own index.
        let (id, last) = buttons
            .iter()
            .find(|(_, n)| n.label() == Some("page 4"))
            .expect("page 4 button");
        let r = last.bounds().expect("bounds");
        assert!(r.x1 > r.x0, "clipped slot keeps a non-empty rect");
        root.perform_accessibility_action(&mut state, *id, Action::Click);
        assert_eq!(state.0, vec![0, 3]);
    }

    #[test]
    fn a_row_that_clips_the_real_last_slot_keeps_its_button_and_index() {
        let mut root: frust_core::RenderRoot<Taps, PageDots<Taps>> = frust_core::RenderRoot::new();
        let mut state = Taps::default();
        let mut logic = |_s: &mut Taps| page_dots(7, 0).on_tap(|s: &mut Taps, i| s.0.push(i));
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        // Slot 6 spans x0 = 80 .. 90 unclamped; an 84 px row clips it to 80..84.
        root.layout_with_text(Size::new(84.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let row = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Status)
            .and_then(|(_, n)| n.bounds())
            .expect("row bounds");
        let buttons: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert_eq!(buttons.len(), 7, "every slot still starts inside the row");
        let (id, last) = buttons
            .iter()
            .find(|(_, n)| n.label() == Some("page 7"))
            .expect("page 7 button");
        let r = last.bounds().expect("bounds");
        assert!(r.x1 > r.x0, "partly inside: {r:?}");
        assert!(row.x0 <= r.x0 && r.x1 <= row.x1, "{r:?} within {row:?}");
        assert!(r.x1 - r.x0 < 10.0, "clipped narrower than a full pitch");
        root.perform_accessibility_action(&mut state, *id, Action::Click);
        assert_eq!(state.0, vec![6]);
    }

    #[test]
    fn decorative_row_exposes_a_single_status_node() {
        let mut root: frust_core::RenderRoot<(), PageDots<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic_five, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let roles: Vec<_> = update
            .nodes
            .iter()
            .map(|(_, n)| n.role())
            .filter(|r| matches!(r, Role::Status | Role::Button))
            .collect();
        assert_eq!(roles, vec![Role::Status]);
    }
}
