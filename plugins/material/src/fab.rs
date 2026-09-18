// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments)
// Upstream: https://github.com/paadevelopments/material_3_expressive
//   lib/components/floating_action_buttons/, lib/components/extended_fabs/
// The reference's separate `M3EFab`/`M3EExtendedFab` widgets are unified
// into one `FabView`/`FabWidget` pair here (constructed via `fab()`/
// `extended_fab()`); `M3ETappable`, the shared interaction primitive both
// build on upstream, is ported separately as `crate::interaction`.

//! The M3 Expressive `FloatingActionButton`: three container sizes (small
//! 40dp / medium 56dp / large 96dp), four container colors, a spring press
//! scale, elevation that lifts on hover, and an animatable corner-radius
//! override seam — plus [`extended_fab`]'s pill-shaped variant, whose label
//! animates open/closed.
//!
//! [`fab`] produces an icon-only FAB (a fixed square, painted centered
//! icon); [`extended_fab`] produces the pill-shaped extended variant (56dp
//! fixed height, a visible text label, and an optional leading icon). Both
//! fire a plain `on_press` callback on release inside their bounds —
//! mirroring [`frust::Button`]'s fire-on-up-inside contract, not a
//! controlled component (a FAB has no reportable/confirmable value, just an
//! action).
//!
//! # Sizes
//!
//! Source: `m3e_fab_theme.dart`'s `M3EFabTheme` defaults. Container /
//! icon / corner-radius token, per [`FabSize`]:
//!
//! | Size | Container | Icon | Radius (themed token) |
//! |---|---|---|---|
//! | [`FabSize::Small`] | 40dp | 24dp | 12dp (`shape.medium`) |
//! | [`FabSize::Medium`] (default) | 56dp | 24dp | 16dp (`shape.large`) |
//! | [`FabSize::Large`] | 96dp | 36dp | 28dp (`shape.extra_large`) |
//!
//! # Colors
//!
//! Source: `m3e_fab_theme.dart`'s `M3EFabTheme._palette`. Container /
//! content role, per [`FabColor`]:
//!
//! | Color (default: [`FabColor::Primary`]) | Container | Content |
//! |---|---|---|
//! | [`FabColor::Primary`] | `primaryContainer` | `onPrimaryContainer` |
//! | [`FabColor::Secondary`] | `secondaryContainer` | `onSecondaryContainer` |
//! | [`FabColor::Tertiary`] | `tertiaryContainer` | `onTertiaryContainer` |
//! | [`FabColor::Surface`] | `surfaceContainerHigh` | `primary` (**not** `onSurface` — verified from the Dart source's `_palette` switch) |
//!
//! # Elevation
//!
//! Rest: M3 elevation level 3. Hover: level 4 (`M3EFabTheme`/
//! `M3EExtendedFabTheme.elevation(hovered:)` both use this exact rest/hover
//! pair). Hover is claimed from the uncaptured `Move` arm via
//! [`frust::authoring::EventCtx::claim_hover`] and self-corrected every
//! paint from [`frust::authoring::PaintCtx::is_hovered`] — the same
//! three-part contract [`super::list_item`] documents (see
//! `docs/CODE_STANDARDS.md`'s Interaction Semantics).
//!
//! # Press scale
//!
//! Both variants play the reference's spatial press-scale spring —
//! stiffness 380, damping ratio 0.55 (`m3e_floating_action_buttons.dart`'s
//! module doc: "plays a spatial spring press scale (380 / 0.55)";
//! `m3e_extended_fabs.dart`'s: "Press uses spatial spring scale only (380 /
//! 0.55)") — toward `pressedScale` (0.95 for [`fab`], 0.97 for
//! [`extended_fab`], `M3EFabTheme.pressedScale`/`M3EExtendedFabTheme.
//! pressedScale`). [`PRESS_SPRING`] is a named constant rather than a
//! paint-time theme read (a press starts in the event pass, which never
//! reads a theme) — the same `button_group`-established
//! `PRESS_SPRING`-plus-`fling` pattern, driven here via
//! [`frust::AnimationController::fling`] and applied at paint time as a
//! [`frust::authoring::PaintScene::push_transform`]/`pop_transform` pair
//! around the whole surface (container, shadow, state layer, icon, label) —
//! the same `scale_about`-pivot technique `frust_widgets::Button` uses,
//! duplicated here rather than depended on (design-system plugins carry no
//! `frust-widgets` production dependency).
//!
//! # Corner-radius override seam (the FAB-menu-morph hook)
//!
//! [`FabView::corner_radius`] (`Option<f64>`, [`fab`] only — a no-op on
//! [`extended_fab`], whose upstream `M3EExtendedFab` constructor carries no
//! such parameter at all) mirrors the reference's `M3EFab.cornerRadius`
//! exactly: when `None`, the radius follows the size-derived themed token
//! and eases toward it with `MaterialMotion::SHORT_4` whenever that target
//! changes; when `Some(r)`, the radius pins to `r` immediately, every frame,
//! with **no** internal animation — the reference's `cornerRadius != null ?
//! Duration.zero : M3EMotion.short4` ternary. The override exists so an
//! external, frame-by-frame-driven radius (a spring the *caller* owns) is
//! never fought by an internal ease of its own. There is no consumer of this
//! seam in this crate yet — it is built for [`mod@super::fab_menu`]'s
//! trigger-to-close-button morph, a follow-up not built here (that module
//! currently duplicates its own private trigger tokens rather than calling
//! into this one).
//!
//! # Extended FAB: label collapse/expand
//!
//! [`FabView::extended`] (default `true`) mirrors `M3EExtendedFab.extended`:
//! setting it `false` animates the visible label away, collapsing the pill
//! toward an icon-only shape. Driven by an
//! [`frust::AnimationController`] (`MaterialMotion::MEDIUM_2` / `EMPHASIZED`
//! — the reference's own `AnimatedContainer`/`AnimatedSize` timing) whose
//! `0.0`..`1.0` value scales both the icon-label gap and the label's own
//! measured width, **and** the container's horizontal padding (`Tween`
//! between the reference's `collapsedHorizontalPadding` 16dp and
//! `extendedHorizontalPadding` 20dp) — a v1 approximation of the reference's
//! `AnimatedSize`-plus-static-`Padding` combination (this crate's
//! `PaintScene` has no generic animated-child-size primitive), which still
//! lands on the reference's exact two endpoint widths: fully collapsed is
//! `pad(16)*2 + icon(24)` = 56dp (a rounded square, since the radius itself
//! never animates with collapse — it stays fixed), fully extended is
//! `pad(20)*2 + icon(24) + gap(12) + label_width`. **Known v1 mount
//! quirk:** because [`frust::AnimationController::advance`]'s first call
//! after starting a motion only seeds its clock (see that method's own
//! doc), a freshly mounted [`extended_fab`] whose label starts shown (the
//! default) briefly grows its label into view over one `MEDIUM_2` duration
//! on its very first frames, rather than mounting already-open the way the
//! reference's own `AnimatedSize` does on its first build — a widget that
//! starts *collapsed* (`.extended(false)`) has no such quirk, since a
//! never-`forward()`-ed controller stays exactly at its resting `0.0`.
//!
//! **The layout-skip trap.** The reveal fraction above is a **layout**
//! value — [`FabWidget::layout`]'s extended branch lerps pad/gap/label width
//! straight off `label_anim.value_clamped()` — so an in-flight reveal needs
//! an explicit relayout request from `paint`, not merely another frame: on
//! the mobile intra-frame layout skip (`docs/SHELLS_ARCHITECTURE.md`'s
//! `frame_gate`), layout does not re-run merely because paint asked for
//! another frame. `FabWidget::paint` therefore calls
//! [`frust::authoring::PaintCtx::request_layout`] (which implies
//! `request_frame`) while `label_anim` is in flight — the same trap
//! [`mod@super::navigation_rail`]'s width motion and
//! [`mod@super::expandable_list`]'s reveal both document at length. Unlike
//! those two, this widget has no `Theme.motion.reduce_motion` handling for
//! `label_anim` at all yet — a v1 gap, not part of this fix.
//!
//! # Haptics: verified none
//!
//! `M3ETappable` defaults to `haptic: M3EHapticFeedback.none`
//! (`m3e_tappable.dart`), and neither `M3EFab` nor `M3EExtendedFab` passes a
//! `haptic:` argument to their `M3ETappable(...)` call — so the default
//! applies, and a FAB press fires **no** platform haptic in the upstream
//! reference. This port matches that verified fact by never calling
//! [`crate::interaction::MaterialHaptics::fire`] from [`FabWidget::event`].
//!
//! # Attribution
//!
//! See `plugins/material/NOTICE`'s "MIT License — Root Crate" section and
//! its Module Attribution Header Convention.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{Action, Role};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use frust::{AnimationController, ShapeScale, SpringDesc, Theme, Tween};
use kurbo::{Affine, Point, Rect, Size};
use peniko::Color;

