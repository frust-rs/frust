//! Ports beUI's `marquee` component.
//!
//! **Source:** `components/motion/marquee.tsx`, beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01 — the one
//! component in the motion catalog with **no motion dependency at all**: it is a
//! flex row duplicated twice under a CSS keyframe translation, paused on hover
//! by a `group-hover:[animation-play-state:paused]` rule.
//!
//! # The loop, restated as a paint transform
//!
//! Upstream renders the child list **twice** so the second copy is already in
//! frame when the first scrolls out. Here the children exist once and the track
//! is *painted* as many times as the box needs, each copy a translation apart —
//! the same seamless result with one retained subtree instead of two, and no
//! `aria-hidden` duplicate for a screen reader to skip.
//!
//! The scroll offset accumulates from the **frame delta**, not from an absolute
//! elapsed time. That is what makes pausing possible at all (a paused marquee
//! must resume where it stopped, not where the wall clock got to) and what keeps
//! the speed constant when the frame gate paces the loop.
//!
//! # Frame cadence
//!
//! A marquee is a perpetual decorative loop, so it asks for a **paced** frame at
//! its own natural cadence — one frame per logical pixel of travel
//! ([`MarqueeWidget::frame_interval`]), clamped into
//! [`MIN_FRAME_INTERVAL`]..=[`MAX_FRAME_INTERVAL`]. A slow ticker asks for four
//! frames a second rather than sixty; a fast one asks for the floor and is paced
//! like any other cosmetic loop.
//!
//! # Pause on hover, and what a press does
//!
//! `pauseOnHover` is honoured through the catalog's [`PointerTracker`], with a
//! rule this component has to settle because the substrate deliberately does
//! not: **a press does not pause the marquee.** The framework's hover link ends
//! at a pointer `Down`, and `PaintCtx::is_hovered` — which this widget re-syncs
//! from every paint — is the authoritative answer, so a press would otherwise
//! pause on the way down and resume on the way up for no reason a user could
//! predict. The tracker is therefore fed from **uncaptured `Move` events only**,
//! and the paint-time sync decides: pausing is a hover affordance, and a press
//! is not a hover.
//!
//! # Degradations against upstream
//!
//! - **The edge fade is an overlay, not a mask.** Upstream applies
//!   `mask-image: linear-gradient(...)`, which fades the content to *whatever is
//!   behind it*. frust's scene has no mask primitive, so [`MarqueeView::fade`]
//!   paints a surface-coloured gradient over each edge instead. That is exact on
//!   a marquee sitting on the theme's `surface` and wrong on any other
//!   background, so the fade is **off by default** here (upstream defaults it
//!   on).
//! - **Children are not pointer targets.** The scroll is a paint transform, so a
//!   child's laid-out box is not where it is drawn; routing a pointer to it
//!   would hit the wrong item, or the wrong copy of it. A marquee's items are
//!   decorative here — upstream marks its own second copy `inert` for the same
//!   class of reason. Broadcasts still reach every child so their pods stay live.

use std::time::Duration;

use frust::authoring::{
    Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx, View, ViewSeq, Widget,
    build_child, rebuild_children, teardown_child, visit_children,
};
use frust::{FrameTime, Theme};
use kurbo::{Point, Size, Vec2};
use peniko::{Brush, Color, ColorStop, Gradient};

use crate::motion::PointerTracker;
use crate::style::with_alpha;
use crate::tokens::BEUI_LIGHT;

/// How long one full track traversal takes — upstream's `speed = 30` seconds.
pub const DEFAULT_SPEED: Duration = Duration::from_secs(30);

/// The gap between items and across the loop seam — upstream's `gap = "1rem"`,
/// which is [`spacing(4.0)`](crate::style::spacing) on the shared ladder.
pub const DEFAULT_GAP: f64 = 16.0;

/// How far the edge fade reaches, as a fraction of the box — upstream's
/// `transparent, black 12%, black 88%, transparent`.
pub const FADE_FRACTION: f64 = 0.12;

