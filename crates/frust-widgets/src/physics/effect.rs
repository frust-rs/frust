//! How overscroll displacement/pull is **visualized**, independent of the
//! [`ScrollPhysics`](super::ScrollPhysics) that computes it — a clamping
//! physics rejecting 100% of a boundary proposal still has an `edge_pull` to
//! paint an effect from (see the [module docs](super)'s design ruling), and a
//! bouncing physics letting the position itself move past the edge can be
//! paired with any of these the same way.
//!
//! [`OverscrollEffect::Stretch`]'s intensity curve is an affine approximation
//! of Android 12's overscroll shader — the same class Flutter's non-Impeller
//! `StretchingOverscrollIndicator` implements — but the curve's constants live
//! with the paint-time application that reads them, not here; this type is
//! only the selector.

/// How overscroll displacement/pull is VISUALIZED — orthogonal to physics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverscrollEffect {
    /// Translate the content by the overscroll displacement (today's behavior).
    #[default]
    Translate,
    /// M3E stretch: paint-side affine scale about the pulled edge; content
    /// origin stays fixed. Intensity reads the widget's edge_pull.
    Stretch,
    /// No visual at all.
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overscroll_effect_default_is_translate() {
        assert_eq!(OverscrollEffect::default(), OverscrollEffect::Translate);
    }
}
