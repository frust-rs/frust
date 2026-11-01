//! [`PageSnap`] — a port of Flutter's `PageScrollPhysics`
//! (`widgets/page_view.dart`): a release settles on a page boundary instead of
//! wherever the fling would have coasted to.
//!
//! It overrides **only** [`ScrollPhysics::create_ballistic_simulation`]; every
//! other method (drag mapping, boundary conditions, fling thresholds, carried
//! momentum, tolerance, spring) walks up the [`chain`](PageSnap::chain) to its
//! parent, [`default_physics`] unless one is named. That is the chaining
//! contract [`ScrollPhysics`] documents for exactly this kind of snap.
//!
//! # Page extent
//!
//! [`ScrollMetrics`] carries no stride, so a `PageSnap` owns its own:
//!
//! * [`PageSnap::fraction`] — `viewport_dimension × fraction`, Flutter's
//!   `PageController.viewportFraction` (`1.0` is one full page per viewport).
//!   The product is used as-is; no rounding to whole pixels is applied, so
//!   `fraction(0.62)` on a 400 px viewport strides `248.0` px (to within f64
//!   epsilon).
//! * [`PageSnap::extent`] — a fixed stride in px, the shape of Flutter's
//!   `FixedExtentScrollPhysics`.
//!
//! A non-finite or non-positive extent has no page boundaries to snap to, so
//! the release is left to the parent.
//!
//! # Ported from Flutter
//!
//! `PageScrollPhysics._getTargetPixels`: `page = (pixels − min) / extent`; a
//! release faster than `tolerance.velocity` toward the leading edge
//! (`velocity < −tolerance.velocity`) targets `floor(page)`, faster toward the
//! trailing edge (`velocity > tolerance.velocity`) targets `ceil(page)`, and
//! anything slower targets `round(page)`. Flutter writes the first two as
//! `page ∓ 0.5` followed by `round()`; the two agree everywhere except on
//! exact half-page positions with a fling in the *other* direction, which
//! never matters for a continuous drag. The target `min + page × extent` is
//! clamped to `[min, max]`, and the settle is `SpringSimulation(spring, pixels,
//! target, velocity, tolerance)` — here the snapping
//! [`ScrollSpringSimulation`](super::simulation::ScrollSpringSimulation), which
//! reports exactly the target once at rest.
//!
//! # Deviations from the Dart source
//!
//! * **Out-of-range releases go to the parent.** Flutter delegates when
//!   `velocity <= 0 && pixels <= min` or `velocity >= 0 && pixels >= max`; this
//!   port delegates whenever `pixels` is outside `[min, max]` (and when the
//!   release already rests on its target), so the parent's rubber-band or edge
//!   settle always owns an overscrolled release.
//! * **The fling gate is the surface's.** Flutter returns `null` below
//!   `tolerance.velocity` only after its own target check; the surfaces here
//!   zero a release below [`ScrollPhysics::min_fling_velocity`] before asking,
//!   and a zero velocity simply rounds to the nearest page.

use super::simulation::ScrollSpringSimulation;
use super::{ScrollMetrics, ScrollPhysics, Simulation, default_physics};

/// How a [`PageSnap`] derives its stride.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PageExtent {
    /// `viewport_dimension × fraction`.
    Fraction(f64),
    /// A fixed stride in px.
    Fixed(f64),
}

/// Snap a release to the nearest page boundary — Flutter's
/// `PageScrollPhysics`. See the [module docs](self) for the ported math and
/// deviations.
#[derive(Debug)]
pub struct PageSnap {
    page: PageExtent,
    parent: Box<dyn ScrollPhysics>,
}

impl PageSnap {
    /// Pages sized `viewport_dimension × viewport_fraction`, parented on
    /// [`default_physics`].
    pub fn fraction(viewport_fraction: f64) -> Self {
        Self {
            page: PageExtent::Fraction(viewport_fraction),
            parent: Box::new(RcParent(default_physics())),
        }
    }

    /// Pages of a fixed `px` stride, parented on [`default_physics`].
    pub fn extent(px: f64) -> Self {
        Self {
            page: PageExtent::Fixed(px),
            parent: Box::new(RcParent(default_physics())),
        }
    }

    /// Chain `parent` behind this physics, replacing the default parent — the
    /// module-wide composition convention ([`ScrollPhysics`]' *Chaining*).
    pub fn chain(self, parent: impl ScrollPhysics + 'static) -> Self {
        Self {
            page: self.page,
            parent: Box::new(parent),
        }
    }

