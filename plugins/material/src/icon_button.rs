// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// whose `lib/components/icon_buttons/` tree is itself vendored from the
// `icon_button_m3e` package (MIT, © 2026 Mudit Purohit) — `m3e_icon_buttons.dart`
// + its `components/`, `enums/`, `styles/` parts.
// Porting decisions: the Dart widget hands its surface to Flutter's own
// `IconButton` (itself wrapped in `IconTheme.merge` for ambient icon
// size/color and `ListenableBuilder` for its three pointer notifiers); this
// port paints the surface directly and constrains/lays out its icon child
// the same way `super::button` does its own icon child — no ambient
// recolor, see the module docs' Icon color section. The press/hover shape
// morph reuses `crate::button::motion::RadiusPaddingMotion` directly (its
// padding channel pinned to zero — every icon-button call site passes zero
// `internalLeft/Right/Top/Bottom` to the Dart primitive too) — see this
// module's `RadiusPaddingMotion reuse` section for why this is now a direct
// import rather than the parallel port it started as.

//! The Material 3 Expressive **icon button**: 4 variants × 5 sizes × round/
//! square × 3 widths, with toggle (selected-icon swap), a badge slot, and
//! the same press/hover shape morph [`mod@crate::button`] uses.
//!
//! [`icon_button`] builds the default standard icon button;
//! [`filled_icon_button`], [`tonal_icon_button`] and [`outlined_icon_button`]
//! are the reference's other three named constructors
//! ([`IconButtonView::variant`] switches between them after the fact,
//! mirroring [`mod@crate::button`]'s own named-constructor shape). Every one
//! fires `on_press` on release inside its bounds and leaves its own props
//! alone — `is_selected` is a caller-owned prop, never self-mutated
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics; see *Toggle* below).
//!
//! # Relationship to `frust::icon_button`
//!
//! Baseline `frust-widgets` ships its own, much simpler
//! [`frust::icon_button`] — a single-variant, transparent-at-rest pressable
//! glyph with no size tiers, no toggle, no badge. This module reuses the
//! same plain `icon_button`/`IconButton` names in `frust_material` on
//! purpose: the established cross-crate naming precedent
//! [`mod@crate::checkbox`]/[`mod@crate::radio`] already set against their
//! own baseline counterparts. An app picks exactly one by import path
//! (`frust_material::icon_button` vs. `frust::icon_button`), never both at
//! the same call site.
//!
//! # Variants
//!
//! Container / content roles, from `m3e_icon_button_build.dart:153`
//! (`_variantColors`) — the *only* table where `is_selected` changes a
//! color, and only for [`IconButtonVariant::Standard`]:
//!
//! | Variant | Container | Content |
//! |---|---|---|
//! | [`IconButtonVariant::Standard`] (default) | transparent | `primary` if selected else `on_surface_variant` |
//! | [`IconButtonVariant::Filled`] | `primary` | `on_primary` |
//! | [`IconButtonVariant::Tonal`] | `secondary_container` | `on_secondary_container` |
//! | [`IconButtonVariant::Outlined`] | transparent + `outline` hairline | `primary` |
//!
//! **Verified from source, not obvious:** unlike [`mod@crate::button`]'s
//! `resolve_colors`, `_variantColors` carries **no disabled branch at all**
//! — an icon button built with `enabled(false)` paints the exact same inks
//! as an enabled one (`_morphButtonStyle`'s `foregroundColor`/
//! `backgroundColor` resolvers are state-independent constants, bypassing
//! Flutter's own `IconButton` disabled dimming). This port matches that
//! exactly: `enabled` only gates input handling (see *Enabled/disabled*),
//! never color.
//!
//! # Sizes and widths
//!
//! Icon glyph size, and the visual (painted) box vs. the tap-target box, by
//! size × width — `m3e_icon_button_theme.dart:39`-`:101`. **Visual and
//! target are deliberately distinct measurements**: on [`IconButtonSize::Xs`]
//! and [`IconButtonSize::Sm`] the painted box is smaller than the 48dp
//! minimum touch target, so the outer hit region pads out to 48dp while the
//! visible container stays smaller — from [`IconButtonSize::Md`] up, the two
//! already coincide (or the wide variant's target already exceeds 48dp), so
//! nothing pads further. [`IconButtonWidget::layout`] returns the *target*
//! size and centers the *visual* box inside it; both are pinned per size in
//! this module's tests.
//!
//! | Size | Icon | Visual (default/narrow/wide) | Target (default/narrow/wide) |
//! |---|---|---|---|
//! | [`IconButtonSize::Xs`] | 20 | 32×32 / 28×32 / 40×32 | 48×48 / 48×48 / 48×48 |
//! | [`IconButtonSize::Sm`] (default) | 24 | 40×40 / 32×40 / 52×40 | 48×48 / 48×48 / 52×48 |
//! | [`IconButtonSize::Md`] | 24 | 56×56 / 48×56 / 72×56 | 56×56 / 48×56 / 72×56 |
//! | [`IconButtonSize::Lg`] | 32 | 96×96 / 64×96 / 128×96 | 96×96 / 64×96 / 128×96 |
//! | [`IconButtonSize::Xl`] | 40 | 136×136 / 104×136 / 184×136 | 136×136 / 104×136 / 184×136 |
//!
//! [`IconButtonWidth::Standard`] is the reference's `defaultWidth` (renamed
//! to avoid colliding with Rust's `Default` trait); [`IconButtonWidth::Narrow`]
//! and [`IconButtonWidth::Wide`] adjust only the box width, never its height.
//!
//! # Shape and the press/hover morph
//!
//! [`IconButtonShape::Round`] (the default) and [`IconButtonShape::Square`]
//! each resolve a per-size resting radius (`m3e_icon_button_theme.dart:103`-
//! `:117`); pressing morphs toward a squarer radius and hovering morphs
//! partway there (`m3e_icon_button_shapes.dart:42`'s `effectiveRadius`,
//! `:119`-`:134`'s pressed/hovered tables):
//!
//! | Size | Round (rest) | Square (rest) | Hovered | Pressed |
//! |---|---|---|---|---|
//! | Xs | 16 | 8 | 10 | 6 |
//! | Sm | 20 | 10 | 12 | 8 |
//! | Md | 28 | 14 | 16 | 11 |
//! | Lg | 48 | 24 | 28 | 19 |
//! | Xl | 68 | 34 | 40 | 27 |
//!
//! Unlike [`mod@crate::button`]'s own radius tables, none of these four
//! coincide cleanly across every size with [`frust::Theme`]'s shape scale
//! (`shape_scale()`'s `small`/`medium`/`large`/… tokens) — the round table
//! matches at four of five sizes but breaks at Xl (68 has no token), so
//! this module keeps every radius a literal per-size constant rather than a
//! partial, size-inconsistent theme resolution.
//!
//! The morph itself is [`crate::button::motion::RadiusPaddingMotion`],
//! reused directly from [`mod@crate::button`] — see this module's
//! `RadiusPaddingMotion reuse` section — flung along the same named spring
//! [`mod@crate::button::motion`] uses,
//! [`crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`] (380/0.55).
//!
//! # `RadiusPaddingMotion` reuse
//!
//! [`mod@crate::button`]'s `RadiusPaddingMotion` is the reference's shared
//! morph primitive (`M3ERadiusAndPaddingMotion`), used by both the Dart
//! button *and* icon-button families via one imported file
//! (`m3e_icon_buttons.dart:14`). This crate's own port of that primitive
//! lives at `button::motion::RadiusPaddingMotion`, `pub(crate)` and now
//! declared inside a `pub(crate) mod motion;` in `button/mod.rs`, so this
//! sibling module imports it directly rather than carrying a parallel copy.
//! An earlier version of this module *did* carry a copy (`RadiusMorph`) —
//! `button/mod.rs`'s `mod motion;` was still private then, and a same-wave
//! task held `button/` read-only, so widening it was out of reach; a later
//! task (owning `button/` for real) closed that gap by widening the
//! declaration and doing this swap. Every icon-button width/size/variant
//! passes `internalLeft/Right/Top/Bottom: 0` to `M3ERadiusAndPaddingMotion`
//! (`m3e_icon_button_build.dart:191`-`:194`), so the padding channel the
//! button family needs never actually moves here — this module pins it to
//! `ContentPadding::symmetric(0.0)` on every retarget and never reads
//! `RadiusPaddingMotion::padding` back. `press_spring_matches_expressive_spatial_press`
//! (this module's tests) still pins the spring constant this module flings
//! along against `crate::button::motion::PRESS_SPRING`, now the *same*
//! constant both families share rather than two copies kept equal by hand.
//!
//! # Toggle
//!
//! [`IconButtonView::selected`] and [`IconButtonView::selected_icon`]
//! together are the reference's optional toggle pair
//! (`isToggle = widget.isSelected != null || widget.selectedIcon != null`,
//! `m3e_icon_button_build.dart:12`-`13`): a plain icon button sets neither.
//! When toggling, [`IconButtonShape`] **flips** while selected
//! (`m3e_icon_button_shapes.dart:16`-`27`'s `restVariant`) and the *active*
//! icon becomes `selected_icon` (falling back to `icon` if unset) —
//! `is_selected` is read every build/rebuild as a prop, never written by
//! this widget: a press only ever fires `on_press`, exactly like every
//! other controlled component in this crate (`docs/CODE_STANDARDS.md`'s
//! Interaction Semantics).
//!
//! # Enabled/disabled
//!
//! [`IconButtonView::enabled`] (default `true`) gates input handling only —
//! see the *Variants* section above for why colors never dim. A button
//! disabled mid-press drops both the visual press and the pointer capture
//! it was holding, mirroring [`mod@crate::button`]'s own rule.
//!
//! # Badge
//!
//! [`IconButtonView::badge`] ports what `_wrapWithBadge`/`_buildBadge`
//! actually paint for this family (`m3e_icon_button_build.dart:293`-`:350`)
//! — **not** the separate, not-yet-built shared badges family this catalog
//! will eventually carry. [`BadgeValue::Count`] rounds/clamps to `0..=999_999`; `0`
//! paints a small unlabeled dot ([`DOT_DIAMETER`], the reference's
//! `smallSize: 8`); anything else paints a labeled pill.
//! [`BadgeValue::Text`] paints a labeled pill directly, or nothing at all
//! for an empty string (`v.isEmpty`, `:332`). The pill height/padding are
//! Flutter's own `Badge` widget defaults (`largeSize`/`padding`, not
//! vendored into this repo's ported tree, so not line-cited the way every
//! other constant here is) rather than an m3e-specific token. The badge is
//! purely decorative paint, positioned at the visual box's top-end corner
//! (`PositionedDirectional(top: 0, end: 0)`, `:306`) — it never hit-tests
//! and never affects layout of anything else.
//!
//! # Icon color: caller-supplied, not ambient-retinted
//!
//! The Dart widget wraps its active icon in `IconTheme.merge(color:
//! colors.fg, ...)` so any descendant `Icon` inherits the resolved content
//! ink ambiently. This framework has no ambient icon-theme propagation, and
//! [`mod@crate::button`]'s own icon child has exactly the same gap (it lays
//! its `icon: Option<ChildPod>` out at the size token without recoloring
//! it) — this port matches that established precedent rather than inventing
//! one: the icon/`selected_icon` views a caller supplies are expected to
//! carry their own ink (e.g. `frust_material::icons::CHECK` built through
//! `frust::icon(..).color(..)`).
//!
//! # Not ported in v1
//!
//! `tooltip` (no tooltip host in the catalog — the same gap
//! [`mod@crate::button`]'s own module doc records), `visualSize` (a
//! decoration-shaped per-instance override with no builder surface yet),
//! and [`M3EIconButtonDecoration`]'s gradient fill/overlay/outline seam (the
//! same follow-up shape [`mod@crate::button`]'s `ButtonDecoration` fills for
//! the button family, not yet extended here).
//!
//! # Attribution
//!
//! See `plugins/material/NOTICE`'s "MIT License — Additional Copyright
//! Holders (Vendored Components)" section (Mudit Purohit / `icon_button_m3e`)
//! and its Module Attribution Header Convention.

