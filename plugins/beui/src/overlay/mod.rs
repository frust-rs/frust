//! The shared overlay hosting seam: the two patterns every beUI panel that
//! leaves its parent's box is built on.
//!
//! Upstream portals every overlay to the document body — `createPortal` into a
//! `position: fixed` layer, positioned there against a measured trigger rect
//! ([`anchored`](mod@anchored)) or spread over the whole viewport behind a
//! backdrop ([`modal`](mod@modal)). frust has no portal: a widget paints inside
//! its parent's box. Both hosts below recover the same effect with a
//! **full-area top-layer widget** — one that fills whatever area it is given and
//! positions its own content inside it.
//!
//! 1. [`mod@anchored`] — the non-modal, trigger-relative host. It paints no
//!    scrim, places its content against an anchor rect the trigger captured, and
//!    light-dismisses on a press outside that content.
//! 2. [`mod@modal`] — the scrim + panel host. It paints the scrim over the whole
//!    area, centres or edge-pins its panel, swallows every event its content
//!    declined, and (when pushed as a navigator page) animates its exit before
//!    the pop that removes it.
//!
//! # Which component mounts through which host
//!
//! The catalog's overlay components each mount through one of these two hosts;
//! the table is what the seam is shaped for, and where a component's own
//! chrome stops and the host's begins.
//!
//! | Host | beUI components |
//! |------|-----------------|
//! | [`anchored`](mod@anchored) | [`components::tooltip`](crate::components::tooltip), [`components::popover`](crate::components::popover), [`components::context_menu`](crate::components::context_menu), and the dropdown panels of [`components::select`](crate::components::select) / [`components::combobox`](crate::components::combobox) / [`components::multi_select`](crate::components::multi_select) |
//! | [`modal`](mod@modal), centred | [`components::morphing_modal`](crate::components::morphing_modal), [`components::center_morph_modal`](crate::components::center_morph_modal), [`blocks::command_palette`](crate::blocks::command_palette) |
//! | [`modal`](mod@modal), edge-mounted | [`components::drawer`](crate::components::drawer) and [`components::animated_sidebar`](crate::components::animated_sidebar) (leading/trailing edge), [`components::bottom_sheet`](crate::components::bottom_sheet) (bottom edge) |
//! | neither, but shares the presence driver | [`components::animated_toast_stack`](crate::components::animated_toast_stack) and [`blocks::notification_stack`](crate::blocks::notification_stack) mount their own stack; [`blocks::dynamic_island`](crate::blocks::dynamic_island) morphs in place. All three still stage enter/exit with [`crate::motion::Presence`], which is why that driver lives in [`crate::motion`] rather than here. |
//!
//! # Mounting a host
//!
//! Either host is a plain widget, so an app mounts it two ways:
//!
//! - **As a transparent navigator page** — [`show_modal`] pushes one through
//!   [`NavigatorController::push_with_options`](frust::NavigatorController::push_with_options),
//!   so the page below stays visible under the scrim, a back press is routed by
//!   the page's own [`BackPolicy`](frust::BackPolicy), and a value travels back
//!   through the navigator's pop-result machinery. This is the primary path for
//!   the modal family.
//! - **As the top child of a full-area [`frust::Stack`]** — the host fills the
//!   stack, `Stack` hit-tests topmost-first, and the app owns the open flag
//!   itself. This is the route for an app with no navigator, and the only
//!   sensible one for a hover-driven overlay, whose lifetime is far shorter than
//!   a navigation.
//!
//! Both hosts assume they are that top layer: they fill their constraints and
//! treat their own box as the window their content is fitted into. **Bounded
//! constraints are part of the mounting contract** — see [`finite_or_zero`].
//!
//! # Kept-mounted, always
//!
//! Both hosts take an open flag rather than being mounted only while open, so
//! the exit ramp has a widget left to play on (`crate::motion::presence`'s
//! kept-mounted contract). A host unmounted the frame its flag clears simply
//! truncates the exit; nothing breaks, the panel just vanishes.
//!
//! # Claim ordering
//!
//! Every host here routes to its content first and claims nothing before that —
//! the container-claims-after-routing rule (`docs/CODE_STANDARDS.md`'s
//! Interaction Semantics). A host's own dismissal arms only after the content
//! pod has had the pass.

use frust::{Color, Theme};

use crate::style;

pub mod anchored;
pub mod modal;

