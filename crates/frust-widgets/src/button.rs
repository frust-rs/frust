//! The `Button` interactive widget (spec §6.4): a labelled, rounded pressable
//! that fires an app-state callback on release *inside* its bounds.
//!
//! [`button`] is the declarative view-fn; it produces a [`ButtonView`] carrying
//! the label and a typed `on_press` closure, which materialises into a retained
//! [`ButtonWidget`]. The masonry-verified interaction is **fire-on-up-inside**:
//! a `Down` inside captures the pointer and paints the pressed state; `Move`
//! only updates the pressed visual (cursor-inside); the callback fires on `Up`
//! *only if the release lands inside*. A `Cancel` (platform gesture steal) just
//! clears the pressed state. See `research/RESEARCH.md`.
//!
//! # Style variants (task 22)
//!
//! [`ButtonStyle`] (`.style(...)`) adds four variants beside the default
//! [`ButtonStyle::Primary`] (today's only look, preserved byte-for-byte under
//! every theme): [`ButtonStyle::Secondary`] (raised surface + outline border),
//! [`ButtonStyle::Ghost`] (transparent + muted label), [`ButtonStyle::Danger`]
//! (error border/text + an error-faint pressed wash), and [`ButtonStyle::Icon`]
//! (square, `Secondary`-shaped — sized to fit an icon-only label). Each
//! style's fill/border/label-role mapping is uniform across every design
//! language (Glyph's own text-vs-fill accent split, task 15, already lands on
//! these same M3 role names) — see [`ButtonStyle::resolve`]/
//! [`ButtonStyle::label_role`]. `.small()` selects a reduced padding scale;
//! `.loading(bool)` shows a rotating spinner in place of the label and
//! suppresses `on_press` while shown (disabled semantics — see
//! [`Widget::semantics`](struct.ButtonWidget.html#impl-Widget-for-ButtonWidget)).
//! The spinner freezes (and stops requesting frames) wherever it currently
//! sits under `Theme.motion.reduce_motion`, the same skip-animation shape
//! [`crate::material::loading_indicator`]'s morph loop uses.
//!
//! Precedence stays token-resolved (`docs/CODE_STANDARDS.md`'s Theming
//! conventions): every fill/border/label color below is `theme > fallback`
//! (this task adds no per-instance color override, so the explicit tier of
//! the usual three-tier precedence has nothing to win over yet — a future
//! task adding one would slot in above the theme resolution in
//! [`ButtonStyle::resolve`]).
//!
//! # Press-feedback scale (task 22)
//!
//! A `Down` scales the button to `0.96` over the theme's `durations.instant`
//! (100ms Glyph/Cupertino, 50ms M3) under the `exit` easing curve; `Up`/
//! `Cancel` springs it back to `1.0` via the theme's `default_spatial`
//! spring. [`PressAnim`] is a from-scratch inline of `motion::animated`'s
//! private `ImplicitAnim` lazy-retarget shape (task 12) — that type is
//! module-private to `motion::animated`, so this task's note ("inline the
//! controller approach rather than wrapping, if simpler") is followed
//! literally rather than exposing it. Like every animated widget in this
//! crate, the driver only advances **during paint**
//! (`docs/CODE_STANDARDS.md`'s Theming & Animation Conventions) — the event
//! pass (`Down`/`Up`/`Cancel`) only records the *target* scale, since
//! `EventCtx` carries no theme to resolve a `Timing` from.