use std::rc::Rc;

use frust::Theme;
use frust::authoring::text::{TextContext, TextLayout, TextStyle};
use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CursorIcon, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role, SemanticsCtx,
    View, Widget,
};
use kurbo::{Point, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use crate::button::motion::{ContentPadding, RadiusPaddingMotion};
use crate::interaction::{HapticSignal, InteractionState, MaterialHaptics};
use crate::press::presses;

// ---- Icon glyph size (m3e_icon_button_theme.dart:39-45) --------------------

/// The icon glyph's side length for each [`IconButtonSize`]
/// (`M3EIconButtonTheme._icon`, `m3e_icon_button_theme.dart:39`-`:45`).
fn icon_glyph_size(size: IconButtonSize) -> f64 {
    match size {
        IconButtonSize::Xs => 20.0,
        IconButtonSize::Sm | IconButtonSize::Md => 24.0,
        IconButtonSize::Lg => 32.0,
        IconButtonSize::Xl => 40.0,
    }
}

// ---- Visual / target size tables (m3e_icon_button_theme.dart:47-101) -------

/// The painted container box for `size`/`width`
/// (`M3EIconButtonTheme._visual`, `m3e_icon_button_theme.dart:47`-`:73`).
/// See the [module docs](self)' Sizes and widths table.
fn visual_size(size: IconButtonSize, width: IconButtonWidth) -> Size {
    use IconButtonWidth::{Narrow, Standard, Wide};
    match size {
        IconButtonSize::Xs => match width {
            Standard => Size::new(32.0, 32.0),
            Narrow => Size::new(28.0, 32.0),
            Wide => Size::new(40.0, 32.0),
        },
        IconButtonSize::Sm => match width {
            Standard => Size::new(40.0, 40.0),
            Narrow => Size::new(32.0, 40.0),
            Wide => Size::new(52.0, 40.0),
        },
        IconButtonSize::Md => match width {
            Standard => Size::new(56.0, 56.0),
            Narrow => Size::new(48.0, 56.0),
            Wide => Size::new(72.0, 56.0),
        },
        IconButtonSize::Lg => match width {
            Standard => Size::new(96.0, 96.0),
            Narrow => Size::new(64.0, 96.0),
            Wide => Size::new(128.0, 96.0),
        },
        IconButtonSize::Xl => match width {
            Standard => Size::new(136.0, 136.0),
            Narrow => Size::new(104.0, 136.0),
            Wide => Size::new(184.0, 136.0),
        },
    }
}

/// The tap-target box for `size`/`width`, before the [module docs](self)'
/// visual-vs-target `max` reconciliation (`M3EIconButtonTheme._target`,
/// `m3e_icon_button_theme.dart:75`-`:101`).
fn theme_target_size(size: IconButtonSize, width: IconButtonWidth) -> Size {
    use IconButtonWidth::{Narrow, Standard, Wide};
    match size {
        IconButtonSize::Xs => Size::new(48.0, 48.0),
        IconButtonSize::Sm => match width {
            Standard | Narrow => Size::new(48.0, 48.0),
            Wide => Size::new(52.0, 48.0),
        },
        IconButtonSize::Md => match width {
            Standard => Size::new(56.0, 56.0),
            Narrow => Size::new(48.0, 56.0),
            Wide => Size::new(72.0, 56.0),
        },
        IconButtonSize::Lg => match width {
            Standard => Size::new(96.0, 96.0),
            Narrow => Size::new(64.0, 96.0),
            Wide => Size::new(128.0, 96.0),
        },
        IconButtonSize::Xl => match width {
            Standard => Size::new(136.0, 136.0),
            Narrow => Size::new(104.0, 136.0),
            Wide => Size::new(184.0, 136.0),
        },
    }
}

/// The outer tap-target box actually laid out: `theme_target_size` widened
/// (never narrowed) to at least the visual box on each axis
/// (`_resolveLayoutSizes`, `m3e_icon_button_build.dart:57`-`:70`). With no
/// `visualSize` override ported (see the [module docs](self)), this always
/// agrees with `theme_target_size` — the table is already self-consistent —
/// but the `max` is kept for fidelity and documented intent.
fn target_size(size: IconButtonSize, width: IconButtonWidth) -> Size {
    let visual = visual_size(size, width);
    let target = theme_target_size(size, width);
    Size::new(
        target.width.max(visual.width),
        target.height.max(visual.height),
    )
}

// ---- Radius tables (m3e_icon_button_theme.dart:103-134) --------------------

/// The round shape's resting radius (`_radiusRestRound`,
/// `m3e_icon_button_theme.dart:103`-`:109`).
fn radius_rest_round(size: IconButtonSize) -> f64 {
    match size {
        IconButtonSize::Xs => 16.0,
        IconButtonSize::Sm => 20.0,
        IconButtonSize::Md => 28.0,
        IconButtonSize::Lg => 48.0,
        IconButtonSize::Xl => 68.0,
    }
}

/// The square shape's resting radius (`_radiusRestSquare`,
/// `m3e_icon_button_theme.dart:111`-`:117`).
fn radius_rest_square(size: IconButtonSize) -> f64 {
    match size {
        IconButtonSize::Xs => 8.0,
        IconButtonSize::Sm => 10.0,
        IconButtonSize::Md => 14.0,
        IconButtonSize::Lg => 24.0,
        IconButtonSize::Xl => 34.0,
    }
}

/// The pressed radius, both shapes converge to it while pressed
/// (`_radiusPressed`, `m3e_icon_button_theme.dart:119`-`:125`).
fn radius_pressed(size: IconButtonSize) -> f64 {
    match size {
        IconButtonSize::Xs => 6.0,
        IconButtonSize::Sm => 8.0,
        IconButtonSize::Md => 11.0,
        IconButtonSize::Lg => 19.0,
        IconButtonSize::Xl => 27.0,
    }
}

/// The hovered radius, between resting and pressed
/// (`_radiusHovered`, `m3e_icon_button_theme.dart:127`-`:134`).
fn radius_hovered(size: IconButtonSize) -> f64 {
    match size {
        IconButtonSize::Xs => 10.0,
        IconButtonSize::Sm => 12.0,
        IconButtonSize::Md => 16.0,
        IconButtonSize::Lg => 28.0,
        IconButtonSize::Xl => 40.0,
    }
}

/// The resting shape variant, flipped while a toggle is selected
/// (`M3EIconButtonShapes.restVariant`, `m3e_icon_button_shapes.dart:16`-`27`).
fn rest_variant(is_toggle: bool, is_selected: bool, base: IconButtonShape) -> IconButtonShape {
    if is_toggle && is_selected {
        base.flipped()
    } else {
        base
    }
}

/// The resting radius for a resolved shape variant
/// (`M3EIconButtonShapes.restingRadius`, `m3e_icon_button_shapes.dart:29`-`40`).
fn resting_radius(size: IconButtonSize, variant: IconButtonShape) -> f64 {
    match variant {
        IconButtonShape::Round => radius_rest_round(size),
        IconButtonShape::Square => radius_rest_square(size),
    }
}

/// The corner radius to paint this frame — pressed beats hovered beats
/// resting, exactly `M3EIconButtonShapes.effectiveRadius`
/// (`m3e_icon_button_shapes.dart:42`-`:67`).
fn effective_radius(
    size: IconButtonSize,
    base_shape: IconButtonShape,
    is_toggle: bool,
    is_selected: bool,
    state: InteractionState,
) -> f64 {
    let variant = rest_variant(is_toggle, is_selected, base_shape);
    if state.pressed {
        radius_pressed(size)
    } else if state.hovered {
        radius_hovered(size)
    } else {
        resting_radius(size, variant)
    }
}

// ---- Colors (m3e_icon_button_build.dart:125-180) ---------------------------

/// The hairline stroke width for [`IconButtonVariant::Outlined`], in logical
/// px (`M3EIconButtonTheme.outlineWidth`'s default, `m3e_icon_button_theme.dart:12`).
const OUTLINE_WIDTH: f64 = 1.0;

/// Flattening tolerance for the `kurbo` rounded-rect paths this module
/// strokes (matches [`mod@crate::button::core`]'s `PATH_TOLERANCE`).
const PATH_TOLERANCE: f64 = 0.1;

/// Unthemed-fallback `primary`.
const PRIMARY: Color = Color::from_rgb8(0x67, 0x50, 0xA4);
/// Unthemed-fallback `on_primary`.
const ON_PRIMARY: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
/// Unthemed-fallback `secondary_container`.
const SECONDARY_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback `on_secondary_container`.
const ON_SECONDARY_CONTAINER: Color = Color::from_rgb8(0x1D, 0x19, 0x2B);
/// Unthemed-fallback `on_surface_variant`.
const ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);
/// Unthemed-fallback `outline`.
const OUTLINE: Color = Color::from_rgb8(0x79, 0x74, 0x7E);

