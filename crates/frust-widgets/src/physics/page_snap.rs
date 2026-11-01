//! [`PageSnap`] — a port of Flutter's `PageScrollPhysics`: a release settles on
//! a page boundary instead of wherever the fling would have coasted to.
//!
//! It overrides only [`ScrollPhysics::create_ballistic_simulation`]; every other
//! method walks up the [`chain`](PageSnap::chain) to its parent,
//! [`default_physics`] unless one is named. [`ScrollMetrics`] carries no stride,
//! so a `PageSnap` owns one ([`PageSnap::fraction`] or [`PageSnap::extent`]).
//! Pages start at `min_scroll_extent` and a target is clamped to the range, so
//! an uneven last page is a shorter step. Physics composition:
//! `docs/WIDGETS_ARCHITECTURE.md` § Scroll Physics.

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

/// Snap a release to the nearest page boundary — Flutter's `PageScrollPhysics`.
#[derive(Debug)]
pub struct PageSnap {
    page: PageExtent,
    parent: Box<dyn ScrollPhysics>,
}

impl PageSnap {
    /// Pages sized `viewport_dimension × viewport_fraction` (Flutter's
    /// `viewportFraction`, unrounded), parented on [`default_physics`]. A
    /// non-finite or non-positive fraction is `debug_assert!`ed; release builds
    /// fall back to one page per viewport where Flutter would divide by zero.
    pub fn fraction(viewport_fraction: f64) -> Self {
        debug_assert!(
            viewport_fraction.is_finite() && viewport_fraction > 0.0,
            "PageSnap::fraction needs a finite positive fraction, got {viewport_fraction}"
        );
        Self {
            page: PageExtent::Fraction(viewport_fraction),
            parent: Box::new(RcParent(default_physics())),
        }
    }

    /// Pages of a fixed `px` stride (Flutter's `FixedExtentScrollPhysics`
    /// shape), parented on [`default_physics`]; an invalid `px` is handled as
    /// in [`fraction`](Self::fraction).
    pub fn extent(px: f64) -> Self {
        debug_assert!(
            px.is_finite() && px > 0.0,
            "PageSnap::extent needs a finite positive stride, got {px}"
        );
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

    /// The configured stride for `metrics`, if it is a positive finite length.
    fn stride(&self, metrics: &ScrollMetrics) -> Option<f64> {
        let px = match self.page {
            PageExtent::Fraction(f) => metrics.viewport_dimension * f,
            PageExtent::Fixed(px) => px,
        };
        (px.is_finite() && px > 0.0).then_some(px)
    }

    /// The stride used for snapping: the configured one, else the viewport
    /// extent (the misconfiguration fallback), else `None`.
    fn effective_stride(&self, metrics: &ScrollMetrics) -> Option<f64> {
        self.stride(metrics).or_else(|| {
            let v = metrics.viewport_dimension;
            (v.is_finite() && v > 0.0).then_some(v)
        })
    }

    /// Flutter's `_getTargetPixels`: `page = (pixels − min) / stride`, rounded
    /// after a ±0.5 bias when the release beats `velocity_tolerance` toward an
    /// edge (both languages round halves away from zero, so a fling released on
    /// a boundary moves one page), and clamped to `[min, max]`.
    fn target_pixels(
        &self,
        metrics: &ScrollMetrics,
        velocity: f64,
        velocity_tolerance: f64,
    ) -> Option<f64> {
        let extent = self.effective_stride(metrics)?;
        let min = metrics.min_scroll_extent;
        let page = (metrics.pixels - min) / extent;
        let page = if velocity < -velocity_tolerance {
            (page - 0.5).round()
        } else if velocity > velocity_tolerance {
            (page + 0.5).round()
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

    /// Flutter's `createBallisticSimulation`: a spring from the release onto the
    /// chosen boundary. Unlike Flutter, any out-of-range release (and a viewport
    /// with no usable extent) is the parent's, so its rubber-band or edge settle
    /// owns overscroll; an in-range release never is.
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
        // Never `None` in range: surfaces read `None` as "not owned" and would
        // run their legacy fling, resting a partial page off. A spring already
        // within tolerance settles on its first tick, exactly on `target`.
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
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "finite positive fraction")]
    fn non_positive_fraction_is_asserted() {
        let _ = PageSnap::fraction(0.0);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "finite positive stride")]
    fn negative_extent_is_asserted() {
        let _ = PageSnap::extent(-1.0);
    }

