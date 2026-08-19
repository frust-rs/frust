// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// whose `lib/components/buttons/` tree is itself vendored from m3e_buttons
// (MIT, © 2026 Mudit Purohit) — `styles/m3e_button_theme.dart` (tables),
// `components/m3e_button_style.dart` (per-state resolvers),
// `components/m3e_button_content.dart` (shape + assembly),
// `components/m3e_button_state.dart` (icon layout),
// `components/m3e_focus_ring.dart` (focus ring).
// Porting decision: the Dart widget hands its surface to Flutter's own
// FilledButton/OutlinedButton/…; this file paints it directly.

//! [`super::ButtonWidget`]'s retained core: the per-size metric tables, the
//! per-variant × per-state color resolvers, the icon+label content layout, and
//! the layout/paint/event passes that drive them. The public shape of all this
//! — the tables, the mapping decisions, the seams — is documented on [the
//! parent module](super); this file carries each citation next to the value it
//! pins.

use frust::Theme;
use frust::authoring::text::{
    FontWeight, LineHeight, TextContext, TextLayout, TextOverflow, TextStyle,
};
use frust::authoring::{
    Action, BoxConstraints, CursorIcon, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, Role, SemanticsCtx, Widget,
};
use kurbo::{Point, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use super::motion::ContentPadding;
use super::{ButtonShape, ButtonSize, ButtonVariant, ButtonWidget, IconAlignment};
use crate::interaction::{
    DISABLED_CONTAINER_OPACITY, DISABLED_CONTENT_OPACITY, HapticSignal, InteractionState,
    MaterialHaptics,
};
use crate::press::presses;

// ---- Metric tables (m3e_button_theme.dart) ---------------------------------

/// The floor every button's width is held above, in logical px
/// (`M3EButtonTheme.minWidthFloor`, `m3e_button_theme.dart:16`).
const MIN_WIDTH_FLOOR: f64 = 48.0;

/// Stroke width of an [`ButtonVariant::Outlined`] button's hairline, in
/// logical px — Flutter's own `BorderSide` default, which
/// `m3e_button_style.dart:135` constructs with a color only.
const OUTLINE_WIDTH: f64 = 1.0;

/// Gap between the container edge and the focus ring, in logical px
/// (`M3EButtonConstants.kFocusRingGap`, `m3e_button_constants.dart:21`).
const FOCUS_RING_GAP: f64 = 2.0;
/// Focus-ring stroke width, in logical px
/// (`M3EButtonConstants.kFocusRingWidth`, `m3e_button_constants.dart:24`).
const FOCUS_RING_WIDTH: f64 = 2.0;

/// Flattening tolerance for the `kurbo` rounded-rect paths this module strokes
/// (matches [`mod@crate::card`]'s `STROKE_TOLERANCE`).
const PATH_TOLERANCE: f64 = 0.1;

/// The ink every label run is *shaped* with; never painted — the run is
/// re-brushed with its resolved per-variant/per-state color at paint time, and
/// holding the shaping color constant keeps the shape cache from missing on a
/// recolor (the contract [`mod@crate::text_field`]'s own runs document).
const SHAPING_INK: Color = Color::BLACK;

/// One row of `m3e_button_theme.dart:76`'s `_measurementsTable`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SizeMetrics {
    /// Fixed container height, logical px.
    pub(super) height: f64,
    /// Symmetric horizontal content padding, logical px.
    pub(super) h_padding: f64,
    /// Icon side length, logical px.
    pub(super) icon_size: f64,
    /// Gap between icon and label, logical px.
    pub(super) icon_gap: f64,
}

/// One row of the M3 type scale, as `(size, line_height, letter_spacing,
/// weight)` — the unthemed fallback for a label role.
type TypeToken = (f32, f32, f32, FontWeight);

const LABEL_SMALL: TypeToken = (11.0, 16.0, 0.5, FontWeight::MEDIUM);
const LABEL_MEDIUM: TypeToken = (12.0, 16.0, 0.5, FontWeight::MEDIUM);
const LABEL_LARGE: TypeToken = (14.0, 20.0, 0.1, FontWeight::MEDIUM);
const TITLE_MEDIUM: TypeToken = (16.0, 24.0, 0.15, FontWeight::MEDIUM);
const TITLE_LARGE: TypeToken = (22.0, 28.0, 0.0, FontWeight::REGULAR);

impl ButtonSize {
    /// The size's row of `m3e_button_theme.dart:76`'s `_measurementsTable`.
    pub(super) const fn metrics(self) -> SizeMetrics {
        match self {
            // m3e_button_theme.dart:77
            ButtonSize::Xs => SizeMetrics {
                height: 32.0,
                h_padding: 16.0,
                icon_size: 20.0,
                icon_gap: 8.0,
            },
            // m3e_button_theme.dart:83
            ButtonSize::Sm => SizeMetrics {
                height: 40.0,
                h_padding: 16.0,
                icon_size: 20.0,
                icon_gap: 8.0,
            },
            // m3e_button_theme.dart:89
            ButtonSize::Md => SizeMetrics {
                height: 56.0,
                h_padding: 24.0,
                icon_size: 24.0,
                icon_gap: 8.0,
            },
            // m3e_button_theme.dart:95
            ButtonSize::Lg => SizeMetrics {
                height: 96.0,
                h_padding: 48.0,
                icon_size: 32.0,
                icon_gap: 12.0,
            },
            // m3e_button_theme.dart:101
            ButtonSize::Xl => SizeMetrics {
                height: 136.0,
                h_padding: 64.0,
                icon_size: 40.0,
                icon_gap: 16.0,
            },
        }
    }

    /// The [`ButtonShape::Square`] corner radius
    /// (`m3e_button_theme.dart:44`'s `_squareRadiusTable`: 12/12/16/28/28).
    /// Resolved from the theme's shape scale, whose `medium`/`large`/
    /// `extra_large` tokens coincide with that table exactly; the literals are
    /// the unthemed fallback.
    pub(super) fn square_radius(self, theme: Option<&Theme>) -> f64 {
        match theme {
            Some(theme) => match self {
                ButtonSize::Xs | ButtonSize::Sm => theme.shape.medium,
                ButtonSize::Md => theme.shape.large,
                ButtonSize::Lg | ButtonSize::Xl => theme.shape.extra_large,
            },
            None => match self {
                ButtonSize::Xs | ButtonSize::Sm => 12.0,
                ButtonSize::Md => 16.0,
                ButtonSize::Lg | ButtonSize::Xl => 28.0,
            },
        }
    }

    /// The pressed corner radius (`m3e_button_theme.dart:52`'s
    /// `_pressedRadiusTable`: 8/8/12/16/16) — one shape-scale rung squarer
    /// than [`Self::square_radius`], and likewise theme-resolved.
    pub(super) fn pressed_radius(self, theme: Option<&Theme>) -> f64 {
        match theme {
            Some(theme) => match self {
                ButtonSize::Xs | ButtonSize::Sm => theme.shape.small,
                ButtonSize::Md => theme.shape.medium,
                ButtonSize::Lg | ButtonSize::Xl => theme.shape.large,
            },
            None => match self {
                ButtonSize::Xs | ButtonSize::Sm => 8.0,
                ButtonSize::Md => 12.0,
                ButtonSize::Lg | ButtonSize::Xl => 16.0,
            },
        }
    }

    /// The hovered corner radius (`m3e_button_theme.dart:60`'s
    /// `_hoveredRadiusTable`: 10/10/14/22/22). Each entry is the midpoint of
    /// its size's square and pressed radii, which lands *between* shape-scale
    /// tokens — so unlike the other two tables this one has nothing to resolve
    /// from and stays literal.
    pub(super) const fn hovered_radius(self) -> f64 {
        match self {
            ButtonSize::Xs | ButtonSize::Sm => 10.0,
            ButtonSize::Md => 14.0,
            ButtonSize::Lg | ButtonSize::Xl => 22.0,
        }
    }

    /// The label's type role (`m3e_base_button_state.dart:169`): `labelSmall`
    /// / `labelMedium` / `labelLarge` / `titleMedium` / `titleLarge`. Themed
    /// from [`frust::Theme`]'s type scale; unthemed from the M3 token literals
    /// above. The returned style always carries [`SHAPING_INK`].
    pub(super) fn label_style(self, theme: Option<&Theme>) -> TextStyle {
        let mut style = match theme {
            Some(theme) => match self {
                ButtonSize::Xs => theme.type_scale.label_small.clone(),
                ButtonSize::Sm => theme.type_scale.label_medium.clone(),
                ButtonSize::Md => theme.type_scale.label_large.clone(),
                ButtonSize::Lg => theme.type_scale.title_medium.clone(),
                ButtonSize::Xl => theme.type_scale.title_large.clone(),
            },
            None => {
                let (size, line_height, letter_spacing, weight) = match self {
                    ButtonSize::Xs => LABEL_SMALL,
                    ButtonSize::Sm => LABEL_MEDIUM,
                    ButtonSize::Md => LABEL_LARGE,
                    ButtonSize::Lg => TITLE_MEDIUM,
                    ButtonSize::Xl => TITLE_LARGE,
                };
                let mut style = TextStyle::new(size, SHAPING_INK);
                style.line_height = LineHeight::Absolute(line_height);
                style.letter_spacing = letter_spacing;
                style.weight = weight;
                style
            }
        };
        style.color = SHAPING_INK;
        style
    }
}