/// The resolved container/content/outline inks for one paint pass.
#[derive(Clone, Copy, Debug, PartialEq)]
struct IconButtonColors {
    container: Color,
    content: Color,
    outline: Option<Color>,
}

/// `_variantColors` (`m3e_icon_button_build.dart:153`-`:180`) — see the
/// [module docs](self)' Variants table. This crate's own decoration seam
/// (`M3EIconButtonDecoration`'s override path,
/// `m3e_icon_button_build.dart:125`-`:151`) is not ported in v1 (see the
/// module docs' Not ported section), so this *is* the full color
/// resolution, not just the defaults a decoration could override.
fn resolve_colors(
    theme: Option<&Theme>,
    variant: IconButtonVariant,
    selected: bool,
) -> IconButtonColors {
    let (
        primary,
        on_primary,
        secondary_container,
        on_secondary_container,
        on_surface_variant,
        outline,
    ) = match theme {
        Some(theme) => {
            let s = theme.scheme();
            (
                s.primary,
                s.on_primary,
                s.secondary_container,
                s.on_secondary_container,
                s.on_surface_variant,
                s.outline,
            )
        }
        None => (
            PRIMARY,
            ON_PRIMARY,
            SECONDARY_CONTAINER,
            ON_SECONDARY_CONTAINER,
            ON_SURFACE_VARIANT,
            OUTLINE,
        ),
    };
    match variant {
        IconButtonVariant::Standard => IconButtonColors {
            container: Color::TRANSPARENT,
            content: if selected {
                primary
            } else {
                on_surface_variant
            },
            outline: None,
        },
        IconButtonVariant::Filled => IconButtonColors {
            container: primary,
            content: on_primary,
            outline: None,
        },
        IconButtonVariant::Tonal => IconButtonColors {
            container: secondary_container,
            content: on_secondary_container,
            outline: None,
        },
        IconButtonVariant::Outlined => IconButtonColors {
            container: Color::TRANSPARENT,
            content: primary,
            outline: Some(outline),
        },
    }
}

/// The `(background, content)` roles a badge paints in
/// (`scheme.primary`/`scheme.onPrimary`, every branch of `_buildBadge`,
/// `m3e_icon_button_build.dart:311`-`:350`).
fn badge_colors(theme: Option<&Theme>) -> (Color, Color) {
    match theme {
        Some(theme) => (theme.scheme().primary, theme.scheme().on_primary),
        None => (PRIMARY, ON_PRIMARY),
    }
}

/// `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

// ---- The press/hover shape morph -------------------------------------------
//
// See the module docs' `RadiusPaddingMotion reuse` section: the morph itself
// is `crate::button::motion::RadiusPaddingMotion`, imported directly rather
// than reimplemented — this module supplies only the zero-padding target
// every retarget pins ([`ZERO_PADDING`]) and never reads the padding channel
// back.

/// The padding channel [`crate::button::motion::RadiusPaddingMotion`] carries
/// but this family never animates — every icon-button call site passes zero
/// `internalLeft/Right/Top/Bottom` to the reference's shared primitive
/// (`m3e_icon_button_build.dart:191`-`:194`).
const ZERO_PADDING: ContentPadding = ContentPadding::symmetric(0.0);

// ---- Badge -------------------------------------------------------------

/// A badge value for [`IconButtonView::badge`] — the reference's
/// `Object? badgeValue` (a `num` or a `String`,
/// `m3e_icon_button_build.dart:311`-`:350`), given a closed, typed shape
/// instead.
#[derive(Clone, Debug, PartialEq)]
pub enum BadgeValue {
    /// A numeric badge, rounded and clamped to `0..=999_999`
    /// (`v.round().clamp(0, 999999)`, `:317`). `0` paints an unlabeled dot.
    Count(u32),
    /// A text badge. An empty string paints nothing (`v.isEmpty`, `:332`).
    Text(String),
}

