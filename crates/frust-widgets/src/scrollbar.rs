//! The baseline scrollbar: an opt-in overlay indicator for the vertical
//! [`ScrollView`](crate::ScrollView) and [`ListView`](crate::ListView).
//!
//! A surface paints no bar unless the view calls `.scrollbar(ScrollbarStyle)`;
//! without it nothing here runs, so an existing app sees no paint, hit-test or
//! controller change. With one, the bar paints in a trailing lane on top of the
//! content: the thumb is as long a share of the track as the viewport is of the
//! content (never shorter than [`ScrollbarStyle::min_thumb`]), it widens to
//! [`ScrollbarStyle::hover_thickness`] while the pointer is over the lane or the
//! thumb is held, and [`ScrollbarShow`] picks when it is visible. A primary press
//! on the thumb captures the pointer and drags it, and a press elsewhere on the
//! track pages by one viewport extent; both write the offset through the
//! surface's [`ScrollController`](crate::ScrollController) `jump_to` — the app's
//! handle when one is attached, otherwise an internal one the surface creates
//! only while a bar is enabled. A thumb press first jumps to where the surface
//! stands, which halts any fling or tween under it.
//!
//! The fade runs at paint off [`PaintCtx::frame_time`]: the bar holds for
//! [`ScrollbarStyle::fade_delay_ms`] after the last scroll, hover or press, then
//! fades over [`ScrollbarStyle::fade_duration_ms`]; `motion.reduce_motion` snaps
//! it out instead. The state machine lives here and the surfaces reach it
//! through `scroll_core.rs`. Horizontal surfaces and `CustomScrollView` take no
//! scrollbar.

use std::time::Duration;

use frust_core::{EventCtx, FrameTime, PaintCtx, PaintScene, PointerEvent, PointerPhase};
use frust_theme::Theme;
use kurbo::{Point, Rect, Size};
use peniko::Color;

use crate::authoring::presses;
use crate::scroll::ScrollInfo;

/// Resting bar thickness, logical px. **Community-approximate**, like the
/// other fallbacks below: overlay scrollbars sit in this range on every
/// platform and no theme token covers them.
const THICKNESS: f64 = 6.0;
/// Thickness while hovered or held.
const HOVER_THICKNESS: f64 = 10.0;
/// Shortest thumb painted, so a long document keeps a grabbable thumb.
const MIN_THUMB: f64 = 24.0;
/// Gap between the bar and the viewport's trailing, top and bottom edges.
const INSET: f64 = 2.0;
/// Idle hold before the fade starts.
const FADE_DELAY_MS: f64 = 800.0;
/// Fade-out length.
const FADE_DURATION_MS: f64 = 250.0;
/// Unthemed thumb/track ink; a theme supplies `on_surface_variant` instead.
const FALLBACK_INK: Color = Color::from_rgb8(0x5C, 0x5C, 0x5C);
/// Thumb alpha at rest, and while widened.
const THUMB_ALPHA: f32 = 0.5;
const THUMB_ACTIVE_ALPHA: f32 = 0.75;
/// Track alpha, painted only while widened.
const TRACK_ALPHA: f32 = 0.12;

/// When an enabled scrollbar is visible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollbarShow {
    /// Never painted and never hit-tested — the same as no `.scrollbar(..)`.
    Never,
    /// Shown while the surface scrolls (or the lane is hovered or the thumb
    /// held), fading out once idle. The default.
    #[default]
    WhileScrolling,
    /// Shown while the pointer is over the surface, and while scrolling.
    Hover,
    /// Shown whenever the content overflows.
    Always,
}

/// Metrics and visibility of an opt-in scrollbar, handed to
/// [`ScrollView::scrollbar`](crate::ScrollView::scrollbar) or
/// [`ListView::scrollbar`](crate::ListView::scrollbar) (vertical only; no
/// horizontal or `CustomScrollView` surface takes one). Colors come from the
/// theme (`on_surface_variant`), with an unthemed fallback.
///
/// ```
/// use frust_widgets::{ScrollView, ScrollbarShow, ScrollbarStyle, scroll_view, text};
/// let style = ScrollbarStyle::default().show(ScrollbarShow::Always).thickness(4.0);
/// let view: ScrollView<()> = scroll_view(text("hi")).scrollbar(style);
/// # let _ = view;
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarStyle {
    /// Resting bar thickness, logical px.
    pub thickness: f64,
    /// Thickness while the lane is hovered or the thumb is held.
    pub hover_thickness: f64,
    /// Shortest thumb painted, clamped to the track length.
    pub min_thumb: f64,
    /// Gap between the bar and the viewport's trailing, top and bottom edges.
    pub inset: f64,
    /// Idle hold before the fade starts, in ms.
    pub fade_delay_ms: f64,
    /// Fade-out length, in ms; `reduce_motion` or `0` snaps instead.
    pub fade_duration_ms: f64,
    /// When the bar is visible.
    pub show: ScrollbarShow,
}

