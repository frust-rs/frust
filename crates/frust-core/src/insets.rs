//! Window insets: Flutter's `ViewportMetrics` inset model ported into the
//! framework core.
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
//! it carries only `f64` scalars, so `frust-core` names it directly with no
//! downstream-crate dependency and threads it by copy.
//!
//! # Coordinate space and origin-independence
//!
//! All values are **logical px** (the shell divides device px by the density
//! before crossing into core). The insets are **global** — measured against the
//! window, not any particular widget's origin — so containers need no per-child
//! adjustment: a `SafeArea` consumes them knowing it spans the window.
//!
//! # Consumption (Flutter's `MediaQuery.removePadding`)
//!
//! A widget that pads its subtree by the safe-area padding also *removes* what
//! it consumed from that subtree: it derives a reduced value with
//! [`WindowInsets::consuming`] and installs it for its children through
//! [`LayoutCtx::with_window_insets`](crate::widget::LayoutCtx::with_window_insets)
//! / [`PaintCtx::with_window_insets`](crate::widget::PaintCtx::with_window_insets).
//! Descendants then read zero padding on the consumed edges, so a self-insetting
//! widget nested inside a `SafeArea` does not inset a second time. Outside such
//! a scope the value is the single root-seeded one. A pod floated through the
//! overlay portal carries its owner's consumed view along in its
//! [`OverlayEntry`](crate::overlay::OverlayEntry), so the same guarantee — a
//! paint-time read agrees with the layout-time one — holds for floated content
//! too, even though the root paints it from a separate pass.
//!
//! [`CornerInsets`] are the exception: they are window-corner facts a
//! `SafeArea` neither pads by nor removes, and a widget that laid out around
//! them does not rewrite them for its subtree.

/// The footprint of a system window control at one corner, in logical pixels.
///
/// `width` is measured inward from the safe-area rectangle's vertical edge and
/// `height` inward from its horizontal edge. See [`CornerInsets`] for the full
/// contract.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CornerInset {
    /// Horizontal extent, in logical px.
    pub width: f64,
    /// Vertical extent, in logical px.
    pub height: f64,
}

impl CornerInset {
    /// No window control at this corner.
    pub const ZERO: CornerInset = CornerInset {
        width: 0.0,
        height: 0.0,
    };

    /// Construct from a width and a height, in logical px.
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// Window-control footprints at the four window corners, in logical pixels.
///
/// * Values are **logical px** (see the [module docs](self)).
/// * Corners are **physical**: frust has no RTL layout, so the shell resolves
///   direction. In an RTL locale the iPadOS window control lands top-RIGHT.
/// * Each value is the extent by which a system window control **protrudes
///   beyond the safe-area rectangle** at that corner: `width` is measured
///   inward from the safe area's vertical edge, `height` inward from its
///   horizontal edge. A widget whose content band starts at the safe-area top
///   therefore overlaps the corner iff `height > 0.0` -- whether it self-insets
///   the top (reads `padding().top`) or sits under a top-consuming `SafeArea`
///   (reads 0) -- because in both cases the control's bottom edge is
///   safe-area-top + `height`.
/// * Corners are **never consumed** and never part of
///   [`WindowInsets::padding`]; [`WindowInsets::consuming`] leaves them alone.
/// * Today only the iOS shell reports them (the iPadOS 26+ window control).
///   Android, desktop, web and iOS < 26 leave them zero.
/// * Accepted gap: a bar that does not consume the horizontal safe-area insets
///   under-shifts for a control on a notched edge (no platform draws one there).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CornerInsets {
    /// Top-left corner footprint.
    pub top_left: CornerInset,
    /// Top-right corner footprint.
    pub top_right: CornerInset,
    /// Bottom-left corner footprint.
    pub bottom_left: CornerInset,
    /// Bottom-right corner footprint.
    pub bottom_right: CornerInset,
}