/// Floor on the paced frame interval: asking for anything shorter is asking for
/// every vsync, which is what an unpaced request already means.
pub const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(8);

/// Ceiling on the paced frame interval, so a very slow marquee still moves
/// visibly rather than stepping once a second.
pub const MAX_FRAME_INTERVAL: Duration = Duration::from_millis(250);

/// Floor on a non-empty track's length, in logical px. A degenerate near-zero
/// track (rounding noise, or children whose combined size collapses toward
/// zero) would otherwise blow the paint-copy count up toward "as many as fit
/// across an astronomically short seam"; flooring it keeps that count bounded
/// without changing the empty-track (no children) case, which stays exactly
/// `0.0` and paints a single static copy.
const MIN_TRACK_LENGTH: f64 = 1.0;

/// Ceiling on how many track copies a single paint pass draws, regardless of
/// how the box extent and track length divide. A pathologically short track
/// (even after the floor above) still repeats a bounded number of times per
/// frame rather than scaling with the box extent.
const MAX_TRACK_COPIES: usize = 64;

/// Unthemed fallback surface (beUI light `--background`) — what the edge fade
/// dissolves into.
const FALLBACK_SURFACE: Color = BEUI_LIGHT.background;

/// Which way a marquee's content travels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarqueeDirection {
    /// Horizontal, content moving left — upstream's default.
    #[default]
    Left,
    /// Horizontal, content moving right (upstream's `animationDirection:
    /// reverse`).
    Right,
    /// Vertical, content moving up.
    Up,
    /// Vertical, content moving down.
    Down,
}

impl MarqueeDirection {
    /// All four, in upstream's own prop order.
    pub const ALL: [MarqueeDirection; 4] = [
        MarqueeDirection::Left,
        MarqueeDirection::Right,
        MarqueeDirection::Up,
        MarqueeDirection::Down,
    ];

    /// Whether the track runs down the box rather than across it.
    pub fn is_vertical(self) -> bool {
        matches!(self, MarqueeDirection::Up | MarqueeDirection::Down)
    }

    /// Whether the content travels toward increasing coordinates.
    pub fn is_reversed(self) -> bool {
        matches!(self, MarqueeDirection::Right | MarqueeDirection::Down)
    }
}

/// A declarative beUI marquee. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust::text;
/// use frust_beui::components::marquee::{MarqueeDirection, marquee};
///
/// let ticker = marquee::<(), _>((text("one"), text("two")))
///     .direction(MarqueeDirection::Right)
///     .pause_on_hover(true);
/// ```
pub struct MarqueeView<State: 'static> {
    children: Vec<AnyView<State>>,
    direction: MarqueeDirection,
    speed: Duration,
    gap: f64,
    pause_on_hover: bool,
    fade: bool,
}

/// Scroll `children` in an endless loop, travelling
/// [`left`](MarqueeDirection::Left) at [`DEFAULT_SPEED`].
pub fn marquee<State: 'static, M>(children: impl ViewSeq<State, M>) -> MarqueeView<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    MarqueeView {
        children: erased,
        direction: MarqueeDirection::default(),
        speed: DEFAULT_SPEED,
        gap: DEFAULT_GAP,
        pause_on_hover: true,
        fade: false,
    }
}

impl<State: 'static> MarqueeView<State> {
    /// Travel in `direction` instead of [`MarqueeDirection::Left`].
    pub fn direction(mut self, direction: MarqueeDirection) -> Self {
        self.direction = direction;
        self
    }

    /// How long one full traversal of the track takes (default
    /// [`DEFAULT_SPEED`]).
    pub fn speed(mut self, speed: Duration) -> Self {
        self.speed = speed;
        self
    }

    /// The gap between items — and, upstream's own note, across the loop seam,
    /// so the join is spaced like everything else (default [`DEFAULT_GAP`]).
    pub fn gap(mut self, gap: f64) -> Self {
        self.gap = gap.max(0.0);
        self
    }

    /// Stop the scroll while the pointer is over the marquee (default `true`,
    /// upstream's own default). See the [module docs](self) for the press rule.
    pub fn pause_on_hover(mut self, pause: bool) -> Self {
        self.pause_on_hover = pause;
        self
    }

    /// Fade the two ends into the theme's surface (default `false`, and see the
    /// [module docs](self) for why it differs from upstream's default).
    pub fn fade(mut self, fade: bool) -> Self {
        self.fade = fade;
        self
    }
}

