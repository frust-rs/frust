//! Ports beUI's **Switch** — the toggle whose thumb springs across the track
//! and squashes toward wherever it is about to go.
//!
//! Source: `components/motion/switch.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `switch`: *"Toggle with a spring-driven thumb and press feedback."*
//!
//! | class / prop | here |
//! |---|---|
//! | `h-7 w-12 px-1` | [`SWITCH_TRACK_WIDTH`] x [`SWITCH_TRACK_HEIGHT`], [`SWITCH_TRACK_PADDING`] |
//! | thumb `h-5 w-5 rounded-full bg-background shadow-md` | [`SWITCH_THUMB_SIZE`], a pill on `surface` |
//! | `rounded-full` | [`style::RADIUS_CONTROL`], resolved by [`style::resolve_radius`] |
//! | `checked ? bg-primary : bg-muted-foreground/60` | the track fill, crossfaded |
//! | `transition-colors duration-200` | [`TRACK_COLOR_RAMP`], the ramp form of `TIMING_COLORS` |
//! | `layout` + `THUMB_SPRING` | [`SWITCH_THUMB_SPRING`] driving the travel |
//! | `animate={{ scale: squish ? 0.9 : 1 }}` + `ml-1`/`mr-1` | [`SWITCH_SQUISH_SCALE`] + [`SWITCH_STRETCH`] |
//! | `x: [0, -2, 2, -1, 0]` on a disabled press | [`SWITCH_REFUSAL_KEYFRAMES`] |
//! | `focus-visible:ring-2 ring-ring ring-offset-2` | [`style::FOCUS_RING_WIDTH`] at [`SWITCH_RING_OFFSET`] |
//! | `disabled:opacity-60 disabled:cursor-not-allowed` | [`DISABLED_OPACITY`] + [`style::DISABLED_CURSOR`] |
//!
//! # The signature: squash *and* stretch
//!
//! Upstream's thumb is a flex child that both scales down (`scale: 0.9`) and
//! *grows a margin on the side it is about to travel toward* (`ml-1` when
//! checked, `mr-1` when not) while a pointer holds it. The margin widens the
//! thumb's own box by one spacing step, so the thumb leans in the direction of
//! its next journey rather than merely shrinking — that lean is what this
//! catalog is recognised for, so it is ported as geometry ([`SWITCH_STRETCH`]
//! on the leading edge) rather than folded into the scale.
//!
//! # Controlled, never self-mutating
//!
//! A release inside the track reports `on_checked_change(state, !checked)`; the
//! widget's own `checked` changes only when the next `rebuild` feeds the
//! app-confirmed value back down.
//!
//! # Motion lanes
//!
//! Three, all advanced during `paint` and all re-requesting a frame while in
//! flight (the framework's advance-during-paint contract):
//!
//! - **travel** on [`SWITCH_THUMB_SPRING`] and **the track crossfade** on
//!   [`TRACK_COLOR_RAMP`] — two lanes on one `0 → 1` axis, because upstream
//!   times them differently (a `MotionConfig` spring for the layout, a 200ms
//!   `transition-colors` for the background);
//! - **squish**, on [`SPRING_PRESS`](crate::tokens::motion::SPRING_PRESS);
//! - **the refusal shake**, a plain keyframe timeline rather than a lane, since
//!   upstream authors it as literal `x` keyframes.
//!
//! Nothing here resizes, so every one of them asks for `request_frame`, never
//! `request_layout`. Under `theme.motion.reduce_motion` all of them collapse:
//! upstream gates its `MotionConfig` on `useReducedMotion` and skips both the
//! squish and the shake outright.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, BoxConstraints, BuildCtx, ChangeFlags, Color, CursorIcon, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect, Role,
    SemanticsCtx, Size, Toggled, View, Widget, erase_callback_arg,
};
use frust::{FrameTime, SpringDescription, Theme};

use crate::motion::Ramp;
use crate::press::{
    Lane, draw_focus_ring, inside_inclusive, is_activation_key, keyframes_at, lerp_color, presses,
};
use crate::style;
use crate::tokens::BeuiTokens;
use crate::tokens::motion::{EASE_OUT, SPRING_PRESS};

