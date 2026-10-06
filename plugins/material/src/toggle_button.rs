// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// whose `lib/components/toggle_button/` tree is itself vendored from
// m3e_buttons (MIT, © 2026 Mudit Purohit) — `m3e_toggle_button.dart` +
// `components/m3e_toggle_button_{state,shape,style,content}.dart` +
// `styles/m3e_toggle_button_theme.dart`, plus the connected-group corner
// constants from `../toggle_button_group/styles/m3e_toggle_button_group_theme.dart`.
// Porting decision: `button::motion::RadiusPaddingMotion` is private to the
// `button` module (a wave-peer-owned tree this task must not touch — see this
// module's own docs), so the checked-shape spring below is a sibling copy of
// its exact contract, generalized from one radius to four independent
// corners for the connected-group case.

//! The Material 3 Expressive **toggle button**: a controlled on/off control
//! that morphs its container between a round (unchecked) and a square
//! (checked) shape, in any of the five button style variants.
//!
//! [`toggle_button`] builds a filled toggle button; [`outlined_toggle_button`],
//! [`tonal_toggle_button`], [`elevated_toggle_button`] and
//! [`text_toggle_button`] are the reference's other four named constructors.
//! Every one is a **controlled component** (see `docs/CODE_STANDARDS.md`'s
//! Interaction Semantics): a tap reports the *requested* value through
//! `on_checked_change` and leaves `checked` untouched — the app feeds the
//! confirmed value back on the next rebuild, exactly like
//! [`mod@crate::switch`]/[`frust::Checkbox`].
//!
//! # Variants × checked (`_resolvedForegroundColor`/`_resolvedBackgroundColor`,
//! `m3e_toggle_button_style.dart:118`/`:134`)
//!
//! | Variant | Content unchecked | Content checked | Container unchecked | Container checked |
//! |---|---|---|---|---|
//! | [`ButtonVariant::Filled`] (default) | `on_surface_variant` | `on_primary` | `surface_container_highest` | `primary` |
//! | [`ButtonVariant::Elevated`] | `primary` | `on_primary` | `surface_container_low` | `primary` |
//! | [`ButtonVariant::Tonal`] | `on_surface_variant` | `on_secondary_container` | `surface_container_highest` | `secondary_container` |
//! | [`ButtonVariant::Outlined`] | `on_surface` | `on_secondary_container` | transparent | `secondary_container` |
//! | [`ButtonVariant::Text`] | `on_surface` | `primary` | transparent | transparent |
//!
//! Unlike the plain [`mod@crate::button`], an unchecked Filled/Tonal toggle
//! still paints an opaque container (`surface_container_highest`) — the "off"
//! pill look every M3E toggle-button row uses. The outline hairline paints
//! only for [`ButtonVariant::Outlined`], both checked and unchecked
//! (`_sideProperty`), reusing `colors.outline` exactly as the plain button
//! does. Disabled colors are checked-independent, reusing the plain button's
//! 38%/12% `on_surface` roles ([`crate::interaction::DISABLED_CONTENT_OPACITY`]/
//! [`crate::interaction::DISABLED_CONTAINER_OPACITY`] — `M3EButtonConstants`'s
//! disabled alphas are the same three constants the plain button cites).
//! Elevation is **not** checked-aware (`_buttonTheme.elevation` reads plain
//! `WidgetState`s, never adding `.selected` the way the color/side resolvers
//! do) — reusing the plain button's own (variant, enabled, pressed, hovered)
//! table verbatim (duplicated in [`elevation_dp`] since `button::core` is
//! private; see this module's Porting decision above).
//!
//! # Sizes: three of the five (`m3e_button_theme.dart:76`'s rows)
//!
//! | Size | Height | Padding (labeled) | Icon | Gap | Label role |
//! |---|---|---|---|---|---|
//! | [`ToggleButtonSize::Sm`] (default) | 40 | 16 | 20 | 8 | `label_medium` |
//! | [`ToggleButtonSize::Md`] | 56 | 24 | 24 | 8 | `label_large` |
//! | [`ToggleButtonSize::Lg`] | 96 | 48 | 32 | 12 | `title_medium` |
//!
//! Scoped to `sm`/`md`/`lg` (the reference's `xs`/`xl` rows are not ported).
//! An icon-only toggle (no effective label) halves its horizontal padding
//! (`_buildWidget`'s `hPad = _hasLabel ? m.hPadding : m.hPadding / 2`) — and,
//! also unlike the plain button, a toggle button has **no** minimum-width
//! floor: the reference's `_buildButtonStyle` sets `minimumSize:
//! Size(0, height)`, never touching `M3EButtonTheme.minWidthFloor` (that
//! constant is read only by the plain button's own `ButtonStyle`,
//! `m3e_button_style.dart:11`) — transcribed faithfully, not "fixed".
//!
//! # Content wider than the box: covered, never re-fitted
//!
//! A toggle button shapes its label **once, at its natural width**, and a box
//! too small for the resulting content row centers and *clips* it rather than
//! ellipsizing it to fit. That is the reference's own answer, not a
//! simplification: `_fitContentToConstraints`
//! (`m3e_toggle_button_content.dart:95`-`:111`) hands a bounded constraint to
//! `FittedBox(fit: BoxFit.none, clipBehavior: Clip.hardEdge)`, which lays the
//! natural row out unbounded, applies no scale, centers it
//! (`Alignment.center`) and hard-clips the overflow. The one ellipsis the
//! reference's label style carries (`m3e_base_button_state.dart:177`) can
//! therefore never fire on this path — nothing ever hands the label a bounded
//! width — and the label merges only `maxLines: 1, softWrap: false`
//! (`_buildLabelText`, `:194`). The plain [`mod@crate::button`] is the genuine
//! contrast, and keeps its own ellipsize-to-fit: its label sits in a
//! `Flexible` inside the content row (`m3e_button_state.dart:28`-`:35`), so it
//! really is handed the width left over.
//!
//! This is what a [`mod@crate::button_group`] member's press squish looks like
//! in the reference and now here: the padding compresses first (the row is
//! centered in a shrinking box), then the box edge slides over stationary
//! glyphs — never a label re-flowing to a new ellipsis every frame, and never
//! a truncation left behind once the box comes back. Two consequences worth
//! naming: [`LabelRun`] caches on style alone (no width key, so a squish
//! re-shapes nothing), and `content_x` is allowed to go negative so the
//! overflow is split evenly off both ends.
//!
//! # Checked shape morph: round ↔ square, one spring
//!
//! Unchecked rests at the round family radius (`height / 2`, a pill);
//! checked rests at the square family radius —
//! [`ToggleButtonSize::square_radius`], the exact per-size token the plain
//! button's own square shape resolves
//! (`theme.shape.medium`/`large`/`extra_large`, 12/16/28 unthemed). Pressed
//! and hovered each override *both* of those with their own per-size token
//! ([`ToggleButtonSize::pressed_radius`]/[`ToggleButtonSize::hovered_radius`],
//! same tables as the plain button), in `pressed > hovered > checked > rest`
//! precedence — `m3e_toggle_button_shape.dart`'s
//! `_resolveStandaloneTargetRadius`. All four corners move together for a
//! standalone toggle button; see *Group-connection corner asymmetry* below
//! for why the driver still carries four independent corners rather than one
//! scalar.
//!
//! The spring is [`SHAPE_SPRING`] — stiffness 380, damping ratio 0.55, the
//! same `expressiveSpatialPress` preset
//! ([`crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`]) the plain
//! button's press morph flings along
//! (`_M3EToggleButtonState.effectiveMotion`'s
//! `M3EButtonMotion.expressiveSpatialPress` default,
//! `m3e_toggle_button_state.dart:61`-`:62`). [`ShapeMotion`] carries the exact
//! retarget/continuity contract [`mod@crate::button`]'s
//! `RadiusPaddingMotion` documents (mount-frame snap, a
//! [`RETARGET_TOLERANCE`]-px dead band, mid-flight retarget continuity from
//! the value being painted *right now* rather than a discontinuous
//! progress-flip, and a 1.5× [`OVERSHOOT_LIMIT`]) — this module's own tests
//! re-pin all of it directly, since that type is private to a sibling module
//! this task must not touch.
//!
//! # Group-connection corner asymmetry
//!
//! [`ToggleButtonView::group_connected`]/[`ToggleButtonView::first_in_group`]/
//! [`ToggleButtonView::last_in_group`] are plumbing for the upcoming
//! button-group rework, exercised directly here. When connected, the two
//! *outer* corners (the group-facing side of a first/last member) stay pinned
//! at the pill radius (`height / 2`) in every state; the two *inner*
//! (adjacent-member) corners instead resolve per state —
//! [`CONNECTED_INNER_RADIUS`] (6dp, unthemed) resting,
//! [`CONNECTED_PRESSED_INNER_RADIUS`] (2dp, unthemed) pressed, the per-size
//! [`ToggleButtonSize::hovered_radius`] token hovered — mirroring
//! `m3e_toggle_button_shape.dart`'s `_resolveConnectedTargetRadius`. A
//! **checked, connected, non-pressed/non-hovered** member is a special case:
//! all four corners go fully round (`checkedConnectedShape`), not the
//! per-size square token a standalone checked member uses — the reference's
//! own asymmetry between the standalone and connected checked shapes,
//! transcribed rather than "corrected". `is_first_in_group`/`is_last_in_group`
//! resolve independently, so a single-member group (`true`/`true`) is all
//! outer corners, matching [`mod@crate::button_group`]'s own single-member
//! convention. Painted with [`frust::authoring::CornerRadii`] via
//! [`PaintScene::fill_rounded_rect_radii`] rather than
//! [`mod@crate::button_group`]'s own `kurbo`-`RoundedRect`-plus-`fill_path`
//! route — both reach the same pixels; this module takes the newer
//! per-corner-native paint call directly. This framework has no
//! `Directionality`/RTL layout yet, so — like `button_group`'s own
//! `member_radii` — "first"/"last" map onto "left"/"right" unconditionally,
//! with no mirrored-in-RTL case to port.
//!
//! # Icon/label swap on checked
//!
//! [`ToggleButtonView::icon`]/[`ToggleButtonView::checked_icon`] and
//! [`ToggleButtonView::label`]/[`ToggleButtonView::checked_label`] each fall
//! back to their unchecked sibling when the checked-state one is unset
//! (`_effectiveIcon`/`_effectiveLabel`, `m3e_toggle_button_state.dart:28`-
//! `:48`) — all four slots are optional, so a toggle button may be icon-only,
//! label-only, both, or (checked-state-only) neither. **Porting decision:**
//! the reference cross-fades and horizontally slides between the unchecked
//! and checked label (`_buildAnimatedLabelSlot`, a second, independent
//! spring) when exactly one of `icon`/`checkedIcon` is paired with exactly
//! one of `label`/`checkedLabel` set on the opposite side
//! (`_hasDistinctLabelStates`). That second animation channel is out of scope
//! here: the effective icon/label swap immediately on the checked flip the
//! same frame it is confirmed, with no cross-fade — a reasonable v1 cut this
//! module's tests pin directly rather than leave undocumented.
//!
//! # Long-press: fires, but on the next pointer event, not immediately
//!
//! [`ToggleButtonView::on_long_press`] fires once a capture has been held past
//! [`LONG_PRESS_MS`] (500ms — same value and citation
//! `frust_widgets::gesture`'s `GestureDetector` uses: **community-approximate**,
//! Android's `ViewConfiguration` defaults to ~400-500ms, iOS's
//! `UILongPressGestureRecognizer.minimumPressDuration` to 0.5s). `paint` is
//! the only pass carrying a clock in this framework, so it latches an
//! `elapsed` flag the same way `GestureDetector` does — but that widget then
//! fires *immediately*, with no further pointer event, via
//! `frust_core::mark_pending_result_flush` forcing an extra rebuild pass. That
//! function is not part of `frust::authoring`'s re-exported surface (a
//! design-system plugin depends on the `frust` facade alone, never
//! `frust-core` directly in production — `docs/PLUGINS_CODE_STANDARDS.md`'s
//! Plugin Conventions), so it is unreachable here. This widget instead fires
//! on the next pointer event that arrives after the threshold — the same
//! *move-arrival* delivery `GestureDetector` itself falls back to when no
//! long-press handler is wired for its own `on_hold_progress`-only case — with
//! an `Up` as the final fallback if no `Move` ever arrives. A long-press and
//! the checked toggle resolve as mutually exclusive outcomes of the same
//! press, exactly like `GestureDetector`'s own tap/long-press arbitration: an
//! `Up` that already crossed the threshold fires the long-press, never the
//! toggle. Movement past [`frust::input::TOUCH_SLOP`] from the initiating
//! `Down` cancels the long-press candidacy outright (a drag, not a hold).
//!
//! # Haptics, focus ring, decoration/gradient seam
//!
//! [`ToggleButtonView::haptic`] defaults to [`HapticSignal::None`], fired
//! (when set) on the same up-inside edge that reports the checked toggle,
//! through [`MaterialHaptics::fire`] — the plain button's own convention
//! (`m3e_toggle_button_state.dart:120`'s `M3EHaptics.trigger` call site).
//! Neither a focus ring (`m3e_focus_ring.dart`, which the plain button ports)
//! nor the `M3EToggleButtonDecoration` gradient/override seam (the plain
//! button's still-unfilled `ButtonDecoration` seam) is ported in this v1 —
//! out of this task's acceptance scope; `PaintCtx::has_focus`/`is_hovered`
//! still drive the state-layer overlay correctly, only the extra ring stroke
//! is deferred.
//!
//! # Attribution
//!
//! See `plugins/material/NOTICE`'s "MIT License — Additional Copyright Holders
//! (Vendored Components)" section (Mudit Purohit / m3e_buttons) and its Module
//! Attribution Header Convention.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{FontWeight, LineHeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CornerRadii, CursorIcon,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role,
    SemanticsCtx, Toggled, View, Widget,
};
use frust::input::TOUCH_SLOP;
use frust::{AnimationController, FrameTime, SpringDesc, Theme};
use kurbo::{Point, Rect, RoundedRect, RoundedRectRadii, Shape, Size};
use peniko::{Brush, Color};