use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::{Action, Role};
use frust_core::{
    AnimationController, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, EventCtx,
    EventResult, FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase,
    SemanticsCtx, Tween, View, Widget, any,
};
use frust_theme::Theme;
use kurbo::{Affine, Arc as KurboArc, Point, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

use crate::nav::transition::{TransitionDriver, make_driver};
use crate::text;
use crate::text::ThemeTextColor;
use crate::{Timing, material::state_layer::PRESSED_OPACITY};

/// Corner radius of the button's rounded-rect background, in logical px (the
/// unthemed fallback; a theme resolves this from `shape.small`).
const RADIUS: f64 = 6.0;
/// Horizontal padding around the label, in logical px. No `Theme` spacing
/// token exists to resolve this from (`frust-theme` publishes a shape
/// scale and a type scale, not a padding/spacing scale) — hoisted here as a
/// named constant per task 6f-10's metric-hardcode-migration pass rather than
/// left as a bare literal, pending a future spacing-token addition.
const PAD_X: f64 = 12.0;
/// Vertical padding around the label, in logical px. See [`PAD_X`]'s doc
/// comment — no suitable `Theme` token exists for this metric either.
const PAD_Y: f64 = 8.0;
/// Horizontal padding under `.small()` — see [`PAD_X`]'s doc comment (same
/// no-spacing-token gap; this is a proportionally reduced hand-tuned value,
/// not a token).
const SMALL_PAD_X: f64 = 8.0;
/// Vertical padding under `.small()` — see [`SMALL_PAD_X`].
const SMALL_PAD_Y: f64 = 4.0;
/// Resting background fill (unthemed fallback; a theme resolves this from
/// `colors.primary`).
const FILL: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
/// Pressed (darker) background fill (unthemed fallback; a theme resolves this
/// from a darkened `colors.primary` — see [`pressed_overlay`]).
const FILL_PRESSED: Color = Color::from_rgb8(0x1D, 0x4E, 0xD8);

/// Unthemed-fallback [`ButtonStyle::Secondary`]/[`ButtonStyle::Icon`] fill (a
/// light neutral "raised surface" look pre-theme — [`FILL`]/[`FILL_PRESSED`]
/// above aren't M3-sourced either; there is no M3 anchor to match without a
/// theme).
const SECONDARY_FILL: Color = Color::from_rgb8(0xE5, 0xE7, 0xEB);
/// Unthemed-fallback Secondary/Icon pressed fill (see [`SECONDARY_FILL`]).
const SECONDARY_FILL_PRESSED: Color = Color::from_rgb8(0xD1, 0xD5, 0xDB);
/// Unthemed-fallback Secondary/Icon outline border (see [`SECONDARY_FILL`]).
const SECONDARY_BORDER: Color = Color::from_rgb8(0x9C, 0xA3, 0xAF);
/// Unthemed-fallback [`ButtonStyle::Ghost`] pressed-wash ink (see
/// [`SECONDARY_FILL`]'s no-M3-anchor note).
const GHOST_PRESSED_INK: Color = Color::from_rgb8(0x11, 0x18, 0x27);
/// Unthemed-fallback [`ButtonStyle::Danger`] border/text color.
const DANGER_BORDER: Color = Color::from_rgb8(0xDC, 0x26, 0x26);
/// Unthemed-fallback Danger pressed-wash fill (see [`DANGER_BORDER`]).
const DANGER_PRESSED_WASH: Color = Color::from_rgb8(0xFE, 0xE2, 0xE2);
/// Unthemed-fallback ink color for every style's label/spinner (matches
/// `TextStyle::default()`'s own color — see [`ButtonStyle::resolve_ink`]).
const UNTHEMED_INK: Color = Color::from_rgb8(0x00, 0x00, 0x00);

/// The border stroke width, in logical px (a hairline, matching
/// `material::card`'s outlined-variant precedent).
const BORDER_WIDTH: f64 = 1.0;
/// Flattening tolerance for the border's rounded-rect stroke path — see
/// `material::card`'s identical precedent/rationale.
const BORDER_TOLERANCE: f64 = 0.1;

/// The fixed multiplier applied to a style's resting fill's RGB to synthesize
/// its pressed fill under a theme (a v1 stand-in reproducing today's press
/// contrast; M3 tonal state-layers land in phase 6c). `0.82` darkens by
/// roughly the same amount today's `FILL`→`FILL_PRESSED` step does.
const PRESSED_DARKEN: f32 = 0.82;

/// The press-feedback pivot scale while pressed (task 22's `0.96`).
const PRESSED_SCALE: f64 = 0.96;
/// The rest (unpressed) scale.
const REST_SCALE: f64 = 1.0;
/// Unthemed-fallback press-feedback duration (task 22's "100ms"), applied on
/// both directions when no theme is threaded. A theme instead resolves
/// `motion.durations.instant`, which differs per baseline (50ms M3, 100ms
/// Glyph/Cupertino — see `frust-theme::motion`'s module docs).
const FALLBACK_PRESS_DURATION: Duration = Duration::from_millis(100);

/// Loading-spinner radius, as a fraction of the button's own content height
/// (keeps it inside the padding — no icon/spinner size token exists to
/// resolve this from instead).
const SPINNER_RADIUS_RATIO: f64 = 0.4;
/// Loading-spinner minimum radius, in logical px (keeps a `.small()` button's
/// spinner from degenerating to an invisible dot).
const SPINNER_MIN_RADIUS: f64 = 5.0;
/// Loading-spinner stroke width, in logical px.
const SPINNER_STROKE_WIDTH: f64 = 2.0;
/// Loading-spinner sweep angle (a partial ring, the universal "activity
/// spinner" shape), in radians — 270°.
const SPINNER_SWEEP: f64 = std::f64::consts::PI * 1.5;
/// Loading-spinner full-rotation period, in ms.
///
/// **Community-approximate**: no design-token source publishes a spinner
/// rotation speed; ~900ms is the cadence common indeterminate
/// activity-indicator implementations converge on.
const SPINNER_PERIOD_MS: u64 = 900;

/// Darken a color by scaling its RGB components toward black by `factor`,
/// leaving alpha untouched. Used to synthesize a style's pressed fill from a
/// themed resting fill (see [`ButtonStyle::resolve`]).
fn pressed_overlay(color: Color, factor: f32) -> Color {
    let c = color.components;
    Color::new([c[0] * factor, c[1] * factor, c[2] * factor, c[3]])
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// `material::state_layer`/`material::card`'s identically-named helper).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Build the affine that scales uniformly by `scale` about the absolute
/// point `pivot` — mirrors `motion::animated::scale_about` (module-private
/// there); duplicated here per this module's inlining note.
fn scale_about(pivot: Point, scale: f64) -> Affine {
    Affine::translate((pivot.x, pivot.y))
        * Affine::scale(scale)
        * Affine::translate((-pivot.x, -pivot.y))
}

/// A view-held, typed press callback (erased to [`crate::ErasedCallback`] on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// Visual style variant for [`Button`] (task 22). Additive: the default
/// preserves today's only look byte-for-byte under every theme (acceptance
/// criterion 1). See the [module docs](self) for the full role-mapping intent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonStyle {
    /// Filled `primary` background + `on_primary` label — today's only style.
    #[default]
    Primary,
    /// Raised `surface_container_high` fill + `outline` border + `on_surface`
    /// label — a secondary/neutral action.
    Secondary,
    /// Transparent fill + `on_surface_variant` (muted) label — a
    /// low-emphasis action; pressed shows a faint `on_surface` wash at
    /// [`PRESSED_OPACITY`].
    Ghost,
    /// Transparent fill + `error` border/label — a destructive action;
    /// pressed shows the theme's `error_container` faint wash.
    Danger,
    /// Square, [`ButtonStyle::Secondary`]-shaped — sized to fit an icon-only
    /// label rather than wrapping arbitrary text width.
    Icon,
}

impl ButtonStyle {
    /// The themed label color role this style paints its text with (see
    /// `text::ThemeTextColor`). Consulted both to build the label child (so
    /// it resolves its own color at *its* layout pass) and to color the
    /// loading spinner identically.
    fn label_role(self) -> ThemeTextColor {
        match self {
            ButtonStyle::Primary => ThemeTextColor::OnPrimary,
            ButtonStyle::Secondary | ButtonStyle::Icon => ThemeTextColor::OnSurface,
            ButtonStyle::Ghost => ThemeTextColor::OnSurfaceVariant,
            ButtonStyle::Danger => ThemeTextColor::Error,
        }
    }

    /// The resolved ink color for this style's loading spinner — the same
    /// color its label would paint with under `theme`, computed directly
    /// (rather than through `ThemeTextColor`, a `Text`-widget-only
    /// abstraction) since the spinner is a raw stroked path, not a nested
    /// `Text` child. Unthemed: [`UNTHEMED_INK`] — matches `TextStyle::default`'s
    /// own color, since an unthemed label ignores its role entirely (see
    /// `text`'s module docs).
    fn resolve_ink(self, theme: Option<&Theme>) -> Color {
        match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                match self {
                    ButtonStyle::Primary => scheme.on_primary,
                    ButtonStyle::Secondary | ButtonStyle::Icon => scheme.on_surface,
                    ButtonStyle::Ghost => scheme.on_surface_variant,
                    ButtonStyle::Danger => scheme.error,
                }
            }
            None => UNTHEMED_INK,
        }
    }

    /// The resolved `(resting fill, pressed fill, optional border color)` for
    /// this style. Themed: per-style `ColorScheme` roles (see the
    /// [module docs](self)). Unthemed: this style's own fallback constants —
    /// [`ButtonStyle::Primary`]'s exactly reproduce [`FILL`]/[`FILL_PRESSED`],
    /// preserving today's pre-theme rendering (acceptance criterion 1).
    fn resolve(self, theme: Option<&Theme>) -> StylePaint {
        match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                match self {
                    ButtonStyle::Primary => StylePaint {
                        fill: scheme.primary,
                        fill_pressed: pressed_overlay(scheme.primary, PRESSED_DARKEN),
                        border: None,
                    },
                    ButtonStyle::Secondary | ButtonStyle::Icon => StylePaint {
                        fill: scheme.surface_container_high,
                        fill_pressed: pressed_overlay(
                            scheme.surface_container_high,
                            PRESSED_DARKEN,
                        ),
                        border: Some(scheme.outline),
                    },
                    ButtonStyle::Ghost => StylePaint {
                        fill: Color::TRANSPARENT,
                        fill_pressed: with_alpha(scheme.on_surface, PRESSED_OPACITY),
                        border: None,
                    },
                    ButtonStyle::Danger => StylePaint {
                        fill: Color::TRANSPARENT,
                        fill_pressed: scheme.error_container,
                        border: Some(scheme.error),
                    },
                }
            }
            None => match self {
                ButtonStyle::Primary => StylePaint {
                    fill: FILL,
                    fill_pressed: FILL_PRESSED,
                    border: None,
                },
                ButtonStyle::Secondary | ButtonStyle::Icon => StylePaint {
                    fill: SECONDARY_FILL,
                    fill_pressed: SECONDARY_FILL_PRESSED,
                    border: Some(SECONDARY_BORDER),
                },
                ButtonStyle::Ghost => StylePaint {
                    fill: Color::TRANSPARENT,
                    fill_pressed: with_alpha(GHOST_PRESSED_INK, PRESSED_OPACITY),
                    border: None,
                },
                ButtonStyle::Danger => StylePaint {
                    fill: Color::TRANSPARENT,
                    fill_pressed: DANGER_PRESSED_WASH,
                    border: Some(DANGER_BORDER),
                },
            },
        }
    }
}