/// Track width, in logical px (`w-12`).
pub const SWITCH_TRACK_WIDTH: f64 = 48.0;
/// Track height, in logical px (`h-7`).
pub const SWITCH_TRACK_HEIGHT: f64 = 28.0;
/// Inner padding the thumb rests against at both ends, in logical px (`px-1`).
pub const SWITCH_TRACK_PADDING: f64 = 4.0;
/// Thumb edge, in logical px (`h-5 w-5`).
pub const SWITCH_THUMB_SIZE: f64 = 20.0;

/// How far the thumb slides between off and on, in logical px — the track less
/// its two paddings and the thumb itself.
pub const SWITCH_THUMB_TRAVEL: f64 =
    SWITCH_TRACK_WIDTH - 2.0 * SWITCH_TRACK_PADDING - SWITCH_THUMB_SIZE;

/// The scale a held thumb squashes to (`animate={{ scale: squish ? 0.9 : 1 }}`).
pub const SWITCH_SQUISH_SCALE: f64 = 0.9;

/// How far a held thumb stretches toward its destination, in logical px — the
/// `ml-1`/`mr-1` (one spacing step) upstream adds on the leading side.
pub const SWITCH_STRETCH: f64 = style::SPACING_UNIT;

/// The thumb's travel spring — **component-local, not one of
/// [`crate::tokens::motion`]'s six**.
///
/// Source: `THUMB_SPRING = { type: "spring", stiffness: 800, damping: 80, mass:
/// 4 }` (`switch.tsx`), whose own comment states the intent: *"Heavy,
/// deliberate thumb — high mass keeps the travel weighty without wobble."* It
/// is authored in the component file rather than `lib/ease.ts`, and its mass is
/// nearly seven times the heaviest shared spring's, so folding it onto
/// `SPRING_PRESS` would throw away the weight that makes this switch feel like
/// beUI's.
pub const SWITCH_THUMB_SPRING: SpringDescription = SpringDescription {
    mass: 4.0,
    stiffness: 800.0,
    damping: 80.0,
};

/// The track's color crossfade — `transition-colors duration-200`, which is
/// exactly [`TIMING_COLORS`](crate::tokens::motion::TIMING_COLORS) expressed as
/// the [`Ramp`] a [`Lane`] drives (a `Timing`'s spring arm carries a different
/// spring shape, so the two vocabularies do not convert; see
/// [`crate::motion`]).
pub const TRACK_COLOR_RAMP: Ramp = Ramp::eased(Duration::from_millis(200), EASE_OUT);

/// Gap between the track and its focus ring, in logical px (`ring-offset-2`).
pub const SWITCH_RING_OFFSET: f64 = 2.0;

/// The refusal shake a **disabled** switch performs when pressed: the literal
/// `x` keyframes upstream animates, in logical px.
///
/// Source: `animate(thumbRef, { x: [0, -2, 2, -1, 0] }, { delay: 0.2, duration:
/// 0.6 })`. The keyframes are evenly spaced across the duration, which is
/// Motion's own default for an array target.
pub const SWITCH_REFUSAL_KEYFRAMES: [f64; 5] = [0.0, -2.0, 2.0, -1.0, 0.0];

/// Delay before the refusal shake starts, in milliseconds (`delay: 0.2`).
const REFUSAL_DELAY_MS: f64 = 200.0;
/// Duration of the refusal shake, in milliseconds (`duration: 0.6`).
const REFUSAL_DURATION_MS: f64 = 600.0;

/// Alpha of the unchecked track: `bg-muted-foreground/60`.
const TRACK_OFF_ALPHA: f32 = 0.6;

/// Opacity of a disabled switch: `disabled:opacity-60`.
const DISABLED_OPACITY: f32 = 0.6;

/// Standard deviation of the thumb's `shadow-md`, in logical px — Tailwind's
/// `0 4px 6px -1px` blur, halved to the Gaussian sigma this scene's shadow
/// primitive takes.
const THUMB_SHADOW_STD_DEV: f64 = 3.0;
/// Alpha of the thumb's `shadow-md` (Tailwind's `rgb(0 0 0 / 0.1)`).
const THUMB_SHADOW_ALPHA: f32 = 0.1;

/// Unthemed fallback ink — the light table's `--primary` (which beUI aliases
/// onto `--foreground`).
const FALLBACK_PRIMARY: Color = crate::BEUI_LIGHT.primary;
/// Unthemed fallback dimmed ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED_FOREGROUND: Color = crate::BEUI_LIGHT.muted_foreground;
/// Unthemed fallback thumb — the light table's `--background`.
const FALLBACK_BACKGROUND: Color = crate::BEUI_LIGHT.background;