impl CornerInsets {
    /// No window control at any corner (the default).
    pub const ZERO: CornerInsets = CornerInsets {
        top_left: CornerInset::ZERO,
        top_right: CornerInset::ZERO,
        bottom_left: CornerInset::ZERO,
        bottom_right: CornerInset::ZERO,
    };

    /// Construct from the four corner footprints.
    pub const fn new(
        top_left: CornerInset,
        top_right: CornerInset,
        bottom_left: CornerInset,
        bottom_right: CornerInset,
    ) -> Self {
        Self {
            top_left,
            top_right,
            bottom_left,
            bottom_right,
        }
    }
}

/// Per-edge inset amounts, in logical pixels.
///
/// The framework-core counterpart of `frust-widgets`' layout `EdgeInsets`
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
    /// Window-control corners, in logical px; see [`CornerInsets`].
    pub corner_insets: CornerInsets,
}

impl WindowInsets {
    /// Construct from the two transported per-edge sets; corners are
    /// [`CornerInsets::ZERO`] (see [`with_corner_insets`](Self::with_corner_insets)).
    pub fn new(view_padding: EdgeInsets, view_insets: EdgeInsets) -> Self {
        Self {
            view_padding,
            view_insets,
            corner_insets: CornerInsets::ZERO,
        }
    }