use super::press::presses;
use super::state_layer::StateLayer;
use frust::authoring::{ThemeTextColor, ThemeTextType};

// ---- Sizes (M3EFabTheme defaults: small/medium/large) ---------------------

/// Container size for the small FAB, in logical px (`M3EFabTheme.
/// smallContainer`).
const SMALL_CONTAINER: f64 = 40.0;
/// Container size for the medium FAB, in logical px (`M3EFabTheme.
/// mediumContainer`) — the default [`FabSize`].
const MEDIUM_CONTAINER: f64 = 56.0;
/// Container size for the large FAB, in logical px (`M3EFabTheme.
/// largeContainer`).
const LARGE_CONTAINER: f64 = 96.0;

/// Icon size for the small FAB, in logical px (`M3EFabTheme.
/// smallIconSize`).
const SMALL_ICON: f64 = 24.0;
/// Icon size for the medium FAB, in logical px (`M3EFabTheme.
/// mediumIconSize`).
const MEDIUM_ICON: f64 = 24.0;
/// Icon size for the large FAB, in logical px (`M3EFabTheme.
/// largeIconSize`).
const LARGE_ICON: f64 = 36.0;

/// Unthemed-fallback corner radius, small FAB (a theme resolves
/// `shape.medium`; `M3EFabTheme.smallRadius`).
const SMALL_RADIUS: f64 = 12.0;
/// Unthemed-fallback corner radius, medium FAB (a theme resolves
/// `shape.large`; `M3EFabTheme.mediumRadius`) — also the extended FAB's
/// fallback radius, which coincides with this value exactly (see
/// [`EXTENDED_RADIUS_FALLBACK`]'s doc).
const MEDIUM_RADIUS: f64 = 16.0;
/// Unthemed-fallback corner radius, large FAB (a theme resolves
/// `shape.extra_large`; `M3EFabTheme.largeRadius`).
const LARGE_RADIUS: f64 = 28.0;

// ---- Extended FAB (M3EExtendedFabTheme defaults) ---------------------------

/// Fixed extended-FAB height, in logical px (`M3EExtendedFabTheme.height`) —
/// constant regardless of the `extended`/collapsed reveal state.
const EXTENDED_HEIGHT: f64 = 56.0;
/// Unthemed-fallback extended-FAB corner radius (a theme resolves
/// `shape.large`; `M3EExtendedFabTheme.cornerRadius`). Coincides with
/// [`MEDIUM_RADIUS`] exactly — both are the same `shape.large`/16dp token —
/// reused rather than duplicated.
const EXTENDED_RADIUS_FALLBACK: f64 = MEDIUM_RADIUS;
/// Symmetric horizontal padding while the label is shown, in logical px
/// (`M3EExtendedFabTheme.extendedHorizontalPadding`).
const EXTENDED_PAD_EXPANDED: f64 = 20.0;
/// Symmetric horizontal padding while the label is collapsed, in logical px
/// (`M3EExtendedFabTheme.collapsedHorizontalPadding`).
const EXTENDED_PAD_COLLAPSED: f64 = 16.0;
/// Extended-FAB icon size, in logical px (`M3EExtendedFabTheme.iconSize`) —
/// fixed regardless of any [`FabSize`] (ignored on an [`extended_fab`]).
const EXTENDED_ICON: f64 = 24.0;
/// Gap between the icon and the label when both are visible, in logical px
/// (`M3EExtendedFabTheme.iconLabelGap`).
const EXTENDED_ICON_LABEL_GAP: f64 = 12.0;
/// Press-scale target for [`extended_fab`] (`M3EExtendedFabTheme.
/// pressedScale`).
const EXTENDED_PRESSED_SCALE: f64 = 0.97;

// ---- Press scale ------------------------------------------------------------

/// Press-scale target for [`fab`] (`M3EFabTheme.pressedScale`).
const FAB_PRESSED_SCALE: f64 = 0.95;

/// The press-scale spring both variants share: stiffness 380, damping ratio
/// 0.55 — the reference's `M3EMotion.expressiveSpatialPress`
/// (`crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`). See the
/// [module docs](self) for why this is a named constant rather than a
/// paint-time theme read; `press_spring_matches_reference` is the tripwire
/// that keeps it equal to the token.
const PRESS_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 380.0,
    damping_ratio: 0.55,
};

/// Nominal period seeding the press [`AnimationController`]'s clock; the
/// motion is spring-driven ([`PRESS_SPRING`]) via `fling`, so this duration
/// only backs the controller's construction and is not itself a timing
/// (mirrors `button_group`'s `PRESS_ANIM_PERIOD`).
const PRESS_ANIM_PERIOD: Duration = Duration::from_millis(300);
/// Launch velocity (value-units/sec) handed to the press-in / press-out
/// [`AnimationController::fling`] — a modest kick so the spring reads snappy
/// (mirrors `button_group`'s `FLING_VELOCITY`).
const FLING_VELOCITY: f64 = 4.0;

// ---- Colors -------------------------------------------------------------

/// Unthemed-fallback primary container fill (a theme resolves
/// `colors.primary_container`).
const PRIMARY_CONTAINER: Color = Color::from_rgb8(0xEA, 0xDD, 0xFF);
/// Unthemed-fallback primary container content (a theme resolves
/// `colors.on_primary_container`).
const ON_PRIMARY_CONTAINER: Color = Color::from_rgb8(0x21, 0x00, 0x5D);
/// Unthemed-fallback secondary container fill (a theme resolves
/// `colors.secondary_container`).
const SECONDARY_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback secondary container content (a theme resolves
/// `colors.on_secondary_container`).
const ON_SECONDARY_CONTAINER: Color = Color::from_rgb8(0x1D, 0x19, 0x2B);
/// Unthemed-fallback tertiary container fill (a theme resolves
/// `colors.tertiary_container`).
const TERTIARY_CONTAINER: Color = Color::from_rgb8(0xFF, 0xD8, 0xE4);
/// Unthemed-fallback tertiary container content (a theme resolves
/// `colors.on_tertiary_container`).
const ON_TERTIARY_CONTAINER: Color = Color::from_rgb8(0x31, 0x11, 0x1D);
/// Unthemed-fallback surface-variant container fill (a theme resolves
/// `colors.surface_container_high`).
const SURFACE_CONTAINER_HIGH: Color = Color::from_rgb8(0xEC, 0xE6, 0xF0);
/// Unthemed-fallback surface-variant content — **`colors.primary`, not
/// `on_surface`** (verified from the Dart source's `_palette` switch; see
/// the [module docs](self)' Colors table).
const SURFACE_FOREGROUND: Color = Color::from_rgb8(0x67, 0x50, 0xA4);

// ---- Shadow (unthemed fallback) --------------------------------------------

/// Unthemed-fallback shadow y-offset at M3 elevation level 3, matching
/// `crate::tokens::elevation().level3`'s `y_offset` exactly (`dp / 2.0 +
/// 1.0` at `dp = 6.0`).
const LEVEL3_SHADOW_Y_OFFSET: f64 = 4.0;
/// Unthemed-fallback shadow blur std-dev at M3 elevation level 3, matching
/// `crate::tokens::elevation().level3`.
const LEVEL3_SHADOW_BLUR: f64 = 6.0;
/// Unthemed-fallback shadow y-offset at M3 elevation level 4, matching
/// `crate::tokens::elevation().level4`'s `y_offset` exactly (`dp / 2.0 +
/// 1.0` at `dp = 8.0`).
const LEVEL4_SHADOW_Y_OFFSET: f64 = 5.0;
/// Unthemed-fallback shadow blur std-dev at M3 elevation level 4, matching
/// `crate::tokens::elevation().level4`.
const LEVEL4_SHADOW_BLUR: f64 = 8.0;
/// Unthemed-fallback shadow color (opaque black at
/// `crate::tokens::elevation()`'s `0.3` alpha — identical at every level).
const FALLBACK_SHADOW_COLOR: Color = Color::new([0.0, 0.0, 0.0, 0.3]);

/// The FAB container size tier. Source: `m3e_fab_theme.dart`'s
/// `M3EFabTheme` defaults — see the [module docs](self)' Sizes table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FabSize {
    /// 40dp compact FAB.
    Small,
    /// 56dp baseline FAB (the default).
    Medium,
    /// 96dp hero FAB.
    Large,
}