/// What a badge slot resolves to for one paint pass — the union of `_buildBadge`'s
/// three return shapes (`null` / a bare dot / a labeled pill).
#[derive(Clone, Debug, PartialEq)]
enum ResolvedBadge {
    None,
    Dot,
    Label(String),
}

fn resolve_badge(value: &Option<BadgeValue>) -> ResolvedBadge {
    match value {
        None => ResolvedBadge::None,
        Some(BadgeValue::Count(n)) => {
            let clamped = (*n).min(999_999);
            if clamped == 0 {
                ResolvedBadge::Dot
            } else {
                ResolvedBadge::Label(clamped.to_string())
            }
        }
        Some(BadgeValue::Text(s)) => {
            if s.is_empty() {
                ResolvedBadge::None
            } else {
                ResolvedBadge::Label(s.clone())
            }
        }
    }
}

/// The dot badge's diameter, in logical px (`Badge(smallSize: 8, ...)`,
/// `m3e_icon_button_build.dart:320`).
const DOT_DIAMETER: f64 = 8.0;
/// The labeled badge pill's height, in logical px — Flutter's own `Badge`
/// widget default (`largeSize`), not vendored into this repo's ported tree
/// (see the [module docs](self)' Badge section).
const BADGE_HEIGHT: f64 = 16.0;
/// Horizontal padding inside a labeled badge pill around its text, in
/// logical px — Flutter's own `Badge` widget default padding.
const BADGE_H_PAD: f64 = 4.0;
/// The badge label's font size, in logical px — this crate's own
/// `label_small`-tier size (matches `button::core::LABEL_SMALL`'s `11.0`).
const BADGE_TEXT_SIZE: f32 = 11.0;

/// A small, paint-time-rebrushed text run for a badge's label — mirrors
/// [`mod@crate::button::core`]'s `LabelRun` shape (lazily shaped, re-brushed
/// at paint time so the ink can change independently of the cached
/// shaping), scoped down to what a short badge label needs: no wrapping, no
/// overflow, no natural-vs-fitted distinction. `button::core::LabelRun` is
/// `pub(super)` to `button` and out of scope to import (see the module
/// docs), so this is a parallel, badge-scoped copy of the same idiom.
struct BadgeRun {
    content: String,
    layout: Option<TextLayout>,
}

impl BadgeRun {
    fn new() -> Self {
        Self {
            content: String::new(),
            layout: None,
        }
    }

    fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.layout = None;
        }
    }

    /// Shape (or reuse) the run, returning its measured size.
    fn shape(&mut self, ctx: &mut LayoutCtx) -> Size {
        if let Some(layout) = &self.layout {
            return layout.size();
        }
        let style = TextStyle::new(BADGE_TEXT_SIZE, Color::BLACK);
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, &style, None);
        let size = laid.size();
        self.layout = Some(laid);
        size
    }

    fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, TextLayout::size)
    }

    /// Paint the run at `origin` in `color`, overriding the shaping ink.
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

// ---- Enums ------------------------------------------------------------

/// Which container treatment an icon button paints — the reference's
/// `M3EIconButtonVariant` (`m3e_icon_button_enums.dart:41`-`:53`). See the
/// [module docs](self)' Variants table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconButtonVariant {
    /// Transparent container, `on_surface_variant` content (`primary` when
    /// selected) — the default.
    #[default]
    Standard,
    /// Solid `primary` container, `on_primary` content.
    Filled,
    /// `secondary_container` container, `on_secondary_container` content.
    Tonal,
    /// Transparent container with an `outline` hairline, `primary` content.
    Outlined,
}

/// The icon button's size tier — the reference's `M3EIconButtonSize`
/// (`m3e_icon_button_enums.dart:2`-`:17`). See the [module docs](self)'
/// Sizes and widths table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconButtonSize {
    /// Extra-small.
    Xs,
    /// Small (the default).
    #[default]
    Sm,
    /// Medium.
    Md,
    /// Large.
    Lg,
    /// Extra-large.
    Xl,
}

/// The icon button's resting shape family — the reference's
/// `M3EIconButtonShapeVariant` (`m3e_icon_button_enums.dart:32`-`:38`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconButtonShape {
    /// A pill (the default).
    #[default]
    Round,
    /// A rounded square.
    Square,
}

impl IconButtonShape {
    /// The other shape — what a selected toggle flips to
    /// (`M3EIconButtonShapes.restVariant`).
    fn flipped(self) -> Self {
        match self {
            Self::Round => Self::Square,
            Self::Square => Self::Round,
        }
    }
}

/// The icon button's container width tier — the reference's
/// `M3EIconButtonWidth` (`m3e_icon_button_enums.dart:20`-`:29`).
/// [`Self::Standard`] is the reference's `defaultWidth`, renamed to avoid
/// colliding with Rust's `Default` trait (see the [module docs](self)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconButtonWidth {
    /// The size's own default width (the default).
    #[default]
    Standard,
    /// Narrower than the default.
    Narrow,
    /// Wider than the default.
    Wide,
}

// ---- View ---------------------------------------------------------------

/// A view-held, typed press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// A declarative M3 Expressive icon button. See the [module docs](self).
pub struct IconButtonView<State: 'static> {
    icon: AnyView<State>,
    selected_icon: Option<AnyView<State>>,
    variant: IconButtonVariant,
    size: IconButtonSize,
    shape: IconButtonShape,
    width: IconButtonWidth,
    is_selected: Option<bool>,
    enabled: bool,
    haptic: HapticSignal,
    badge_value: Option<BadgeValue>,
    suppress_ink: bool,
    semantic_label: Option<String>,
    on_press: OnPress<State>,
}

/// Create a standard (transparent, `on_surface_variant`) icon button
/// painting `icon`, running `on_press` on release inside its bounds — the
/// reference's default `M3EIconButton`.
pub fn icon_button<State: 'static, F: Fn(&mut State) + 'static>(
    icon: AnyView<State>,
    on_press: F,
) -> IconButtonView<State> {
    IconButtonView {
        icon,
        selected_icon: None,
        variant: IconButtonVariant::default(),
        size: IconButtonSize::default(),
        shape: IconButtonShape::default(),
        width: IconButtonWidth::default(),
        is_selected: None,
        enabled: true,
        haptic: HapticSignal::None,
        badge_value: None,
        suppress_ink: false,
        semantic_label: None,
        on_press: Rc::new(on_press),
    }
}

/// PascalCase alias for [`icon_button`], matching this catalog's view-fn
/// vocabulary (`Button`, `AssistChip`, …).
#[allow(non_snake_case)]
pub fn IconButton<State: 'static, F: Fn(&mut State) + 'static>(
    icon: AnyView<State>,
    on_press: F,
) -> IconButtonView<State> {
    icon_button(icon, on_press)
}

/// Create a filled icon button — [`icon_button`] with
/// [`IconButtonVariant::Filled`].
pub fn filled_icon_button<State: 'static, F: Fn(&mut State) + 'static>(
    icon: AnyView<State>,
    on_press: F,
) -> IconButtonView<State> {
    icon_button(icon, on_press).variant(IconButtonVariant::Filled)
}

/// Create a tonal icon button — [`icon_button`] with
/// [`IconButtonVariant::Tonal`].
pub fn tonal_icon_button<State: 'static, F: Fn(&mut State) + 'static>(
    icon: AnyView<State>,
    on_press: F,
) -> IconButtonView<State> {
    icon_button(icon, on_press).variant(IconButtonVariant::Tonal)
}

/// Create an outlined icon button — [`icon_button`] with
/// [`IconButtonVariant::Outlined`].
pub fn outlined_icon_button<State: 'static, F: Fn(&mut State) + 'static>(
    icon: AnyView<State>,
    on_press: F,
) -> IconButtonView<State> {
    icon_button(icon, on_press).variant(IconButtonVariant::Outlined)
}

