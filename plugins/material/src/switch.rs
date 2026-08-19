//! The Material 3 Expressive `Switch` toggle: a controlled component
//! reporting a requested on/off value, painted as a 52×32dp track with a
//! spring-driven thumb (16dp unselected / 24dp selected, 32dp while pressed)
//! and the shared [`super::state_layer`] interaction overlay.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/switch_control/m3e_switch_control.dart` +
//! `styles/m3e_switch_theme.dart` (retrieved 2026-08-19).
//! Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! [`switch`] produces a [`SwitchView`] carrying the current `checked` value
//! and an `on_toggle` closure — a controlled component mirroring
//! [`frust::Checkbox`]: it fires `on_toggle(state, !checked)` on release
//! inside its bounds and never flips its own `checked` field; the app
//! mutates its state and the next `rebuild` feeds the confirmed value back
//! in.
//!
//! # Two independent springs, not one
//!
//! The reference drives the thumb's slide and its size off two *separate*
//! `SingleMotionController`s (`m3e_switch_control.dart:60-61`), both built on
//! `MaterialSpringMotion.expressiveSpatialDefault` (stiffness `380`,
//! `tmp/motor-1.1.0/lib/src/motion.dart:610-616`) but each `copyWith`-ing a
//! different damping ratio: position at `0.55` (a visible overshoot "snap")
//! and size at `0.7` (a more directly-settling follow) —
//! `m3e_switch_control.dart:66-76`. [`POSITION_SPRING`]/[`SIZE_SPRING`]
//! transcribe those two specs exactly. Both are driven in-widget during
//! [`Widget::paint`] via [`AnimationController::fling`], re-requested with
//! [`PaintCtx::request_frame`] while either is in flight (the crate's shared
//! advance-during-paint contract — see `docs/CODE_STANDARDS.md`'s Theming &
//! Animation Conventions). `rebuild` only ever updates the *confirmed*
//! `checked` value; both flings start lazily the next time `paint` observes
//! `checked` disagreeing with `anim_target`.
//!
//! Motor's `damping` parameter feeds `SpringDescription.withDurationAndBounce`-style
//! construction as a damping *ratio* (`motion.dart:678-690`: `ratio: damping`),
//! the same ζ semantics [`frust::SpringDesc::damping_ratio`] uses — so these two
//! constants carry over as a direct `(mass, stiffness, damping_ratio)` triple
//! with no unit conversion.
//!
//! # Thumb travel and press-grow
//!
//! The thumb's bounding box is computed in closed form
//! (`m3e_switch_control.dart:132` padding + `:171-187`'s `_buildThumb`,
//! collapsed from Flutter's nested `LayoutBuilder`-in-padding-with-bleed
//! shape into one pair of expressions — the arithmetic is unchanged, only
//! not re-derived from a separate constraints pass): [`thumb_origin`]. Both
//! the position spring's `value()` (used for on-axis travel) and the size
//! spring's `value()` (used for the unselected↔selected diameter blend) are
//! read **unclamped** — an under-damped spring's overshoot past its target
//! is real, intended motion (see [`AnimationController`]'s Overshoot docs),
//! and the reference itself never clamps either read in this formula. A
//! press instantly overrides the diameter to [`THUMB_PRESSED`] regardless of
//! the size spring's own value (`m3e_switch_control.dart:171-173`) — this is
//! not spring-driven; it snaps the instant `pressed` flips, same as the
//! pre-rework behavior.
//!
//! # Icons: value-selected, size-faded
//!
//! [`SwitchView::selected_icon`]/[`SwitchView::unselected_icon`] each take an
//! optional [`frust::IconSource`], painted inside the thumb
//! (`m3e_switch_control.dart:233-258`). The reference does **not** cross-fade
//! between the two icons — it picks *one* icon from the **confirmed**
//! `checked` boolean (not the animated position), then fades/scales *that*
//! icon by the **size** spring's clamped `[0, 1]` value
//! (`:242`: `t = _sizeCtrl.value.clamp(0.0, 1.0)`; `:243-246`: `opacity: t`,
//! `scale: 0.5 + 0.5 * t`). One consequence, transcribed faithfully rather
//! than "fixed": toggling off shows the *unselected* icon (chosen the instant
//! `checked` flips) fading out as the thumb *shrinks* (`t` falling from ~1 to
//! 0), not fading in — the reference's own shape, not a symmetric two-icon
//! cross-fade.
//!
//! # Track/thumb/outline/icon/state-layer colors
//!
//! [`SwitchColors`] plus [`track_color`]/[`thumb_color`]/[`outline_color`]/
//! [`icon_color`]/[`state_layer_color`] transcribe
//! `M3ESwitchTheme`'s five color methods exactly
//! (`m3e_switch_theme.dart:76-123`), including that `icon_color`/
//! `state_layer_color` take no `enabled` branch in the source — an
//! asymmetry preserved here rather than "corrected". An off-track outline
//! ring (`m3e_switch_theme.dart:103-113`'s `outlineColor`,
//! `m3e_switch_control.dart:140-148`'s `Border.all`) paints only while
//! `!checked`, at [`BORDER_WIDTH`].
//!
//! **Porting decision — continuous color, not the reference's separate
//! crossfade.** The reference re-paints track/thumb/outline color through an
//! outer `AnimatedContainer`'s own implicit ~150ms linear duration crossfade
//! (`m3e_switch_control.dart:129`, `M3EMotion.short3`), a *third* animation
//! channel independent of the two springs above. This port instead lerps
//! each color pair by the position spring's `value_clamped()` — one fewer
//! animation channel, and no harsher a transition than the reference's own
//! short (150ms) one — the same continuous-color idiom the pre-rework
//! version of this module used.
//!
//! # No drag-to-toggle
//!
//! The reference wires only a tap (`M3ETappable(onTap: ...)`,
//! `m3e_switch_control.dart:121`) — no pan/drag gesture anywhere in the
//! component. None is added here either.
//!
//! # Haptics: wired, firing `None`
//!
//! `M3ETappable` always fires a haptic on tap
//! (`tmp/material_3_expressive/lib/foundations/m3e_tappable.dart:187-198`'s
//! `_wrapTap`/`_fireHaptic`), but `M3ESwitch` never overrides `M3ETappable`'s
//! `haptic` field, so it fires at that field's own default,
//! `M3EHapticFeedback.none` — a documented no-op
//! (`m3e_haptics.dart`'s `trigger` `none` case). This port matches that
//! *exactly*: [`super::interaction::MaterialHaptics::fire`] is called with
//! [`super::interaction::HapticSignal::None`] on every confirmed toggle,
//! same call site the reference's `_fireHaptic` occupies (immediately before
//! the tap's own callback), producing no audible/tactile feedback today —
//! the hook is live for a future revision that wants to override it, exactly
//! as `M3ETappable.haptic` is for the reference.
//!
//! # `enabled`, not a nullable callback
//!
//! The reference represents "disabled" as `onChanged == null`
//! (`m3e_switch_control.dart:63`'s `_enabled` getter). This crate's
//! controlled-component contract always takes a concrete `on_toggle`
//! closure, so [`SwitchView::enabled`] is an explicit flag instead — same
//! effect (gates interaction and switches the disabled color table), a
//! different shape to fit this framework's callback contract.
//!
//! `focusNode`/`autofocus`/`semanticLabel` are Dart-only parameters
//! (`m3e_switch_control.dart:16-26`) with no port here — none of the springs/
//! icons/colors/haptics rework above touches focus routing or semantics
//! labeling, so this stays out of scope for this pass.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{Action, Role, Toggled};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use frust::{AnimationController, IconData, IconSource, SpringDesc, Theme, Tween};
use kurbo::{Affine, BezPath, Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

use super::interaction::{HapticSignal, MaterialHaptics};
use super::press::presses;
use super::state_layer::StateLayer;

/// Tessellation tolerance for the border/icon `BezPath`s (same value as
/// [`super::split_button`]/[`super::button_group`]/[`super::sheet`]'s own
/// `PATH_TOLERANCE`).
const PATH_TOLERANCE: f64 = 0.1;

/// Track width, in logical px (`m3e_switch_theme.dart:10`'s `trackWidth`
/// default). No `Theme` size token exists for a fixed control dimension like
/// this (`ShapeScale` publishes corner radii, not track/thumb sizes) —
/// hoisted as a named constant rather than left as a bare literal.
const TRACK_W: f64 = 52.0;
/// Track height, in logical px (`m3e_switch_theme.dart:11`'s `trackHeight`).
/// See [`TRACK_W`]'s doc comment — no suitable `Theme` token exists.
const TRACK_H: f64 = 32.0;
/// Uniform inset the thumb travels within, in logical px
/// (`m3e_switch_theme.dart:12`'s `trackPadding`).
const TRACK_PADDING: f64 = 4.0;
/// Thumb diameter while pressed (either state), in logical px
/// (`m3e_switch_theme.dart:13`'s `thumbSizePressed`) — overrides the
/// unselected/selected interpolation while a press is in progress, instantly
/// (not spring-driven; see the [module docs](self)).
const THUMB_PRESSED: f64 = 32.0;
/// Thumb diameter while selected, in logical px
/// (`m3e_switch_theme.dart:14`'s `thumbSizeSelected`).
const THUMB_SELECTED: f64 = 24.0;
/// Thumb diameter while unselected, in logical px
/// (`m3e_switch_theme.dart:15`'s `thumbSizeUnselected`).
const THUMB_UNSELECTED: f64 = 16.0;
/// Default diameter of the state-layer overlay painted behind the thumb, in
/// logical px (`m3e_switch_theme.dart:16`'s `stateLayerSize`) —
/// [`SwitchView::state_layer_size`] overrides it.
const STATE_LAYER_SIZE_DEFAULT: f64 = 48.0;
/// Side length of the optional per-state icon's own box, in logical px
/// (`m3e_switch_theme.dart:17`'s `iconSize`), before the size spring's
/// fade/scale is applied (see the [module docs](self)).
const ICON_SIZE: f64 = 16.0;
/// Width of the off-track outline ring, in logical px
/// (`m3e_switch_theme.dart:18`'s `borderWidth`).
const BORDER_WIDTH: f64 = 2.0;
/// Track opacity while disabled (`m3e_switch_theme.dart:19`'s
/// `disabledTrackOpacity`).
const DISABLED_TRACK_OPACITY: f32 = 0.12;
/// Thumb opacity while disabled (`m3e_switch_theme.dart:20`'s
/// `disabledThumbOpacity`).
const DISABLED_THUMB_OPACITY: f32 = 0.38;
/// Outline opacity while disabled (`m3e_switch_theme.dart:21`'s
/// `disabledOutlineOpacity`).
const DISABLED_OUTLINE_OPACITY: f32 = 0.12;

/// Unselected track fill (unthemed fallback; a theme resolves this from
/// `colors.surface_container_highest`).
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
/// Unthemed fallback for `colors.on_surface` (disabled-state base color and
/// the unselected state-layer color) — the same value
/// `frust-widgets`' own `IconWidget` uses as its unthemed `on_surface`
/// default, for the same "pre-theme app" reason.
const FALLBACK_ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed fallback for `colors.on_primary_container` (the selected icon's
/// color). No dedicated container-role fallback exists elsewhere in this
/// module, so this reuses [`THUMB_ON`]'s value — light-on-saturated-primary,
/// the same relationship `on_primary_container` has to `primary_container`
/// in a real generated scheme.
const FALLBACK_ON_PRIMARY_CONTAINER: Color = THUMB_ON;

/// Position spring: `MaterialSpringMotion.expressiveSpatialDefault`
/// (stiffness `380`, `tmp/motor-1.1.0/lib/src/motion.dart:610-616`) with
/// damping overridden to `0.55` (`m3e_switch_control.dart:66-70`) — visibly
/// overshoots before settling, the spec's "snap".
const POSITION_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 380.0,
    damping_ratio: 0.55,
};
/// Size spring: the same `expressiveSpatialDefault` stiffness with damping
/// overridden to `0.7` (`m3e_switch_control.dart:72-76`) — settles more
/// directly than the position spring, so the thumb's size change trails its
/// slide without as pronounced a bounce.
const SIZE_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 380.0,
    damping_ratio: 0.7,
};