impl FabSize {
    fn container(self) -> f64 {
        match self {
            FabSize::Small => SMALL_CONTAINER,
            FabSize::Medium => MEDIUM_CONTAINER,
            FabSize::Large => LARGE_CONTAINER,
        }
    }

    fn icon_size(self) -> f64 {
        match self {
            FabSize::Small => SMALL_ICON,
            FabSize::Medium => MEDIUM_ICON,
            FabSize::Large => LARGE_ICON,
        }
    }

    fn fallback_radius(self) -> f64 {
        match self {
            FabSize::Small => SMALL_RADIUS,
            FabSize::Medium => MEDIUM_RADIUS,
            FabSize::Large => LARGE_RADIUS,
        }
    }

    fn themed_radius(self, shape: &ShapeScale) -> f64 {
        match self {
            FabSize::Small => shape.medium,
            FabSize::Medium => shape.large,
            FabSize::Large => shape.extra_large,
        }
    }
}

/// The FAB container color variant. Source: `m3e_fab_theme.dart`'s
/// `M3EFabTheme._palette` — see the [module docs](self)' Colors table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FabColor {
    /// `primaryContainer` / `onPrimaryContainer` (the default).
    Primary,
    /// `secondaryContainer` / `onSecondaryContainer`.
    Secondary,
    /// `tertiaryContainer` / `onTertiaryContainer`.
    Tertiary,
    /// `surfaceContainerHigh` / `primary` (not `onSurface`).
    Surface,
}

/// `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved `(container, content)` colors for `color`. Themed: the
/// [module docs](self)' Colors table. Unthemed: the matching fallback
/// constants exactly.
fn resolve_colors(theme: Option<&Theme>, color: FabColor) -> (Color, Color) {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            match color {
                FabColor::Primary => (s.primary_container, s.on_primary_container),
                FabColor::Secondary => (s.secondary_container, s.on_secondary_container),
                FabColor::Tertiary => (s.tertiary_container, s.on_tertiary_container),
                FabColor::Surface => (s.surface_container_high, s.primary),
            }
        }
        None => match color {
            FabColor::Primary => (PRIMARY_CONTAINER, ON_PRIMARY_CONTAINER),
            FabColor::Secondary => (SECONDARY_CONTAINER, ON_SECONDARY_CONTAINER),
            FabColor::Tertiary => (TERTIARY_CONTAINER, ON_TERTIARY_CONTAINER),
            FabColor::Surface => (SURFACE_CONTAINER_HIGH, SURFACE_FOREGROUND),
        },
    }
}

/// The resolved `(blur_std_dev, y_offset, color)` shadow parameters: M3
/// elevation level 3 at rest, level 4 while `hovered`. Themed: `theme.
/// elevation.{level3,level4}`'s `ShadowSpec`, colored by `colors.shadow` at
/// the spec's `color_alpha`. Unthemed: the `LEVEL{3,4}_SHADOW_*` constants
/// exactly (the same values `crate::tokens::elevation()` produces).
fn resolve_shadow(theme: Option<&Theme>, hovered: bool) -> (f64, f64, Color) {
    match theme {
        Some(theme) => {
            let level = if hovered {
                theme.elevation.level4
            } else {
                theme.elevation.level3
            };
            let shadow = level.shadow(theme.brightness);
            let color = with_alpha(theme.scheme().shadow, shadow.color_alpha);
            (shadow.blur_std_dev, shadow.y_offset, color)
        }
        None => {
            if hovered {
                (
                    LEVEL4_SHADOW_BLUR,
                    LEVEL4_SHADOW_Y_OFFSET,
                    FALLBACK_SHADOW_COLOR,
                )
            } else {
                (
                    LEVEL3_SHADOW_BLUR,
                    LEVEL3_SHADOW_Y_OFFSET,
                    FALLBACK_SHADOW_COLOR,
                )
            }
        }
    }
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// The affine transform scaling uniformly by `scale` about a fixed `pivot`
/// point — mirrors `frust_widgets::Button`'s module-private `scale_about`
/// (duplicated rather than depended on; see the [module docs](self)' Press
/// scale section).
fn scale_about(pivot: Point, scale: f64) -> Affine {
    Affine::translate((pivot.x, pivot.y))
        * Affine::scale(scale)
        * Affine::translate((-pivot.x, -pivot.y))
}

/// A view-held, typed press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// Build the extended FAB's visible label view, themed `OnPrimaryContainer`,
/// its family following the live theme's `labelLarge` role (the M3 extended
/// FAB label token). Shared by build/rebuild/teardown so the role stays
/// consistent.
fn label_view<State: 'static>(label: String) -> AnyView<State> {
    frust::authoring::any::<State, _>(
        frust::text(label)
            .themed_role(ThemeTextColor::OnPrimaryContainer)
            .themed_family(ThemeTextType::LabelLarge),
    )
}

/// A declarative M3E FAB. See the [module docs](self).
pub struct FabView<State: 'static> {
    size: FabSize,
    color: FabColor,
    icon: Option<AnyView<State>>,
    /// Doubles as the extended FAB's visible text (when built via
    /// [`extended_fab`]) and every FAB's accessible name — an icon-only FAB
    /// has no visible text, so [`FabView::label`] is how it gets a
    /// screen-reader name.
    label: Option<String>,
    /// Whether this view was built via [`extended_fab`] (the pill-shaped
    /// variant) rather than [`fab`] — fixed by the constructor, not exposed
    /// as a public setter (mirrors the reference's separate `M3EFab`/
    /// `M3EExtendedFab` widget types).
    is_extended: bool,
    /// Whether the label is currently shown (only meaningful when
    /// `is_extended`) — [`FabView::extended`]'s target.
    label_shown: bool,
    /// [`FabView::corner_radius`]'s override — see the [module docs](self)'
    /// Corner-radius override seam section.
    corner_radius: Option<f64>,
    on_press: OnPress<State>,
}

/// Create an icon-only, medium-size FAB running `on_press` on release
/// inside its bounds. Chain [`FabView::size`]/[`FabView::color`] for the
/// other size/color tiers, and [`FabView::label`] to attach an accessible
/// name (recommended — an icon-only FAB has no visible text a screen reader
/// can read).
pub fn fab<State: 'static, F: Fn(&mut State) + 'static>(
    icon: AnyView<State>,
    on_press: F,
) -> FabView<State> {
    FabView {
        size: FabSize::Medium,
        color: FabColor::Primary,
        icon: Some(icon),
        label: None,
        is_extended: false,
        label_shown: true,
        corner_radius: None,
        on_press: Rc::new(on_press),
    }
}

/// Create an extended FAB labelled `label` (56dp fixed height), running
/// `on_press` on release inside its bounds. Chain [`FabView::icon`] to
/// attach an optional leading icon, [`FabView::color`] for the color tier,
/// and [`FabView::extended`] to control the label collapse/expand state.
pub fn extended_fab<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> FabView<State> {
    FabView {
        size: FabSize::Medium,
        color: FabColor::Primary,
        icon: None,
        label: Some(label.into()),
        is_extended: true,
        label_shown: true,
        corner_radius: None,
        on_press: Rc::new(on_press),
    }
}

