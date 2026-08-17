//! [`tooltip`]/[`TooltipView`]: a transparent long-press-to-peek label bubble
//! wrapper — the Glyph design system's light-page `.tooltip-bubble`
//! rule (retrieved 2026-07-21): a small dark bubble, centered 8px above the
//! wrapped child, shown while the child is held past the shared long-press
//! threshold and dismissed on release. Like `crate::term_block`, every
//! painted color here is a [`GlyphInk`] field or a literal fallback constant
//! — never a brightness-swapped `ColorScheme` role ("a
//! tooltip is a floating overlay, not page content"). The design system's
//! own light-page CSS override for `.tooltip-bubble` (border/shadow spelled
//! out as literal `rgba(255,255,255,.1)`/`rgba(34,29,18,.25)`, distinct from
//! the dark page's `var(--border-bright)`/black-shadow rule) is exactly what
//! this module's [`TOOLTIP_BORDER`]/[`TOOLTIP_SHADOW_BASE`] pin — the same
//! "light source's literal dark-ink swatches" precedent
//! [`GlyphInk`]'s own module docs cite.
//!
//! # Long-press mechanics
//!
//! Mirrors `crate::gesture::GestureDetectorWidget`'s paint-clock long-press
//! idiom (see that module's doc comment for the full rationale): a press
//! records its start [`frust::FrameTime`] on the first paint after
//! `Down` and marks itself elapsed once held past [`LONG_PRESS_MS`] (this
//! widget's own constant — duplicated rather than shared, since
//! `crate::gesture::LONG_PRESS_MS` is private; both carry the identical
//! 500ms community-approximate value). Unlike `GestureDetector`, `Tooltip`
//! has no app callback to fire, so its paint-clock timer flips `self.phase`
//! directly from `paint` the instant it elapses — no need to wait for a
//! subsequent event the way a callback-firing gesture does. Move-past-slop
//! detection reuses the shared [`frust::input::TOUCH_SLOP`] constant.
//!
//! # Show / dismiss timeline
//!
//! `Down` arms the press timer. Once it elapses **while still held**, the
//! bubble begins a 150ms `effects`-eased fade-in ([`ENTER_DURATION`]/
//! [`ENTER_CURVE`] — `durations.fast`/`easing.effects`, `MotionScheme::glyph`'s
//! exact source values). `Up` (release) or a drag past [`frust::input::TOUCH_SLOP`]
//! begins a 100ms `exit`-eased fade-out ([`EXIT_DURATION`]/[`EXIT_CURVE`] —
//! `durations.instant`/`easing.exit`). `Cancel` clears all internal state
//! immediately with no animation, and — per the interaction-semantics
//! contract — never touches app state; there is none to touch (`Tooltip` has
//! no callback at all). `reduce_motion` collapses both fades to a
//! zero-duration snap (matching `frust::MotionScheme::reduce_motion`'s
//! contract elsewhere in this catalog — e.g. `crate::skeleton`'s
//! sweep going static).
//!
//! # Positioning (v1 limitation)
//!
//! The bubble paints centered horizontally above the child at a fixed 8px
//! gap, unclamped to any ancestor's bounds — the layout protocol gives a
//! widget no visibility into ancestor bounds to clamp against, so a tooltip
//! near a screen edge can paint partially off-screen. Documented, not fixed,
//! as a v1 limitation: above-center placement only, no ancestor-bounds
//! clamping.
//!
//! # Why this widget doesn't nest `Text` for the bubble
//!
//! See `crate::badge`'s module docs — the bubble needs the
//! [`GlyphInk`] `tooltip_fg` color, which `TextView`'s four fixed
//! `ThemeTextColor` roles don't cover, so this widget shapes/paints its own
//! run directly via `frust_text` (mirrors `crate::alert`/
//! `crate::toast`).
//!
//! # Semantics
//!
//! A `Role::Group` container node wraps the child, its `description` set to
//! the tooltip text — a best-effort described-by-style augmentation (limited
//! to where the semantics vocabulary allows). This is
//! always attached, not only while the bubble is visually shown, matching
//! how a platform `aria-describedby` relationship is a static accessibility
//! fact independent of the tooltip's current visibility.

use std::time::Duration;