/// A sub-visible release velocity used only to select which target
/// ([`AnimationController::fling`]'s `1.0`/`0.0`) each spring drives toward —
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

/// Return `color` with its alpha channel replaced by `alpha` (the same
/// helper [`super::state_layer`]/[`super::button_group`] each carry their
/// own copy of).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The raw `ColorScheme` roles this module's color table reads, resolved
/// once per paint from the threaded [`Theme`] (or the unthemed fallback
/// constants) — see [`resolve_switch_colors`].
#[derive(Clone, Copy)]
struct SwitchColors {
    on_surface: Color,
    surface_container_highest: Color,
    primary: Color,
    outline: Color,
    on_primary: Color,
    on_primary_container: Color,
}

/// Resolve the six `ColorScheme` roles [`SwitchColors`] carries. Themed:
/// straight from `theme.scheme()`. Unthemed: the module's own fallback
/// constants — see the [module docs](self) for the "MaterialTokens-only
/// resolution" contract this satisfies (no ad hoc `ThemeExtensions` type).
fn resolve_switch_colors(theme: Option<&Theme>) -> SwitchColors {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            SwitchColors {
                on_surface: s.on_surface,
                surface_container_highest: s.surface_container_highest,
                primary: s.primary,
                outline: s.outline,
                on_primary: s.on_primary,
                on_primary_container: s.on_primary_container,
            }
        }
        None => SwitchColors {
            on_surface: FALLBACK_ON_SURFACE,
            surface_container_highest: TRACK_OFF,
            primary: TRACK_ON,
            outline: THUMB_OFF,
            on_primary: THUMB_ON,
            on_primary_container: FALLBACK_ON_PRIMARY_CONTAINER,
        },
    }
}