/// The retained widget for a [`MarqueeView`].
pub struct MarqueeWidget {
    children: Vec<ChildPod>,
    direction: MarqueeDirection,
    speed: Duration,
    gap: f64,
    pause_on_hover: bool,
    fade: bool,
    /// One track's length along the travel axis, including the seam gap.
    track: f64,
    /// How far the track has travelled, in logical px, folded into `0..track`.
    offset: f64,
    /// The previous frame's timestamp, so the offset advances by a real delta
    /// rather than from an absolute clock a pause would break.
    last_frame: Option<FrameTime>,
    /// Widget-local pointer state — fed from uncaptured moves, corrected from
    /// `PaintCtx::is_hovered` every paint.
    pointer: PointerTracker,
}

impl MarqueeWidget {
    /// One track's length along the travel axis, including the seam gap.
    pub fn track_length(&self) -> f64 {
        self.track
    }

    /// How far the track has travelled, in logical px within `0..track`.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// Whether the scroll is currently held — the pointer is over the marquee
    /// and [`MarqueeView::pause_on_hover`] is on.
    pub fn is_paused(&self) -> bool {
        self.pause_on_hover && self.pointer.hovered()
    }

    /// The cadence this marquee asks the frame gate for: one frame per logical
    /// pixel of travel, clamped into
    /// [`MIN_FRAME_INTERVAL`]..=[`MAX_FRAME_INTERVAL`].
    pub fn frame_interval(&self) -> Duration {
        if self.track <= 0.0 || self.speed.is_zero() {
            return MAX_FRAME_INTERVAL;
        }
        // Clamp the seconds *before* building a `Duration` from it — an
        // extreme speed/track ratio (a huge `speed`, or a track shrunk toward
        // the floor above) can overflow `Duration::from_secs_f64`'s range,
        // which panics rather than saturating.
        let secs = (self.speed.as_secs_f64() / self.track).clamp(
            MIN_FRAME_INTERVAL.as_secs_f64(),
            MAX_FRAME_INTERVAL.as_secs_f64(),
        );
        Duration::from_secs_f64(secs)
    }

    /// Advance the scroll by the time since the previous frame, holding it while
    /// paused. Returns whether it moved.
    fn advance(&mut self, now: FrameTime, paused: bool) -> bool {
        let previous = self.last_frame.replace(now);
        if paused || self.track <= 0.0 || self.speed.is_zero() {
            return false;
        }
        let Some(previous) = previous else {
            // The first frame establishes the clock; there is no delta yet.
            return false;
        };
        let delta = now.saturating_sub(previous).as_secs_f64();
        let travelled = delta / self.speed.as_secs_f64() * self.track;
        self.offset = (self.offset + travelled).rem_euclid(self.track);
        travelled > 0.0
    }

    /// Where the first painted copy of the track starts, relative to the box's
    /// leading edge along the travel axis.
    fn first_copy(&self) -> f64 {
        if self.direction.is_reversed() {
            self.offset - self.track
        } else {
            -self.offset
        }
    }
}

