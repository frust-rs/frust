//! Box-constraint layout model.
//!
//! Constraints flow *down* the widget tree; sizes flow *up*. This is the
//! Flutter/Masonry box model — not full CSS. A parent hands each child a
//! [`BoxConstraints`] describing the allowed `min`/`max` size, and the child
//! returns the concrete [`Size`] it chose within those bounds.

use kurbo::Size;

/// Clamp both dimensions of a size to be non-negative.
fn non_negative(size: Size) -> Size {
    Size::new(size.width.max(0.0), size.height.max(0.0))
}

/// Immutable min/max size bounds passed down during layout.
///
/// Invariant: both `min` and `max` are non-negative and `min <= max`
/// component-wise. This is enforced unconditionally by every constructor —
/// there is no way to construct a `BoxConstraints` that violates it, so
/// [`constrain`](Self::constrain) can never panic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxConstraints {
    min: Size,
    max: Size,
}

impl BoxConstraints {
    /// Construct constraints from explicit `min`/`max` bounds.
    ///
    /// Both bounds are clamped to be non-negative. If `max` is smaller than
    /// `min` on either axis (an inverted/inconsistent input), `max` is
    /// widened up to `min` on that axis — `min` is authoritative — so the
    /// resulting constraints always satisfy `min <= max`. Inverted input
    /// usually indicates an upstream layout bug (e.g. padding subtraction
    /// underflow), so debug builds print a diagnostic when this correction
    /// fires; release builds correct silently.
    pub fn new(min: Size, max: Size) -> Self {
        let min = non_negative(min);
        let max = non_negative(max);
        #[cfg(debug_assertions)]
        if max.width < min.width || max.height < min.height {
            eprintln!(
                "frust-core: BoxConstraints::new received inverted bounds \
                 (min {min:?} > max {max:?}); widening max to min — likely an \
                 upstream layout bug"
            );
        }
        let max = Size::new(max.width.max(min.width), max.height.max(min.height));
        Self { min, max }
    }

    /// Tight constraints that force an exact `size` (`min == max == size`).
    pub fn tight(size: Size) -> Self {
        let size = non_negative(size);
        Self {
            min: size,
            max: size,
        }
    }

    /// Loose constraints allowing anything from zero up to `max`.
    pub fn loose(max: Size) -> Self {
        Self {
            min: Size::ZERO,
            max: non_negative(max),
        }
    }

    /// The minimum allowed size.
    pub fn min(&self) -> Size {
        self.min
    }

    /// The maximum allowed size.
    pub fn max(&self) -> Size {
        self.max
    }

    /// Clamp `size` so it lies within `[min, max]` on both axes.
    pub fn constrain(&self, size: Size) -> Size {
        Size::new(
            size.width.clamp(self.min.width, self.max.width),
            size.height.clamp(self.min.height, self.max.height),
        )
    }

    /// Relax the minimum to zero, keeping the same maximum.
    pub fn loosen(&self) -> Self {
        Self {
            min: Size::ZERO,
            max: self.max,
        }
    }

    /// Return tight constraints at the `size` clamped into this range.
    ///
    /// Useful when a parent wants to force a child to a specific size that
    /// still respects the incoming bounds.
    pub fn tighten(&self, size: Size) -> Self {
        let size = self.constrain(size);
        Self {
            min: size,
            max: size,
        }
    }

    /// Whether the constraints force an exact size (`min == max`).
    pub fn is_tight(&self) -> bool {
        self.min == self.max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constrain_clamps_into_range() {
        let bc = BoxConstraints::new(Size::new(10.0, 10.0), Size::new(100.0, 100.0));
        // Below the minimum snaps up.
        assert_eq!(bc.constrain(Size::new(5.0, 5.0)), Size::new(10.0, 10.0));
        // Above the maximum snaps down.
        assert_eq!(
            bc.constrain(Size::new(200.0, 200.0)),
            Size::new(100.0, 100.0)
        );
        // Within range is untouched.
        assert_eq!(bc.constrain(Size::new(50.0, 40.0)), Size::new(50.0, 40.0));
    }

    #[test]
    fn loose_and_loosen_zero_the_minimum() {
        let bc = BoxConstraints::loose(Size::new(80.0, 60.0));
        assert_eq!(bc.min(), Size::ZERO);
        assert_eq!(bc.max(), Size::new(80.0, 60.0));

        let tight = BoxConstraints::tight(Size::new(30.0, 30.0));
        let loosened = tight.loosen();
        assert_eq!(loosened.min(), Size::ZERO);
        assert_eq!(loosened.max(), Size::new(30.0, 30.0));
    }

    #[test]
    fn tight_forces_exact_size() {
        let bc = BoxConstraints::tight(Size::new(42.0, 24.0));
        assert!(bc.is_tight());
        assert_eq!(bc.min(), bc.max());
        assert_eq!(
            bc.constrain(Size::new(1000.0, 1000.0)),
            Size::new(42.0, 24.0)
        );
    }

    #[test]
    fn tighten_clamps_then_locks() {
        let bc = BoxConstraints::new(Size::new(10.0, 10.0), Size::new(100.0, 100.0));
        // Request beyond max is clamped, then locked as tight.
        let t = bc.tighten(Size::new(500.0, 5.0));
        assert!(t.is_tight());
        assert_eq!(t.min(), Size::new(100.0, 10.0));
    }

    #[test]
    fn constructors_reject_negative_sizes() {
        let bc = BoxConstraints::loose(Size::new(-5.0, -5.0));
        assert_eq!(bc.max(), Size::ZERO);
    }

    #[test]
    fn new_widens_max_when_one_axis_is_inverted() {
        // width is inverted (min > max); height is consistent.
        let bc = BoxConstraints::new(Size::new(100.0, 10.0), Size::new(10.0, 100.0));
        assert!(bc.min().width <= bc.max().width);
        assert!(bc.min().height <= bc.max().height);
        assert_eq!(bc.min(), Size::new(100.0, 10.0));
        assert_eq!(bc.max(), Size::new(100.0, 100.0));

        // Does not panic and returns a sane, in-range value.
        let constrained = bc.constrain(Size::new(0.0, 0.0));
        assert_eq!(constrained, Size::new(100.0, 10.0));
    }

    #[test]
    fn new_widens_max_when_both_axes_are_inverted() {
        let bc = BoxConstraints::new(Size::new(100.0, 100.0), Size::new(10.0, 10.0));
        assert!(bc.min().width <= bc.max().width);
        assert!(bc.min().height <= bc.max().height);
        assert_eq!(bc.min(), Size::new(100.0, 100.0));
        assert_eq!(bc.max(), Size::new(100.0, 100.0));
        assert!(bc.is_tight());

        // Does not panic and returns a sane, in-range value.
        let constrained = bc.constrain(Size::new(1000.0, 1000.0));
        assert_eq!(constrained, Size::new(100.0, 100.0));
    }
}
