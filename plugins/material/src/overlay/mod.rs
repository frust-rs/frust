// Ported from `frust-shadcn`'s overlay seam (`plugins/shadcn/src/overlay/`
// `{mod,anchored,modal}.rs`), itself a port of shadcn/ui v4 rev
// `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9` (MIT, © shadcn).
// A **sibling port, never a dependency**: a design-system plugin may not depend
// on another plugin (`docs/PLUGINS_ARCHITECTURE.md`'s Design-System Plugins
// charter), so the proven geometry math and dismiss semantics are carried over
// verbatim while every token read is re-folded onto a Material role.
// Porting decisions per module: see each module's own header.

//! The shared overlay hosting seam: the two patterns every Material panel that
//! leaves its parent's box is built on, plus the token accessors those panels
//! paint from.
//!
//! A widget paints inside its parent's box, so an overlay needs a host that
//! stands in for the web's portal: a **full-area top-layer widget** that fills
//! whatever area it is given and positions its own content inside it.
//!
//! 1. [`mod@anchored`] — the non-modal, trigger-relative host (menu, dropdown,
//!    tooltip, rich tooltip, exposed-dropdown text field). It paints no scrim,
//!    positions its content against an anchor rect captured by the trigger, and
//!    light-dismisses on a press outside that content.
//! 2. [`mod@modal`] — the scrim + panel host (dialog, alert dialog, bottom
//!    sheet, side sheet, full-screen search, picker). It paints the M3 32%
//!    scrim over the whole area, centres or edge-pins its panel, and swallows
//!    every input the panel's own content did not take.
//!
//! # Mounting a host: navigator page (primary) or `Stack` layer
//!
//! Either host is a plain widget, so an app can mount it two ways:
//!
//! - **As a transparent navigator page** — [`show_overlay_modal`] pushes one
//!   through
//!   [`NavigatorController::push_with_options`](frust::NavigatorController::push_with_options),
//!   so the page below stays visible under the scrim, dismissal is
//!   `controller.pop()`, a value travels back through the navigator's own
//!   pop-result machinery, and an Android back press routes through the page's
//!   [`BackPolicy`](frust::BackPolicy) (see [`mod@modal`]'s back-dismiss
//!   section). This is the **primary, documented path** — the
//!   [`mod@crate::dialog`]/[`crate::sheet`] precedent, Flutter's
//!   dialogs-are-routes model — and the navigator routes input to the top page
//!   only, so everything below goes inert for pointers *and* for assistive
//!   technology for free.
//! - **As the top child of a full-area [`frust::Stack`]** — the host fills the
//!   stack, `Stack` hit-tests topmost-first, and the app owns the "is it open"
//!   flag itself. This is the supported route for an app with no navigator,
//!   and the only sensible one for a hover-driven overlay, whose lifetime is
//!   far shorter than a navigation.
//!
//! Both hosts assume they are that top layer: they fill their constraints and
//! treat their own box as the window the content is fitted into. That makes
//! **bounded constraints part of the mounting contract**: neither the navigator
//! nor the `Stack` may itself sit inside a [`frust::scroll_view`], whose child
//! constraints are unbounded on the scroll axis — see [`finite_or_zero`]'s
//! scroll-view trap for what collapses when one does.
//!
//! # Claim ordering
//!
//! Every host here **routes to its content first and claims nothing before
//! that** — the container-claims-after-routing rule
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics). A host's own chrome (the
//! modal's close button and drag handle) hit-tests and claims hover only after
//! the content pod has had the pass, so a hovered control inside the panel wins
//! the claim and the host still reads hovered through the path.
//!
//! # Token accessors
//!
//! The free functions below are this seam's read side of the Material token
//! fold: each resolves one `ColorScheme`/`ShapeScale` role, with the M3
//! baseline **light** value as its unthemed fallback — the same values
//! [`crate::baseline`] carries, so a pass with no theme threaded paints
//! Material's own defaults rather than a hand-picked hex per accessor. They
//! stay namespaced (`frust_material::overlay::scrim`) rather than flat
//! re-exported, the same carve-out [`crate::icons`] takes: they are token
//! plumbing, not catalog surface.

use frust::{Color, Theme};

pub mod anchored;
pub mod modal;

