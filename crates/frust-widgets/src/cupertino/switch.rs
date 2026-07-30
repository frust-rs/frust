//! `CupertinoSwitch`: the iOS toggle switch, a controlled component
//! mirroring [`crate::material::switch`]'s architecture, with the kit-cited
//! 64×28pt track, a systemGreen "on" track, and a white spring-driven thumb
//! carrying a static specular-highlight/drop-shadow "glassy knob" treatment
//! (see Reflective knob below).
//!
//! [`cupertino_switch`] produces a [`CupertinoSwitchView`] carrying the current
//! `checked` value and an `on_toggle` closure — a controlled component (see
//! `docs/CODE_STANDARDS.md`'s Interaction Semantics): it fires
//! `on_toggle(state, !checked)` on release inside its bounds and never flips its
//! own `checked` field; the app mutates its state and the next `rebuild` feeds
//! the confirmed value back in.
//!
//! # Track metric resolution
//!
//! The module previously shipped a **community-approximate** 51×31pt track
//! (Flutter's `CupertinoSwitch` value; a competing Apple developer-forums
//! thread claimed 49×31pt — both were contested, with neither independently
//! verified). This was resolved against the Apple iOS 27 UI Kit's own mined
//! design-file data: the kit's `Toggles/{Light,Dark}/{On,Off}/{1 -
//! Idle,3 - Pressed,4 - Disabled}` layer records — 8 of 8 — unanimously give a
//! `64×28` track. That is a real, citable kit record (unlike an earlier
//! community-sourced summary claiming the same number without an actual
//! page/layer citation), so — adopting kit numbers only when they trace to a
//! real, citable kit record — the track ships at **64×28** — see
//! [`TRACK_W`]/[`TRACK_H`]. The thumb inset stays the pre-existing
//! community-approximate derivation (see [`THUMB_INSET`]'s doc comment for
//! why the kit's own ambiguous `"Knob"` record wasn't adopted for it).
//!
//! # Thumb travel
//!
//! The thumb's on/off position is driven by a
//! [`frust_core::AnimationController`] spring
//! [`fling`](frust_core::AnimationController::fling) (the theme's
//! `motion.default_spatial` — [`Theme::cupertino_baseline`]'s single documented
//! iOS spring — falling back to the same values when unthemed), advanced during
//! [`Widget::paint`] and re-requested via [`PaintCtx::request_frame`] while in
//! flight (the crate's shared advance-during-paint contract). The fling starts
//! lazily the next time `paint` observes `checked` disagreeing with the
//! animation's last-driven target (`anim_target`) — the one place a theme (and
//! thus a spring) is in scope — exactly as [`crate::material::switch`] does.
//!
//! # Reflective knob
//!
//! iOS 26+'s native switch renders a reflective/glassy knob; vello 0.9 has no
//! backdrop-blur or lensing primitive, so this ships a **static
//! approximation**: a soft drop shadow beneath the knob sourced from
//! `theme.glass.control`'s [`frust_theme::elevation::ShadowSpec`]
//! (the same "buttons/toggles" glass tier [`crate::cupertino`]'s other
//! widgets read — see `frust-theme/src/glass.rs`'s module docs; unthemed
//! falls back to [`GlassScale::ios27`]'s `control` tier, the same values a
//! Cupertino-themed paint resolves), then a radial specular-gradient highlight
//! offset toward the upper-left, peaked at the tier's `hairline_alpha`. The
//! kit's own `Toggles/Toggles/𝛘/{Light,Dark}/Knob` records carry a soft
//! ambient glow (blur 20) plus tight top/bottom rim shadows (blur 0.5) around
//! the knob — this is the source the shadow+highlight pairing approximates,
//! not a literal replay of those blur radii (vello has no blur-layer
//! primitive to replay them with).

use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::{Action, Role, Toggled};
use frust_core::{
    AnimationController, BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, SpringDesc, Tween, View, Widget,
};
use frust_theme::{Brightness, GlassMaterial, GlassScale, Theme};
use kurbo::{Point, Size};
use peniko::{Brush, Color, Gradient};

