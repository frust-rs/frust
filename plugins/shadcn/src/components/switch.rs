//! Ports shadcn/ui's **Switch** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/switch.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17).
//!
//! | class | here |
//! |---|---|
//! | `data-[size=default]:h-[1.15rem] w-8` + thumb `size-4` | [`SwitchSize::Default`] |
//! | `data-[size=sm]:h-3.5 w-6` + thumb `size-3` | [`SwitchSize::Sm`] |
//! | `rounded-full` | a pill: radius = track height / 2 |
//! | `border border-transparent` | [`style::BORDER_WIDTH`] of inset, painted only while focused |
//! | `shadow-xs` | [`style::SHADOW_XS`] |
//! | `data-[state=checked]:bg-primary` / `data-[state=unchecked]:bg-input` | the track fill |
//! | `dark:data-[state=unchecked]:bg-input/80` | [`DARK_UNCHECKED_TRACK_ALPHA`] |
//! | thumb `bg-background`; `dark:` checked `bg-primary-foreground`, unchecked `bg-foreground` | the thumb fill |
//! | `translate-x-[calc(100%-2px)]` | [`SWITCH_THUMB_TRAVEL_INSET`] off the thumb's own width |
//! | `focus-visible:border-ring` + `ring-[3px] ring-ring/50` | [`style::focus_border`] + [`style::draw_focus_ring`] |
//! | `disabled:opacity-50` + `disabled:cursor-not-allowed` | [`style::disabled_tint`] + [`style::DISABLED_CURSOR`] |
//!
//! The transparent border is not decoration: `box-sizing: border-box` means it
//! insets the thumb by 1px on every edge, which is exactly what makes the
//! `calc(100%-2px)` travel land the thumb flush against the track's inner right
//! edge. It is therefore modelled as an inset, and only *painted* (in the ring
//! color) when the control has focus.
//!
//! # Controlled, never self-mutating
//!
//! A release inside the track reports `on_checked_change(state, !checked)`; the
//! widget's own `checked` changes only when the next `rebuild` feeds the
//! app-confirmed value back down.
//!
//! # One motion lane for two properties
//!
//! The source animates the root with `transition-all` and the thumb with
//! `transition-transform`, both at Tailwind's default duration/easing
//! ([`SWITCH_DURATION`]/[`SWITCH_CURVE`]) — same timing, so one progress lane
//! drives the thumb's travel *and* the track/thumb color blend. It advances
//! during `paint` and re-requests a frame while in flight (the framework's
//! advance-during-paint contract, `docs/WIDGETS_CODE_STANDARDS.md`); the track
//! never resizes, so this is a paint-only animation and asks for
//! `request_frame`, never `request_layout`.
//!
//! Under `theme.motion.reduce_motion` the lane snaps to its target on the next
//! paint and owes no further frame.
//!
//! # No hover chrome
//!
//! The class list authors no `hover:` variant. The widget still claims the hover
//! link from its uncaptured `Move` arm (so an enclosing row reads hovered) and
//! asks for a cursor there, but keeps no hover flag — nothing it paints depends
//! on one.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, CursorIcon, EventCtx, EventResult,
    InputEvent, Key, KeyEvent, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerPhase,
    Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, Toggled, View, Widget, erase_callback_arg,
};
use frust::{AnimationController, Brightness, Curve, FrameTime, Theme, Tween};

use crate::hit::inside;
use crate::style::{self, PATH_TOLERANCE};

/// Track width for [`SwitchSize::Default`], in logical px (`w-8`).
pub const SWITCH_TRACK_WIDTH: f64 = 32.0;
/// Track height for [`SwitchSize::Default`], in logical px (`h-[1.15rem]` at the
/// CSS default 16px root font size — an arbitrary value, so it is not on any
/// scale).
pub const SWITCH_TRACK_HEIGHT: f64 = 18.4;
/// Thumb edge for [`SwitchSize::Default`], in logical px (`size-4`).
pub const SWITCH_THUMB_SIZE: f64 = 16.0;

