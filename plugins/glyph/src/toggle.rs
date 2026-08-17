//! [`toggle`]/[`ToggleView`]: the Glyph switch — a 38×22 pill track whose 16px
//! knob springs 16px across on a state change (the Glyph design system's
//! `.toggle` rules and the motion reference's own "toggle spring" section,
//! retrieved 2026-07-21).
//!
//! # Why the catalog carries a switch at all
//!
//! The baseline widget set ships no `Switch` (`frust::Checkbox` is its only
//! boolean control), and Glyph authors this one as its own design rather than a
//! re-tint of the M3 switch: a hairline-bordered pill, a small
//! constant-diameter knob, no state-layer overlay, an accent *wash* instead of
//! a filled track. The public shape still mirrors `frust_material::switch`
//! (`toggle(checked, on_toggle)`), so an app migrates between catalogs
//! mechanically.
//!
//! # Controlled component (never self-mutating)
//!
//! Like [`crate::segmented_control`]/[`crate::tabs`], it reports the
//! *requested* value through `on_toggle(state, !checked)` on a release inside
//! its bounds and never flips its own `checked`; the app's next `rebuild` feeds
//! the confirmed value back in, which is what starts the animation.
//!
//! # A touch slab around a fixed pill
//!
//! `layout` requests a [`TOUCH_TARGET`]-square slab and `paint` centers the
//! fixed pill inside whatever box it was given, so the hit-tested area (the
//! whole slab) and the painted control never diverge — and the a11y target
//! clears WCAG 2.2 and iOS's minimums a bare 38×22 misses (4dp under
//! Android's; see [`TOUCH_TARGET`]'s doc for why that shortfall is accepted).
//!
//! # Two motion tracks: the knob springs, the colors fade
//!
//! The motion reference states the split outright — "the knob is a spatial
//! change (position) — it gets the spring. The track fill is a color change —
//! it fades, no bounce" (`glyph-motion.html`, section 02) — so the widget runs
//! two independent progress lanes ([`Progress`]):
//!
//! - **knob position**: 220ms on the overshooting spatial bezier
//!   ([`KNOB_DURATION`]/[`KNOB_CURVE`]), read **unclamped** — the knob sliding
//!   briefly past its rest spot is the source's intended travel.
//! - **track/border/knob color**: 150ms on the non-overshooting effects bezier
//!   ([`FADE_DURATION`]/[`FADE_CURVE`]), read clamped — a color never
//!   extrapolates past its endpoint.
//!
//! Both pairs stay hardcoded on the
//! [`crate::tabs`]/[`crate::segmented_control`] convention: a Glyph
//! transition's timing is part of the design's identity,
//! not theme-tunable decoration. Nothing technical forces that — a controller's
//! duration and curve resolve fine at paint time, as [`crate::accordion`] and
//! `frust_material::switch`'s spring both do — it is a fidelity choice. The
//! values are the exact `MotionScheme::glyph()` `durations.base`/
//! `easing.spatial` and `durations.fast`/`easing.effects` ([`crate::tokens`]),
//! so a themed app animates identically.
//!
//! Both lanes advance during `paint` and re-request a frame while in flight
//! (`docs/WIDGETS_CODE_STANDARDS.md`'s Theming & Animation Conventions); under
//! `theme.motion.reduce_motion` they snap to their targets and request nothing,
//! so a state change paints its final frame at once. The pill never resizes, so
//! this is a paint-only animation: bare `request_frame`, never
//! `request_layout`.
//!
//! # Per-brightness paint (the source's dark/light split)
//!
//! Only the *off* knob and the lift shadow differ between brightnesses, so the
//! resolve branches on `theme.brightness` (the [`crate::segmented_control`]
//! precedent): dark draws the off knob from `--fg-dim` with no shadow; light
//! draws the white `--bg-surface` slot over a `0 1px 2px` ink lift, because a
//! white knob on a warm-paper track needs the lift to read as an object at all
//! (painted inline rather than as an `Elevation` level — again
//! [`crate::segmented_control`]'s call for its micro-shadow).
//!
//! Everything else is one role per value: the `bg-overlay` track slot off and an
//! accent wash on, the `fg`-derived hairline border off and an accent wash on,
//! and the bright-fill accent role (`primary_container`, `#ffb627` in both
//! brightnesses) for the *on* knob — never the accent *text* role
//! (`docs/WIDGETS_CODE_STANDARDS.md`'s Glyph accent-role split).
//!
//! ## The `--fg-dim` role gap, and what it costs
//!
//! `--fg-dim` has no `ColorScheme` role at all ([`crate::tokens::color`] maps
//! `fg-muted` to `on_surface_variant` and leaves `fg-dim` unmapped), so the
//! themed dark off-knob follows the catalog convention
//! `docs/WIDGETS_CODE_STANDARDS.md` records — `--fg-dim` as a *fill* resolves to
//! `on_surface_variant` — and paints `#a39c88` where the source authors
//! `#6b6556`: ~2.5× the authored relative luminance, reading 4.9:1 against the
//! `#272d3d` track instead of 2.4:1. That is the deliberate price of one
//! catalog-wide convention over a per-widget token; both hexes are pinned by
//! test (themed and unthemed) so it cannot drift unnoticed. The unthemed
//! fallback keeps the exact source hex ([`TOGGLE_KNOB_OFF`]).
//!
//! # No disabled, focus, or pressed states in v1
//!
//! The sources author none — no `:disabled`, `:focus-visible`, or `:active` rule
//! in either brightness, and no state-layer overlay — so a press is armed state
//! only, painting nothing. A future disabled state should follow the baseline
//! `Button`'s 0.38 disabled-opacity convention rather than invent one.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{Action, Role, Toggled};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use frust::{AnimationController, Brightness, Curve, FrameTime, ShapeScale, Theme, Tween};
use kurbo::{Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

/// Track width, logical px (`.toggle{width:38px}` —
/// `glyph-design-system.html:255`, `glyph-design-system-light.html:152`,
/// `glyph-motion.html:59`, identical in all three). No `Theme` token carries a
/// fixed control dimension: `ShapeScale` publishes corner radii, not
/// track/knob sizes.
const TRACK_W: f64 = 38.0;
/// Track height, logical px (`.toggle{height:22px}`, same three sources). See
/// [`TRACK_W`] for why this is a constant rather than a token.
const TRACK_H: f64 = 22.0;
/// Track border thickness, logical px (`.toggle{border:1px solid …}`,
/// `glyph-design-system.html:255`). Stroked on a path inset by half this width
/// so the *painted* box measures exactly [`TRACK_W`]×[`TRACK_H`] — all three
/// sources set `box-sizing:border-box`, which is also the premise
/// [`KNOB_OFFSET`]'s math already assumes.
const BORDER_W: f64 = 1.0;
/// Interactive slab edge, logical px: `layout` requests
/// `max(TRACK_W, TOUCH_TARGET) × max(TRACK_H, TOUCH_TARGET)` and centers the
/// pill in it, because a 38×22 control clears no platform minimum (WCAG 2.2
/// target-size 24px, iOS 44pt, Android 48dp). 44 clears WCAG 2.2 and iOS
/// outright but sits 4dp under Android's 48dp — accepted for consistency with
/// [`crate::sheet`]'s `HANDLE_TOUCH_TARGET`, already the same 44 value its 3px
/// drag handle wraps in — one slab size for the catalog's undersized chrome.
const TOUCH_TARGET: f64 = 44.0;
/// Knob diameter, logical px (`.toggle::after{width:16px;height:16px}`,
/// `glyph-design-system.html:256`). Constant in both states — unlike the M3
/// switch, Glyph's knob never resizes.
const KNOB_SIZE: f64 = 16.0;
/// Knob inset from the track's *inner* (border-box) edge, logical px
/// (`.toggle::after{top:2px;left:2px}`, `glyph-design-system.html:256`). The
/// source's `::after` is positioned against the padding box, i.e. inside the
/// 1px border — [`KNOB_OFFSET`] is that offset measured from the pill's origin.
const KNOB_INSET: f64 = 2.0;
/// The off-state knob offset from the *pill's* origin on both axes, logical px:
/// the border plus the source's 2px inset. Vertically this also centers the
/// knob exactly (`(22 − 16) / 2 == 3`).
const KNOB_OFFSET: f64 = BORDER_W + KNOB_INSET;
/// Knob travel, logical px (`.toggle:checked::after{transform:translateX(16px)}`
/// — `glyph-design-system.html:258`, `glyph-design-system-light.html:155`,
/// `glyph-motion.html:62`). Equivalently `TRACK_W − TRACK_H`: the knob's two
/// rest centers sit exactly on the pill's two corner centers (x = 11 and 27).
const KNOB_TRAVEL: f64 = 16.0;
/// Corner-rounding tolerance for the hairline border path (the crate's shared
/// `RoundedRect::to_path` value — see [`crate::badge`]'s `PATH_TOLERANCE`).
const PATH_TOLERANCE: f64 = 0.1;

// ---- Motion (hardcoded named constants; see the module docs) ---------------

/// Knob travel duration — `--dur-base` (220ms, `glyph-motion.html:20`), the
/// duration the "toggle spring" section pins for the knob
/// (`glyph-motion.html:60`, `:166`–`:173`); the exact `MotionScheme::glyph()`
/// `durations.base`.
///
/// **The sources disagree, and 220ms is the deliberate resolution.** Both
/// design-system sheets author the knob as
/// `transition: transform .18s var(--ease-spring)`
/// (`glyph-design-system.html:256`, `glyph-design-system-light.html:153`) —
/// 180ms — while the motion reference, which is *the* motion authority for this
/// catalog, pins `--dur-base` 220ms on `--ease-spatial`
/// (`glyph-motion.html:20`–`:21`, `:59`–`:62`) and repeats it in its toggle
/// section. The authority wins: this is not a transcription slip to "fix" back
/// to 180ms.
const KNOB_DURATION: Duration = Duration::from_millis(220);
/// Knob travel easing — `--ease-spatial`/`--ease-spring`,
/// `cubic-bezier(0.34,1.35,0.64,1)` (`glyph-motion.html:21`,
/// `glyph-design-system.html:53`); the exact `MotionScheme::glyph()`
/// `easing.spatial`. Overshoots past `1.0` on purpose — that is the spring.
const KNOB_CURVE: Curve = Curve::Cubic(0.34, 1.35, 0.64, 1.0);
/// Color-fade duration — `--dur-fast` (150ms, `glyph-motion.html:20`), the
/// `background` transition on `.toggle`/`.toggle::after`
/// (`glyph-motion.html:59`–`:60`); the exact `MotionScheme::glyph()`
/// `durations.fast`.
const FADE_DURATION: Duration = Duration::from_millis(150);
/// Color-fade easing — `--ease-effects`/`--ease-out`,
/// `cubic-bezier(0.16,1,0.3,1)` (`glyph-motion.html:22`,
/// `glyph-design-system.html:52`); the exact `MotionScheme::glyph()`
/// `easing.effects`. Never overshoots, so no color extrapolates.
const FADE_CURVE: Curve = Curve::Cubic(0.16, 1.0, 0.3, 1.0);

// ---- Source alpha tokens ---------------------------------------------------

/// Off-state border alpha — `--border-bright` is the foreground ink at 18%
/// (`rgba(242,234,217,0.18)` dark `glyph-design-system.html:41`,
/// `rgba(34,29,18,0.18)` light `glyph-design-system-light.html:45`): one alpha,
/// both brightnesses, over the `on_surface` role each carries.
///
/// Painted as a true alpha stroke rather than the pre-flattened `outline` role
/// ([`crate::tokens::color`]'s "Alpha pre-flattening"): that flattening assumes
/// a `bg-surface` backdrop, and a toggle sits on whatever surface its row does.
const BORDER_OFF_ALPHA: f32 = 0.18;
/// Checked track alpha, dark — `--amber-faint` = `rgba(255,182,39,0.12)`
/// (`glyph-design-system.html:21`), i.e. the accent at 12%.
const TRACK_ON_ALPHA_DARK: f32 = 0.12;
/// Checked track alpha, light — `--amber-faint` = `rgba(163,101,10,0.10)`
/// (`glyph-design-system-light.html:24`): the light accent-text tone at 10%,
/// re-tuned rather than flipped.
const TRACK_ON_ALPHA_LIGHT: f32 = 0.10;
/// Checked border alpha, dark — `--amber-border` = `rgba(255,182,39,0.35)`
/// (`glyph-design-system.html:23`), applied by
/// `.toggle:checked{border-color:var(--amber-border)}` (`:257`).
const BORDER_ON_ALPHA_DARK: f32 = 0.35;
/// Checked border alpha, light — `--amber-border` = `rgba(163,101,10,0.38)`
/// (`glyph-design-system-light.html:26`, applied at `:154`).
const BORDER_ON_ALPHA_LIGHT: f32 = 0.38;
/// Light-only knob lift shadow: `box-shadow:0 1px 2px rgba(0,0,0,0.2)`
/// (`glyph-design-system-light.html:153`) — alpha 0.2 over the theme's
/// `shadow` role, which on the Glyph light baseline is warm ink rather than
/// black ("light elevation reads as warm-ink lift", [`crate::tokens::color`]).
const KNOB_SHADOW_ALPHA: f32 = 0.2;
/// The lift shadow's blur, logical px: the CSS blur radius stored directly as a
/// `blur_std_dev`, matching `frust_theme::elevation`'s v1 treatment and
/// [`crate::segmented_control`]'s identical micro-shadow.
const KNOB_SHADOW_BLUR: f64 = 2.0;
/// The lift shadow's vertical offset, logical px (the `1px` in `0 1px 2px`).
const KNOB_SHADOW_DY: f64 = 1.0;

// ---- Unthemed fallback constants (Glyph **dark** values) -------------------

/// Off-state track fill — `--bg-overlay` (`glyph-design-system.html:16`),
/// themed `surface_container_highest`.
const TOGGLE_TRACK_OFF: Color = Color::from_rgb8(0x27, 0x2d, 0x3d);
/// Off-state knob fill — `--fg-dim` (`glyph-design-system.html:37`). Exact
/// here; the themed path resolves `on_surface_variant` instead and lands
/// ~2.5× brighter (see the module docs' `--fg-dim` role gap).
const TOGGLE_KNOB_OFF: Color = Color::from_rgb8(0x6b, 0x65, 0x56);
/// The accent — `--amber` (`glyph-design-system.html:19`), themed
/// `primary_container` for the knob fill and `primary` for the washes.
const TOGGLE_ACCENT: Color = Color::from_rgb8(0xff, 0xb6, 0x27);
/// The ink the 18% `--border-bright` hairline is derived from — `--fg`
/// (`glyph-design-system.html:35`), themed `on_surface`.
const TOGGLE_BORDER_INK: Color = Color::from_rgb8(0xf2, 0xea, 0xd9);

/// Return `color` with its alpha channel replaced (mirrors the same helper in
/// the sibling Glyph widgets).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Interpolate `begin`→`end` at `t`, snapping exactly to an endpoint at (or
/// past) `0.0`/`1.0` instead of routing it through [`Tween::lerp`]'s `f32`
/// arithmetic, which can land a few ULPs off `end`. Keeps a resting toggle
/// pixel-identical to its source color, the same guarantee (and shape)
/// `frust_material::switch`'s `lerp_color_exact` keeps.
fn lerp_color(begin: Color, end: Color, t: f64) -> Color {
    if t <= 0.0 {
        begin
    } else if t >= 1.0 {
        end
    } else {
        Tween::new(begin, end).lerp(t)
    }
}

/// One curve-driven `0 → 1` progress lane: a controller plus the endpoints it
/// interpolates between.
///
/// The endpoint pair is what [`crate::tabs`]' indicator and
/// [`crate::accordion`]'s height driver both use, for two reasons a bare
/// controller value can't cover: a freshly-built widget rests at its confirmed
/// state with no entry animation (`from == to`), and a mid-flight reversal
/// starts from the value actually on screen rather than snapping.
struct Progress {
    ctrl: AnimationController,
    duration: Duration,
    curve: Curve,
    from: f64,
    to: f64,
    /// The value last computed by [`Progress::advance`] — what paint reads.
    displayed: f64,
}

impl Progress {
    /// An idle lane resting at `value`.
    fn at_rest(value: f64, duration: Duration, curve: Curve) -> Self {
        Self {
            ctrl: AnimationController::new(duration).with_curve(curve),
            duration,
            curve,
            from: value,
            to: value,
            displayed: value,
        }
    }

    /// Re-aim at `to`, starting from whatever is on screen now.
    fn retarget(&mut self, to: f64) {
        self.from = self.displayed;
        self.to = to;
        self.ctrl = AnimationController::new(self.duration).with_curve(self.curve);
        self.ctrl.forward();
    }

    /// Land on the target immediately, cancelling any flight — the
    /// `reduce_motion` path ([`crate::accordion`]'s `snap` idiom). A fresh
    /// controller is idle, so a following [`Progress::advance`] stays at rest
    /// and asks for nothing.
    fn snap(&mut self) {
        self.from = self.to;
        self.displayed = self.to;
        self.ctrl = AnimationController::new(self.duration).with_curve(self.curve);
    }

    /// Advance to frame time `now`, returning whether the lane is still in
    /// flight (in which case the caller must request another frame).
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

    /// The current value, unclamped — an overshooting curve reads past `1.0`.
    fn value(&self) -> f64 {
        self.displayed
    }

    /// [`Progress::value`] clamped to `0.0..=1.0`, for a consumer (a color
    /// lerp) that must not extrapolate.
    fn value_clamped(&self) -> f64 {
        self.displayed.clamp(0.0, 1.0)
    }
}

/// The resolved toggle palette. `knob_shadow` is `Some` only on the light
/// baseline — see the module docs' per-brightness split.
struct ToggleColors {
    track_off: Color,
    track_on: Color,
    knob_off: Color,
    knob_on: Color,
    border_off: Color,
    border_on: Color,
    knob_shadow: Option<Color>,
}

/// Resolve the palette from the theme, falling back to the literal Glyph
/// **dark** constants with none threaded.
fn resolve_toggle_colors(theme: Option<&Theme>) -> ToggleColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            let (track_on_alpha, border_on_alpha, knob_off, knob_shadow) = match theme.brightness {
                // `on_surface_variant` is `--fg-muted`, not the authored
                // `--fg-dim`: the catalog convention, with the cost measured in
                // the module docs and pinned by test.
                Brightness::Dark => (
                    TRACK_ON_ALPHA_DARK,
                    BORDER_ON_ALPHA_DARK,
                    scheme.on_surface_variant,
                    None,
                ),
                Brightness::Light => (
                    TRACK_ON_ALPHA_LIGHT,
                    BORDER_ON_ALPHA_LIGHT,
                    scheme.surface_container_lowest,
                    Some(with_alpha(scheme.shadow, KNOB_SHADOW_ALPHA)),
                ),
            };
            ToggleColors {
                track_off: scheme.surface_container_highest,
                track_on: with_alpha(scheme.primary, track_on_alpha),
                knob_off,
                knob_on: scheme.primary_container,
                border_off: with_alpha(scheme.on_surface, BORDER_OFF_ALPHA),
                border_on: with_alpha(scheme.primary, border_on_alpha),
                knob_shadow,
            }
        }
        None => ToggleColors {
            track_off: TOGGLE_TRACK_OFF,
            track_on: with_alpha(TOGGLE_ACCENT, TRACK_ON_ALPHA_DARK),
            knob_off: TOGGLE_KNOB_OFF,
            knob_on: TOGGLE_ACCENT,
            border_off: with_alpha(TOGGLE_BORDER_INK, BORDER_OFF_ALPHA),
            border_on: with_alpha(TOGGLE_ACCENT, BORDER_ON_ALPHA_DARK),
            knob_shadow: None,
        },
    }
}