/// Track width, in logical px.
///
/// **Kit-cited** (retrieved 2026-07-18): the Apple iOS 27 UI Kit's
/// `Toggles/{Light,Dark}/{On,Off}/{1 - Idle,3 - Pressed,4 - Disabled}` layer
/// records — 8 of 8 — unanimously give `64`. This resolves the module's prior
/// community-approximate 51×31 (Flutter's `CupertinoSwitch` value; a
/// competing Apple developer-forums thread claimed 49×31) in the kit's favor
/// — see the module docs' Track metric resolution section.
const TRACK_W: f64 = 64.0;
/// Track height, in logical px (see [`TRACK_W`]'s kit citation — the same 8/8
/// records agree on `28`).
const TRACK_H: f64 = 28.0;
/// Inset from the track edge to the thumb, in logical px.
///
/// **Community-approximate**, kept rather than kit-adopted: the kit also
/// carries a flat `"Knob"` record (`38×24`)
/// but it is not attributable with confidence to the standard `64×28` track
/// group above — unlike every track record it carries no `Light|Dark`/
/// `On|Off` prefix, and sits alongside a `Toggles/𝛘/{Light,Dark}/Knob` pair
/// at `59×37` naming a *different* (icon) toggle variant the kit also ships,
/// so `"Knob"` may belong to that variant instead. Its 38pt width would also
/// leave only ~22pt of thumb travel inside a 64pt track — implausibly little
/// for a resting thumb. Since kit numbers are adopted only when they trace
/// to a real, citable kit record, an ambiguous record isn't citable, so this
/// module keeps the pre-existing derivation instead: iOS does not publish
/// the thumb inset, and ~2pt is the value
/// community reimplementations converge on (thumb diameter = `TRACK_H -
/// 2*THUMB_INSET`). The 24pt result this derivation lands on for the new 28pt
/// track does, at least, corroborate the ambiguous record's own height field.
const THUMB_INSET: f64 = 2.0;
/// Thumb diameter, in logical px (derived from [`TRACK_H`]/[`THUMB_INSET`]).
const THUMB_DIAM: f64 = TRACK_H - 2.0 * THUMB_INSET;

/// systemGreen (light), the "on" track fill.
///
/// **Kit-measured** (2026-07-18 refresh): the iOS 27 UI Kit's `System
/// Colors/Light/4 Green` swatch is
/// `#34C658` — Apple does not publish an exact hex for the system accent
/// colors (they vary by trait environment), so this remains a design-tool
/// snapshot rather than a guarantee, just a newer/more-precise one than the
/// pre-refresh community value it replaces (`#34C759`), mirroring the same
/// kind of 1-bit-per-channel refinement `color.rs`'s Cupertino
/// refresh documents for systemBlue/Red/Purple.
const SYSTEM_GREEN_LIGHT: Color = Color::from_rgb8(0x34, 0xC6, 0x58);
/// systemGreen (dark) — the kit's `System Colors/Dark/4 Green` swatch,
/// `#2FD157` (see [`SYSTEM_GREEN_LIGHT`]; refines the pre-refresh `#30D158`).
const SYSTEM_GREEN_DARK: Color = Color::from_rgb8(0x2F, 0xD1, 0x57);
/// The "off" track fill (light) — iOS systemGray5.
///
/// **Community-measured**, re-checked against the 2026-07-18 kit refresh and
/// kept unchanged: the kit's `Grays/*` swatch group (six generically-named
/// steps, `Gray`..`Gray 6`) has no record naming which step (if any) the
/// toggle's off-track fill actually uses, unlike the on-track's directly-named
/// `System Colors/*/4 Green` swatch above — adopting one by positional guess
/// (`Grays/Light/Gray 5` is `#E5E5E9`, close but not identical to the value
/// below) would violate the cite-a-real-record evidence rule this module
/// follows, so the pre-refresh community value stays.
const TRACK_OFF_LIGHT: Color = Color::from_rgb8(0xE9, 0xE9, 0xEA);
/// The "off" track fill (dark) — iOS systemGray5 dark (see
/// [`TRACK_OFF_LIGHT`]'s re-check note; kept unchanged for the same reason —
/// the kit's nearest-named candidate, `Grays/Dark/Gray 5` at `#2B2B2D`, reads
/// implausibly dark for a switch track against a dark background, so it was
/// not adopted either).
const TRACK_OFF_DARK: Color = Color::from_rgb8(0x39, 0x39, 0x3D);
/// The thumb fill — white in both light and dark (iOS keeps the knob white).
const THUMB: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);