pub use anchored::{
    ANCHORED_ENTER_SCALE, AnchoredOverlayView, AnchoredOverlayWidget, OVERLAY_ANCHOR_GAP,
    OverlayAlign, OverlayAnchor, OverlayAnchorView, OverlayAnchorWidget, OverlayPlacement,
    OverlaySide, anchored_overlay, overlay_anchor, place_anchored,
};
pub use modal::{
    ModalDismiss, OVERLAY_DIALOG_MAX_WIDTH, OVERLAY_DIALOG_MIN_WIDTH, OVERLAY_EDGE_FRACTION,
    OVERLAY_FLING_VELOCITY, OVERLAY_HANDLE_RESERVE, OVERLAY_SHEET_MAX_HEIGHT_FRACTION,
    OVERLAY_SIDE_SHEET_MAX_WIDTH, OverlayBorder, OverlayCorners, OverlayEntrance, OverlayExtent,
    OverlayGeometry, OverlayLimit, OverlayModalConfig, OverlayModalContent, OverlayModalView,
    OverlayModalWidget, OverlayRole, overlay_modal, show_overlay_modal,
};

/// Scrim opacity behind a modal overlay — M3 spec: 32%, the value
/// [`mod@crate::dialog`] and [`crate::sheet`] already paint their own scrims at.
pub const OVERLAY_SCRIM_ALPHA: f32 = 0.32;

/// Unthemed-fallback scrim base color (`colors.scrim`), applied at
/// [`OVERLAY_SCRIM_ALPHA`].
const FALLBACK_SCRIM: Color = Color::from_rgb8(0x00, 0x00, 0x00);
/// Unthemed-fallback `surfaceContainerLow`.
const FALLBACK_CONTAINER_LOW: Color = Color::from_rgb8(0xF7, 0xF2, 0xFA);
/// Unthemed-fallback `surfaceContainer`.
const FALLBACK_CONTAINER: Color = Color::from_rgb8(0xF3, 0xED, 0xF7);
/// Unthemed-fallback `surfaceContainerHigh`.
const FALLBACK_CONTAINER_HIGH: Color = Color::from_rgb8(0xEC, 0xE6, 0xF0);
/// Unthemed-fallback `surfaceContainerHighest`.
const FALLBACK_CONTAINER_HIGHEST: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);
/// Unthemed-fallback `onSurface` — a panel's own ink (the close affordance).
const FALLBACK_ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed-fallback `onSurfaceVariant` — the M3 drag-handle color role.
const FALLBACK_ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);
/// Unthemed-fallback `outlineVariant` — a panel's hairline border.
const FALLBACK_OUTLINE_VARIANT: Color = Color::from_rgb8(0xCA, 0xC4, 0xD0);
/// Unthemed-fallback `shadow`, applied at the elevation table's own alpha.
const FALLBACK_SHADOW: Color = Color::from_rgb8(0x00, 0x00, 0x00);
/// Unthemed-fallback panel corner radius (`shape.extra_large`, 28dp).
const FALLBACK_RADIUS: f64 = 28.0;
/// Unthemed-fallback shadow alpha — the `elevation_level` table's own 0.3.
const FALLBACK_SHADOW_ALPHA: f32 = 0.3;

/// Which `surfaceContainer*` role a panel fills with.
///
/// M3 assigns a different container rung per overlay family; a component
/// picks its own rather than the seam guessing one for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayContainer {
    /// `surfaceContainerLow` — the M3 bottom-sheet / side-sheet container.
    Low,
    /// `surfaceContainer` — the M3 menu container.
    Standard,
    /// `surfaceContainerHigh` — the M3 dialog container (the default).
    #[default]
    High,
    /// `surfaceContainerHighest` — the deepest container rung.
    Highest,
}

/// A panel's elevation rung, resolved from [`frust::Theme::elevation`] at
/// paint (the [`mod@crate::card`] shadow-resolution precedent).
///
/// M3 puts a dialog at level 3 and a sheet at level 1; a component declares
/// its own rung rather than the seam guessing one for it. The shadow math
/// itself is `crate::tokens`' documented v1 mapping, not an M3-published
/// spec — see that module's `elevation_level`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayElevation {
    /// No shadow at all (a full-screen panel has no edge to lift off).
    #[default]
    None,
    /// Level 1 (1dp) — the M3 bottom-sheet / side-sheet rung.
    Level1,
    /// Level 2 (3dp) — the M3 menu rung.
    Level2,
    /// Level 3 (6dp) — the M3 dialog rung.
    Level3,
}