/// `M3ESwitchTheme.trackColor` (`m3e_switch_theme.dart:76-88`).
fn track_color(c: SwitchColors, enabled: bool, value: bool) -> Color {
    if !enabled {
        with_alpha(
            if value {
                c.on_surface
            } else {
                c.surface_container_highest
            },
            DISABLED_TRACK_OPACITY,
        )
    } else if value {
        c.primary
    } else {
        c.surface_container_highest
    }
}

/// `M3ESwitchTheme.thumbColor` (`m3e_switch_theme.dart:90-101`).
fn thumb_color(c: SwitchColors, enabled: bool, value: bool) -> Color {
    if !enabled {
        with_alpha(c.on_surface, DISABLED_THUMB_OPACITY)
    } else if value {
        c.on_primary
    } else {
        c.outline
    }
}

/// `M3ESwitchTheme.outlineColor` (`m3e_switch_theme.dart:103-113`).
fn outline_color(c: SwitchColors, enabled: bool) -> Color {
    if !enabled {
        with_alpha(c.on_surface, DISABLED_OUTLINE_OPACITY)
    } else {
        c.outline
    }
}

/// `M3ESwitchTheme.iconColor` (`m3e_switch_theme.dart:115-118`) — no
/// `enabled` branch in the source; preserved as-is (see the
/// [module docs](self)).
fn icon_color(c: SwitchColors, value: bool) -> Color {
    if value {
        c.on_primary_container
    } else {
        c.surface_container_highest
    }
}