/// The unthemed fallback spring, matching [`Theme::cupertino_baseline`]'s
/// `motion.default_spatial` exactly (ζ ≈ 0.5753, stiffness 170) — the thumb
/// travels identically whether or not a theme is threaded.
const FALLBACK_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 170.0,
    damping_ratio: 0.5753,
};

/// A sub-visible release velocity used only to select which target
/// ([`AnimationController::fling`]'s `1.0`/`0.0`) the spring drives toward —
/// only its sign matters (mirrors [`crate::material::switch`]).
const RELEASE_VELOCITY: f64 = 1e-3;

/// Peak alpha of the knob's specular-highlight gradient at its center,
/// expressed as a multiplier on the resolved control glass tier's
/// `hairline_alpha` (**tuned Frust policy**, mirroring `GlassScale`'s own
/// per-tier tuning — see `frust-theme/src/glass.rs`'s module docs). See
/// the module docs' Reflective knob section for what this approximates and
/// why (no vello blur/lensing primitive to replay the kit's real glow/rim
/// shadow stack with).
const SPECULAR_PEAK_SCALE: f32 = 1.8;

/// The `control` glass tier (buttons/toggles — see `frust-theme/src/glass.rs`)
/// this widget's knob highlight/shadow reads. Themed: `theme.glass.control`
/// (always [`GlassScale::ios27`]'s tier on a Cupertino-tagged `Theme`,
/// per [`Theme::cupertino_baseline`]). Unthemed: the same `ios27` control
/// tier directly, so an unthemed paint matches a Cupertino-themed one exactly
/// — there is no separate hand-tuned fallback to drift out of sync.
fn resolve_control_glass(theme: Option<&Theme>) -> GlassMaterial {
    theme
        .map(|t| t.glass.control.clone())
        .unwrap_or_else(|| GlassScale::ios27().control)
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors the
/// per-module helper of the same shape used across `frust-widgets`, e.g.
/// `material::card`'s).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The active `(track_on, track_off, thumb)` colors, selected by the theme's
/// brightness (or the light fallbacks when unthemed).
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color, Color) {
    let brightness = theme.map(|t| t.brightness).unwrap_or(Brightness::Light);
    match brightness {
        Brightness::Light => (SYSTEM_GREEN_LIGHT, TRACK_OFF_LIGHT, THUMB),
        Brightness::Dark => (SYSTEM_GREEN_DARK, TRACK_OFF_DARK, THUMB),
    }
}

/// The spring driving thumb travel. Themed: `theme.motion.default_spatial`.
/// Unthemed: [`FALLBACK_SPRING`] (the same iOS baseline values).
fn resolve_spring(theme: Option<&Theme>) -> SpringDesc {
    match theme {
        Some(theme) => theme.motion.default_spatial.into(),
        None => FALLBACK_SPRING,
    }
}

/// Interpolate `begin`→`end` at `t`, snapping exactly to an endpoint at (or
/// past) `0.0`/`1.0` rather than routing through [`Tween::lerp`]'s `f32`
/// arithmetic — which can round a fully-off/-on track a few ULPs off its
/// resting color. Keeps a settled switch pixel-identical to its resting fill,
/// matching the crate's unthemed/themed-exact paint guarantee (mirrors
/// [`crate::material::switch`]'s helper of the same shape, reimplemented here
/// since that helper is private to its own module).
fn lerp_color_exact(begin: Color, end: Color, t: f64) -> Color {
    if t <= 0.0 {
        begin
    } else if t >= 1.0 {
        end
    } else {
        Tween::new(begin, end).lerp(t)
    }
}