/// A view-held, typed change callback (erased on build).
type OnCheckedChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative beUI switch. See the [module docs](self).
pub struct SwitchView<State: 'static> {
    checked: bool,
    disabled: bool,
    label: Option<String>,
    on_checked_change: OnCheckedChange<State>,
}

/// Create a switch reflecting `checked` that reports
/// `on_checked_change(state, !checked)` on a release inside its bounds — a
/// **controlled** component (see the [module docs](self)).
///
/// Upstream renders its `label` prop as a sibling `<label>` element rather than
/// inside the control, and so does this port: compose the visible text
/// alongside (a `Row` of `switch(..)` plus `frust::text(..)`) and name the
/// control for assistive tech with [`SwitchView::label`].
pub fn switch<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_checked_change: F,
) -> SwitchView<State> {
    SwitchView {
        checked,
        disabled: false,
        label: None,
        on_checked_change: Rc::new(on_checked_change),
    }
}

impl<State: 'static> SwitchView<State> {
    /// Disable the control: 60% opacity, a not-allowed cursor, and — the beUI
    /// touch — a refusal shake instead of a toggle when it is pressed anyway.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Name the control for assistive tech (a switch paints no text of its own).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// The resolved switch palette.
struct SwitchColors {
    /// `bg-muted-foreground/60`, the unchecked track.
    track_off: Color,
    /// `bg-primary`, the checked track.
    track_on: Color,
    /// `bg-background`, the thumb (one color for both states upstream).
    thumb: Color,
    /// `--ring`, the focus ring.
    ring: Color,
}

/// Resolve the palette, falling back to the vendored light table with no theme
/// threaded.
fn resolve_colors(theme: Option<&Theme>) -> SwitchColors {
    let ring = BeuiTokens::resolve_ring(None, theme);
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            SwitchColors {
                track_off: style::with_alpha(scheme.on_surface_variant, TRACK_OFF_ALPHA),
                track_on: scheme.primary,
                thumb: scheme.surface,
                ring,
            }
        }
        None => SwitchColors {
            track_off: style::with_alpha(FALLBACK_MUTED_FOREGROUND, TRACK_OFF_ALPHA),
            track_on: FALLBACK_PRIMARY,
            thumb: FALLBACK_BACKGROUND,
            ring,
        },
    }
}