impl<State: 'static> FabView<State> {
    /// Set the container size tier ([`fab`] only — ignored on an
    /// [`extended_fab`], whose height is always `EXTENDED_HEIGHT`).
    pub fn size(mut self, size: FabSize) -> Self {
        self.size = size;
        self
    }

    /// Set the container color tier (both variants).
    pub fn color(mut self, color: FabColor) -> Self {
        self.color = color;
        self
    }

    /// Attach a leading icon (usable on [`extended_fab`]; [`fab`] already
    /// requires one at construction).
    pub fn icon(mut self, icon: AnyView<State>) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Attach an accessible label ([`fab`]'s content description). A no-op
    /// override on an [`extended_fab`], whose visible label is already the
    /// accessible name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Whether the label is shown (only meaningful on an [`extended_fab`] —
    /// a no-op on a plain [`fab`], which has no label to show). Defaults to
    /// `true`. Setting `false` animates the label away, collapsing the pill
    /// toward an icon-only shape — see the [module docs](self)' Extended
    /// FAB section.
    pub fn extended(mut self, extended: bool) -> Self {
        self.label_shown = extended;
        self
    }

    /// Override the corner radius ([`fab`] only — a no-op on
    /// [`extended_fab`]; see the [module docs](self)' Corner-radius
    /// override seam section). `None` (the default) resolves the
    /// size-derived themed radius, easing toward a changed target with
    /// `MaterialMotion::SHORT_4`; `Some(r)` pins the radius to `r`
    /// immediately every frame, with no internal animation.
    pub fn corner_radius(mut self, radius: Option<f64>) -> Self {
        self.corner_radius = radius;
        self
    }
}

/// The retained widget for a [`FabView`].
pub struct FabWidget {
    size: FabSize,
    color: FabColor,
    is_extended: bool,
    label_shown: bool,
    corner_radius: Option<f64>,
    icon: Option<ChildPod>,
    /// The extended FAB's visible label child — `None` for an icon-only FAB
    /// or an extended FAB with no label (shouldn't normally happen, but not
    /// enforced at the type level). Stays mounted regardless of
    /// `label_shown` so it remains measurable while collapsed.
    visible_label: Option<ChildPod>,
    /// The accessible name (see [`FabView::label`]'s docs); always
    /// populated for an extended FAB from its visible text.
    label_text: Option<String>,
    pressed: bool,
    captured: bool,
    state_layer: StateLayer,
    /// Drives the press-scale spring — `0.0` at rest, `1.0` (or briefly
    /// beyond, under-damped) while pressed. See the [module docs](self)'
    /// Press scale section.
    press_anim: AnimationController,
    /// Drives the extended FAB's label reveal fraction — `0.0` (collapsed)
    /// .. `1.0` (fully shown). See the [module docs](self)' Extended FAB
    /// section.
    label_anim: AnimationController,
    /// The corner radius `radius_anim` is currently animating *from* (dp).
    radius_from: f64,
    /// The corner radius `radius_anim` is currently animating *to* (dp) —
    /// the last resolved target (override or themed).
    radius_to: f64,
    /// Becomes `true` after the first paint has resolved an initial radius
    /// target — the mount frame snaps directly to it with no animation
    /// (an `AnimatedContainer`'s first build is never itself animated; only
    /// a later prop change triggers motion).
    radius_seeded: bool,
    /// Duration+curve-driven (`MaterialMotion::SHORT_4`/`STANDARD`) unless
    /// an override is active, in which case it is left idle and unused —
    /// see [`FabWidget::advance_radius`].
    radius_anim: AnimationController,
    on_press: frust::authoring::ErasedCallback,
}