/// A view-held, typed toggle callback (erased on build).
type OnToggle<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative iOS switch. See the [module docs](self).
pub struct CupertinoSwitchView<State: 'static> {
    checked: bool,
    on_toggle: OnToggle<State>,
}

/// Create an iOS switch reflecting `checked` that fires `on_toggle(state,
/// !checked)` on release inside its bounds.
pub fn cupertino_switch<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_toggle: F,
) -> CupertinoSwitchView<State> {
    CupertinoSwitchView {
        checked,
        on_toggle: Rc::new(on_toggle),
    }
}

/// PascalCase alias for [`cupertino_switch`].
#[allow(non_snake_case)]
pub fn CupertinoSwitch<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_toggle: F,
) -> CupertinoSwitchView<State> {
    cupertino_switch(checked, on_toggle)
}

/// The retained widget for a [`CupertinoSwitchView`].
pub struct CupertinoSwitchWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    checked: bool,
    /// Drives the thumb's `0.0` (off) .. `1.0` (on) travel fraction.
    anim: AnimationController,
    /// The `checked` value the animation is currently driving toward — compared
    /// against `checked` at paint time to decide whether a fresh fling starts.
    anim_target: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    captured: bool,
    on_toggle: crate::authoring::ErasedArgCallback<bool>,
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl<State: 'static> View<State> for CupertinoSwitchView<State> {
    type Element = CupertinoSwitchWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CupertinoSwitchWidget {
        // Zero duration: forward()/reverse() snap instantly; only fling()
        // overrides at paint time with a real spring (mirrors material switch).
        let mut anim = AnimationController::new(Duration::ZERO);
        if self.checked {
            anim.forward();
        }
        CupertinoSwitchWidget {
            checked: self.checked,
            anim,
            anim_target: self.checked,
            captured: false,
            on_toggle: crate::authoring::erase_callback_arg(&self.on_toggle),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CupertinoSwitchWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_toggle = crate::authoring::erase_callback_arg(&self.on_toggle);
        if prev.checked != self.checked {
            // The app is the source of truth: adopt the new value. The fling
            // starts lazily in `paint`, once a theme (spring) is in scope.
            element.checked = self.checked;
            ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

impl Widget for CupertinoSwitchWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(TRACK_W, TRACK_H))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (track_on, track_off, thumb) = resolve_colors(theme);
        let spring = resolve_spring(theme);
        // Resolved eagerly (before any `&mut ctx` use below) since it, unlike
        // `theme`, is an owned value carried past `ctx.request_frame()`.
        let glass_control = resolve_control_glass(theme);

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

        // Clamped for the track color crossfade; raw for the thumb travel so a
        // spatial spring's overshoot is real, intended motion.
        let frac = self.anim.value();
        let frac_clamped = self.anim.value_clamped();

        let o = ctx.origin();
        let track_color = lerp_color_exact(track_off, track_on, frac_clamped);
        scene.fill_rounded_rect(o, Size::new(TRACK_W, TRACK_H), TRACK_H / 2.0, track_color);

        let min_center_x = o.x + THUMB_INSET + THUMB_DIAM / 2.0;
        let max_center_x = o.x + TRACK_W - THUMB_INSET - THUMB_DIAM / 2.0;
        let center_x = min_center_x + (max_center_x - min_center_x) * frac;
        let center_y = o.y + TRACK_H / 2.0;
        let thumb_origin = Point::new(center_x - THUMB_DIAM / 2.0, center_y - THUMB_DIAM / 2.0);
        let thumb_size = Size::new(THUMB_DIAM, THUMB_DIAM);

        // Reflective knob (see the module docs): a soft drop shadow from the
        // control glass tier, under the base fill, then a specular gradient
        // highlight on top — the static approximation vello's lack of a
        // backdrop-blur primitive limits this to.
        scene.draw_shadow(
            Point::new(
                thumb_origin.x,
                thumb_origin.y + glass_control.shadow.y_offset,
            ),
            thumb_size,
            THUMB_DIAM / 2.0,
            glass_control.shadow.blur_std_dev,
            with_alpha(Color::BLACK, glass_control.shadow.color_alpha),
        );

        scene.fill_rounded_rect(thumb_origin, thumb_size, THUMB_DIAM / 2.0, thumb);

        let highlight_center =
            Point::new(center_x - THUMB_DIAM * 0.18, center_y - THUMB_DIAM * 0.28);
        let peak_alpha = (glass_control.hairline_alpha * SPECULAR_PEAK_SCALE).min(1.0);
        let highlight = Gradient::new_radial(highlight_center, (THUMB_DIAM * 0.65) as f32)
            .with_stops([
                (0.0f32, with_alpha(thumb, peak_alpha)),
                (1.0f32, with_alpha(thumb, 0.0)),
            ]);
        scene.fill_rounded_rect_brush(
            thumb_origin,
            thumb_size,
            THUMB_DIAM / 2.0,
            &Brush::Gradient(highlight),
        );
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
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
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
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
    use frust_core::FrameTime;
    use std::any::Any;

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    #[derive(Default)]
    struct ToggleState {
        last: Option<bool>,
        toggles: u32,
    }

    fn widget(checked: bool) -> CupertinoSwitchWidget {
        let view = cupertino_switch::<ToggleState, _>(checked, |s: &mut ToggleState, v: bool| {
            s.last = Some(v);
            s.toggles += 1;
        });
        let mut counter = 0u64;
        View::<ToggleState>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut CupertinoSwitchWidget, state: &mut ToggleState, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(TRACK_W, TRACK_H));
        w.event(&mut ctx, event);
    }

    #[test]
    fn ships_the_kit_cited_64x28_track() {
        let mut w = widget(false);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert_eq!(size, Size::new(64.0, 28.0));
    }

    #[test]
    fn off_fires_true_and_does_not_self_toggle() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 15.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 15.0));
        assert_eq!(state.last, Some(true));
        assert_eq!(state.toggles, 1);
        assert!(!w.checked, "switch must not mutate its own checked flag");
    }

    #[test]
    fn on_fires_false() {
        let mut w = widget(true);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 15.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 15.0));
        assert_eq!(state.last, Some(false));
        assert!(w.checked, "still checked until the app rebuilds it");
    }

    #[test]
    fn up_outside_does_not_fire() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 15.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 15.0));
        assert_eq!(state.toggles, 0);
    }

    #[test]
    fn cancel_clears_armed_state() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 15.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 5.0, 15.0));
        assert!(!w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 15.0));
        assert_eq!(state.toggles, 0);
    }

    /// Records each rounded rect's `(size, radius, color)` in paint order:
    /// track, then thumb. `shadows`/`brushes` separately record the knob's
    /// reflective-treatment calls (`draw_shadow`/`fill_rounded_rect_brush`),
    /// which don't land in `rrects`.
    #[derive(Default)]
    struct RRectRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        brushes: Vec<(Point, Size, f64, Brush)>,
    }

    impl PaintScene for RRectRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
        fn fill_rounded_rect_brush(&mut self, o: Point, s: Size, radius: f64, brush: &Brush) {
            self.brushes.push((o, s, radius, brush.clone()));
        }
    }

    fn paint_rec(w: &mut CupertinoSwitchWidget, theme: Option<&Theme>) -> RRectRecorder {
        let mut rec = RRectRecorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H)).with_theme(t),
            None => PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H)),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn off_paints_gray_track_and_white_thumb_on_the_left() {
        let mut w = widget(false);
        let rec = paint_rec(&mut w, None);
        assert_eq!(rec.rrects.len(), 2, "track + thumb");
        assert_eq!(rec.rrects[0].3, TRACK_OFF_LIGHT);
        assert_eq!(rec.rrects[0].2, TRACK_H / 2.0, "pill radius");
        assert_eq!(rec.rrects[1].3, THUMB);
        assert_eq!(rec.rrects[1].1, Size::new(THUMB_DIAM, THUMB_DIAM));
        // Thumb hugs the left inset when off.
        assert!((rec.rrects[1].0.x - THUMB_INSET).abs() < 1e-9);
    }

    #[test]
    fn on_paints_green_track_and_thumb_on_the_right() {
        let mut w = widget(true);
        let rec = paint_rec(&mut w, None);
        assert_eq!(rec.rrects[0].3, SYSTEM_GREEN_LIGHT);
        // Thumb hugs the right inset when on.
        let expected_x = TRACK_W - THUMB_INSET - THUMB_DIAM;
        assert!((rec.rrects[1].0.x - expected_x).abs() < 1e-9);
    }

    #[test]
    fn dark_theme_selects_dark_track_colors() {
        let mut theme = Theme::cupertino_baseline();
        theme.brightness = Brightness::Dark;
        let mut on = widget(true);
        let rec = paint_rec(&mut on, Some(&theme));
        assert_eq!(rec.rrects[0].3, SYSTEM_GREEN_DARK);
        let mut off = widget(false);
        let rec = paint_rec(&mut off, Some(&theme));
        assert_eq!(rec.rrects[0].3, TRACK_OFF_DARK);
    }

    #[test]
    fn paint_draws_one_soft_shadow_beneath_the_knob_from_the_control_glass_tier() {
        let mut w = widget(false);
        let rec = paint_rec(&mut w, None);
        assert_eq!(rec.shadows.len(), 1, "one drop shadow, under the knob");
        let expected = GlassScale::ios27().control.shadow;
        assert_eq!(rec.shadows[0].2, THUMB_DIAM / 2.0, "circular, knob radius");
        assert_eq!(rec.shadows[0].3, expected.blur_std_dev);
        assert_eq!(rec.shadows[0].4.components[3], expected.color_alpha);
    }

    #[test]
    fn paint_draws_one_specular_gradient_highlight_on_the_knob() {
        let mut w = widget(false);
        let rec = paint_rec(&mut w, None);
        assert_eq!(rec.brushes.len(), 1, "one gradient highlight on the knob");
        assert_eq!(rec.brushes[0].2, THUMB_DIAM / 2.0, "circular, knob radius");
        match &rec.brushes[0].3 {
            Brush::Gradient(g) => assert_eq!(g.stops.len(), 2, "peak + fade-to-transparent"),
            _ => panic!("expected a gradient brush for the specular highlight"),
        }
    }

    #[test]
    fn cupertino_themed_and_unthemed_knob_treatment_match() {
        // No hand-tuned unthemed fallback to drift: both read the same
        // ios27 control glass tier (see resolve_control_glass's doc comment).
        let mut themed = widget(false);
        let theme = Theme::cupertino_baseline();
        let rec_themed = paint_rec(&mut themed, Some(&theme));
        let mut unthemed = widget(false);
        let rec_unthemed = paint_rec(&mut unthemed, None);
        assert_eq!(rec_themed.shadows[0].3, rec_unthemed.shadows[0].3);
        assert_eq!(rec_themed.shadows[0].4, rec_unthemed.shadows[0].4);
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
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H));
        let mut rec = RRectRecorder::default();
        w.paint(&mut ctx, &mut rec); // starts the fling

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
    fn semantics_reports_switch_role_and_toggled_state() {
        fn logic(_state: &mut ToggleState) -> CupertinoSwitchView<ToggleState> {
            cupertino_switch::<ToggleState, _>(true, |_s, _v| {})
        }
        let mut root: frust_core::RenderRoot<ToggleState, CupertinoSwitchView<ToggleState>> =
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
    }
}