use crate::GlyphInk;
use frust::Theme;
use frust::authoring::Role;
use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, LineHeight, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust::input::TOUCH_SLOP;
use frust::{Curve, FrameTime};
use kurbo::{Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use frust::Timing;
use frust::{TransitionDriver, make_driver};

// ---- Long-press timing ----------------------------------------------------

/// The press duration after which a held press reveals the tooltip.
///
/// **Community-approximate**, matching `crate::gesture::LONG_PRESS_MS`'s
/// identical 500ms value and rationale (that module's doc comment has the
/// full citation) — duplicated here since that constant is private.
const LONG_PRESS_MS: f64 = 500.0;

// ---- Bubble geometry (`.tooltip-bubble` design-system values) -------------

/// Gap between the child's top edge and the bubble's bottom edge
/// (`bottom:calc(100% + 8px)`).
const GAP_ABOVE: f64 = 8.0;
/// Horizontal bubble padding (`padding:6px 10px`).
const PAD_X: f64 = 10.0;
/// Vertical bubble padding (`padding:6px 10px`).
const PAD_Y: f64 = 6.0;
/// Bubble font size (`.tooltip-bubble{font-size:10.5px}`).
const FONT_SIZE: f32 = 10.5;
/// Single-line label height multiplier — no line-height is specified for a
/// `white-space:nowrap` single-line bubble; an original, hand-picked value.
const LINE_HEIGHT: f32 = 1.3;
/// Border stroke width.
const BORDER_WIDTH: f64 = 1.0;
/// Corner-rounding tolerance for the border stroke (matches
/// `crate::badge`'s constant of the same name).
const PATH_TOLERANCE: f64 = 0.1;

/// Unthemed corner-radius fallback — Glyph's canonical `--radius-sm` (6px).
const FALLBACK_RADIUS: f64 = 6.0;

// ---- Fixed (never brightness-swapped) border/shadow — see the module docs -

/// `rgba(255,255,255,.1)` — the light page's literal `.tooltip-bubble`
/// border override.
const TOOLTIP_BORDER: Color = Color::new([1.0, 1.0, 1.0, 0.1]);
/// `box-shadow:0 8px 20px rgba(34,29,18,.25)` — the light page's literal
/// override (a softer warm-ink shadow than the dark page's black one; fixed
/// regardless of the active brightness, per the module docs).
const TOOLTIP_SHADOW_Y: f64 = 8.0;
const TOOLTIP_SHADOW_BLUR: f64 = 20.0;
const TOOLTIP_SHADOW_ALPHA: f32 = 0.25;
const TOOLTIP_SHADOW_BASE: Color = Color::from_rgb8(0x22, 0x1d, 0x12);

// ---- Motion (design-system values — see the module docs) -----------------

/// Enter duration (`durations.fast`, exact Glyph source value).
const ENTER_DURATION: Duration = Duration::from_millis(150);
/// Enter easing (Glyph's `effects` cubic-bezier, exact source value).
const ENTER_CURVE: Curve = Curve::Cubic(0.16, 1.0, 0.3, 1.0);
/// Exit duration (`durations.instant`, exact Glyph source value).
const EXIT_DURATION: Duration = Duration::from_millis(100);
/// Exit easing (Glyph's `exit` cubic-bezier, exact source value).
const EXIT_CURVE: Curve = Curve::Cubic(0.4, 0.0, 1.0, 1.0);
/// `reduce_motion`'s collapsed snap: zero duration, so the fade completes on
/// its very first `advance` (mirrors `crate::skeleton`'s
/// sweep-goes-static contract).
const REDUCE_MOTION_DURATION: Duration = Duration::ZERO;

/// The `(enter, exit)` [`Timing`]s for the current `reduce_motion` state.
fn resolve_timings(reduce_motion: bool) -> (Timing, Timing) {
    if reduce_motion {
        let t = Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear);
        (t, t)
    } else {
        (
            Timing::Duration(ENTER_DURATION, ENTER_CURVE),
            Timing::Duration(EXIT_DURATION, EXIT_CURVE),
        )
    }
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors the
/// per-module helper of the same shape used across `frust-widgets`).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// `GlyphInk`, themed (`theme.extension::<GlyphInk>()`) or the unthemed
/// [`GlyphInk::default_ink`] fallback.
fn resolve_ink(theme: Option<&Theme>) -> GlyphInk {
    theme
        .and_then(|t| t.extension::<GlyphInk>().copied())
        .unwrap_or_else(GlyphInk::default_ink)
}

fn bubble_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::REGULAR,
        line_height: LineHeight::FontSizeRelative(LINE_HEIGHT),
        ..TextStyle::new(FONT_SIZE, color)
    }
}