use crate::ButtonVariant;
use crate::interaction::{
    DISABLED_CONTAINER_OPACITY, DISABLED_CONTENT_OPACITY, HapticSignal, InteractionState,
    MaterialHaptics,
};
use crate::press::presses;

// ---- Metric tables (m3e_button_theme.dart, sm/md/lg rows only) ------------

/// Stroke width of an [`ButtonVariant::Outlined`] hairline, in logical px —
/// same value the plain button uses (Flutter's `BorderSide` default).
const OUTLINE_WIDTH: f64 = 1.0;

/// Flattening tolerance for the `kurbo` rounded-rect paths this module
/// strokes (matches [`mod@crate::button_group`]'s `PATH_TOLERANCE`).
const PATH_TOLERANCE: f64 = 0.1;

/// The ink every label run is *shaped* with; never painted — see
/// [`mod@crate::button`]'s `SHAPING_INK` for why (recolor-without-reshape).
const SHAPING_INK: Color = Color::BLACK;

/// One row of the per-size measurement table.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SizeMetrics {
    height: f64,
    h_padding: f64,
    icon_size: f64,
    icon_gap: f64,
}

/// One row of the M3 type scale, as `(size, line_height, letter_spacing,
/// weight)` — the unthemed fallback for a label role.
type TypeToken = (f32, f32, f32, FontWeight);

const LABEL_MEDIUM: TypeToken = (12.0, 16.0, 0.5, FontWeight::MEDIUM);
const LABEL_LARGE: TypeToken = (14.0, 20.0, 0.1, FontWeight::MEDIUM);
const TITLE_MEDIUM: TypeToken = (16.0, 24.0, 0.15, FontWeight::MEDIUM);

/// The toggle button's size tier — a three-row subset of the reference's
/// `M3EButtonSize` (`sm`/`md`/`lg` only). See the [module docs](self)' Sizes
/// table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToggleButtonSize {
    /// 40dp tall (the default).
    #[default]
    Sm,
    /// 56dp tall.
    Md,
    /// 96dp tall.
    Lg,
}

impl ToggleButtonSize {
    /// The size's row of `m3e_button_theme.dart:76`'s `_measurementsTable`.
    fn metrics(self) -> SizeMetrics {
        match self {
            // m3e_button_theme.dart:83
            ToggleButtonSize::Sm => SizeMetrics {
                height: 40.0,
                h_padding: 16.0,
                icon_size: 20.0,
                icon_gap: 8.0,
            },
            // m3e_button_theme.dart:89
            ToggleButtonSize::Md => SizeMetrics {
                height: 56.0,
                h_padding: 24.0,
                icon_size: 24.0,
                icon_gap: 8.0,
            },
            // m3e_button_theme.dart:95
            ToggleButtonSize::Lg => SizeMetrics {
                height: 96.0,
                h_padding: 48.0,
                icon_size: 32.0,
                icon_gap: 12.0,
            },
        }
    }

    /// The checked-state (square-family) resting radius
    /// (`m3e_button_theme.dart:44`'s `_squareRadiusTable`: 12/16/28 for
    /// sm/md/lg). Resolved from the theme's shape scale, whose
    /// `medium`/`large`/`extra_large` tokens coincide with that table
    /// exactly — the same resolution [`mod@crate::button`]'s own
    /// `ButtonSize::square_radius` performs; duplicated here since that
    /// method is private to the `button` module.
    fn square_radius(self, theme: Option<&Theme>) -> f64 {
        match theme {
            Some(theme) => match self {
                ToggleButtonSize::Sm => theme.shape.medium,
                ToggleButtonSize::Md => theme.shape.large,
                ToggleButtonSize::Lg => theme.shape.extra_large,
            },
            None => match self {
                ToggleButtonSize::Sm => 12.0,
                ToggleButtonSize::Md => 16.0,
                ToggleButtonSize::Lg => 28.0,
            },
        }
    }

    /// The pressed-state resting radius (`m3e_button_theme.dart:52`'s
    /// `_pressedRadiusTable`: 8/12/16 for sm/md/lg) — one shape-scale rung
    /// squarer than [`Self::square_radius`], and likewise theme-resolved.
    fn pressed_radius(self, theme: Option<&Theme>) -> f64 {
        match theme {
            Some(theme) => match self {
                ToggleButtonSize::Sm => theme.shape.small,
                ToggleButtonSize::Md => theme.shape.medium,
                ToggleButtonSize::Lg => theme.shape.large,
            },
            None => match self {
                ToggleButtonSize::Sm => 8.0,
                ToggleButtonSize::Md => 12.0,
                ToggleButtonSize::Lg => 16.0,
            },
        }
    }

    /// The hovered-state resting radius (`m3e_button_theme.dart:60`'s
    /// `_hoveredRadiusTable`: 10/14/22 for sm/md/lg) — the midpoint of its
    /// size's square and pressed radii, so (like the plain button's own
    /// table) it has no shape-scale token to resolve from and stays literal.
    fn hovered_radius(self) -> f64 {
        match self {
            ToggleButtonSize::Sm => 10.0,
            ToggleButtonSize::Md => 14.0,
            ToggleButtonSize::Lg => 22.0,
        }
    }

    /// The label's type role (`m3e_base_button_state.dart:169`, the mixin
    /// both the plain and toggle button share): `labelMedium`/`labelLarge`/
    /// `titleMedium` for sm/md/lg. Themed from [`frust::Theme`]'s type scale;
    /// unthemed from the M3 token literals above. The returned style always
    /// carries [`SHAPING_INK`].
    fn label_style(self, theme: Option<&Theme>) -> TextStyle {
        let mut style = match theme {
            Some(theme) => match self {
                ToggleButtonSize::Sm => theme.type_scale.label_medium.clone(),
                ToggleButtonSize::Md => theme.type_scale.label_large.clone(),
                ToggleButtonSize::Lg => theme.type_scale.title_medium.clone(),
            },
            None => {
                let (size, line_height, letter_spacing, weight) = match self {
                    ToggleButtonSize::Sm => LABEL_MEDIUM,
                    ToggleButtonSize::Md => LABEL_LARGE,
                    ToggleButtonSize::Lg => TITLE_MEDIUM,
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

// ---- Group-connection constants (m3e_toggle_button_group_theme.dart) ------

/// The connected-group inner-corner radius while resting, in logical px —
/// `M3EToggleButtonGroupTheme.connectedInnerRadius`'s default
/// (`m3e_toggle_button_group_theme.dart:39`). Unthemed: `frust::Theme` has no
/// matching shape-scale slot for this M3E-specific concept, so this stays a
/// plain constant until the button-group rework lands a themeable extension.
const CONNECTED_INNER_RADIUS: f64 = 6.0;
/// The connected-group inner-corner radius while pressed, in logical px —
/// `M3EToggleButtonGroupTheme.connectedPressedInnerRadius`'s default
/// (`m3e_toggle_button_group_theme.dart:40`). Same unthemed status as
/// [`CONNECTED_INNER_RADIUS`].
const CONNECTED_PRESSED_INNER_RADIUS: f64 = 2.0;

// ---- Colors (m3e_toggle_button_style.dart) ---------------------------------

/// Unthemed-fallback `primary` (a theme resolves `colors.primary`).
const PRIMARY: Color = Color::from_rgb8(0x67, 0x50, 0xA4);
/// Unthemed-fallback `on_primary`.
const ON_PRIMARY: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
/// Unthemed-fallback `on_surface_variant`.
const ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);
/// Unthemed-fallback `surface_container_highest`.
const SURFACE_CONTAINER_HIGHEST: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);
/// Unthemed-fallback `surface_container_low`.
const SURFACE_CONTAINER_LOW: Color = Color::from_rgb8(0xF7, 0xF2, 0xFA);
/// Unthemed-fallback `secondary_container`.
const SECONDARY_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback `on_secondary_container`.
const ON_SECONDARY_CONTAINER: Color = Color::from_rgb8(0x1D, 0x19, 0x2B);
/// Unthemed-fallback `outline`.
const OUTLINE: Color = Color::from_rgb8(0x79, 0x74, 0x7E);
/// Unthemed-fallback `on_surface` — the disabled roles' base color.
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed-fallback shadow color (opaque black at
/// `crate::tokens::elevation()`'s `0.3` alpha).
const FALLBACK_SHADOW_COLOR: Color = Color::new([0.0, 0.0, 0.0, 0.3]);

/// The nine color roles a toggle button reads, resolved once per pass from
/// the live theme or from this module's unthemed fallbacks.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Roles {
    primary: Color,
    on_primary: Color,
    on_surface_variant: Color,
    surface_container_highest: Color,
    surface_container_low: Color,
    secondary_container: Color,
    on_secondary_container: Color,
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
                    on_surface_variant: s.on_surface_variant,
                    surface_container_highest: s.surface_container_highest,
                    surface_container_low: s.surface_container_low,
                    secondary_container: s.secondary_container,
                    on_secondary_container: s.on_secondary_container,
                    outline: s.outline,
                    on_surface: s.on_surface,
                }
            }
            None => Self {
                primary: PRIMARY,
                on_primary: ON_PRIMARY,
                on_surface_variant: ON_SURFACE_VARIANT,
                surface_container_highest: SURFACE_CONTAINER_HIGHEST,
                surface_container_low: SURFACE_CONTAINER_LOW,
                secondary_container: SECONDARY_CONTAINER,
                on_secondary_container: ON_SECONDARY_CONTAINER,
                outline: OUTLINE,
                on_surface: ON_SURFACE,
            },
        }
    }
}