impl<State: 'static> View<State> for MarqueeView<State> {
    type Element = MarqueeWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MarqueeWidget {
        MarqueeWidget {
            children: self
                .children
                .iter()
                .map(|child| build_child(child, ctx))
                .collect(),
            direction: self.direction,
            speed: self.speed,
            gap: self.gap,
            pause_on_hover: self.pause_on_hover,
            fade: self.fade,
            track: 0.0,
            offset: 0.0,
            last_frame: None,
            pointer: PointerTracker::new(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MarqueeWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |view| view,
            |_| None,
        );
        if element.direction != self.direction {
            element.direction = self.direction;
            // A different axis is a different track.
            element.offset = 0.0;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.speed != self.speed {
            element.speed = self.speed;
            flags |= ChangeFlags::PAINT;
        }
        if element.gap != self.gap {
            element.gap = self.gap;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.pause_on_hover != self.pause_on_hover {
            element.pause_on_hover = self.pause_on_hover;
            flags |= ChangeFlags::PAINT;
        }
        if element.fade != self.fade {
            element.fade = self.fade;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut MarqueeWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for MarqueeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let vertical = self.direction.is_vertical();
        // Items are measured against the box's cross extent and are free along
        // the travel axis — a track is as long as its content.
        let room = bc.max();
        let child_bc = BoxConstraints::loose(if vertical {
            Size::new(room.width, f64::INFINITY)
        } else {
            Size::new(f64::INFINITY, room.height)
        });

        let mut along = 0.0_f64;
        let mut across = 0.0_f64;
        for pod in &mut self.children {
            let size = pod.layout_child(ctx, &child_bc);
            if vertical {
                pod.set_origin(Point::new(0.0, along));
                along += size.height + self.gap;
                across = across.max(size.width);
            } else {
                pod.set_origin(Point::new(along, 0.0));
                along += size.width + self.gap;
                across = across.max(size.height);
            }
        }
        // The trailing gap is deliberate: it is the seam between one copy of the
        // track and the next, and upstream spaces it identically. A non-empty
        // track is floored at `MIN_TRACK_LENGTH` so a near-zero sum can't blow
        // up the paint-copy count; an empty one (no children) stays exactly
        // `0.0`.
        self.track = if along > 0.0 {
            along.max(MIN_TRACK_LENGTH)
        } else {
            0.0
        };
        self.offset = if self.track > 0.0 {
            self.offset.rem_euclid(self.track)
        } else {
            0.0
        };

        let natural = if vertical {
            Size::new(across, room.height.min(self.track.max(across)))
        } else {
            Size::new(room.width.min(self.track.max(across)), across)
        };
        bc.constrain(natural)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Both theme reads are taken up front: the theme borrows `ctx`, and
        // painting a child needs it mutably.
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let fade_into = surface(theme);
        let size = ctx.size();
        let origin = ctx.origin();
        self.pointer.set_size(size);
        // `PaintCtx::is_hovered` is the authoritative hover answer, and the only
        // thing that catches a pointer that left without another event.
        self.pointer.sync_hovered(ctx.is_hovered());

        let paused = reduce || self.is_paused();
        self.advance(ctx.frame_time(), paused);

        // `overflow: hidden` — a copy leaving the box disappears at the edge
        // rather than painting over a sibling.
        scene.push_clip(origin, size);
        let vertical = self.direction.is_vertical();
        let extent = if vertical { size.height } else { size.width };
        let (copies, paint_track) = if self.track > 0.0 {
            let min_effective_track = extent / (MAX_TRACK_COPIES - 1) as f64;
            let effective_track = min_effective_track.max(self.track);
            let copies = ((extent / effective_track).ceil() as usize + 1).min(MAX_TRACK_COPIES);
            (copies, effective_track)
        } else {
            (1, 0.0)
        };
        let first = self.first_copy();
        for copy in 0..copies {
            let shift = first + copy as f64 * paint_track;
            let translation = if vertical {
                Vec2::new(0.0, shift)
            } else {
                Vec2::new(shift, 0.0)
            };
            scene.push_transform(Affine::translate(translation));
            for pod in &mut self.children {
                pod.paint_child(ctx, scene);
            }
            scene.pop_transform();
        }
        scene.pop_clip();

        if self.fade {
            paint_fade(scene, origin, size, vertical, fade_into);
        }

        if !paused && self.track > 0.0 {
            // A perpetual decorative loop, asked for at its own cadence rather
            // than at every vsync. `request_frame_paced_at` already carries
            // `TickClass::CosmeticLoop`; naming the class separately would fold
            // `Duration::ZERO` into the same MIN-lattice and throw the cadence
            // away.
            ctx.request_frame_paced_at(self.frame_interval());
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in &mut self.children {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(pointer) = event else {
            return EventResult::Ignored;
        };
        // Moves only: a `Down` ends the framework's hover link, so treating one
        // as a hover would pause the scroll for the length of a tap. See the
        // module docs.
        if pointer.phase != frust::authoring::PointerPhase::Move {
            return EventResult::Ignored;
        }
        let size = ctx.size();
        let inside = pointer.position.x >= 0.0
            && pointer.position.y >= 0.0
            && pointer.position.x < size.width
            && pointer.position.y < size.height;
        if inside {
            ctx.claim_hover();
        }
        if self.pointer.on_pointer(pointer, size) && self.pause_on_hover {
            ctx.request_redraw();
        }
        // Watching a move is not consuming it.
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // One list, however many copies are painted — the duplicate upstream
        // marks `aria-hidden` has no counterpart here because there is only ever
        // one retained subtree.
        ctx.push_container(
            Role::List,
            |_| {},
            |ctx| {
                for pod in &self.children {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(children);
}

/// The colour the edge fade dissolves into.
fn surface(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK_SURFACE, |t| t.scheme().surface)
}

/// Paint the two edge fades — the overlay standing in for upstream's
/// `mask-image` (see the [module docs](self)).
fn paint_fade(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    vertical: bool,
    surface: Color,
) {
    let extent = if vertical { size.height } else { size.width };
    let reach = extent * FADE_FRACTION;
    if reach <= 0.0 {
        return;
    }
    let clear = with_alpha(surface, 0.0);
    let leading_size = if vertical {
        Size::new(size.width, reach)
    } else {
        Size::new(reach, size.height)
    };
    let axis = |from: Point, to: Point, near: Color, far: Color| {
        Gradient::new_linear(from, to).with_stops(
            [
                ColorStop::from((0.0f32, near)),
                ColorStop::from((1.0f32, far)),
            ]
            .as_slice(),
        )
    };

    let (lead_from, lead_to) = if vertical {
        (origin, Point::new(origin.x, origin.y + reach))
    } else {
        (origin, Point::new(origin.x + reach, origin.y))
    };
    scene.fill_rect_brush(
        origin,
        leading_size,
        &Brush::Gradient(axis(lead_from, lead_to, surface, clear)),
    );

    let trailing_origin = if vertical {
        Point::new(origin.x, origin.y + size.height - reach)
    } else {
        Point::new(origin.x + size.width - reach, origin.y)
    };
    let (trail_from, trail_to) = if vertical {
        (
            trailing_origin,
            Point::new(trailing_origin.x, trailing_origin.y + reach),
        )
    } else {
        (
            trailing_origin,
            Point::new(trailing_origin.x + reach, trailing_origin.y),
        )
    };
    scene.fill_rect_brush(
        trailing_origin,
        leading_size,
        &Brush::Gradient(axis(trail_from, trail_to, clear, surface)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent, PointerPhase};
    use frust::{SizedBox, any};
    use frust_core::{BuildCtx, EventCtx, PaintCtx};
    use std::any::Any;

    /// The box every paint test lays its marquee into.
    const BOX: Size = Size::new(100.0, 40.0);
    /// Each test item's box: two of them plus two gaps make a 100px track.
    const ITEM: Size = Size::new(40.0, 20.0);
    /// The gap the test track is spaced with.
    const GAP: f64 = 10.0;

    /// Records the translations painted copies were pushed under, and the
    /// brushes any edge fade was drawn with.
    #[derive(Default)]
    struct Recorder {
        transforms: Vec<Affine>,
        fades: Vec<Brush>,
        clips: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn push_clip(&mut self, _origin: Point, _size: Size) {
            self.clips += 1;
        }
        fn fill_rect_brush(&mut self, _origin: Point, _size: Size, brush: &Brush) {
            self.fades.push(brush.clone());
        }
    }

    fn items() -> Vec<frust::AnyView<()>> {
        vec![
            any(SizedBox::<()>(Some(ITEM.width), Some(ITEM.height))),
            any(SizedBox::<()>(Some(ITEM.width), Some(ITEM.height))),
        ]
    }

    fn laid_out(view: &MarqueeView<()>) -> MarqueeWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::tight(BOX));
        widget
    }

    fn painted(
        widget: &mut MarqueeWidget,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool, Option<Duration>) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame(), ctx.paced_interval())
    }

    fn send(widget: &mut MarqueeWidget, phase: PointerPhase, x: f64, y: f64) -> bool {
        let mut state = ();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, BOX);
        widget.event(
            &mut ctx,
            &InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(x, y),
                button: PointerButton::Primary,
            }),
        );
        ctx.needs_redraw()
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// The track is the items plus a gap after each — the trailing one is the
    /// seam, and dropping it is what makes a loop visibly stutter.
    #[test]
    fn the_track_carries_a_gap_after_every_item_including_the_seam() {
        let widget = laid_out(&marquee(items()).gap(GAP));
        assert_eq!(
            widget.track_length(),
            (ITEM.width + GAP) * 2.0,
            "two items, two gaps"
        );
        // A gapless marquee is just the items.
        let flush = laid_out(&marquee(items()).gap(0.0));
        assert_eq!(flush.track_length(), ITEM.width * 2.0);
    }

    /// The loop wraps: one full traversal returns the offset to its start, and
    /// nothing in between leaves `0..track`.
    #[test]
    fn the_offset_wraps_once_per_speed() {
        let mut widget = laid_out(&marquee(items()).gap(GAP).speed(Duration::from_millis(1000)));
        // The first paint only establishes the clock.
        painted(&mut widget, 0, None);
        assert_eq!(widget.offset(), 0.0);

        painted(&mut widget, 250, None);
        assert!(
            (widget.offset() - 25.0).abs() < 1e-6,
            "a quarter of a 100px track: {}",
            widget.offset()
        );

        // A full traversal from there returns to exactly the same place —
        // which is what "seamless" means.
        painted(&mut widget, 1250, None);
        assert!(
            (widget.offset() - 25.0).abs() < 1e-6,
            "a full traversal did not return to its start: {}",
            widget.offset()
        );

        // And it never leaves the track at any point in between.
        for ms in 0..40 {
            painted(&mut widget, 1300 + ms * 37, None);
            assert!((0.0..widget.track_length()).contains(&widget.offset()));
        }
    }

    /// Enough copies are painted to cover the box plus the one arriving, all
    /// inside a single clip.
    #[test]
    fn the_track_is_painted_enough_times_to_fill_the_box() {
        let mut widget = laid_out(&marquee(items()).gap(GAP));
        let (recorder, _, _) = painted(&mut widget, 0, None);
        // A 100px track in a 100px box: the visible one plus its successor.
        assert_eq!(recorder.transforms.len(), 2);
        assert_eq!(recorder.clips, 1, "overflow: hidden");

        // A short track needs more copies, not a stretched one.
        let mut short = laid_out(&marquee((SizedBox::<()>(Some(20.0), Some(20.0)),)).gap(0.0));
        let (recorder, _, _) = painted(&mut short, 0, None);
        assert_eq!(recorder.transforms.len(), 6, "five to fill 100px, plus one");
    }

    /// Each direction travels the way its name says, and the vertical pair runs
    /// down the box rather than across it.
    #[test]
    fn every_direction_is_constructible_and_travels_its_own_way() {
        assert_eq!(MarqueeDirection::ALL.len(), 4);
        for direction in MarqueeDirection::ALL {
            let mut widget = laid_out(&marquee(items()).gap(GAP).direction(direction));
            painted(&mut widget, 0, None);
            painted(&mut widget, 100, None);
            let first = widget.first_copy();
            if direction.is_reversed() {
                assert!(
                    first < 0.0 && first > -widget.track_length(),
                    "{direction:?}"
                );
            } else {
                assert!(first <= 0.0, "{direction:?}");
            }
        }
        assert!(MarqueeDirection::Up.is_vertical());
        assert!(MarqueeDirection::Down.is_vertical());
        assert!(!MarqueeDirection::Left.is_vertical());
        assert!(MarqueeDirection::Right.is_reversed());
        assert!(MarqueeDirection::Down.is_reversed());
    }

    /// A vertical marquee lays its items down the box and runs its track along
    /// the height.
    #[test]
    fn a_vertical_marquee_stacks_its_items() {
        let widget = laid_out(&marquee(items()).gap(GAP).direction(MarqueeDirection::Up));
        assert_eq!(widget.track_length(), (ITEM.height + GAP) * 2.0);
    }

    /// A hover pauses the scroll; the offset does not move while it is held.
    #[test]
    fn hovering_pauses_the_scroll() {
        let mut widget = laid_out(&marquee(items()).gap(GAP).speed(Duration::from_millis(1000)));
        painted(&mut widget, 0, None);
        painted(&mut widget, 250, None);
        let held = widget.offset();
        assert!(held > 0.0);

        // A move inside claims hover, and a paused advance holds the offset.
        assert!(send(&mut widget, PointerPhase::Move, 10.0, 10.0));
        assert!(widget.is_paused());
        widget.advance(FrameTime::from_nanos(500_000_000), true);
        assert_eq!(widget.offset(), held, "the scroll moved while paused");

        // Turning the affordance off means the same hover pauses nothing.
        let mut unpausable = laid_out(&marquee(items()).gap(GAP).pause_on_hover(false));
        send(&mut unpausable, PointerPhase::Move, 10.0, 10.0);
        assert!(!unpausable.is_paused());
    }

    /// A press is not a hover: the settled rule this component had to choose,
    /// because the framework's hover link ends at a `Down`.
    #[test]
    fn a_press_does_not_pause_the_marquee() {
        let mut widget = laid_out(&marquee(items()).gap(GAP));
        assert!(!send(&mut widget, PointerPhase::Down, 10.0, 10.0));
        assert!(!widget.is_paused(), "a press paused the scroll");

        // Nor does a release, and neither disturbs a hover already established.
        send(&mut widget, PointerPhase::Move, 10.0, 10.0);
        assert!(widget.is_paused());
        send(&mut widget, PointerPhase::Up, 10.0, 10.0);
        assert!(widget.is_paused(), "an up cleared a live hover");
    }

    /// A move that lands outside the box releases the pause, and the paint-time
    /// sync is authoritative over any latched flag.
    #[test]
    fn leaving_releases_the_pause_and_paint_has_the_last_word() {
        let mut widget = laid_out(&marquee(items()).gap(GAP));
        send(&mut widget, PointerPhase::Move, 10.0, 10.0);
        assert!(widget.is_paused());
        send(&mut widget, PointerPhase::Move, 500.0, 10.0);
        assert!(!widget.is_paused(), "a move outside did not release");

        // `PaintCtx::is_hovered` corrects a latch no event could — the only
        // thing that catches a pointer that left without another event.
        send(&mut widget, PointerPhase::Move, 10.0, 10.0);
        assert!(widget.is_paused());
        painted(&mut widget, 0, None);
        assert!(!widget.is_paused(), "paint did not re-sync the latch");
    }

    /// The marquee asks for a paced cosmetic frame at its own cadence — one
    /// frame per pixel of travel, clamped at both ends.
    #[test]
    fn the_frame_cadence_is_one_frame_per_pixel_within_its_clamps() {
        let slow = laid_out(&marquee(items()).gap(GAP).speed(Duration::from_secs(30)));
        // 100px over 30s is 300ms/px, past the ceiling.
        assert_eq!(slow.frame_interval(), MAX_FRAME_INTERVAL);

        let measured = laid_out(&marquee(items()).gap(GAP).speed(Duration::from_millis(1000)));
        // 100px over 1s is 10ms/px, inside the band.
        assert_eq!(measured.frame_interval(), Duration::from_millis(10));

        let fast = laid_out(&marquee(items()).gap(GAP).speed(Duration::from_millis(100)));
        assert_eq!(fast.frame_interval(), MIN_FRAME_INTERVAL);

        let mut widget = laid_out(&marquee(items()).gap(GAP).speed(Duration::from_millis(1000)));
        let (_, needs_frame, paced) = painted(&mut widget, 0, None);
        assert!(needs_frame);
        assert_eq!(paced, Some(Duration::from_millis(10)));
    }

    /// A near-zero track (children whose combined size collapses toward zero)
    /// is floored, and the paint-copy count stays capped rather than scaling
    /// with the box extent.
    #[test]
    fn a_near_zero_track_is_floored_and_its_paint_copies_are_capped() {
        let mut widget = laid_out(&marquee((SizedBox::<()>(Some(0.01), Some(20.0)),)).gap(0.0));
        assert_eq!(
            widget.track_length(),
            MIN_TRACK_LENGTH,
            "a near-zero track is floored"
        );
        let (recorder, _, _) = painted(&mut widget, 0, None);
        assert_eq!(
            recorder.transforms.len(),
            MAX_TRACK_COPIES,
            "the copy count stays capped rather than scaling with the box extent"
        );
    }

    /// An extreme speed against a floored track does not overflow
    /// `Duration::from_secs_f64` — it clamps into the same band as any other
    /// cadence instead of panicking.
    #[test]
    fn an_extreme_speed_track_ratio_clamps_instead_of_panicking() {
        let widget = laid_out(
            &marquee((SizedBox::<()>(Some(0.01), Some(20.0)),))
                .gap(0.0)
                .speed(Duration::MAX),
        );
        assert_eq!(widget.frame_interval(), MAX_FRAME_INTERVAL);
    }

    /// `reduce_motion` stops the scroll outright and asks for nothing further —
    /// a scrolling banner is exactly the motion the flag is about.
    #[test]
    fn reduce_motion_stops_the_marquee() {
        let theme = reduced();
        let mut widget = laid_out(&marquee(items()).gap(GAP).speed(Duration::from_millis(1000)));
        painted(&mut widget, 0, Some(&theme));
        let (recorder, needs_frame, _) = painted(&mut widget, 500, Some(&theme));
        assert!(!needs_frame);
        assert_eq!(widget.offset(), 0.0);
        assert!(!recorder.transforms.is_empty(), "the track still paints");
    }

    /// The edge fade is off by default and paints two gradients when asked —
    /// see the module docs on why the default differs from upstream's.
    #[test]
    fn the_edge_fade_is_opt_in_and_paints_both_ends() {
        let mut plain = laid_out(&marquee(items()).gap(GAP));
        let (recorder, _, _) = painted(&mut plain, 0, None);
        assert!(recorder.fades.is_empty(), "fade is off by default");

        let mut faded = laid_out(&marquee(items()).gap(GAP).fade(true));
        let (recorder, _, _) = painted(&mut faded, 0, None);
        assert_eq!(recorder.fades.len(), 2, "one gradient per end");
        for brush in &recorder.fades {
            assert!(matches!(brush, Brush::Gradient(_)));
        }
    }

    /// An empty marquee is inert rather than a division by zero.
    #[test]
    fn an_empty_marquee_is_inert() {
        let mut widget = laid_out(&marquee::<(), _>(Vec::<AnyView<()>>::new()));
        assert_eq!(widget.track_length(), 0.0);
        let (_, needs_frame, _) = painted(&mut widget, 0, None);
        assert!(!needs_frame);
        painted(&mut widget, 1000, None);
        assert_eq!(widget.offset(), 0.0);
    }
}