impl<State: 'static> View<State> for FabView<State> {
    type Element = FabWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> FabWidget {
        let icon = self
            .icon
            .as_ref()
            .map(|icon| frust::authoring::build_child(icon, ctx));
        let visible_label = if self.is_extended {
            self.label.as_ref().map(|label| {
                frust::authoring::build_child(&label_view::<State>(label.clone()), ctx)
            })
        } else {
            None
        };
        let mut label_anim = AnimationController::new(crate::tokens::MaterialMotion::MEDIUM_2)
            .with_curve(crate::tokens::MaterialMotion::EMPHASIZED);
        if self.label_shown {
            label_anim.forward();
        }
        FabWidget {
            size: self.size,
            color: self.color,
            is_extended: self.is_extended,
            label_shown: self.label_shown,
            corner_radius: self.corner_radius,
            icon,
            visible_label,
            label_text: self.label.clone(),
            pressed: false,
            captured: false,
            state_layer: StateLayer::new(),
            press_anim: AnimationController::new(PRESS_ANIM_PERIOD),
            label_anim,
            radius_from: 0.0,
            radius_to: 0.0,
            radius_seeded: false,
            radius_anim: AnimationController::new(crate::tokens::MaterialMotion::SHORT_4)
                .with_curve(crate::tokens::MaterialMotion::STANDARD),
            on_press: frust::authoring::erase_callback(&self.on_press),
        }
    }

    fn rebuild(&self, prev: &Self, element: &mut FabWidget, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        element.on_press = frust::authoring::erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;

        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.color != self.color {
            element.color = self.color;
            flags |= ChangeFlags::PAINT;
        }
        if prev.is_extended != self.is_extended {
            element.is_extended = self.is_extended;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.label_shown != self.label_shown {
            element.label_shown = self.label_shown;
            if self.label_shown {
                element.label_anim.forward();
            } else {
                element.label_anim.reverse();
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.corner_radius != self.corner_radius {
            element.corner_radius = self.corner_radius;
            flags |= ChangeFlags::PAINT;
        }

        match (&prev.icon, &self.icon) {
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

        if self.is_extended {
            // Whether `visible_label` starts this block unmounted — including
            // a Plain->Extended swap that carried no prior pod at all, since
            // `element` is reused across the type flip (see this `rebuild`'s
            // top-level docs and G9's device-gate note below).
            let freshly_mounted = element.visible_label.is_none();
            match (&prev.label, &self.label) {
                (None, None) => {}
                (Some(p), Some(n)) if p == n && prev.is_extended == self.is_extended => {}
                (Some(_), Some(n)) => {
                    element.label_text = Some(n.clone());
                    if let Some(pod) = element.visible_label.as_mut() {
                        let prev_view = label_view::<State>(prev.label.clone().unwrap_or_default());
                        let next_view = label_view::<State>(n.clone());
                        flags |= frust::authoring::rebuild_child(&prev_view, &next_view, pod, ctx);
                    } else {
                        element.visible_label = Some(frust::authoring::build_child(
                            &label_view::<State>(n.clone()),
                            ctx,
                        ));
                        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                    }
                }
                (None, Some(n)) => {
                    element.label_text = Some(n.clone());
                    element.visible_label = Some(frust::authoring::build_child(
                        &label_view::<State>(n.clone()),
                        ctx,
                    ));
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                (Some(_), None) => {
                    element.label_text = None;
                    if let Some(mut pod) = element.visible_label.take() {
                        let prev_view = label_view::<State>(prev.label.clone().unwrap_or_default());
                        frust::authoring::teardown_child(&prev_view, &mut pod, ctx);
                    }
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
            // A pod that just mounted (None -> Some above) — the common case
            // being a Plain->Extended swap — mirrors `build()`'s own mount
            // behavior exactly: reset the reveal controller fresh rather
            // than inherit whatever value it drifted to while this FAB was
            // plain (`paint` advances `label_anim` every frame regardless of
            // `is_extended`, since `is_extended` can itself flip via this
            // same `rebuild`). Without this, a Plain->Extended swap could
            // land on an already-`Completed` controller and snap straight to
            // the fully open pill instead of reveal-animating in — see the
            // [module docs](self)' "known v1 mount quirk".
            if freshly_mounted && element.visible_label.is_some() {
                element.label_anim =
                    AnimationController::new(crate::tokens::MaterialMotion::MEDIUM_2)
                        .with_curve(crate::tokens::MaterialMotion::EMPHASIZED);
                if self.label_shown {
                    element.label_anim.forward();
                }
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        } else {
            // Icon-only FAB: tear down any stale visible-label pod carried
            // over from an Extended->Plain swap (`element` is reused across
            // the type flip since the rail diffs same-typed `FabView`s via
            // `rebuild_child` — see the [module docs](self)' Extended FAB
            // section). `layout`'s square branch never lays out
            // `visible_label`, so a pod left mounted here paints at its
            // stale prior origin/size, outside the square — the G9
            // device-gate defect this guards against.
            if prev.is_extended
                && let Some(mut pod) = element.visible_label.take()
            {
                let prev_view = label_view::<State>(prev.label.clone().unwrap_or_default());
                frust::authoring::teardown_child(&prev_view, &mut pod, ctx);
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            if prev.label != self.label {
                // The label is accessibility-only, no child to reconcile.
                element.label_text = self.label.clone();
                flags |= ChangeFlags::PAINT;
            }
        }

        flags
    }

    fn teardown(&self, element: &mut FabWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(icon_view), Some(pod)) = (&self.icon, element.icon.as_mut()) {
            frust::authoring::teardown_child(icon_view, pod, ctx);
        }
        if self.is_extended
            && let (Some(label), Some(pod)) = (&self.label, element.visible_label.as_mut())
        {
            let label_view = label_view::<State>(label.clone());
            frust::authoring::teardown_child(&label_view, pod, ctx);
        }
    }
}

impl FabWidget {
    /// The current corner-radius target: the override (when set, on a plain
    /// [`fab`]) or the size-/`is_extended`-derived themed/fallback radius.
    /// See the [module docs](self)' Corner-radius override seam section.
    fn resolve_radius_target(&self, theme: Option<&Theme>) -> f64 {
        if let Some(r) = self.corner_radius
            && !self.is_extended
        {
            return r;
        }
        match theme {
            Some(theme) => {
                if self.is_extended {
                    theme.shape.large
                } else {
                    self.size.themed_radius(&theme.shape)
                }
            }
            None => {
                if self.is_extended {
                    EXTENDED_RADIUS_FALLBACK
                } else {
                    self.size.fallback_radius()
                }
            }
        }
    }

    /// Advance (or seed/retarget) [`FabWidget::radius_anim`] toward
    /// `target`, returning the radius to paint this frame. `immediate`
    /// (the override-active case) snaps directly with no animation;
    /// otherwise a changed target eases in over `MaterialMotion::SHORT_4`.
    /// See the [module docs](self)' Corner-radius override seam section.
    fn advance_radius(&mut self, ctx: &mut PaintCtx, target: f64, immediate: bool) -> f64 {
        if !self.radius_seeded {
            self.radius_from = target;
            self.radius_to = target;
            self.radius_seeded = true;
        } else if (self.radius_to - target).abs() > f64::EPSILON {
            if immediate {
                self.radius_from = target;
                self.radius_to = target;
                self.radius_anim = AnimationController::new(Duration::ZERO);
            } else {
                let current = Tween::new(self.radius_from, self.radius_to)
                    .lerp(self.radius_anim.value_clamped());
                self.radius_from = current;
                self.radius_to = target;
                self.radius_anim = AnimationController::new(crate::tokens::MaterialMotion::SHORT_4)
                    .with_curve(crate::tokens::MaterialMotion::STANDARD);
                self.radius_anim.forward();
            }
        }
        if self.radius_anim.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        Tween::new(self.radius_from, self.radius_to).lerp(self.radius_anim.value_clamped())
    }
}

impl Widget for FabWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        if !self.is_extended {
            let container = self.size.container();
            if let Some(icon) = self.icon.as_mut() {
                let dim = self.size.icon_size();
                let icon_size = icon.layout_child(ctx, &BoxConstraints::tight(Size::new(dim, dim)));
                icon.set_origin(Point::new(
                    (container - icon_size.width) / 2.0,
                    (container - icon_size.height) / 2.0,
                ));
            }
            return bc.constrain(Size::new(container, container));
        }

        let height = EXTENDED_HEIGHT;
        let t = self.label_anim.value_clamped();
        let pad = Tween::new(EXTENDED_PAD_COLLAPSED, EXTENDED_PAD_EXPANDED).lerp(t);
        let has_icon = self.icon.is_some();

        let mut x = pad;
        if let Some(icon) = self.icon.as_mut() {
            let icon_size = icon.layout_child(
                ctx,
                &BoxConstraints::tight(Size::new(EXTENDED_ICON, EXTENDED_ICON)),
            );
            icon.set_origin(Point::new(x, (height - icon_size.height) / 2.0));
            x += icon_size.width;
        }
        if let Some(label) = self.visible_label.as_mut() {
            let label_size = label.layout_child(
                ctx,
                &BoxConstraints::loose(Size::new(f64::INFINITY, height)),
            );
            let gap = if has_icon {
                EXTENDED_ICON_LABEL_GAP * t
            } else {
                0.0
            };
            x += gap;
            label.set_origin(Point::new(x, (height - label_size.height) / 2.0));
            x += label_size.width * t;
        }
        x += pad;
        bc.constrain(Size::new(x, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Resolve every theme-derived value up front, before any `&mut ctx`
        // use below (`advance_radius`/`request_frame`) — `theme` borrows
        // `ctx` shared, and its last use must precede a later mutable
        // reborrow for this to typecheck under NLL (mirrors
        // `frust_widgets::Button::paint`'s own documented ordering).
        let theme = Theme::from_paint_ctx(ctx);
        let (container_color, content_color) = resolve_colors(theme, self.color);
        let hovered = ctx.is_hovered();
        let radius_target = self.resolve_radius_target(theme);
        let (blur, y_offset, shadow_color) = resolve_shadow(theme, hovered);

        self.state_layer.set_hovered(hovered);

        let immediate = self.corner_radius.is_some() && !self.is_extended;
        let radius = self.advance_radius(ctx, radius_target, immediate);

        if self.press_anim.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        let pressed_scale = if self.is_extended {
            EXTENDED_PRESSED_SCALE
        } else {
            FAB_PRESSED_SCALE
        };
        let scale = Tween::new(1.0, pressed_scale).lerp(self.press_anim.value());

        // The label reveal is a **layout** value — `FabWidget::layout`'s
        // extended branch lerps pad/gap/label width straight off
        // `label_anim.value_clamped()` — so an in-flight reveal needs an
        // explicit relayout request, not merely another frame: on the
        // mobile intra-frame layout skip (`docs/SHELLS_ARCHITECTURE.md`'s
        // `frame_gate`), layout does not re-run merely because paint asked
        // for another frame. This mirrors the trap
        // [`mod@super::navigation_rail`]'s width motion and
        // [`mod@super::expandable_list`]'s reveal both document at length;
        // comparing the value before/after (rather than trusting
        // `advance`'s return alone) catches the exact settling frame, where
        // `advance` reports "done" but `value` still moved to its final
        // target on this call — the same reason `WidthMotion`'s caller does
        // the same comparison.
        let label_before = self.label_anim.value_clamped();
        let label_animating = self.label_anim.advance(ctx.frame_time());
        if label_animating || self.label_anim.value_clamped() != label_before {
            ctx.request_layout();
        }

        let o = ctx.origin();
        let size = ctx.size();
        let pivot = Point::new(o.x + size.width / 2.0, o.y + size.height / 2.0);
        scene.push_transform(scale_about(pivot, scale));

        scene.draw_shadow(
            Point::new(o.x, o.y + y_offset),
            size,
            radius,
            blur,
            shadow_color,
        );
        scene.fill_rounded_rect(o, size, radius, container_color);
        self.state_layer.paint(
            ctx,
            scene,
            Rect::from_origin_size(o, size),
            radius,
            content_color,
        );
        if let Some(icon) = self.icon.as_mut() {
            icon.paint_child(ctx, scene);
        }
        // Belt-and-braces: `layout`'s square branch never lays out
        // `visible_label`, so gate its paint on `is_extended` too — the
        // `rebuild` fix above already keeps `visible_label` `None` whenever
        // `is_extended` is false, but this keeps paint itself from ever
        // drawing a pod at a stale origin if that invariant is broken later
        // (see G9's device-gate note on [`FabView::rebuild`]).
        if self.is_extended
            && let Some(label) = self.visible_label.as_mut()
        {
            label.paint_child(ctx, scene);
        }

        scene.pop_transform();
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
                self.press_anim.fling(FLING_VELOCITY, PRESS_SPRING);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    // No capture: this is the hover pass — see the [module
                    // docs](self)' Elevation section and
                    // `docs/CODE_STANDARDS.md`'s Interaction Semantics.
                    let over = inside(p.position, ctx.size());
                    if over {
                        ctx.claim_hover();
                    }
                    if self.state_layer.set_hovered(over) {
                        ctx.request_redraw();
                    }
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
                    (self.on_press)(ctx);
                }
                self.pressed = false;
                self.captured = false;
                self.state_layer.set_pressed(false);
                self.press_anim.fling(-FLING_VELOCITY, PRESS_SPRING);
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
                self.press_anim.fling(-FLING_VELOCITY, PRESS_SPRING);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Button, |node| {
            if let Some(label) = &self.label_text {
                node.set_label(label.as_str());
            }
            node.add_action(Action::Click);
        });
    }

    frust::authoring::visit_children!(icon, visible_label);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Primary,
        })
    }

    /// Records each rounded rect's `(origin, size, radius, color)`, each
    /// shadow's `(origin, size, radius, std_dev, color)`, and push/pop
    /// transform activity.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        transforms: Vec<Affine>,
        transform_pops: u32,
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
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn pop_transform(&mut self) {
            self.transform_pops += 1;
        }
    }

    fn build_icon_fab(size: FabSize) -> FabWidget {
        let view = fab::<(), _>(leaf_any(24.0, 24.0), |_s: &mut ()| {}).size(size);
        let mut counter = 0u64;
        View::<()>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn container_sizes_match_m3e_theme_table() {
        // m3e_fab_theme.dart: smallContainer=40, mediumContainer=56,
        // largeContainer=96.
        assert_eq!(FabSize::Small.container(), 40.0);
        assert_eq!(FabSize::Medium.container(), 56.0);
        assert_eq!(FabSize::Large.container(), 96.0);
    }

    #[test]
    fn icon_sizes_match_m3e_theme_table() {
        // m3e_fab_theme.dart: smallIconSize=24, mediumIconSize=24,
        // largeIconSize=36.
        assert_eq!(FabSize::Small.icon_size(), 24.0);
        assert_eq!(FabSize::Medium.icon_size(), 24.0);
        assert_eq!(FabSize::Large.icon_size(), 36.0);
    }

    #[test]
    fn themed_radii_match_m3e_theme_table() {
        // m3e_fab_theme.dart: smallRadius=12, mediumRadius=16,
        // largeRadius=28 — this crate maps them onto shape.medium/large/
        // extra_large, whose baseline values coincide exactly.
        let theme = crate::baseline();
        assert_eq!(FabSize::Small.themed_radius(&theme.shape), 12.0);
        assert_eq!(FabSize::Medium.themed_radius(&theme.shape), 16.0);
        assert_eq!(FabSize::Large.themed_radius(&theme.shape), 28.0);
    }

    #[test]
    fn color_mapping_matches_m3e_theme_table_themed() {
        let theme = crate::baseline();
        let s = theme.scheme();
        assert_eq!(
            resolve_colors(Some(&theme), FabColor::Primary),
            (s.primary_container, s.on_primary_container)
        );
        assert_eq!(
            resolve_colors(Some(&theme), FabColor::Secondary),
            (s.secondary_container, s.on_secondary_container)
        );
        assert_eq!(
            resolve_colors(Some(&theme), FabColor::Tertiary),
            (s.tertiary_container, s.on_tertiary_container)
        );
        // Surface's content color is `primary`, NOT `on_surface` — verified
        // from `m3e_fab_theme.dart`'s `_palette` switch.
        assert_eq!(
            resolve_colors(Some(&theme), FabColor::Surface),
            (s.surface_container_high, s.primary)
        );
    }

    #[test]
    fn color_mapping_matches_m3e_theme_table_unthemed() {
        assert_eq!(
            resolve_colors(None, FabColor::Primary),
            (PRIMARY_CONTAINER, ON_PRIMARY_CONTAINER)
        );
        assert_eq!(
            resolve_colors(None, FabColor::Secondary),
            (SECONDARY_CONTAINER, ON_SECONDARY_CONTAINER)
        );
        assert_eq!(
            resolve_colors(None, FabColor::Tertiary),
            (TERTIARY_CONTAINER, ON_TERTIARY_CONTAINER)
        );
        assert_eq!(
            resolve_colors(None, FabColor::Surface),
            (SURFACE_CONTAINER_HIGH, SURFACE_FOREGROUND)
        );
    }

    #[test]
    fn press_spring_matches_reference() {
        // m3e_floating_action_buttons.dart's module doc: "plays a spatial
        // spring press scale (380 / 0.55)" == M3EMotion.expressiveSpatialPress.
        let spring = crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS;
        assert_eq!(PRESS_SPRING.stiffness, spring.stiffness);
        assert_eq!(PRESS_SPRING.damping_ratio, spring.damping_ratio);
        assert_eq!(PRESS_SPRING.mass, 1.0);
    }

    #[test]
    fn pressed_scale_targets_match_m3e_theme_table() {
        // M3EFabTheme.pressedScale=0.95, M3EExtendedFabTheme.pressedScale=0.97.
        assert_eq!(FAB_PRESSED_SCALE, 0.95);
        assert_eq!(EXTENDED_PRESSED_SCALE, 0.97);
    }

    #[test]
    fn layout_is_a_fixed_square_at_the_container_size() {
        let mut w = build_icon_fab(FabSize::Large);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(96.0, 96.0));
        let icon = w.icon.as_ref().unwrap();
        assert_eq!(icon.size(), Size::new(36.0, 36.0));
        // centered
        assert_eq!(icon.origin(), Point::new(30.0, 30.0));
    }

    #[test]
    fn unthemed_paint_uses_fallback_container_and_shadow() {
        let mut w = build_icon_fab(FabSize::Medium);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(56.0, 56.0));
        w.paint(&mut pctx, &mut rec);

        assert_eq!(rec.rrects[0].3, PRIMARY_CONTAINER);
        assert_eq!(rec.rrects[0].2, MEDIUM_RADIUS);
        assert_eq!(rec.shadows.len(), 1, "a resting FAB paints one shadow");
        assert_eq!(rec.shadows[0].3, LEVEL3_SHADOW_BLUR);
        assert_eq!(rec.shadows[0].4, FALLBACK_SHADOW_COLOR);
        assert_eq!(
            rec.shadows[0].0,
            Point::new(0.0, LEVEL3_SHADOW_Y_OFFSET),
            "the shadow origin is offset by y_offset"
        );
        assert_eq!(rec.transforms.len(), 1, "one press-scale transform pushed");
        assert_eq!(rec.transform_pops, 1, "paired with one pop");
    }

    #[test]
    fn themed_paint_resolves_primary_container_tokens() {
        let theme = crate::baseline();
        let mut w = build_icon_fab(FabSize::Small);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(40.0, 40.0)).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);

        let scheme = theme.scheme();
        assert_eq!(rec.rrects[0].3, scheme.primary_container);
        assert_eq!(rec.rrects[0].2, theme.shape.medium);
        assert_eq!(
            rec.shadows[0].4.components[3],
            theme.elevation.level3.shadow(theme.brightness).color_alpha
        );
    }

    #[test]
    fn extended_layout_has_fixed_height_and_grows_width() {
        let view: FabView<()> =
            extended_fab("Compose", |_s: &mut ()| {}).icon(leaf_any(24.0, 24.0));
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        // Settle the label reveal to fully open (see the module docs' known
        // v1 mount quirk: a freshly built controller hasn't been advanced
        // yet, so it still reads its untouched `0.0`).
        w.label_anim.advance(FrameTime::ZERO);
        w.label_anim.advance(ft_secs(1.0));
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size.height, EXTENDED_HEIGHT);
        assert!(size.width > EXTENDED_ICON + EXTENDED_PAD_EXPANDED * 2.0);
    }

    #[test]
    fn extended_label_collapse_animates_width_between_endpoints() {
        // Fully extended: settle the label reveal to 1.0.
        let extended_view: FabView<()> =
            extended_fab("Compose", |_s: &mut ()| {}).icon(leaf_any(24.0, 24.0));
        let mut counter = 0u64;
        let mut w = View::<()>::build(&extended_view, &mut BuildCtx::new(&mut counter));
        w.label_anim.advance(FrameTime::ZERO);
        w.label_anim.advance(ft_secs(1.0)); // >> MEDIUM_2 (300ms): settles at 1.0
        assert_eq!(w.label_anim.value_clamped(), 1.0);
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let extended_size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));

        // Fully collapsed: built with `.extended(false)` — the controller's
        // untouched resting value (`0.0`) already matches, no advance needed.
        let collapsed_view: FabView<()> = extended_fab("Compose", |_s: &mut ()| {})
            .icon(leaf_any(24.0, 24.0))
            .extended(false);
        let mut w2 = View::<()>::build(&collapsed_view, &mut BuildCtx::new(&mut counter));
        assert_eq!(w2.label_anim.value_clamped(), 0.0);
        let mut tcx2 = frust::authoring::text::TextContext::new();
        let mut lctx2 = LayoutCtx::with_text_context(&mut tcx2 as &mut dyn Any);
        let collapsed_size = w2.layout(&mut lctx2, &BoxConstraints::loose(Size::new(500.0, 500.0)));

        assert_eq!(extended_size.height, EXTENDED_HEIGHT);
        assert_eq!(collapsed_size.height, EXTENDED_HEIGHT);
        assert!(
            collapsed_size.width < extended_size.width,
            "a collapsed extended FAB is narrower than a fully open one"
        );
        // The collapsed pill's exact M3E formula: pad(16)*2 + icon(24) = 56.
        assert_eq!(
            collapsed_size.width,
            EXTENDED_PAD_COLLAPSED * 2.0 + EXTENDED_ICON
        );
    }

    /// The regression guard for G9 (device-gate round 3): the rail swaps
    /// `extended_fab()`/`fab()` via `rebuild_child` on the same `FabView`
    /// type when its type toggles — pre-fix, `visible_label` was built only
    /// in `build()`, so an Extended->Plain swap left the label pod mounted,
    /// painting stale text outside the collapsed square (screenshot-
    /// confirmed on the Xiaomi).
    #[test]
    fn rebuild_extended_to_plain_tears_down_the_stale_label_pod() {
        let extended_view: FabView<()> =
            extended_fab("Compose", |_s: &mut ()| {}).icon(leaf_any(24.0, 24.0));
        let mut counter = 0u64;
        let mut w = View::<()>::build(&extended_view, &mut BuildCtx::new(&mut counter));
        assert!(
            w.visible_label.is_some(),
            "an extended FAB mounts a visible label pod"
        );

        let plain_view: FabView<()> = fab::<(), _>(leaf_any(24.0, 24.0), |_s: &mut ()| {});
        View::<()>::rebuild(
            &plain_view,
            &extended_view,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        assert!(
            w.visible_label.is_none(),
            "swapping to a plain FAB tears down the stale label pod"
        );

        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(
            size,
            Size::new(MEDIUM_CONTAINER, MEDIUM_CONTAINER),
            "the collapsed FAB lays out as the plain square, not the old pill"
        );
    }

    /// The reverse of the above: a Plain->Extended swap must mount the
    /// label pod and reveal-animate it in across advanced frames, not snap
    /// straight to the open pill from whatever value `label_anim` drifted
    /// to while this FAB was plain (`paint` advances it every frame
    /// regardless of `is_extended`).
    #[test]
    fn rebuild_plain_to_extended_mounts_the_label_and_reveal_animates() {
        let plain_view: FabView<()> = fab::<(), _>(leaf_any(24.0, 24.0), |_s: &mut ()| {});
        let mut counter = 0u64;
        let mut w = View::<()>::build(&plain_view, &mut BuildCtx::new(&mut counter));
        // Drift the plain widget's (layout-irrelevant) `label_anim` to fully
        // settled, exactly as real paint calls would over several frames —
        // this is the state a naively-reused controller would wrongly
        // inherit on the swap below.
        w.label_anim.advance(FrameTime::ZERO);
        w.label_anim.advance(ft_secs(1.0));
        assert_eq!(w.label_anim.value_clamped(), 1.0);
        assert!(w.visible_label.is_none());

        let extended_view: FabView<()> =
            extended_fab("Compose", |_s: &mut ()| {}).icon(leaf_any(24.0, 24.0));
        View::<()>::rebuild(
            &extended_view,
            &plain_view,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        assert!(
            w.visible_label.is_some(),
            "swapping to an extended FAB mounts the label pod"
        );

        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let just_swapped_size =
            w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));

        // Settle the label reveal to fully open (the module docs' known v1
        // mount quirk: the first `advance` after a fresh `forward()` only
        // seeds the clock).
        w.label_anim.advance(FrameTime::ZERO);
        w.label_anim.advance(ft_secs(1.0));
        let mut tcx2 = frust::authoring::text::TextContext::new();
        let mut lctx2 = LayoutCtx::with_text_context(&mut tcx2 as &mut dyn Any);
        let grown_size = w.layout(&mut lctx2, &BoxConstraints::loose(Size::new(500.0, 500.0)));

        assert!(
            grown_size.width > just_swapped_size.width,
            "the label grows in across advanced frames: {just_swapped_size:?} -> {grown_size:?}"
        );
    }

    /// The regression guard for the layout-skip trap (see the module docs'
    /// Extended FAB section): the pill's width is computed in `layout` from
    /// `label_anim.value_clamped()`, so every in-flight reveal frame must ask
    /// for a relayout — a paint-only frame request would freeze the pill at
    /// its collapsed (~56dp) square width under the mobile intra-frame
    /// layout skip, exactly the screenshot-confirmed device defect this test
    /// pins.
    #[test]
    fn paint_requests_layout_while_label_reveal_is_in_flight_and_stops_once_settled() {
        fn layout_at(w: &mut FabWidget) -> Size {
            let mut tcx = frust::authoring::text::TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)))
        }
        fn paint_at(w: &mut FabWidget, size: Size, t: f64) -> (Recorder, bool) {
            let mut rec = Recorder::default();
            let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_secs(t));
            w.paint(&mut ctx, &mut rec);
            let needs_layout = ctx.needs_layout();
            (rec, needs_layout)
        }

        let collapsed: FabView<()> = extended_fab("Compose", |_s: &mut ()| {})
            .icon(leaf_any(24.0, 24.0))
            .extended(false);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&collapsed, &mut BuildCtx::new(&mut counter));
        let mut size = layout_at(&mut w);
        let collapsed_width = size.width;

        // Rebuild into the extended state: this starts the reveal
        // (`label_anim.forward()`) but hasn't advanced it yet — layout still
        // reads the untouched `0.0`, matching the collapsed width exactly
        // (the module docs' known v1 mount quirk).
        let expanded: FabView<()> =
            extended_fab("Compose", |_s: &mut ()| {}).icon(leaf_any(24.0, 24.0));
        View::<()>::rebuild(
            &expanded,
            &collapsed,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        size = layout_at(&mut w);
        assert_eq!(size.width, collapsed_width);

        let mut widths = vec![size.width];
        let mut frames = 0u32;
        let mut t = 0.0;
        loop {
            let (_, needs_layout) = paint_at(&mut w, size, t);
            if !needs_layout {
                break;
            }
            frames += 1;
            // A real shell relayouts on seeing `needs_layout` — this is the
            // exact step the pre-fix code never asked for, freezing the pill
            // at `collapsed_width` while the label kept painting past it.
            size = layout_at(&mut w);
            widths.push(size.width);
            t += 1.0 / 60.0;
            assert!(
                frames < 600,
                "the reveal should settle well inside 600 frames"
            );
        }
        assert!(frames > 1, "the reveal spans more than one frame");
        assert!(
            widths.windows(2).all(|pair| pair[1] + 1e-9 >= pair[0]),
            "the pill's laid-out width never shrinks while opening: {widths:?}"
        );
        let last = *widths.last().expect("at least one frame recorded");
        assert!(
            last > collapsed_width,
            "the settled pill is wider than the frozen collapsed square: {last} > {collapsed_width}"
        );

        // A settled pill stops asking for layout.
        let (_, still) = paint_at(&mut w, size, t + 1.0 / 60.0);
        assert!(!still, "settled: no more layout requests");
    }

    #[test]
    fn corner_radius_override_is_immediate_no_animation() {
        let view: FabView<()> =
            fab::<(), _>(leaf_any(24.0, 24.0), |_s: &mut ()| {}).corner_radius(Some(40.0));
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(56.0, 56.0));
        w.paint(&mut pctx, &mut rec);
        assert_eq!(rec.rrects[0].2, 40.0, "paints the override radius exactly");

        // A second, different override snaps immediately too — no lingering
        // easing from the first value.
        let view2: FabView<()> =
            fab::<(), _>(leaf_any(24.0, 24.0), |_s: &mut ()| {}).corner_radius(Some(8.0));
        let mut lctx2 = LayoutCtx::new();
        View::<()>::rebuild(&view2, &view, &mut w, &mut BuildCtx::new(&mut counter));
        w.layout(&mut lctx2, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        let mut rec2 = Recorder::default();
        let mut pctx2 = PaintCtx::new(Point::ZERO, Size::new(56.0, 56.0));
        w.paint(&mut pctx2, &mut rec2);
        assert_eq!(
            rec2.rrects[0].2, 8.0,
            "a changed override still snaps immediately, no ease from the prior value"
        );
    }

    #[test]
    fn corner_radius_none_eases_a_changed_target_with_short_4() {
        let mut w = build_icon_fab(FabSize::Small);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));

        let theme = crate::baseline();
        // First paint: seeds at the small-size themed radius (12dp), no
        // animation from an unknown prior value.
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(40.0, 40.0)).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);
        assert_eq!(rec.rrects[0].2, 12.0);

        // Switch to Large (target 28dp): the very next paint call must
        // still read the *old* radius (progress not yet advanced past the
        // clock-seeding first `advance`), then settle at 28dp once advanced
        // further — matching `MaterialMotion::SHORT_4` (200ms).
        let prev_view: FabView<()> =
            fab::<(), _>(leaf_any(24.0, 24.0), |_s: &mut ()| {}).size(FabSize::Small);
        let next_view: FabView<()> =
            fab::<(), _>(leaf_any(24.0, 24.0), |_s: &mut ()| {}).size(FabSize::Large);
        let mut counter = 1u64;
        View::<()>::rebuild(
            &next_view,
            &prev_view,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));

        let mut rec2 = Recorder::default();
        let mut pctx2 = PaintCtx::new(Point::ZERO, Size::new(96.0, 96.0)).with_theme(&theme);
        w.paint(&mut pctx2, &mut rec2);
        assert_eq!(
            rec2.rrects[0].2, 12.0,
            "the change-triggering frame hasn't eased yet (first advance only seeds the clock)"
        );

        // Advance well past SHORT_4 (200ms): settles at the new target.
        w.radius_anim.advance(ft_secs(1.0));
        let mut rec3 = Recorder::default();
        let mut pctx3 = PaintCtx::new(Point::ZERO, Size::new(96.0, 96.0)).with_theme(&theme);
        w.paint(&mut pctx3, &mut rec3);
        assert_eq!(
            rec3.rrects[0].2, 28.0,
            "settled at the new (Large) themed radius"
        );
    }

    /// A minimal `State`-generic icon stand-in (`test_support::leaf`/`leaf_any`
    /// only implement `View<()>`; the interactive tests below need a generic
    /// `State`, so a plain themed [`frust::text`] run stands in for the
    /// icon — its content is irrelevant to firing behavior).
    fn icon_stub<State: 'static>() -> AnyView<State> {
        frust::authoring::any::<State, _>(frust::text("i"))
    }

    fn dispatch(w: &mut FabWidget, state: &mut u32, event: &InputEvent, size: Size) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    #[test]
    fn up_inside_fires_on_press() {
        let view: FabView<u32> =
            fab(icon_stub::<u32>(), |s: &mut u32| *s += 1).size(FabSize::Medium);
        let mut counter = 0u64;
        let mut w = View::<u32>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut state = 0u32;
        let size = Size::new(56.0, 56.0);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 28.0, 28.0),
            size,
        );
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 28.0, 28.0), size);
        assert_eq!(state, 1);
    }

    #[test]
    fn up_outside_does_not_fire() {
        let view: FabView<u32> = fab(icon_stub::<u32>(), |s: &mut u32| *s += 1);
        let mut counter = 0u64;
        let mut w = View::<u32>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut state = 0u32;
        let size = Size::new(56.0, 56.0);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 10.0, 10.0),
            size,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 500.0, 500.0),
            size,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 500.0, 500.0),
            size,
        );
        assert_eq!(state, 0);
    }

    #[test]
    fn cancel_clears_armed_state_without_firing() {
        let view: FabView<u32> = fab(icon_stub::<u32>(), |s: &mut u32| *s += 1);
        let mut counter = 0u64;
        let mut w = View::<u32>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut state = 0u32;
        let size = Size::new(56.0, 56.0);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 10.0, 10.0),
            size,
        );
        assert!(w.captured);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Cancel, 10.0, 10.0),
            size,
        );
        assert!(!w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0), size);
        assert_eq!(state, 0);
    }

    #[test]
    fn hover_move_outside_bounds_without_down_is_ignored_noop() {
        let mut w = build_icon_fab(FabSize::Medium);
        let mut state = 0u32;
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(56.0, 56.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 500.0, 500.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(
            !ctx.needs_redraw(),
            "outside the bounds, nothing claims hover"
        );
    }

    #[test]
    fn hover_move_inside_bounds_without_down_claims_hover_and_redraws() {
        // Still `Ignored` — an uncaptured hover pass never consumes the
        // event — but it does claim the hover link and, since the flag
        // changed, asks for a redraw (see the module docs' Elevation
        // section and `docs/CODE_STANDARDS.md`'s Interaction Semantics).
        let mut w = build_icon_fab(FabSize::Medium);
        let mut state = 0u32;
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(56.0, 56.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 10.0, 10.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(ctx.needs_redraw());
    }

    #[test]
    fn down_starts_the_press_scale_spring_toward_pressed_and_up_reverses_it() {
        let view: FabView<u32> = fab(icon_stub::<u32>(), |_s: &mut u32| {}).size(FabSize::Medium);
        let mut counter = 0u64;
        let mut w = View::<u32>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut state = 0u32;
        let size = Size::new(56.0, 56.0);
        assert!(!w.press_anim.is_animating());

        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 28.0, 28.0),
            size,
        );
        assert!(
            w.press_anim.is_animating(),
            "Down starts the fling toward pressed"
        );

        // Settle the fling (bounded loop, mirrors `frust-core::anim`'s own
        // settle-loop test pattern).
        let mut t = 0.0;
        let mut running = true;
        for _ in 0..100_000 {
            running = w.press_anim.advance(ft_secs(t));
            if !running {
                break;
            }
            t += 1.0 / 120.0;
        }
        assert!(!running, "press-in fling failed to settle");
        let pressed_scale = Tween::new(1.0, FAB_PRESSED_SCALE).lerp(w.press_anim.value());
        assert!((pressed_scale - FAB_PRESSED_SCALE).abs() < 1e-6);

        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 28.0, 28.0), size);
        assert!(
            w.press_anim.is_animating(),
            "Up starts the fling back to rest"
        );
        t = 0.0;
        running = true;
        for _ in 0..100_000 {
            running = w.press_anim.advance(ft_secs(t));
            if !running {
                break;
            }
            t += 1.0 / 120.0;
        }
        assert!(!running, "press-out fling failed to settle");
        let rest_scale = Tween::new(1.0, FAB_PRESSED_SCALE).lerp(w.press_anim.value());
        assert!((rest_scale - 1.0).abs() < 1e-6);
    }

    #[test]
    fn semantics_reports_button_role_and_label() {
        fn logic(_s: &mut ()) -> FabView<()> {
            fab::<(), _>(leaf_any(24.0, 24.0), |_s: &mut ()| {}).label("Compose")
        }
        let mut root: frust_core::RenderRoot<(), FabView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("fab contributes a Role::Button node");
        assert_eq!(node.label(), Some("Compose"));
        assert!(node.supports_action(Action::Click));
    }

    #[test]
    fn extended_semantics_uses_visible_label_as_accessible_name() {
        fn logic(_s: &mut ()) -> FabView<()> {
            extended_fab::<(), _>("New item", |_s: &mut ()| {})
        }
        let mut root: frust_core::RenderRoot<(), FabView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("extended fab contributes a Role::Button node");
        assert_eq!(node.label(), Some("New item"));
    }

    /// A single FAB under a real `RenderRoot` — the only harness that can
    /// exercise hover at all, since the hover link is recorded by the
    /// root's event pass and read back through `PaintCtx::is_hovered`
    /// (mirrors `list_item.rs`'s own `HoverHarness`).
    #[test]
    fn hover_lifts_elevation_from_level3_to_level4() {
        fn logic(_s: &mut ()) -> FabView<()> {
            fab::<(), _>(leaf_any(24.0, 24.0), |_s: &mut ()| {})
        }
        let mut root: frust_core::RenderRoot<(), FabView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));

        let mut rec = Recorder::default();
        root.paint(&mut rec, frust::FrameTime::ZERO);
        assert_eq!(
            rec.shadows[0].3, LEVEL3_SHADOW_BLUR,
            "resting FAB is at level 3"
        );

        root.event(
            &mut state,
            &InputEvent::Pointer(frust::authoring::PointerEvent {
                phase: PointerPhase::Move,
                position: Point::new(28.0, 28.0),
                button: frust::authoring::PointerButton::Primary,
            }),
        );
        let mut rec2 = Recorder::default();
        root.paint(&mut rec2, frust::FrameTime::ZERO);
        assert_eq!(
            rec2.shadows[0].3, LEVEL4_SHADOW_BLUR,
            "a hovered FAB lifts to level 4"
        );
    }
}