/// The resolved inks for one paint pass.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ToggleColors {
    /// Container fill (transparent for an unchecked Outlined/Text button).
    container: Color,
    /// Label + icon ink, and the state layer's tint.
    content: Color,
    /// The hairline, when the variant draws one.
    outline: Option<Color>,
}

/// `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Whether `variant` never paints a container even when checked, per the
/// reference's `_isTransparentStyle` (`m3e_toggle_button_style.dart:150`) —
/// note this is style-only, unlike the checked/unchecked container switch
/// itself: an Outlined toggle *does* paint `secondary_container` while
/// checked (see [`resolve_colors`]), so this flag only gates the *disabled*
/// container branch, matching the reference's own scope for it.
fn is_transparent(variant: ButtonVariant) -> bool {
    matches!(variant, ButtonVariant::Outlined | ButtonVariant::Text)
}

/// Resolve the container/content/outline inks for `variant` at `checked`.
///
/// Enabled follows `m3e_toggle_button_style.dart:118`
/// (`_resolvedForegroundColor`) and `:134` (`_resolvedBackgroundColor`) — see
/// the [module docs](self)' Variants × checked table. Disabled follows the
/// same `M3EButtonConstants` alphas the plain button uses: content at
/// [`DISABLED_CONTENT_OPACITY`] (38%), an opaque container at
/// [`DISABLED_CONTAINER_OPACITY`] (12%), both over `on_surface`, and a
/// transparent-style container stays transparent — checked-independent.
fn resolve_colors(
    theme: Option<&Theme>,
    variant: ButtonVariant,
    checked: bool,
    enabled: bool,
) -> ToggleColors {
    let roles = Roles::resolve(theme);

    if !enabled {
        return ToggleColors {
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

    let (content, container) = match variant {
        ButtonVariant::Filled => (
            if checked {
                roles.on_primary
            } else {
                roles.on_surface_variant
            },
            if checked {
                roles.primary
            } else {
                roles.surface_container_highest
            },
        ),
        ButtonVariant::Elevated => (
            if checked {
                roles.on_primary
            } else {
                roles.primary
            },
            if checked {
                roles.primary
            } else {
                roles.surface_container_low
            },
        ),
        ButtonVariant::Tonal => (
            if checked {
                roles.on_secondary_container
            } else {
                roles.on_surface_variant
            },
            if checked {
                roles.secondary_container
            } else {
                roles.surface_container_highest
            },
        ),
        ButtonVariant::Outlined => (
            if checked {
                roles.on_secondary_container
            } else {
                roles.on_surface
            },
            if checked {
                roles.secondary_container
            } else {
                Color::TRANSPARENT
            },
        ),
        ButtonVariant::Text => (
            if checked {
                roles.primary
            } else {
                roles.on_surface
            },
            Color::TRANSPARENT,
        ),
    };

    ToggleColors {
        container,
        content,
        outline: (variant == ButtonVariant::Outlined).then_some(roles.outline),
    }
}

/// The container elevation in dp for `variant` in the current state —
/// **not** checked-aware (`_buttonTheme.elevation(widget.style, states)` is
/// called with plain `WidgetState`s, never `.selected`,
/// `m3e_toggle_button_style.dart:188`-`:190`), so this is a byte-for-byte
/// duplicate of [`mod@crate::button`]'s own `elevation_dp` — private to that
/// module, hence the copy (see this module's Porting decision header).
fn elevation_dp(variant: ButtonVariant, enabled: bool, pressed: bool, hovered: bool) -> f64 {
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
/// `None` for a flat button — the same M3 elevation-level resolution
/// [`mod@crate::button`]'s own `resolve_shadow` performs (duplicated here for
/// the same private-module reason as [`elevation_dp`]).
fn resolve_shadow(theme: Option<&Theme>, dp: f64) -> Option<(f64, f64, Color)> {
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

// ---- The checked-shape + padding spring (sibling of button::motion) -------

/// The morph's spring: stiffness 380, damping ratio 0.55, mass 1 — the exact
/// [`crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`] preset. See
/// the [module docs](self)' Checked shape morph section.
const SHAPE_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 380.0,
    damping_ratio: 0.55,
};

/// Nominal period seeding the morph [`AnimationController`]'s clock — spring-
/// driven via `fling`, so this duration only backs construction (mirrors
/// `button::motion::PRESS_ANIM_PERIOD`).
const ANIM_PERIOD: Duration = Duration::from_millis(300);

/// Launch velocity (progress-units/sec) handed to each leg's
/// [`AnimationController::fling`] — the same modest kick every spring in this
/// crate uses so a retarget reads snappy rather than creeping off zero.
const FLING_VELOCITY: f64 = 4.0;

/// The dead band (logical px) a new target must exceed (per corner, per
/// padding edge) before it starts a new spring leg — mirrors
/// `button::motion::RETARGET_TOLERANCE` (the reference's
/// `kSpringRetargetTolerance`).
const RETARGET_TOLERANCE: f64 = 0.1;

/// How far past its target the spring's progress is allowed to read —
/// mirrors `button::motion::OVERSHOOT_LIMIT` (the reference's
/// `rawFactor.clamp(0.0, 1.5)`).
const OVERSHOOT_LIMIT: f64 = 1.5;

/// Linear interpolation between `from` and `to` at `t` (unclamped — the
/// spring's overshoot rides past `t = 1`).
fn lerp(from: f64, to: f64, t: f64) -> f64 {
    from + (to - from) * t
}

/// The button's internal content padding — symmetric horizontal only (a
/// toggle button's height is fixed by its size token), mirroring
/// `button::motion::ContentPadding`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ContentPadding {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}

impl ContentPadding {
    const fn symmetric(horizontal: f64) -> Self {
        Self {
            left: horizontal,
            right: horizontal,
            top: 0.0,
            bottom: 0.0,
        }
    }

    fn lerp(self, to: Self, t: f64) -> Self {
        Self {
            left: lerp(self.left, to.left, t),
            right: lerp(self.right, to.right, t),
            top: lerp(self.top, to.top, t),
            bottom: lerp(self.bottom, to.bottom, t),
        }
    }

    fn matches(self, other: Self) -> bool {
        (self.left - other.left).abs() <= RETARGET_TOLERANCE
            && (self.right - other.right).abs() <= RETARGET_TOLERANCE
            && (self.top - other.top).abs() <= RETARGET_TOLERANCE
            && (self.bottom - other.bottom).abs() <= RETARGET_TOLERANCE
    }
}

/// Whether every corner of `a` is within [`RETARGET_TOLERANCE`] of `b`'s.
fn corners_match(a: CornerRadii, b: CornerRadii) -> bool {
    (a.top_left - b.top_left).abs() <= RETARGET_TOLERANCE
        && (a.top_right - b.top_right).abs() <= RETARGET_TOLERANCE
        && (a.bottom_right - b.bottom_right).abs() <= RETARGET_TOLERANCE
        && (a.bottom_left - b.bottom_left).abs() <= RETARGET_TOLERANCE
}

/// Per-corner interpolation of `from` toward `to` at `t`, each corner floored
/// at `0` (the reference's `v < 0 ? 0.0 : v` corner guard).
fn lerp_corners(from: CornerRadii, to: CornerRadii, t: f64) -> CornerRadii {
    CornerRadii::new(
        lerp(from.top_left, to.top_left, t).max(0.0),
        lerp(from.top_right, to.top_right, t).max(0.0),
        lerp(from.bottom_right, to.bottom_right, t).max(0.0),
        lerp(from.bottom_left, to.bottom_left, t).max(0.0),
    )
}

/// The checked-shape + padding spring [`ToggleButtonWidget`] owns — a sibling
/// of `button::motion::RadiusPaddingMotion`, generalized from one radius to
/// four independent corners. See the [module docs](self)' Checked shape
/// morph section for the full contract and why this is a copy rather than a
/// reuse.
#[derive(Clone, Copy, Debug)]
struct ShapeMotion {
    from_radii: CornerRadii,
    to_radii: CornerRadii,
    from_padding: ContentPadding,
    to_padding: ContentPadding,
    anim: AnimationController,
    /// Whether a first target has been seeded — the mount frame snaps rather
    /// than springs.
    seeded: bool,
}

impl ShapeMotion {
    /// An unseeded morph resting at every corner `0` and zero padding — the
    /// first [`Self::retarget`] snaps to its argument instead of springing.
    fn new() -> Self {
        let zero_pad = ContentPadding {
            left: 0.0,
            right: 0.0,
            top: 0.0,
            bottom: 0.0,
        };
        Self {
            from_radii: CornerRadii::default(),
            to_radii: CornerRadii::default(),
            from_padding: zero_pad,
            to_padding: zero_pad,
            anim: AnimationController::new(ANIM_PERIOD),
            seeded: false,
        }
    }

    /// Aim the morph at `radii`/`padding`, returning whether this actually
    /// started a new spring leg. Same three-outcome contract as
    /// `button::motion::RadiusPaddingMotion::retarget`: unseeded snaps, a
    /// within-[`RETARGET_TOLERANCE`] target is ignored, anything else springs
    /// from the value being painted *right now* (mid-flight continuity).
    fn retarget(&mut self, radii: CornerRadii, padding: ContentPadding) -> bool {
        if !self.seeded {
            self.snap_to(radii, padding);
            return false;
        }
        if corners_match(self.to_radii, radii) && self.to_padding.matches(padding) {
            return false;
        }
        self.from_radii = self.radii();
        self.from_padding = self.padding();
        self.to_radii = radii;
        self.to_padding = padding;
        self.anim = AnimationController::new(ANIM_PERIOD);
        self.anim.fling(FLING_VELOCITY, SHAPE_SPRING);
        true
    }

    /// Pin both channels to `radii`/`padding` with no motion at all,
    /// marking the morph seeded.
    fn snap_to(&mut self, radii: CornerRadii, padding: ContentPadding) {
        self.from_radii = radii;
        self.to_radii = radii;
        self.from_padding = padding;
        self.to_padding = padding;
        self.anim = AnimationController::new(ANIM_PERIOD);
        self.seeded = true;
    }

    /// Advance the spring to frame time `now`, returning whether it is still
    /// animating.
    fn advance(&mut self, now: FrameTime) -> bool {
        self.anim.advance(now)
    }

    /// Whether a first target has been seeded.
    fn is_seeded(&self) -> bool {
        self.seeded
    }

    /// The progress factor this frame paints at.
    fn factor(&self) -> f64 {
        let raw = self.anim.value();
        if raw.is_finite() {
            raw.clamp(0.0, OVERSHOOT_LIMIT)
        } else {
            0.0
        }
    }

    /// The per-corner radii to paint this frame.
    fn radii(&self) -> CornerRadii {
        lerp_corners(self.from_radii, self.to_radii, self.factor())
    }

    /// The content padding to lay out with this frame.
    fn padding(&self) -> ContentPadding {
        self.from_padding.lerp(self.to_padding, self.factor())
    }

    /// The radii the morph is currently springing *toward*.
    #[cfg(test)]
    fn target_radii(&self) -> CornerRadii {
        self.to_radii
    }
}

// ---- The label run (sibling of button::core::LabelRun) --------------------

/// The toggle button's own lazily-shaped label run, re-brushed at paint
/// time — a sibling of `button::core::LabelRun` (private to that module) for
/// the same reason as [`elevation_dp`].
///
/// **Shaped at its natural width and nothing else**, which is where it parts
/// company with that sibling: the plain button fits its label to the room
/// left over (`Flexible` + `overflow: ellipsis`,
/// `m3e_button_state.dart:28`-`:35`), while a toggle button's content row is
/// laid out unbounded and *clipped* by a box too small for it
/// (`FittedBox(fit: BoxFit.none, clipBehavior: Clip.hardEdge)`,
/// `m3e_toggle_button_content.dart:103`-`:110`). So this run carries no
/// width-keyed cache and no ellipsis path at all — see the [module docs](self)'
/// clipping section.
struct LabelRun {
    content: String,
    layout: Option<TextLayout>,
    shaped_for: Option<TextStyle>,
}

impl LabelRun {
    fn new(content: String) -> Self {
        Self {
            content,
            layout: None,
            shaped_for: None,
        }
    }

    fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.layout = None;
            self.shaped_for = None;
        }
    }

    fn content(&self) -> &str {
        &self.content
    }

    /// Shape (or reuse) the run in `style` at its natural width, returning its
    /// measured size. Nothing but the style can invalidate it.
    fn shape(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if let Some(cached) = &self.layout
            && self.shaped_for.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, None);
        let size = laid.size();
        self.layout = Some(laid);
        self.shaped_for = Some(style.clone());
        size
    }

    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let Some(layout) = &self.layout else {
            return;
        };
        for mut run in layout.to_scene_runs(origin) {
            run.brush = Brush::Solid(color);
            scene.draw_glyph_run(run);
        }
    }
}