/// `M3ESwitchTheme.stateLayerColor` (`m3e_switch_theme.dart:120-123`) — no
/// `enabled` branch in the source; preserved as-is (see the
/// [module docs](self)).
fn state_layer_color(c: SwitchColors, value: bool) -> Color {
    if value { c.primary } else { c.on_surface }
}

/// The thumb's top-left corner, in the widget's own local (0-based)
/// coordinate space, for a given (unclamped) position fraction and diameter.
/// See the [module docs](self)' Thumb travel section for provenance.
fn thumb_origin(pos_frac: f64, diam: f64) -> (f64, f64) {
    let max_w = TRACK_W - 2.0 * TRACK_PADDING;
    let max_h = TRACK_H - 2.0 * TRACK_PADDING;
    let bleed = if diam > max_h {
        (diam - max_h) / 2.0
    } else {
        0.0
    };
    let left = TRACK_PADDING - bleed + (max_w - diam + 2.0 * bleed) * pos_frac;
    let top = TRACK_PADDING + (max_h - diam) / 2.0;
    (left, top)
}

/// Whether two optional icon sources name the same geometry (`d`/`design`
/// equality) — lets `rebuild` skip re-resolving an unchanged icon, the same
/// cheap-identity purpose `frust::IconData::same` serves for `IconWidget`.
fn same_icon_source(a: Option<IconSource>, b: Option<IconSource>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x.d == y.d && x.design == y.design,
        (None, None) => true,
        _ => false,
    }
}

/// Resolve an optional [`IconSource`] into its cached `(design-space path,
/// design box)` pair.
fn resolve_icon(source: Option<IconSource>) -> Option<(BezPath, f64)> {
    source.map(|s| IconData::from(s).resolve())
}

/// A view-held, typed toggle callback (erased on build).
type OnToggle<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative M3E switch. See the [module docs](self).
pub struct SwitchView<State: 'static> {
    checked: bool,
    on_toggle: OnToggle<State>,
    enabled: bool,
    selected_icon: Option<IconSource>,
    unselected_icon: Option<IconSource>,
    state_layer_size: Option<f64>,
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
        enabled: true,
        selected_icon: None,
        unselected_icon: None,
        state_layer_size: None,
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

impl<State: 'static> SwitchView<State> {
    /// Gate interaction and switch to the disabled color table
    /// (`m3e_switch_theme.dart:76-113`'s `enabled` branch). See the
    /// [module docs](self) for why this is an explicit flag rather than the
    /// reference's nullable `onChanged`.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Icon painted inside the thumb while `checked` is true
    /// (`m3e_switch_control.dart:36`'s `selectedIcon`).
    pub fn selected_icon(mut self, icon: IconSource) -> Self {
        self.selected_icon = Some(icon);
        self
    }

    /// Icon painted inside the thumb while `checked` is false
    /// (`m3e_switch_control.dart:38`'s `unselectedIcon`).
    pub fn unselected_icon(mut self, icon: IconSource) -> Self {
        self.unselected_icon = Some(icon);
        self
    }

    /// Diameter of the thumb-centered state-layer overlay, overriding
    /// [`STATE_LAYER_SIZE_DEFAULT`] (`m3e_switch_control.dart:41-44`'s
    /// `stateLayerSize`).
    pub fn state_layer_size(mut self, size: f64) -> Self {
        self.state_layer_size = Some(size);
        self
    }
}