/// A minimal retained text run — see `crate::badge`'s `GlyphLabel` for
/// the full shape/rationale (duplicated per this crate's existing
/// small-helper convention).
struct GlyphLabel {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
    laid_out_max_width: Option<f32>,
}

impl GlyphLabel {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
            laid_out_max_width: None,
        }
    }

    fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: Option<f32>) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_out_max_width == max_width
            && self.laid_out_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, max_width);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_out_style = Some(style.clone());
        self.laid_out_max_width = max_width;
        size
    }

    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// A declarative tooltip wrapper. See the [module docs](self).
pub struct TooltipView<State: 'static> {
    child: AnyView<State>,
    text: String,
}

/// Wrap `child` in a long-press-to-peek tooltip showing `text`.
pub fn tooltip<State: 'static, V: View<State>>(
    child: V,
    text: impl Into<String>,
) -> TooltipView<State> {
    TooltipView {
        child: any(child),
        text: text.into(),
    }
}

/// PascalCase alias for [`tooltip`], matching the widget-fn vocabulary
/// (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn Tooltip<State: 'static, V: View<State>>(
    child: V,
    text: impl Into<String>,
) -> TooltipView<State> {
    tooltip(child, text)
}

impl<State: 'static> View<State> for TooltipView<State> {
    type Element = TooltipWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TooltipWidget {
        TooltipWidget {
            child: frust::authoring::build_child(&self.child, ctx),
            text: self.text.clone(),
            bubble: GlyphLabel::new(self.text.clone()),
            bubble_size: Size::ZERO,
            press: PressState::Idle,
            phase: TooltipPhase::Hidden,
            driver: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TooltipWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags =
            frust::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        if prev.text != self.text {
            element.text = self.text.clone();
            element.bubble.set_content(self.text.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut TooltipWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

/// The press recognizer's state machine — mirrors
/// `crate::gesture::GestureDetectorWidget`'s `Recognizer` (see that module's
/// doc comment), minus the `Fired` state: `Tooltip` never needs to wait for a
/// subsequent event to act on an elapsed press (see the module docs).
#[derive(Clone, Copy)]
enum PressState {
    Idle,
    Pressed {
        down_pos: Point,
        press_start: Option<FrameTime>,
        elapsed: bool,
    },
    Dragged,
}

/// [`TooltipWidget`]'s show/dismiss lifecycle phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TooltipPhase {
    Hidden,
    Entering,
    Shown,
    Exiting,
}

/// The retained widget for a [`TooltipView`]. See the [module docs](self).
pub struct TooltipWidget {
    child: ChildPod,
    text: String,
    bubble: GlyphLabel,
    bubble_size: Size,
    press: PressState,
    phase: TooltipPhase,
    /// The active enter/exit progress driver — `None` while `Hidden`/`Shown`
    /// (no motion in flight) and lazily (re)built the first time
    /// [`TooltipWidget::advance`] sees a fresh `Entering`/`Exiting` phase.
    driver: Option<TransitionDriver>,
}

impl TooltipWidget {
    /// Begin dismissing (`Entering`/`Shown` -> `Exiting`) if not already
    /// `Hidden`/`Exiting` — shared by the `Up` and drag-past-slop paths.
    fn begin_exit(&mut self) {
        if matches!(self.phase, TooltipPhase::Entering | TooltipPhase::Shown) {
            self.phase = TooltipPhase::Exiting;
            self.driver = None;
        }
    }

    /// Advance the long-press timer and the show/dismiss phase machine to
    /// frame time `now`, returning the bubble's current alpha (`0.0` while
    /// `Hidden`). Factored out of `paint` so the full press -> enter -> exit
    /// timeline is drivable with synthetic `FrameTime`s in a unit test
    /// (mirrors `ToastHostWidget::advance`'s rationale).
    fn advance(&mut self, now: FrameTime, enter: Timing, exit: Timing) -> f64 {
        if let PressState::Pressed {
            press_start,
            elapsed,
            ..
        } = &mut self.press
            && !*elapsed
        {
            let start = *press_start.get_or_insert(now);
            if now.saturating_sub(start).as_secs_f64() * 1000.0 >= LONG_PRESS_MS {
                *elapsed = true;
                if self.phase == TooltipPhase::Hidden {
                    self.phase = TooltipPhase::Entering;
                    self.driver = None;
                }
            }
        }
        match self.phase {
            TooltipPhase::Hidden => 0.0,
            TooltipPhase::Entering => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(enter).0)
                    .advance(now);
                if adv.done {
                    self.phase = TooltipPhase::Shown;
                    self.driver = None;
                    1.0
                } else {
                    adv.value.clamp(0.0, 1.0)
                }
            }
            TooltipPhase::Shown => 1.0,
            TooltipPhase::Exiting => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(exit).0)
                    .advance(now);
                let alpha = 1.0 - adv.value.clamp(0.0, 1.0);
                if adv.done {
                    self.phase = TooltipPhase::Hidden;
                    self.driver = None;
                }
                alpha
            }
        }
    }

    fn paint_bubble(
        &self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        alpha: f32,
        ink: &GlyphInk,
        radius: f64,
    ) {
        if alpha <= 0.0 {
            return;
        }
        let origin = ctx.origin();
        let size = ctx.size();
        let bw = self.bubble_size.width + PAD_X * 2.0;
        let bh = self.bubble_size.height + PAD_Y * 2.0;
        let bx = origin.x + (size.width - bw) / 2.0;
        let by = origin.y - bh - GAP_ABOVE;
        let bubble_origin = Point::new(bx, by);
        let bubble_size = Size::new(bw, bh);

        scene.push_layer(bubble_origin, bubble_size, alpha);
        scene.draw_shadow(
            Point::new(bx, by + TOOLTIP_SHADOW_Y),
            bubble_size,
            radius,
            TOOLTIP_SHADOW_BLUR,
            with_alpha(TOOLTIP_SHADOW_BASE, TOOLTIP_SHADOW_ALPHA),
        );
        scene.fill_rounded_rect(bubble_origin, bubble_size, radius, ink.tooltip_bg);
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, bubble_size), radius);
        let path = rr.to_path(PATH_TOLERANCE);
        scene.stroke_path(
            bubble_origin,
            &path,
            BORDER_WIDTH,
            &Brush::Solid(TOOLTIP_BORDER),
        );
        self.bubble.paint(Point::new(bx + PAD_X, by + PAD_Y), scene);
        scene.pop_layer();
    }
}