// ---- Geometry helpers -------------------------------------------------------

/// Whether local point `pos` is inside a widget of `size`.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// Per-corner outer/inner split for a connected-group member — outer on the
/// group-facing side (first gets outer on the left, last on the right),
/// inner on the adjacent side. Mirrors [`crate::button_group::member_radii`]'s
/// left/right split, generalized to independently-set first/last flags.
fn horizontal_radii(is_first: bool, is_last: bool, outer: f64, inner: f64) -> CornerRadii {
    let left = if is_first { outer } else { inner };
    let right = if is_last { outer } else { inner };
    CornerRadii::new(left, right, right, left)
}

/// The rounded rect to *stroke* for a `width`-wide hairline lying fully
/// inside a container of `size` with per-corner `radii` — a stroke is
/// centered on its path, so both the rect and every radius pull in by half
/// the width (the per-corner generalization of [`mod@crate::button`]'s
/// `inset_stroke_rect`).
fn inset_stroke_rrect(size: Size, radii: CornerRadii, width: f64) -> RoundedRect {
    let half = width / 2.0;
    let r = RoundedRectRadii::new(
        (radii.top_left - half).max(0.0),
        (radii.top_right - half).max(0.0),
        (radii.bottom_right - half).max(0.0),
        (radii.bottom_left - half).max(0.0),
    );
    RoundedRect::from_rect(
        Rect::new(half, half, size.width - half, size.height - half),
        r,
    )
}

// ---- The declarative view ---------------------------------------------------

/// A view-held, typed checked-change callback (erased on build).
type OnCheckedChange<State> = Rc<dyn Fn(&mut State, bool)>;
/// A view-held, typed long-press callback (erased on build).
type OnLongPress<State> = Rc<dyn Fn(&mut State)>;

/// A declarative M3 Expressive toggle button. See the [module docs](self).
pub struct ToggleButtonView<State: 'static> {
    checked: bool,
    on_checked_change: OnCheckedChange<State>,
    variant: ButtonVariant,
    size: ToggleButtonSize,
    enabled: bool,
    icon: Option<AnyView<State>>,
    checked_icon: Option<AnyView<State>>,
    label: Option<String>,
    checked_label: Option<String>,
    is_group_connected: bool,
    is_first_in_group: bool,
    is_last_in_group: bool,
    haptic: HapticSignal,
    on_long_press: Option<OnLongPress<State>>,
}

impl<State: 'static> ToggleButtonView<State> {
    /// The icon this frame should show, given `checked`
    /// (`_effectiveIcon`).
    fn effective_icon_view(&self) -> Option<&AnyView<State>> {
        if self.checked {
            self.checked_icon.as_ref().or(self.icon.as_ref())
        } else {
            self.icon.as_ref()
        }
    }

    /// The label text this frame should show, given `checked`
    /// (`_effectiveLabel`).
    fn effective_label_text(&self) -> Option<&str> {
        if self.checked {
            self.checked_label.as_deref().or(self.label.as_deref())
        } else {
            self.label.as_deref()
        }
    }
}

/// Create a filled (highest-emphasis) toggle button reporting the requested
/// checked value through `on_checked_change` on release inside its bounds —
/// the reference's default `M3EToggleButton`/`M3EToggleButton.filled`.
///
/// A controlled component (see the [module docs](self)): `checked` never
/// self-mutates. Chain [`ToggleButtonView::icon`]/[`ToggleButtonView::label`]
/// (and their `checked_*` siblings) to add content, and
/// [`ToggleButtonView::size`]/[`ToggleButtonView::variant`] for the other
/// tiers.
pub fn toggle_button<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_checked_change: F,
) -> ToggleButtonView<State> {
    ToggleButtonView {
        checked,
        on_checked_change: Rc::new(on_checked_change),
        variant: ButtonVariant::default(),
        size: ToggleButtonSize::default(),
        enabled: true,
        icon: None,
        checked_icon: None,
        label: None,
        checked_label: None,
        is_group_connected: false,
        is_first_in_group: true,
        is_last_in_group: true,
        haptic: HapticSignal::None,
        on_long_press: None,
    }
}

/// PascalCase alias for [`toggle_button`], matching this catalog's view-fn
/// vocabulary (`Button`, `ButtonGroup`, …).
#[allow(non_snake_case)]
pub fn ToggleButton<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_checked_change: F,
) -> ToggleButtonView<State> {
    toggle_button(checked, on_checked_change)
}

/// Create a filled toggle button — [`toggle_button`] under the name the
/// reference's `M3EToggleButton.filled` constructor uses.
pub fn filled_toggle_button<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_checked_change: F,
) -> ToggleButtonView<State> {
    toggle_button(checked, on_checked_change)
}

/// Create a tonal toggle button — `M3EToggleButton.tonal`.
pub fn tonal_toggle_button<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_checked_change: F,
) -> ToggleButtonView<State> {
    toggle_button(checked, on_checked_change).variant(ButtonVariant::Tonal)
}

/// Create an elevated toggle button — `M3EToggleButton.elevated`.
pub fn elevated_toggle_button<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_checked_change: F,
) -> ToggleButtonView<State> {
    toggle_button(checked, on_checked_change).variant(ButtonVariant::Elevated)
}

/// Create an outlined toggle button — `M3EToggleButton.outlined`.
pub fn outlined_toggle_button<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_checked_change: F,
) -> ToggleButtonView<State> {
    toggle_button(checked, on_checked_change).variant(ButtonVariant::Outlined)
}

/// Create a text toggle button — `M3EToggleButton.text`.
pub fn text_toggle_button<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_checked_change: F,
) -> ToggleButtonView<State> {
    toggle_button(checked, on_checked_change).variant(ButtonVariant::Text)
}

impl<State: 'static> ToggleButtonView<State> {
    /// Set the container treatment (the [module docs](self)' Variants
    /// table).
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set the size tier (the [module docs](self)' Sizes table).
    pub fn size(mut self, size: ToggleButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Whether the toggle button accepts a press. A disabled button paints
    /// the 38%/12% disabled roles and reports disabled semantics.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Icon shown while unchecked (and while checked too, if
    /// [`Self::checked_icon`] is unset).
    pub fn icon(mut self, icon: impl View<State>) -> Self {
        self.icon = Some(AnyView::new(icon));
        self
    }

    /// Icon shown while checked. Falls back to [`Self::icon`] when unset.
    pub fn checked_icon(mut self, icon: impl View<State>) -> Self {
        self.checked_icon = Some(AnyView::new(icon));
        self
    }

    /// Label shown while unchecked (and while checked too, if
    /// [`Self::checked_label`] is unset).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Label shown while checked. Falls back to [`Self::label`] when unset.
    pub fn checked_label(mut self, label: impl Into<String>) -> Self {
        self.checked_label = Some(label.into());
        self
    }

    /// Whether this button is part of a connected group — the [module
    /// docs](self)' Group-connection corner asymmetry section. Plumbing for
    /// the upcoming button-group rework.
    pub fn group_connected(mut self, connected: bool) -> Self {
        self.is_group_connected = connected;
        self
    }

    /// Whether this is the first (leading) member of a connected group —
    /// only consulted while [`Self::group_connected`] is `true`. Defaults to
    /// `true` (a standalone button behaves as if it were both ends).
    pub fn first_in_group(mut self, first: bool) -> Self {
        self.is_first_in_group = first;
        self
    }

    /// Whether this is the last (trailing) member of a connected group —
    /// only consulted while [`Self::group_connected`] is `true`. Defaults to
    /// `true`.
    pub fn last_in_group(mut self, last: bool) -> Self {
        self.is_last_in_group = last;
        self
    }

    /// The haptic signal to fire on a confirmed checked-toggle. Defaults to
    /// [`HapticSignal::None`] (the [module docs](self)' Haptics section).
    pub fn haptic(mut self, haptic: HapticSignal) -> Self {
        self.haptic = haptic;
        self
    }

    /// Fire `on_long_press` when a press is held past the threshold — the
    /// [module docs](self)' Long-press section documents the exact firing
    /// contract (deferred to the next pointer event, not immediate).
    pub fn on_long_press<F: Fn(&mut State) + 'static>(mut self, on_long_press: F) -> Self {
        self.on_long_press = Some(Rc::new(on_long_press));
        self
    }
}

// ---- The retained widget -----------------------------------------------------

/// Latched long-press timing state — see the [module docs](self)' Long-press
/// section.
#[derive(Debug, Default, Clone, Copy)]
struct LongPressState {
    /// The frame time the capture started, seeded on the first `paint` after
    /// a `Down`.
    press_start: Option<FrameTime>,
    /// Whether the hold has crossed [`LONG_PRESS_MS`] — latched in `paint`,
    /// the only pass with a clock.
    elapsed: bool,
    /// Whether the callback has already fired for this press (fire-exactly-
    /// once).
    fired: bool,
}

impl LongPressState {
    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The press duration after which a held toggle-button press becomes a
/// long-press. **Community-approximate** — see the [module docs](self)'
/// Long-press section for the same citation `frust_widgets::gesture`'s
/// `GestureDetector` uses.
const LONG_PRESS_MS: f64 = 500.0;

/// The retained widget for a [`ToggleButtonView`]. See the [module
/// docs](self).
pub struct ToggleButtonWidget {
    variant: ButtonVariant,
    size: ToggleButtonSize,
    enabled: bool,
    /// The app-confirmed checked value (controlled — never self-mutated).
    checked: bool,
    /// The currently-effective label run (already resolved for `checked`).
    label: LabelRun,
    label_origin: Point,
    /// Whether the last layout's content row is wider than the box it was
    /// constrained into, so `paint` must clip it (`Clip.hardEdge`).
    content_clipped: bool,
    icon: Option<ChildPod>,
    is_group_connected: bool,
    is_first_in_group: bool,
    is_last_in_group: bool,
    haptic: HapticSignal,
    /// Hover/focus/pressed tracking feeding the state-layer overlay and the
    /// morph's target radii.
    state: InteractionState,
    /// Whether this widget holds the pointer capture a `Down` took.
    captured: bool,
    /// Where the capturing `Down` landed, in local coordinates — the
    /// long-press slop reference point.
    down_pos: Point,
    /// The checked-shape + padding press morph.
    motion: ShapeMotion,
    long_press: LongPressState,
    on_checked_change: frust::authoring::ErasedArgCallback<bool>,
    on_long_press: Option<frust::authoring::ErasedCallback>,
}

impl ToggleButtonWidget {
    /// Whether the widget currently has an effective label (drives the
    /// halved-padding rule, `_hasLabel`).
    fn has_label(&self) -> bool {
        !self.label.content().is_empty()
    }

    /// The horizontal content padding for the current label presence —
    /// `_buildWidget`'s `hPad = _hasLabel ? m.hPadding : m.hPadding / 2`.
    fn h_padding(&self, metrics: &SizeMetrics) -> f64 {
        if self.has_label() {
            metrics.h_padding
        } else {
            metrics.h_padding / 2.0
        }
    }