/// The resolved per-style paint values — see [`ButtonStyle::resolve`].
struct StylePaint {
    fill: Color,
    fill_pressed: Color,
    border: Option<Color>,
}

/// Resolve the effective press-feedback [`Timing`]: pressed (Down) uses
/// `durations.instant` under the theme's `exit` easing; releasing (Up/Cancel)
/// uses the theme's `default_spatial` spring — see the [module docs](self).
/// Falls back to [`FALLBACK_PRESS_DURATION`] under a plain ease when no theme
/// is threaded.
fn resolve_press_timing(theme: Option<&Theme>, pressed: bool) -> Timing {
    match theme {
        Some(theme) => {
            if pressed {
                let instant_ms = theme.motion.durations.instant.max(0.0);
                Timing::Duration(
                    Duration::from_secs_f64(instant_ms / 1000.0),
                    theme.motion.easing.exit,
                )
            } else {
                Timing::Spring(theme.motion.default_spatial)
            }
        }
        None => {
            let curve = if pressed {
                Curve::EaseIn
            } else {
                Curve::EaseOut
            };
            Timing::Duration(FALLBACK_PRESS_DURATION, curve)
        }
    }
}

/// An inlined implicit-scale driver for the press-feedback animation — the
/// same lazy-retarget shape `motion::animated`'s module-private `ImplicitAnim`
/// uses (task 12), reproduced here per this module's inlining note (see the
/// [module docs](self)). `Down`/`Up`/`Cancel` (the event pass, which carries
/// no theme) only call [`PressAnim::set_pressed`]; `paint` (which does carry
/// one) resolves the direction's [`Timing`] and calls [`PressAnim::advance`],
/// which lazily launches the retarget the first time it sees the target
/// disagree with what's actually driving.
struct PressAnim {
    target: f64,
    driving_target: f64,
    tween: Tween<f64>,
    driver: TransitionDriver,
}