    /// The page stride for `metrics`, `None` when it is not a positive finite
    /// length.
    fn stride(&self, metrics: &ScrollMetrics) -> Option<f64> {
        let px = match self.page {
            PageExtent::Fraction(f) => metrics.viewport_dimension * f,
            PageExtent::Fixed(px) => px,
        };
        (px.is_finite() && px > 0.0).then_some(px)
    }

    /// Flutter's `_getTargetPixels`, clamped to the scroll range.
    fn target_pixels(
        &self,
        metrics: &ScrollMetrics,
        velocity: f64,
        velocity_tolerance: f64,
    ) -> Option<f64> {
        let extent = self.stride(metrics)?;
        let min = metrics.min_scroll_extent;
        let page = (metrics.pixels - min) / extent;
        let page = if velocity < -velocity_tolerance {
            page.floor()
        } else if velocity > velocity_tolerance {
            page.ceil()
        } else {
            page.round()
        };
        Some((min + page * extent).clamp(min, metrics.max_scroll_extent))
    }
}

/// Adapter so the shared `Rc<dyn ScrollPhysics>` [`default_physics`] returns
/// can sit behind the `Box<dyn ScrollPhysics>` parent slot.
#[derive(Debug)]
struct RcParent(std::rc::Rc<dyn ScrollPhysics>);

impl ScrollPhysics for RcParent {
    fn parent(&self) -> Option<&dyn ScrollPhysics> {
        Some(self.0.as_ref())
    }
}

impl ScrollPhysics for PageSnap {
    fn parent(&self) -> Option<&dyn ScrollPhysics> {
        Some(self.parent.as_ref())
    }