    /// This frame's target corner radii, dispatching to the standalone or
    /// connected resolver. See the [module docs](self)' Checked shape morph
    /// and Group-connection sections.
    fn target_radii(&self, theme: Option<&Theme>, height: f64) -> CornerRadii {
        let pill = height / 2.0;
        if self.is_group_connected {
            self.connected_target_radii(theme, pill)
        } else {
            self.standalone_target_radii(theme, pill)
        }
    }

    /// `m3e_toggle_button_shape.dart`'s `_resolveStandaloneTargetRadius`:
    /// `pressed > hovered > checked > rest` precedence, uniform across all
    /// four corners.
    fn standalone_target_radii(&self, theme: Option<&Theme>, pill: f64) -> CornerRadii {
        if self.enabled && self.state.pressed {
            return CornerRadii::uniform(self.size.pressed_radius(theme));
        }
        if self.enabled && self.state.hovered {
            return CornerRadii::uniform(self.size.hovered_radius());
        }
        if self.checked {
            return CornerRadii::uniform(self.size.square_radius(theme));
        }
        CornerRadii::uniform(pill)
    }

    /// `m3e_toggle_button_shape.dart`'s `_resolveConnectedTargetRadius`:
    /// outer corners (group-facing) stay pinned at `pill` in every state;
    /// inner corners resolve per state, except a checked-and-otherwise-idle
    /// member which goes fully round on *every* corner
    /// (`checkedConnectedShape`).
    /// `theme` is unused today: every branch here reads either a plain
    /// constant or [`ToggleButtonSize::hovered_radius`] (itself literal-only,
    /// like the plain button's own table). Kept in the signature — matching
    /// [`Self::standalone_target_radii`]'s — as the seam a themeable
    /// `M3EToggleButtonGroupTheme` extension will read from once the
    /// button-group rework lands (see [`CONNECTED_INNER_RADIUS`]'s doc).
    fn connected_target_radii(&self, _theme: Option<&Theme>, outer: f64) -> CornerRadii {
        if self.enabled && self.state.pressed {
            return horizontal_radii(
                self.is_first_in_group,
                self.is_last_in_group,
                outer,
                CONNECTED_PRESSED_INNER_RADIUS,
            );
        }
        if self.enabled && self.state.hovered {
            return horizontal_radii(
                self.is_first_in_group,
                self.is_last_in_group,
                outer,
                self.size.hovered_radius(),
            );
        }
        if self.checked {
            return CornerRadii::uniform(outer);
        }
        horizontal_radii(
            self.is_first_in_group,
            self.is_last_in_group,
            outer,
            CONNECTED_INNER_RADIUS,
        )
    }
}

impl<State: 'static> View<State> for ToggleButtonView<State> {
    type Element = ToggleButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ToggleButtonWidget {
        let icon = self
            .effective_icon_view()
            .map(|icon| frust::authoring::build_child(icon, ctx));
        let mut label = LabelRun::new(String::new());
        if let Some(text) = self.effective_label_text() {
            label.set_content(text);
        }
        ToggleButtonWidget {
            variant: self.variant,
            size: self.size,
            enabled: self.enabled,
            checked: self.checked,
            label,
            label_origin: Point::ZERO,
            content_clipped: false,
            icon,
            is_group_connected: self.is_group_connected,
            is_first_in_group: self.is_first_in_group,
            is_last_in_group: self.is_last_in_group,
            haptic: self.haptic,
            state: InteractionState::new(),
            captured: false,
            down_pos: Point::ZERO,
            motion: ShapeMotion::new(),
            long_press: LongPressState::default(),
            on_checked_change: frust::authoring::erase_callback_arg(&self.on_checked_change),
            on_long_press: self
                .on_long_press
                .as_ref()
                .map(frust::authoring::erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ToggleButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_checked_change = frust::authoring::erase_callback_arg(&self.on_checked_change);
        element.on_long_press = self
            .on_long_press
            .as_ref()
            .map(frust::authoring::erase_callback);
        element.haptic = self.haptic;
        let mut flags = ChangeFlags::NONE;

        if prev.checked != self.checked {
            element.checked = self.checked;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled {
                // A button disabled mid-press keeps neither the press, the
                // capture, nor a pending long-press candidacy.
                element.state.set_pressed(false);
                element.captured = false;
                element.long_press.clear();
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.is_group_connected != self.is_group_connected {
            element.is_group_connected = self.is_group_connected;
            flags |= ChangeFlags::PAINT;
        }
        if prev.is_first_in_group != self.is_first_in_group {
            element.is_first_in_group = self.is_first_in_group;
            flags |= ChangeFlags::PAINT;
        }
        if prev.is_last_in_group != self.is_last_in_group {
            element.is_last_in_group = self.is_last_in_group;
            flags |= ChangeFlags::PAINT;
        }

        let prev_label = prev.effective_label_text();
        let next_label = self.effective_label_text();
        if prev_label != next_label {
            element.label.set_content(next_label.unwrap_or(""));
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        match (prev.effective_icon_view(), self.effective_icon_view()) {
            (None, None) => {}
            (Some(p), Some(n)) => {
                let pod = element.icon.as_mut().expect("icon pod present");
                flags |= frust::authoring::rebuild_child(p, n, pod, ctx);
            }
            (None, Some(n)) => {
                element.icon = Some(frust::authoring::build_child(n, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(p), None) => {
                let mut pod = element.icon.take().expect("icon pod present");
                frust::authoring::teardown_child(p, &mut pod, ctx);
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }

        flags
    }

    fn teardown(&self, element: &mut ToggleButtonWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(icon_view), Some(pod)) = (self.effective_icon_view(), element.icon.as_mut()) {
            frust::authoring::teardown_child(icon_view, pod, ctx);
        }
    }
}

impl Widget for ToggleButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let metrics = self.size.metrics();
        let (label_style, h_padding) = {
            let theme = Theme::from_layout_ctx(ctx);
            (self.size.label_style(theme), self.h_padding(&metrics))
        };
        if !self.motion.is_seeded() {
            // The mount frame snaps; only a later target change springs.
            let theme = Theme::from_layout_ctx(ctx);
            let resting = self.target_radii(theme, metrics.height);
            self.motion
                .snap_to(resting, ContentPadding::symmetric(h_padding));
        }
        // The padding target is a pure function of `(self.size, has_label)`
        // — unlike the radius channel, which also depends on interaction
        // state only `paint` tracks — so `layout` resolves it directly
        // (`h_padding`, just above) rather than reading it back off the
        // morph. Reading `self.motion.padding()` here would lag a runtime
        // size or label-presence change by a whole pass: `layout` always
        // runs before `paint` in a frame, and `paint` below is the only
        // place that ever calls `retarget` with the new padding — its bare
        // `request_frame` (never `request_layout`) leaves the stale width
        // standing until an unrelated layout dirty happens along (the exact
        // hazard `crate::button::core`'s identical fix names). Both props
        // that move `h_padding` already raise `ChangeFlags::LAYOUT` on
        // `rebuild` (`size`, and the label text whose emptiness
        // `has_label` reads), so the value computed here is always
        // immediately correct, with no in-flight frame that would need its
        // own `request_layout`.
        let padding = ContentPadding::symmetric(h_padding);

        let icon_size = self.icon.as_mut().map(|pod| {
            pod.layout_child(
                ctx,
                &BoxConstraints::tight(Size::new(metrics.icon_size, metrics.icon_size)),
            )
        });
        let icon_width = icon_size.map_or(0.0, |s| s.width);
        let has_label = self.has_label();
        // The gap exists only between an icon and a label.
        let gap = if icon_size.is_some() && has_label {
            metrics.icon_gap
        } else {
            0.0
        };

        // The label is shaped once, at its natural width, whatever the
        // incoming constraint: a box too small for the content covers it
        // rather than re-fitting it (see the [module docs](self)' clipping
        // section, and `paint`, which applies that clip).
        let label_size = self.label.shape(ctx, &label_style);

        let content_width = icon_width + gap + label_size.width;
        // No minimum-width floor — see the [module docs](self)' Sizes
        // section for why this differs from the plain button.
        let width = content_width + padding.left + padding.right;
        let size = bc.constrain(Size::new(width, metrics.height));

        // The content row is centered as a whole, icon always leading (a
        // toggle button has no `IconAlignment` concept). Content wider than
        // the box centers too — `FittedBox`'s default `Alignment.center`, so
        // the overflow is split evenly and clipped off both ends rather than
        // pinned to the leading edge.
        let content_x = (size.width - content_width) / 2.0;
        self.content_clipped = content_width > size.width;
        let icon_x = content_x;
        let label_x = content_x + icon_width + gap;
        if let Some(pod) = self.icon.as_mut() {
            let icon_height = icon_size.map_or(0.0, |s| s.height);
            pod.set_origin(Point::new(icon_x, (size.height - icon_height) / 2.0));
        }
        self.label_origin = Point::new(label_x, (size.height - label_size.height) / 2.0);

        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();

        // Authoritative hover/focus reads, self-correcting the latched flags
        // (`docs/CODE_STANDARDS.md`'s Interaction Semantics); both inert
        // while disabled, mirroring the reference's `effectivelyEnabled`
        // guards.
        self.state.set_hovered(self.enabled && ctx.is_hovered());
        self.state.set_focused(self.enabled && ctx.has_focus());

        let (colors, shadow, radii_target, h_padding) = {
            let theme = Theme::from_paint_ctx(ctx);
            let dp = elevation_dp(
                self.variant,
                self.enabled,
                self.state.pressed,
                self.state.hovered,
            );
            (
                resolve_colors(theme, self.variant, self.checked, self.enabled),
                resolve_shadow(theme, dp),
                self.target_radii(theme, size.height),
                self.h_padding(&self.size.metrics()),
            )
        };

        let padding = ContentPadding::symmetric(h_padding);
        if self.motion.retarget(radii_target, padding) {
            ctx.request_frame();
        }
        if self.motion.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        let radii = self.motion.radii();

        if let Some((blur, y_offset, shadow_color)) = shadow {
            // `draw_shadow` takes one scalar radius — the per-corner lowering
            // documented on `frust_scene::CornerRadii::largest`.
            scene.draw_shadow(
                Point::new(origin.x, origin.y + y_offset),
                size,
                radii.largest(),
                blur,
                shadow_color,
            );
        }

        if colors.container.components[3] > 0.0 {
            scene.fill_rounded_rect_radii(origin, size, radii, colors.container);
        }

        // The M3E state layer: `dragged > pressed > focused > hovered`
        // precedence. Mirrors the plain button's own choice to keep the
        // pressed step despite the reference suppressing it for its own
        // `InkSparkle` ripple — this framework has no ripple engine, so the
        // state layer *is* the press feedback here too.
        if self.enabled {
            let opacity = self.state.resolve_opacity();
            if opacity > 0.0 {
                scene.fill_rounded_rect_radii(
                    origin,
                    size,
                    radii,
                    with_alpha(colors.content, opacity),
                );
            }
        }

        if let Some(outline) = colors.outline {
            let path = inset_stroke_rrect(size, radii, OUTLINE_WIDTH).to_path(PATH_TOLERANCE);
            scene.stroke_path(origin, &path, OUTLINE_WIDTH, &Brush::Solid(outline));
        }

        // A content row wider than the box is clipped to it, never re-fitted
        // — `FittedBox`'s `Clip.hardEdge`. Pushed only when it actually
        // overflows: a clip layer is not free, and the common case does not
        // need one.
        if self.content_clipped {
            scene.push_clip(origin, size);
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
        if self.content_clipped {
            scene.pop_clip();
        }

        // Long-press threshold latch — `paint` is the only pass with a
        // clock. See the [module docs](self)' Long-press section.
        if self.enabled && self.captured && self.on_long_press.is_some() && !self.long_press.elapsed
        {
            let start = *self.long_press.press_start.get_or_insert(ctx.frame_time());
            let elapsed_ms = ctx.frame_time().saturating_sub(start).as_secs_f64() * 1000.0;
            if elapsed_ms >= LONG_PRESS_MS {
                self.long_press.elapsed = true;
            } else {
                ctx.request_frame();
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.enabled {
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
                self.down_pos = p.position;
                self.long_press.clear();
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let over = inside(p.position, ctx.size());
                if !self.captured {
                    // The hover pass: claim, latch, and let paint
                    // self-correct.
                    if over {
                        ctx.claim_hover();
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
                // Long-press slop check + fire-on-move-arrival (the [module
                // docs](self)' Long-press section).
                if (p.position - self.down_pos).hypot() > TOUCH_SLOP {
                    self.long_press.clear();
                } else if self.long_press.elapsed && !self.long_press.fired {
                    self.long_press.fired = true;
                    if let Some(cb) = self.on_long_press.as_mut() {
                        cb(ctx);
                    }
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // A held-past-threshold press resolves as a long-press,
                // never also as a checked toggle — mutually exclusive
                // outcomes of the same gesture (the [module docs](self)'
                // Long-press section). `elapsed` alone gates the toggle
                // (not `elapsed && !fired`): a press already resolved as a
                // long-press via a Move-arrival fire above must not *also*
                // toggle here just because it already fired once.
                if self.long_press.elapsed {
                    if !self.long_press.fired {
                        self.long_press.fired = true;
                        if let Some(cb) = self.on_long_press.as_mut() {
                            cb(ctx);
                        }
                    }
                } else if inside(p.position, ctx.size()) {
                    if self.haptic != HapticSignal::None {
                        MaterialHaptics::fire(self.haptic);
                    }
                    // Report the *requested* value; never self-toggle.
                    let requested = !self.checked;
                    (self.on_checked_change)(ctx, requested);
                }
                self.state.set_pressed(false);
                self.captured = false;
                self.long_press.clear();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.state.set_pressed(false);
                self.captured = false;
                self.long_press.clear();
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A toggle button reports as a plain `Button` carrying a `Toggled`
        // state (the ARIA `role="button" aria-pressed`/accesskit shape) —
        // accesskit has no dedicated "toggle button" role. The icon child is
        // not forwarded (decoration only), mirroring the plain button.
        ctx.push_node(Role::Button, |node| {
            if !self.label.content().is_empty() {
                node.set_label(self.label.content());
            }
            node.set_toggled(Toggled::from(self.checked));
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
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::{PointerButton, PointerEvent};
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;

    // ---- fixtures ---------------------------------------------------------

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    /// Build a widget from a view over any state type.
    fn build<S: 'static>(view: &ToggleButtonView<S>) -> ToggleButtonWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    /// Advance the widget's shape morph past its settling point.
    fn settle_morph(widget: &mut ToggleButtonWidget) {
        let mut t = 0.0;
        for _ in 0..600 {
            t += 1.0 / 60.0;
            if !widget.motion.advance(ft_secs(t)) {
                return;
            }
        }
        panic!("the shape morph never settled");
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch<S: 'static>(
        widget: &mut ToggleButtonWidget,
        state: &mut S,
        size: Size,
        event: &InputEvent,
    ) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, event);
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, CornerRadii, Color)>,
        strokes: Vec<(Point, f64, Color)>,
        runs: Vec<(Point, Color)>,
        /// Glyphs per painted run, in paint order — the only paint-side
        /// window onto how much of a label was shaped (`GlyphRun` carries
        /// resolved glyph ids, never the text).
        run_glyphs: Vec<usize>,
        /// `(origin, size)` of every clip pushed.
        clips: Vec<(Point, Size)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, radii: CornerRadii, color: Color) {
            self.rrects.push((o, s, radii, color));
        }
        fn stroke_path(&mut self, o: Point, _path: &kurbo::BezPath, width: f64, brush: &Brush) {
            self.strokes.push((o, width, solid(brush)));
        }
        fn push_clip(&mut self, o: Point, s: Size) {
            self.clips.push((o, s));
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            self.runs.push((Point::new(t.x, t.y), solid(&run.brush)));
            self.run_glyphs.push(run.glyphs.len());
        }
    }

    fn solid(brush: &Brush) -> Color {
        match brush {
            Brush::Solid(c) => *c,
            _ => Color::TRANSPARENT,
        }
    }

    fn layout_with(widget: &mut ToggleButtonWidget, max_width: f64, theme: Option<&Theme>) -> Size {
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

    /// Lay the widget out under the tight-main/loose-cross constraint a
    /// button group hands a member (`BoxConstraints.tightFor(width:)`,
    /// `m3e_toggle_button_group_render.dart:319`).
    fn layout_tight_main(widget: &mut ToggleButtonWidget, main: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        widget.layout(
            &mut lctx,
            &BoxConstraints::new(Size::new(main, 0.0), Size::new(main, 500.0)),
        )
    }

    fn paint_at(widget: &mut ToggleButtonWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        if let Some(theme) = theme {
            pctx = pctx.with_theme(theme as &dyn Any);
        }
        widget.paint(&mut pctx, &mut rec);
        rec
    }

    const ALL_SIZES: [ToggleButtonSize; 3] = [
        ToggleButtonSize::Sm,
        ToggleButtonSize::Md,
        ToggleButtonSize::Lg,
    ];

    const ALL_VARIANTS: [ButtonVariant; 5] = [
        ButtonVariant::Filled,
        ButtonVariant::Outlined,
        ButtonVariant::Tonal,
        ButtonVariant::Elevated,
        ButtonVariant::Text,
    ];

    // ---- Acceptance 1: metric + color tables, 5 variants x 3 sizes --------

    #[test]
    fn measurements_match_the_plain_buttons_sm_md_lg_rows() {
        let expected: [(ToggleButtonSize, f64, f64, f64, f64); 3] = [
            (ToggleButtonSize::Sm, 40.0, 16.0, 20.0, 8.0),
            (ToggleButtonSize::Md, 56.0, 24.0, 24.0, 8.0),
            (ToggleButtonSize::Lg, 96.0, 48.0, 32.0, 12.0),
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
        let theme = crate::baseline();
        let expected: [(ToggleButtonSize, f64, f64, f64); 3] = [
            (ToggleButtonSize::Sm, 12.0, 8.0, 10.0),
            (ToggleButtonSize::Md, 16.0, 12.0, 14.0),
            (ToggleButtonSize::Lg, 28.0, 16.0, 22.0),
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
    fn label_roles_match_the_shared_base_button_state_switch() {
        let theme = crate::baseline();
        let expected = [
            (ToggleButtonSize::Sm, theme.type_scale.label_medium.clone()),
            (ToggleButtonSize::Md, theme.type_scale.label_large.clone()),
            (ToggleButtonSize::Lg, theme.type_scale.title_medium.clone()),
        ];
        for (size, role) in expected {
            let style = size.label_style(Some(&theme));
            assert_eq!(style.size, role.size, "{size:?} size");
            assert_eq!(style.weight, role.weight, "{size:?} weight");
        }
    }

    #[test]
    fn enabled_colors_match_the_reference_variant_x_checked_table() {
        let theme = crate::baseline();
        let s = theme.scheme();
        // (variant, fg_unchecked, fg_checked, bg_unchecked, bg_checked, outline?)
        let expected: [(ButtonVariant, Color, Color, Color, Color, bool); 5] = [
            (
                ButtonVariant::Filled,
                s.on_surface_variant,
                s.on_primary,
                s.surface_container_highest,
                s.primary,
                false,
            ),
            (
                ButtonVariant::Outlined,
                s.on_surface,
                s.on_secondary_container,
                Color::TRANSPARENT,
                s.secondary_container,
                true,
            ),
            (
                ButtonVariant::Tonal,
                s.on_surface_variant,
                s.on_secondary_container,
                s.surface_container_highest,
                s.secondary_container,
                false,
            ),
            (
                ButtonVariant::Elevated,
                s.primary,
                s.on_primary,
                s.surface_container_low,
                s.primary,
                false,
            ),
            (
                ButtonVariant::Text,
                s.on_surface,
                s.primary,
                Color::TRANSPARENT,
                Color::TRANSPARENT,
                false,
            ),
        ];
        for (variant, fg_u, fg_c, bg_u, bg_c, has_outline) in expected {
            let unchecked = resolve_colors(Some(&theme), variant, false, true);
            let checked = resolve_colors(Some(&theme), variant, true, true);
            assert_eq!(unchecked.content, fg_u, "{variant:?} unchecked content");
            assert_eq!(unchecked.container, bg_u, "{variant:?} unchecked container");
            assert_eq!(checked.content, fg_c, "{variant:?} checked content");
            assert_eq!(checked.container, bg_c, "{variant:?} checked container");
            assert_eq!(
                unchecked.outline.is_some(),
                has_outline,
                "{variant:?} unchecked outline presence"
            );
            assert_eq!(
                checked.outline.is_some(),
                has_outline,
                "{variant:?} checked outline presence"
            );
            if has_outline {
                assert_eq!(unchecked.outline, Some(s.outline));
                assert_eq!(checked.outline, Some(s.outline));
            }
        }
    }

    #[test]
    fn unthemed_colors_fall_back_to_the_baseline_light_roles() {
        let unchecked = resolve_colors(None, ButtonVariant::Filled, false, true);
        assert_eq!(unchecked.content, ON_SURFACE_VARIANT);
        assert_eq!(unchecked.container, SURFACE_CONTAINER_HIGHEST);
        let checked = resolve_colors(None, ButtonVariant::Filled, true, true);
        assert_eq!(checked.content, ON_PRIMARY);
        assert_eq!(checked.container, PRIMARY);
    }

    /// The full 5 variants × 3 sizes × checked/unchecked matrix: every
    /// combination builds, lays out, and paints without panicking, with a
    /// finite, non-negative resting corner radius resolved for it — the
    /// acceptance criterion's "tables pinned" cross-product, exercised end to
    /// end rather than table-by-table.
    #[test]
    fn every_variant_x_size_x_checked_combination_builds_lays_out_and_paints() {
        let theme = crate::baseline();
        for variant in ALL_VARIANTS {
            for size in ALL_SIZES {
                for checked in [false, true] {
                    let view = toggle_button::<u32, _>(checked, |_, _| {})
                        .variant(variant)
                        .size(size)
                        .label("Toggle");
                    let mut widget = build(&view);
                    let laid = layout_with(&mut widget, 200.0, Some(&theme));
                    assert!(
                        laid.width.is_finite() && laid.width >= 0.0,
                        "{variant:?} {size:?} {checked}"
                    );
                    assert_eq!(laid.height, size.metrics().height);
                    let target = widget.target_radii(Some(&theme), laid.height);
                    for r in [
                        target.top_left,
                        target.top_right,
                        target.bottom_left,
                        target.bottom_right,
                    ] {
                        assert!(r.is_finite() && r >= 0.0, "{variant:?} {size:?} {checked}");
                    }
                    let _ = paint_at(&mut widget, laid, Some(&theme));
                }
            }
        }
    }

    #[test]
    fn disabled_colors_are_the_38_and_12_percent_on_surface_roles_checked_independent() {
        let theme = crate::baseline();
        let on_surface = theme.scheme().on_surface;
        for variant in ALL_VARIANTS {
            for checked in [false, true] {
                let colors = resolve_colors(Some(&theme), variant, checked, false);
                assert_eq!(
                    colors.content,
                    with_alpha(on_surface, DISABLED_CONTENT_OPACITY),
                    "{variant:?} checked={checked} content"
                );
                if is_transparent(variant) {
                    assert_eq!(colors.container, Color::TRANSPARENT, "{variant:?}");
                } else {
                    assert_eq!(
                        colors.container,
                        with_alpha(on_surface, DISABLED_CONTAINER_OPACITY),
                        "{variant:?} checked={checked} container"
                    );
                }
            }
        }
    }

    #[test]
    fn elevation_is_not_checked_aware_and_matches_the_plain_buttons_table() {
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
                    assert_eq!(elevation_dp(variant, true, pressed, hovered), expected);
                }
            }
        }
    }

    // ---- Acceptance 2: checked shape morph ---------------------------------

    #[test]
    fn shape_spring_matches_expressive_spatial_press() {
        let token = crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS;
        assert_eq!(SHAPE_SPRING.stiffness, token.stiffness);
        assert_eq!(SHAPE_SPRING.damping_ratio, token.damping_ratio);
        assert_eq!(SHAPE_SPRING.mass, 1.0);
        assert_eq!(SHAPE_SPRING.stiffness, 380.0);
        assert_eq!(SHAPE_SPRING.damping_ratio, 0.55);
    }

    #[test]
    fn the_first_target_snaps_rather_than_springing() {
        let mut motion = ShapeMotion::new();
        let pad = ContentPadding::symmetric(16.0);
        assert!(!motion.retarget(CornerRadii::uniform(20.0), pad));
        assert_eq!(motion.radii(), CornerRadii::uniform(20.0));
        assert!(!motion.advance(ft_secs(0.0)));
    }

    #[test]
    fn a_target_inside_the_dead_band_is_ignored() {
        let mut motion = ShapeMotion::new();
        let pad = ContentPadding::symmetric(16.0);
        motion.retarget(CornerRadii::uniform(20.0), pad);
        assert!(!motion.retarget(CornerRadii::uniform(20.0 + RETARGET_TOLERANCE / 2.0), pad));
        assert_eq!(motion.target_radii(), CornerRadii::uniform(20.0));
        assert!(motion.retarget(CornerRadii::uniform(20.0 + RETARGET_TOLERANCE * 2.0), pad));
    }

    #[test]
    fn checked_toggle_springs_from_round_rest_to_the_square_radius_and_settles_exactly() {
        let mut motion = ShapeMotion::new();
        let pad = ContentPadding::symmetric(16.0);
        motion.retarget(CornerRadii::uniform(20.0), pad); // unchecked pill rest
        assert!(motion.retarget(CornerRadii::uniform(12.0), pad)); // checked square
        assert_eq!(
            motion.radii(),
            CornerRadii::uniform(20.0),
            "starts where it was"
        );
        settle_at(&mut motion);
        assert_eq!(motion.radii(), CornerRadii::uniform(12.0));
    }

    fn settle_at(motion: &mut ShapeMotion) {
        let mut t = 0.0;
        for _ in 0..600 {
            t += 1.0 / 60.0;
            if !motion.advance(ft_secs(t)) {
                return;
            }
        }
        panic!("never settled");
    }

    #[test]
    fn rapid_toggle_retarget_mid_flight_is_continuous() {
        let mut motion = ShapeMotion::new();
        let pad = ContentPadding::symmetric(16.0);
        motion.retarget(CornerRadii::uniform(20.0), pad);
        motion.retarget(CornerRadii::uniform(12.0), pad);
        motion.advance(ft_secs(0.0));
        motion.advance(ft_secs(0.05));
        let mid = motion.radii();
        assert!(
            mid.top_left < 20.0 && mid.top_left > 12.0,
            "sampled mid-flight (got {mid:?})"
        );

        // Rapid re-toggle back while still in flight: no jump.
        assert!(motion.retarget(CornerRadii::uniform(20.0), pad));
        assert_eq!(motion.radii(), mid);

        settle_at(&mut motion);
        assert_eq!(motion.radii(), CornerRadii::uniform(20.0));
    }

    #[test]
    fn overshoot_never_drives_a_corner_negative() {
        let mut motion = ShapeMotion::new();
        let pad = ContentPadding::symmetric(16.0);
        motion.retarget(CornerRadii::uniform(28.0), pad);
        motion.retarget(CornerRadii::uniform(0.0), pad);
        let mut t = 0.0;
        for _ in 0..600 {
            t += 1.0 / 60.0;
            let animating = motion.advance(ft_secs(t));
            let r = motion.radii();
            assert!(
                r.top_left >= 0.0
                    && r.top_right >= 0.0
                    && r.bottom_left >= 0.0
                    && r.bottom_right >= 0.0,
                "a corner went negative at t={t}"
            );
            if !animating {
                break;
            }
        }
        assert_eq!(motion.radii(), CornerRadii::uniform(0.0));
    }

    #[test]
    fn widget_paint_retargets_the_morph_on_a_checked_flip() {
        let theme = crate::baseline();
        let mut widget = build(&toggle_button::<u32, _>(false, |_, _| {}));
        layout_with(&mut widget, 200.0, Some(&theme));
        paint_at(&mut widget, Size::new(40.0, 40.0), Some(&theme));
        settle_morph(&mut widget);
        let unchecked_radius = widget.motion.radii().top_left;
        assert_eq!(unchecked_radius, 20.0, "sm pill = height/2");

        widget.checked = true;
        layout_with(&mut widget, 200.0, Some(&theme));
        paint_at(&mut widget, Size::new(40.0, 40.0), Some(&theme));
        assert_eq!(
            widget.motion.radii().top_left,
            unchecked_radius,
            "the leg starts where it was (continuity)"
        );
        settle_morph(&mut widget);
        assert_eq!(widget.motion.radii().top_left, 12.0, "sm square token");
    }

    #[test]
    fn a_runtime_size_change_relayouts_at_the_new_sizes_padding_immediately() {
        // `ToggleButtonView::rebuild` already raises `ChangeFlags::LAYOUT` on
        // a `size` change, so the very next `layout` pass must already
        // reflect the NEW size's (halved, icon-only) padding — it must not
        // wait for an intervening `paint` to retarget the morph first, since
        // the crate's per-frame order always runs `layout` before `paint`.
        // Mutating `size` directly (the same field write `rebuild` performs)
        // with no `paint` call in between reproduces exactly that ordering —
        // the same revert-verified shape as `crate::button::core`'s
        // `a_runtime_size_change_relayouts_at_the_new_sizes_padding_immediately`.
        let mut widget = build(
            &toggle_button::<(), _>(false, |_, _| {})
                .icon(leaf_any(20.0, 20.0))
                .size(ToggleButtonSize::Sm),
        );
        let sm = layout_with(&mut widget, 500.0, None);
        let sm_metrics = ToggleButtonSize::Sm.metrics();
        assert_eq!(
            sm.width,
            sm_metrics.icon_size + sm_metrics.h_padding, // halved padding, both sides
            "sm icon-only width"
        );

        widget.size = ToggleButtonSize::Lg;
        let lg = layout_with(&mut widget, 500.0, None);
        let lg_metrics = ToggleButtonSize::Lg.metrics();
        assert_eq!(
            lg.width,
            lg_metrics.icon_size + lg_metrics.h_padding,
            "layout must resolve the NEW size's padding on its very next pass, \
             not the stale one only a later paint would have retargeted"
        );
    }

    // ---- Acceptance 3: group-connection corner asymmetry -------------------

    #[test]
    fn standalone_single_member_shape_is_uniform() {
        let theme = crate::baseline();
        let mut widget = build(&toggle_button::<u32, _>(false, |_, _| {}));
        layout_with(&mut widget, 200.0, Some(&theme));
        let target = widget.target_radii(Some(&theme), 40.0);
        assert_eq!(target, CornerRadii::uniform(20.0));
    }

    #[test]
    fn connected_first_member_is_outer_left_inner_right() {
        let theme = crate::baseline();
        let mut widget = build(
            &toggle_button::<u32, _>(false, |_, _| {})
                .group_connected(true)
                .first_in_group(true)
                .last_in_group(false),
        );
        layout_with(&mut widget, 200.0, Some(&theme));
        let target = widget.target_radii(Some(&theme), 40.0);
        assert_eq!(
            (target.top_left, target.bottom_left),
            (20.0, 20.0),
            "outer left"
        );
        assert_eq!(
            (target.top_right, target.bottom_right),
            (CONNECTED_INNER_RADIUS, CONNECTED_INNER_RADIUS),
            "inner right"
        );
    }

    #[test]
    fn connected_last_member_is_inner_left_outer_right() {
        let theme = crate::baseline();
        let mut widget = build(
            &toggle_button::<u32, _>(false, |_, _| {})
                .group_connected(true)
                .first_in_group(false)
                .last_in_group(true),
        );
        layout_with(&mut widget, 200.0, Some(&theme));
        let target = widget.target_radii(Some(&theme), 40.0);
        assert_eq!(
            (target.top_left, target.bottom_left),
            (CONNECTED_INNER_RADIUS, CONNECTED_INNER_RADIUS),
            "inner left"
        );
        assert_eq!(
            (target.top_right, target.bottom_right),
            (20.0, 20.0),
            "outer right"
        );
    }

    #[test]
    fn connected_middle_member_is_all_inner_corners() {
        let theme = crate::baseline();
        let mut widget = build(
            &toggle_button::<u32, _>(false, |_, _| {})
                .group_connected(true)
                .first_in_group(false)
                .last_in_group(false),
        );
        layout_with(&mut widget, 200.0, Some(&theme));
        let target = widget.target_radii(Some(&theme), 40.0);
        assert_eq!(
            target,
            CornerRadii::uniform(CONNECTED_INNER_RADIUS),
            "a middle member is all inner corners"
        );
    }

    #[test]
    fn connected_pressed_member_uses_the_pressed_inner_radius() {
        let theme = crate::baseline();
        let mut widget = build(
            &toggle_button::<u32, _>(false, |_, _| {})
                .group_connected(true)
                .first_in_group(false)
                .last_in_group(false),
        );
        layout_with(&mut widget, 200.0, Some(&theme));
        widget.state.set_pressed(true);
        let target = widget.target_radii(Some(&theme), 40.0);
        assert_eq!(target, CornerRadii::uniform(CONNECTED_PRESSED_INNER_RADIUS));
    }

    #[test]
    fn connected_checked_idle_member_is_fully_round_on_every_corner() {
        let theme = crate::baseline();
        let mut widget = build(
            &toggle_button::<u32, _>(true, |_, _| {})
                .group_connected(true)
                .first_in_group(false)
                .last_in_group(false),
        );
        layout_with(&mut widget, 200.0, Some(&theme));
        let target = widget.target_radii(Some(&theme), 40.0);
        assert_eq!(
            target,
            CornerRadii::uniform(20.0),
            "checked+connected+idle overrides the inner-corner asymmetry"
        );
    }

    // ---- Acceptance 4: icon/label swap on checked ---------------------------

    #[test]
    fn label_falls_back_to_the_unchecked_one_when_checked_label_is_unset() {
        let view = toggle_button::<u32, _>(true, |_, _| {}).label("Off");
        assert_eq!(view.effective_label_text(), Some("Off"));
    }

    #[test]
    fn checked_label_wins_over_the_unchecked_one_when_both_are_set() {
        let view = toggle_button::<u32, _>(true, |_, _| {})
            .label("Off")
            .checked_label("On");
        assert_eq!(view.effective_label_text(), Some("On"));
        let unchecked = toggle_button::<u32, _>(false, |_, _| {})
            .label("Off")
            .checked_label("On");
        assert_eq!(unchecked.effective_label_text(), Some("Off"));
    }

    #[test]
    fn build_resolves_the_effective_label_for_the_initial_checked_value() {
        let checked_view = toggle_button::<u32, _>(true, |_, _| {})
            .label("Off")
            .checked_label("On");
        let widget = build(&checked_view);
        assert_eq!(widget.label.content(), "On");

        let unchecked_view = toggle_button::<u32, _>(false, |_, _| {})
            .label("Off")
            .checked_label("On");
        let widget = build(&unchecked_view);
        assert_eq!(widget.label.content(), "Off");
    }

    #[test]
    fn rebuild_swaps_the_label_content_on_a_checked_flip() {
        let prev = toggle_button::<u32, _>(false, |_, _| {})
            .label("Off")
            .checked_label("On");
        let next = toggle_button::<u32, _>(true, |_, _| {})
            .label("Off")
            .checked_label("On");
        let mut widget = build(&prev);
        assert_eq!(widget.label.content(), "Off");
        let mut counter = 0u64;
        let flags =
            View::<u32>::rebuild(&next, &prev, &mut widget, &mut BuildCtx::new(&mut counter));
        assert_eq!(widget.label.content(), "On");
        assert!(flags.contains(ChangeFlags::LAYOUT));
    }

    #[test]
    fn icon_falls_back_and_swaps_the_same_way_as_the_label() {
        let unchecked = toggle_button::<(), _>(false, |_, _| {}).icon(leaf_any(20.0, 20.0));
        assert!(unchecked.effective_icon_view().is_some());

        let checked_no_override =
            toggle_button::<(), _>(true, |_, _| {}).icon(leaf_any(20.0, 20.0));
        assert!(
            checked_no_override.effective_icon_view().is_some(),
            "falls back to `icon` while checked"
        );

        let checked_with_override = toggle_button::<(), _>(true, |_, _| {})
            .icon(leaf_any(20.0, 20.0))
            .checked_icon(leaf_any(24.0, 24.0));
        let widget = build(&checked_with_override);
        assert!(widget.icon.is_some());
    }

    #[test]
    fn an_icon_only_toggle_button_halves_its_horizontal_padding() {
        let mut icon_only =
            build(&toggle_button::<(), _>(false, |_, _| {}).icon(leaf_any(20.0, 20.0)));
        layout_with(&mut icon_only, 300.0, None);
        assert!(!icon_only.has_label());
        assert_eq!(icon_only.h_padding(&ToggleButtonSize::Sm.metrics()), 8.0);

        let mut labeled = build(&toggle_button::<(), _>(false, |_, _| {}).label("Bold"));
        layout_with(&mut labeled, 300.0, None);
        assert!(labeled.has_label());
        assert_eq!(labeled.h_padding(&ToggleButtonSize::Sm.metrics()), 16.0);
    }

    // ---- Content wider than the box: clipped, never re-fitted -------------

    #[test]
    fn a_box_narrower_than_the_content_clips_it_instead_of_ellipsizing() {
        let mut widget = build(&toggle_button::<(), _>(false, |_, _| {}).label("Days per week"));
        let natural = layout_with(&mut widget, f64::INFINITY, None);
        let full = paint_at(&mut widget, natural, None).run_glyphs;
        assert_eq!(full.len(), 1, "one label run");
        assert!(full[0] > 1, "the label shaped some glyphs");
        assert!(!widget.content_clipped, "nothing to clip at natural width");

        // A group hands its members a tight main mid-squish. A shallow
        // squeeze eats into the horizontal padding first — the label is whole
        // and still inside the box, which is what makes the reference's
        // squish read as padding compressing rather than text truncating.
        let padding = ToggleButtonSize::Sm.metrics().h_padding;
        let shallow = layout_tight_main(&mut widget, natural.width - padding);
        let rec = paint_at(&mut widget, shallow, None);
        assert_eq!(rec.run_glyphs, full, "the label is untouched");
        assert!(
            rec.clips.is_empty(),
            "and still fits, so nothing is clipped"
        );

        // Past the padding the box starts covering the label itself.
        let squeezed = layout_tight_main(&mut widget, natural.width - padding * 2.0 - 4.0);
        let rec = paint_at(&mut widget, squeezed, None);
        assert_eq!(
            rec.run_glyphs, full,
            "the shrinking box covers the label; it is never re-shaped to fit"
        );
        assert_eq!(
            rec.clips,
            vec![(Point::ZERO, squeezed)],
            "the overflowing content row is clipped to the box (`Clip.hardEdge`)"
        );
        // Centered, so the overflow is split evenly off both ends.
        assert!(
            widget.label_origin.x < 0.0,
            "the centered row hangs off the leading edge too: {}",
            widget.label_origin.x
        );
    }

    #[test]
    fn a_squeezed_then_restored_label_keeps_its_full_shape() {
        let mut widget = build(&toggle_button::<(), _>(false, |_, _| {}).label("Days per week"));
        let natural = layout_with(&mut widget, f64::INFINITY, None);
        let full = paint_at(&mut widget, natural, None).run_glyphs;

        layout_tight_main(&mut widget, natural.width - 12.0);
        // Back to natural — including the sub-pixel residue a spring landing
        // on its target leaves behind, which an exact fit test would answer
        // by dropping a whole word.
        for main in [natural.width - 0.0002, natural.width] {
            let size = layout_tight_main(&mut widget, main);
            assert_eq!(
                paint_at(&mut widget, size, None).run_glyphs,
                full,
                "the label is whole again at main {main}"
            );
        }
        assert!(!widget.content_clipped, "and needs no clip once it fits");
    }

    // ---- Controlled contract -------------------------------------------------

    #[test]
    fn defaults_match_the_reference_constructor_defaults() {
        let view = toggle_button::<u32, _>(false, |_, _| {});
        assert_eq!(view.variant, ButtonVariant::Filled);
        assert_eq!(view.size, ToggleButtonSize::Sm);
        assert!(view.enabled);
        assert_eq!(view.haptic, HapticSignal::None);
        assert!(!view.is_group_connected);
        assert!(view.is_first_in_group);
        assert!(view.is_last_in_group);
    }

    #[test]
    fn the_named_constructors_map_onto_the_reference_styles() {
        assert_eq!(
            filled_toggle_button::<u32, _>(false, |_, _| {}).variant,
            ButtonVariant::Filled
        );
        assert_eq!(
            tonal_toggle_button::<u32, _>(false, |_, _| {}).variant,
            ButtonVariant::Tonal
        );
        assert_eq!(
            elevated_toggle_button::<u32, _>(false, |_, _| {}).variant,
            ButtonVariant::Elevated
        );
        assert_eq!(
            outlined_toggle_button::<u32, _>(false, |_, _| {}).variant,
            ButtonVariant::Outlined
        );
        assert_eq!(
            text_toggle_button::<u32, _>(false, |_, _| {}).variant,
            ButtonVariant::Text
        );
    }

    #[test]
    fn a_tap_reports_the_requested_value_and_never_self_mutates() {
        let mut widget = build(&toggle_button::<bool, _>(false, |s, v| *s = v));
        layout_with(&mut widget, 200.0, None);
        let size = Size::new(60.0, 40.0);
        let mut state = false;
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, &ev(PointerPhase::Down, 30.0, 20.0));
        assert!(widget.captured);
        assert!(!widget.checked, "the widget itself never flips `checked`");

        widget.event(&mut ctx, &ev(PointerPhase::Up, 30.0, 20.0));
        assert!(state, "on_checked_change fired with the requested value");
        assert!(!widget.checked, "still not self-mutated");
    }

    #[test]
    fn disabling_mid_press_clears_the_press_capture_and_long_press_candidacy() {
        let prev = toggle_button::<u32, _>(false, |_, _| {});
        let next = toggle_button::<u32, _>(false, |_, _| {}).enabled(false);
        let mut widget = build(&prev);
        widget.state.set_pressed(true);
        widget.captured = true;
        widget.long_press.elapsed = true;
        let mut counter = 0u64;
        View::<u32>::rebuild(&next, &prev, &mut widget, &mut BuildCtx::new(&mut counter));
        assert!(!widget.state.pressed);
        assert!(!widget.captured);
        assert!(!widget.long_press.elapsed);
    }

    // ---- Long-press ------------------------------------------------------

    #[test]
    fn a_long_press_fires_on_move_arrival_after_the_threshold_and_suppresses_the_tap() {
        // `PaintCtx::for_test`, the chosen-`FrameTime` seam, is behind a
        // `frust-core` feature this crate's dev-dependency does not enable
        // (see `switch.rs`'s own note) — so the threshold-crossing latch this
        // test exercises is set directly rather than paced through real
        // paint passes; the paint-side latch logic itself is exercised by
        // `paint`'s own request_frame plumbing, not re-derived here.
        let view =
            toggle_button::<(u32, u32), _>(false, |s, _| s.0 += 1).on_long_press(|s| s.1 += 1);
        let mut widget = build(&view);
        layout_with(&mut widget, 200.0, None);
        let size = Size::new(60.0, 40.0);

        let mut state = (0u32, 0u32);
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, 30.0, 20.0),
        );
        assert!(widget.captured);

        // Simulate paint having latched the threshold crossing.
        widget.long_press.elapsed = true;

        // A Move within slop fires the long-press now (fire-on-arrival), not
        // the tap.
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Move, 31.0, 20.0),
        );
        assert_eq!(state.1, 1, "on_long_press fired");
        assert_eq!(state.0, 0, "on_checked_change must not also fire");

        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, 31.0, 20.0),
        );
        assert_eq!(
            state.0, 0,
            "the up-inside toggle is suppressed after a long-press"
        );
    }

    #[test]
    fn a_long_press_still_fires_at_up_if_no_move_arrives_first() {
        let view =
            toggle_button::<(u32, u32), _>(false, |s, _| s.0 += 1).on_long_press(|s| s.1 += 1);
        let mut widget = build(&view);
        layout_with(&mut widget, 200.0, None);
        let size = Size::new(60.0, 40.0);
        let mut state = (0u32, 0u32);
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, &ev(PointerPhase::Down, 30.0, 20.0));
        widget.long_press.elapsed = true;
        widget.event(&mut ctx, &ev(PointerPhase::Up, 30.0, 20.0));
        assert_eq!(state.1, 1, "the Up fallback fires the long-press");
        assert_eq!(state.0, 0, "the checked toggle is suppressed");
    }

    #[test]
    fn movement_past_touch_slop_cancels_long_press_candidacy() {
        let view = toggle_button::<u32, _>(false, |s, _| *s += 1).on_long_press(|s| *s += 100);
        let mut widget = build(&view);
        layout_with(&mut widget, 200.0, None);
        let size = Size::new(200.0, 40.0);
        let mut state = 0u32;
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, &ev(PointerPhase::Down, 30.0, 20.0));
        widget.long_press.elapsed = true; // simulate the threshold already crossed
        widget.event(
            &mut ctx,
            &ev(PointerPhase::Move, 30.0 + TOUCH_SLOP + 5.0, 20.0),
        );
        assert!(!widget.long_press.elapsed, "a drag cancels the candidacy");
    }

    #[test]
    fn cancel_clears_long_press_state_without_touching_app_state() {
        let mut widget = build(&toggle_button::<u32, _>(false, |s, _| *s = 1));
        layout_with(&mut widget, 200.0, None);
        let size = Size::new(60.0, 40.0);
        let mut state = 0u32;
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, &ev(PointerPhase::Down, 30.0, 20.0));
        widget.long_press.elapsed = true;
        widget.event(&mut ctx, &ev(PointerPhase::Cancel, 30.0, 20.0));
        assert!(!widget.long_press.elapsed);
        assert!(!widget.captured);
        assert_eq!(state, 0);
    }

    // ---- Semantics ---------------------------------------------------------

    /// A `SemanticsCtx` is only ever handed out by `RenderRoot::semantics` (no
    /// public constructor) — the same fixture route `button_group.rs`'s own
    /// semantics test uses.
    fn semantics_button_node(
        mut view_fn: impl FnMut(&mut ()) -> ToggleButtonView<()> + 'static,
    ) -> frust_core::accesskit::Node {
        let mut root: frust_core::RenderRoot<(), ToggleButtonView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut view_fn, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("a Button node is contributed")
            .1
            .clone()
    }

    #[test]
    fn semantics_reports_toggled_state_and_label() {
        let node = semantics_button_node(|_| toggle_button::<(), _>(true, |_, _| {}).label("Bold"));
        assert_eq!(node.label(), Some("Bold"));
        assert_eq!(node.toggled(), Some(Toggled::True));
    }

    #[test]
    fn disabled_semantics_report_no_click_action() {
        let node =
            semantics_button_node(|_| toggle_button::<(), _>(false, |_, _| {}).enabled(false));
        assert!(node.is_disabled());
    }
}