/// The shadow to paint for `level`, as `(blur_std_dev, y_offset, color)`;
/// `None` for [`OverlayElevation::None`].
pub fn shadow(theme: Option<&Theme>, level: OverlayElevation) -> Option<(f64, f64, Color)> {
    let dp = match level {
        OverlayElevation::None => return None,
        OverlayElevation::Level1 => 1.0,
        OverlayElevation::Level2 => 3.0,
        OverlayElevation::Level3 => 6.0,
    };
    Some(match theme {
        Some(theme) => {
            let table = &theme.elevation;
            let entry = match level {
                OverlayElevation::None => unreachable!("returned above"),
                OverlayElevation::Level1 => table.level1,
                OverlayElevation::Level2 => table.level2,
                OverlayElevation::Level3 => table.level3,
            };
            let spec = entry.shadow(theme.brightness);
            (
                spec.blur_std_dev,
                spec.y_offset,
                with_alpha(theme.scheme().shadow, spec.color_alpha),
            )
        }
        // The same `y_offset = dp / 2 + 1`, `blur = dp` mapping the themed
        // table is built from (`crate::tokens`' `elevation_level`).
        None => (
            dp,
            dp / 2.0 + 1.0,
            with_alpha(FALLBACK_SHADOW, FALLBACK_SHADOW_ALPHA),
        ),
    })
}

/// Return `color` with its alpha channel replaced by `alpha` (the same helper
/// [`mod@crate::dialog`] and [`crate::sheet`] each carry).
pub(crate) fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The modal scrim: `colors.scrim` at [`OVERLAY_SCRIM_ALPHA`].
pub fn scrim(theme: Option<&Theme>) -> Color {
    let base = theme.map_or(FALLBACK_SCRIM, |t| t.scheme().scrim);
    with_alpha(base, OVERLAY_SCRIM_ALPHA)
}

/// A panel's container fill, for the [`OverlayContainer`] rung it declared.
pub fn container(theme: Option<&Theme>, role: OverlayContainer) -> Color {
    match (theme, role) {
        (Some(t), OverlayContainer::Low) => t.scheme().surface_container_low,
        (Some(t), OverlayContainer::Standard) => t.scheme().surface_container,
        (Some(t), OverlayContainer::High) => t.scheme().surface_container_high,
        (Some(t), OverlayContainer::Highest) => t.scheme().surface_container_highest,
        (None, OverlayContainer::Low) => FALLBACK_CONTAINER_LOW,
        (None, OverlayContainer::Standard) => FALLBACK_CONTAINER,
        (None, OverlayContainer::High) => FALLBACK_CONTAINER_HIGH,
        (None, OverlayContainer::Highest) => FALLBACK_CONTAINER_HIGHEST,
    }
}

/// A panel's own ink (`colors.on_surface`) — the close affordance's color.
pub fn on_surface(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK_ON_SURFACE, |t| t.scheme().on_surface)
}

/// The M3 drag-handle color role (`colors.on_surface_variant`).
pub fn on_surface_variant(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK_ON_SURFACE_VARIANT, |t| {
        t.scheme().on_surface_variant
    })
}

/// A panel's hairline border (`colors.outline_variant`).
pub fn outline_variant(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK_OUTLINE_VARIANT, |t| t.scheme().outline_variant)
}

/// A panel's corner radius (`shape.extra_large`, the 28dp token both the M3
/// dialog and the M3 sheet round to).
pub fn radius(theme: Option<&Theme>) -> f64 {
    theme.map_or(FALLBACK_RADIUS, |t| t.shape.extra_large)
}

/// Whether this pass runs under `Theme.motion.reduce_motion` (unthemed: no).
pub(crate) fn reduce_motion(theme: Option<&Theme>) -> bool {
    theme.is_some_and(|t| t.motion.reduce_motion)
}

