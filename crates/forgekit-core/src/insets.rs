//! Window insets: Flutter's `ViewportMetrics` inset model ported into the
//! framework core (spec device-parity #3, see
//! `workflow/plans/features/device-parity/research/RESEARCH.md`, "Insets /
//! SafeArea / SystemChrome").
//!
//! A [`WindowInsets`] value flows shell → [`RenderRoot`](crate::app::RenderRoot)
//! → layout/paint contexts, delivered exactly like the theme: the shell reads
//! the platform's per-edge occlusion (Android `WindowInsets`, iOS
//! `safeAreaInsets`/keyboard frame), converts device px to **logical** px at the
//! FFI boundary, and pushes a [`WindowInsets`] onto the render root. A widget
//! (v1: `SafeArea`) recovers the resolved [`WindowInsets::padding`] through
//! [`LayoutCtx::window_insets`](crate::widget::LayoutCtx::window_insets) /
//! [`PaintCtx::window_insets`](crate::widget::PaintCtx::window_insets).
//!
//! Unlike the theme this is a **concrete** core-owned type (not `Box<dyn Any>`):
//! it carries only `f64` scalars, so `forgekit-core` names it directly with no
//! downstream-crate dependency and threads it by copy.
//!
//! # Coordinate space and origin-independence
//!
//! All values are **logical px** (the shell divides device px by the density
//! before crossing into core). The insets are **global** — measured against the
//! window, not any particular widget's origin — so containers need no per-child
//! adjustment: a `SafeArea` consumes them knowing it spans the window. Nested
//! inset semantics (Flutter's `MediaQuery.removePadding`) are a documented
//! non-goal for v1.

/// Per-edge inset amounts, in logical pixels.
///
/// The framework-core counterpart of `forgekit-widgets`' layout `EdgeInsets`
/// (that one is a `Padding` container's spacing; this one is the platform
/// occlusion model — a different layer, so it is not reused). Every value is a
/// non-negative logical-px distance from the corresponding window edge.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeInsets {
    /// Inset from the left edge.
    pub left: f64,
    /// Inset from the top edge.
    pub top: f64,
    /// Inset from the right edge.
    pub right: f64,
    /// Inset from the bottom edge.
    pub bottom: f64,
}

impl EdgeInsets {
    /// The zero inset — no occlusion on any edge (the default, and the value a
    /// [`WindowInsets`] carries until a shell pushes a real one).
    pub const ZERO: EdgeInsets = EdgeInsets {
        left: 0.0,
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
    };

    /// Construct per-edge insets directly.
    pub fn new(left: f64, top: f64, right: f64, bottom: f64) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    /// Per-edge maximum of `self` and `other`.
    ///
    /// Mirrors Flutter's engine-side merge of `Type.systemBars()` with the
    /// display cutout (`FlutterView.java:751-793`): a shell that assembles its
    /// `view_padding` from several platform inset sources combines them per edge
    /// with this rather than summing.
    #[must_use]
    pub fn max(self, other: EdgeInsets) -> EdgeInsets {
        EdgeInsets {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    /// Per-edge saturating subtraction: `max(0.0, self.edge - other.edge)` for
    /// each edge, clamping a would-be-negative result to zero.
    ///
    /// This is the building block of [`WindowInsets::padding`] (Flutter's
    /// `padding = max(0.0, viewPadding - viewInsets)`,
    /// `media_query.dart:152-170`): where the IME (`view_insets`) fully covers a
    /// system-bar edge (`view_padding`), the derived safe-area padding for that
    /// edge collapses to zero rather than going negative.
    #[must_use]
    pub fn saturating_sub(self, other: EdgeInsets) -> EdgeInsets {
        EdgeInsets {
            left: (self.left - other.left).max(0.0),
            top: (self.top - other.top).max(0.0),
            right: (self.right - other.right).max(0.0),
            bottom: (self.bottom - other.bottom).max(0.0),
        }
    }
}

/// The window's inset state, mirroring Flutter's `ViewportMetrics`
/// (`media_query.dart`).
///
/// Carries the two per-edge sets a shell transports; the third (the derived
/// safe-area [`padding`](WindowInsets::padding)) is computed on demand, never
/// stored:
///
/// * [`view_padding`](WindowInsets::view_padding) — system-UI-occluded edges
///   (status/navigation bars, display cutout). Never includes the IME.
/// * [`view_insets`](WindowInsets::view_insets) — fully-obscured area, in
///   practice the on-screen keyboard (IME). The status bar is never part of
///   this on either platform.
///
/// All values are **logical px** (see the [module docs](self)). `Default` is
/// the all-zero state (no occlusion). `PartialEq` lets a shell compare the
/// freshly-read platform insets against the last-pushed value and skip a no-op
/// [`set_insets`](crate::app::RenderRoot::set_insets).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowInsets {
    /// System-UI-occluded edges (status/navigation bars, cutout), in logical px.
    pub view_padding: EdgeInsets,
    /// Fully-obscured edges (the IME/keyboard), in logical px.
    pub view_insets: EdgeInsets,
}

impl WindowInsets {
    /// Construct from the two transported per-edge sets.
    pub fn new(view_padding: EdgeInsets, view_insets: EdgeInsets) -> Self {
        Self {
            view_padding,
            view_insets,
        }
    }

    /// The derived safe-area padding: `max(0.0, view_padding - view_insets)`
    /// per edge (Flutter's formula, `media_query.dart:152-170`).
    ///
    /// This is what a `SafeArea` widget insets by — where the IME
    /// (`view_insets`) overlaps a system-bar edge (`view_padding`), that edge's
    /// safe-area padding clamps to zero (the IME already handles keyboard
    /// avoidance for that edge). Computed on demand; never transported.
    pub fn padding(&self) -> EdgeInsets {
        self.view_padding.saturating_sub(self.view_insets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_insets_zero_is_all_zero() {
        assert_eq!(
            EdgeInsets::ZERO,
            EdgeInsets::new(0.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(EdgeInsets::ZERO, EdgeInsets::default());
    }

    #[test]
    fn edge_insets_max_is_per_edge() {
        let a = EdgeInsets::new(10.0, 0.0, 5.0, 30.0);
        let b = EdgeInsets::new(0.0, 24.0, 8.0, 20.0);
        assert_eq!(a.max(b), EdgeInsets::new(10.0, 24.0, 8.0, 30.0));
    }

    #[test]
    fn edge_insets_saturating_sub_clamps_to_zero() {
        let padding = EdgeInsets::new(10.0, 24.0, 10.0, 34.0);
        // The IME covers the whole bottom (and then some) but no other edge.
        let ime = EdgeInsets::new(0.0, 0.0, 0.0, 300.0);
        assert_eq!(
            padding.saturating_sub(ime),
            EdgeInsets::new(10.0, 24.0, 10.0, 0.0),
            "bottom clamps to 0, others untouched"
        );
    }

    #[test]
    fn window_insets_padding_is_flutter_formula() {
        // A phone with a 24px status bar / 34px home indicator and a keyboard up.
        let insets = WindowInsets::new(
            EdgeInsets::new(0.0, 24.0, 0.0, 34.0),
            EdgeInsets::new(0.0, 0.0, 0.0, 340.0),
        );
        // Bottom safe-area padding collapses to 0 while the IME is up (its 340px
        // overlaps the 34px home-indicator inset); the top status bar is intact.
        assert_eq!(insets.padding(), EdgeInsets::new(0.0, 24.0, 0.0, 0.0));
    }

    #[test]
    fn window_insets_default_is_zero_padding() {
        assert_eq!(WindowInsets::default().padding(), EdgeInsets::ZERO);
    }
}