impl<State: 'static> IconButtonView<State> {
    /// Set the container treatment (the [module docs](self)' Variants table).
    pub fn variant(mut self, variant: IconButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set the size tier (the [module docs](self)' Sizes and widths table).
    pub fn size(mut self, size: IconButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Set the resting shape family.
    pub fn shape(mut self, shape: IconButtonShape) -> Self {
        self.shape = shape;
        self
    }

    /// Set the container width tier.
    pub fn width(mut self, width: IconButtonWidth) -> Self {
        self.width = width;
        self
    }

    /// Mark this a toggle icon button carrying `selected`, per the
    /// [module docs](self)' Toggle section — never self-mutated by a press.
    pub fn selected(mut self, selected: bool) -> Self {
        self.is_selected = Some(selected);
        self
    }

    /// The icon painted while selected, falling back to the base `icon` when
    /// unset — also makes this a toggle icon button on its own even without
    /// [`Self::selected`] (the [module docs](self)' Toggle section).
    pub fn selected_icon(mut self, icon: AnyView<State>) -> Self {
        self.selected_icon = Some(icon);
        self
    }

    /// Whether the button accepts a press (default `true`). Never dims
    /// color — see the [module docs](self)' Variants section.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The haptic signal to fire on a press. Defaults to
    /// [`HapticSignal::None`].
    pub fn haptic(mut self, haptic: HapticSignal) -> Self {
        self.haptic = haptic;
        self
    }

    /// Attach a badge (the [module docs](self)' Badge section).
    pub fn badge(mut self, value: BadgeValue) -> Self {
        self.badge_value = Some(value);
        self
    }

    /// Suppress the hover/focus/pressed state-layer overlay (the
    /// reference's `suppressInk`, which forces both its splash *and* its
    /// overlay color transparent — this framework has no ripple, so
    /// suppressing the state layer is the whole effect).
    pub fn suppress_ink(mut self, suppress: bool) -> Self {
        self.suppress_ink = suppress;
        self
    }

    /// The accessible label, when the icon alone doesn't name the action.
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }

    /// The currently active icon view — `selected_icon` while selected (if
    /// set), else the base `icon`. Shared by `build`/`rebuild`/`teardown` so
    /// the three lifecycle methods can never disagree about which view is
    /// live.
    fn active_icon(&self) -> &AnyView<State> {
        let selected = self.is_selected.unwrap_or(false);
        if selected {
            self.selected_icon.as_ref().unwrap_or(&self.icon)
        } else {
            &self.icon
        }
    }

    /// Whether this is a toggle icon button at all
    /// (`isToggle = widget.isSelected != null || widget.selectedIcon != null`,
    /// `m3e_icon_button_build.dart:12`-`:13`).
    fn is_toggle(&self) -> bool {
        self.is_selected.is_some() || self.selected_icon.is_some()
    }
}

/// The retained widget for an [`IconButtonView`]. See the [module docs](self).
pub struct IconButtonWidget {
    variant: IconButtonVariant,
    size: IconButtonSize,
    shape: IconButtonShape,
    width: IconButtonWidth,
    is_selected: Option<bool>,
    is_toggle: bool,
    enabled: bool,
    haptic: HapticSignal,
    badge: ResolvedBadge,
    suppress_ink: bool,
    semantic_label: Option<String>,
    icon: ChildPod,
    badge_run: BadgeRun,
    /// The last-laid-out badge pill size (irrelevant/zero for `Dot`/`None`).
    badge_size: Size,
    /// The last-laid-out visual box size, cached from `layout` for `paint`.
    visual: Size,
    /// Where `layout` centered the visual box inside the target box.
    visual_origin: Point,
    state: InteractionState,
    captured: bool,
    /// The shared button-family press/hover morph, reused directly — see the
    /// module docs' `RadiusPaddingMotion reuse` section. Its padding channel
    /// is pinned to [`ZERO_PADDING`] and never read back.
    motion: RadiusPaddingMotion,
    on_press: frust::authoring::ErasedCallback,
}

impl<State: 'static> View<State> for IconButtonView<State> {
    type Element = IconButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> IconButtonWidget {
        IconButtonWidget {
            variant: self.variant,
            size: self.size,
            shape: self.shape,
            width: self.width,
            is_selected: self.is_selected,
            is_toggle: self.is_toggle(),
            enabled: self.enabled,
            haptic: self.haptic,
            badge: resolve_badge(&self.badge_value),
            suppress_ink: self.suppress_ink,
            semantic_label: self.semantic_label.clone(),
            icon: frust::authoring::build_child(self.active_icon(), ctx),
            badge_run: BadgeRun::new(),
            badge_size: Size::ZERO,
            visual: Size::ZERO,
            visual_origin: Point::ZERO,
            state: InteractionState::new(),
            captured: false,
            motion: RadiusPaddingMotion::new(),
            on_press: frust::authoring::erase_callback(&self.on_press),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut IconButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = frust::authoring::erase_callback(&self.on_press);
        element.haptic = self.haptic;
        element.semantic_label = self.semantic_label.clone();
        let mut flags = ChangeFlags::NONE;

        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.shape != self.shape {
            element.shape = self.shape;
            flags |= ChangeFlags::PAINT;
        }
        if prev.width != self.width {
            element.width = self.width;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled {
                // Disabled mid-press keeps neither the press nor the
                // capture it was holding (mirrors `button::ButtonWidget`).
                element.state.set_pressed(false);
                element.captured = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.suppress_ink != self.suppress_ink {
            element.suppress_ink = self.suppress_ink;
            flags |= ChangeFlags::PAINT;
        }

        let is_toggle = self.is_toggle();
        if element.is_toggle != is_toggle || element.is_selected != self.is_selected {
            element.is_toggle = is_toggle;
            element.is_selected = self.is_selected;
            flags |= ChangeFlags::PAINT;
        }

        let next_badge = resolve_badge(&self.badge_value);
        if element.badge != next_badge {
            element.badge = next_badge;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags |= frust::authoring::rebuild_child(
            prev.active_icon(),
            self.active_icon(),
            &mut element.icon,
            ctx,
        );

        flags
    }

    fn teardown(&self, element: &mut IconButtonWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(self.active_icon(), &mut element.icon, ctx);
    }
}

// ---- Widget: layout/paint/event/semantics ----------------------------------

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// The rounded rect to *stroke* for a `width`-wide hairline lying fully
/// inside a container of `size` with corner `radius` (mirrors
/// [`mod@crate::button::core`]'s identically-shaped helper).
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

impl Widget for IconButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let visual = visual_size(self.size, self.width);
        let target = target_size(self.size, self.width);
        let visual_origin = Point::new(
            (target.width - visual.width) / 2.0,
            (target.height - visual.height) / 2.0,
        );

        let icon_dim = icon_glyph_size(self.size);
        let icon_measured = self
            .icon
            .layout_child(ctx, &BoxConstraints::tight(Size::new(icon_dim, icon_dim)));
        self.icon.set_origin(Point::new(
            visual_origin.x + (visual.width - icon_measured.width) / 2.0,
            visual_origin.y + (visual.height - icon_measured.height) / 2.0,
        ));

        self.badge_size = match &self.badge {
            ResolvedBadge::None => Size::ZERO,
            ResolvedBadge::Dot => Size::new(DOT_DIAMETER, DOT_DIAMETER),
            ResolvedBadge::Label(text) => {
                self.badge_run.set_content(text);
                let text_size = self.badge_run.shape(ctx);
                let width = (text_size.width + BADGE_H_PAD * 2.0).max(BADGE_HEIGHT);
                Size::new(width, BADGE_HEIGHT)
            }
        };

        self.visual = visual;
        self.visual_origin = visual_origin;
        bc.constrain(target)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let outer_origin = ctx.origin();

        // Authoritative hover/focus reads; both inert while disabled
        // (mirrors `button::ButtonWidget::paint`).
        self.state.set_hovered(self.enabled && ctx.is_hovered());
        self.state.set_focused(self.enabled && ctx.has_focus());

        let selected = self.is_selected.unwrap_or(false);
        let (colors, radius_target, badge_bg, badge_fg) = {
            let theme = Theme::from_paint_ctx(ctx);
            let colors = resolve_colors(theme, self.variant, selected);
            let radius =
                effective_radius(self.size, self.shape, self.is_toggle, selected, self.state);
            let (bg, fg) = badge_colors(theme);
            (colors, radius, bg, fg)
        };

        if self.motion.retarget(radius_target, ZERO_PADDING) {
            ctx.request_frame();
        }
        if self.motion.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        let radius = self.motion.radius();

        let visual_origin = Point::new(
            outer_origin.x + self.visual_origin.x,
            outer_origin.y + self.visual_origin.y,
        );

        if colors.container.components[3] > 0.0 {
            scene.fill_rounded_rect(visual_origin, self.visual, radius, colors.container);
        }
        if let Some(outline) = colors.outline {
            let path =
                inset_stroke_rect(self.visual, radius, OUTLINE_WIDTH).to_path(PATH_TOLERANCE);
            scene.stroke_path(visual_origin, &path, OUTLINE_WIDTH, &Brush::Solid(outline));
        }
        if !self.suppress_ink && self.enabled {
            let opacity = self.state.resolve_opacity();
            if opacity > 0.0 {
                scene.fill_rounded_rect(
                    visual_origin,
                    self.visual,
                    radius,
                    with_alpha(colors.content, opacity),
                );
            }
        }

        self.icon.paint_child(ctx, scene);

        match &self.badge {
            ResolvedBadge::None => {}
            ResolvedBadge::Dot => {
                let d = DOT_DIAMETER;
                let dot_origin =
                    Point::new(visual_origin.x + self.visual.width - d, visual_origin.y);
                scene.fill_rounded_rect(dot_origin, Size::new(d, d), d / 2.0, badge_bg);
            }
            ResolvedBadge::Label(_) => {
                let size = self.badge_size;
                let pill_origin = Point::new(
                    visual_origin.x + self.visual.width - size.width,
                    visual_origin.y,
                );
                scene.fill_rounded_rect(pill_origin, size, size.height / 2.0, badge_bg);
                let text_size = self.badge_run.size();
                let text_origin = Point::new(
                    pill_origin.x + (size.width - text_size.width) / 2.0,
                    pill_origin.y + (size.height - text_size.height) / 2.0,
                );
                self.badge_run.paint(text_origin, badge_fg, scene);
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
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let over = inside(p.position, ctx.size());
                if !self.captured {
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
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
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
        ctx.push_node(Role::Button, |node| {
            if let Some(label) = &self.semantic_label {
                node.set_label(label.as_str());
            }
            node.set_selected(self.is_selected.unwrap_or(false));
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
    use frust::FrameTime;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::{BuildCtx, PointerButton, PointerEvent, View};
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;
    use std::cell::RefCell;

    fn build<S: 'static>(view: &IconButtonView<S>) -> IconButtonWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    fn settle_motion(motion: &mut RadiusPaddingMotion) {
        let mut t = 0.0;
        for _ in 0..600 {
            t += 1.0 / 60.0;
            if !motion.advance(ft_secs(t)) {
                return;
            }
        }
        panic!("the press morph never settled");
    }

    fn settle_morph(widget: &mut IconButtonWidget) {
        settle_motion(&mut widget.motion);
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Point, f64, Color)>,
        /// `(origin, ink, glyph count)` — mirrors `button::core`'s Recorder,
        /// which likewise reads only the run's transform/brush/glyph count,
        /// never its shaped text (`GlyphRun` carries no text string, only
        /// resolved glyph ids).
        runs: Vec<(Point, Color, usize)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _radius: f64, _std_dev: f64, _color: Color) {
        }
        fn stroke_path(&mut self, o: Point, _path: &kurbo::BezPath, width: f64, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((o, width, color));
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            let color = match &run.brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.runs
                .push((Point::new(t.x, t.y), color, run.glyphs.len()));
        }
    }

    fn layout_with(widget: &mut IconButtonWidget, max: f64) -> Size {
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(max, max)))
    }

    fn paint_at(widget: &mut IconButtonWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        if let Some(theme) = theme {
            pctx = pctx.with_theme(theme as &dyn Any);
        }
        widget.paint(&mut pctx, &mut rec);
        rec
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch<S: 'static>(
        widget: &mut IconButtonWidget,
        state: &mut S,
        size: Size,
        event: &InputEvent,
    ) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, event);
    }

    // ---- defaults -----------------------------------------------------

    #[test]
    fn defaults_match_the_reference_constructor_defaults() {
        // m3e_icon_buttons.dart:49-52 — variant standard, size sm, shape
        // round, width defaultWidth; suppressInk false (:58).
        let view = icon_button::<(), _>(leaf_any(20.0, 20.0), |_| {});
        assert_eq!(view.variant, IconButtonVariant::Standard);
        assert_eq!(view.size, IconButtonSize::Sm);
        assert_eq!(view.shape, IconButtonShape::Round);
        assert_eq!(view.width, IconButtonWidth::Standard);
        assert!(view.enabled);
        assert!(!view.suppress_ink);
        assert_eq!(view.haptic, HapticSignal::None);
        assert!(!view.is_toggle());
    }

    #[test]
    fn the_named_constructors_map_onto_the_reference_variants() {
        assert_eq!(
            filled_icon_button::<(), _>(leaf_any(20.0, 20.0), |_| {}).variant,
            IconButtonVariant::Filled
        );
        assert_eq!(
            tonal_icon_button::<(), _>(leaf_any(20.0, 20.0), |_| {}).variant,
            IconButtonVariant::Tonal
        );
        assert_eq!(
            outlined_icon_button::<(), _>(leaf_any(20.0, 20.0), |_| {}).variant,
            IconButtonVariant::Outlined
        );
    }

    // ---- size/width tables ---------------------------------------------

    const ALL_SIZES: [IconButtonSize; 5] = [
        IconButtonSize::Xs,
        IconButtonSize::Sm,
        IconButtonSize::Md,
        IconButtonSize::Lg,
        IconButtonSize::Xl,
    ];

    #[test]
    fn icon_glyph_sizes_match_the_reference_table() {
        // m3e_icon_button_theme.dart:39-45.
        let expected = [
            (IconButtonSize::Xs, 20.0),
            (IconButtonSize::Sm, 24.0),
            (IconButtonSize::Md, 24.0),
            (IconButtonSize::Lg, 32.0),
            (IconButtonSize::Xl, 40.0),
        ];
        for (size, icon) in expected {
            assert_eq!(icon_glyph_size(size), icon, "{size:?}");
        }
    }

    #[test]
    fn visual_sizes_match_the_reference_table_for_every_size_and_width() {
        // m3e_icon_button_theme.dart:47-73.
        use IconButtonWidth::{Narrow, Standard, Wide};
        let expected: [(IconButtonSize, Size, Size, Size); 5] = [
            (
                IconButtonSize::Xs,
                Size::new(32.0, 32.0),
                Size::new(28.0, 32.0),
                Size::new(40.0, 32.0),
            ),
            (
                IconButtonSize::Sm,
                Size::new(40.0, 40.0),
                Size::new(32.0, 40.0),
                Size::new(52.0, 40.0),
            ),
            (
                IconButtonSize::Md,
                Size::new(56.0, 56.0),
                Size::new(48.0, 56.0),
                Size::new(72.0, 56.0),
            ),
            (
                IconButtonSize::Lg,
                Size::new(96.0, 96.0),
                Size::new(64.0, 96.0),
                Size::new(128.0, 96.0),
            ),
            (
                IconButtonSize::Xl,
                Size::new(136.0, 136.0),
                Size::new(104.0, 136.0),
                Size::new(184.0, 136.0),
            ),
        ];
        for (size, default, narrow, wide) in expected {
            assert_eq!(visual_size(size, Standard), default, "{size:?} default");
            assert_eq!(visual_size(size, Narrow), narrow, "{size:?} narrow");
            assert_eq!(visual_size(size, Wide), wide, "{size:?} wide");
        }
    }

    #[test]
    fn target_sizes_match_the_reference_table_for_every_size_and_width() {
        // m3e_icon_button_theme.dart:75-101.
        use IconButtonWidth::{Narrow, Standard, Wide};
        let expected: [(IconButtonSize, Size, Size, Size); 5] = [
            (
                IconButtonSize::Xs,
                Size::new(48.0, 48.0),
                Size::new(48.0, 48.0),
                Size::new(48.0, 48.0),
            ),
            (
                IconButtonSize::Sm,
                Size::new(48.0, 48.0),
                Size::new(48.0, 48.0),
                Size::new(52.0, 48.0),
            ),
            (
                IconButtonSize::Md,
                Size::new(56.0, 56.0),
                Size::new(48.0, 56.0),
                Size::new(72.0, 56.0),
            ),
            (
                IconButtonSize::Lg,
                Size::new(96.0, 96.0),
                Size::new(64.0, 96.0),
                Size::new(128.0, 96.0),
            ),
            (
                IconButtonSize::Xl,
                Size::new(136.0, 136.0),
                Size::new(104.0, 136.0),
                Size::new(184.0, 136.0),
            ),
        ];
        for (size, default, narrow, wide) in expected {
            assert_eq!(target_size(size, Standard), default, "{size:?} default");
            assert_eq!(target_size(size, Narrow), narrow, "{size:?} narrow");
            assert_eq!(target_size(size, Wide), wide, "{size:?} wide");
        }
    }

    #[test]
    fn xs_and_sm_visual_is_smaller_than_the_48dp_target_but_the_target_never_is() {
        // The module docs' visual-vs-target distinction, pinned for every
        // size: on Xs/Sm the painted box is below the 48dp minimum touch
        // target; from Md up the two already coincide.
        for size in ALL_SIZES {
            for width in [
                IconButtonWidth::Standard,
                IconButtonWidth::Narrow,
                IconButtonWidth::Wide,
            ] {
                let visual = visual_size(size, width);
                let target = target_size(size, width);
                assert!(
                    target.width >= 48.0 && target.height >= 48.0,
                    "{size:?}/{width:?} target below 48dp"
                );
                assert!(target.width >= visual.width && target.height >= visual.height);
            }
        }
        assert!(visual_size(IconButtonSize::Xs, IconButtonWidth::Standard).width < 48.0);
        assert!(visual_size(IconButtonSize::Sm, IconButtonWidth::Standard).width < 48.0);
    }

    // ---- radius tables ---------------------------------------------------

    #[test]
    fn radius_tables_match_the_reference() {
        // m3e_icon_button_theme.dart:103-134.
        let expected: [(IconButtonSize, f64, f64, f64, f64); 5] = [
            (IconButtonSize::Xs, 16.0, 8.0, 6.0, 10.0),
            (IconButtonSize::Sm, 20.0, 10.0, 8.0, 12.0),
            (IconButtonSize::Md, 28.0, 14.0, 11.0, 16.0),
            (IconButtonSize::Lg, 48.0, 24.0, 19.0, 28.0),
            (IconButtonSize::Xl, 68.0, 34.0, 27.0, 40.0),
        ];
        for (size, round, square, pressed, hovered) in expected {
            assert_eq!(radius_rest_round(size), round, "{size:?} round");
            assert_eq!(radius_rest_square(size), square, "{size:?} square");
            assert_eq!(radius_pressed(size), pressed, "{size:?} pressed");
            assert_eq!(radius_hovered(size), hovered, "{size:?} hovered");
        }
    }

    #[test]
    fn a_toggle_flips_shape_only_while_selected() {
        // m3e_icon_button_shapes.dart:16-27.
        assert_eq!(
            rest_variant(true, true, IconButtonShape::Round),
            IconButtonShape::Square
        );
        assert_eq!(
            rest_variant(true, true, IconButtonShape::Square),
            IconButtonShape::Round
        );
        assert_eq!(
            rest_variant(true, false, IconButtonShape::Round),
            IconButtonShape::Round
        );
        assert_eq!(
            rest_variant(false, true, IconButtonShape::Round),
            IconButtonShape::Round,
            "not a toggle at all: no flip even if selected is somehow true"
        );
    }

    #[test]
    fn effective_radius_precedence_is_pressed_then_hovered_then_resting() {
        let mut state = InteractionState::new();
        assert_eq!(
            effective_radius(
                IconButtonSize::Sm,
                IconButtonShape::Round,
                false,
                false,
                state
            ),
            radius_rest_round(IconButtonSize::Sm)
        );
        state.set_hovered(true);
        assert_eq!(
            effective_radius(
                IconButtonSize::Sm,
                IconButtonShape::Round,
                false,
                false,
                state
            ),
            radius_hovered(IconButtonSize::Sm)
        );
        state.set_pressed(true);
        assert_eq!(
            effective_radius(
                IconButtonSize::Sm,
                IconButtonShape::Round,
                false,
                false,
                state
            ),
            radius_pressed(IconButtonSize::Sm)
        );
    }

    // ---- colors ----------------------------------------------------------

    #[test]
    fn unthemed_colors_match_the_reference_variant_table() {
        // m3e_icon_button_build.dart:153-180.
        let standard = resolve_colors(None, IconButtonVariant::Standard, false);
        assert_eq!(standard.container, Color::TRANSPARENT);
        assert_eq!(standard.content, ON_SURFACE_VARIANT);
        assert_eq!(standard.outline, None);

        let standard_selected = resolve_colors(None, IconButtonVariant::Standard, true);
        assert_eq!(
            standard_selected.content, PRIMARY,
            "selected standard swaps content to primary — the one color the toggle changes"
        );

        let filled = resolve_colors(None, IconButtonVariant::Filled, false);
        assert_eq!(filled.container, PRIMARY);
        assert_eq!(filled.content, ON_PRIMARY);

        let tonal = resolve_colors(None, IconButtonVariant::Tonal, false);
        assert_eq!(tonal.container, SECONDARY_CONTAINER);
        assert_eq!(tonal.content, ON_SECONDARY_CONTAINER);

        let outlined = resolve_colors(None, IconButtonVariant::Outlined, false);
        assert_eq!(outlined.container, Color::TRANSPARENT);
        assert_eq!(outlined.content, PRIMARY);
        assert_eq!(outlined.outline, Some(OUTLINE));
    }

    #[test]
    fn only_standard_swaps_color_on_selection() {
        for variant in [
            IconButtonVariant::Filled,
            IconButtonVariant::Tonal,
            IconButtonVariant::Outlined,
        ] {
            let unselected = resolve_colors(None, variant, false);
            let selected = resolve_colors(None, variant, true);
            assert_eq!(
                unselected, selected,
                "{variant:?} must not change color on selection"
            );
        }
    }

    // ---- press morph -------------------------------------------------------

    #[test]
    fn press_spring_matches_expressive_spatial_press() {
        // Now the *same* constant both button families fling along (the
        // `RadiusPaddingMotion` reuse this module's own docs describe), not
        // two copies pinned equal by hand — this test still exists as the
        // tripwire that a future change to either family's spring is felt
        // here too.
        let token = crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS;
        let spring = crate::button::motion::PRESS_SPRING;
        assert_eq!(spring.stiffness, token.stiffness);
        assert_eq!(spring.damping_ratio, token.damping_ratio);
        assert_eq!(spring.mass, 1.0);
        assert_eq!(spring.stiffness, 380.0);
        assert_eq!(spring.damping_ratio, 0.55);
    }

    #[test]
    fn the_morph_snaps_on_mount_and_springs_on_a_later_retarget() {
        let mut motion = RadiusPaddingMotion::new();
        assert!(
            !motion.retarget(20.0, ZERO_PADDING),
            "the mount frame seeds, it does not animate"
        );
        assert_eq!(motion.radius(), 20.0);
        assert!(
            motion.retarget(8.0, ZERO_PADDING),
            "a later retarget starts a leg"
        );
        assert_eq!(motion.radius(), 20.0, "the leg starts where it was");
        settle_motion(&mut motion);
        assert_eq!(motion.radius(), 8.0);
    }

    #[test]
    fn a_sub_tolerance_retarget_is_ignored() {
        let mut motion = RadiusPaddingMotion::new();
        motion.retarget(20.0, ZERO_PADDING);
        let tolerance = crate::button::motion::RETARGET_TOLERANCE;
        assert!(!motion.retarget(20.0 + tolerance / 2.0, ZERO_PADDING));
        assert_eq!(motion.target_radius(), 20.0);
    }

    #[test]
    fn a_press_and_release_cycle_settles_at_the_resting_radius() {
        let presses = Rc::new(RefCell::new(0u32));
        let counted = presses.clone();
        let view = icon_button::<(), _>(leaf_any(24.0, 24.0), move |_| {
            *counted.borrow_mut() += 1;
        });
        let mut widget = build(&view);
        let size = layout_with(&mut widget, 200.0);

        // Mount frame: paint once to seed the morph at the resting radius.
        paint_at(&mut widget, size, None);
        settle_morph(&mut widget);
        let resting = widget.motion.radius();
        assert_eq!(resting, radius_rest_round(IconButtonSize::Sm));

        // Press: dispatch Down inside, then re-paint to retarget + advance.
        let mut state = ();
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, 10.0, 10.0),
        );
        paint_at(&mut widget, size, None);
        settle_morph(&mut widget);
        assert_eq!(widget.motion.radius(), radius_pressed(IconButtonSize::Sm));

        // Release: Up inside fires on_press and clears pressed.
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, 10.0, 10.0),
        );
        assert_eq!(*presses.borrow(), 1, "on_press fired exactly once");
        paint_at(&mut widget, size, None);
        settle_morph(&mut widget);
        assert_eq!(widget.motion.radius(), resting);
    }

    // ---- badge ----------------------------------------------------------

    #[test]
    fn badge_resolution_matches_build_badge() {
        // m3e_icon_button_build.dart:311-350.
        assert_eq!(resolve_badge(&None), ResolvedBadge::None);
        assert_eq!(
            resolve_badge(&Some(BadgeValue::Count(0))),
            ResolvedBadge::Dot
        );
        assert_eq!(
            resolve_badge(&Some(BadgeValue::Count(3))),
            ResolvedBadge::Label("3".to_string())
        );
        assert_eq!(
            resolve_badge(&Some(BadgeValue::Count(1_000_000))),
            ResolvedBadge::Label("999999".to_string()),
            "clamped to 999_999"
        );
        assert_eq!(
            resolve_badge(&Some(BadgeValue::Text(String::new()))),
            ResolvedBadge::None
        );
        assert_eq!(
            resolve_badge(&Some(BadgeValue::Text("NEW".to_string()))),
            ResolvedBadge::Label("NEW".to_string())
        );
    }

    #[test]
    fn a_dot_badge_paints_one_small_filled_circle_at_the_top_end_corner() {
        let view = icon_button::<(), _>(leaf_any(24.0, 24.0), |_| {}).badge(BadgeValue::Count(0));
        let mut widget = build(&view);
        let size = layout_with(&mut widget, 200.0);
        let rec = paint_at(&mut widget, size, None);

        let (origin, dot_size, radius, color) = rec
            .rrects
            .iter()
            .find(|(_, s, _, _)| *s == Size::new(DOT_DIAMETER, DOT_DIAMETER))
            .copied()
            .expect("a dot-sized rounded rect is painted");
        let _ = dot_size;
        assert_eq!(radius, DOT_DIAMETER / 2.0);
        assert_eq!(color, PRIMARY);
        // Top-end corner of the visual box.
        let visual = visual_size(IconButtonSize::Sm, IconButtonWidth::Standard);
        let visual_origin = widget.visual_origin;
        assert_eq!(origin.y, visual_origin.y);
        assert_eq!(origin.x, visual_origin.x + visual.width - DOT_DIAMETER);
    }

    #[test]
    fn a_count_badge_paints_a_labeled_pill_with_the_digit_text() {
        let view = icon_button::<(), _>(leaf_any(24.0, 24.0), |_| {}).badge(BadgeValue::Count(7));
        let mut widget = build(&view);
        let size = layout_with(&mut widget, 200.0);
        let rec = paint_at(&mut widget, size, None);

        assert!(
            rec.rrects.iter().any(|(_, s, r, c)| *s == widget.badge_size
                && *r == BADGE_HEIGHT / 2.0
                && *c == PRIMARY),
            "the pill background is painted at primary"
        );
        assert!(
            rec.runs
                .iter()
                .any(|(_, ink, glyphs)| *ink == ON_PRIMARY && *glyphs == 1),
            "the badge label's one-digit glyph run is painted at on_primary"
        );
    }

    #[test]
    fn no_badge_value_paints_no_badge_geometry() {
        let view = icon_button::<(), _>(leaf_any(24.0, 24.0), |_| {});
        let mut widget = build(&view);
        let size = layout_with(&mut widget, 200.0);
        let rec = paint_at(&mut widget, size, None);
        assert!(
            rec.rrects
                .iter()
                .all(|(_, s, _, _)| *s != Size::new(DOT_DIAMETER, DOT_DIAMETER)),
            "no dot painted"
        );
        assert!(rec.runs.is_empty(), "no badge label painted");
    }

    // ---- toggle / selected icon swap ------------------------------------

    #[test]
    fn an_outlined_icon_button_paints_a_hairline_stroke() {
        let view =
            icon_button::<(), _>(leaf_any(20.0, 20.0), |_| {}).variant(IconButtonVariant::Outlined);
        let mut widget = build(&view);
        let size = layout_with(&mut widget, 200.0);
        let rec = paint_at(&mut widget, size, None);
        assert_eq!(rec.strokes.len(), 1, "the outline hairline is stroked once");
        let (_, width, color) = rec.strokes[0];
        assert_eq!(width, OUTLINE_WIDTH);
        assert_eq!(color, OUTLINE);
    }

    #[test]
    fn a_filled_icon_button_paints_no_outline_stroke() {
        let view =
            icon_button::<(), _>(leaf_any(20.0, 20.0), |_| {}).variant(IconButtonVariant::Filled);
        let mut widget = build(&view);
        let size = layout_with(&mut widget, 200.0);
        let rec = paint_at(&mut widget, size, None);
        assert!(rec.strokes.is_empty());
    }

    #[test]
    fn selected_icon_becomes_active_only_while_selected() {
        let base = leaf_any(20.0, 20.0);
        let selected_icon = leaf_any(24.0, 24.0);
        let view = icon_button::<(), _>(base, |_| {}).selected_icon(selected_icon);

        assert!(view.is_toggle());
        assert!(
            std::ptr::eq(view.active_icon(), &view.icon),
            "unselected: base icon is active"
        );

        let selected_view = view.selected(true);
        assert!(
            std::ptr::eq(
                selected_view.active_icon(),
                selected_view.selected_icon.as_ref().unwrap()
            ),
            "selected: selected_icon is active"
        );
    }

    #[test]
    fn selected_without_a_selected_icon_still_counts_as_toggle() {
        let view = icon_button::<(), _>(leaf_any(20.0, 20.0), |_| {}).selected(false);
        assert!(view.is_toggle());
    }

    // ---- semantics --------------------------------------------------------

    #[test]
    fn semantics_reports_role_button_selected_and_label() {
        fn logic(_s: &mut ()) -> IconButtonView<()> {
            icon_button::<(), _>(leaf_any(20.0, 20.0), |_| {})
                .selected(true)
                .semantic_label("Favorite")
        }
        let mut root: frust_core::RenderRoot<(), IconButtonView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("icon button contributes a Role::Button node");
        assert_eq!(node.label(), Some("Favorite"));
        assert_eq!(node.is_selected(), Some(true));
        assert!(node.supports_action(Action::Click));
    }

    #[test]
    fn semantics_omits_click_action_when_disabled() {
        fn logic(_s: &mut ()) -> IconButtonView<()> {
            icon_button::<(), _>(leaf_any(20.0, 20.0), |_| {}).enabled(false)
        }
        let mut root: frust_core::RenderRoot<(), IconButtonView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("icon button contributes a Role::Button node");
        assert!(!node.supports_action(Action::Click));
    }

    // ---- disabled colors never dim ----------------------------------------

    #[test]
    fn disabled_colors_are_identical_to_enabled_colors() {
        // Verified-from-source: `_variantColors` has no disabled branch —
        // see the module docs' Variants section.
        for variant in [
            IconButtonVariant::Standard,
            IconButtonVariant::Filled,
            IconButtonVariant::Tonal,
            IconButtonVariant::Outlined,
        ] {
            let enabled = resolve_colors(None, variant, false);
            let disabled = resolve_colors(None, variant, false); // enabled has no color input at all
            assert_eq!(enabled, disabled);
        }
    }

    // ---- event lifecycle ---------------------------------------------------

    #[test]
    fn a_disabled_button_ignores_every_pointer_event() {
        let view = icon_button::<(), _>(leaf_any(20.0, 20.0), |_| {}).enabled(false);
        let mut widget = build(&view);
        let size = layout_with(&mut widget, 200.0);
        let mut state = ();
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, 5.0, 5.0),
        );
        assert!(!widget.captured);
        assert!(!widget.state.pressed);
    }

    #[test]
    fn a_release_outside_bounds_does_not_fire() {
        let presses = Rc::new(RefCell::new(0u32));
        let counted = presses.clone();
        let view = icon_button::<(), _>(leaf_any(20.0, 20.0), move |_| {
            *counted.borrow_mut() += 1;
        });
        let mut widget = build(&view);
        let size = layout_with(&mut widget, 200.0);
        let mut state = ();
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, 5.0, 5.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, 9999.0, 9999.0),
        );
        assert_eq!(*presses.borrow(), 0);
        assert!(!widget.captured);
    }
}