// ---- Colors (m3e_button_theme.dart + m3e_button_style.dart) ----------------

/// Unthemed-fallback `primary` (a theme resolves `colors.primary`).
const PRIMARY: Color = Color::from_rgb8(0x67, 0x50, 0xA4);
/// Unthemed-fallback `on_primary`.
const ON_PRIMARY: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
/// Unthemed-fallback `secondary_container`.
const SECONDARY_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback `on_secondary_container`.
const ON_SECONDARY_CONTAINER: Color = Color::from_rgb8(0x1D, 0x19, 0x2B);
/// Unthemed-fallback `surface_container_low`.
const SURFACE_CONTAINER_LOW: Color = Color::from_rgb8(0xF7, 0xF2, 0xFA);
/// Unthemed-fallback `outline`.
const OUTLINE: Color = Color::from_rgb8(0x79, 0x74, 0x7E);
/// Unthemed-fallback `on_surface` — the disabled roles' base color.
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed-fallback shadow color (opaque black at
/// `crate::tokens::elevation()`'s `0.3` alpha — identical at every level).
const FALLBACK_SHADOW_COLOR: Color = Color::new([0.0, 0.0, 0.0, 0.3]);

/// The seven color roles a button reads, resolved once per pass from the live
/// theme or from this module's unthemed fallbacks.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Roles {
    primary: Color,
    on_primary: Color,
    secondary_container: Color,
    on_secondary_container: Color,
    surface_container_low: Color,
    outline: Color,
    on_surface: Color,
}

impl Roles {
    fn resolve(theme: Option<&Theme>) -> Self {
        match theme {
            Some(theme) => {
                let s = theme.scheme();
                Self {
                    primary: s.primary,
                    on_primary: s.on_primary,
                    secondary_container: s.secondary_container,
                    on_secondary_container: s.on_secondary_container,
                    surface_container_low: s.surface_container_low,
                    outline: s.outline,
                    on_surface: s.on_surface,
                }
            }
            None => Self {
                primary: PRIMARY,
                on_primary: ON_PRIMARY,
                secondary_container: SECONDARY_CONTAINER,
                on_secondary_container: ON_SECONDARY_CONTAINER,
                surface_container_low: SURFACE_CONTAINER_LOW,
                outline: OUTLINE,
                on_surface: ON_SURFACE,
            },
        }
    }
}

/// The resolved inks for one paint pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ButtonColors {
    /// Container fill (fully transparent for outlined/text).
    pub(super) container: Color,
    /// Label + icon ink, and the state layer's tint.
    pub(super) content: Color,
    /// The hairline, when the variant draws one.
    pub(super) outline: Option<Color>,
}

/// `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Whether `variant` paints no container at all
/// (`m3e_button_style.dart:85`'s `isTransparent`).
fn is_transparent(variant: ButtonVariant) -> bool {
    matches!(variant, ButtonVariant::Outlined | ButtonVariant::Text)
}

/// Resolve the container/content/outline inks for `variant`.
///
/// Enabled follows `m3e_button_theme.dart:111`/`:127`/`:142`; disabled follows
/// `m3e_button_style.dart:69`/`:89`/`:128` — content at
/// [`DISABLED_CONTENT_OPACITY`] (38%) and an opaque container or an outline at
/// [`DISABLED_CONTAINER_OPACITY`] (12%), both over `on_surface`, while a
/// transparent container stays transparent.
pub(super) fn resolve_colors(
    theme: Option<&Theme>,
    variant: ButtonVariant,
    enabled: bool,
) -> ButtonColors {
    let roles = Roles::resolve(theme);

    if !enabled {
        return ButtonColors {
            container: if is_transparent(variant) {
                Color::TRANSPARENT
            } else {
                with_alpha(roles.on_surface, DISABLED_CONTAINER_OPACITY)
            },
            content: with_alpha(roles.on_surface, DISABLED_CONTENT_OPACITY),
            outline: (variant == ButtonVariant::Outlined)
                .then(|| with_alpha(roles.on_surface, DISABLED_CONTAINER_OPACITY)),
        };
    }

    ButtonColors {
        container: match variant {
            ButtonVariant::Filled => roles.primary,
            ButtonVariant::Tonal => roles.secondary_container,
            ButtonVariant::Elevated => roles.surface_container_low,
            ButtonVariant::Outlined | ButtonVariant::Text => Color::TRANSPARENT,
        },
        content: match variant {
            ButtonVariant::Filled => roles.on_primary,
            ButtonVariant::Tonal => roles.on_secondary_container,
            ButtonVariant::Elevated | ButtonVariant::Outlined | ButtonVariant::Text => {
                roles.primary
            }
        },
        outline: (variant == ButtonVariant::Outlined).then_some(roles.outline),
    }
}

/// The container elevation in dp for `variant` in the current state
/// (`m3e_button_theme.dart:150`): an elevated button rests at 1, lifts to 3 on
/// hover and flattens to 0 on press; filled and tonal lift to 1 on hover only;
/// outlined, text and *every* disabled button stay flat.
pub(super) fn elevation_dp(
    variant: ButtonVariant,
    enabled: bool,
    pressed: bool,
    hovered: bool,
) -> f64 {
    if !enabled {
        return 0.0;
    }
    match variant {
        ButtonVariant::Elevated => {
            if pressed {
                0.0
            } else if hovered {
                3.0
            } else {
                1.0
            }
        }
        ButtonVariant::Filled | ButtonVariant::Tonal => {
            if !pressed && hovered {
                1.0
            } else {
                0.0
            }
        }
        ButtonVariant::Outlined | ButtonVariant::Text => 0.0,
    }
}

/// The `(blur_std_dev, y_offset, color)` shadow for an elevation of `dp`, or
/// `None` for a flat button.
///
/// The reference's table only ever produces 1dp or 3dp, which are exactly M3
/// elevation levels 1 and 2 — so a theme resolves through
/// `theme.elevation.level1`/`level2`'s own `ShadowSpec`. Unthemed, the same
/// math `crate::tokens::elevation()` uses (`y = dp / 2 + 1`, `blur = dp`,
/// alpha 0.3) is applied directly.
pub(super) fn resolve_shadow(theme: Option<&Theme>, dp: f64) -> Option<(f64, f64, Color)> {
    if dp <= 0.0 {
        return None;
    }
    match theme {
        Some(theme) => {
            let level = if dp >= theme.elevation.level2.dp {
                theme.elevation.level2
            } else {
                theme.elevation.level1
            };
            let shadow = level.shadow(theme.brightness);
            let color = with_alpha(theme.scheme().shadow, shadow.color_alpha);
            Some((shadow.blur_std_dev, shadow.y_offset, color))
        }
        None => Some((dp, dp / 2.0 + 1.0, FALLBACK_SHADOW_COLOR)),
    }
}

// ---- Seams (filled in by the follow-up gradient/overflow tasks) ------------

/// The morphing surface a [`ButtonDecoration`] paints against: window-space
/// geometry, the live corner radius, and the inks + interaction state the core
/// resolved for this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonSurface {
    /// The container's window-space origin.
    pub origin: Point,
    /// The container's size.
    pub size: Size,
    /// This frame's animating corner radius.
    pub radius: f64,
    /// The resolved container fill (transparent for outlined/text).
    pub container: Color,
    /// The resolved label/icon ink.
    pub content: Color,
    /// The resolved hairline ink, when the variant draws one.
    pub outline: Option<Color>,
    /// The hairline's stroke width, logical px.
    pub outline_width: f64,
    /// Whether the button is enabled.
    pub enabled: bool,
    /// The live interaction state — what a per-state gradient resolves
    /// against, the way the reference's `WidgetStateProperty<Gradient?>` does.
    pub state: InteractionState,
}

/// Whether a [`ButtonDecoration`] hook took over a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationOutcome {
    /// The hook painted this layer; the core skips its own.
    Painted,
    /// The hook painted nothing; the core paints its own.
    Skipped,
}

/// The paint-time decoration seam — where the reference's
/// `m3eGradientSurfaceBuilder` fill/overlay/outline layers attach.
///
/// Every method defaults to [`DecorationOutcome::Skipped`], so an
/// implementation overrides only the layers it draws and a button with no
/// decoration behaves exactly as if this trait did not exist. The hooks fire
/// in paint order — fill, overlay, outline — each with the same
/// [`ButtonSurface`], whose `radius` is the morph's live value rather than a
/// resting token (the reference threads `animatedRadius` into its own builders
/// for the same reason: a gradient must clip to the shape actually painted).
pub trait ButtonDecoration {
    /// Paint the container fill. [`DecorationOutcome::Painted`] suppresses the
    /// core's own solid fill.
    fn paint_fill(
        &self,
        _surface: &ButtonSurface,
        _scene: &mut dyn PaintScene,
    ) -> DecorationOutcome {
        DecorationOutcome::Skipped
    }

    /// Paint an overlay above the fill. [`DecorationOutcome::Painted`]
    /// suppresses the core's own interaction state layer — mirroring
    /// `m3e_button_style.dart:47`, where a gradient overlay forces
    /// `overlayColor` to transparent so the two never stack.
    fn paint_overlay(
        &self,
        _surface: &ButtonSurface,
        _scene: &mut dyn PaintScene,
    ) -> DecorationOutcome {
        DecorationOutcome::Skipped
    }