    #[test]
    fn invalid_extent_falls_back_to_the_viewport() {
        // Built directly: the constructors assert in debug builds.
        for bad in [0.0, -4.0, f64::NAN, f64::INFINITY] {
            let p = PageSnap {
                page: PageExtent::Fixed(bad),
                parent: Box::new(Bouncing::new()),
            };
            assert_eq!(p.stride(&metrics(0.0)), None);
            assert_eq!(settle(&p, 40.0, 0.0), Some(0.0));
            assert_eq!(settle(&p, 60.0, 0.0), Some(100.0));
        }
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

    fn parents() -> [PageSnap; 2] {
        [
            PageSnap::extent(EXTENT).chain(Bouncing::new()),
            PageSnap::extent(EXTENT).chain(Clamping::new()),
        ]
    }

    #[test]
    fn a_fling_on_a_boundary_moves_one_page_in_its_direction() {
        for p in parents() {
            // Flutter: round(k + 0.5) = k + 1 forward, round(k - 0.5) = k back.
            assert_eq!(settle(&p, 200.0, 2000.0), Some(300.0));
            assert_eq!(settle(&p, 200.0, -2000.0), Some(200.0));
            // At the range edges the fling is absorbed on the boundary.
            assert_eq!(settle(&p, 400.0, 2000.0), Some(400.0));
            assert_eq!(settle(&p, 0.0, -2000.0), Some(0.0));
        }
    }

    #[test]
    fn a_fling_just_short_of_a_boundary_ends_on_it() {
        for p in parents() {
            assert_eq!(settle(&p, 199.5, 2000.0), Some(200.0));
            assert_eq!(settle(&p, 200.4, -2000.0), Some(200.0));
            assert_eq!(settle(&p, 199.9999, 2000.0), Some(200.0));
        }
    }

    #[test]
    fn a_slow_release_on_a_boundary_is_owned_and_rests_exactly_there() {
        for p in parents() {
            let tol = p.tolerance_for(&metrics(200.0)).velocity;
            for v in [0.0, tol * 0.5, -tol * 0.5] {
                let sim = p
                    .create_ballistic_simulation(&metrics(200.0), v)
                    .expect("an in-range release is never left to the surface");
                assert!(sim.is_done(0.0), "v {v}: settled on its first tick");
                assert_eq!(sim.x(0.0), 200.0, "v {v}");
                assert_eq!(sim.x(100.0), 200.0, "v {v}");
            }
            // Within `tolerance.distance` of the boundary, not exactly on it:
            // still owned, and the spring reports the boundary itself.
            let sim = p
                .create_ballistic_simulation(&metrics(199.9999), 0.0)
                .expect("owned");
            assert_eq!(sim.x(100.0), 200.0);
        }
    }

    #[test]
    fn an_in_range_release_always_ends_on_a_boundary() {
        for p in parents() {
            for px in [
                0.0, 0.3, 49.9, 50.0, 99.9, 100.0, 199.5, 200.4, 399.9, 400.0,
            ] {
                for v in [-3000.0, -500.0, 0.0, 500.0, 3000.0] {
                    let x = settle(&p, px, v).unwrap_or_else(|| {
                        panic!("px {px} v {v}: an in-range release must be owned")
                    });
                    assert_eq!(x % EXTENT, 0.0, "px {px} v {v} ended at {x}");
                }
            }
        }
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

    /// Drag onto a page boundary and release with the finger still creeping:
    /// faster than the legacy fling's `FLING_STOP` (30 px/s) but slower than
    /// the physics' `min_fling_velocity`, which zeroes the release before the
    /// physics sees it. The physics must own that release (a settled spring),
    /// or the surface falls through to its legacy fling and coasts off the
    /// boundary.
    #[test]
    fn a_creeping_release_on_a_boundary_stays_on_it() {
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
        // Take the gesture over, then two fast pulls that park the offset
        // 9 px short of the second page boundary ...
        for x in [70.0, 20.0, -21.0] {
            t += 20.0;
            root.paint(&mut sink, ft(t));
            root.event(&mut state, &ev(PointerPhase::Move, x));
        }
        // ... pause long enough for those samples to age out of the velocity
        // window, then creep onto the boundary at 3 px per 40 ms (75 px/s).
        t += 200.0;
        root.paint(&mut sink, ft(t));
        for x in [-24.0, -27.0, -30.0] {
            t += 40.0;
            root.paint(&mut sink, ft(t));
            root.event(&mut state, &ev(PointerPhase::Move, x));
        }
        let parked = offset(&root);
        assert!(
            (parked - EXTENT).abs() < 1.0,
            "the setup must park within tolerance of a boundary, got {parked}"
        );
        root.event(&mut state, &ev(PointerPhase::Up, -30.0));
        for _ in 0..600 {
            t += 16.0;
            root.paint(&mut sink, ft(t));
        }
        let rested = offset(&root);
        assert_eq!(rested % EXTENT, 0.0, "rested at {rested}");
    }
}
