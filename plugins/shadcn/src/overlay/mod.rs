//! The shared overlay hosting seam: the two patterns every shadcn panel that
//! leaves its parent's box is built on, plus the token accessors those panels
//! paint from.
//!
//! shadcn/ui portals every overlay to the document body and positions it there
//! (Radix `Portal` + `Popper` for the anchored family, `Portal` + a
//! `fixed inset-0` overlay for the modal family). frust has no portal: a widget
//! paints inside its parent's box. Both patterns below recover the same effect
//! with a **full-area top-layer widget** — one that fills whatever area it is
//! given and positions its own content inside it:
//!
//! 1. [`mod@anchored`] — the non-modal, trigger-relative host (popover, tooltip,
//!    hover-card, dropdown/context menu, select, combobox). It paints no scrim,
//!    positions its content against an anchor rect captured by the trigger, and
//!    light-dismisses on a press outside that content.
//! 2. [`mod@modal`] — the scrim + panel host (dialog, alert-dialog, sheet, drawer,
//!    command dialog). It paints shadcn's `bg-black/50` scrim over the whole
//!    area, centers or edge-pins its panel, and swallows every pointer event the
//!    panel's own content did not take.
//!
//! # Mounting a host: navigator page (primary) or `Stack` layer
//!
//! Either host is a plain widget, so an app can mount it two ways:
//!
//! - **As a transparent navigator page** — [`show_modal`] (and each component's
//!   `show_*` wrapper) pushes one through
//!   [`NavigatorController::push_transparent_for_result`](frust::NavigatorController::push_transparent_for_result),
//!   so the page below stays visible under the scrim, dismissal is
//!   `controller.pop()`, and a value travels back through the navigator's own
//!   pop-result machinery. This is the **primary, documented path** (the
//!   `frust_material::dialog` precedent; Flutter's dialogs-are-routes model),
//!   and the navigator routes input to the top page only, so everything below
//!   goes inert for pointers *and* for assistive technology for free.
//! - **As the top child of a full-area [`frust::Stack`]** — the host fills the
//!   stack, `Stack` hit-tests topmost-first, and the app owns the "is it open"
//!   flag itself (mount the host only while open). This is the supported route
//!   for an app with no navigator, and the only sensible one for a hover-driven
//!   overlay, whose lifetime is far shorter than a navigation.
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
//! The shadcn CSS variables these panels paint with resolve to frust
//! `ColorScheme` roles through [`crate::tokens`]' fold; the free functions below
//! are that fold's read side, each with the vendored `neutral` light table as
//! its unthemed fallback (the same table every other component in the catalog
//! falls back to).

use frust::{Color, Theme};

use crate::style;
use crate::tokens::{ShadcnBase, ShadcnPalette};

pub mod anchored;
pub mod modal;

pub use anchored::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayAnchorView,
    OverlayAnchorWidget, OverlayPlacement, OverlaySide, SIDE_OFFSET, anchor, anchored, place,
};
pub use modal::{
    HANDLE_RESERVE, ModalBorder, ModalConfig, ModalContent, ModalCorners, ModalEntrance,
    ModalExtent, ModalGeometry, ModalLimit, ModalRole, ModalView, ModalWidget, modal,
    panel_description, panel_title, show_modal, stack_slots, trailing_row,
};

/// The unthemed fallback token table: shadcn's own `neutral` base preset in
/// light mode — the same tables [`crate::theme`] folds, so a pass with no theme
/// threaded paints shadcn's default light values rather than a hand-copied hex
/// per accessor.
pub const FALLBACK: ShadcnPalette = ShadcnBase::Neutral.light();

/// `--background`: the app's base surface, and a dialog panel's own fill
/// (`bg-background`). Themed `surface`.
pub fn background(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.background, |t| t.scheme().surface)
}

/// `--foreground`: default ink. Themed `on_surface`.
pub fn foreground(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.foreground, |t| t.scheme().on_surface)
}

/// `--popover`: an anchored panel's surface (`bg-popover`). Themed
/// `surface_container_high`.
pub fn popover(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.popover, |t| t.scheme().surface_container_high)
}