    /// Flutter's `PageScrollPhysics.createBallisticSimulation`: a spring from
    /// the release position onto the chosen page boundary, or the parent's
    /// answer when out of range or already settled on the target.
    fn create_ballistic_simulation(
        &self,
        metrics: &ScrollMetrics,
        velocity: f64,
    ) -> Option<Box<dyn Simulation>> {
        let parent = || self.parent.create_ballistic_simulation(metrics, velocity);
        if metrics.out_of_range() {
            return parent();
        }
        let tolerance = self.tolerance_for(metrics);
        let Some(target) = self.target_pixels(metrics, velocity, tolerance.velocity) else {
            return parent();
        };
        if (target - metrics.pixels).abs() < tolerance.distance {
            return parent();
        }
        Some(Box::new(ScrollSpringSimulation::new(
            self.spring(),
            metrics.pixels,
            target,
            velocity,
            tolerance,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::parity::{Bouncing, Clamping};
    use crate::test_support::{RecordingScene, leaf};
    use crate::{CustomScrollView, custom_scroll_view};
    use frust_core::{
        FrameTime, InputEvent, PointerButton, PointerEvent, PointerPhase, RenderRoot, ScrollAxis,
    };
    use kurbo::{Point, Size};
    use std::any::Any;

    const EXTENT: f64 = 100.0;

    fn metrics(pixels: f64) -> ScrollMetrics {
        ScrollMetrics {
            pixels,
            min_scroll_extent: 0.0,
            max_scroll_extent: 400.0,
            viewport_dimension: 100.0,
            device_pixel_ratio: 1.0,
        }
    }

    /// The settle position of the simulation `physics` builds, far in the
    /// future; `None` when it builds nothing.
    fn settle(physics: &PageSnap, pixels: f64, velocity: f64) -> Option<f64> {
        let sim = physics.create_ballistic_simulation(&metrics(pixels), velocity)?;
        Some(sim.x(100.0))
    }

    fn snap() -> PageSnap {
        PageSnap::extent(EXTENT).chain(Bouncing::new())
    }

    #[test]
    fn target_selection_follows_direction_then_nearest() {
        let p = snap();
        let tol = p.tolerance_for(&metrics(0.0)).velocity;
        let slow = tol * 0.5;
        let fast = tol * 10.0;
        assert_eq!(settle(&p, 40.0, slow), Some(0.0));
        assert_eq!(settle(&p, 40.0, fast), Some(100.0));
        assert_eq!(settle(&p, 60.0, -fast), Some(0.0));
        assert_eq!(settle(&p, 60.0, slow), Some(100.0));
    }

    #[test]
    fn targets_clamp_to_the_range() {
        let p = snap();
        assert_eq!(settle(&p, 390.0, 5000.0), Some(400.0));
        assert_eq!(settle(&p, 10.0, -5000.0), Some(0.0));
        // A stride that does not divide the range evenly clamps the last page.
        let uneven = PageSnap::extent(150.0).chain(Bouncing::new());
        assert_eq!(settle(&uneven, 390.0, 5000.0), Some(400.0));
    }

    #[test]
    fn fraction_multiplies_the_viewport() {
        let p = PageSnap::fraction(0.62);
        let m = ScrollMetrics {
            viewport_dimension: 400.0,
            ..metrics(0.0)
        };
        let stride = p.stride(&m).expect("positive stride");
        assert!((stride - 248.0).abs() < 1e-9, "stride {stride}");
        assert_eq!(PageSnap::fraction(0.0).stride(&m), None);
        assert_eq!(PageSnap::extent(f64::NAN).stride(&m), None);
        assert_eq!(PageSnap::extent(-4.0).stride(&m), None);
    }

    #[test]
    fn overscrolled_release_is_the_parents_spring_back() {
        let snapped = snap();
        let bouncing = Bouncing::new();
        let m = metrics(430.0);
        let a = snapped.create_ballistic_simulation(&m, 0.0).expect("sim");
        let b = bouncing.create_ballistic_simulation(&m, 0.0).expect("sim");
        for t in [0.0, 0.05, 0.2, 0.6, 3.0] {
            assert_eq!(a.x(t), b.x(t), "x({t})");
            assert_eq!(a.dx(t), b.dx(t), "dx({t})");
        }
        let m = metrics(-30.0);
        let a = snapped.create_ballistic_simulation(&m, 0.0).expect("sim");
        let b = bouncing.create_ballistic_simulation(&m, 0.0).expect("sim");
        assert_eq!(a.x(0.3), b.x(0.3));
    }

    #[test]
    fn other_methods_delegate_to_the_parent() {
        let snapped = snap();
        let bouncing = Bouncing::new();
        let m = metrics(390.0);
        assert_eq!(
            snapped.apply_physics_to_user_offset(&m, 25.0),
            bouncing.apply_physics_to_user_offset(&m, 25.0)
        );
        assert_eq!(
            snapped.apply_boundary_conditions(&m, 430.0),
            bouncing.apply_boundary_conditions(&m, 430.0)
        );
        assert_eq!(snapped.min_fling_velocity(), bouncing.min_fling_velocity());
        assert_eq!(snapped.max_fling_velocity(), bouncing.max_fling_velocity());
        assert_eq!(snapped.spring(), bouncing.spring());
        let clamped = PageSnap::extent(EXTENT).chain(Clamping::new());
        assert_eq!(
            clamped.apply_boundary_conditions(&m, 430.0),
            Clamping::new().apply_boundary_conditions(&m, 430.0)
        );
    }

    #[test]
    fn resting_on_a_boundary_defers_to_the_parent() {
        let p = snap();
        assert!(
            p.create_ballistic_simulation(&metrics(200.0), 0.0)
                .is_none()
        );
    }

    // --- behaviour through the surface -----------------------------------

    fn ev(phase: PointerPhase, x: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, 50.0),
            button: PointerButton::Primary,
        })
    }

    fn ft(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn offset(root: &RenderRoot<(), CustomScrollView<()>>) -> f64 {
        let id = root.root_id().expect("root built");
        (root.tree().pod(id).expect("pod").widget() as &dyn Any)
            .downcast_ref::<crate::viewport::ViewportWidget>()
            .expect("viewport")
            .offset()
    }

    /// Drag left 60 px (past the touch slop) with `gap_ms` between samples,
    /// release, then pump frames until the motion rests; returns the offset.
    fn drag_and_settle(gap_ms: f64) -> f64 {
        let mut logic = |_: &mut ()| {
            custom_scroll_view(leaf(500.0, 100.0))
                .axis(ScrollAxis::Horizontal)
                .physics(PageSnap::extent(EXTENT))
        };
        let mut root: RenderRoot<(), CustomScrollView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));
        let mut sink = RecordingScene::default();
        let mut t = 0.0;
        root.paint(&mut sink, ft(t));
        root.event(&mut state, &ev(PointerPhase::Down, 90.0));
        t += gap_ms;
        root.paint(&mut sink, ft(t));
        // The move that crosses the slop only takes the gesture over; the
        // offset follows the finger from the next move on.
        for x in [70.0, 50.0, 30.0] {
            root.event(&mut state, &ev(PointerPhase::Move, x));
            t += gap_ms;
            root.paint(&mut sink, ft(t));
        }
        let moved = offset(&root);
        assert!(moved > 0.0 && moved < 50.0, "released at {moved}");
        root.event(&mut state, &ev(PointerPhase::Up, 30.0));
        for _ in 0..600 {
            t += 16.0;
            root.paint(&mut sink, ft(t));
        }
        offset(&root)
    }

    #[test]
    fn slow_release_settles_back_on_the_first_page() {
        assert_eq!(drag_and_settle(2000.0), 0.0);
    }

    #[test]
    fn strong_release_settles_on_the_next_page() {
        assert_eq!(drag_and_settle(8.0), 100.0);
    }
}