/// Track width for [`SwitchSize::Sm`], in logical px (`w-6`).
pub const SWITCH_TRACK_WIDTH_SM: f64 = 24.0;
/// Track height for [`SwitchSize::Sm`], in logical px (`h-3.5`).
pub const SWITCH_TRACK_HEIGHT_SM: f64 = 14.0;
/// Thumb edge for [`SwitchSize::Sm`], in logical px (`size-3`).
pub const SWITCH_THUMB_SIZE_SM: f64 = 12.0;

/// How far short of its own width the thumb travels
/// (`translate-x-[calc(100%-2px)]`), in logical px.
pub const SWITCH_THUMB_TRAVEL_INSET: f64 = 2.0;

/// Alpha of the unchecked track in dark mode
/// (`dark:data-[state=unchecked]:bg-input/80`).
const DARK_UNCHECKED_TRACK_ALPHA: f32 = 0.80;

/// The state transition's duration — Tailwind's `--default-transition-duration`
/// (150ms, `tailwindcss@4.3.0`'s `theme.css`), which is what a bare
/// `transition-all`/`transition-transform` resolves to.
const SWITCH_DURATION: Duration = Duration::from_millis(150);
/// The state transition's easing — Tailwind's
/// `--default-transition-timing-function`, `cubic-bezier(0.4, 0, 0.2, 1)`. Never
/// overshoots, so neither the travel nor the color blend extrapolates.
const SWITCH_CURVE: Curve = Curve::Cubic(0.4, 0.0, 0.2, 1.0);

/// Unthemed fallback checked track — the `neutral` preset's light `--primary`.
const FALLBACK_PRIMARY: Color = Color::from_rgb8(0x17, 0x17, 0x17);
/// Unthemed fallback unchecked track — light `--input`.
const FALLBACK_INPUT: Color = Color::from_rgb8(0xE5, 0xE5, 0xE5);
/// Unthemed fallback thumb — light `--background`.
const FALLBACK_BACKGROUND: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);

/// The switch's two authored sizes (`data-size`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SwitchSize {
    /// `size="default"`: a 32×18.4 track with a 16px thumb.
    #[default]
    Default,
    /// `size="sm"`: a 24×14 track with a 12px thumb.
    Sm,
}

impl SwitchSize {
    /// The track box for this size.
    pub fn track(self) -> Size {
        match self {
            SwitchSize::Default => Size::new(SWITCH_TRACK_WIDTH, SWITCH_TRACK_HEIGHT),
            SwitchSize::Sm => Size::new(SWITCH_TRACK_WIDTH_SM, SWITCH_TRACK_HEIGHT_SM),
        }
    }

    /// The thumb edge for this size.
    pub fn thumb(self) -> f64 {
        match self {
            SwitchSize::Default => SWITCH_THUMB_SIZE,
            SwitchSize::Sm => SWITCH_THUMB_SIZE_SM,
        }
    }

    /// How far the thumb slides between off and on: its own width less
    /// [`SWITCH_THUMB_TRAVEL_INSET`].
    pub fn travel(self) -> f64 {
        self.thumb() - SWITCH_THUMB_TRAVEL_INSET
    }
}

/// A view-held, typed change callback (erased on build).
type OnCheckedChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative shadcn switch. See the [module docs](self).
pub struct SwitchView<State: 'static> {
    checked: bool,
    size: SwitchSize,
    disabled: bool,
    label: Option<String>,
    on_checked_change: OnCheckedChange<State>,
}

/// Create a switch reflecting `checked` that reports
/// `on_checked_change(state, !checked)` on a release inside its bounds — a
/// **controlled** component (see the [module docs](self)).
pub fn switch<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_checked_change: F,
) -> SwitchView<State> {
    SwitchView {
        checked,
        size: SwitchSize::Default,
        disabled: false,
        label: None,
        on_checked_change: Rc::new(on_checked_change),
    }
}

impl<State: 'static> SwitchView<State> {
    /// Pick the authored size (`size="sm" | "default"`).
    pub fn size(mut self, size: SwitchSize) -> Self {
        self.size = size;
        self
    }

    /// Disable the control: 50% opacity, inert, not-allowed cursor.
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

/// The resolved switch palette, off and on.
struct SwitchColors {
    track_off: Color,
    track_on: Color,
    thumb_off: Color,
    thumb_on: Color,
}