    /// Paint the outline. [`DecorationOutcome::Painted`] suppresses the core's
    /// own hairline.
    fn paint_outline(
        &self,
        _surface: &ButtonSurface,
        _scene: &mut dyn PaintScene,
    ) -> DecorationOutcome {
        DecorationOutcome::Skipped
    }
}

/// What one layout pass measured of a button's content — the input an overflow
/// strategy decides on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentMetrics {
    /// The label's width if nothing constrained it, logical px.
    pub natural_label_width: f64,
    /// The width actually left for the label after padding, icon and gap —
    /// [`f64::INFINITY`] when the incoming constraint is unbounded.
    pub available_label_width: f64,
    /// The width the label was finally laid out at (ellipsized when it had to
    /// fit), logical px.
    pub painted_label_width: f64,
    /// Icon + gap + label, logical px.
    pub content_width: f64,
}

impl ContentMetrics {
    /// The pre-layout value: nothing measured yet, nothing constrained.
    pub(super) fn empty() -> Self {
        Self {
            natural_label_width: 0.0,
            available_label_width: f64::INFINITY,
            painted_label_width: 0.0,
            content_width: 0.0,
        }
    }

    /// Whether the label did not fit the width available to it — the question
    /// the reference's `M3EOverflowStrategy` family exists to answer.
    pub fn overflows(&self) -> bool {
        self.natural_label_width > self.available_label_width
    }
}

/// The layout-time content-measurement seam: handed the [`ContentMetrics`] of
/// every layout pass, including whether the label overflowed.
///
/// Measurement only — deliberately no return value, so an observer cannot
/// steer layout from inside it. The strategies that *react* to an overflow
/// (scroll, popup, bottom sheet) are a follow-up; this is the signal they
/// subscribe to.
pub trait OverflowObserver {
    /// Called at the end of each of the button's layout passes.
    fn measured(&self, metrics: ContentMetrics);
}

// ---- The label run ---------------------------------------------------------

/// The button's own lazily-shaped label run, re-brushed at paint time.
///
/// A catalog-owned run rather than a nested `frust::text` child for two
/// reasons: the ink is per-variant *and* per-state (no themed color role could
/// express it), and the overflow seam needs the natural width, which only the
/// shaper knows. Mirrors [`mod@crate::text_field`]'s `Run`.
pub(super) struct LabelRun {
    content: String,
    layout: Option<TextLayout>,
    /// The `(style, max_width)` the cached `layout` was shaped/broken for.
    shaped_for: Option<(TextStyle, Option<f64>)>,
    /// The `(style, width)` of the last natural (unconstrained) measurement.
    natural: Option<(TextStyle, f64)>,
}

impl LabelRun {
    /// A run holding `content`, unshaped until the first measurement.
    pub(super) fn new(content: String) -> Self {
        Self {
            content,
            layout: None,
            shaped_for: None,
            natural: None,
        }
    }

    /// Replace the text, dropping every cached measurement — but only if it
    /// actually changed.
    pub(super) fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.layout = None;
            self.shaped_for = None;
            self.natural = None;
        }
    }

    /// The text this run holds — also the accessible name.
    pub(super) fn content(&self) -> &str {
        &self.content
    }

    /// The run's width with nothing constraining it.
    pub(super) fn natural_width(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> f64 {
        if let Some((cached_style, width)) = &self.natural
            && cached_style == style
        {
            return *width;
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let width = text_ctx.layout(&self.content, style, None).size().width;
        self.natural = Some((style.clone(), width));
        width
    }

    /// Shape (or reuse) the run in `style`, fitted to `max_width` when one is
    /// given, and return its measured size.
    ///
    /// A fitted run is a **single ellipsized line**: `softWrap: false` +
    /// `overflow: ellipsis` is what the reference's label style and icon
    /// layout apply (`m3e_base_button_state.dart:177`,
    /// `m3e_button_state.dart:29`), and a button's height is a fixed size
    /// token — there is no second line for wrapped text to occupy.
    pub(super) fn shape(
        &mut self,
        ctx: &mut LayoutCtx,
        style: &TextStyle,
        max_width: Option<f64>,
    ) -> Size {
        let key = (style.clone(), max_width);
        if let Some(cached) = &self.layout
            && self.shaped_for.as_ref() == Some(&key)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = match max_width {
            Some(width) => text_ctx.layout_bounded(
                &self.content,
                style,
                Some(width as f32),
                Some(1),
                TextOverflow::Ellipsis,
            ),
            None => text_ctx.layout(&self.content, style, None),
        };
        let size = laid.size();
        self.layout = Some(laid);
        self.shaped_for = Some(key);
        size
    }

    /// Paint the run at `origin` in `color`, overriding the [`SHAPING_INK`] it
    /// was shaped with. A run that has never been shaped paints nothing.
    pub(super) fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let Some(layout) = &self.layout else {
            return;
        };
        for mut run in layout.to_scene_runs(origin) {
            run.brush = Brush::Solid(color);
            scene.draw_glyph_run(run);
        }
    }
}

// ---- Geometry helpers ------------------------------------------------------

/// Whether local point `pos` is inside a widget of `size`.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// The rounded rect to *stroke* for a `width`-wide hairline lying fully inside
/// a container of `size` with corner `radius` — a stroke is centered on its
/// path, so both the rect and the radius pull in by half the width (the inset
/// [`mod@crate::card`]'s outlined variant applies too).
fn inset_stroke_rect(size: Size, radius: f64, width: f64) -> RoundedRect {
    let half = width / 2.0;
    RoundedRect::new(
        half,
        half,
        size.width - half,
        size.height - half,
        (radius - half).max(0.0),
    )
}

/// The focus ring's stroke geometry in widget-local coordinates, for a
/// container of `size` painting at corner `radius`.
///
/// `m3e_focus_ring.dart:49` insets the ring by `-(gap + width)` on every side
/// and grows the radius by that same outset; this returns the *stroke* path for
/// that box, pulled in by half the stroke width so the ring's outer edge lands
/// exactly on `-(gap + width)`.
fn focus_ring_rect(size: Size, radius: f64) -> RoundedRect {
    let outset = FOCUS_RING_GAP + FOCUS_RING_WIDTH;
    let half = FOCUS_RING_WIDTH / 2.0;
    RoundedRect::new(
        -outset + half,
        -outset + half,
        size.width + outset - half,
        size.height + outset - half,
        (radius + outset - half).max(0.0),
    )
}

/// Stroke the focus ring for a container of `size` at corner `radius`, in
/// `color`, around window-space `origin`. See [`focus_ring_rect`] and the
/// [parent module](super)'s Focus ring section.
fn paint_focus_ring(
    origin: Point,
    size: Size,
    radius: f64,
    color: Color,
    scene: &mut dyn PaintScene,
) {
    let path = focus_ring_rect(size, radius).to_path(PATH_TOLERANCE);
    scene.stroke_path(origin, &path, FOCUS_RING_WIDTH, &Brush::Solid(color));
}

impl ButtonWidget {
    /// The corner radius the morph is currently aiming at, per
    /// `m3e_button_content.dart:40`'s `_resolveShapes` precedence: an explicit
    /// pressed override beats an explicit radius beats the pressed token while
    /// pressed; an explicit radius beats the hovered token while hovered;
    /// otherwise an explicit radius beats the shape family's own resting
    /// radius (a pill at `height / 2`, or the size's square token). A disabled
    /// button never leaves its resting radius (the reference's
    /// `effectivelyEnabled &&` guards, `m3e_button_content.dart:86`).
    fn target_radius(&self, theme: Option<&Theme>, height: f64) -> f64 {
        let resting = self.corner_radius.unwrap_or(match self.shape {
            ButtonShape::Round => height / 2.0,
            ButtonShape::Square => self.size.square_radius(theme),
        });
        if !self.enabled {
            return resting;
        }
        if self.state.pressed {
            return self
                .pressed_radius
                .or(self.corner_radius)
                .unwrap_or_else(|| self.size.pressed_radius(theme));
        }
        if self.state.hovered {
            return self
                .corner_radius
                .unwrap_or_else(|| self.size.hovered_radius());
        }
        resting
    }

    /// What the last layout pass measured. The [`OverflowObserver`] seam is
    /// handed this same value; a caller holding the widget can read it back
    /// here instead of installing one.
    pub fn content_metrics(&self) -> ContentMetrics {
        self.metrics
    }
}