impl Default for ScrollbarStyle {
    fn default() -> Self {
        Self {
            thickness: THICKNESS,
            hover_thickness: HOVER_THICKNESS,
            min_thumb: MIN_THUMB,
            inset: INSET,
            fade_delay_ms: FADE_DELAY_MS,
            fade_duration_ms: FADE_DURATION_MS,
            show: ScrollbarShow::default(),
        }
    }
}

impl ScrollbarStyle {
    /// The default style with `show` as its visibility rule.
    pub fn new(show: ScrollbarShow) -> Self {
        Self::default().show(show)
    }

    /// Set [`ScrollbarStyle::show`].
    pub fn show(mut self, show: ScrollbarShow) -> Self {
        self.show = show;
        self
    }

    /// Set [`ScrollbarStyle::thickness`].
    pub fn thickness(mut self, px: f64) -> Self {
        self.thickness = px;
        self
    }

    /// Set [`ScrollbarStyle::hover_thickness`].
    pub fn hover_thickness(mut self, px: f64) -> Self {
        self.hover_thickness = px;
        self
    }

    /// Set [`ScrollbarStyle::min_thumb`].
    pub fn min_thumb(mut self, px: f64) -> Self {
        self.min_thumb = px;
        self
    }

    /// Set [`ScrollbarStyle::inset`].
    pub fn inset(mut self, px: f64) -> Self {
        self.inset = px;
        self
    }

    /// Set [`ScrollbarStyle::fade_delay_ms`].
    pub fn fade_delay_ms(mut self, ms: f64) -> Self {
        self.fade_delay_ms = ms;
        self
    }

    /// Set [`ScrollbarStyle::fade_duration_ms`].
    pub fn fade_duration_ms(mut self, ms: f64) -> Self {
        self.fade_duration_ms = ms;
        self
    }
}

/// One frame's bar geometry in surface-local coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Scrollbar {
    /// The full-height hit lane along the trailing edge (widest bar plus insets).
    pub(crate) lane: Rect,
    /// The painted track: the viewport extent minus the insets.
    pub(crate) track: Rect,
    /// The thumb, inside the track.
    pub(crate) thumb: Rect,
    /// How far the thumb's top can slide.
    travel: f64,
    max_offset: f64,
}

impl Scrollbar {
    /// The bar for a `size` viewport at `info`, or `None` when nothing
    /// overflows or no track fits.
    pub(crate) fn new(
        style: &ScrollbarStyle,
        size: Size,
        info: ScrollInfo,
        widened: bool,
    ) -> Option<Self> {
        let max_offset = info.max_offset;
        let inset = style.inset.max(0.0);
        let thickness = if widened {
            style.hover_thickness
        } else {
            style.thickness
        };
        let track_len = size.height - 2.0 * inset;
        if !(max_offset > 0.0 && track_len > 0.0 && thickness > 0.0 && size.width > 0.0) {
            return None;
        }
        let right = size.width - inset;
        let track = Rect::new(right - thickness, inset, right, inset + track_len);
        let lane_width = style.hover_thickness.max(style.thickness) + 2.0 * inset;
        let lane = Rect::new(
            (size.width - lane_width).max(0.0),
            0.0,
            size.width,
            size.height,
        );
        let share = size.height / (size.height + max_offset);
        let thumb_len = (track_len * share)
            .max(style.min_thumb.min(track_len))
            .min(track_len);
        let travel = track_len - thumb_len;
        let progress = (info.offset / max_offset).clamp(0.0, 1.0);
        let top = inset + travel * progress;
        let thumb = Rect::new(track.x0, top, track.x1, top + thumb_len);
        Some(Self {
            lane,
            track,
            thumb,
            travel,
            max_offset,
        })
    }