impl PressAnim {
    /// A fresh driver settled at rest — no animate-in on first mount.
    fn new() -> Self {
        let (driver, _) = make_driver(Timing::Duration(Duration::ZERO, Curve::Linear));
        Self {
            target: REST_SCALE,
            driving_target: REST_SCALE,
            tween: Tween::new(REST_SCALE, REST_SCALE),
            driver,
        }
    }

    /// The current interpolated scale.
    fn value(&self) -> f64 {
        self.tween.lerp(self.driver.value())
    }

    /// Record the target scale for the next [`Self::advance`] call (called
    /// from the event pass — see the [module docs](self)).
    fn set_pressed(&mut self, pressed: bool) {
        self.target = if pressed { PRESSED_SCALE } else { REST_SCALE };
    }

    /// Advance one frame at `now` under `timing`. Returns whether still
    /// animating (the caller should `PaintCtx::request_frame`).
    fn advance(&mut self, now: FrameTime, timing: Timing) -> bool {
        if self.target != self.driving_target {
            let from = self.value();
            self.tween = Tween::new(from, self.target);
            self.driving_target = self.target;
            let (driver, _) = make_driver(timing);
            self.driver = driver;
        }
        self.driver.advance(now).animating
    }
}

/// Build the type-erased label view, tagged with `style`'s themed color role
/// (see [`ButtonStyle::label_role`]) so the label reads correctly against its
/// style's fill (an unthemed button keeps its black label — see `text`'s
/// module docs). Shared by build/rebuild/teardown so the role stays
/// consistent across the child's whole lifecycle.
fn label_view<State: 'static>(label: String, style: ButtonStyle) -> frust_core::AnyView<State> {
    any::<State, _>(text(label).themed_role(style.label_role()))
}

/// A declarative pressable button. See the [module docs](self).
pub struct ButtonView<State: 'static> {
    label: String,
    on_press: OnPress<State>,
    style: ButtonStyle,
    small: bool,
    loading: bool,
}

/// Create a button labelled `label` that runs `on_press` against the app state
/// when released inside its bounds.
pub fn button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> ButtonView<State> {
    ButtonView {
        label: label.into(),
        on_press: Rc::new(on_press),
        style: ButtonStyle::default(),
        small: false,
        loading: false,
    }
}

/// PascalCase alias for [`button`], matching the container view-fn vocabulary.
#[allow(non_snake_case)]
pub fn Button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> ButtonView<State> {
    button(label, on_press)
}

impl<State: 'static> ButtonView<State> {
    /// Select the visual style (default [`ButtonStyle::Primary`], task 22).
    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
        self
    }

    /// Use the reduced `.small()` padding scale (task 22).
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// Show a loading spinner in place of the label while `loading` is
    /// `true`, suppressing `on_press` and reporting disabled semantics for as
    /// long as it's shown (task 22).
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }
}

/// The retained widget for a [`ButtonView`]. The label is a nested
/// [`crate::TextWidget`] owned as a [`ChildPod`].
pub struct ButtonWidget {
    label: ChildPod,
    /// The label text, retained for the semantics node's accessible name (the
    /// label lives inside the `label` pod as a `TextWidget`; a button is a single
    /// a11y node, so it reads its name from here rather than recursing).
    label_text: String,
    /// The pressed *visual* state (background darkens, and the press-scale
    /// animation targets `PRESSED_SCALE`). Follows the cursor in/out while
    /// captured, and is purely cosmetic.
    pressed: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    /// Gates all `Move`/`Up` handling so a hover `Move` (dispatched by the
    /// desktop shell on every cursor motion) never latches `pressed` or fires
    /// the callback without a preceding press.
    captured: bool,
    on_press: crate::ErasedCallback,
    style: ButtonStyle,
    small: bool,
    loading: bool,
    /// The press-feedback scale driver — see the [module docs](self).
    press: PressAnim,
    /// The loading-spinner rotation controller (task 22) — always `repeat()`ing
    /// once started in `build`; only advanced/painted while `loading`.
    spinner: AnimationController,
}

/// Whether a widget-local `pos` lies within a `size`-sized box anchored at the
/// origin (the button's own bounds).
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl ButtonWidget {
    /// The corner radius: themed `shape.small` (8dp — one step up from today's 6px
    /// fallback, the closest M3 token; a visually negligible change), resolved
    /// against the box so it never exceeds a pill. Unthemed: the [`RADIUS`]
    /// constant exactly. Shared by every style, including
    /// [`ButtonStyle::Icon`] ("secondary-shaped" — see the [module docs](self)).
    fn resolve_radius(theme: Option<&Theme>, size: Size) -> f64 {
        match theme {
            Some(theme) => {
                frust_theme::ShapeScale::resolve(theme.shape.small, size.width, size.height)
            }
            None => RADIUS,
        }
    }

    /// Paint the loading spinner (a rotating partial ring) centered on the
    /// button, in `color`.
    fn paint_spinner(&self, ctx: &PaintCtx, scene: &mut dyn PaintScene, color: Color) {
        let size = ctx.size();
        let center_local = Point::new(size.width / 2.0, size.height / 2.0);
        let radius = (size.height / 2.0 * SPINNER_RADIUS_RATIO).max(SPINNER_MIN_RADIUS);
        let angle = self.spinner.value() * std::f64::consts::TAU;
        let arc = KurboArc::new(
            center_local,
            Vec2::new(radius, radius),
            angle,
            SPINNER_SWEEP,
            0.0,
        );
        let path = arc.to_path(0.1);
        scene.stroke_path(
            ctx.origin(),
            &path,
            SPINNER_STROKE_WIDTH,
            &Brush::Solid(color),
        );
    }
}