impl Widget for ButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let metrics = self.size.metrics();
        // Resolve every theme-derived value first: the borrow has to end
        // before anything takes `ctx` mutably (child layout, text shaping).
        let (label_style, resting_radius) = {
            let theme = Theme::from_layout_ctx(ctx);
            (
                self.size.label_style(theme),
                self.target_radius(theme, metrics.height),
            )
        };
        if !self.motion.is_seeded() {
            // The mount frame snaps; only a later target change springs.
            self.motion
                .snap_to(resting_radius, ContentPadding::symmetric(metrics.h_padding));
        }
        let padding = self.motion.padding();

        let icon_size = self.icon.as_mut().map(|pod| {
            pod.layout_child(
                ctx,
                &BoxConstraints::tight(Size::new(metrics.icon_size, metrics.icon_size)),
            )
        });
        let icon_width = icon_size.map_or(0.0, |s| s.width);
        // The gap exists only between an icon and a label
        // (`m3e_button_state.dart:27`).
        let gap = if icon_size.is_some() {
            metrics.icon_gap
        } else {
            0.0
        };

        let available_label_width = if bc.max().width.is_finite() {
            (bc.max().width - padding.left - padding.right - icon_width - gap).max(0.0)
        } else {
            f64::INFINITY
        };

        let natural_label_width = self.label.natural_width(ctx, &label_style);
        let fit_to = (natural_label_width > available_label_width)
            .then_some(available_label_width)
            .filter(|w| w.is_finite());
        let label_size = self.label.shape(ctx, &label_style, fit_to);

        let content_width = icon_width + gap + label_size.width;
        let width = (content_width + padding.left + padding.right).max(MIN_WIDTH_FLOOR);
        let size = bc.constrain(Size::new(width, metrics.height));

        // The content row is centered as a whole (`Row(mainAxisSize: min,
        // mainAxisAlignment: center)` inside a center-aligned button,
        // `m3e_button_state.dart:42` + `m3e_button_style.dart:19`).
        let content_x = ((size.width - content_width) / 2.0).max(0.0);
        let (icon_x, label_x) = match self.icon_alignment {
            IconAlignment::Start => (content_x, content_x + icon_width + gap),
            IconAlignment::End => (content_x + label_size.width + gap, content_x),
        };
        if let Some(pod) = self.icon.as_mut() {
            let icon_height = icon_size.map_or(0.0, |s| s.height);
            pod.set_origin(Point::new(icon_x, (size.height - icon_height) / 2.0));
        }
        self.label_origin = Point::new(label_x, (size.height - label_size.height) / 2.0);

        self.metrics = ContentMetrics {
            natural_label_width,
            available_label_width,
            painted_label_width: label_size.width,
            content_width,
        };
        if let Some(observer) = &self.overflow {
            observer.measured(self.metrics);
        }
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();

        // Authoritative hover/focus reads, self-correcting the latched flags
        // (`docs/CODE_STANDARDS.md`'s Interaction Semantics); both are inert
        // while disabled, mirroring the reference's `effectivelyEnabled`
        // guards (`m3e_button_content.dart:86`).
        self.state.set_hovered(self.enabled && ctx.is_hovered());
        self.state.set_focused(self.enabled && ctx.has_focus());

        // Every theme read happens here, before `ctx` is taken mutably below.
        let (colors, shadow, radius_target, focus_ring_color) = {
            let theme = Theme::from_paint_ctx(ctx);
            let dp = elevation_dp(
                self.variant,
                self.enabled,
                self.state.pressed,
                self.state.hovered,
            );
            (
                resolve_colors(theme, self.variant, self.enabled),
                resolve_shadow(theme, dp),
                self.target_radius(theme, size.height),
                // `m3e_button_theme.dart:146` — the ring takes `primary`.
                theme.map_or(PRIMARY, |theme| theme.scheme().primary),
            )
        };

        let padding = ContentPadding::symmetric(self.size.metrics().h_padding);
        if self.motion.retarget(radius_target, padding) {
            ctx.request_frame();
        }
        if self.motion.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        let radius = self.motion.radius();

        let surface = ButtonSurface {
            origin,
            size,
            radius,
            container: colors.container,
            content: colors.content,
            outline: colors.outline,
            outline_width: OUTLINE_WIDTH,
            enabled: self.enabled,
            state: self.state,
        };

        if let Some((blur, y_offset, shadow_color)) = shadow {
            scene.draw_shadow(
                Point::new(origin.x, origin.y + y_offset),
                size,
                radius,
                blur,
                shadow_color,
            );
        }

        let fill = self
            .decoration
            .as_ref()
            .map_or(DecorationOutcome::Skipped, |d| {
                d.paint_fill(&surface, scene)
            });
        if fill == DecorationOutcome::Skipped && colors.container.components[3] > 0.0 {
            scene.fill_rounded_rect(origin, size, radius, colors.container);
        }

        let overlay = self
            .decoration
            .as_ref()
            .map_or(DecorationOutcome::Skipped, |d| {
                d.paint_overlay(&surface, scene)
            });
        // The M3E state layer: `dragged > pressed > focused > hovered`
        // precedence (`m3e_state_layer.dart`'s `M3EInteractionState.opacity`,
        // ported as `InteractionState::resolve_opacity`). The reference's own
        // button suppresses the *pressed* step because `InkSparkle` draws the
        // press for it (`m3e_button_style.dart:153`); this framework has no
        // ripple engine, so the state layer *is* the press feedback here and
        // the step is kept.
        if overlay == DecorationOutcome::Skipped && self.enabled {
            let opacity = self.state.resolve_opacity();
            if opacity > 0.0 {
                scene.fill_rounded_rect(origin, size, radius, with_alpha(colors.content, opacity));
            }
        }

        let outlined = self
            .decoration
            .as_ref()
            .map_or(DecorationOutcome::Skipped, |d| {
                d.paint_outline(&surface, scene)
            });
        if outlined == DecorationOutcome::Skipped
            && let Some(outline) = colors.outline
        {
            let path = inset_stroke_rect(size, radius, OUTLINE_WIDTH).to_path(PATH_TOLERANCE);
            scene.stroke_path(origin, &path, OUTLINE_WIDTH, &Brush::Solid(outline));
        }

        if let Some(pod) = self.icon.as_mut() {
            pod.paint_child(ctx, scene);
        }
        self.label.paint(
            Point::new(
                origin.x + self.label_origin.x,
                origin.y + self.label_origin.y,
            ),
            colors.content,
            scene,
        );

        // Outside the container, and only while focused — see the parent
        // module's Focus ring section, including why nothing reaches it yet.
        if self.state.focused {
            paint_focus_ring(origin, size, radius, focus_ring_color, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.enabled {
            // A disabled button arms nothing and asks for no cursor —
            // `m3e_button_style.dart:146` resolves the plain arrow, which is
            // what a pass where nobody asks already produces.
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                self.state.set_pressed(true);
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let over = inside(p.position, ctx.size());
                if !self.captured {
                    // The hover pass: claim, latch, and let paint self-correct
                    // (`docs/CODE_STANDARDS.md`'s three-part hover contract).
                    if over {
                        ctx.claim_hover();
                        // `mouseCursor: SystemMouseCursors.click`
                        // (`m3e_buttons.dart:56`), re-asked every move.
                        ctx.set_cursor(CursorIcon::Pointer);
                    }
                    if self.state.set_hovered(over) {
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                }
                ctx.set_cursor(CursorIcon::Pointer);
                if self.state.set_pressed(over) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
                    // The reference fires its haptic inside the same
                    // `onPressed` wrapper, ahead of the app callback
                    // (`m3e_button_content.dart:178`). `None` is defined as a
                    // no-op, so it is elided rather than routed through the
                    // process-global hook.
                    if self.haptic != HapticSignal::None {
                        MaterialHaptics::fire(self.haptic);
                    }
                    (self.on_press)(ctx);
                }
                self.state.set_pressed(false);
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.state.set_pressed(false);
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // One node labelled by the button's own text; the icon child is not
        // forwarded (it is decoration, and the label already names the action)
        // — the same shape `frust_widgets::Button` reports.
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label.content());
            if self.enabled {
                node.add_action(Action::Click);
            } else {
                node.set_disabled();
            }
        });
    }

    frust::authoring::visit_children!(icon);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::button::{ButtonView, button, button_with_icon};
    use frust::FrameTime;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::{BuildCtx, PointerButton, PointerEvent, View};
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;
    use std::cell::RefCell;
    use std::rc::Rc;

    // ---- fixtures ---------------------------------------------------------

    /// Records the rounded rects, stroked paths, shadows and glyph runs a
    /// paint pass emits.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Point, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        runs: Vec<(Point, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
        fn stroke_path(&mut self, o: Point, _path: &kurbo::BezPath, width: f64, brush: &Brush) {
            self.strokes.push((o, width, solid(brush)));
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            self.runs.push((Point::new(t.x, t.y), solid(&run.brush)));
        }
    }

    fn solid(brush: &Brush) -> Color {
        match brush {
            Brush::Solid(c) => *c,
            _ => Color::TRANSPARENT,
        }
    }

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    /// Build a widget from a view over any state type — icon-carrying views
    /// are `()`-stated, since `frust-widgets`' `leaf_any` fixture erases to
    /// `AnyView<()>`.
    fn build<S: 'static>(view: &ButtonView<S>) -> ButtonWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    /// Lay `widget` out against `max_width` with a real text context, and
    /// return the resolved size.
    fn layout_with(widget: &mut ButtonWidget, max_width: f64, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        if let Some(theme) = theme {
            lctx = lctx.with_theme(theme as &dyn Any);
        }
        widget.layout(
            &mut lctx,
            &BoxConstraints::loose(Size::new(max_width, 500.0)),
        )
    }

    fn paint_at(widget: &mut ButtonWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        if let Some(theme) = theme {
            pctx = pctx.with_theme(theme as &dyn Any);
        }
        widget.paint(&mut pctx, &mut rec);
        rec
    }

    /// Advance the widget's press morph past its settling point, the way a run
    /// of real frames would (`PaintCtx::for_test`, the chosen-`FrameTime`
    /// seam, is behind a `frust-core` feature this crate's dev-dependency does
    /// not enable — see `switch.rs`'s own note).
    fn settle_morph(widget: &mut ButtonWidget) {
        let mut t = 0.0;
        for _ in 0..600 {
            t += 1.0 / 60.0;
            if !widget.motion.advance(ft_secs(t)) {
                return;
            }
        }
        panic!("the press morph never settled");
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(widget: &mut ButtonWidget, state: &mut u32, size: Size, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, event);
    }

    const ALL_SIZES: [ButtonSize; 5] = [
        ButtonSize::Xs,
        ButtonSize::Sm,
        ButtonSize::Md,
        ButtonSize::Lg,
        ButtonSize::Xl,
    ];

    const ALL_VARIANTS: [ButtonVariant; 5] = [
        ButtonVariant::Filled,
        ButtonVariant::Outlined,
        ButtonVariant::Tonal,
        ButtonVariant::Elevated,
        ButtonVariant::Text,
    ];

    // ---- metric tables ----------------------------------------------------

    #[test]
    fn measurements_match_the_reference_table() {
        // m3e_button_theme.dart:76 `_measurementsTable`:
        // (height, hPadding, iconSize, iconGap).
        let expected: [(ButtonSize, f64, f64, f64, f64); 5] = [
            (ButtonSize::Xs, 32.0, 16.0, 20.0, 8.0),
            (ButtonSize::Sm, 40.0, 16.0, 20.0, 8.0),
            (ButtonSize::Md, 56.0, 24.0, 24.0, 8.0),
            (ButtonSize::Lg, 96.0, 48.0, 32.0, 12.0),
            (ButtonSize::Xl, 136.0, 64.0, 40.0, 16.0),
        ];
        for (size, height, h_padding, icon_size, icon_gap) in expected {
            assert_eq!(
                size.metrics(),
                SizeMetrics {
                    height,
                    h_padding,
                    icon_size,
                    icon_gap
                },
                "{size:?}"
            );
        }
    }

    #[test]
    fn radius_tables_match_the_reference_themed_and_unthemed() {
        // m3e_button_theme.dart:44 (square), :52 (pressed), :60 (hovered).
        let theme = crate::baseline();
        let expected: [(ButtonSize, f64, f64, f64); 5] = [
            (ButtonSize::Xs, 12.0, 8.0, 10.0),
            (ButtonSize::Sm, 12.0, 8.0, 10.0),
            (ButtonSize::Md, 16.0, 12.0, 14.0),
            (ButtonSize::Lg, 28.0, 16.0, 22.0),
            (ButtonSize::Xl, 28.0, 16.0, 22.0),
        ];
        for (size, square, pressed, hovered) in expected {
            assert_eq!(size.square_radius(None), square, "{size:?} square unthemed");
            assert_eq!(
                size.square_radius(Some(&theme)),
                square,
                "{size:?} square themed"
            );
            assert_eq!(
                size.pressed_radius(None),
                pressed,
                "{size:?} pressed unthemed"
            );
            assert_eq!(
                size.pressed_radius(Some(&theme)),
                pressed,
                "{size:?} pressed themed"
            );
            assert_eq!(size.hovered_radius(), hovered, "{size:?} hovered");
        }
    }

    #[test]
    fn every_hovered_radius_is_the_midpoint_of_its_square_and_pressed_radii() {
        // Documented on `hovered_radius`: why that table needs no token.
        for size in ALL_SIZES {
            assert_eq!(
                size.hovered_radius(),
                (size.square_radius(None) + size.pressed_radius(None)) / 2.0,
                "{size:?}"
            );
        }
    }

    #[test]
    fn label_roles_match_the_reference_switch() {
        // m3e_base_button_state.dart:169 — labelSmall/labelMedium/labelLarge/
        // titleMedium/titleLarge.
        let theme = crate::baseline();
        let expected = [
            (ButtonSize::Xs, theme.type_scale.label_small.clone()),
            (ButtonSize::Sm, theme.type_scale.label_medium.clone()),
            (ButtonSize::Md, theme.type_scale.label_large.clone()),
            (ButtonSize::Lg, theme.type_scale.title_medium.clone()),
            (ButtonSize::Xl, theme.type_scale.title_large.clone()),
        ];
        for (size, role) in expected {
            let style = size.label_style(Some(&theme));
            assert_eq!(style.size, role.size, "{size:?} size");
            assert_eq!(style.weight, role.weight, "{size:?} weight");
            assert_eq!(style.line_height, role.line_height, "{size:?} line height");
            assert_eq!(style.color, SHAPING_INK, "{size:?} shapes in the fixed ink");
        }
    }

    #[test]
    fn unthemed_label_roles_match_the_m3_type_tokens() {
        let expected: [(ButtonSize, TypeToken); 5] = [
            (ButtonSize::Xs, LABEL_SMALL),
            (ButtonSize::Sm, LABEL_MEDIUM),
            (ButtonSize::Md, LABEL_LARGE),
            (ButtonSize::Lg, TITLE_MEDIUM),
            (ButtonSize::Xl, TITLE_LARGE),
        ];
        for (size, (pt, line_height, letter_spacing, weight)) in expected {
            let style = size.label_style(None);
            assert_eq!(style.size, pt, "{size:?}");
            assert_eq!(style.line_height, LineHeight::Absolute(line_height));
            assert_eq!(style.letter_spacing, letter_spacing);
            assert_eq!(style.weight, weight);
        }
    }

    // ---- colors -----------------------------------------------------------

    #[test]
    fn enabled_colors_match_the_reference_container_and_foreground_switches() {
        // m3e_button_theme.dart:111 (container) and :127 (foreground).
        let theme = crate::baseline();
        let s = theme.scheme();
        let expected: [(ButtonVariant, Color, Color, Option<Color>); 5] = [
            (ButtonVariant::Filled, s.primary, s.on_primary, None),
            (
                ButtonVariant::Outlined,
                Color::TRANSPARENT,
                s.primary,
                Some(s.outline),
            ),
            (
                ButtonVariant::Tonal,
                s.secondary_container,
                s.on_secondary_container,
                None,
            ),
            (
                ButtonVariant::Elevated,
                s.surface_container_low,
                s.primary,
                None,
            ),
            (ButtonVariant::Text, Color::TRANSPARENT, s.primary, None),
        ];
        for (variant, container, content, outline) in expected {
            assert_eq!(
                resolve_colors(Some(&theme), variant, true),
                ButtonColors {
                    container,
                    content,
                    outline
                },
                "{variant:?}"
            );
        }
    }

    #[test]
    fn unthemed_colors_fall_back_to_the_baseline_light_roles() {
        let expected: [(ButtonVariant, Color, Color, Option<Color>); 5] = [
            (ButtonVariant::Filled, PRIMARY, ON_PRIMARY, None),
            (
                ButtonVariant::Outlined,
                Color::TRANSPARENT,
                PRIMARY,
                Some(OUTLINE),
            ),
            (
                ButtonVariant::Tonal,
                SECONDARY_CONTAINER,
                ON_SECONDARY_CONTAINER,
                None,
            ),
            (
                ButtonVariant::Elevated,
                SURFACE_CONTAINER_LOW,
                PRIMARY,
                None,
            ),
            (ButtonVariant::Text, Color::TRANSPARENT, PRIMARY, None),
        ];
        for (variant, container, content, outline) in expected {
            assert_eq!(
                resolve_colors(None, variant, true),
                ButtonColors {
                    container,
                    content,
                    outline
                },
                "{variant:?}"
            );
        }
    }

    #[test]
    fn disabled_colors_are_the_38_and_12_percent_on_surface_roles() {
        // m3e_button_constants.dart:27/:30/:33 — 0.38 content, 0.12 container,
        // 0.12 outline.
        assert_eq!(DISABLED_CONTENT_OPACITY, 0.38);
        assert_eq!(DISABLED_CONTAINER_OPACITY, 0.12);
        let theme = crate::baseline();
        let on_surface = theme.scheme().on_surface;
        for variant in ALL_VARIANTS {
            let colors = resolve_colors(Some(&theme), variant, false);
            assert_eq!(
                colors.content,
                with_alpha(on_surface, 0.38),
                "{variant:?} content"
            );
            if is_transparent(variant) {
                assert_eq!(
                    colors.container,
                    Color::TRANSPARENT,
                    "{variant:?} stays transparent"
                );
            } else {
                assert_eq!(
                    colors.container,
                    with_alpha(on_surface, 0.12),
                    "{variant:?} container"
                );
            }
            if variant == ButtonVariant::Outlined {
                assert_eq!(colors.outline, Some(with_alpha(on_surface, 0.12)));
            } else {
                assert_eq!(colors.outline, None, "{variant:?} draws no outline");
            }
        }
    }

    #[test]
    fn elevation_matches_the_reference_state_table() {
        // m3e_button_theme.dart:150: (variant, pressed, hovered) -> dp.
        for variant in ALL_VARIANTS {
            for pressed in [false, true] {
                for hovered in [false, true] {
                    let expected = match variant {
                        ButtonVariant::Elevated => {
                            if pressed {
                                0.0
                            } else if hovered {
                                3.0
                            } else {
                                1.0
                            }
                        }
                        ButtonVariant::Filled | ButtonVariant::Tonal => {
                            if !pressed && hovered {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        ButtonVariant::Outlined | ButtonVariant::Text => 0.0,
                    };
                    assert_eq!(
                        elevation_dp(variant, true, pressed, hovered),
                        expected,
                        "{variant:?} pressed={pressed} hovered={hovered}"
                    );
                    assert_eq!(
                        elevation_dp(variant, false, pressed, hovered),
                        0.0,
                        "{variant:?} disabled is always flat"
                    );
                }
            }
        }
    }

    #[test]
    fn shadows_resolve_onto_m3_elevation_levels_one_and_two() {
        let theme = crate::baseline();
        assert_eq!(
            resolve_shadow(Some(&theme), 0.0),
            None,
            "flat casts nothing"
        );
        let (blur, y, _) = resolve_shadow(Some(&theme), 1.0).expect("1dp casts a shadow");
        let level1 = theme.elevation.level1.shadow(theme.brightness);
        assert_eq!((blur, y), (level1.blur_std_dev, level1.y_offset));
        let (blur, y, _) = resolve_shadow(Some(&theme), 3.0).expect("3dp casts a shadow");
        let level2 = theme.elevation.level2.shadow(theme.brightness);
        assert_eq!((blur, y), (level2.blur_std_dev, level2.y_offset));
        // Unthemed applies `crate::tokens::elevation()`'s own math.
        assert_eq!(
            resolve_shadow(None, 3.0),
            Some((3.0, 2.5, FALLBACK_SHADOW_COLOR))
        );
    }

    // ---- layout -----------------------------------------------------------

    #[test]
    fn every_size_lays_out_at_its_token_height() {
        for size in ALL_SIZES {
            let mut widget = build(&button::<u32, _>("Save", |_| {}).size(size));
            let laid = layout_with(&mut widget, 500.0, None);
            assert_eq!(laid.height, size.metrics().height, "{size:?}");
        }
    }

    #[test]
    fn width_is_the_content_plus_the_size_padding_floored_at_48() {
        for size in ALL_SIZES {
            let mut widget = build(&button::<u32, _>("Save", |_| {}).size(size));
            let laid = layout_with(&mut widget, 500.0, None);
            let metrics = widget.content_metrics();
            let expected =
                (metrics.content_width + size.metrics().h_padding * 2.0).max(MIN_WIDTH_FLOOR);
            assert_eq!(laid.width, expected, "{size:?}");
        }
    }

    #[test]
    fn an_empty_label_still_respects_the_min_width_floor() {
        // M3EButtonTheme.minWidthFloor = 48 (m3e_button_theme.dart:16).
        let mut widget = build(&button::<u32, _>("", |_| {}).size(ButtonSize::Xs));
        let laid = layout_with(&mut widget, 500.0, None);
        assert_eq!(laid.width, MIN_WIDTH_FLOOR);
    }

    #[test]
    fn the_icon_constructor_lays_the_icon_out_at_the_size_icon_and_gap() {
        // m3e_button_state.dart:19-:36 — icon at `m.iconSize`, then a
        // `m.iconGap`-wide box, then the label.
        for size in ALL_SIZES {
            let metrics = size.metrics();
            let view = button_with_icon::<(), _>(leaf_any(64.0, 64.0), "Save", |_| {}).size(size);
            let mut widget = build(&view);
            let laid = layout_with(&mut widget, 500.0, None);

            let icon = widget.icon.as_ref().expect("icon pod");
            assert_eq!(
                icon.size(),
                Size::new(metrics.icon_size, metrics.icon_size),
                "{size:?} icon is tight-constrained to the token size"
            );
            assert_eq!(
                icon.origin().y,
                (laid.height - metrics.icon_size) / 2.0,
                "{size:?} icon is vertically centered"
            );
            assert_eq!(
                widget.label_origin.x - (icon.origin().x + metrics.icon_size),
                metrics.icon_gap,
                "{size:?} label starts one icon gap past the icon"
            );
            let m = widget.content_metrics();
            assert_eq!(
                m.content_width,
                metrics.icon_size + metrics.icon_gap + m.painted_label_width,
                "{size:?} content is icon + gap + label"
            );
        }
    }

    #[test]
    fn end_alignment_moves_the_icon_to_the_trailing_side() {
        let view = button_with_icon::<(), _>(leaf_any(20.0, 20.0), "Save", |_| {})
            .icon_alignment(IconAlignment::End);
        let mut widget = build(&view);
        layout_with(&mut widget, 500.0, None);
        let icon = widget.icon.as_ref().expect("icon pod");
        let metrics = ButtonSize::Sm.metrics();
        assert!(
            icon.origin().x > widget.label_origin.x,
            "the icon trails the label"
        );
        assert_eq!(
            icon.origin().x
                - (widget.label_origin.x + widget.content_metrics().painted_label_width),
            metrics.icon_gap,
            "still separated by exactly one icon gap"
        );
    }

    #[test]
    fn a_button_with_no_icon_adds_no_gap() {
        let mut widget = build(&button::<u32, _>("Save", |_| {}));
        layout_with(&mut widget, 500.0, None);
        let m = widget.content_metrics();
        assert_eq!(m.content_width, m.painted_label_width);
    }

    // ---- the overflow seam ------------------------------------------------

    #[derive(Default)]
    struct RecordingObserver {
        seen: RefCell<Vec<ContentMetrics>>,
    }

    impl OverflowObserver for RecordingObserver {
        fn measured(&self, metrics: ContentMetrics) {
            self.seen.borrow_mut().push(metrics);
        }
    }

    #[test]
    fn the_overflow_seam_fires_every_layout_with_what_it_measured() {
        let observer = Rc::new(RecordingObserver::default());
        let view = button::<u32, _>("A comfortably long button label", |_| {})
            .overflow_observer(observer.clone());
        let mut widget = build(&view);
        layout_with(&mut widget, 500.0, None);

        let seen = observer.seen.borrow().clone();
        assert_eq!(seen.len(), 1, "one measurement per layout pass");
        let metrics = seen[0];
        assert!(metrics.natural_label_width > 0.0);
        assert!(!metrics.overflows(), "500px is plenty of room");
        assert_eq!(
            metrics,
            widget.content_metrics(),
            "the observer sees exactly what the widget kept"
        );
    }

    #[test]
    fn a_label_wider_than_the_room_left_reports_an_overflow_and_is_fitted() {
        let observer = Rc::new(RecordingObserver::default());
        let view = button::<u32, _>(
            "A label far too long to ever fit inside a narrow button",
            |_| {},
        )
        .overflow_observer(observer.clone());
        let mut widget = build(&view);
        let laid = layout_with(&mut widget, 120.0, None);

        let metrics = widget.content_metrics();
        assert!(
            metrics.overflows(),
            "the natural width exceeds what is available"
        );
        assert!(
            metrics.painted_label_width <= metrics.available_label_width + 0.5,
            "the painted label was fitted to the available width"
        );
        assert!(laid.width <= 120.0, "and the button honors its constraint");
        assert_eq!(observer.seen.borrow().len(), 1);
    }

    #[test]
    fn an_unbounded_constraint_never_reports_an_overflow() {
        let mut widget = build(&button::<u32, _>("Save", |_| {}));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        widget.layout(
            &mut lctx,
            &BoxConstraints::loose(Size::new(f64::INFINITY, 500.0)),
        );
        let metrics = widget.content_metrics();
        assert_eq!(metrics.available_label_width, f64::INFINITY);
        assert!(!metrics.overflows());
    }

    // ---- the decoration seam ----------------------------------------------

    struct RecordingDecoration {
        fills: RefCell<Vec<ButtonSurface>>,
        overlays: RefCell<Vec<ButtonSurface>>,
        outlines: RefCell<Vec<ButtonSurface>>,
        /// What every hook reports back to the core.
        outcome: DecorationOutcome,
    }

    impl RecordingDecoration {
        fn new(outcome: DecorationOutcome) -> Self {
            Self {
                fills: RefCell::new(Vec::new()),
                overlays: RefCell::new(Vec::new()),
                outlines: RefCell::new(Vec::new()),
                outcome,
            }
        }
    }

    impl ButtonDecoration for RecordingDecoration {
        fn paint_fill(
            &self,
            surface: &ButtonSurface,
            _scene: &mut dyn PaintScene,
        ) -> DecorationOutcome {
            self.fills.borrow_mut().push(*surface);
            self.outcome
        }
        fn paint_overlay(
            &self,
            surface: &ButtonSurface,
            _scene: &mut dyn PaintScene,
        ) -> DecorationOutcome {
            self.overlays.borrow_mut().push(*surface);
            self.outcome
        }
        fn paint_outline(
            &self,
            surface: &ButtonSurface,
            _scene: &mut dyn PaintScene,
        ) -> DecorationOutcome {
            self.outlines.borrow_mut().push(*surface);
            self.outcome
        }
    }

    #[test]
    fn every_decoration_hook_fires_with_the_painted_surface() {
        let decoration = Rc::new(RecordingDecoration::new(DecorationOutcome::Skipped));
        let view = button::<u32, _>("Save", |_| {})
            .variant(ButtonVariant::Outlined)
            .decoration(decoration.clone());
        let mut widget = build(&view);
        let laid = layout_with(&mut widget, 500.0, None);
        paint_at(&mut widget, laid, None);

        for (name, seen) in [
            ("fill", decoration.fills.borrow().clone()),
            ("overlay", decoration.overlays.borrow().clone()),
            ("outline", decoration.outlines.borrow().clone()),
        ] {
            assert_eq!(seen.len(), 1, "the {name} hook fired exactly once");
            let surface = seen[0];
            assert_eq!(surface.size, laid);
            assert_eq!(surface.radius, laid.height / 2.0, "the live pill radius");
            assert_eq!(surface.outline_width, OUTLINE_WIDTH);
            assert!(surface.enabled);
            assert_eq!(
                surface.outline,
                Some(OUTLINE),
                "outlined carries a hairline"
            );
        }
    }

    #[test]
    fn a_default_decoration_is_a_no_op_the_core_paints_through() {
        struct Silent;
        impl ButtonDecoration for Silent {}

        let mut plain = build(&button::<u32, _>("Save", |_| {}));
        let laid = layout_with(&mut plain, 500.0, None);
        let without = paint_at(&mut plain, laid, None);

        let mut decorated = build(&button::<u32, _>("Save", |_| {}).decoration(Rc::new(Silent)));
        layout_with(&mut decorated, 500.0, None);
        let with = paint_at(&mut decorated, laid, None);

        assert_eq!(with.rrects, without.rrects, "identical fills");
        assert_eq!(with.strokes.len(), without.strokes.len());
        assert_eq!(with.runs, without.runs, "and an identically painted label");
    }

    #[test]
    fn a_painting_decoration_suppresses_the_core_layers_it_took_over() {
        let decoration = Rc::new(RecordingDecoration::new(DecorationOutcome::Painted));
        let view = button::<u32, _>("Save", |_| {})
            .variant(ButtonVariant::Outlined)
            .decoration(decoration.clone());
        let mut widget = build(&view);
        let laid = layout_with(&mut widget, 500.0, None);
        let rec = paint_at(&mut widget, laid, None);

        assert!(
            rec.rrects.is_empty(),
            "the fill was taken over, and so is the overlay slot the state \
             layer would paint into"
        );
        assert!(rec.strokes.is_empty(), "and so was the outline");
        assert_eq!(rec.runs.len(), 1, "the label is still the core's to paint");
    }

    // ---- paint ------------------------------------------------------------

    #[test]
    fn a_resting_round_button_paints_its_container_at_a_pill_radius() {
        let mut widget = build(&button::<u32, _>("Save", |_| {}));
        let laid = layout_with(&mut widget, 500.0, None);
        let rec = paint_at(&mut widget, laid, None);
        let (origin, size, radius, color) = rec.rrects[0];
        assert_eq!(origin, Point::ZERO);
        assert_eq!(size, laid);
        assert_eq!(radius, laid.height / 2.0);
        assert_eq!(color, PRIMARY);
        assert!(rec.shadows.is_empty(), "a filled button rests flat");
        assert_eq!(rec.runs.len(), 1, "the label paints one run");
        assert_eq!(rec.runs[0].1, ON_PRIMARY);
    }

    #[test]
    fn a_square_button_paints_the_size_token_radius() {
        let theme = crate::baseline();
        let mut widget = build(
            &button::<u32, _>("Save", |_| {})
                .shape(ButtonShape::Square)
                .size(ButtonSize::Md),
        );
        let laid = layout_with(&mut widget, 500.0, Some(&theme));
        let rec = paint_at(&mut widget, laid, Some(&theme));
        assert_eq!(rec.rrects[0].2, ButtonSize::Md.square_radius(Some(&theme)));
        assert_eq!(rec.rrects[0].3, theme.scheme().primary);
    }

    #[test]
    fn an_explicit_corner_radius_overrides_both_shape_families() {
        let mut widget = build(&button::<u32, _>("Save", |_| {}).corner_radius(3.0));
        let laid = layout_with(&mut widget, 500.0, None);
        let rec = paint_at(&mut widget, laid, None);
        assert_eq!(rec.rrects[0].2, 3.0);
    }

    #[test]
    fn a_text_button_paints_no_container_and_no_outline() {
        let mut widget = build(&button::<u32, _>("Save", |_| {}).variant(ButtonVariant::Text));
        let laid = layout_with(&mut widget, 500.0, None);
        let rec = paint_at(&mut widget, laid, None);
        assert!(
            rec.rrects.is_empty(),
            "a fully transparent container is never filled"
        );
        assert!(rec.strokes.is_empty());
        assert_eq!(rec.runs[0].1, PRIMARY);
    }

    #[test]
    fn an_outlined_button_strokes_a_one_dp_hairline() {
        let mut widget = build(&button::<u32, _>("Save", |_| {}).variant(ButtonVariant::Outlined));
        let laid = layout_with(&mut widget, 500.0, None);
        let rec = paint_at(&mut widget, laid, None);
        assert!(rec.rrects.is_empty(), "no container fill");
        assert_eq!(rec.strokes.len(), 1);
        assert_eq!(rec.strokes[0].1, OUTLINE_WIDTH);
        assert_eq!(rec.strokes[0].2, OUTLINE);
    }

    #[test]
    fn the_outlined_hairline_is_inset_by_half_its_width() {
        // Mirrors `card.rs`'s outlined inset: a centered stroke would spill
        // half its width outside the container.
        let rr = inset_stroke_rect(Size::new(100.0, 40.0), 20.0, OUTLINE_WIDTH);
        assert_eq!(rr.rect().x0, 0.5);
        assert_eq!(rr.rect().y0, 0.5);
        assert_eq!(rr.rect().x1, 99.5);
        assert_eq!(rr.rect().y1, 39.5);
    }

    #[test]
    fn an_elevated_button_casts_its_resting_level_one_shadow() {
        let mut widget = build(&button::<u32, _>("Save", |_| {}).variant(ButtonVariant::Elevated));
        let laid = layout_with(&mut widget, 500.0, None);
        let rec = paint_at(&mut widget, laid, None);
        assert_eq!(rec.shadows.len(), 1);
        let (origin, _, _, blur, _) = rec.shadows[0];
        assert_eq!(blur, 1.0, "1dp of elevation");
        assert_eq!(origin.y, 1.5, "offset by the level's own y");
    }

    #[test]
    fn a_disabled_button_paints_the_twelve_percent_container_and_a_38_percent_label() {
        let mut widget = build(&button::<u32, _>("Save", |_| {}).enabled(false));
        let laid = layout_with(&mut widget, 500.0, None);
        let rec = paint_at(&mut widget, laid, None);
        assert_eq!(rec.rrects.len(), 1, "container only, no state layer");
        assert_eq!(rec.rrects[0].3, with_alpha(ON_SURFACE, 0.12));
        assert_eq!(rec.runs[0].1, with_alpha(ON_SURFACE, 0.38));
    }

    #[test]
    fn an_unfocused_button_draws_no_ring_and_paint_re_syncs_the_flag() {
        let mut widget = build(&button::<u32, _>("Save", |_| {}));
        let laid = layout_with(&mut widget, 500.0, None);
        let rec = paint_at(&mut widget, laid, None);
        assert!(rec.strokes.is_empty(), "an unfocused button draws no ring");

        // `PaintCtx::has_focus` is authoritative: a stale latched flag is
        // corrected by the paint pass, not honored by it.
        widget.state.set_focused(true);
        let rec = paint_at(&mut widget, laid, None);
        assert!(rec.strokes.is_empty());
        assert!(!widget.state.focused, "the flag was re-synced from the ctx");
    }

    #[test]
    fn the_focus_ring_paints_at_the_reference_outset_and_radius() {
        // m3e_focus_ring.dart:49 — gap 2 + width 2 outside the container, with
        // the radius grown by the same outset.
        let size = Size::new(100.0, 40.0);
        let ring = focus_ring_rect(size, 20.0);
        let outset = FOCUS_RING_GAP + FOCUS_RING_WIDTH;
        let half = FOCUS_RING_WIDTH / 2.0;
        assert_eq!(ring.rect().x0, -outset + half);
        assert_eq!(ring.rect().y0, -outset + half);
        assert_eq!(ring.rect().x1, size.width + outset - half);
        assert_eq!(ring.rect().y1, size.height + outset - half);
        assert_eq!(ring.radii().top_left, 20.0 + outset - half);
        assert_eq!(
            ring.rect().x0 - half,
            -outset,
            "the ring's outer edge lands exactly on the reference's inset"
        );

        let mut rec = Recorder::default();
        paint_focus_ring(Point::ZERO, size, 20.0, PRIMARY, &mut rec);
        assert_eq!(rec.strokes.len(), 1);
        assert_eq!(rec.strokes[0].1, FOCUS_RING_WIDTH);
        assert_eq!(rec.strokes[0].2, PRIMARY);
    }

    // ---- the press morph --------------------------------------------------

    #[test]
    fn a_press_morphs_the_radius_from_the_pill_to_the_pressed_token() {
        let mut state = 0u32;
        let mut widget = build(&button::<u32, _>("Save", |_| {}));
        let laid = layout_with(&mut widget, 500.0, None);
        let rec = paint_at(&mut widget, laid, None);
        let resting = rec.rrects[0].2;
        assert_eq!(resting, laid.height / 2.0);

        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Down, 10.0, 10.0),
        );
        // The frame right after the press still paints the resting radius —
        // the morph springs, it does not cut.
        let rec = paint_at(&mut widget, laid, None);
        assert_eq!(rec.rrects[0].2, resting);

        settle_morph(&mut widget);
        let rec = paint_at(&mut widget, laid, None);
        assert_eq!(
            rec.rrects[0].2,
            ButtonSize::Sm.pressed_radius(None),
            "settled on the pressed radius"
        );

        // Releasing springs it back to the pill.
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Up, 10.0, 10.0),
        );
        paint_at(&mut widget, laid, None);
        settle_morph(&mut widget);
        let rec = paint_at(&mut widget, laid, None);
        assert_eq!(rec.rrects[0].2, resting, "and back to rest on release");
    }

    #[test]
    fn a_pressed_radius_override_wins_over_the_token() {
        let mut state = 0u32;
        let mut widget = build(&button::<u32, _>("Save", |_| {}).pressed_radius(2.0));
        let laid = layout_with(&mut widget, 500.0, None);
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Down, 10.0, 10.0),
        );
        assert_eq!(widget.target_radius(None, laid.height), 2.0);
    }

    #[test]
    fn hover_targets_the_hovered_radius_and_press_outranks_it() {
        let mut widget = build(&button::<u32, _>("Save", |_| {}).size(ButtonSize::Md));
        let laid = layout_with(&mut widget, 500.0, None);
        assert_eq!(widget.target_radius(None, laid.height), laid.height / 2.0);

        widget.state.set_hovered(true);
        assert_eq!(
            widget.target_radius(None, laid.height),
            ButtonSize::Md.hovered_radius()
        );

        widget.state.set_pressed(true);
        assert_eq!(
            widget.target_radius(None, laid.height),
            ButtonSize::Md.pressed_radius(None)
        );
    }

    #[test]
    fn a_disabled_button_never_leaves_its_resting_radius() {
        let mut widget = build(&button::<u32, _>("Save", |_| {}).enabled(false));
        let laid = layout_with(&mut widget, 500.0, None);
        widget.state.set_pressed(true);
        widget.state.set_hovered(true);
        assert_eq!(widget.target_radius(None, laid.height), laid.height / 2.0);
    }

    // ---- events -----------------------------------------------------------

    #[test]
    fn a_press_released_inside_fires_on_press() {
        let mut state = 0u32;
        let mut widget = build(&button::<u32, _>("Save", |s: &mut u32| *s += 1));
        let laid = layout_with(&mut widget, 500.0, None);
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Down, 10.0, 10.0),
        );
        assert!(widget.state.pressed);
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Up, 10.0, 10.0),
        );
        assert_eq!(state, 1);
        assert!(!widget.state.pressed);
        assert!(!widget.captured);
    }

    #[test]
    fn a_press_released_outside_fires_nothing() {
        let mut state = 0u32;
        let mut widget = build(&button::<u32, _>("Save", |s: &mut u32| *s += 1));
        let laid = layout_with(&mut widget, 500.0, None);
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Down, 10.0, 10.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Up, 999.0, 10.0),
        );
        assert_eq!(state, 0);
    }

    #[test]
    fn a_cancelled_press_fires_nothing_and_clears_the_press() {
        let mut state = 0u32;
        let mut widget = build(&button::<u32, _>("Save", |s: &mut u32| *s += 1));
        let laid = layout_with(&mut widget, 500.0, None);
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Down, 10.0, 10.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Cancel, 10.0, 10.0),
        );
        assert_eq!(state, 0);
        assert!(!widget.state.pressed);
        assert!(!widget.captured);
    }

    #[test]
    fn only_a_primary_button_presses() {
        let mut state = 0u32;
        let mut widget = build(&button::<u32, _>("Save", |s: &mut u32| *s += 1));
        let laid = layout_with(&mut widget, 500.0, None);
        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(10.0, 10.0),
            button: PointerButton::Secondary,
        });
        dispatch(&mut widget, &mut state, laid, &secondary);
        assert!(!widget.state.pressed);
    }

    #[test]
    fn a_disabled_button_ignores_every_pointer_phase() {
        let mut state = 0u32;
        let mut widget = build(&button::<u32, _>("Save", |s: &mut u32| *s += 1).enabled(false));
        let laid = layout_with(&mut widget, 500.0, None);
        for phase in [
            PointerPhase::Down,
            PointerPhase::Move,
            PointerPhase::Up,
            PointerPhase::Cancel,
        ] {
            dispatch(&mut widget, &mut state, laid, &ev(phase, 10.0, 10.0));
        }
        assert_eq!(state, 0);
        assert!(!widget.state.pressed);
    }

    #[test]
    fn dragging_off_the_button_disarms_the_press_and_dragging_back_rearms_it() {
        let mut state = 0u32;
        let mut widget = build(&button::<u32, _>("Save", |s: &mut u32| *s += 1));
        let laid = layout_with(&mut widget, 500.0, None);
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Down, 10.0, 10.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Move, 999.0, 10.0),
        );
        assert!(!widget.state.pressed);
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Move, 10.0, 10.0),
        );
        assert!(widget.state.pressed);
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Up, 10.0, 10.0),
        );
        assert_eq!(state, 1);
    }

    #[test]
    fn an_uncaptured_move_latches_hover_without_handling_the_event() {
        let mut state = 0u32;
        let mut widget = build(&button::<u32, _>("Save", |_| {}));
        let laid = layout_with(&mut widget, 500.0, None);
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Move, 10.0, 10.0),
        );
        assert!(
            widget.state.hovered,
            "the latch is the frame source for gain"
        );
        dispatch(
            &mut widget,
            &mut state,
            laid,
            &ev(PointerPhase::Move, 999.0, 10.0),
        );
        assert!(!widget.state.hovered);
    }

    // ---- through a real RenderRoot ----------------------------------------
    //
    // Hover and focus are *authoritative reads* off `PaintCtx`, seeded by the
    // pod chain, so a bare `PaintCtx` can never fake them — only a real tree
    // can (the pattern `list_item.rs`'s own hover harness uses). The same
    // harness is what makes the semantics pass reachable, since
    // `SemanticsCtx::new`/`finish` are `pub(crate)` to `frust-core`.

    const HARNESS_WINDOW: Size = Size::new(300.0, 200.0);

    struct Harness {
        root: frust_core::RenderRoot<u32, ButtonView<u32>>,
        state: u32,
        tcx: TextContext,
        enabled: bool,
    }

    impl Harness {
        fn new(enabled: bool) -> Self {
            let mut harness = Self {
                root: frust_core::RenderRoot::new(),
                state: 0,
                tcx: TextContext::new(),
                enabled,
            };
            harness.sync();
            harness
        }

        fn sync(&mut self) {
            let enabled = self.enabled;
            let mut app =
                move |_: &mut u32| button::<u32, _>("Save", |s: &mut u32| *s += 1).enabled(enabled);
            self.root.rebuild(&mut app, &mut self.state);
            self.root
                .layout_with_text(HARNESS_WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn dispatch(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        /// The alpha of the state-layer overlay painted this frame, if any
        /// (the container fill is always the first rect).
        fn overlay_alpha(&mut self) -> Option<f32> {
            let rec = self.paint();
            rec.rrects
                .get(1)
                .map(|(_, _, _, color)| color.components[3])
        }
    }

    #[test]
    fn hovering_paints_the_hover_state_layer_and_pressing_outranks_it() {
        let mut h = Harness::new(true);
        assert_eq!(h.overlay_alpha(), None, "no overlay at rest");

        h.dispatch(PointerPhase::Move, 10.0, 10.0);
        assert_eq!(
            h.overlay_alpha(),
            Some(crate::interaction::HOVER_OPACITY),
            "the hovered button tints at the M3E hover opacity"
        );

        h.dispatch(PointerPhase::Down, 10.0, 10.0);
        assert_eq!(
            h.overlay_alpha(),
            Some(crate::interaction::PRESSED_OPACITY),
            "pressed outranks hover in the precedence order"
        );

        h.dispatch(PointerPhase::Up, 10.0, 10.0);
        assert_eq!(h.state, 1, "and the release fired the callback");
    }

    #[test]
    fn moving_off_the_button_drops_the_hover_tint() {
        let mut h = Harness::new(true);
        h.dispatch(PointerPhase::Move, 10.0, 10.0);
        assert!(h.overlay_alpha().is_some());
        h.dispatch(PointerPhase::Move, 290.0, 190.0);
        assert_eq!(h.overlay_alpha(), None);
    }

    #[test]
    fn a_disabled_button_never_tints_and_never_fires() {
        let mut h = Harness::new(false);
        h.dispatch(PointerPhase::Move, 10.0, 10.0);
        assert_eq!(h.overlay_alpha(), None, "a disabled button claims no hover");
        h.dispatch(PointerPhase::Down, 10.0, 10.0);
        h.dispatch(PointerPhase::Up, 10.0, 10.0);
        assert_eq!(h.state, 0);
    }

    // ---- semantics --------------------------------------------------------

    fn semantics_node<V>(logic: impl FnMut(&mut ()) -> V) -> frust::authoring::Node
    where
        V: frust::authoring::View<()> + 'static,
    {
        let mut root: frust_core::RenderRoot<(), V> = frust_core::RenderRoot::new();
        let mut state = ();
        let mut tcx = TextContext::new();
        let mut logic = logic;
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(HARNESS_WINDOW, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(id, _)| *id != update.root)
            .expect("the button contributes exactly one node");
        node.clone()
    }

    #[test]
    fn semantics_report_a_labelled_clickable_button() {
        let node = semantics_node(|_| button::<(), _>("Save", |_| {}));
        assert_eq!(node.role(), Role::Button);
        assert_eq!(node.label(), Some("Save"));
        assert!(!node.is_disabled());
    }

    #[test]
    fn a_disabled_button_reports_disabled_semantics() {
        let node = semantics_node(|_| button::<(), _>("Save", |_| {}).enabled(false));
        assert_eq!(node.role(), Role::Button);
        assert!(node.is_disabled());
    }
}
