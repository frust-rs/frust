//! The M3 `Switch` toggle: a controlled component reporting a requested
//! on/off value, painted as a 52×32dp track (androidx `SwitchTokens`) with a
//! spring-driven thumb (16dp unselected / 24dp selected, 28dp while pressed)
//! and the shared [`super::state_layer`] interaction overlay.
//!
//! [`switch`] produces a [`SwitchView`] carrying the current `checked` value
//! and an `on_toggle` closure — a controlled component mirroring
//! [`frust::Checkbox`]: it fires `on_toggle(state, !checked)` on release
//! inside its bounds and never flips its own `checked` field; the app mutates
//! its state and the next `rebuild` feeds the confirmed value back in.
//!
//! # Thumb travel
//!
//! The thumb's on/off position and size are driven by a
//! [`frust::AnimationController`] spring [`fling`](frust::AnimationController::fling)
//! (androidx `SwitchTokens`' `default_spatial` motion — `Theme::motion.default_spatial`,
//! falling back to the same M3 default-spatial constants when unthemed),
//! advanced during [`Widget::paint`] and re-requested via
//! [`PaintCtx::request_frame`] while in flight (the crate's shared
//! advance-during-paint contract — see `docs/CODE_STANDARDS.md`'s Theming &
//! Animation Conventions). `rebuild` only ever updates the *confirmed*
//! `checked` value (a `BuildCtx` pass has no theme to resolve a spring
//! from); the fling itself is started lazily the next time `paint` observes
//! `checked` disagreeing with the animation's last-driven target
//! (`anim_target`), which is the one place a theme is in scope.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{Action, Role, Toggled};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use frust::{AnimationController, SpringDesc, Theme, Tween};
use kurbo::{Point, Rect, Size};
use peniko::Color;

use super::press::presses;
use super::state_layer::StateLayer;

/// Track width, in logical px (androidx `SwitchTokens.TrackWidth`). No
/// `Theme` size token exists for a fixed control dimension like this
/// (`ShapeScale` publishes corner radii, not track/thumb sizes) — hoisted as
/// a named constant rather than left as a bare literal.
const TRACK_W: f64 = 52.0;
/// Track height, in logical px (androidx `SwitchTokens.TrackHeight`).
/// See [`TRACK_W`]'s doc comment — no suitable `Theme` token exists.
const TRACK_H: f64 = 32.0;
/// Thumb diameter while unselected, in logical px (androidx
/// `SwitchTokens.UnselectedHandleWidth`). See [`TRACK_W`]'s doc comment.
const THUMB_UNSELECTED: f64 = 16.0;
/// Thumb diameter while selected, in logical px (androidx
/// `SwitchTokens.SelectedHandleWidth`). See [`TRACK_W`]'s doc comment.
const THUMB_SELECTED: f64 = 24.0;
/// Thumb diameter while pressed (either state), in logical px (androidx
/// `SwitchTokens.PressedHandleWidth`) — overrides the
/// unselected/selected interpolation while a press is in progress. See
/// [`TRACK_W`]'s doc comment.
const THUMB_PRESSED: f64 = 28.0;
/// Diameter of the state-layer overlay painted behind the thumb, in logical
/// px (M3's standard 40dp interactive target size for a switch handle).
const STATE_LAYER_SIZE: f64 = 40.0;

/// Unselected track fill (unthemed fallback; a theme resolves this from
/// `colors.surface_container_highest`, per androidx `SwitchTokens`).
const TRACK_OFF: Color = Color::from_rgb8(0xE5, 0xE7, 0xEB);
/// Selected track fill (unthemed fallback; a theme resolves this from
/// `colors.primary`).
const TRACK_ON: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
/// Unselected thumb fill (unthemed fallback; a theme resolves this from
/// `colors.outline`).
const THUMB_OFF: Color = Color::from_rgb8(0x9C, 0xA3, 0xAF);
/// Selected thumb fill (unthemed fallback; a theme resolves this from
/// `colors.on_primary`).
const THUMB_ON: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);