impl<State: 'static> View<State> for ButtonView<State> {
    type Element = ButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ButtonWidget {
        let label_view = label_view::<State>(self.label.clone(), self.style);
        let mut spinner = AnimationController::new(Duration::from_millis(SPINNER_PERIOD_MS))
            .with_curve(Curve::Linear);
        spinner.repeat();
        ButtonWidget {
            label: crate::build_child(&label_view, ctx),
            label_text: self.label.clone(),
            pressed: false,
            captured: false,
            on_press: crate::erase_callback(&self.on_press),
            style: self.style,
            small: self.small,
            loading: self.loading,
            press: PressAnim::new(),
            spinner,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable — always reinstall the adapter.
        element.on_press = crate::erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label || prev.style != self.style {
            element.label_text = self.label.clone();
            let prev_view = label_view::<State>(prev.label.clone(), prev.style);
            let next_view = label_view::<State>(self.label.clone(), self.style);
            flags |= crate::rebuild_child(&prev_view, &next_view, &mut element.label, ctx);
        }
        if prev.style != self.style {
            element.style = self.style;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.small != self.small {
            element.small = self.small;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.loading != self.loading {
            element.loading = self.loading;
            flags |= ChangeFlags::PAINT;
            if self.loading {
                // Loading suppresses the whole event pass from here on (see
                // `Widget::event`), so a still-armed press would never see
                // its terminating Up/Cancel — disarm it now rather than
                // leave a stale capture flag behind.
                element.pressed = false;
                element.captured = false;
                element.press.set_pressed(false);
            }
        }
        flags
    }

    fn teardown(&self, element: &mut ButtonWidget, ctx: &mut BuildCtx<'_>) {
        let label_view = label_view::<State>(self.label.clone(), self.style);
        crate::teardown_child(&label_view, &mut element.label, ctx);
    }
}

impl Widget for ButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let (pad_x, pad_y) = if self.small {
            (SMALL_PAD_X, SMALL_PAD_Y)
        } else {
            (PAD_X, PAD_Y)
        };
        // Lay the label out inside the padded content box, then grow to wrap it.
        let inset = Size::new(pad_x * 2.0, pad_y * 2.0);
        let inner_max = Size::new(
            (bc.max().width - inset.width).max(0.0),
            (bc.max().height - inset.height).max(0.0),
        );
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(inner_max));
        self.label.set_origin(Point::new(pad_x, pad_y));
        let mut size = Size::new(
            label_size.width + inset.width,
            label_size.height + inset.height,
        );
        if self.style == ButtonStyle::Icon {
            // Square, secondary-shaped (module docs): grow the shorter side to
            // match the longer one, then re-center the label inside it.
            let side = size.width.max(size.height);
            size = Size::new(side, side);
            self.label.set_origin(Point::new(
                (side - label_size.width) / 2.0,
                (side - label_size.height) / 2.0,
            ));
        }
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Resolve every theme-derived value up front, before any `&mut ctx`
        // use below (`request_frame`) — `theme` borrows `ctx` shared, and its
        // last use must precede a later mutable reborrow for this to
        // typecheck under NLL (the reason `ink`/`press_timing` are resolved
        // here rather than lazily, next to where each is consumed).
        let theme = Theme::from_paint_ctx(ctx);
        let paint = self.style.resolve(theme);
        let radius = Self::resolve_radius(theme, ctx.size());
        // Press-feedback scale: `Down`/`Up`/`Cancel` only recorded the target
        // (see the module docs); resolve the direction's `Timing` now that a
        // theme is in scope.
        let press_timing = resolve_press_timing(theme, self.pressed);
        let ink = self.style.resolve_ink(theme);
        // Resolved now (last use of the shared `theme` borrow — see the
        // comment above) so the `loading` branch below can check it without
        // re-borrowing `theme` across the intervening `&mut ctx` calls.
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);

        let fill = if self.pressed {
            paint.fill_pressed
        } else {
            paint.fill
        };

        if self.press.advance(ctx.frame_time(), press_timing) {
            ctx.request_frame();
        }
        let scale = self.press.value();
        let origin = ctx.origin();
        let size = ctx.size();
        let pivot = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        scene.push_transform(scale_about(pivot, scale));

        if fill != Color::TRANSPARENT {
            scene.fill_rounded_rect(origin, size, radius, fill);
        }
        if let Some(border) = paint.border {
            // Inset by half the stroke width so the hairline paints fully
            // inside the button's own bounds (a stroke is centered on its
            // path) — mirrors `material::card`'s outlined-variant precedent.
            let half = BORDER_WIDTH / 2.0;
            let rr = RoundedRect::new(
                half,
                half,
                size.width - half,
                size.height - half,
                (radius - half).max(0.0),
            );
            let path = rr.to_path(BORDER_TOLERANCE);
            scene.stroke_path(origin, &path, BORDER_WIDTH, &Brush::Solid(border));
        }

        if self.loading {
            // `reduce_motion` freezes the spinner wherever it currently sits
            // and stops requesting frames — the same skip-animation shape
            // `material::loading_indicator`'s morph loop uses (and
            // `glyph::skeleton`'s shimmer).
            if !reduce_motion {
                self.spinner.advance(ctx.frame_time());
                // The loading-spinner is a perpetual decorative loop — its exact
                // cadence is imperceptible, so the mobile frame gate may pace it
                // (task 08).
                ctx.request_frame_paced();
            }
            self.paint_spinner(ctx, scene, ink);
        } else {
            self.label.paint_child(ctx, scene);
        }

        scene.pop_transform();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Loading suppresses on_press and every other interaction (disabled
        // semantics — see `Widget::semantics` and the module docs).
        if self.loading {
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                self.pressed = true;
                self.captured = true;
                self.press.set_pressed(true);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                // Only a press we armed on `Down` tracks the cursor; a hover
                // `Move` (no prior press) is not ours.
                if !self.captured {
                    return EventResult::Ignored;
                }
                // Visual only: track whether the cursor is still over the button.
                self.pressed = inside(p.position, ctx.size());
                self.press.set_pressed(self.pressed);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // Fire on up-inside only (masonry semantics).
                if inside(p.position, ctx.size()) {
                    (self.on_press)(ctx);
                }
                self.pressed = false;
                self.captured = false;
                self.press.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                self.press.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A button is a single a11y node (Role::Button) labelled by its text; it
        // does not expose its inner label as a separate child node. It advertises
        // the Click action it fires on release — unless loading, which reports
        // disabled semantics instead (task 22).
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label_text.as_str());
            if self.loading {
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
    use std::any::Any;

    #[derive(Default)]
    struct Counter {
        presses: u32,
    }

    /// Build a button widget over `Counter` state, with a known 100x40 size for
    /// the inside/outside geometry (set directly, avoiding a text-context layout).
    fn widget() -> ButtonWidget {
        let view = button::<Counter, _>("go", |s: &mut Counter| s.presses += 1);
        let mut counter = 0u64;
        View::<Counter>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut ButtonWidget, state: &mut Counter, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 40.0));
        w.event(&mut ctx, event);
    }

    #[test]
    fn down_then_up_inside_fires_once() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 12.0, 12.0));
        assert_eq!(state.presses, 1);
        assert!(!w.pressed);
    }

    #[test]
    fn down_inside_move_out_up_outside_does_not_fire() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 200.0, 10.0));
        assert!(!w.pressed, "moving out clears the pressed visual");
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 200.0, 10.0));
        assert_eq!(state.presses, 0, "up outside must not fire");
    }

    #[test]
    fn cancel_clears_pressed_without_firing() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 10.0, 10.0));
        assert!(!w.pressed);
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn hover_move_without_down_is_ignored_noop() {
        let mut w = widget();
        let mut state = Counter::default();
        // A cursor drifting over the button with no prior press must not latch
        // pressed, fire, or request a redraw.
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 40.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 20.0, 20.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.pressed, "hover must not press");
        assert!(!ctx.needs_redraw(), "hover must not request a redraw");
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn up_without_down_does_not_fire() {
        let mut w = widget();
        let mut state = Counter::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 40.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Up, 20.0, 20.0));
        assert!(matches!(result, EventResult::Ignored));
        assert_eq!(state.presses, 0, "an unarmed Up must never fire");
    }

    #[test]
    fn cancel_clears_armed_state() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 10.0, 10.0));
        assert!(!w.captured, "Cancel disarms the press");
        // A subsequent hover Move must not re-press or fire.
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 40.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 12.0, 12.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.pressed);
    }

    /// A recording scene that captures each rounded rect's `(radius, color)`
    /// plus stroke calls' `(width, color)` and push_transform/pop_transform
    /// counts — extended from the original `RRectRecorder` (task 22) to cover
    /// the new border/press-scale paint paths.
    #[derive(Default)]
    struct RRectRecorder {
        rrects: Vec<(f64, Color)>,
        strokes: Vec<(f64, Color)>,
        transforms: Vec<Affine>,
        transform_pops: u32,
    }

    impl PaintScene for RRectRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, radius: f64, color: Color) {
            self.rrects.push((radius, color));
        }
        fn stroke_path(
            &mut self,
            _origin: Point,
            _path: &kurbo::BezPath,
            width: f64,
            brush: &Brush,
        ) {
            if let Brush::Solid(color) = brush {
                self.strokes.push((width, *color));
            }
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn pop_transform(&mut self) {
            self.transform_pops += 1;
        }
    }

    fn paint_bg(w: &mut ButtonWidget, theme: Option<&frust_theme::Theme>) -> (f64, Color) {
        let mut rec = RRectRecorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)).with_theme(t),
            None => PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)),
        };
        w.paint(&mut ctx, &mut rec);
        *rec.rrects.first().expect("button paints its background")
    }

    #[test]
    fn unthemed_paint_uses_fallback_constants() {
        // Parity: no theme → exactly today's fill and radius.
        let mut w = widget();
        assert_eq!(paint_bg(&mut w, None), (RADIUS, FILL));
        // Pressed uses the darker constant, unchanged.
        w.pressed = true;
        assert_eq!(paint_bg(&mut w, None), (RADIUS, FILL_PRESSED));
    }

    #[test]
    fn themed_paint_resolves_primary_and_shape_small() {
        let theme = frust_theme::Theme::m3_baseline();
        let mut w = widget();
        let (radius, color) = paint_bg(&mut w, Some(&theme));
        assert_eq!(color, theme.scheme().primary, "resting fill is primary");
        assert_eq!(radius, theme.shape.small, "radius is shape.small (8dp)");
        // Pressed fill is a darkened primary (not the unthemed constant).
        w.pressed = true;
        let (_, pressed) = paint_bg(&mut w, Some(&theme));
        assert_eq!(
            pressed,
            pressed_overlay(theme.scheme().primary, PRESSED_DARKEN)
        );
    }

    #[test]
    fn move_back_inside_then_up_fires() {
        // out then back in: up-inside fires (masonry re-hover behaviour).
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 200.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 20.0, 10.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 20.0, 10.0));
        assert_eq!(state.presses, 1);
    }

    // --- task 22: style variants, small/loading, press-scale --------------

    fn styled_widget(style: ButtonStyle) -> ButtonWidget {
        let view = button::<Counter, _>("go", |s: &mut Counter| s.presses += 1).style(style);
        let mut counter = 0u64;
        View::<Counter>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn default_style_is_primary_and_matches_pre_task22_rendering() {
        // Acceptance criterion 1: default-style rendering is byte-identical to
        // today's under both no theme and M3 — proven against the exact
        // pre-existing assertions above (same constants, same theme roles),
        // plus an explicit `ButtonStyle::default()` identity check here.
        assert_eq!(ButtonStyle::default(), ButtonStyle::Primary);
        let mut w = widget();
        assert_eq!(w.style, ButtonStyle::Primary);
        assert_eq!(paint_bg(&mut w, None), (RADIUS, FILL));
        let theme = frust_theme::Theme::m3_baseline();
        let mut w2 = widget();
        assert_eq!(paint_bg(&mut w2, Some(&theme)).1, theme.scheme().primary);
    }

    #[test]
    fn secondary_style_paints_raised_surface_and_outline_border() {
        let theme = frust_theme::Theme::m3_baseline();
        let mut w = styled_widget(ButtonStyle::Secondary);
        let mut rec = RRectRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.rrects[0].1, theme.scheme().surface_container_high);
        assert_eq!(rec.strokes[0].1, theme.scheme().outline);
    }

    #[test]
    fn ghost_style_is_transparent_at_rest_and_washes_on_press() {
        let theme = frust_theme::Theme::m3_baseline();
        let mut w = styled_widget(ButtonStyle::Ghost);
        let mut rec = RRectRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);
        assert!(
            rec.rrects.is_empty(),
            "a transparent resting fill paints no rect"
        );
        w.pressed = true;
        let mut rec2 = RRectRecorder::default();
        let mut ctx2 = PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)).with_theme(&theme);
        w.paint(&mut ctx2, &mut rec2);
        assert_eq!(
            rec2.rrects[0].1,
            with_alpha(theme.scheme().on_surface, PRESSED_OPACITY)
        );
    }

    #[test]
    fn danger_style_paints_error_border_and_error_faint_pressed_wash() {
        let theme = frust_theme::Theme::m3_baseline();
        let mut w = styled_widget(ButtonStyle::Danger);
        let mut rec = RRectRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);
        assert!(rec.rrects.is_empty(), "Danger is transparent at rest");
        assert_eq!(rec.strokes[0].1, theme.scheme().error);
        w.pressed = true;
        let mut rec2 = RRectRecorder::default();
        let mut ctx2 = PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)).with_theme(&theme);
        w.paint(&mut ctx2, &mut rec2);
        assert_eq!(rec2.rrects[0].1, theme.scheme().error_container);
    }

    #[test]
    fn per_style_fill_border_label_hold_under_glyph_dark_light_and_m3() {
        // Acceptance criterion 2: the same style -> role mapping (task 22's
        // whole point, per the module docs' "uniform across languages") reads
        // straight off `ColorScheme` fields rather than hardcoding a per-
        // baseline table, so this asserts it against three concrete baselines
        // rather than just M3 (already covered field-by-field by the
        // `secondary`/`ghost`/`danger`_style_* tests above).
        let baselines = [
            frust_theme::Theme::m3_baseline(),
            frust_theme::Theme::glyph_baseline(), // dark (the canonical brightness)
            frust_theme::Theme::glyph_baseline().with_brightness(frust_theme::Brightness::Light),
        ];
        for theme in &baselines {
            let scheme = theme.scheme();

            // Secondary: raised surface + outline border, on_surface label role.
            let mut secondary = styled_widget(ButtonStyle::Secondary);
            assert_eq!(
                ButtonStyle::Secondary.label_role(),
                ThemeTextColor::OnSurface
            );
            let mut rec = RRectRecorder::default();
            let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)).with_theme(theme);
            secondary.paint(&mut ctx, &mut rec);
            assert_eq!(rec.rrects[0].1, scheme.surface_container_high);
            assert_eq!(rec.strokes[0].1, scheme.outline);

            // Ghost: transparent at rest, on_surface_variant (muted) label role.
            assert_eq!(
                ButtonStyle::Ghost.label_role(),
                ThemeTextColor::OnSurfaceVariant
            );
            let mut ghost = styled_widget(ButtonStyle::Ghost);
            let mut rec_g = RRectRecorder::default();
            let mut ctx_g = PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)).with_theme(theme);
            ghost.paint(&mut ctx_g, &mut rec_g);
            assert!(rec_g.rrects.is_empty());

            // Danger: error border + error label role, error_container pressed wash.
            assert_eq!(ButtonStyle::Danger.label_role(), ThemeTextColor::Error);
            let mut danger = styled_widget(ButtonStyle::Danger);
            danger.pressed = true;
            let mut rec_d = RRectRecorder::default();
            let mut ctx_d = PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)).with_theme(theme);
            danger.paint(&mut ctx_d, &mut rec_d);
            assert_eq!(rec_d.rrects[0].1, scheme.error_container);
        }
    }

    #[test]
    fn icon_style_layout_is_square() {
        let mut w = styled_widget(ButtonStyle::Icon);
        let mut text_ctx = frust_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx);
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert_eq!(size.width, size.height, "Icon style must be square");
    }

    #[test]
    fn small_uses_reduced_padding() {
        let normal_view = button::<Counter, _>("go", |_: &mut Counter| {});
        let small_view = button::<Counter, _>("go", |_: &mut Counter| {}).small();
        let mut counter = 0u64;
        let mut normal_w = View::<Counter>::build(&normal_view, &mut BuildCtx::new(&mut counter));
        let mut small_w = View::<Counter>::build(&small_view, &mut BuildCtx::new(&mut counter));

        let mut text_ctx = frust_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx);
        let normal_size =
            normal_w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        let small_size = small_w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert!(
            small_size.width < normal_size.width && small_size.height < normal_size.height,
            "small() must produce a smaller laid-out box: small={small_size:?} normal={normal_size:?}"
        );
    }

    #[test]
    fn loading_suppresses_on_press_and_reports_disabled() {
        let view = button::<Counter, _>("go", |s: &mut Counter| s.presses += 1).loading(true);
        let mut counter = 0u64;
        let mut w = View::<Counter>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.presses, 0, "loading must suppress on_press");
    }

    /// `reduce_motion` freezes the loading spinner wherever it currently sits
    /// and stops requesting frames — the same skip-animation shape
    /// `material::loading_indicator`'s morph loop uses (catalog-animation-
    /// performance bug task 10: without this, an always-`.loading(true)`
    /// button — e.g. a disabled-look demo — spins forever regardless of the
    /// header animations-off toggle forcing `Theme.motion.reduce_motion`).
    /// The spinner requests frames via the paced (CosmeticLoop) class, letting
    /// the mobile frame gate throttle it to the theme's `cosmetic_loop_rate`
    /// (task f2, following task 08's pattern).
    #[test]
    fn loading_spinner_freezes_and_stops_requesting_frames_under_reduce_motion() {
        let mut w = widget();
        w.loading = true;

        let mut theme = frust_theme::Theme::m3_baseline();
        theme.motion.reduce_motion = false;
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)).with_theme(&theme);
        let mut scene = RRectRecorder::default();
        w.paint(&mut ctx, &mut scene);
        assert!(
            ctx.needs_frame(),
            "with reduce_motion off, the loading spinner must keep requesting frames"
        );
        assert!(
            ctx.needs_frame_paced_only(),
            "the loading spinner is a CosmeticLoop request — the frame gate must be able to pace it"
        );

        theme.motion.reduce_motion = true;
        let mut ctx2 = PaintCtx::new(Point::ZERO, Size::new(100.0, 40.0)).with_theme(&theme);
        let mut scene2 = RRectRecorder::default();
        w.paint(&mut ctx2, &mut scene2);
        assert!(
            !ctx2.needs_frame(),
            "with reduce_motion on, the loading spinner must stop requesting frames"
        );
    }

    #[test]
    fn press_scale_timeline_down_mid_up_cancel() {
        // Down -> mid-anim scale < 1.0 -> Up restores; a fresh Down -> Cancel
        // also restores without firing (acceptance criterion 3). Every
        // `dispatch` below drives the real event path (which itself calls
        // `PressAnim::set_pressed`); the retained driver is then advanced
        // directly at chosen frame times — mirrors `motion::animated`'s test
        // precedent, since `PaintCtx::set_frame_time` is crate-private, so a
        // widget test cannot drive `paint` at an arbitrary frame time.
        let mut w = widget();
        let mut state = Counter::default();

        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        let down_timing = resolve_press_timing(None, true);
        assert!(
            w.press.advance(FrameTime::from_nanos(0), down_timing),
            "the seed frame must still report animating"
        );
        // Advance partway into the 100ms fallback duration.
        let mid = FrameTime::from_nanos(50_000_000);
        w.press.advance(mid, down_timing);
        assert!(
            w.press.value() < 1.0,
            "mid-press the scale must read below 1.0, got {}",
            w.press.value()
        );

        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.presses, 1);
        let up_timing = resolve_press_timing(None, false);
        // Settle the release animation (bounded loop, mirrors `motion::animated`'s
        // settle-and-assert precedent).
        let mut t = mid.as_nanos();
        let mut still_animating = true;
        for _ in 0..1000 {
            still_animating = w.press.advance(FrameTime::from_nanos(t), up_timing);
            if !still_animating {
                break;
            }
            t += 1_000_000; // +1ms
        }
        assert!(
            !still_animating,
            "the release animation must settle within a bounded number of steps"
        );
        assert!(
            (w.press.value() - REST_SCALE).abs() < 1e-6,
            "Up must restore to REST_SCALE, got {}",
            w.press.value()
        );

        // Cancel path: press again, then Cancel restores without firing again.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 10.0, 10.0));
        assert_eq!(state.presses, 1, "Cancel must not fire");
        assert_eq!(w.press.target, REST_SCALE, "Cancel retargets to rest");
    }
}