/// Resolve the palette, falling back to the `neutral` preset's light values with
/// no theme threaded.
///
/// Light mode paints one thumb color for both states (`bg-background`); dark mode
/// splits it (`dark:data-[state=checked]:bg-primary-foreground`,
/// `dark:data-[state=unchecked]:bg-foreground`).
fn resolve_colors(theme: Option<&Theme>) -> SwitchColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            match theme.brightness {
                Brightness::Dark => SwitchColors {
                    track_off: style::scale_alpha(
                        scheme.outline_variant,
                        DARK_UNCHECKED_TRACK_ALPHA,
                    ),
                    track_on: scheme.primary,
                    thumb_off: scheme.on_surface,
                    thumb_on: scheme.on_primary,
                },
                Brightness::Light => SwitchColors {
                    track_off: scheme.outline_variant,
                    track_on: scheme.primary,
                    thumb_off: scheme.surface,
                    thumb_on: scheme.surface,
                },
            }
        }
        None => SwitchColors {
            track_off: FALLBACK_INPUT,
            track_on: FALLBACK_PRIMARY,
            thumb_off: FALLBACK_BACKGROUND,
            thumb_on: FALLBACK_BACKGROUND,
        },
    }
}

/// Interpolate `begin`→`end` at `t`, snapping exactly to an endpoint at (or
/// past) `0.0`/`1.0` rather than routing it through [`Tween::lerp`]'s `f32`
/// arithmetic, which can land a few ULPs off `end`. Keeps a resting switch
/// pixel-identical to its resting token, the guarantee every catalog's
/// two-state control keeps.
fn lerp_color(begin: Color, end: Color, t: f64) -> Color {
    if t <= 0.0 {
        begin
    } else if t >= 1.0 {
        end
    } else {
        Tween::new(begin, end).lerp(t)
    }
}

/// Whether `key` activates the control: `Space` (arriving as typed text — there
/// is no `NamedKey::Space`) or `Enter`.
fn is_activation_key(key: &KeyEvent) -> bool {
    match &key.key {
        Key::Named(NamedKey::Enter) => true,
        Key::Character(text) => text == " ",
        _ => false,
    }
}