/// The unthemed fallback spring, matching `crate::tokens::motion_scheme()`'s
/// `default_spatial` preset exactly (ζ 0.9, stiffness 700) — the thumb
/// travel animates identically whether or not a theme is threaded.
const FALLBACK_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 700.0,
    damping_ratio: 0.9,
};

/// A sub-visible release velocity used only to select which target
/// ([`AnimationController::fling`]'s `1.0`/`0.0`) the spring drives toward —
/// the actual motion is governed by the displacement (`x0`), not this
/// magnitude, so the "flick" is imperceptible; only its sign matters.
const RELEASE_VELOCITY: f64 = 1e-3;

/// Interpolate from `begin` to `end` at `t`, snapping exactly to an endpoint
/// when `t` is at (or past) `0.0`/`1.0` rather than routing it through
/// [`Tween::lerp`]'s `f32` arithmetic — which, unlike `f64`'s exact `x*1.0 ==
/// x`/`x+0.0 == x` identities, can round `begin + (end - begin) * 1.0` to a
/// value a few ULPs off `end` for colors with widely-separated channels.
/// This keeps a fully-off/-on (at-rest, unanimated) switch pixel-identical to
/// its resting color, matching every other widget's unthemed/themed-exact
/// paint guarantee (see `docs/CODE_STANDARDS.md`'s Theming conventions).
fn lerp_color_exact(begin: Color, end: Color, t: f64) -> Color {
    if t <= 0.0 {
        begin
    } else if t >= 1.0 {
        end
    } else {
        Tween::new(begin, end).lerp(t)
    }
}

/// The resolved switch paint colors: `(track_off, track_on, thumb_off,
/// thumb_on)`. Themed per androidx `SwitchTokens`: unselected track
/// `surface_container_highest`, selected track `primary`, unselected handle
/// `outline`, selected handle `on_primary`. Unthemed: the
/// [`TRACK_OFF`]/[`TRACK_ON`]/[`THUMB_OFF`]/[`THUMB_ON`] constants exactly.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color, Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (
                scheme.surface_container_highest,
                scheme.primary,
                scheme.outline,
                scheme.on_primary,
            )
        }
        None => (TRACK_OFF, TRACK_ON, THUMB_OFF, THUMB_ON),
    }
}

/// The spring driving thumb travel. Themed: `theme.motion.default_spatial`.
/// Unthemed: [`FALLBACK_SPRING`] (the same M3 default-spatial values).
fn resolve_spring(theme: Option<&Theme>) -> SpringDesc {
    match theme {
        Some(theme) => theme.motion.default_spatial.into(),
        None => FALLBACK_SPRING,
    }
}

/// A view-held, typed toggle callback (erased on build).
type OnToggle<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative M3 switch. See the [module docs](self).
pub struct SwitchView<State: 'static> {
    checked: bool,
    on_toggle: OnToggle<State>,
}

/// Create a switch reflecting `checked` that fires `on_toggle(state,
/// !checked)` on release inside its bounds.
pub fn switch<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_toggle: F,
) -> SwitchView<State> {
    SwitchView {
        checked,
        on_toggle: Rc::new(on_toggle),
    }
}

/// PascalCase alias for [`switch`].
#[allow(non_snake_case)]
pub fn Switch<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_toggle: F,
) -> SwitchView<State> {
    switch(checked, on_toggle)
}