/// Coerce a possibly-infinite constraint dimension to a finite value.
///
/// A host fills its area, so it expects bounded constraints — a navigator page
/// or a full-screen [`frust::Stack`] always gives it those; this is the guard
/// for the degenerate case (the same one [`mod@crate::dialog`] and [`crate::sheet`]
/// each apply).
///
/// # The scroll-view trap
///
/// The degenerate case has one common source: [`frust::scroll_view`] lays its
/// child out with an *infinite* max on the scroll axis. A host mounted anywhere
/// inside one — directly, or under a navigator/`Stack` that passes its own
/// constraints through — therefore reads a zero-length area on that axis and
/// collapses: a modal panel shrinks to a hairline while its barrier still
/// swallows every pointer press, and an anchored panel clamps to the area's
/// origin far from its trigger. Each host's test module pins that outcome so
/// the shape stays visible.
///
/// The remedy is at the mount, not here: keep the host outside the scroll view
/// and scroll the page's own body inside it instead. This coercion cannot
/// invent an extent nobody offered, and picking an arbitrary one would place
/// panels against a box that does not exist.
pub(crate) fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Brightness;

    #[test]
    fn every_accessor_folds_onto_its_material_role() {
        let t = crate::baseline().with_brightness(Brightness::Light);
        let s = t.scheme();
        assert_eq!(scrim(Some(&t)), with_alpha(s.scrim, OVERLAY_SCRIM_ALPHA));
        assert_eq!(
            container(Some(&t), OverlayContainer::Low),
            s.surface_container_low
        );
        assert_eq!(
            container(Some(&t), OverlayContainer::Standard),
            s.surface_container
        );
        assert_eq!(
            container(Some(&t), OverlayContainer::High),
            s.surface_container_high
        );
        assert_eq!(
            container(Some(&t), OverlayContainer::Highest),
            s.surface_container_highest
        );
        assert_eq!(on_surface(Some(&t)), s.on_surface);
        assert_eq!(on_surface_variant(Some(&t)), s.on_surface_variant);
        assert_eq!(outline_variant(Some(&t)), s.outline_variant);
        assert_eq!(radius(Some(&t)), t.shape.extra_large);
    }

    #[test]
    fn the_accessors_follow_a_live_brightness_flip() {
        let light = crate::baseline().with_brightness(Brightness::Light);
        let dark = crate::baseline().with_brightness(Brightness::Dark);
        assert_ne!(
            container(Some(&light), OverlayContainer::High),
            container(Some(&dark), OverlayContainer::High)
        );
        assert_ne!(on_surface(Some(&light)), on_surface(Some(&dark)));
    }

    #[test]
    fn the_unthemed_fallbacks_are_the_m3_baseline_light_values() {
        let light = crate::baseline().with_brightness(Brightness::Light);
        let s = light.scheme();
        assert_eq!(
            container(None, OverlayContainer::Low),
            s.surface_container_low
        );
        assert_eq!(
            container(None, OverlayContainer::High),
            s.surface_container_high
        );
        assert_eq!(on_surface(None), s.on_surface);
        assert_eq!(on_surface_variant(None), s.on_surface_variant);
        assert_eq!(outline_variant(None), s.outline_variant);
        assert_eq!(radius(None), light.shape.extra_large);
        // The scrim is the one role carried pre-alpha'd, at the M3 32%.
        let unthemed = scrim(None);
        assert_eq!(unthemed.components[..3], [0.0, 0.0, 0.0]);
        assert!((unthemed.components[3] - OVERLAY_SCRIM_ALPHA).abs() < 1e-6);
        assert_eq!(unthemed, scrim(Some(&light)));
    }

    #[test]
    fn the_elevation_rungs_resolve_off_the_themes_own_table() {
        let t = crate::baseline();
        assert_eq!(shadow(Some(&t), OverlayElevation::None), None);
        let (blur, y, color) = shadow(Some(&t), OverlayElevation::Level3).expect("a level-3 rung");
        let spec = t.elevation.level3.shadow(t.brightness);
        assert_eq!(blur, spec.blur_std_dev);
        assert_eq!(y, spec.y_offset);
        assert_eq!(color, with_alpha(t.scheme().shadow, spec.color_alpha));
        // A dialog's rung casts a longer shadow than a sheet's.
        let (sheet_blur, _, _) =
            shadow(Some(&t), OverlayElevation::Level1).expect("a level-1 rung");
        assert!(sheet_blur < blur);
    }

    #[test]
    fn the_unthemed_elevation_fallback_matches_the_tables_own_mapping() {
        // `y_offset = dp / 2 + 1`, `blur = dp` — `crate::tokens`' mapping.
        let (blur, y, _) = shadow(None, OverlayElevation::Level3).expect("a level-3 rung");
        assert_eq!(blur, 6.0);
        assert_eq!(y, 4.0);
        assert_eq!(shadow(None, OverlayElevation::None), None);
    }

    #[test]
    fn a_non_material_theme_still_resolves_every_role() {
        // An app is free to thread a bare theme; nothing here may panic.
        let bare = frust::Theme::neutral();
        let _ = scrim(Some(&bare));
        let _ = container(Some(&bare), OverlayContainer::High);
        let _ = on_surface(Some(&bare));
        let _ = radius(Some(&bare));
        assert!(!reduce_motion(None));
    }

    #[test]
    fn finite_or_zero_collapses_an_unbounded_axis() {
        assert_eq!(finite_or_zero(400.0), 400.0);
        assert_eq!(finite_or_zero(f64::INFINITY), 0.0);
    }
}