/// Track radius (`--radius-full`) — themed `shape.full` resolved against the
/// box (always a pill), else the literal half-height.
fn track_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.full, size.width, size.height),
        None => size.height / 2.0,
    }
}

/// A view-held, typed toggle callback (erased on build/rebuild).
type OnToggle<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative Glyph toggle. See the [module docs](self).
pub struct ToggleView<State: 'static> {
    checked: bool,
    label: Option<String>,
    on_toggle: OnToggle<State>,
}

/// Create a toggle reflecting `checked` that fires `on_toggle(state, !checked)`
/// on a release inside its bounds — a **controlled** component, mirroring
/// `frust_material::switch`'s shape.
pub fn toggle<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_toggle: F,
) -> ToggleView<State> {
    ToggleView {
        checked,
        label: None,
        on_toggle: Rc::new(on_toggle),
    }
}

/// PascalCase alias for [`toggle`], matching the widget-fn vocabulary
/// (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn Toggle<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_toggle: F,
) -> ToggleView<State> {
    toggle(checked, on_toggle)
}

impl<State: 'static> ToggleView<State> {
    /// Name the control for assistive tech (the semantics node's label — a
    /// switch has no visible text of its own to derive one from).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// The retained widget for a [`ToggleView`].
pub struct ToggleWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    checked: bool,
    label: Option<String>,
    /// Knob position, `0.0` off .. `1.0` on (read unclamped — it overshoots).
    knob: Progress,
    /// Track/border/knob color blend, `0.0` off .. `1.0` on.
    fade: Progress,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on
    /// `Up`/`Cancel`. Nothing paints differently while armed — Glyph authors no
    /// pressed state (see the module docs).
    captured: bool,
    on_toggle: frust::authoring::ErasedArgCallback<bool>,
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl<State: 'static> View<State> for ToggleView<State> {
    type Element = ToggleWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ToggleWidget {
        let rest = if self.checked { 1.0 } else { 0.0 };
        ToggleWidget {
            checked: self.checked,
            label: self.label.clone(),
            knob: Progress::at_rest(rest, KNOB_DURATION, KNOB_CURVE),
            fade: Progress::at_rest(rest, FADE_DURATION, FADE_CURVE),
            captured: false,
            on_toggle: frust::authoring::erase_callback_arg(&self.on_toggle),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ToggleWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable — always reinstall the adapter.
        element.on_toggle = frust::authoring::erase_callback_arg(&self.on_toggle);
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label = self.label.clone();
            // Semantics-only, but `PAINT` is what bumps the root's semantics
            // dirty gate, and there is no narrower flag.
            flags |= ChangeFlags::PAINT;
        }
        if prev.checked != self.checked {
            // The app is the source of truth: adopt the value and animate to
            // it. Both lanes' timing is a constant, so `rebuild` can retarget
            // them without waiting for a paint-time theme.
            element.checked = self.checked;
            let target = if self.checked { 1.0 } else { 0.0 };
            element.knob.retarget(target);
            element.fade.retarget(target);
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for ToggleWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The touch slab, not the pill — see `TOUCH_TARGET`.
        bc.constrain(Size::new(
            TRACK_W.max(TOUCH_TARGET),
            TRACK_H.max(TOUCH_TARGET),
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Everything theme-derived is resolved (and copied out) up front, so
        // the borrow ends before the animation asks for the next frame.
        let track_size = Size::new(TRACK_W, TRACK_H);
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_toggle_colors(theme);
        let radius = track_radius(theme, track_size);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);

        if reduce_motion {
            // Land both lanes on the confirmed state and owe no frame.
            self.knob.snap();
            self.fade.snap();
        } else {
            // Both lanes advance every paint; either one still in flight owes
            // the next frame (paint-only — the slab's size never changes).
            let now = ctx.frame_time();
            let knob_running = self.knob.advance(now);
            let fade_running = self.fade.advance(now);
            if knob_running || fade_running {
                ctx.request_frame();
            }
        }

        // The pill keeps its authored size and centers in the slab, so paint
        // and hit-testing agree however the box was constrained.
        let box_size = ctx.size();
        let origin = ctx.origin()
            + Vec2::new(
                ((box_size.width - TRACK_W) / 2.0).max(0.0),
                ((box_size.height - TRACK_H) / 2.0).max(0.0),
            );
        let fade = self.fade.value_clamped();

        // Track fill, then its hairline border — stroked on the border's own
        // centerline (inset half a border) so the painted box is border-box
        // exact.
        scene.fill_rounded_rect(
            origin,
            track_size,
            radius,
            lerp_color(colors.track_off, colors.track_on, fade),
        );
        let inset = BORDER_W / 2.0;
        let outline = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, track_size).inset(-inset),
            (radius - inset).max(0.0),
        );
        scene.stroke_path(
            origin,
            &Shape::to_path(&outline, PATH_TOLERANCE),
            BORDER_W,
            &Brush::Solid(lerp_color(colors.border_off, colors.border_on, fade)),
        );

        // Knob: unclamped travel (the spring's overshoot is intended), with the
        // light-mode lift shadow beneath it.
        let knob_origin =
            origin + Vec2::new(KNOB_OFFSET + KNOB_TRAVEL * self.knob.value(), KNOB_OFFSET);
        let knob_size = Size::new(KNOB_SIZE, KNOB_SIZE);
        let knob_radius = KNOB_SIZE / 2.0;
        if let Some(shadow) = colors.knob_shadow {
            scene.draw_shadow(
                knob_origin + Vec2::new(0.0, KNOB_SHADOW_DY),
                knob_size,
                knob_radius,
                KNOB_SHADOW_BLUR,
                shadow,
            );
        }
        scene.fill_rounded_rect(
            knob_origin,
            knob_size,
            knob_radius,
            lerp_color(colors.knob_off, colors.knob_on, fade),
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
                EventResult::Handled
            }
            PointerPhase::Move => {
                // Nothing paints differently while armed, so a captured move
                // is consumed without a redraw; `Up` re-tests the position.
                if self.captured {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                // `ctx.size()` is the touch slab, so the margin around the pill
                // is live too.
                if inside(p.position, ctx.size()) {
                    // Report the *requested* value; never self-toggle.
                    (self.on_toggle)(ctx, !self.checked);
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Switch, |node| {
            node.set_toggled(Toggled::from(self.checked));
            node.add_action(Action::Click);
            if let Some(label) = &self.label {
                node.set_label(label.as_str());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{PointerButton, PointerEvent};
    use std::any::Any;

    #[derive(Default)]
    struct ToggleState {
        last: Option<bool>,
        toggles: u32,
    }

    /// Records the paint commands the token tables assert over, in paint order:
    /// `rrects` = [track, knob], `strokes` = [border], `shadows` = the
    /// light-only knob lift.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// The laid-out box: the 44-square touch slab, not the pill.
    const SLAB: Size = Size::new(TOUCH_TARGET, TOUCH_TARGET);

    /// Where the centered pill starts inside [`SLAB`] — every geometry
    /// assertion below is expressed relative to this, so the source-faithful
    /// pill numbers stay readable.
    fn pill_origin() -> Point {
        Point::new(
            (TOUCH_TARGET - TRACK_W) / 2.0,
            (TOUCH_TARGET - TRACK_H) / 2.0,
        )
    }

    fn widget(checked: bool) -> ToggleWidget {
        build(&toggle::<ToggleState, _>(
            checked,
            |s: &mut ToggleState, v| {
                s.last = Some(v);
                s.toggles += 1;
            },
        ))
    }

    fn build(view: &ToggleView<ToggleState>) -> ToggleWidget {
        let mut counter = 0u64;
        View::<ToggleState>::build(view, &mut BuildCtx::new(&mut counter))
    }

    /// Paint at frame time `ms`, returning the recorder and whether the widget
    /// asked for another frame.
    fn paint_at(w: &mut ToggleWidget, theme: Option<&Theme>, ms: f64) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let base = PaintCtx::for_test(Point::ZERO, SLAB, ft_ms(ms));
        let mut ctx = match theme {
            Some(t) => base.with_theme(t),
            None => base,
        };
        w.paint(&mut ctx, &mut rec);
        let again = ctx.needs_frame();
        (rec, again)
    }

    fn paint(w: &mut ToggleWidget, theme: Option<&Theme>) -> Recorder {
        paint_at(w, theme, 0.0).0
    }

    /// The knob's x offset **inside the pill** — the source's own frame of
    /// reference (`0` off, `KNOB_TRAVEL` across), independent of where the pill
    /// sits in the slab.
    fn knob_x(rec: &Recorder) -> f64 {
        rec.rrects[1].0.x - pill_origin().x
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut ToggleWidget, state: &mut ToggleState, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, SLAB);
        w.event(&mut ctx, event);
    }

    // ---- Token tables ------------------------------------------------------

    #[test]
    fn glyph_dark_theme_paints_the_source_token_table() {
        let theme = crate::baseline();
        let scheme = theme.scheme();

        let mut off = widget(false);
        let rec = paint(&mut off, Some(&theme));
        assert_eq!(rec.rrects.len(), 2, "track + knob only");
        // Track: `--bg-overlay` #272d3d, a pill centered in the slab.
        assert_eq!(rec.rrects[0].3, scheme.surface_container_highest);
        assert_eq!(rec.rrects[0].3, Color::from_rgb8(0x27, 0x2d, 0x3d));
        assert_eq!(rec.rrects[0].1, Size::new(TRACK_W, TRACK_H));
        assert_eq!(rec.rrects[0].0, pill_origin());
        assert_eq!(rec.rrects[0].2, TRACK_H / 2.0);
        // Border: `--border-bright` rgba(242,234,217,0.18).
        assert_eq!(
            rec.strokes[0],
            with_alpha(Color::from_rgb8(0xf2, 0xea, 0xd9), 0.18)
        );
        // Knob: `--fg-dim`'s themed stand-in — `on_surface_variant` #a39c88,
        // NOT the authored #6b6556 (the documented role gap; the divergence is
        // pinned here so it can't drift silently).
        assert_eq!(rec.rrects[1].3, scheme.on_surface_variant);
        assert_eq!(rec.rrects[1].3, Color::from_rgb8(0xa3, 0x9c, 0x88));
        assert_ne!(rec.rrects[1].3, TOGGLE_KNOB_OFF, "and it is not `--fg-dim`");
        assert_eq!(rec.rrects[1].1, Size::new(KNOB_SIZE, KNOB_SIZE));
        assert_eq!(knob_x(&rec), KNOB_OFFSET, "off rests at the pill inset");
        assert_eq!(rec.rrects[1].0.y, pill_origin().y + KNOB_OFFSET);
        assert!(rec.shadows.is_empty(), "dark authors no knob shadow");

        let mut on = widget(true);
        let rec = paint(&mut on, Some(&theme));
        // Track: `--amber-faint` rgba(255,182,39,0.12).
        assert_eq!(
            rec.rrects[0].3,
            with_alpha(Color::from_rgb8(0xff, 0xb6, 0x27), 0.12)
        );
        // Border: `--amber-border` rgba(255,182,39,0.35).
        assert_eq!(
            rec.strokes[0],
            with_alpha(Color::from_rgb8(0xff, 0xb6, 0x27), 0.35)
        );
        // Knob: `--amber` #ffb627 — the bright-fill role, not accent text.
        assert_eq!(rec.rrects[1].3, scheme.primary_container);
        assert_eq!(rec.rrects[1].3, Color::from_rgb8(0xff, 0xb6, 0x27));
        assert_eq!(knob_x(&rec), KNOB_OFFSET + KNOB_TRAVEL);
        assert!(rec.shadows.is_empty());
    }

    #[test]
    fn glyph_light_theme_paints_a_white_knob_with_a_lift_shadow() {
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let scheme = theme.scheme();

        let mut off = widget(false);
        let rec = paint(&mut off, Some(&theme));
        // Track: `--bg-overlay` #e8e1d0 (the light ramp's darkest slot).
        assert_eq!(rec.rrects[0].3, scheme.surface_container_highest);
        assert_eq!(rec.rrects[0].3, Color::from_rgb8(0xe8, 0xe1, 0xd0));
        // Border: `--border-bright` rgba(34,29,18,0.18).
        assert_eq!(
            rec.strokes[0],
            with_alpha(Color::from_rgb8(0x22, 0x1d, 0x12), 0.18)
        );
        // Knob: the white `--bg-surface` slot, over a 0 1px 2px ink lift.
        assert_eq!(rec.rrects[1].3, scheme.surface_container_lowest);
        assert_eq!(rec.rrects[1].3, Color::from_rgb8(0xff, 0xff, 0xff));
        assert_eq!(knob_x(&rec), KNOB_OFFSET);
        assert_eq!(rec.shadows.len(), 1, "light lifts the knob");
        let (shadow_origin, shadow_size, _, blur, shadow_color) = rec.shadows[0];
        assert_eq!(
            shadow_origin,
            pill_origin() + Vec2::new(KNOB_OFFSET, KNOB_OFFSET + KNOB_SHADOW_DY)
        );
        assert_eq!(shadow_size, Size::new(KNOB_SIZE, KNOB_SIZE));
        assert_eq!(blur, KNOB_SHADOW_BLUR);
        assert_eq!(shadow_color, with_alpha(scheme.shadow, KNOB_SHADOW_ALPHA));
        // The `shadow` role is warm ink #221d12 on Glyph light, not black.
        assert_eq!(
            shadow_color,
            with_alpha(Color::from_rgb8(0x22, 0x1d, 0x12), KNOB_SHADOW_ALPHA)
        );

        let mut on = widget(true);
        let rec = paint(&mut on, Some(&theme));
        // Track: `--amber-faint` rgba(163,101,10,0.10) — a re-tuned light alpha.
        assert_eq!(
            rec.rrects[0].3,
            with_alpha(Color::from_rgb8(0xa3, 0x65, 0x0a), 0.10)
        );
        // Border: `--amber-border` rgba(163,101,10,0.38).
        assert_eq!(
            rec.strokes[0],
            with_alpha(Color::from_rgb8(0xa3, 0x65, 0x0a), 0.38)
        );
        // Knob: `--amber-fill` #ffb627 — identical to dark.
        assert_eq!(rec.rrects[1].3, scheme.primary_container);
        assert_eq!(rec.rrects[1].3, Color::from_rgb8(0xff, 0xb6, 0x27));
        assert_eq!(knob_x(&rec), KNOB_OFFSET + KNOB_TRAVEL);
        // The source never overrides `box-shadow` on `:checked`.
        assert_eq!(rec.shadows.len(), 1, "the lift survives the checked state");
    }

    #[test]
    fn unthemed_paint_uses_the_dark_fallback_constants() {
        let mut off = widget(false);
        let rec = paint(&mut off, None);
        assert_eq!(rec.rrects[0].3, TOGGLE_TRACK_OFF);
        assert_eq!(rec.rrects[0].2, TRACK_H / 2.0, "pill radius");
        assert_eq!(rec.rrects[1].3, TOGGLE_KNOB_OFF, "the exact `fg-dim` hex");
        assert_eq!(
            rec.strokes[0],
            with_alpha(TOGGLE_BORDER_INK, BORDER_OFF_ALPHA)
        );
        assert!(rec.shadows.is_empty());

        let mut on = widget(true);
        let rec = paint(&mut on, None);
        assert_eq!(
            rec.rrects[0].3,
            with_alpha(TOGGLE_ACCENT, TRACK_ON_ALPHA_DARK)
        );
        assert_eq!(rec.rrects[1].3, TOGGLE_ACCENT);
        assert_eq!(
            rec.strokes[0],
            with_alpha(TOGGLE_ACCENT, BORDER_ON_ALPHA_DARK)
        );
    }

    // ---- Layout slab + centered pill --------------------------------------

    #[test]
    fn layout_requests_a_touch_slab_around_the_pill() {
        let mut w = widget(false);
        let mut ctx = LayoutCtx::new();
        let size = w.layout(&mut ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert_eq!(size, SLAB);
        assert!(
            size.width > TRACK_W && size.height > TRACK_H,
            "the interactive box is larger than the 38×22 pill it paints"
        );
        assert!(
            size.width >= TOUCH_TARGET && size.height >= TOUCH_TARGET,
            "and clears WCAG 2.2 / iOS's minimum on both axes (4dp under \
             Android's 48dp, accepted — see `TOUCH_TARGET`'s doc)"
        );
    }

    #[test]
    fn paint_centers_the_fixed_pill_in_whatever_box_it_is_given() {
        // A stretched/tightened box must move the pill, never scale it — that
        // is what keeps paint and hit-testing (the whole box) in agreement.
        let mut w = widget(true);
        let mut rec = Recorder::default();
        let box_size = Size::new(80.0, 60.0);
        let mut ctx = PaintCtx::for_test(Point::new(10.0, 20.0), box_size, ft_ms(0.0));
        w.paint(&mut ctx, &mut rec);

        let expected = Point::new(
            10.0 + (box_size.width - TRACK_W) / 2.0,
            20.0 + (box_size.height - TRACK_H) / 2.0,
        );
        assert_eq!(rec.rrects[0].0, expected);
        assert_eq!(rec.rrects[0].1, Size::new(TRACK_W, TRACK_H), "never scaled");
        // The knob's on-state rest spot rides with the pill.
        assert_eq!(
            rec.rrects[1].0,
            expected + Vec2::new(KNOB_OFFSET + KNOB_TRAVEL, KNOB_OFFSET)
        );
    }

    #[test]
    fn the_overshooting_knob_stays_inside_the_slab() {
        // The spring reads unclamped, so the peak is past the rest spot — it
        // must still land inside the painted box.
        let prev = toggle::<ToggleState, _>(false, |_s, _v| {});
        let mut w = build(&prev);
        let next = toggle::<ToggleState, _>(true, |_s, _v| {});
        let mut counter = 0u64;
        View::<ToggleState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        paint_at(&mut w, None, 0.0);

        let mut peak_right = f64::MIN;
        for ms in (10..=210).step_by(10) {
            let (rec, _) = paint_at(&mut w, None, ms as f64);
            peak_right = peak_right.max(rec.rrects[1].0.x + KNOB_SIZE);
        }
        let rest_right = pill_origin().x + KNOB_OFFSET + KNOB_TRAVEL + KNOB_SIZE;
        assert!(
            peak_right > rest_right,
            "the overshoot is real ({peak_right} vs {rest_right})"
        );
        assert!(
            peak_right <= pill_origin().x + TRACK_W - BORDER_W,
            "yet the knob never escapes the track's inner edge ({peak_right})"
        );
        assert!(
            peak_right <= SLAB.width,
            "nor the slab ({peak_right} vs {})",
            SLAB.width
        );
    }

    #[test]
    fn a_tap_in_the_slab_margin_outside_the_pill_still_toggles() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        // Bottom-right corner of the slab: outside the pill, inside the target.
        let (x, y) = (TOUCH_TARGET - 1.0, TOUCH_TARGET - 1.0);
        assert!(
            x > pill_origin().x + TRACK_W && y > pill_origin().y + TRACK_H,
            "the probe really is outside the painted pill"
        );
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, x, y));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, x, y));
        assert_eq!(state.last, Some(true));
        assert_eq!(state.toggles, 1);
    }

    // ---- Knob travel + frame requests -------------------------------------

    #[test]
    fn knob_travels_the_source_distance_and_settles() {
        let prev = toggle::<ToggleState, _>(false, |_s, _v| {});
        let mut w = build(&prev);
        let (rec, again) = paint_at(&mut w, None, 0.0);
        assert_eq!(knob_x(&rec), KNOB_OFFSET, "off rests at the inset");
        assert!(!again, "a resting toggle requests no frame");

        // The app confirms the flip; the animation starts in `rebuild`.
        let next = toggle::<ToggleState, _>(true, |_s, _v| {});
        let mut counter = 0u64;
        let flags =
            View::<ToggleState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(flags.needs_paint());
        assert!(!flags.needs_layout(), "the track never resizes");

        // The first paint only seeds the animation clock (see
        // `AnimationController::advance`), so travel starts on the next one.
        let (rec, again) = paint_at(&mut w, None, 0.0);
        assert_eq!(knob_x(&rec), KNOB_OFFSET);
        assert!(again, "an in-flight knob owes another frame");

        let settled_x = KNOB_OFFSET + KNOB_TRAVEL;
        let mut peak_x = f64::MIN;
        for ms in (10..=210).step_by(10) {
            let (rec, again) = paint_at(&mut w, None, ms as f64);
            peak_x = peak_x.max(knob_x(&rec));
            assert!(again, "still animating at {ms}ms");
        }
        assert!(
            peak_x > settled_x,
            "the spatial curve overshoots its rest spot ({peak_x} vs {settled_x})"
        );

        let (rec, again) = paint_at(&mut w, None, 400.0);
        assert_eq!(knob_x(&rec), settled_x, "lands exactly 16px across");
        assert!(!again, "a settled toggle stops requesting frames");
        assert_eq!(rec.rrects[1].3, TOGGLE_ACCENT, "and the fade completed");
    }

    #[test]
    fn a_reversal_travels_back_to_the_off_inset() {
        let prev = toggle::<ToggleState, _>(true, |_s, _v| {});
        let mut w = build(&prev);
        let next = toggle::<ToggleState, _>(false, |_s, _v| {});
        let mut counter = 0u64;
        View::<ToggleState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        let (_, again) = paint_at(&mut w, None, 0.0);
        assert!(again);
        let (rec, again) = paint_at(&mut w, None, 400.0);
        assert_eq!(knob_x(&rec), KNOB_OFFSET);
        assert!(!again);
        assert_eq!(rec.rrects[1].3, TOGGLE_KNOB_OFF);
    }

    #[test]
    fn reduce_motion_paints_the_final_state_at_once_and_requests_no_frame() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;

        let prev = toggle::<ToggleState, _>(false, |_s, _v| {});
        let mut w = build(&prev);
        let next = toggle::<ToggleState, _>(true, |_s, _v| {});
        let mut counter = 0u64;
        View::<ToggleState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        // The first paint after the flip is already the settled state — no
        // seeding frame, no travel, nothing owed.
        let (rec, again) = paint_at(&mut w, Some(&theme), 0.0);
        assert!(!again, "reduce_motion owes no frame");
        assert_eq!(
            knob_x(&rec),
            KNOB_OFFSET + KNOB_TRAVEL,
            "the knob lands immediately"
        );
        assert_eq!(
            rec.rrects[1].3,
            theme.scheme().primary_container,
            "and the color lane is done too"
        );

        // And it stays there: a later frame neither moves nor re-requests.
        let (rec, again) = paint_at(&mut w, Some(&theme), 400.0);
        assert!(!again);
        assert_eq!(knob_x(&rec), KNOB_OFFSET + KNOB_TRAVEL);
    }

    #[test]
    fn reduce_motion_snaps_a_reversal_back_as_well() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;

        let prev = toggle::<ToggleState, _>(true, |_s, _v| {});
        let mut w = build(&prev);
        let next = toggle::<ToggleState, _>(false, |_s, _v| {});
        let mut counter = 0u64;
        View::<ToggleState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        let (rec, again) = paint_at(&mut w, Some(&theme), 0.0);
        assert!(!again);
        assert_eq!(knob_x(&rec), KNOB_OFFSET);
        assert_eq!(rec.rrects[1].3, theme.scheme().on_surface_variant);
    }

    // ---- Interaction -------------------------------------------------------

    #[test]
    fn release_inside_reports_the_requested_value_once() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 8.0, 11.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 8.0, 11.0));
        assert_eq!(state.last, Some(true));
        assert_eq!(state.toggles, 1);
        assert!(!w.checked, "a controlled toggle never self-mutates");

        let mut w = widget(true);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 8.0, 11.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 8.0, 11.0));
        assert_eq!(state.last, Some(false));
        assert!(w.checked, "still checked until the app rebuilds it");
    }

    #[test]
    fn release_outside_does_not_fire() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 8.0, 11.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 11.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 11.0));
        assert_eq!(state.toggles, 0);
        assert!(!w.captured, "the release disarms either way");
    }

    #[test]
    fn cancel_disarms_the_press() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 8.0, 11.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 8.0, 11.0));
        assert!(!w.captured);
        // A post-cancel Up is not this widget's gesture any more.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 8.0, 11.0));
        assert_eq!(state.toggles, 0);
    }

    #[test]
    fn an_unarmed_move_is_ignored() {
        let mut w = widget(false);
        let mut state = ToggleState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, SLAB);
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 8.0, 11.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!ctx.needs_redraw(), "a hover must not request a redraw");
    }

    // ---- Semantics ---------------------------------------------------------

    #[test]
    fn semantics_reports_a_labelled_switch_with_its_toggled_state() {
        // `SemanticsCtx`'s constructor is crate-private to `frust-core`, so
        // this drives a real `RenderRoot` pass rather than the context itself
        // (the `crate::segmented_control` precedent).
        fn logic(_s: &mut ToggleState) -> ToggleView<ToggleState> {
            toggle::<ToggleState, _>(true, |_s, _v| {}).label("auto-reconnect")
        }
        let mut root: frust_core::RenderRoot<ToggleState, ToggleView<ToggleState>> =
            frust_core::RenderRoot::new();
        let mut state = ToggleState::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        let update = root.semantics();

        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Switch)
            .expect("the toggle contributes a Role::Switch node");
        assert_eq!(node.toggled(), Some(Toggled::True));
        assert!(node.supports_action(Action::Click));
        assert_eq!(node.label(), Some("auto-reconnect"));
        // The a11y bounds are the touch slab, not the painted pill — the
        // bigger target is the point.
        let bounds = node.bounds().expect("the switch node has bounds");
        assert_eq!(
            (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0),
            (TOUCH_TARGET, TOUCH_TARGET)
        );
    }

    #[test]
    fn semantics_mirrors_an_unchecked_unlabelled_toggle() {
        fn logic(_s: &mut ToggleState) -> ToggleView<ToggleState> {
            toggle::<ToggleState, _>(false, |_s, _v| {})
        }
        let mut root: frust_core::RenderRoot<ToggleState, ToggleView<ToggleState>> =
            frust_core::RenderRoot::new();
        let mut state = ToggleState::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        let update = root.semantics();

        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Switch)
            .expect("the toggle contributes a Role::Switch node");
        assert_eq!(node.toggled(), Some(Toggled::False));
        assert_eq!(node.label(), None);
    }

    #[test]
    fn a_label_change_alone_is_a_republish() {
        let prev = toggle::<ToggleState, _>(false, |_s, _v| {});
        let mut w = build(&prev);
        assert_eq!(w.label, None);
        let next = toggle::<ToggleState, _>(false, |_s, _v| {}).label("haptics");
        let mut counter = 0u64;
        let flags =
            View::<ToggleState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.label.as_deref(), Some("haptics"));
        assert!(
            flags.needs_paint(),
            "the semantics dirty gate rides `PAINT`"
        );
    }
}