impl<State: 'static> View<State> for SwitchView<State> {
    type Element = SwitchWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SwitchWidget {
        let rest = if self.checked { 1.0 } else { 0.0 };
        SwitchWidget {
            checked: self.checked,
            disabled: self.disabled,
            label: self.label.clone(),
            travel: Lane::at_rest(Ramp::spring(SWITCH_THUMB_SPRING), rest),
            blend: Lane::at_rest(TRACK_COLOR_RAMP, rest),
            squish: Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            refusal_armed: false,
            refusal_start: None,
            captured: false,
            on_checked_change: erase_callback_arg(&self.on_checked_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SwitchWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled every pass.
        element.on_checked_change = erase_callback_arg(&self.on_checked_change);
        let mut flags = ChangeFlags::NONE;
        if prev.checked != self.checked {
            // The app is the source of truth: adopt the confirmed value and
            // animate toward it.
            element.checked = self.checked;
            let target = if self.checked { 1.0 } else { 0.0 };
            element.travel.retarget(target);
            element.blend.retarget(target);
            flags |= ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                // A control disabled mid-press keeps no armed state behind.
                element.captured = false;
                element.squish.retarget(0.0);
            } else {
                element.refusal_armed = false;
                element.refusal_start = None;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label = self.label.clone();
            // Semantics-only, but `PAINT` is what bumps the root's semantics
            // dirty gate and there is no narrower flag.
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained widget for a [`SwitchView`].
pub struct SwitchWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    checked: bool,
    disabled: bool,
    label: Option<String>,
    /// Thumb travel, `0.0` off .. `1.0` on, on [`SWITCH_THUMB_SPRING`].
    travel: Lane,
    /// The track's color crossfade, on the same `0 → 1` axis but upstream's own
    /// 200ms `transition-colors` timing.
    blend: Lane,
    /// How squashed-and-stretched the thumb is, `0.0` .. `1.0`.
    squish: Lane,
    /// A disabled press asking for the refusal shake. It cannot start the shake
    /// itself — an `EventCtx` carries no clock — so `paint` stamps the start.
    refusal_armed: bool,
    /// The frame the running refusal shake started on.
    refusal_start: Option<FrameTime>,
    /// Armed by a `Down` inside, cleared on `Up`/`Cancel`.
    captured: bool,
    on_checked_change: frust::authoring::ErasedArgCallback<bool>,
}

impl SwitchWidget {
    /// The cursor this control asks for in its current state.
    fn cursor(&self) -> CursorIcon {
        if self.disabled {
            style::DISABLED_CURSOR
        } else {
            style::ACTIVE_CURSOR
        }
    }

    /// The refusal shake's x offset at `now`, and whether it is still running.
    fn refusal_offset(&self, now: FrameTime) -> (f64, bool) {
        let Some(start) = self.refusal_start else {
            return (0.0, false);
        };
        let elapsed = now.saturating_sub(start).as_secs_f64() * 1000.0;
        if elapsed >= REFUSAL_DELAY_MS + REFUSAL_DURATION_MS {
            return (0.0, false);
        }
        if elapsed < REFUSAL_DELAY_MS {
            return (0.0, true);
        }
        let t = (elapsed - REFUSAL_DELAY_MS) / REFUSAL_DURATION_MS;
        (keyframes_at(&SWITCH_REFUSAL_KEYFRAMES, t), true)
    }

    /// The thumb's painted box at travel `t` and squish `s`, before the uniform
    /// squash scale: the nominal 20px square widened by [`SWITCH_STRETCH`] on
    /// whichever side it would travel toward next.
    fn thumb_box(&self, t: f64, s: f64) -> Rect {
        let stretch = SWITCH_STRETCH * s.clamp(0.0, 1.0);
        let x = SWITCH_TRACK_PADDING + SWITCH_THUMB_TRAVEL * t;
        // `justify-end` pins the checked thumb's right edge, so its `ml-1`
        // grows leftward; the unchecked thumb's `mr-1` grows rightward.
        let left = if self.checked { x - stretch } else { x };
        let y = (SWITCH_TRACK_HEIGHT - SWITCH_THUMB_SIZE) / 2.0;
        Rect::from_origin_size(
            Point::new(left, y),
            Size::new(SWITCH_THUMB_SIZE + stretch, SWITCH_THUMB_SIZE),
        )
    }
}

impl Widget for SwitchWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(SWITCH_TRACK_WIDTH, SWITCH_TRACK_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let focused = ctx.has_focus();
        let now = ctx.frame_time();

        // Advance now, ask for the next frame at the very end of the pass: the
        // theme borrow taken above lives until the last token read, and
        // `request_frame` needs the context mutably.
        let mut owes_frame = false;
        if reduce {
            self.travel.snap();
            self.blend.snap();
            self.squish.snap();
            self.refusal_armed = false;
            self.refusal_start = None;
        } else {
            if self.refusal_armed {
                self.refusal_armed = false;
                self.refusal_start = Some(now);
            }
            owes_frame |= self.travel.advance(now);
            owes_frame |= self.blend.advance(now);
            owes_frame |= self.squish.advance(now);
        }
        let (shake, shaking) = self.refusal_offset(now);
        if !shaking {
            self.refusal_start = None;
        }
        owes_frame |= shaking;

        let track = Size::new(SWITCH_TRACK_WIDTH, SWITCH_TRACK_HEIGHT);
        let radius = style::resolve_radius(style::RADIUS_CONTROL, track.width, track.height);
        let origin = ctx.origin();
        let tint = |color: Color| style::disabled_tint(color, self.disabled, DISABLED_OPACITY);

        scene.fill_rounded_rect(
            origin,
            track,
            radius,
            tint(lerp_color(
                colors.track_off,
                colors.track_on,
                self.blend.value().clamp(0.0, 1.0),
            )),
        );

        let squish = self.squish.value().clamp(0.0, 1.0);
        let thumb = self.thumb_box(self.travel.value(), squish);
        let scale = 1.0 - (1.0 - SWITCH_SQUISH_SCALE) * squish;
        let pivot = thumb.center() + origin.to_vec2();
        scene.push_transform(
            Affine::translate((shake, 0.0))
                * Affine::translate(pivot.to_vec2())
                * Affine::scale(scale)
                * Affine::translate(-pivot.to_vec2()),
        );
        let thumb_origin = thumb.origin() + origin.to_vec2();
        let thumb_size = thumb.size();
        let thumb_radius = thumb_size.height / 2.0;
        scene.draw_shadow(
            thumb_origin,
            thumb_size,
            thumb_radius,
            THUMB_SHADOW_STD_DEV,
            style::with_alpha(Color::BLACK, THUMB_SHADOW_ALPHA),
        );
        scene.fill_rounded_rect(thumb_origin, thumb_size, thumb_radius, tint(colors.thumb));
        scene.pop_transform();

        if focused {
            draw_focus_ring(
                scene,
                origin,
                track,
                radius,
                SWITCH_RING_OFFSET,
                tint(colors.ring),
            );
        }

        // Paint-only animation (the track never resizes), so a bare frame
        // request is the right one — never `request_layout`.
        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if self.disabled || !is_activation_key(key) {
                    return EventResult::Ignored;
                }
                // Report the requested value; never self-toggle.
                (self.on_checked_change)(ctx, !self.checked);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => {
                let size = ctx.size();
                match p.phase {
                    PointerPhase::Down => {
                        if !presses(p) || !inside_inclusive(p.position, size) {
                            return EventResult::Ignored;
                        }
                        if self.disabled {
                            // `disabled:cursor-not-allowed` keeps the control a
                            // pointer target, and upstream answers a refused
                            // press with the shake rather than with silence.
                            self.refusal_armed = true;
                            self.refusal_start = None;
                            ctx.request_redraw();
                            return EventResult::Handled;
                        }
                        self.captured = true;
                        self.squish.retarget(1.0);
                        ctx.capture_pointer();
                        ctx.request_focus();
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        if !self.captured {
                            if inside_inclusive(p.position, size) {
                                ctx.claim_hover();
                                ctx.set_cursor(self.cursor());
                            }
                            return EventResult::Ignored;
                        }
                        // Captured: re-ask so the shape survives a drag outside
                        // the track. `onPointerLeave` ends the squish upstream,
                        // which here is the stretch releasing once the pointer
                        // wanders off the control.
                        ctx.set_cursor(self.cursor());
                        let want = if inside_inclusive(p.position, size) {
                            1.0
                        } else {
                            0.0
                        };
                        if self.squish.target() != want {
                            self.squish.retarget(want);
                            ctx.request_redraw();
                        }
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        self.captured = false;
                        self.squish.retarget(0.0);
                        if inside_inclusive(p.position, size) {
                            (self.on_checked_change)(ctx, !self.checked);
                        }
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Cancel => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        // Internal state only — never the callback.
                        self.captured = false;
                        self.squish.retarget(0.0);
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                }
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Switch, |node| {
            if let Some(label) = &self.label {
                node.set_label(label.as_str());
            }
            node.set_toggled(Toggled::from(self.checked));
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Click);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{
        BezPath, Brush, EventOutcome, Key, KeyEvent, Modifiers, NamedKey, PointerButton,
        PointerEvent, SemanticsUpdate,
    };
    use kurbo::Shape;
    use std::any::Any;

    /// Records the rounded-rect fills (track, then thumb), stroked paths,
    /// shadows and transform pushes this widget emits.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        transforms: Vec<Affine>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
    }

    #[derive(Default)]
    struct Toggles {
        last: Option<bool>,
        count: u32,
    }

    const TRACK: Size = Size::new(SWITCH_TRACK_WIDTH, SWITCH_TRACK_HEIGHT);

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn view(checked: bool, disabled: bool) -> SwitchView<Toggles> {
        switch::<Toggles, _>(checked, |s: &mut Toggles, v: bool| {
            s.last = Some(v);
            s.count += 1;
        })
        .disabled(disabled)
        .label("wifi")
    }

    fn widget(checked: bool, disabled: bool) -> SwitchWidget {
        let mut counter = 0u64;
        View::<Toggles>::build(&view(checked, disabled), &mut BuildCtx::new(&mut counter))
    }

    fn paint_at(w: &mut SwitchWidget, theme: Option<&Theme>, ms: f64) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, TRACK, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn paint(w: &mut SwitchWidget, theme: Option<&Theme>) -> Recorder {
        paint_at(w, theme, 0.0).0
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn space() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Character(" ".into()),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(w: &mut SwitchWidget, state: &mut Toggles, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, TRACK);
        w.event(&mut ctx, event)
    }

    // `Lane`'s own contract (rest/retarget/retarget_with/snap/advance) and
    // `keyframes_at` each carry their leaf tests in `crate::press`'s test
    // module now.

    // ---- Metrics / paint --------------------------------------------------

    #[test]
    fn the_track_carries_the_authored_metrics_and_a_contained_travel() {
        assert_eq!(SWITCH_TRACK_WIDTH, 48.0, "w-12");
        assert_eq!(SWITCH_TRACK_HEIGHT, 28.0, "h-7");
        assert_eq!(SWITCH_THUMB_SIZE, 20.0, "h-5 w-5");
        assert_eq!(SWITCH_TRACK_PADDING, 4.0, "px-1");
        assert_eq!(SWITCH_THUMB_TRAVEL, 20.0);
        // The thumb lands flush inside the padding at both ends.
        let right = SWITCH_TRACK_PADDING + SWITCH_THUMB_TRAVEL + SWITCH_THUMB_SIZE;
        assert_eq!(right, SWITCH_TRACK_WIDTH - SWITCH_TRACK_PADDING);
    }

    #[test]
    fn unchecked_unthemed_paint_is_a_dimmed_track_with_the_thumb_parked_left() {
        let mut w = widget(false, false);
        let rec = paint(&mut w, None);
        assert_eq!(rec.rrects.len(), 2, "track then thumb");
        let (_, track, radius, fill) = rec.rrects[0];
        assert_eq!(track, TRACK);
        assert_eq!(radius, SWITCH_TRACK_HEIGHT / 2.0, "a pill");
        assert_eq!(fill.components[3], TRACK_OFF_ALPHA, "muted-foreground/60");

        let (thumb_origin, thumb, thumb_radius, thumb_fill) = rec.rrects[1];
        assert_eq!(thumb, Size::new(SWITCH_THUMB_SIZE, SWITCH_THUMB_SIZE));
        assert_eq!(thumb_radius, SWITCH_THUMB_SIZE / 2.0);
        assert_eq!(thumb_fill, FALLBACK_BACKGROUND);
        assert_eq!(thumb_origin.x, SWITCH_TRACK_PADDING, "off, so no travel");
        assert_eq!(
            thumb_origin.y,
            (SWITCH_TRACK_HEIGHT - SWITCH_THUMB_SIZE) / 2.0
        );
        // `shadow-md` under the thumb, and no ring at rest.
        assert_eq!(rec.shadows.len(), 1);
        assert!(rec.strokes.is_empty());
    }

    #[test]
    fn checked_paint_fills_primary_and_parks_the_thumb_at_the_far_end() {
        let theme = crate::theme();
        let mut w = widget(true, false);
        let rec = paint(&mut w, Some(&theme));
        assert_eq!(rec.rrects[0].3, theme.scheme().primary);
        assert_eq!(
            rec.rrects[1].0.x,
            SWITCH_TRACK_PADDING + SWITCH_THUMB_TRAVEL
        );
        assert_eq!(rec.rrects[1].3, theme.scheme().surface, "bg-background");
    }

    #[test]
    fn disabled_paint_dims_every_painted_alpha_to_sixty_percent() {
        let theme = crate::theme();
        let mut enabled = widget(true, false);
        let mut disabled = widget(true, true);
        let on = paint(&mut enabled, Some(&theme));
        let off = paint(&mut disabled, Some(&theme));
        for index in 0..2 {
            assert!(
                (off.rrects[index].3.components[3]
                    - on.rrects[index].3.components[3] * DISABLED_OPACITY)
                    .abs()
                    < 1e-6
            );
        }
    }

    // ---- Motion -----------------------------------------------------------

    #[test]
    fn a_confirmed_change_springs_the_thumb_across_and_settles_on_token() {
        let mut counter = 0u64;
        let prev = view(false, false);
        let mut w = View::<Toggles>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(true, false);
        View::<Toggles>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        let (start, owes) = paint_at(&mut w, None, 0.0);
        assert!(owes, "an in-flight lane owes the next frame");
        assert_eq!(start.rrects[1].0.x, SWITCH_TRACK_PADDING);

        let (mid, owes) = paint_at(&mut w, None, 60.0);
        assert!(owes);
        let travelled = mid.rrects[1].0.x - SWITCH_TRACK_PADDING;
        assert!(
            travelled > 0.0 && travelled < SWITCH_THUMB_TRAVEL,
            "mid-slide: {travelled}"
        );
        // The track crossfades on its own 200ms lane, so it is neither token.
        assert_ne!(mid.rrects[0].3, FALLBACK_PRIMARY);

        let (end, owes) = paint_at(&mut w, None, 3_000.0);
        assert!(!owes, "a settled pair of lanes asks for nothing");
        assert_eq!(
            end.rrects[1].0.x,
            SWITCH_TRACK_PADDING + SWITCH_THUMB_TRAVEL
        );
        assert_eq!(end.rrects[0].3, FALLBACK_PRIMARY, "lands exactly on-token");
    }

    #[test]
    fn reduce_motion_snaps_to_the_target_and_owes_no_frame() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut counter = 0u64;
        let prev = view(false, false);
        let mut w = View::<Toggles>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(true, false);
        View::<Toggles>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        let (rec, owes) = paint_at(&mut w, Some(&theme), 0.0);
        assert!(!owes, "reduce_motion asks for no animation frame");
        assert_eq!(
            rec.rrects[1].0.x,
            SWITCH_TRACK_PADDING + SWITCH_THUMB_TRAVEL
        );
        assert_eq!(rec.rrects[0].3, theme.scheme().primary);
        assert_eq!(rec.transforms.len(), 1, "an identity-scaled thumb");
    }

    #[test]
    fn a_held_thumb_squashes_and_stretches_toward_its_destination() {
        let mut w = widget(false, false);
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 14.0));
        paint_at(&mut w, None, 0.0);
        let (rec, owes) = paint_at(&mut w, None, 30.0);
        assert!(owes);
        let (thumb_origin, thumb, ..) = rec.rrects[1];
        assert!(
            thumb.width > SWITCH_THUMB_SIZE && thumb.width <= SWITCH_THUMB_SIZE + SWITCH_STRETCH,
            "an unchecked thumb stretches rightward: {thumb:?}"
        );
        assert_eq!(
            thumb_origin.x, SWITCH_TRACK_PADDING,
            "its left edge is pinned"
        );
        // ...under a shrinking uniform scale about the thumb's own centre.
        let scale = rec.transforms[0].as_coeffs()[0];
        assert!(
            (SWITCH_SQUISH_SCALE..1.0).contains(&scale),
            "squash scale {scale}"
        );

        // A checked thumb stretches the other way — its right edge is pinned.
        let mut on = widget(true, false);
        dispatch(
            &mut on,
            &mut state,
            &pointer(PointerPhase::Down, 38.0, 14.0),
        );
        paint_at(&mut on, None, 0.0);
        let rec = paint_at(&mut on, None, 30.0).0;
        let (thumb_origin, thumb, ..) = rec.rrects[1];
        assert!(thumb_origin.x < SWITCH_TRACK_PADDING + SWITCH_THUMB_TRAVEL);
        assert!(
            (thumb_origin.x + thumb.width
                - (SWITCH_TRACK_PADDING + SWITCH_THUMB_TRAVEL + SWITCH_THUMB_SIZE))
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn a_refused_press_shakes_after_its_delay_and_then_stops() {
        let mut w = widget(false, true);
        let mut state = Toggles::default();
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 14.0)),
            EventResult::Handled,
            "a disabled switch still consumes the press it refuses"
        );
        assert_eq!(state.count, 0, "and reports nothing");

        // The first paint stamps the shake's start; nothing has moved yet.
        let (rest, owes) = paint_at(&mut w, None, 0.0);
        assert!(owes, "the shake is pending");
        assert_eq!(rest.transforms[0].translation().x, 0.0);

        let (shaken, owes) = paint_at(&mut w, None, REFUSAL_DELAY_MS + 150.0);
        assert!(owes);
        assert!(
            shaken.transforms[0].translation().x < 0.0,
            "the first keyframe leans left"
        );

        let (done, owes) = paint_at(&mut w, None, REFUSAL_DELAY_MS + REFUSAL_DURATION_MS + 1.0);
        assert!(!owes, "the shake stops asking once it is over");
        assert_eq!(done.transforms[0].translation().x, 0.0);
    }

    // ---- Interaction ------------------------------------------------------

    #[test]
    fn up_inside_reports_the_requested_value_without_self_mutating() {
        let mut w = widget(false, false);
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 14.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 10.0, 14.0));
        assert_eq!(state.last, Some(true));
        assert!(!w.checked, "the app owns `checked`");
        assert!(!w.captured);
    }

    #[test]
    fn up_outside_and_cancel_never_fire_and_a_secondary_press_never_arms() {
        let mut w = widget(true, false);
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 14.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 900.0, 14.0));
        assert_eq!(state.count, 0);

        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 14.0));
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Cancel, 10.0, 14.0),
        );
        assert_eq!(state.count, 0);
        assert!(!w.captured);

        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(10.0, 14.0),
            button: PointerButton::Secondary,
        });
        assert_eq!(
            dispatch(&mut w, &mut state, &secondary),
            EventResult::Ignored
        );
        assert!(!w.captured, "no capture for the shell to get stuck on");
    }

    #[test]
    fn space_and_enter_activate_and_a_disabled_switch_takes_no_keys() {
        let mut w = widget(true, false);
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, &space());
        assert_eq!(state.last, Some(false));
        let enter = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        dispatch(&mut w, &mut state, &enter);
        assert_eq!(state.count, 2);

        let mut disabled = widget(false, true);
        assert_eq!(
            dispatch(&mut disabled, &mut state, &space()),
            EventResult::Ignored
        );
        assert_eq!(state.count, 2);
    }

    #[test]
    fn rebuild_adopts_the_confirmed_value_and_disarms_on_disable() {
        let mut counter = 0u64;
        let prev = view(false, false);
        let mut w = View::<Toggles>::build(&prev, &mut BuildCtx::new(&mut counter));
        w.captured = true;
        let next = view(true, true);
        let flags =
            View::<Toggles>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.checked);
        assert!(w.disabled);
        assert!(!w.captured, "disabling clears an armed press");
        assert!(flags.needs_paint());
    }

    // ---- Root-driven: focus ring, cursor, semantics ------------------------

    /// One switch under a real `RenderRoot` — the only harness that can
    /// exercise focus and the cursor.
    struct Harness {
        root: frust_core::RenderRoot<Toggles, SwitchView<Toggles>>,
        state: Toggles,
    }

    impl Harness {
        fn new(disabled: bool) -> Self {
            let mut h = Harness {
                root: frust_core::RenderRoot::new(),
                state: Toggles::default(),
            };
            h.root.set_theme(Box::new(crate::theme()));
            let mut logic = move |_s: &mut Toggles| view(false, disabled);
            h.root.rebuild(&mut logic, &mut h.state);
            h.root.layout(Size::new(200.0, 200.0));
            h
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventOutcome {
            self.root.event(&mut self.state, event)
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        fn semantics(&self) -> SemanticsUpdate {
            self.root.semantics()
        }
    }

    #[test]
    fn focus_paints_the_offset_ring_in_the_beui_ring_token() {
        let mut h = Harness::new(false);
        assert!(h.paint().strokes.is_empty(), "nothing stroked at rest");

        h.dispatch(&pointer(PointerPhase::Down, 10.0, 14.0));
        assert!(h.root.is_focus_active());
        let rec = h.paint();
        assert_eq!(rec.strokes.len(), 1, "the ring, and nothing else");
        let (bbox, width, color) = rec.strokes[0];
        assert_eq!(width, style::FOCUS_RING_WIDTH);
        assert_eq!(color, BeuiTokens::resolve_ring(None, Some(&crate::theme())));
        // `ring-offset-2`: it clears the track entirely.
        assert!(bbox.x0 <= -SWITCH_RING_OFFSET);
        assert!(bbox.x1 >= SWITCH_TRACK_WIDTH + SWITCH_RING_OFFSET);
    }

    #[test]
    fn a_move_resolves_the_pointer_cursor_and_not_allowed_when_disabled() {
        let mut h = Harness::new(false);
        h.dispatch(&pointer(PointerPhase::Move, 10.0, 14.0));
        assert_eq!(h.root.cursor(), style::ACTIVE_CURSOR);

        let mut disabled = Harness::new(true);
        disabled.dispatch(&pointer(PointerPhase::Move, 10.0, 14.0));
        assert_eq!(disabled.root.cursor(), style::DISABLED_CURSOR);
    }

    #[test]
    fn semantics_reports_a_switch_node_with_its_toggle_state_and_bounds() {
        let h = Harness::new(false);
        let update = h.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Switch)
            .expect("a Role::Switch node");
        assert_eq!(node.label(), Some("wifi"));
        assert_eq!(node.toggled(), Some(Toggled::False));
        assert!(node.supports_action(Action::Click));
        let bounds = node.bounds().expect("the switch node has bounds");
        assert_eq!(
            (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0),
            (SWITCH_TRACK_WIDTH, SWITCH_TRACK_HEIGHT)
        );
    }
}