    /// The offset a thumb drag `dy` px from a press at `start_offset` maps to,
    /// at the ratio the thumb is painted at.
    pub(crate) fn offset_for_drag(&self, start_offset: f64, dy: f64) -> f64 {
        if self.travel <= 0.0 {
            return start_offset;
        }
        (start_offset + dy * self.max_offset / self.travel).clamp(0.0, self.max_offset)
    }

    fn hits_lane(&self, p: Point) -> bool {
        self.lane.contains(p)
    }

    /// Lane-wide across, the thumb's extent along.
    fn hits_thumb(&self, p: Point) -> bool {
        self.hits_lane(p) && p.y >= self.thumb.y0 && p.y < self.thumb.y1
    }
}

/// What a pointer event did to the bar, for the core to act on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BarEvent {
    /// Not the bar's: route it to the gesture machinery.
    Pass,
    /// Consumed with nothing to write.
    Handled,
    /// Consumed: jump the controller to this offset.
    JumpTo(f64),
}

/// A primary press the bar captured.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Press {
    /// Dragging the thumb from a press at `start_y` over `start_offset`.
    Thumb { start_y: f64, start_offset: f64 },
    /// A track press that already paged; its moves and release are swallowed.
    Track,
}

/// The paint/fade schedule [`ScrollbarState::visibility`] asks for.
/// Longest single hold wake: a far-off fade re-arms after this instead of
/// overflowing `Duration`.
const MAX_HOLD_SECS: f64 = 3600.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Wake {
    Idle,
    Hold(Duration),
    Fade,
}

/// A surface's live scrollbar: its style plus the hover, press and fade state.
pub(crate) struct ScrollbarState {
    style: ScrollbarStyle,
    lane_hovered: bool,
    surface_hovered: bool,
    press: Option<Press>,
    /// The last frame the bar was active (scrolled, hovered or held).
    last_active: Option<FrameTime>,
    /// `(offset, overscroll)` at the last paint, to detect motion.
    last_position: Option<(f64, f64)>,
    /// An event-pass activity the next paint stamps (events carry no clock).
    stamp_pending: bool,
    /// The opacity last painted; a press hits only a visible bar.
    opacity: f64,
}

impl ScrollbarState {
    pub(crate) fn new(style: ScrollbarStyle) -> Self {
        Self {
            style,
            lane_hovered: false,
            surface_hovered: false,
            press: None,
            last_active: None,
            last_position: None,
            stamp_pending: false,
            opacity: 0.0,
        }
    }

    /// Replace the style, returning whether it changed.
    pub(crate) fn set_style(&mut self, style: ScrollbarStyle) -> bool {
        let changed = self.style != style;
        self.style = style;
        changed
    }

    pub(crate) fn is_widened(&self) -> bool {
        self.lane_hovered || self.press.is_some()
    }

    /// Hovered or held: shown at full opacity with no fade running.
    fn engaged(&self) -> bool {
        self.is_widened() || (self.style.show == ScrollbarShow::Hover && self.surface_hovered)
    }

    /// The opacity the last paint used (test-only read).
    #[cfg(test)]
    pub(crate) fn opacity(&self) -> f64 {
        self.opacity
    }

    /// The bar's half of a pointer event, ahead of the gesture machinery;
    /// `can_drive` is whether a current controller binding can take the jump.
    pub(crate) fn pointer(
        &mut self,
        ctx: &mut EventCtx,
        p: &PointerEvent,
        info: ScrollInfo,
        can_drive: bool,
    ) -> BarEvent {
        let size = ctx.size();
        match p.phase {
            PointerPhase::Down => {
                self.press = None;
                if !presses(p) || !can_drive || self.opacity <= 0.0 {
                    return BarEvent::Pass;
                }
                let Some(bar) = Scrollbar::new(&self.style, size, info, true) else {
                    return BarEvent::Pass;
                };
                if !bar.hits_lane(p.position) {
                    return BarEvent::Pass;
                }
                ctx.capture_pointer();
                ctx.request_redraw();
                self.stamp_pending = true;
                if bar.hits_thumb(p.position) {
                    self.press = Some(Press::Thumb {
                        start_y: p.position.y,
                        start_offset: info.offset,
                    });
                    return BarEvent::JumpTo(info.offset);
                }
                self.press = Some(Press::Track);
                let page = if p.position.y < bar.thumb.y0 {
                    -size.height
                } else {
                    size.height
                };
                BarEvent::JumpTo((info.offset + page).clamp(0.0, info.max_offset))
            }
            PointerPhase::Move => match self.press {
                Some(Press::Thumb {
                    start_y,
                    start_offset,
                }) => {
                    ctx.request_redraw();
                    match Scrollbar::new(&self.style, size, info, true) {
                        Some(bar) => BarEvent::JumpTo(
                            bar.offset_for_drag(start_offset, p.position.y - start_y),
                        ),
                        None => BarEvent::Handled,
                    }
                }
                Some(Press::Track) => BarEvent::Handled,
                None => BarEvent::Pass,
            },
            PointerPhase::Up | PointerPhase::Cancel => {
                if self.press.take().is_none() {
                    return BarEvent::Pass;
                }
                self.stamp_pending = true;
                ctx.request_redraw();
                BarEvent::Handled
            }
        }
    }

