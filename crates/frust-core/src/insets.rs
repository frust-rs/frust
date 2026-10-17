//! Window insets: Flutter's `ViewportMetrics` inset model in the framework core.
//!
//! The shell converts the platform's per-edge occlusion to logical px and pushes a
//! [`WindowInsets`] onto the render root; widgets read it through
//! [`LayoutCtx::window_insets`](crate::widget::LayoutCtx::window_insets). Values
//! are window-global, not relative to any widget's origin.
//!
//! A widget that pads by the safe area installs [`WindowInsets::consuming`] for its
//! subtree (Flutter's `MediaQuery.removePadding`), so nested self-insetting widgets
//! do not inset twice; an overlay entry carries its owner's consumed value along.
//! [`CornerInsets`] are never consumed.

/// The footprint of a system window control at one corner, in logical px; see
/// [`CornerInsets`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CornerInset {
    pub width: f64,
    pub height: f64,
}

impl CornerInset {
    /// No window control at this corner.
    pub const ZERO: CornerInset = CornerInset {
        width: 0.0,
        height: 0.0,
    };

    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// Window-control footprints at the four window corners, in logical px.
///
/// * Corners are physical (frust has no RTL layout; the shell resolves direction).
/// * Each value is how far a control protrudes past the safe-area rectangle:
///   `width` inward from its vertical edge, `height` from its horizontal edge, so
///   content starting at the safe-area top overlaps the corner iff `height > 0.0`.
/// * Never consumed or part of [`WindowInsets::padding`]; only iOS reports them (iPadOS 26+).
/// * Accepted gap: a bar not consuming the horizontal insets under-shifts for a
///   control on a notched edge (no platform draws one there).
/// * Accepted gap: content not spanning the window's left/right edges (a detail
///   pane, sheet or dialog) still gets window-wide values and would over-shift;
///   shipped bars expose `corner_shift(false)` as the opt-out.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CornerInsets {
    pub top_left: CornerInset,
    pub top_right: CornerInset,
    pub bottom_left: CornerInset,
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

/// Non-negative per-edge distances from the window edges, in logical px. The
/// platform occlusion model, distinct from `frust-widgets`' layout `EdgeInsets`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeInsets {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl EdgeInsets {
    /// No occlusion on any edge.
    pub const ZERO: EdgeInsets = EdgeInsets {
        left: 0.0,
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
    };

    pub fn new(left: f64, top: f64, right: f64, bottom: f64) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    /// Per-edge maximum: how a shell merges several platform inset sources, as
    /// Flutter's engine merges system bars with the display cutout.
    #[must_use]
    pub fn max(self, other: EdgeInsets) -> EdgeInsets {
        EdgeInsets {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    /// Per-edge `max(0.0, self - other)`.
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

/// The window's inset state, mirroring Flutter's `ViewportMetrics`; `Default` is
/// no occlusion.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowInsets {
    /// System-UI-occluded edges (status/navigation bars, cutout); never the IME.
    pub view_padding: EdgeInsets,
    /// Fully obscured edges: in practice the on-screen keyboard.
    pub view_insets: EdgeInsets,
    pub corner_insets: CornerInsets,
}

impl WindowInsets {
    /// From the two per-edge sets, with [`CornerInsets::ZERO`].
    pub fn new(view_padding: EdgeInsets, view_insets: EdgeInsets) -> Self {
        Self {
            view_padding,
            view_insets,
            corner_insets: CornerInsets::ZERO,
        }
    }

    #[must_use]
    pub fn with_corner_insets(mut self, corner_insets: CornerInsets) -> Self {
        self.corner_insets = corner_insets;
        self
    }

    /// The safe-area padding a `SafeArea` insets by: `view_padding - view_insets`
    /// per edge, saturating at zero (Flutter's formula). Never transported.
    pub fn padding(&self) -> EdgeInsets {
        self.view_padding.saturating_sub(self.view_insets)
    }

    /// These insets with [`padding`](Self::padding) consumed on each enabled edge:
    /// those edges then read zero padding, `view_insets` and corners are unchanged
    /// (the IME still reaches descendants), and repeating it is a no-op.
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