/// The retained widget for a [`SwitchView`].
pub struct SwitchWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    checked: bool,
    enabled: bool,
    /// Drives the thumb's on-axis `0.0` (off) .. `1.0` (on) travel fraction.
    position_anim: AnimationController,
    /// Drives the thumb's unselected↔selected diameter blend fraction.
    size_anim: AnimationController,
    /// The `checked` value both animations are currently driving toward (or
    /// have already settled at) — compared against `checked` at paint time
    /// to decide whether a fresh pair of flings needs to start (see the
    /// [module docs](self)).
    anim_target: bool,
    state_layer: StateLayer,
    /// The pressed *visual* state; follows the cursor in/out while captured,
    /// and also instantly overrides the thumb diameter to
    /// [`THUMB_PRESSED`].
    pressed: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    captured: bool,
    selected_icon: Option<IconSource>,
    unselected_icon: Option<IconSource>,
    selected_icon_geom: Option<(BezPath, f64)>,
    unselected_icon_geom: Option<(BezPath, f64)>,
    state_layer_size: Option<f64>,
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
        let mut position_anim = AnimationController::new(Duration::ZERO);
        let mut size_anim = AnimationController::new(Duration::ZERO);
        if self.checked {
            position_anim.forward();
            size_anim.forward();
        }
        SwitchWidget {
            checked: self.checked,
            enabled: self.enabled,
            position_anim,
            size_anim,
            anim_target: self.checked,
            state_layer: StateLayer::new(),
            pressed: false,
            captured: false,
            selected_icon: self.selected_icon,
            unselected_icon: self.unselected_icon,
            selected_icon_geom: resolve_icon(self.selected_icon),
            unselected_icon_geom: resolve_icon(self.unselected_icon),
            state_layer_size: self.state_layer_size,
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
        let mut flags = ChangeFlags::NONE;

        if prev.checked != self.checked {
            // The app is the source of truth: adopt the new value. Both
            // flings start lazily in `paint`, once a theme is in scope
            // again.
            element.checked = self.checked;
            flags |= ChangeFlags::PAINT;
        }
        if prev.enabled != self.enabled {
            element.enabled = self.enabled;
            flags |= ChangeFlags::PAINT;
        }
        if prev.state_layer_size != self.state_layer_size {
            element.state_layer_size = self.state_layer_size;
            flags |= ChangeFlags::PAINT;
        }
        if !same_icon_source(prev.selected_icon, self.selected_icon) {
            element.selected_icon = self.selected_icon;
            element.selected_icon_geom = resolve_icon(self.selected_icon);
            flags |= ChangeFlags::PAINT;
        }
        if !same_icon_source(prev.unselected_icon, self.unselected_icon) {
            element.unselected_icon = self.unselected_icon;
            element.unselected_icon_geom = resolve_icon(self.unselected_icon);
            flags |= ChangeFlags::PAINT;
        }

        flags
    }
}