/// The retained widget for a [`SwitchView`].
pub struct SwitchWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    checked: bool,
    /// Drives the thumb's `0.0` (off) .. `1.0` (on) travel/size fraction.
    anim: AnimationController,
    /// The `checked` value the animation is currently driving toward (or has
    /// already settled at) — compared against `checked` at paint time to
    /// decide whether a fresh fling needs to start (see the module docs).
    anim_target: bool,
    state_layer: StateLayer,
    /// The pressed *visual* state; follows the cursor in/out while captured,
    /// and also expands the thumb to [`THUMB_PRESSED`].
    pressed: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    captured: bool,
    on_toggle: frust::authoring::ErasedArgCallback<bool>,
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl<State: 'static> View<State> for SwitchView<State> {
    type Element = SwitchWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SwitchWidget {
        // Zero duration: forward()/reverse() below always snap instantly (see
        // AnimationController::start_duration), which only fling() ever
        // overrides at paint time with a real spring.
        let mut anim = AnimationController::new(Duration::ZERO);
        if self.checked {
            anim.forward();
        }
        SwitchWidget {
            checked: self.checked,
            anim,
            anim_target: self.checked,
            state_layer: StateLayer::new(),
            pressed: false,
            captured: false,
            on_toggle: frust::authoring::erase_callback_arg(&self.on_toggle),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SwitchWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_toggle = frust::authoring::erase_callback_arg(&self.on_toggle);
        if prev.checked != self.checked {
            // The app is the source of truth: adopt the new value. The
            // animation itself starts lazily in `paint`, once a theme (and
            // thus a spring) is in scope again.
            element.checked = self.checked;
            ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

impl Widget for SwitchWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(TRACK_W, TRACK_H))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (track_off, track_on, thumb_off, thumb_on) = resolve_colors(theme);
        let spring = resolve_spring(theme);

        if self.checked != self.anim_target {
            let velocity = if self.checked {
                RELEASE_VELOCITY
            } else {
                -RELEASE_VELOCITY
            };
            self.anim.fling(velocity, spring);
            self.anim_target = self.checked;
        }
        if self.anim.advance(ctx.frame_time()) {
            ctx.request_frame();
        }

        // Unclamped: a spatial spring's overshoot past the target is real,
        // intended thumb travel (see AnimationController's Overshoot docs).
        let frac = self.anim.value();
        // Clamped: color/size never extrapolate past their end values.
        let frac_clamped = self.anim.value_clamped();

        let o = ctx.origin();
        let track_color = lerp_color_exact(track_off, track_on, frac_clamped);
        scene.fill_rounded_rect(o, Size::new(TRACK_W, TRACK_H), TRACK_H / 2.0, track_color);

        let thumb_color = lerp_color_exact(thumb_off, thumb_on, frac_clamped);
        let base_diam = Tween::new(THUMB_UNSELECTED, THUMB_SELECTED).lerp(frac_clamped);
        let diam = if self.pressed {
            THUMB_PRESSED
        } else {
            base_diam
        };

        let min_center_x = o.x + TRACK_H / 2.0;
        let max_center_x = o.x + TRACK_W - TRACK_H / 2.0;
        let center_x = min_center_x + (max_center_x - min_center_x) * frac;
        let center_y = o.y + TRACK_H / 2.0;

        self.state_layer.paint(
            ctx,
            scene,
            Rect::from_center_size(
                Point::new(center_x, center_y),
                Size::new(STATE_LAYER_SIZE, STATE_LAYER_SIZE),
            ),
            STATE_LAYER_SIZE / 2.0,
            thumb_color,
        );

        scene.fill_rounded_rect(
            Point::new(center_x - diam / 2.0, center_y - diam / 2.0),
            Size::new(diam, diam),
            diam / 2.0,
            thumb_color,
        );
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                self.state_layer.set_pressed(true);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                let inside_now = inside(p.position, ctx.size());
                self.pressed = inside_now;
                self.state_layer.set_pressed(inside_now);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
                    // Report the *requested* value; never self-toggle.
                    let requested = !self.checked;
                    (self.on_toggle)(ctx, requested);
                }
                self.pressed = false;
                self.captured = false;
                self.state_layer.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                self.state_layer.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Switch, |node| {
            node.set_toggled(Toggled::from(self.checked));
            node.add_action(Action::Click);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use std::any::Any;

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    #[derive(Default)]
    struct ToggleState {
        last: Option<bool>,
        toggles: u32,
    }

    fn widget(checked: bool) -> SwitchWidget {
        let view = switch::<ToggleState, _>(checked, |s: &mut ToggleState, v: bool| {
            s.last = Some(v);
            s.toggles += 1;
        });
        let mut counter = 0u64;
        View::<ToggleState>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Primary,
        })
    }

    /// The same event on the secondary (right) button.
    fn secondary_ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Secondary,
        })
    }

    fn dispatch(w: &mut SwitchWidget, state: &mut ToggleState, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(TRACK_W, TRACK_H));
        w.event(&mut ctx, event);
    }

    #[test]
    fn a_secondary_press_never_presses_captures_or_toggles() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(
            &mut w,
            &mut state,
            &secondary_ev(PointerPhase::Down, 5.0, 12.0),
        );
        assert!(!w.pressed, "no pressed state layer on a right-click");
        assert!(!w.captured, "and no capture for the shell to wedge on");
        dispatch(
            &mut w,
            &mut state,
            &secondary_ev(PointerPhase::Up, 5.0, 12.0),
        );
        assert_eq!(state.toggles, 0);

        // The primary gesture is untouched by the guard.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.toggles, 1);
    }

    #[test]
    fn off_fires_true_and_does_not_self_toggle() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.last, Some(true));
        assert_eq!(state.toggles, 1);
        assert!(!w.checked, "switch must not mutate its own checked flag");
    }

    #[test]
    fn on_fires_false() {
        let mut w = widget(true);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.last, Some(false));
        assert!(w.checked, "still checked until the app rebuilds it");
    }

    #[test]
    fn up_outside_does_not_fire() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 12.0));
        assert_eq!(state.toggles, 0);
    }

    #[test]
    fn hover_move_without_down_is_ignored_noop() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(TRACK_W, TRACK_H));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 5.0, 12.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.pressed, "hover must not press");
        assert!(!ctx.needs_redraw(), "hover must not request a redraw");
        assert_eq!(state.toggles, 0);
    }

    #[test]
    fn cancel_clears_armed_state() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 5.0, 12.0));
        assert!(!w.captured, "Cancel disarms the press");
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.toggles, 0);
    }

    #[test]
    fn rebuild_adopts_new_checked_value_without_self_mutation() {
        let mut counter = 0u64;
        let prev = switch::<ToggleState, _>(false, |_s, _v| {});
        let mut w = View::<ToggleState>::build(&prev, &mut BuildCtx::new(&mut counter));
        assert!(!w.checked);
        assert!(!w.anim_target);
        let next = switch::<ToggleState, _>(true, |_s, _v| {});
        let flags =
            View::<ToggleState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.checked, "the confirmed value adopts immediately");
        assert!(
            !w.anim_target,
            "the animation target only updates lazily in paint, once a theme is in scope"
        );
        assert!(flags.needs_paint());
    }

    /// Records each rounded rect's `(origin, size, radius, color)` in paint order:
    /// track, then (if active) the state-layer overlay, then the thumb.
    #[derive(Default)]
    struct RRectRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for RRectRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
    }

    fn paint_rec(w: &mut SwitchWidget, theme: Option<&Theme>) -> RRectRecorder {
        let mut rec = RRectRecorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H)).with_theme(t),
            None => PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H)),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn unthemed_paint_uses_fallback_constants() {
        let mut off = widget(false);
        let rec = paint_rec(&mut off, None);
        // Track first, thumb last (no active state layer): 2 rects.
        assert_eq!(rec.rrects.len(), 2, "no active state layer paints nothing");
        assert_eq!(rec.rrects[0].3, TRACK_OFF);
        assert_eq!(rec.rrects[0].2, TRACK_H / 2.0, "pill radius");
        assert_eq!(rec.rrects[1].3, THUMB_OFF);
        assert_eq!(
            rec.rrects[1].1,
            Size::new(THUMB_UNSELECTED, THUMB_UNSELECTED)
        );

        let mut on = widget(true);
        let rec = paint_rec(&mut on, None);
        assert_eq!(rec.rrects[0].3, TRACK_ON);
        assert_eq!(rec.rrects[1].3, THUMB_ON);
        assert_eq!(rec.rrects[1].1, Size::new(THUMB_SELECTED, THUMB_SELECTED));
    }

    #[test]
    fn themed_paint_resolves_switch_tokens() {
        let theme = crate::baseline();
        let scheme = theme.scheme();
        let mut off = widget(false);
        let rec = paint_rec(&mut off, Some(&theme));
        assert_eq!(rec.rrects[0].3, scheme.surface_container_highest);
        assert_eq!(rec.rrects[1].3, scheme.outline);

        let mut on = widget(true);
        let rec = paint_rec(&mut on, Some(&theme));
        assert_eq!(rec.rrects[0].3, scheme.primary);
        assert_eq!(rec.rrects[1].3, scheme.on_primary);
    }

    #[test]
    fn pressed_thumb_expands_regardless_of_state() {
        let mut w = widget(false);
        w.pressed = true;
        let rec = paint_rec(&mut w, None);
        assert_eq!(rec.rrects[1].1, Size::new(THUMB_PRESSED, THUMB_PRESSED));
    }

    #[test]
    fn paint_starts_a_fling_when_checked_disagrees_with_anim_target() {
        let mut w = widget(false);
        w.checked = true; // simulate the confirmed value rebuild adopts
        assert!(!w.anim.is_animating());
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H));
        let mut rec = RRectRecorder::default();
        w.paint(&mut ctx, &mut rec);
        assert!(w.anim_target, "paint syncs the animation target");
        assert!(
            w.anim.is_animating(),
            "a fling toward the new target started"
        );
        assert!(
            ctx.needs_frame(),
            "an in-flight fling must request another frame"
        );
    }

    #[test]
    fn animation_progresses_toward_and_settles_at_target() {
        let mut w = widget(false);
        w.checked = true;
        // First paint starts the fling (module docs: lazily, once themed).
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H));
        let mut rec = RRectRecorder::default();
        w.paint(&mut ctx, &mut rec);

        // Directly advance the animation controller with injected frame
        // times (mirrors frust-core::anim's own test pattern) rather than
        // through PaintCtx, whose frame_time setter is crate-private.
        let mut running = true;
        let mut t = 0.0;
        for _ in 0..100_000 {
            running = w.anim.advance(ft_secs(t));
            if !running {
                break;
            }
            t += 1.0 / 120.0;
        }
        assert!(!running, "fling failed to settle");
        assert!((w.anim.value() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn semantics_reports_switch_role_toggled_state_and_bounds() {
        // SemanticsCtx is only constructible inside frust-core (its `new`/
        // `finish` are crate-private there), so — mirroring
        // `tests/semantics_tree.rs` — this drives the widget through a real
        // `RenderRoot` rebuild/layout/semantics pass rather than poking the
        // pass's context directly.
        fn logic(_state: &mut ToggleState) -> SwitchView<ToggleState> {
            switch::<ToggleState, _>(true, |_s, _v| {})
        }
        let mut root: frust_core::RenderRoot<ToggleState, SwitchView<ToggleState>> =
            frust_core::RenderRoot::new();
        let mut state = ToggleState::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        let update = root.semantics();

        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Switch)
            .expect("switch contributes a Role::Switch node");
        assert_eq!(node.toggled(), Some(Toggled::True));
        assert!(node.supports_action(Action::Click));
        let bounds = node.bounds().expect("switch node has bounds");
        assert_eq!((bounds.x0, bounds.y0), (0.0, 0.0));
        assert_eq!(
            (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0),
            (TRACK_W, TRACK_H)
        );
    }
}