impl<State: 'static> View<State> for SwitchView<State> {
    type Element = SwitchWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SwitchWidget {
        let rest = if self.checked { 1.0 } else { 0.0 };
        SwitchWidget {
            checked: self.checked,
            size: self.size,
            disabled: self.disabled,
            label: self.label.clone(),
            travel: Progress::at_rest(rest),
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
        element.on_checked_change = erase_callback_arg(&self.on_checked_change);
        let mut flags = ChangeFlags::NONE;
        if prev.checked != self.checked {
            // The app is the source of truth: adopt the confirmed value and
            // animate toward it. The timing is a constant, so `rebuild` can
            // retarget without waiting for a paint-time theme.
            element.checked = self.checked;
            element
                .travel
                .retarget(if self.checked { 1.0 } else { 0.0 });
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                element.captured = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label = self.label.clone();
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// One `0 → 1` progress lane on [`SWITCH_DURATION`]/[`SWITCH_CURVE`]: a
/// controller plus the endpoints it interpolates between.
///
/// The endpoint pair (rather than a bare controller value) is what lets a
/// freshly-built switch rest at its confirmed state with no entry animation, and
/// a mid-flight reversal start from what is actually on screen — the same shape
/// `frust_glyph::toggle`'s lane carries.
struct Progress {
    ctrl: AnimationController,
    from: f64,
    to: f64,
    /// The value last computed by [`Progress::advance`] — what paint reads.
    displayed: f64,
}

impl Progress {
    /// An idle lane resting at `value`.
    fn at_rest(value: f64) -> Self {
        Self {
            ctrl: AnimationController::new(SWITCH_DURATION).with_curve(SWITCH_CURVE),
            from: value,
            to: value,
            displayed: value,
        }
    }

    /// Re-aim at `to`, starting from whatever is on screen now.
    fn retarget(&mut self, to: f64) {
        self.from = self.displayed;
        self.to = to;
        self.ctrl = AnimationController::new(SWITCH_DURATION).with_curve(SWITCH_CURVE);
        self.ctrl.forward();
    }

    /// Land on the target immediately, cancelling any flight (the
    /// `reduce_motion` path). A fresh controller is idle, so the following
    /// [`Progress::advance`] stays at rest and asks for nothing.
    fn snap(&mut self) {
        self.from = self.to;
        self.displayed = self.to;
        self.ctrl = AnimationController::new(SWITCH_DURATION).with_curve(SWITCH_CURVE);
    }

    /// Advance to frame time `now`, returning whether the lane is still in
    /// flight (in which case the caller owes another frame).
    fn advance(&mut self, now: FrameTime) -> bool {
        if self.ctrl.is_animating() {
            let animating = self.ctrl.advance(now);
            self.displayed = self.from + (self.to - self.from) * self.ctrl.value();
            animating
        } else {
            self.displayed = self.to;
            false
        }
    }

    /// The current value (the curve never overshoots, so this needs no clamp).
    fn value(&self) -> f64 {
        self.displayed
    }
}

/// The retained widget for a [`SwitchView`].
pub struct SwitchWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    checked: bool,
    size: SwitchSize,
    disabled: bool,
    label: Option<String>,
    /// Thumb travel and color blend, `0.0` off .. `1.0` on.
    travel: Progress,
    /// Armed by a `Down` inside, cleared on `Up`/`Cancel`. Nothing paints
    /// differently while armed (the source authors no `:active` rule).
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
}

impl Widget for SwitchWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.size.track())
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let focused = ctx.has_focus();

        // Advance now, ask for the next frame at the very end of the pass: the
        // theme borrow taken above lives until the last token read, and
        // `request_frame` needs the context mutably.
        let owes_frame = if reduce_motion {
            self.travel.snap();
            false
        } else {
            self.travel.advance(ctx.frame_time())
        };
        let t = self.travel.value();

        let track = self.size.track();
        let radius = track.height / 2.0;
        let origin = ctx.origin();
        let tint = |color: Color| style::disabled_tint(color, self.disabled);

        style::draw_shadow(scene, origin, track, radius, style::SHADOW_XS, theme);
        scene.fill_rounded_rect(
            origin,
            track,
            radius,
            tint(lerp_color(colors.track_off, colors.track_on, t)),
        );

        // `border-transparent` paints nothing at rest; `focus-visible:border-ring`
        // is the one state that makes it visible.
        if focused {
            let inset = style::BORDER_WIDTH / 2.0;
            let outline = RoundedRect::from_rect(
                Rect::from_origin_size(Point::ORIGIN, track).inset(-inset),
                radius + inset,
            );
            scene.stroke_path(
                origin,
                &Shape::to_path(&outline, PATH_TOLERANCE),
                style::BORDER_WIDTH,
                &Brush::Solid(tint(style::focus_border(Color::TRANSPARENT, true, theme))),
            );
        }

        // The thumb rides inside the 1px transparent border, vertically centred,
        // and slides `travel()` px between its two rest positions.
        let thumb = self.size.thumb();
        let thumb_x = style::BORDER_WIDTH + self.size.travel() * t;
        let thumb_y = (track.height - thumb) / 2.0;
        scene.fill_rounded_rect(
            Point::new(origin.x + thumb_x, origin.y + thumb_y),
            Size::new(thumb, thumb),
            thumb / 2.0,
            tint(lerp_color(colors.thumb_off, colors.thumb_on, t)),
        );

        if focused {
            style::draw_focus_ring(scene, origin, track, radius, style::ring_color(None, theme));
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
                        if self.disabled || !inside(p.position, size) {
                            return EventResult::Ignored;
                        }
                        self.captured = true;
                        ctx.capture_pointer();
                        ctx.request_focus();
                        // The focus ring is what earns the frame.
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        if !self.captured {
                            if inside(p.position, size) {
                                ctx.claim_hover();
                                ctx.set_cursor(self.cursor());
                            }
                            return EventResult::Ignored;
                        }
                        // Captured: re-ask so the shape survives a drag outside
                        // the track.
                        ctx.set_cursor(self.cursor());
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        self.captured = false;
                        if inside(p.position, size) {
                            (self.on_checked_change)(ctx, !self.checked);
                        }
                        EventResult::Handled
                    }
                    PointerPhase::Cancel => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        // Internal flags only.
                        self.captured = false;
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
        BezPath, EventOutcome, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use std::any::Any;

    /// Records the rounded-rect fills (track, then thumb), stroked paths and
    /// shadows this widget emits.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
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
    }

    #[derive(Default)]
    struct Toggles {
        last: Option<bool>,
        count: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn view(checked: bool, size: SwitchSize, disabled: bool) -> SwitchView<Toggles> {
        switch::<Toggles, _>(checked, |s: &mut Toggles, v: bool| {
            s.last = Some(v);
            s.count += 1;
        })
        .size(size)
        .disabled(disabled)
        .label("wifi")
    }

    fn widget(checked: bool, size: SwitchSize, disabled: bool) -> SwitchWidget {
        let mut counter = 0u64;
        View::<Toggles>::build(
            &view(checked, size, disabled),
            &mut BuildCtx::new(&mut counter),
        )
    }

    fn paint_at(w: &mut SwitchWidget, theme: Option<&Theme>, ms: f64) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let track = w.size.track();
        let mut ctx = PaintCtx::for_test(Point::ZERO, track, ft_ms(ms));
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
        let mut ctx = EventCtx::new(state_any, Point::ZERO, w.size.track());
        w.event(&mut ctx, event)
    }

    // ---- Metrics / paint --------------------------------------------------

    #[test]
    fn the_two_sizes_carry_the_authored_track_thumb_and_travel() {
        assert_eq!(
            SwitchSize::Default.track(),
            Size::new(SWITCH_TRACK_WIDTH, SWITCH_TRACK_HEIGHT)
        );
        assert_eq!(SwitchSize::Sm.track(), Size::new(24.0, 14.0));
        assert_eq!(SwitchSize::Default.thumb(), 16.0);
        assert_eq!(SwitchSize::Sm.thumb(), 12.0);
        assert_eq!(SwitchSize::default(), SwitchSize::Default);
        // `calc(100%-2px)`, and the travel lands the thumb flush inside the
        // track's 1px border on both ends.
        assert_eq!(SwitchSize::Default.travel(), 14.0);
        for size in [SwitchSize::Default, SwitchSize::Sm] {
            let right = style::BORDER_WIDTH + size.travel() + size.thumb();
            assert!((right - (size.track().width - style::BORDER_WIDTH)).abs() < 1e-9);
        }
    }

    #[test]
    fn unchecked_unthemed_paint_is_an_input_track_with_a_background_thumb_at_rest() {
        let mut w = widget(false, SwitchSize::Default, false);
        let rec = paint(&mut w, None);
        assert_eq!(rec.rrects.len(), 2, "track then thumb");
        let (_, track, radius, fill) = rec.rrects[0];
        assert_eq!(track, SwitchSize::Default.track());
        assert_eq!(radius, SWITCH_TRACK_HEIGHT / 2.0, "a pill");
        assert_eq!(fill, FALLBACK_INPUT);

        let (thumb_origin, thumb, thumb_radius, thumb_fill) = rec.rrects[1];
        assert_eq!(thumb, Size::new(SWITCH_THUMB_SIZE, SWITCH_THUMB_SIZE));
        assert_eq!(thumb_radius, SWITCH_THUMB_SIZE / 2.0);
        assert_eq!(thumb_fill, FALLBACK_BACKGROUND);
        assert_eq!(thumb_origin.x, style::BORDER_WIDTH, "off, so no travel");
        assert!((thumb_origin.y - (SWITCH_TRACK_HEIGHT - SWITCH_THUMB_SIZE) / 2.0).abs() < 1e-9);

        assert_eq!(rec.shadows.len(), 1);
        assert_eq!(rec.shadows[0].3, style::SHADOW_XS.std_dev);
        assert!(
            rec.strokes.is_empty(),
            "`border-transparent` paints nothing"
        );
    }

    #[test]
    fn checked_paint_fills_primary_and_parks_the_thumb_at_the_far_end() {
        let theme = crate::theme();
        let mut w = widget(true, SwitchSize::Default, false);
        let rec = paint(&mut w, Some(&theme));
        assert_eq!(rec.rrects[0].3, theme.scheme().primary);
        assert_eq!(
            rec.rrects[1].0.x,
            style::BORDER_WIDTH + SwitchSize::Default.travel()
        );
        // Light mode keeps one thumb color for both states.
        assert_eq!(rec.rrects[1].3, theme.scheme().surface);
    }

    #[test]
    fn dark_mode_dims_the_unchecked_track_and_splits_the_thumb_color() {
        let theme = crate::theme().with_brightness(Brightness::Dark);
        let scheme = theme.scheme();
        let mut off = widget(false, SwitchSize::Default, false);
        let rec = paint(&mut off, Some(&theme));
        let input = scheme.outline_variant;
        assert!(
            (rec.rrects[0].3.components[3] - input.components[3] * DARK_UNCHECKED_TRACK_ALPHA)
                .abs()
                < 1e-6
        );
        assert_eq!(rec.rrects[1].3, scheme.on_surface, "`dark:bg-foreground`");

        let mut on = widget(true, SwitchSize::Default, false);
        let rec = paint(&mut on, Some(&theme));
        assert_eq!(
            rec.rrects[1].3, scheme.on_primary,
            "`dark:bg-primary-foreground`"
        );
    }

    #[test]
    fn the_sm_size_paints_its_own_smaller_geometry() {
        let mut w = widget(true, SwitchSize::Sm, false);
        let rec = paint(&mut w, None);
        assert_eq!(rec.rrects[0].1, Size::new(24.0, 14.0));
        assert_eq!(rec.rrects[1].1, Size::new(12.0, 12.0));
        assert_eq!(rec.rrects[1].0.x, style::BORDER_WIDTH + 10.0);
    }

    #[test]
    fn disabled_paint_halves_every_painted_alpha() {
        let theme = crate::theme();
        let mut enabled = widget(true, SwitchSize::Default, false);
        let mut disabled = widget(true, SwitchSize::Default, true);
        let on = paint(&mut enabled, Some(&theme));
        let off = paint(&mut disabled, Some(&theme));
        assert_eq!(
            off.rrects[0].3.components[3],
            on.rrects[0].3.components[3] * style::DISABLED_OPACITY
        );
        assert_eq!(
            off.rrects[1].3.components[3],
            on.rrects[1].3.components[3] * style::DISABLED_OPACITY
        );
    }

    // ---- Motion -----------------------------------------------------------

    #[test]
    fn a_confirmed_change_slides_the_thumb_and_settles_at_the_target() {
        let mut counter = 0u64;
        let prev = view(false, SwitchSize::Default, false);
        let mut w = View::<Toggles>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(true, SwitchSize::Default, false);
        View::<Toggles>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        // First paint seeds the clock and the lane is in flight, so a frame is
        // owed and the thumb has not arrived.
        let (start, owes_frame) = paint_at(&mut w, None, 0.0);
        assert!(owes_frame, "an in-flight lane owes the next frame");
        assert_eq!(start.rrects[1].0.x, style::BORDER_WIDTH);

        let (mid, owes_frame) = paint_at(&mut w, None, 75.0);
        assert!(owes_frame);
        let travel = SwitchSize::Default.travel();
        let mid_x = mid.rrects[1].0.x - style::BORDER_WIDTH;
        assert!(mid_x > 0.0 && mid_x < travel, "mid-slide: {mid_x}");
        // The track color blends on the same lane.
        assert_ne!(mid.rrects[0].3, FALLBACK_INPUT);
        assert_ne!(mid.rrects[0].3, FALLBACK_PRIMARY);

        let (end, owes_frame) = paint_at(&mut w, None, 400.0);
        assert!(!owes_frame, "a settled lane asks for nothing");
        assert_eq!(end.rrects[1].0.x, style::BORDER_WIDTH + travel);
        assert_eq!(
            end.rrects[0].3, FALLBACK_PRIMARY,
            "and lands exactly on-token"
        );
    }

    #[test]
    fn reduce_motion_snaps_to_the_target_and_owes_no_frame() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut counter = 0u64;
        let prev = view(false, SwitchSize::Default, false);
        let mut w = View::<Toggles>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(true, SwitchSize::Default, false);
        View::<Toggles>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        let (rec, owes_frame) = paint_at(&mut w, Some(&theme), 0.0);
        assert!(!owes_frame, "reduce_motion asks for no animation frame");
        assert_eq!(
            rec.rrects[1].0.x,
            style::BORDER_WIDTH + SwitchSize::Default.travel(),
            "the thumb is already at its target"
        );
        assert_eq!(rec.rrects[0].3, theme.scheme().primary);
    }

    // ---- Interaction ------------------------------------------------------

    #[test]
    fn up_inside_reports_the_requested_value_without_self_mutating() {
        let mut w = widget(false, SwitchSize::Default, false);
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 8.0, 9.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 8.0, 9.0));
        assert_eq!(state.last, Some(true));
        assert!(!w.checked, "the app owns `checked`");
        assert!(!w.captured);
    }

    #[test]
    fn up_outside_and_cancel_never_fire() {
        let mut w = widget(true, SwitchSize::Default, false);
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 8.0, 9.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 90.0, 9.0));
        assert_eq!(state.count, 0);

        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 8.0, 9.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Cancel, 8.0, 9.0));
        assert_eq!(state.count, 0);
        assert!(!w.captured);
    }

    #[test]
    fn space_activates_and_a_disabled_switch_is_inert() {
        let mut w = widget(true, SwitchSize::Default, false);
        let mut state = Toggles::default();
        dispatch(&mut w, &mut state, &space());
        assert_eq!(state.last, Some(false));

        let mut disabled = widget(false, SwitchSize::Default, true);
        assert_eq!(
            dispatch(&mut disabled, &mut state, &space()),
            EventResult::Ignored
        );
        assert_eq!(
            dispatch(
                &mut disabled,
                &mut state,
                &pointer(PointerPhase::Down, 8.0, 9.0)
            ),
            EventResult::Ignored
        );
        assert_eq!(state.count, 1, "only the enabled switch reported");
    }

    #[test]
    fn a_hover_move_claims_without_arming_or_repainting() {
        let mut w = widget(false, SwitchSize::Default, false);
        let mut state = Toggles::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, SwitchSize::Default.track());
        let result = w.event(&mut ctx, &pointer(PointerPhase::Move, 8.0, 9.0));
        assert_eq!(result, EventResult::Ignored);
        assert!(!w.captured);
        assert!(!ctx.needs_redraw(), "no hover chrome, so no frame is owed");
    }

    #[test]
    fn rebuild_adopts_the_confirmed_value_and_a_size_change_relayouts() {
        let mut counter = 0u64;
        let prev = view(false, SwitchSize::Default, false);
        let mut w = View::<Toggles>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(true, SwitchSize::Sm, false);
        let flags =
            View::<Toggles>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.checked);
        assert_eq!(w.size, SwitchSize::Sm);
        assert!(flags.needs_layout());
    }

    // ---- Root-driven: focus ring, cursor, semantics ------------------------

    /// One switch under a real `RenderRoot` — the only harness that can exercise
    /// focus and the cursor.
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
            let mut logic = |_s: &mut Toggles| view(false, SwitchSize::Default, disabled);
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
    fn focus_paints_the_ring_and_makes_the_transparent_border_visible() {
        let mut h = Harness::new(false);
        assert!(h.paint().strokes.is_empty(), "nothing stroked at rest");

        h.dispatch(&pointer(PointerPhase::Down, 8.0, 9.0));
        assert!(h.root.is_focus_active());
        let rec = h.paint();
        assert_eq!(rec.strokes.len(), 2, "border + focus ring");
        let ring = style::ring_color(None, Some(&crate::theme()));
        assert_eq!(rec.strokes[0].2, ring, "`focus-visible:border-ring`");
        assert_eq!(rec.strokes[0].1, style::BORDER_WIDTH);
        let (bbox, width, color) = rec.strokes[1];
        assert_eq!(width, style::FOCUS_RING_WIDTH);
        assert_eq!(color.components[3], style::FOCUS_RING_OPACITY);
        assert!(bbox.x0 < 0.0 && bbox.x1 > SWITCH_TRACK_WIDTH);
    }

    #[test]
    fn a_move_resolves_the_pointer_cursor_and_not_allowed_when_disabled() {
        let mut h = Harness::new(false);
        h.dispatch(&pointer(PointerPhase::Move, 8.0, 9.0));
        assert_eq!(h.root.cursor(), style::ACTIVE_CURSOR);

        let mut disabled = Harness::new(true);
        disabled.dispatch(&pointer(PointerPhase::Move, 8.0, 9.0));
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