    /// These insets with the window-control `corner_insets` set. `padding()`
    /// is unaffected.
    #[must_use]
    pub fn with_corner_insets(mut self, corner_insets: CornerInsets) -> Self {
        self.corner_insets = corner_insets;
        self
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

    /// These insets with the safe-area [`padding`](WindowInsets::padding) on
    /// each enabled edge marked as consumed — the value a widget that has
    /// already padded by those edges hands to its subtree.
    ///
    /// Flutter parity: `MediaQuery.removePadding` as applied by `SafeArea`. For
    /// each enabled edge, `view_padding.<edge>` is reduced by
    /// `self.padding().<edge>` (saturating at zero); `view_insets` is left
    /// untouched. The invariants are:
    ///
    /// * `consuming(..).padding().<edge> == 0.0` on every enabled edge;
    /// * `view_insets` is unchanged, so the IME still reaches descendants for
    ///   keyboard avoidance;
    /// * disabled edges are unchanged in both sets;
    /// * `consuming(..).corner_insets == self.corner_insets` — corners are never
    ///   consumed;
    /// * the operation is idempotent — consuming an already-consumed edge is a
    ///   no-op, which is what makes nested `SafeArea`s consume only once.
    #[must_use]
    pub fn consuming(self, left: bool, top: bool, right: bool, bottom: bool) -> WindowInsets {
        let padding = self.padding();
        let consume = |enabled: bool, view_padding: f64, padding: f64| {
            if enabled {
                (view_padding - padding).max(0.0)
            } else {
                view_padding
            }
        };
        WindowInsets {
            view_padding: EdgeInsets {
                left: consume(left, self.view_padding.left, padding.left),
                top: consume(top, self.view_padding.top, padding.top),
                right: consume(right, self.view_padding.right, padding.right),
                bottom: consume(bottom, self.view_padding.bottom, padding.bottom),
            },
            view_insets: self.view_insets,
            corner_insets: self.corner_insets,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_corners() -> CornerInsets {
        CornerInsets::new(
            CornerInset::new(0.0, 0.0),
            CornerInset::new(72.0, 24.0),
            CornerInset::new(1.0, 2.0),
            CornerInset::new(3.0, 4.0),
        )
    }

    #[test]
    fn corner_insets_default_is_zero() {
        assert_eq!(CornerInsets::default(), CornerInsets::ZERO);
        assert_eq!(CornerInset::default(), CornerInset::ZERO);
        assert_eq!(CornerInset::ZERO, CornerInset::new(0.0, 0.0));
    }

    #[test]
    fn window_insets_new_has_zero_corners() {
        let w = WindowInsets::new(EdgeInsets::new(0.0, 24.0, 0.0, 0.0), EdgeInsets::ZERO);
        assert_eq!(w.corner_insets, CornerInsets::ZERO);
        assert_eq!(WindowInsets::default().corner_insets, CornerInsets::ZERO);
    }

    #[test]
    fn with_corner_insets_sets_corners_and_keeps_padding() {
        let base = WindowInsets::new(
            EdgeInsets::new(0.0, 24.0, 0.0, 34.0),
            EdgeInsets::new(0.0, 0.0, 0.0, 10.0),
        );
        let with = base.with_corner_insets(sample_corners());
        assert_eq!(with.corner_insets, sample_corners());
        assert_eq!(with.padding(), base.padding());
        assert_eq!(with.view_padding, base.view_padding);
        assert_eq!(with.view_insets, base.view_insets);
    }

    #[test]
    fn consuming_leaves_corner_insets_untouched() {
        let w = WindowInsets::new(EdgeInsets::new(5.0, 24.0, 6.0, 34.0), EdgeInsets::ZERO)
            .with_corner_insets(sample_corners());
        let c = w.consuming(true, true, true, true);
        assert_eq!(c.corner_insets, sample_corners());
        assert_eq!(c.padding(), EdgeInsets::ZERO);
    }

    #[test]
    fn edge_insets_zero_is_all_zero() {
        assert_eq!(EdgeInsets::ZERO, EdgeInsets::new(0.0, 0.0, 0.0, 0.0));
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

    #[test]
    fn consuming_all_edges_zeroes_padding_and_keeps_view_insets() {
        let insets = WindowInsets::new(
            EdgeInsets::new(10.0, 20.0, 30.0, 40.0),
            EdgeInsets::new(0.0, 0.0, 0.0, 15.0),
        );
        let consumed = insets.consuming(true, true, true, true);
        assert_eq!(consumed.padding(), EdgeInsets::ZERO);
        // Bottom: padding was 40 - 15 = 25, so view_padding drops to 15 (the
        // part the IME already covers); the other edges drop to 0.
        assert_eq!(consumed.view_padding, EdgeInsets::new(0.0, 0.0, 0.0, 15.0));
        assert_eq!(consumed.view_insets, insets.view_insets);
    }

    #[test]
    fn consuming_bottom_only_leaves_other_edges_visible() {
        let insets = WindowInsets::new(EdgeInsets::new(10.0, 20.0, 30.0, 40.0), EdgeInsets::ZERO);
        let consumed = insets.consuming(false, false, false, true);
        assert_eq!(consumed.padding(), EdgeInsets::new(10.0, 20.0, 30.0, 0.0));
        assert_eq!(
            consumed.view_padding,
            EdgeInsets::new(10.0, 20.0, 30.0, 0.0)
        );
        assert_eq!(consumed.view_insets, EdgeInsets::ZERO);
    }

    #[test]
    fn consuming_an_ime_covered_edge_changes_nothing() {
        // The IME (300) fully covers the 40px bottom system inset, so that
        // edge's padding is already 0: consuming it leaves view_padding intact.
        let insets = WindowInsets::new(
            EdgeInsets::new(0.0, 24.0, 0.0, 40.0),
            EdgeInsets::new(0.0, 0.0, 0.0, 300.0),
        );
        assert_eq!(insets.padding().bottom, 0.0);
        let consumed = insets.consuming(false, false, false, true);
        assert_eq!(consumed, insets);
        assert_eq!(consumed.padding().bottom, 0.0);
        assert_eq!(consumed.view_insets.bottom, 300.0);
    }

    #[test]
    fn consuming_is_idempotent() {
        let insets = WindowInsets::new(
            EdgeInsets::new(10.0, 20.0, 30.0, 40.0),
            EdgeInsets::new(5.0, 0.0, 0.0, 60.0),
        );
        for edges in [
            (true, true, true, true),
            (true, false, true, false),
            (false, true, false, true),
            (false, false, false, false),
        ] {
            let (l, t, r, b) = edges;
            let once = insets.consuming(l, t, r, b);
            assert_eq!(once.consuming(l, t, r, b), once, "edges {edges:?}");
        }
        assert_eq!(insets.consuming(false, false, false, false), insets);
    }
}