    /// Latch hover from an uncaptured `Move` the content has already seen
    /// (a container claims after its children), redrawing only on a change
    /// that alters the bar.
    pub(crate) fn hover(&mut self, ctx: &mut EventCtx, position: Point, info: ScrollInfo) {
        let size = ctx.size();
        let in_surface = Rect::from_origin_size(Point::ZERO, size).contains(position);
        let in_lane = Scrollbar::new(&self.style, size, info, true)
            .is_some_and(|bar| bar.hits_lane(position));
        let hover_mode = self.style.show == ScrollbarShow::Hover;
        if in_lane || (hover_mode && in_surface) {
            ctx.claim_hover();
        }
        let was_engaged = self.engaged();
        let lane_changed = self.lane_hovered != in_lane;
        let surface_changed = self.surface_hovered != in_surface;
        self.lane_hovered = in_lane;
        self.surface_hovered = in_surface;
        if was_engaged != self.engaged() {
            self.stamp_pending = true;
        }
        if lane_changed || (hover_mode && surface_changed) {
            ctx.request_redraw();
        }
    }

    /// The opacity at `now` and the frame schedule that keeps the fade moving.
    fn visibility(&self, now: FrameTime, reduce_motion: bool) -> (f64, Wake) {
        match self.style.show {
            ScrollbarShow::Always => return (1.0, Wake::Idle),
            ScrollbarShow::Never => return (0.0, Wake::Idle),
            ScrollbarShow::WhileScrolling | ScrollbarShow::Hover => {}
        }
        if self.engaged() {
            return (1.0, Wake::Idle);
        }
        let Some(active) = self.last_active else {
            return (0.0, Wake::Idle);
        };
        let elapsed = now.saturating_sub(active).as_secs_f64() * 1000.0;
        let delay = self.style.fade_delay_ms.max(0.0);
        if !delay.is_finite() {
            return (1.0, Wake::Idle); // never fades: nothing to wake for
        }
        if elapsed < delay {
            let remaining = ((delay - elapsed) / 1000.0).min(MAX_HOLD_SECS);
            return (1.0, Wake::Hold(Duration::from_secs_f64(remaining)));
        }
        let duration = self.style.fade_duration_ms;
        if reduce_motion || duration.is_nan() || duration <= 0.0 {
            return (0.0, Wake::Idle);
        }
        if !duration.is_finite() {
            return (1.0, Wake::Idle); // an endless fade stays opaque, no frame loop
        }
        let t = (elapsed - delay) / duration;
        if t >= 1.0 {
            (0.0, Wake::Idle)
        } else {
            (1.0 - t, Wake::Fade)
        }
    }

    /// Advance the fade and paint the bar over the content at `info`.
    pub(crate) fn paint(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        info: ScrollInfo,
        reduce_motion: bool,
    ) {
        let was_engaged = self.engaged();
        if !ctx.is_hovered() {
            self.lane_hovered = false;
            self.surface_hovered = false;
        }
        let now = ctx.frame_time();
        let position = (info.offset, info.overscroll);
        let moved = self.last_position.is_some_and(|last| last != position);
        self.last_position = Some(position);
        let pending = std::mem::take(&mut self.stamp_pending);
        if moved || pending || was_engaged || self.engaged() {
            self.last_active = Some(now);
        }
        let (opacity, wake) = self.visibility(now, reduce_motion);
        self.opacity = opacity;
        match wake {
            Wake::Idle => {}
            Wake::Hold(remaining) => ctx.request_frame_paced_at(remaining),
            Wake::Fade => ctx.request_frame(),
        }
        if opacity <= 0.0 {
            return;
        }
        let widened = self.is_widened();
        let Some(bar) = Scrollbar::new(&self.style, ctx.size(), info, widened) else {
            return;
        };
        let ink =
            Theme::from_paint_ctx(ctx).map_or(FALLBACK_INK, |t| t.scheme().on_surface_variant);
        let shift = ctx.origin().to_vec2();
        let radius = bar.track.width() / 2.0;
        let opacity = opacity as f32;
        if widened {
            let track = with_alpha(ink, TRACK_ALPHA * opacity);
            scene.fill_rounded_rect(bar.track.origin() + shift, bar.track.size(), radius, track);
        }
        let alpha = if widened {
            THUMB_ACTIVE_ALPHA
        } else {
            THUMB_ALPHA
        };
        let thumb = with_alpha(ink, alpha * opacity);
        scene.fill_rounded_rect(bar.thumb.origin() + shift, bar.thumb.size(), radius, thumb);
    }
}