impl Widget for SwitchWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(TRACK_W, TRACK_H))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_switch_colors(theme);

        if self.checked != self.anim_target {
            let velocity = if self.checked {
                RELEASE_VELOCITY
            } else {
                -RELEASE_VELOCITY
            };
            self.position_anim.fling(velocity, POSITION_SPRING);
            self.size_anim.fling(velocity, SIZE_SPRING);
            self.anim_target = self.checked;
        }
        let position_animating = self.position_anim.advance(ctx.frame_time());
        let size_animating = self.size_anim.advance(ctx.frame_time());
        if position_animating || size_animating {
            ctx.request_frame();
        }

        // Unclamped for spatial travel/size: a spring's overshoot past the
        // target is real, intended motion (see AnimationController's
        // Overshoot docs, and the module docs' Thumb travel section).
        let pos_frac = self.position_anim.value();
        let pos_frac_clamped = self.position_anim.value_clamped();
        let size_frac = self.size_anim.value();
        let size_frac_clamped = self.size_anim.value_clamped().clamp(0.0, 1.0);

        let o = ctx.origin();

        // Track fill: continuous cross-fade via the position spring's
        // clamped fraction (a documented porting decision — see the module
        // docs).
        let track_off = track_color(colors, self.enabled, false);
        let track_on = track_color(colors, self.enabled, true);
        let track_fill = lerp_color_exact(track_off, track_on, pos_frac_clamped);
        scene.fill_rounded_rect(o, Size::new(TRACK_W, TRACK_H), TRACK_H / 2.0, track_fill);

        // Off-track outline ring (m3e_switch_control.dart:140-148: only
        // while `!checked`).
        if !self.checked {
            let outline = outline_color(colors, self.enabled);
            let inset = BORDER_WIDTH / 2.0;
            let border_rect = Rect::new(inset, inset, TRACK_W - inset, TRACK_H - inset);
            let border_radius = TRACK_H / 2.0 - inset;
            let border_path =
                RoundedRect::from_rect(border_rect, border_radius).to_path(PATH_TOLERANCE);
            scene.stroke_path(o, &border_path, BORDER_WIDTH, &Brush::Solid(outline));
        }

        let base_diam = THUMB_UNSELECTED + size_frac * (THUMB_SELECTED - THUMB_UNSELECTED);
        let diam = if self.pressed {
            THUMB_PRESSED
        } else {
            base_diam
        };
        let (left, top) = thumb_origin(pos_frac, diam);

        let state_layer_size = self.state_layer_size.unwrap_or(STATE_LAYER_SIZE_DEFAULT);
        let center = Point::new(o.x + left + diam / 2.0, o.y + top + diam / 2.0);
        self.state_layer.paint(
            ctx,
            scene,
            Rect::from_center_size(center, Size::new(state_layer_size, state_layer_size)),
            state_layer_size / 2.0,
            state_layer_color(colors, self.checked),
        );

        let thumb_off = thumb_color(colors, self.enabled, false);
        let thumb_on = thumb_color(colors, self.enabled, true);
        let thumb_fill = lerp_color_exact(thumb_off, thumb_on, pos_frac_clamped);
        scene.fill_rounded_rect(
            Point::new(o.x + left, o.y + top),
            Size::new(diam, diam),
            diam / 2.0,
            thumb_fill,
        );

        // Icon: value-selected, size-faded (see the module docs).
        let icon_geom = if self.checked {
            &self.selected_icon_geom
        } else {
            &self.unselected_icon_geom
        };
        if let Some((base_path, design)) = icon_geom {
            let opacity = size_frac_clamped as f32;
            if opacity > 0.0 {
                let icon_render_scale = 0.5 + 0.5 * size_frac_clamped;
                let effective = ICON_SIZE * icon_render_scale;
                let path_scale = if *design > 0.0 {
                    effective / design
                } else {
                    1.0
                };
                let icon_local = Point::new(
                    left + diam / 2.0 - effective / 2.0,
                    top + diam / 2.0 - effective / 2.0,
                );
                let transform = Affine::translate(Vec2::new(icon_local.x, icon_local.y))
                    * Affine::scale(path_scale);
                let scaled_path = transform * base_path.clone();
                let icon_fill = with_alpha(icon_color(colors, self.checked), opacity);
                scene.fill_path(o, &scaled_path, &Brush::Solid(icon_fill));
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !self.enabled || !presses(p) {
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
                    // Same call order as the reference's `_wrapTap`: haptic
                    // first, then the tap's own effect. Fires `None` — see
                    // the module docs' Haptics section.
                    MaterialHaptics::fire(HapticSignal::None);
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
            if self.enabled {
                node.add_action(Action::Click);
            }
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
    fn disabled_ignores_pointer_and_never_toggles() {
        let view = switch::<ToggleState, _>(false, |s: &mut ToggleState, v: bool| {
            s.last = Some(v);
            s.toggles += 1;
        })
        .enabled(false);
        let mut counter = 0u64;
        let mut w = View::<ToggleState>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        assert!(!w.pressed, "disabled never presses");
        assert!(!w.captured, "disabled never captures");
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.toggles, 0, "disabled never toggles");
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
            "both flings only start lazily in paint, once a theme is in scope"
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
        // Track first, thumb last (no active state layer): 2 rects. The
        // off-track outline border and any icon paint through
        // fill_path/stroke_path, not fill_rounded_rect, so they never touch
        // this recorder.
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
    fn paint_starts_both_flings_when_checked_disagrees_with_anim_target() {
        let mut w = widget(false);
        w.checked = true; // simulate the confirmed value rebuild adopts
        assert!(!w.position_anim.is_animating());
        assert!(!w.size_anim.is_animating());
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H));
        let mut rec = RRectRecorder::default();
        w.paint(&mut ctx, &mut rec);
        assert!(w.anim_target, "paint syncs the animation target");
        assert!(
            w.position_anim.is_animating(),
            "a position fling toward the new target started"
        );
        assert!(
            w.size_anim.is_animating(),
            "a size fling toward the new target started"
        );
        assert!(
            ctx.needs_frame(),
            "an in-flight fling must request another frame"
        );
    }

    #[test]
    fn both_springs_progress_toward_and_settle_at_target() {
        let mut w = widget(false);
        w.checked = true;
        // First paint starts both flings (module docs: lazily, once themed).
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H));
        let mut rec = RRectRecorder::default();
        w.paint(&mut ctx, &mut rec);

        // Directly advance both controllers with injected frame times
        // (mirrors frust-core::anim's own test pattern) rather than through
        // PaintCtx, whose frame_time setter is crate-private.
        let mut position_running = true;
        let mut size_running = true;
        let mut t = 0.0;
        for _ in 0..100_000 {
            position_running = w.position_anim.advance(ft_secs(t));
            size_running = w.size_anim.advance(ft_secs(t));
            if !position_running && !size_running {
                break;
            }
            t += 1.0 / 120.0;
        }
        assert!(!position_running, "position fling failed to settle");
        assert!(!size_running, "size fling failed to settle");
        assert!((w.position_anim.value() - 1.0).abs() < 1e-6);
        assert!((w.size_anim.value() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn spring_specs_match_the_m3e_reference_table() {
        // (spring, expected stiffness, expected damping ratio) — mirrors
        // m3e_switch_control.dart:66-76's `.copyWith(damping: ...)`
        // overrides atop MaterialSpringMotion.expressiveSpatialDefault's
        // stiffness (tmp/motor-1.1.0/lib/src/motion.dart:610-616: 380).
        let table = [
            ("position", POSITION_SPRING, 380.0_f64, 0.55_f64),
            ("size", SIZE_SPRING, 380.0_f64, 0.7_f64),
        ];
        for (name, spring, stiffness, damping_ratio) in table {
            assert_eq!(spring.mass, 1.0, "{name} spring mass");
            assert_eq!(spring.stiffness, stiffness, "{name} spring stiffness");
            assert_eq!(
                spring.damping_ratio, damping_ratio,
                "{name} spring damping ratio"
            );
        }
    }

    #[test]
    fn thumb_rests_at_spec_exact_positions_off_and_on() {
        // Off at rest: pos_frac=0, diam=16 (unselected) — no bleed
        // (16 <= max_h=24). left = 4, top = 4 + (24-16)/2 = 8.
        let mut off = widget(false);
        let rec = paint_rec(&mut off, None);
        assert_eq!(rec.rrects[1].0, Point::new(4.0, 8.0));
        assert_eq!(rec.rrects[1].1, Size::new(16.0, 16.0));

        // On at rest: pos_frac=1, diam=24 (selected) — no bleed
        // (24 <= max_h=24). left = 4 + (44-24) = 24, top = 4.
        let mut on = widget(true);
        let rec = paint_rec(&mut on, None);
        assert_eq!(rec.rrects[1].0, Point::new(24.0, 4.0));
        assert_eq!(rec.rrects[1].1, Size::new(24.0, 24.0));
    }

    #[test]
    fn thumb_travel_is_monotone_toward_target_mid_flight() {
        let mut w = widget(false);
        w.checked = true;
        // First paint starts both flings and seeds the clock (module docs:
        // lazily, once themed) — mirrors
        // `both_springs_progress_toward_and_settle_at_target`'s pattern
        // rather than `PaintCtx::for_test` (a `frust-core` `test-support`-
        // feature-gated seam this crate's dev-dependency doesn't enable).
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H));
        let mut rec = RRectRecorder::default();
        w.paint(&mut ctx, &mut rec);

        // Sample well inside the position spring's monotonic rise (its
        // damped period is ~0.39s at stiffness 380/damping 0.55, so the
        // first ~50ms stays short of any overshoot peak).
        let mut xs = Vec::new();
        let mut t = 0.0;
        for _ in 0..6 {
            t += 1.0 / 120.0;
            w.position_anim.advance(ft_secs(t));
            w.size_anim.advance(ft_secs(t));
            let diam = THUMB_UNSELECTED + w.size_anim.value() * (THUMB_SELECTED - THUMB_UNSELECTED);
            let (left, _top) = thumb_origin(w.position_anim.value(), diam);
            xs.push(left);
        }
        for pair in xs.windows(2) {
            assert!(
                pair[1] > pair[0],
                "thumb x-origin must move monotonically toward the target mid-flight: {xs:?}"
            );
        }
    }

    #[test]
    fn icon_paints_selected_variant_at_full_opacity_when_on_at_rest() {
        let view = switch::<ToggleState, _>(true, |_s, _v| {})
            .selected_icon(crate::icons::CHECK)
            .unselected_icon(crate::icons::CLOSE);
        let mut counter = 0u64;
        let mut w = View::<ToggleState>::build(&view, &mut BuildCtx::new(&mut counter));

        #[derive(Default)]
        struct IconRecorder {
            fills: Vec<(BezPath, Color)>,
        }
        impl PaintScene for IconRecorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn fill_path(&mut self, _o: Point, path: &BezPath, brush: &Brush) {
                let Brush::Solid(color) = brush else {
                    panic!("expected a solid brush");
                };
                self.fills.push((path.clone(), *color));
            }
        }

        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H));
        let mut rec = IconRecorder::default();
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.fills.len(), 1, "exactly one icon paints");
        assert_eq!(
            rec.fills[0].1.components[3], 1.0,
            "fully opaque at rest (size fraction 1.0)"
        );
        // Selected (CHECK), not unselected (CLOSE): bounding boxes of the
        // two source icons differ, so this pins which one was chosen.
        let (unselected_path, unselected_design) = IconData::from(crate::icons::CLOSE).resolve();
        let unselected_scale = ICON_SIZE / unselected_design;
        let unselected_bounds = (Affine::scale(unselected_scale) * unselected_path).bounding_box();
        assert_ne!(
            rec.fills[0].0.bounding_box().width(),
            unselected_bounds.width(),
            "must not paint the unselected icon while checked"
        );
    }

    #[test]
    fn icon_selects_unselected_variant_from_confirmed_value_even_mid_size_flight() {
        // Force the size spring into a nonzero, non-rest state while
        // `checked` stays false, mirroring the reference's own decoupling
        // (icon choice follows the confirmed value, not the size spring) —
        // see the module docs' Icons section.
        let view = switch::<ToggleState, _>(false, |_s, _v| {})
            .selected_icon(crate::icons::CHECK)
            .unselected_icon(crate::icons::CLOSE);
        let mut counter = 0u64;
        let mut w = View::<ToggleState>::build(&view, &mut BuildCtx::new(&mut counter));
        w.size_anim.forward(); // value -> 1.0, but `checked`/`anim_target` stay false

        #[derive(Default)]
        struct IconRecorder {
            count: u32,
        }
        impl PaintScene for IconRecorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn fill_path(&mut self, _o: Point, _p: &BezPath, _b: &Brush) {
                self.count += 1;
            }
        }
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H));
        let mut rec = IconRecorder::default();
        w.paint(&mut ctx, &mut rec);
        assert_eq!(
            rec.count, 1,
            "the unselected icon still paints (opacity 1.0)"
        );
    }

    #[test]
    fn no_icon_configured_paints_nothing() {
        let mut w = widget(true);
        #[derive(Default)]
        struct IconRecorder {
            count: u32,
        }
        impl PaintScene for IconRecorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn fill_path(&mut self, _o: Point, _p: &BezPath, _b: &Brush) {
                self.count += 1;
            }
        }
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H));
        let mut rec = IconRecorder::default();
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.count, 0);
    }

    #[test]
    fn off_track_paints_an_outline_border_on_paints_none() {
        #[derive(Default)]
        struct StrokeRecorder {
            strokes: u32,
        }
        impl PaintScene for StrokeRecorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
                self.strokes += 1;
            }
        }
        let mut off = widget(false);
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H));
        let mut rec = StrokeRecorder::default();
        off.paint(&mut ctx, &mut rec);
        assert_eq!(rec.strokes, 1, "off track paints its outline ring");

        let mut on = widget(true);
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(TRACK_W, TRACK_H));
        let mut rec = StrokeRecorder::default();
        on.paint(&mut ctx, &mut rec);
        assert_eq!(rec.strokes, 0, "on track paints no outline ring");
    }

    #[test]
    fn custom_state_layer_size_flows_into_paint() {
        let view = switch::<ToggleState, _>(false, |_s, _v| {}).state_layer_size(64.0);
        let mut counter = 0u64;
        let mut w = View::<ToggleState>::build(&view, &mut BuildCtx::new(&mut counter));
        w.pressed = true;
        w.state_layer.set_pressed(true);
        let rec = paint_rec(&mut w, None);
        assert_eq!(rec.rrects.len(), 3, "track, state layer, thumb");
        assert_eq!(rec.rrects[1].1, Size::new(64.0, 64.0));
        assert_eq!(rec.rrects[1].2, 32.0, "radius is half the custom diameter");
    }

    #[test]
    fn color_table_matches_the_m3e_theme_for_every_enabled_value_combo() {
        let theme = crate::baseline();
        let scheme = theme.scheme();
        let colors = resolve_switch_colors(Some(&theme));

        for enabled in [true, false] {
            for value in [true, false] {
                let track = track_color(colors, enabled, value);
                let thumb = thumb_color(colors, enabled, value);
                let expected_track = if !enabled {
                    with_alpha(
                        if value {
                            scheme.on_surface
                        } else {
                            scheme.surface_container_highest
                        },
                        DISABLED_TRACK_OPACITY,
                    )
                } else if value {
                    scheme.primary
                } else {
                    scheme.surface_container_highest
                };
                let expected_thumb = if !enabled {
                    with_alpha(scheme.on_surface, DISABLED_THUMB_OPACITY)
                } else if value {
                    scheme.on_primary
                } else {
                    scheme.outline
                };
                assert_eq!(track, expected_track, "enabled={enabled} value={value}");
                assert_eq!(thumb, expected_thumb, "enabled={enabled} value={value}");
            }
            let outline = outline_color(colors, enabled);
            let expected_outline = if !enabled {
                with_alpha(scheme.on_surface, DISABLED_OUTLINE_OPACITY)
            } else {
                scheme.outline
            };
            assert_eq!(outline, expected_outline, "enabled={enabled}");
        }

        // icon_color/state_layer_color: no enabled branch (see module docs).
        assert_eq!(icon_color(colors, true), scheme.on_primary_container);
        assert_eq!(icon_color(colors, false), scheme.surface_container_highest);
        assert_eq!(state_layer_color(colors, true), scheme.primary);
        assert_eq!(state_layer_color(colors, false), scheme.on_surface);
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

    #[test]
    fn semantics_omits_click_action_when_disabled() {
        fn logic(_state: &mut ToggleState) -> SwitchView<ToggleState> {
            switch::<ToggleState, _>(true, |_s, _v| {}).enabled(false)
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
        assert!(!node.supports_action(Action::Click));
    }
}