pub use anchored::{
    ANCHORED_ENTER_SCALE, ANCHORED_EXIT, ANCHORED_EXIT_SCALE, AnchoredOverlayView,
    AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayAnchorView, OverlayAnchorWidget,
    OverlayPlacement, OverlaySide, SIDE_OFFSET, TOOLTIP_OFFSET, VIEWPORT_PADDING, anchor, anchored,
    place, transform_origin,
};
pub use modal::{
    DRAWER_WIDTH, DRAWER_WIDTH_FRACTION, ModalConfig, ModalContent, ModalEdge, ModalExtent,
    ModalLimit, ModalMount, ModalRole, ModalView, ModalWidget, PANEL_ENTER_LIFT, PANEL_ENTER_SCALE,
    PANEL_EXIT, PANEL_EXIT_SCALE, PANEL_MARGIN, PANEL_MAX_WIDTH, SCRIM_FADE, SHEET_HEIGHT_FRACTION,
    modal, show_modal,
};

/// Alpha of the modal scrim — beUI's `bg-black/40`.
///
/// Restated here as the **unthemed** fallback for [`scrim`]; a themed pass reads
/// the pre-alpha'd `scrim` role, which [`crate::tokens::theme`](mod@crate::tokens::theme) folds from the
/// same source value (`drawer.tsx`'s and `animated-sidebar.tsx`'s backdrops,
/// the only two full-screen overlays upstream ships).
pub const SCRIM_ALPHA: f32 = 0.40;

/// The scrim a [`modal`](mod@modal) host paints over the whole area.
///
/// Themed: the `scrim` role. Unthemed: black at [`SCRIM_ALPHA`], which is the
/// same value the role carries.
///
/// `morphing-modal.tsx` is the one upstream overlay that backs itself with a
/// *frosted* wash instead (`bg-background/5` under a 14px backdrop blur) rather
/// than this dimming one. That is panel chrome, not a host decision: a component
/// wanting it paints its own backdrop over a host built with
/// [`ModalConfig::scrim`] switched off.
pub fn scrim(theme: Option<&Theme>) -> Color {
    theme.map_or(style::with_alpha(Color::BLACK, SCRIM_ALPHA), |t| {
        t.scheme().scrim
    })
}

/// Coerce a possibly-infinite constraint dimension to a finite value.
///
/// A host fills its area, so it expects bounded constraints — a navigator page
/// or a full-screen [`frust::Stack`] always gives it those; this is the guard
/// for the degenerate case.
///
/// # The scroll-view trap
///
/// The degenerate case has one common source: [`frust::scroll_view`] lays its
/// child out with an *infinite* max on the scroll axis. A host mounted anywhere
/// inside one — directly, or under a navigator/`Stack` that passes its own
/// constraints through — therefore reads a zero-length area on that axis and
/// collapses: a modal panel shrinks to a hairline while its scrim still swallows
/// every pointer press, and an anchored panel clamps to the area's origin far
/// from its trigger. Each host's test module pins that outcome so the shape
/// stays visible.
///
/// The remedy is at the mount, not here: keep the host outside the scroll view
/// and scroll the page's own body inside it instead. This coercion cannot invent
/// an extent nobody offered, and picking an arbitrary one would place panels
/// against a box that does not exist.
pub(crate) fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Brightness;

    #[test]
    fn the_scrim_folds_onto_the_themes_own_role() {
        let t = crate::theme().with_brightness(Brightness::Light);
        assert_eq!(scrim(Some(&t)), t.scheme().scrim);
        let dark = crate::theme().with_brightness(Brightness::Dark);
        assert_eq!(scrim(Some(&dark)), dark.scheme().scrim);
    }

    #[test]
    fn the_unthemed_scrim_is_the_same_black_wash_the_role_carries() {
        let unthemed = scrim(None);
        assert_eq!(unthemed.components[..3], [0.0, 0.0, 0.0]);
        assert!((unthemed.components[3] - SCRIM_ALPHA).abs() < 1e-6);
        assert_eq!(unthemed, scrim(Some(&crate::theme())));
    }

    #[test]
    fn an_infinite_constraint_coerces_to_zero_and_a_finite_one_is_kept() {
        assert_eq!(finite_or_zero(f64::INFINITY), 0.0);
        assert_eq!(finite_or_zero(f64::NEG_INFINITY), 0.0);
        assert_eq!(finite_or_zero(f64::NAN), 0.0);
        assert_eq!(finite_or_zero(600.0), 600.0);
    }
}