/// `color` with its alpha replaced by `alpha` (the crate's per-module helper).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

#[cfg(test)]
pub(crate) mod tests {
    use std::any::Any;

    use frust_core::PointerButton;

    use super::*;

    /// A recorder for the bar's rounded rects (the content paints plain rects).
    #[derive(Default)]
    pub(crate) struct BarScene {
        pub(crate) rounded: Vec<(Point, Size, Color)>,
    }

    impl PaintScene for BarScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, color: Color) {
            self.rounded.push((origin, size, color));
        }
    }

    const VIEW: Size = Size::new(200.0, 100.0);

    fn info(offset: f64, max_offset: f64) -> ScrollInfo {
        ScrollInfo {
            offset,
            max_offset,
            overscroll: 0.0,
        }
    }

    fn at(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> PointerEvent {
        PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        }
    }

    fn send(state: &mut ScrollbarState, p: PointerEvent, info: ScrollInfo) -> (BarEvent, bool) {
        let mut unit = ();
        let any: &mut dyn Any = &mut unit;
        let mut ctx = EventCtx::new(any, Point::ZERO, VIEW);
        let result = state.pointer(&mut ctx, &p, info, true);
        (result, ctx.is_pointer_captured())
    }

    fn hover(state: &mut ScrollbarState, x: f64, y: f64, info: ScrollInfo) -> bool {
        let mut unit = ();
        let any: &mut dyn Any = &mut unit;
        let mut ctx = EventCtx::new(any, Point::ZERO, VIEW);
        state.hover(&mut ctx, Point::new(x, y), info);
        ctx.needs_redraw()
    }

    /// Paint at `ms`, returning how many rects drew and whether a frame was asked.
    fn paint(state: &mut ScrollbarState, ms: u64, info: ScrollInfo, reduce: bool) -> (usize, bool) {
        let mut ctx = PaintCtx::for_test(Point::ZERO, VIEW, at(ms));
        let mut scene = BarScene::default();
        state.paint(&mut ctx, &mut scene, info, reduce);
        (scene.rounded.len(), ctx.needs_frame())
    }

    #[test]
    fn the_thumb_stays_in_the_track_and_tracks_progress() {
        let style = ScrollbarStyle::default();
        for (offset, max) in [
            (0.0, 900.0),
            (450.0, 900.0),
            (900.0, 900.0),
            (5000.0, 900.0),
        ] {
            let bar = Scrollbar::new(&style, VIEW, info(offset, max), false).unwrap();
            assert!(bar.thumb.y0 >= bar.track.y0 && bar.thumb.y1 <= bar.track.y1);
            assert_eq!(bar.track.height(), VIEW.height - 2.0 * INSET);
            assert_eq!(bar.track.x1, VIEW.width - INSET);
        }
        let top = Scrollbar::new(&style, VIEW, info(-50.0, 900.0), false).unwrap();
        assert_eq!(top.thumb.y0, top.track.y0, "progress clamps at the top");
        let end = Scrollbar::new(&style, VIEW, info(5000.0, 900.0), false).unwrap();
        assert_eq!(end.thumb.y1, end.track.y1, "and at the bottom");
    }

    #[test]
    fn the_thumb_is_proportional_with_a_minimum() {
        let style = ScrollbarStyle::default();
        let half = Scrollbar::new(&style, VIEW, info(0.0, 100.0), false).unwrap();
        assert_eq!(half.thumb.height(), half.track.height() / 2.0);
        let long = Scrollbar::new(&style, VIEW, info(0.0, 100_000.0), false).unwrap();
        assert_eq!(long.thumb.height(), MIN_THUMB);
        let tiny = Scrollbar::new(&style.min_thumb(500.0), VIEW, info(0.0, 10.0), false).unwrap();
        assert_eq!(
            tiny.thumb.height(),
            tiny.track.height(),
            "min clamps to the track"
        );
        assert!(Scrollbar::new(&style, VIEW, info(0.0, 0.0), false).is_none());
    }

    #[test]
    fn widening_thickens_the_bar_without_moving_the_thumb() {
        let style = ScrollbarStyle::default();
        let rest = Scrollbar::new(&style, VIEW, info(300.0, 900.0), false).unwrap();
        let wide = Scrollbar::new(&style, VIEW, info(300.0, 900.0), true).unwrap();
        assert_eq!(rest.track.width(), THICKNESS);
        assert_eq!(wide.track.width(), HOVER_THICKNESS);
        assert_eq!(
            (rest.thumb.y0, rest.thumb.y1),
            (wide.thumb.y0, wide.thumb.y1)
        );
    }

    #[test]
    fn a_thumb_drag_maps_travel_to_offset() {
        let mut state = ScrollbarState::new(ScrollbarStyle::new(ScrollbarShow::Always));
        paint(&mut state, 0, info(0.0, 900.0), false);
        let bar = Scrollbar::new(&state.style, VIEW, info(0.0, 900.0), true).unwrap();
        let (down, captured) = send(
            &mut state,
            pointer(PointerPhase::Down, 195.0, 5.0),
            info(0.0, 900.0),
        );
        assert_eq!(
            down,
            BarEvent::JumpTo(0.0),
            "the press halts where it landed"
        );
        assert!(captured);
        let half = bar.travel / 2.0;
        let (moved, _) = send(
            &mut state,
            pointer(PointerPhase::Move, 150.0, 5.0 + half),
            info(0.0, 900.0),
        );
        assert_eq!(moved, BarEvent::JumpTo(450.0));
        let (past, _) = send(
            &mut state,
            pointer(PointerPhase::Move, 150.0, 900.0),
            info(0.0, 900.0),
        );
        assert_eq!(past, BarEvent::JumpTo(900.0), "clamped to the range");
        let (up, _) = send(
            &mut state,
            pointer(PointerPhase::Up, 150.0, 900.0),
            info(0.0, 900.0),
        );
        assert_eq!(up, BarEvent::Handled);
        let (after, _) = send(
            &mut state,
            pointer(PointerPhase::Move, 150.0, 50.0),
            info(0.0, 900.0),
        );
        assert_eq!(after, BarEvent::Pass, "the release ends the drag");
    }

    #[test]
    fn a_track_press_pages_by_one_viewport() {
        let mut state = ScrollbarState::new(ScrollbarStyle::new(ScrollbarShow::Always));
        let at_mid = info(400.0, 900.0);
        paint(&mut state, 0, at_mid, false);
        let (below, _) = send(&mut state, pointer(PointerPhase::Down, 195.0, 98.0), at_mid);
        assert_eq!(below, BarEvent::JumpTo(400.0 + VIEW.height));
        let (moved, _) = send(&mut state, pointer(PointerPhase::Move, 195.0, 10.0), at_mid);
        assert_eq!(moved, BarEvent::Handled, "a track press swallows its moves");
        send(&mut state, pointer(PointerPhase::Up, 195.0, 10.0), at_mid);
        let (above, _) = send(&mut state, pointer(PointerPhase::Down, 195.0, 3.0), at_mid);
        assert_eq!(above, BarEvent::JumpTo(400.0 - VIEW.height));
    }

    #[test]
    fn presses_off_the_lane_or_non_primary_pass_through() {
        let mut state = ScrollbarState::new(ScrollbarStyle::new(ScrollbarShow::Always));
        paint(&mut state, 0, info(0.0, 900.0), false);
        let (content, captured) = send(
            &mut state,
            pointer(PointerPhase::Down, 50.0, 5.0),
            info(0.0, 900.0),
        );
        assert_eq!(content, BarEvent::Pass);
        assert!(!captured);
        let mut secondary = pointer(PointerPhase::Down, 195.0, 5.0);
        secondary.button = PointerButton::Secondary;
        assert_eq!(
            send(&mut state, secondary, info(0.0, 900.0)).0,
            BarEvent::Pass
        );
    }

    #[test]
    fn hovering_the_lane_widens_and_redraws_only_on_change() {
        let mut state = ScrollbarState::new(ScrollbarStyle::default());
        assert!(
            !hover(&mut state, 50.0, 50.0, info(0.0, 900.0)),
            "content hover: no redraw"
        );
        assert!(hover(&mut state, 195.0, 50.0, info(0.0, 900.0)));
        assert!(state.is_widened());
        assert!(
            !hover(&mut state, 196.0, 52.0, info(0.0, 900.0)),
            "still in the lane"
        );
        assert!(hover(&mut state, 50.0, 50.0, info(0.0, 900.0)));
        assert!(!state.is_widened());
    }

    #[test]
    fn while_scrolling_fades_after_the_delay_under_a_stepped_clock() {
        let mut state = ScrollbarState::new(ScrollbarStyle::default());
        assert_eq!(
            paint(&mut state, 0, info(0.0, 900.0), false),
            (0, false),
            "idle: hidden"
        );
        assert_eq!(
            paint(&mut state, 16, info(40.0, 900.0), false),
            (1, true),
            "a scroll shows it"
        );
        assert_eq!(state.opacity(), 1.0);
        paint(&mut state, 16 + 700, info(40.0, 900.0), false);
        assert_eq!(state.opacity(), 1.0, "still holding");
        paint(&mut state, 16 + 800 + 125, info(40.0, 900.0), false);
        assert!(
            (state.opacity() - 0.5).abs() < 1e-9,
            "halfway through the fade"
        );
        assert_eq!(
            paint(&mut state, 16 + 800 + 250, info(40.0, 900.0), false),
            (0, false)
        );
        assert_eq!(state.opacity(), 0.0);
    }

    #[test]
    fn non_finite_fade_values_neither_panic_nor_spin() {
        let hold = ScrollbarStyle::default().fade_delay_ms(f64::INFINITY);
        let mut state = ScrollbarState::new(hold);
        paint(&mut state, 0, info(0.0, 900.0), false);
        assert_eq!(
            paint(&mut state, 16, info(40.0, 900.0), false),
            (1, false),
            "an infinite delay holds the bar with no wake"
        );
        paint(&mut state, 16 + 100_000, info(40.0, 900.0), false);
        assert_eq!(state.opacity(), 1.0);

        let huge = ScrollbarStyle::default().fade_delay_ms(1e30);
        let mut state = ScrollbarState::new(huge);
        paint(&mut state, 0, info(0.0, 900.0), false);
        let (drawn, _) = paint(&mut state, 16, info(40.0, 900.0), false);
        assert_eq!(drawn, 1, "a huge delay holds without overflowing the wake");

        let forever = ScrollbarStyle::default().fade_duration_ms(f64::INFINITY);
        let mut state = ScrollbarState::new(forever);
        paint(&mut state, 0, info(0.0, 900.0), false);
        paint(&mut state, 16, info(40.0, 900.0), false);
        assert_eq!(
            paint(&mut state, 16 + 800 + 500, info(40.0, 900.0), false),
            (1, false),
            "an endless fade stays opaque and asks for no frame"
        );
        assert_eq!(state.opacity(), 1.0);
    }

    #[test]
    fn reduce_motion_snaps_the_bar_out_after_the_delay() {
        let mut state = ScrollbarState::new(ScrollbarStyle::default());
        paint(&mut state, 0, info(0.0, 900.0), true);
        paint(&mut state, 16, info(40.0, 900.0), true);
        assert_eq!(state.opacity(), 1.0);
        assert_eq!(
            paint(&mut state, 16 + 801, info(40.0, 900.0), true),
            (0, false)
        );
    }

    #[test]
    fn always_paints_and_a_hidden_bar_takes_no_press() {
        let mut always = ScrollbarState::new(ScrollbarStyle::new(ScrollbarShow::Always));
        assert_eq!(paint(&mut always, 0, info(0.0, 900.0), false), (1, false));
        let mut idle = ScrollbarState::new(ScrollbarStyle::default());
        paint(&mut idle, 0, info(0.0, 900.0), false);
        let (result, captured) = send(
            &mut idle,
            pointer(PointerPhase::Down, 195.0, 5.0),
            info(0.0, 900.0),
        );
        assert_eq!(result, BarEvent::Pass);
        assert!(!captured);
    }
}