impl Widget for TooltipWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let ink = resolve_ink(theme);
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        self.bubble_size = self.bubble.layout(ctx, &bubble_style(ink.tooltip_fg), None);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);

        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let (enter, exit) = resolve_timings(reduce_motion);
        let ink = resolve_ink(theme);
        let radius = theme.map(|t| t.shape.small).unwrap_or(FALLBACK_RADIUS);
        let now = ctx.frame_time();

        let alpha = self.advance(now, enter, exit);
        self.paint_bubble(ctx, scene, alpha as f32, &ink, radius);

        let waiting_on_timer = matches!(self.press, PressState::Pressed { elapsed: false, .. });
        if waiting_on_timer || matches!(self.phase, TooltipPhase::Entering | TooltipPhase::Exiting)
        {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event {
            match p.phase {
                PointerPhase::Down => {
                    self.press = PressState::Pressed {
                        down_pos: p.position,
                        press_start: None,
                        elapsed: false,
                    };
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    self.child.event_child(ctx, event);
                    return EventResult::Handled;
                }
                PointerPhase::Move => {
                    if let PressState::Pressed { down_pos, .. } = self.press
                        && (p.position - down_pos).hypot() > TOUCH_SLOP
                    {
                        self.press = PressState::Dragged;
                        self.begin_exit();
                    }
                    self.child.event_child(ctx, event);
                    return EventResult::Handled;
                }
                PointerPhase::Up => {
                    self.press = PressState::Idle;
                    self.child.event_child(ctx, event);
                    self.child.set_active(false);
                    self.begin_exit();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                PointerPhase::Cancel => {
                    // Cancel only clears internal flags and dismisses with no
                    // animation — never touches app state (there is none:
                    // `Tooltip` has no callback at all).
                    self.press = PressState::Idle;
                    self.phase = TooltipPhase::Hidden;
                    self.driver = None;
                    self.child.event_child(ctx, event);
                    self.child.set_active(false);
                    return EventResult::Handled;
                }
            }
        }
        self.child.event_child(ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let text = self.text.clone();
        ctx.push_container(
            Role::Group,
            move |node| node.set_description(text.as_str()),
            |ctx| self.child.semantics_child(ctx),
        );
    }

    frust::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BuildCtx, EventCtx, PointerButton, PointerEvent};
    use frust_core::RenderRoot;
    use std::any::Any;

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// A trivial full-bleed child that paints/contributes nothing — keeps the
    /// bubble-focused rendering assertions below free of an unrelated themed
    /// child's own colors (mirrors `crate::gesture`'s test-only `Blank`).
    struct Blank;
    struct BlankWidget;
    impl View<()> for Blank {
        type Element = BlankWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> BlankWidget {
            BlankWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut BlankWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for BlankWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(24.0, 24.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    fn build_tooltip(text: &str) -> TooltipWidget {
        let view: TooltipView<()> = tooltip(Blank, text);
        let mut counter = 0u64;
        <TooltipView<()> as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut TooltipWidget, event: &InputEvent) {
        let mut state = ();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 40.0));
        w.event(&mut ctx, event);
    }

    // ---- Long-press timeline ----------------------------------------------

    #[test]
    fn quick_tap_never_shows_the_bubble() {
        let mut w = build_tooltip("Server settings");
        let (enter, exit) = resolve_timings(false);
        dispatch(&mut w, &ev(PointerPhase::Down, 10.0, 10.0));
        let a0 = w.advance(ft_ms(0.0), enter, exit);
        assert_eq!(a0, 0.0);
        dispatch(&mut w, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(w.phase, TooltipPhase::Hidden);
        let a1 = w.advance(ft_ms(50.0), enter, exit);
        assert_eq!(a1, 0.0);
    }

    #[test]
    fn hold_past_threshold_shows_then_release_hides() {
        let mut w = build_tooltip("Server settings");
        let (enter, exit) = resolve_timings(false);

        dispatch(&mut w, &ev(PointerPhase::Down, 10.0, 10.0));
        w.advance(ft_ms(0.0), enter, exit); // seed the press clock
        w.advance(ft_ms(LONG_PRESS_MS), enter, exit); // crosses the threshold
        assert_eq!(w.phase, TooltipPhase::Entering);

        let shown = w.advance(ft_ms(LONG_PRESS_MS + 200.0), enter, exit); // past the 150ms enter
        assert_eq!(shown, 1.0);
        assert_eq!(w.phase, TooltipPhase::Shown);

        dispatch(&mut w, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(w.phase, TooltipPhase::Exiting);

        let seed = w.advance(ft_ms(LONG_PRESS_MS + 200.0 + 10.0), enter, exit); // seeds the exit driver
        assert!((seed - 1.0).abs() < 1e-6, "exit starts fully shown");

        let hidden = w.advance(ft_ms(LONG_PRESS_MS + 200.0 + 10.0 + 200.0), enter, exit); // past the 100ms exit
        assert_eq!(hidden, 0.0);
        assert_eq!(w.phase, TooltipPhase::Hidden);
    }

    #[test]
    fn cancel_hides_immediately_with_no_animation() {
        let mut w = build_tooltip("Server settings");
        let (enter, exit) = resolve_timings(false);
        dispatch(&mut w, &ev(PointerPhase::Down, 10.0, 10.0));
        w.advance(ft_ms(0.0), enter, exit);
        w.advance(ft_ms(LONG_PRESS_MS), enter, exit);
        assert_eq!(w.phase, TooltipPhase::Entering);

        // A Cancel over a throwaway `()` state — this only ever touches
        // internal fields (`press`/`phase`/`driver`), never `EventCtx::state_mut`,
        // which would panic on the `()` downcast if it tried.
        dispatch(&mut w, &ev(PointerPhase::Cancel, 10.0, 10.0));
        assert_eq!(w.phase, TooltipPhase::Hidden);
        assert!(matches!(w.press, PressState::Idle));
        assert!(w.driver.is_none());
    }

    #[test]
    fn drag_past_slop_dismisses_a_shown_bubble() {
        let mut w = build_tooltip("Server settings");
        let (enter, exit) = resolve_timings(false);
        dispatch(&mut w, &ev(PointerPhase::Down, 10.0, 10.0));
        w.advance(ft_ms(0.0), enter, exit); // seed the press clock
        w.advance(ft_ms(LONG_PRESS_MS), enter, exit); // crosses the threshold, seeds the enter driver
        w.advance(ft_ms(LONG_PRESS_MS + 200.0), enter, exit); // settles past the 150ms enter
        assert_eq!(w.phase, TooltipPhase::Shown);

        dispatch(
            &mut w,
            &ev(PointerPhase::Move, 10.0 + TOUCH_SLOP + 5.0, 10.0),
        );
        assert_eq!(w.phase, TooltipPhase::Exiting);
    }

    #[test]
    fn reduce_motion_uses_a_zero_duration_snap() {
        let (enter, exit) = resolve_timings(true);
        assert_eq!(
            enter,
            Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear)
        );
        assert_eq!(
            exit,
            Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear)
        );
    }

    // ---- Rendering ---------------------------------------------------------

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        shadows: Vec<Color>,
        glyph_colors: Vec<Color>,
        layers: Vec<(Point, Size, f32)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _path: &kurbo::BezPath, _width: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _radius: f64, _blur: f64, color: Color) {
            self.shadows.push(color);
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
        fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
            self.layers.push((origin, size, alpha));
        }
        fn pop_layer(&mut self) {}
    }

    /// Layout + force `Shown` (bypassing the press timer) + paint, returning
    /// the recorded scene.
    fn shown_layout_and_paint(text: &str, theme: Option<&Theme>) -> (TooltipWidget, Recorder) {
        let mut w = build_tooltip(text);
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 60.0)));
        w.phase = TooltipPhase::Shown;
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::new(0.0, 60.0), size).with_theme(t),
            None => PaintCtx::new(Point::new(0.0, 60.0), size),
        };
        w.paint(&mut pctx, &mut rec);
        (w, rec)
    }

    #[test]
    fn unthemed_bubble_uses_glyph_ink_default_and_fixed_border() {
        let (_w, rec) = shown_layout_and_paint("Server settings", None);
        let ink = GlyphInk::default_ink();
        assert_eq!(rec.rrects[0].3, ink.tooltip_bg);
        assert_eq!(rec.strokes[0], TOOLTIP_BORDER);
        assert_eq!(rec.glyph_colors[0], ink.tooltip_fg);
        assert_eq!(rec.layers[0].2, 1.0, "Shown paints at full alpha");
    }

    #[test]
    fn brightness_invariance_recording_scene_identical_dark_vs_light() {
        // Identical paint under Light and Dark
        // glyph_baseline() brightness.
        let dark = crate::baseline();
        let light = dark.clone().with_brightness(frust::Brightness::Light);

        let (_wd, rec_dark) = shown_layout_and_paint("Server settings", Some(&dark));
        let (_wl, rec_light) = shown_layout_and_paint("Server settings", Some(&light));

        assert_eq!(rec_dark.rrects, rec_light.rrects);
        assert_eq!(rec_dark.strokes, rec_light.strokes);
        assert_eq!(rec_dark.shadows, rec_light.shadows);
        assert_eq!(rec_dark.glyph_colors, rec_light.glyph_colors);
    }

    #[test]
    fn themed_ink_resolves_from_glyph_baseline_extension() {
        let theme = crate::baseline();
        let (_w, rec) = shown_layout_and_paint("Server settings", Some(&theme));
        let ink = theme.extension::<GlyphInk>().unwrap();
        assert_eq!(rec.rrects[0].3, ink.tooltip_bg);
        assert_eq!(rec.glyph_colors[0], ink.tooltip_fg);
    }

    #[test]
    fn hidden_phase_paints_no_bubble() {
        let mut w = build_tooltip("Server settings");
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 60.0)));
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::new(Point::new(0.0, 60.0), size);
        w.paint(&mut ctx, &mut rec);
        assert!(rec.rrects.is_empty());
        assert!(rec.layers.is_empty());
        assert!(!ctx.needs_frame());
    }

    // ---- Semantics ---------------------------------------------------------

    #[test]
    fn semantics_attaches_tooltip_text_as_a_group_description() {
        fn logic(_s: &mut ()) -> TooltipView<()> {
            tooltip(Blank, "Server settings")
        }
        let mut root: RenderRoot<(), TooltipView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(200.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("tooltip contributes a Role::Group node");
        assert_eq!(node.description(), Some("Server settings"));
    }
}