/// `--muted`: the muted surface (a drawer's drag handle, `bg-muted`). Themed
/// `surface_container_highest`.
pub fn muted(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.muted, |t| t.scheme().surface_container_highest)
}

/// `--muted-foreground`: the dimmed ink role (`text-muted-foreground`). Themed
/// `on_surface_variant`.
pub fn muted_foreground(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.muted_foreground, |t| t.scheme().on_surface_variant)
}

/// `--border`: hairline rules and panel borders. Themed `outline`.
pub fn border(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.border, |t| t.scheme().outline)
}

/// `--accent`: the hover/selected wash (`bg-accent`). Themed
/// `primary_container`.
pub fn accent(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.accent, |t| t.scheme().primary_container)
}

/// `--accent-foreground`: ink on [`accent`]. Themed `on_primary_container`.
pub fn accent_foreground(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.accent_foreground, |t| {
        t.scheme().on_primary_container
    })
}

/// The modal scrim: shadcn's `bg-black/50`, which the shadcn `Theme` already
/// carries pre-alpha'd on the `scrim` role. Unthemed: black at the same 50%.
pub fn scrim(theme: Option<&Theme>) -> Color {
    theme.map_or(style::with_alpha(Color::BLACK, SCRIM_ALPHA), |t| {
        t.scheme().scrim
    })
}

/// Alpha of the modal scrim — shadcn's `bg-black/50`, shared verbatim by the
/// dialog, alert-dialog, sheet and drawer overlays. Restated here as the
/// *unthemed* fallback; a themed pass reads the pre-alpha'd `scrim` role
/// (`crate::tokens::theme`'s own `SCRIM_ALPHA`).
pub const SCRIM_ALPHA: f32 = 0.50;

/// Coerce a possibly-infinite constraint dimension to a finite value.
///
/// A host fills its area, so it expects bounded constraints — a navigator page
/// or a full-screen [`frust::Stack`] always gives it those; this is the guard
/// for the degenerate case (the same one `frust_material::dialog` applies).
///
/// # The scroll-view trap
///
/// The degenerate case has one common source: [`frust::scroll_view`] lays its
/// child out with an *infinite* max on the scroll axis. A host mounted anywhere
/// inside one — directly, or under a navigator/`Stack` that passes its own
/// constraints through — therefore reads a zero-length area on that axis and
/// collapses: a modal panel shrinks to a hairline while its barrier still
/// swallows every pointer press, and an anchored panel or tooltip clamps to the
/// area's origin far from its trigger. Each host's test module pins that
/// outcome so the shape stays visible.
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
    fn every_accessor_folds_its_css_variable_onto_the_theme_role() {
        let t = crate::theme().with_brightness(Brightness::Light);
        let s = t.scheme();
        assert_eq!(background(Some(&t)), s.surface);
        assert_eq!(foreground(Some(&t)), s.on_surface);
        assert_eq!(popover(Some(&t)), s.surface_container_high);
        assert_eq!(muted(Some(&t)), s.surface_container_highest);
        assert_eq!(muted_foreground(Some(&t)), s.on_surface_variant);
        assert_eq!(border(Some(&t)), s.outline);
        assert_eq!(accent(Some(&t)), s.primary_container);
        assert_eq!(accent_foreground(Some(&t)), s.on_primary_container);
        assert_eq!(scrim(Some(&t)), s.scrim);
    }

    #[test]
    fn the_unthemed_fallbacks_are_the_neutral_light_table() {
        assert_eq!(background(None), FALLBACK.background);
        assert_eq!(popover(None), FALLBACK.popover);
        assert_eq!(border(None), FALLBACK.border);
        // `bg-black/50`, matching the themed `scrim` role exactly.
        let unthemed = scrim(None);
        assert_eq!(unthemed.components[..3], [0.0, 0.0, 0.0]);
        assert!((unthemed.components[3] - SCRIM_ALPHA).abs() < 1e-6);
        assert_eq!(unthemed, scrim(Some(&crate::theme())));
    }
}
