//! Snap-point detents for sheets and drawers: [`Detents`], the pure target
//! math, and [`DetentController`], a headless drag-and-spring driver.
//!
//! Progress runs `0.0` (closed) to `1.0` (open). A point in `0.0..=1.0` is a
//! fraction of the panel's extent along its slide axis, above `1.0` logical px.
//! `close_sign` is `+1.0` when the panel closes toward growing coordinates
//! (bottom/right), else `-1.0`. For finite input the math is exactly the
//! `frust-shadcn`/`frust-material` overlay modals'; nothing here panics or
//! returns `NaN`. Unrelated to the native-widgets `Detent` host-sheet type.

use std::cmp::Ordering;
use std::time::Duration;

use frust_core::{AnimationController, FrameTime, SpringDesc};

/// Progress at or below which a panel counts as closed.
pub const PROGRESS_EPSILON: f64 = 1e-4;

/// Release speed (px/s along the axis) from which a release takes the next
/// detent in its direction of travel instead of the nearest.
///
/// **Community-approximate**: no design system publishes one; 400 px/s is a
/// deliberate flick, both catalog overlay modals' value.
pub const DEFAULT_FLING_VELOCITY: f64 = 400.0;

/// What a resolved release does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DetentRelease {
    /// Landed closed on a dismissable panel: the caller runs its own exit.
    Dismiss,
    /// Rest at this progress.
    SnapTo(f64),
}

/// How a closed landing resolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DismissPolicy {
    /// A closed landing dismisses.
    Dismissable,
    /// A closed landing springs back to [`Detents::open_progress`].
    SpringBackTo,
}

/// A panel's authored snap points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detents {
    points: Vec<f64>,
}

impl Detents {
    /// Detents from `points`, dropping any NaN or infinite one.
    pub fn new(points: &[f64]) -> Self {
        Detents {
            points: points.iter().copied().filter(|p| p.is_finite()).collect(),
        }
    }

    /// The kept points, in authored order.
    pub fn points(&self) -> &[f64] {
        &self.points
    }

    /// `point` in progress space: clamped into `0.0..=1.0`, px over `extent`
    /// above `1.0` (`1.0` while `extent` is not positive). A would-be `NaN` is
    /// `0.0`, the closed target every set already holds.
    pub fn normalize(point: f64, extent: f64) -> f64 {
        let progress = if point <= 1.0 {
            point.clamp(0.0, 1.0)
        } else if extent > 0.0 {
            (point / extent).clamp(0.0, 1.0)
        } else {
            1.0
        };
        if progress.is_nan() { 0.0 } else { progress }
    }

    /// The resting points, ascending: `0.0` plus every point normalized, or
    /// `{0.0, 1.0}` with none (so "nearest" is the past-the-midpoint commit).
    pub fn targets(&self, extent: f64) -> Vec<f64> {
        let mut targets: Vec<f64> = std::iter::once(0.0)
            .chain(self.points.iter().map(|p| Self::normalize(*p, extent)))
            .collect();
        if targets.len() == 1 {
            targets.push(1.0);
        }
        targets.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        targets
    }

    /// Where an open panel rests: the first point normalized, or `1.0` with
    /// none or when that is at most [`PROGRESS_EPSILON`].
    pub fn open_progress(&self, extent: f64) -> f64 {
        self.points
            .first()
            .map(|p| Self::normalize(*p, extent))
            .filter(|p| *p > PROGRESS_EPSILON)
            .unwrap_or(1.0)
    }

    /// Progress after `delta_px` of travel from a drag begun at `from`, bounded
    /// to `0.0..=` the furthest target; `None` while `extent` is not positive.
    /// Bounded by `max`/`min`, not `clamp`, so a NaN input lands on `0.0`.
    pub fn drag_to(&self, from: f64, delta_px: f64, close_sign: f64, extent: f64) -> Option<f64> {
        if extent.is_nan() || extent <= 0.0 {
            return None;
        }
        let open = self.targets(extent).last().copied().unwrap_or(1.0);
        let target = from - close_sign * delta_px / extent;
        Some(target.max(0.0).min(open))
    }

    /// The target a release settles to: the nearest (a tie takes the lower), or
    /// from `|velocity| >= fling_velocity` the next one in the direction of
    /// travel. Always one of [`targets`](Self::targets): a NaN `progress` takes
    /// the lowest; a NaN `velocity` or `fling_velocity` never flings.
    pub fn release_target(
        &self,
        progress: f64,
        velocity: f64,
        close_sign: f64,
        extent: f64,
        fling_velocity: f64,
    ) -> f64 {
        let targets = self.targets(extent);
        if velocity.abs() >= fling_velocity {
            let opening = velocity * close_sign < 0.0;
            let next = if opening {
                targets.iter().copied().find(|t| *t > progress)
            } else {
                targets.iter().rev().copied().find(|t| *t < progress)
            };
            if let Some(next) = next {
                return next;
            }
        }
        targets
            .iter()
            .copied()
            .min_by(|a, b| {
                (a - progress)
                    .abs()
                    .partial_cmp(&(b - progress).abs())
                    .unwrap_or(Ordering::Equal)
            })
            .unwrap_or(0.0)
    }

    /// Resolve a release `target`: at most [`PROGRESS_EPSILON`] is a closed
    /// landing (dismiss, or snap to [`open_progress`](Self::open_progress) per
    /// `policy`); a non-finite one snaps to `open_progress`.
    pub fn settle(&self, target: f64, policy: DismissPolicy, extent: f64) -> DetentRelease {
        if !target.is_finite() {
            return DetentRelease::SnapTo(self.open_progress(extent));
        }
        if target > PROGRESS_EPSILON {
            return DetentRelease::SnapTo(target);
        }
        match policy {
            DismissPolicy::Dismissable => DetentRelease::Dismiss,
            DismissPolicy::SpringBackTo => DetentRelease::SnapTo(self.open_progress(extent)),
        }
    }
}

/// A headless detent driver: a drag scrubs progress; a release resolves
/// through [`Detents`] and springs to the target from the release velocity
/// (unclamped, so an under-damped [`SpringDesc`] overshoots before landing).
///
/// [`release`](Self::release) takes px/s along the axis (positive toward
/// growing coordinates) on the raw input clock (`PaintCtx::input_time`) and
/// converts it to progress/s as `-close_sign * velocity_px_s / extent`: a flick
/// toward the closing edge lowers progress. [`advance`](Self::advance) takes
/// the animation clock (`PaintCtx::frame_time`); the two agree at time scale
/// `1.0` (see [`AnimationController::spring_to_with_velocity`]).
#[derive(Clone, Debug)]
pub struct DetentController {
    detents: Detents,
    extent: f64,
    anim: AnimationController,
    drag_from: f64,
}

impl DetentController {
    /// A controller resting closed, at `0.0`, over a panel `extent` px thick.
    pub fn new(detents: Detents, extent: f64) -> Self {
        DetentController {
            detents,
            extent,
            // Zero duration: `animate_to` is then an instant placement.
            anim: AnimationController::new(Duration::ZERO),
            drag_from: 0.0,
        }
    }

    /// The detents this controller resolves against.
    pub fn detents(&self) -> &Detents {
        &self.detents
    }

    /// The panel's extent along its slide axis, in logical px.
    pub fn extent(&self) -> f64 {
        self.extent
    }

    /// Update the extent after a relayout.
    pub fn set_extent(&mut self, extent: f64) {
        self.extent = extent;
    }

    /// The current progress.
    pub fn progress(&self) -> f64 {
        self.anim.value()
    }

    /// The rate of change of progress, in progress units per second.
    pub fn velocity(&self) -> f64 {
        self.anim.velocity()
    }

    /// Whether a release's spring is still running.
    pub fn is_settling(&self) -> bool {
        self.anim.is_animating()
    }

    /// Rest at `progress` (clamped to `0.0..=1.0`, non-finite ignored) at once.
    pub fn jump_to(&mut self, progress: f64) {
        if progress.is_finite() {
            self.anim.stop();
            self.anim.animate_to(progress);
        }
    }

    /// Start a drag: halt any settle where it is and record the start progress.
    pub fn begin_drag(&mut self) {
        self.anim.stop();
        self.drag_from = self.progress();
    }

    /// Scrub by `delta_px` of total travel since [`begin_drag`](Self::begin_drag)
    /// ([`Detents::drag_to`]).
    pub fn drag_by(&mut self, delta_px: f64, close_sign: f64) {
        let next = self
            .detents
            .drag_to(self.drag_from, delta_px, close_sign, self.extent);
        if let Some(progress) = next {
            self.jump_to(progress);
        }
    }

    /// Release at `velocity_px_s` ([`DEFAULT_FLING_VELOCITY`]). `SnapTo` starts
    /// `spring` at the converted velocity; `Dismiss` holds progress for the
    /// caller's own exit.
    pub fn release(
        &mut self,
        velocity_px_s: f64,
        close_sign: f64,
        policy: DismissPolicy,
        spring: SpringDesc,
    ) -> DetentRelease {
        let target = self.detents.release_target(
            self.progress(),
            velocity_px_s,
            close_sign,
            self.extent,
            DEFAULT_FLING_VELOCITY,
        );
        let release = self.detents.settle(target, policy, self.extent);
        match release {
            DetentRelease::SnapTo(target) => {
                let velocity = -close_sign * velocity_px_s / self.extent;
                let velocity = if velocity.is_finite() { velocity } else { 0.0 };
                self.anim.spring_to_with_velocity(target, velocity, spring);
            }
            DetentRelease::Dismiss => self.anim.stop(),
        }
        release
    }

    /// Advance a settle to `now`; `true` while it still owes a frame.
    pub fn advance(&mut self, now: FrameTime) -> bool {
        self.anim.advance(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAN: f64 = f64::NAN;
    const INF: f64 = f64::INFINITY;

    /// Critically damped, so a settle never overshoots.
    const SPRING: SpringDesc = SpringDesc {
        mass: 1.0,
        stiffness: 400.0,
        damping_ratio: 1.0,
    };

    fn at(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    /// One parity row: a point set over an extent, and the values both
    /// catalogs' `open_progress` and `snap_targets` give for it.
    struct Row {
        points: &'static [f64],
        extent: f64,
        open: f64,
        targets: &'static [f64],
        /// `(progress, velocity px/s, close_sign, release_target)`.
        releases: &'static [(f64, f64, f64, f64)],
    }

    /// Worked by hand from `open_progress`, `normalize_snap`, `snap_targets`
    /// and `release_target` in `plugins/shadcn/src/overlay/modal.rs` and
    /// `plugins/material/src/overlay/modal.rs`, which agree for finite points.
    const ROWS: &[Row] = &[
        Row {
            points: &[],
            extent: 400.0,
            open: 1.0,
            targets: &[0.0, 1.0],
            releases: &[
                (0.5, 0.0, 1.0, 0.0), // an exact tie takes the lower target
                (0.6, 0.0, 1.0, 1.0),
                (0.4, 399.0, -1.0, 0.0),
                (0.6, 400.0, 1.0, 0.0),
                (0.4, -400.0, 1.0, 1.0),
                (0.4, 400.0, -1.0, 1.0),
                (0.6, -401.0, -1.0, 0.0),
                (0.6, -399.0, -1.0, 1.0),
                (1.0, -400.0, 1.0, 1.0), // nothing further open: nearest
                (0.0, 400.0, 1.0, 0.0),  // nothing further closed: nearest
            ],
        },
        Row {
            points: &[0.25, 0.75, 1.0],
            extent: 400.0,
            open: 0.25,
            targets: &[0.0, 0.25, 0.75, 1.0],
            releases: &[
                (0.6, 0.0, 1.0, 0.75),
                (0.6, 399.0, 1.0, 0.75),
                (0.6, 400.0, 1.0, 0.25),
                (0.6, 401.0, 1.0, 0.25),
                (0.6, -400.0, 1.0, 0.75),
                (0.8, -400.0, 1.0, 1.0),
                (0.8, -399.0, 1.0, 0.75),
                (0.8, 400.0, -1.0, 1.0),
                (0.8, -400.0, -1.0, 0.75),
                (0.3, 401.0, 1.0, 0.25),
                (0.1, 0.0, -1.0, 0.0),
                (0.1, 400.0, -1.0, 0.25),
            ],
        },
        Row {
            points: &[100.0, 1.0],
            extent: 400.0,
            open: 0.25,
            targets: &[0.0, 0.25, 1.0],
            releases: &[
                (0.5, 0.0, 1.0, 0.25),
                (0.7, 0.0, 1.0, 1.0),
                (0.7, 400.0, 1.0, 0.25),
            ],
        },
        Row {
            points: &[100.0, 1.0],
            extent: 0.0,
            open: 1.0,
            targets: &[0.0, 1.0, 1.0],
            releases: &[(0.5, 0.0, 1.0, 0.0), (0.7, 400.0, 1.0, 0.0)],
        },
        Row {
            points: &[100.0],
            extent: -10.0,
            open: 1.0,
            targets: &[0.0, 1.0],
            releases: &[(0.7, -400.0, 1.0, 1.0)],
        },
        Row {
            points: &[0.0, 0.6],
            extent: 400.0,
            open: 1.0,
            targets: &[0.0, 0.0, 0.6],
            releases: &[
                (0.5, 0.0, 1.0, 0.6),
                (0.2, 0.0, 1.0, 0.0),
                (0.6, -400.0, 1.0, 0.6),
                (0.5, 400.0, 1.0, 0.0),
            ],
        },
        Row {
            points: &[800.0],
            extent: 400.0,
            open: 1.0,
            targets: &[0.0, 1.0],
            releases: &[(0.6, 0.0, 1.0, 1.0)],
        },
        Row {
            points: &[0.75, 0.25],
            extent: 400.0,
            open: 0.75,
            targets: &[0.0, 0.25, 0.75],
            releases: &[(0.4, 0.0, 1.0, 0.25), (0.4, -400.0, 1.0, 0.75)],
        },
        Row {
            points: &[-0.5, 0.5],
            extent: 400.0,
            open: 1.0,
            targets: &[0.0, 0.0, 0.5],
            releases: &[(0.3, 0.0, 1.0, 0.5)],
        },
        Row {
            points: &[1.5],
            extent: 400.0,
            open: 0.00375,
            targets: &[0.0, 0.00375],
            releases: &[(0.5, 0.0, 1.0, 0.00375)],
        },
        Row {
            points: &[2.0],
            extent: 100_000.0,
            open: 1.0,
            targets: &[0.0, 2e-5],
            releases: &[(0.5, 0.0, 1.0, 2e-5)],
        },
    ];

    /// `(points, extent, from, delta px, close_sign, drag_move's scrub target)`.
    type DragRow = (&'static [f64], f64, f64, f64, f64, Option<f64>);
    const DRAGS: &[DragRow] = &[
        (&[], 400.0, 1.0, 100.0, 1.0, Some(0.75)),
        (&[], 400.0, 1.0, -100.0, 1.0, Some(1.0)),
        (&[], 400.0, 0.5, 100.0, -1.0, Some(0.75)),
        (&[], 400.0, 0.25, 200.0, 1.0, Some(0.0)),
        (&[0.5], 400.0, 0.5, -100.0, 1.0, Some(0.5)),
        (&[0.5], 400.0, 0.25, -100.0, 1.0, Some(0.5)),
        (&[0.25, 0.75, 1.0], 400.0, 0.25, -200.0, 1.0, Some(0.75)),
        (&[200.0], 400.0, 0.5, -400.0, 1.0, Some(0.5)),
        (&[0.0, 0.6], 400.0, 1.0, 0.0, 1.0, Some(0.6)),
        (&[], 0.0, 1.0, 100.0, 1.0, None),
        (&[], -1.0, 1.0, 100.0, 1.0, None),
    ];

    /// `(points, extent, target, policy, settle_drag's outcome)`.
    type SettleRow = (&'static [f64], f64, f64, DismissPolicy, DetentRelease);
    const SETTLES: &[SettleRow] = &[
        (
            &[],
            400.0,
            0.0,
            DismissPolicy::Dismissable,
            DetentRelease::Dismiss,
        ),
        (
            &[],
            400.0,
            0.0,
            DismissPolicy::SpringBackTo,
            DetentRelease::SnapTo(1.0),
        ),
        (
            &[0.25, 0.75],
            400.0,
            1e-4,
            DismissPolicy::Dismissable,
            DetentRelease::Dismiss,
        ),
        (
            &[0.25, 0.75],
            400.0,
            1e-4,
            DismissPolicy::SpringBackTo,
            DetentRelease::SnapTo(0.25),
        ),
        (
            &[0.25, 0.75],
            400.0,
            2e-4,
            DismissPolicy::Dismissable,
            DetentRelease::SnapTo(2e-4),
        ),
        (
            &[0.25, 0.75],
            400.0,
            0.75,
            DismissPolicy::SpringBackTo,
            DetentRelease::SnapTo(0.75),
        ),
        (
            &[0.0, 0.6],
            400.0,
            0.0,
            DismissPolicy::SpringBackTo,
            DetentRelease::SnapTo(1.0),
        ),
    ];

    #[test]
    fn detents_match_the_catalog_modal_math() {
        for row in ROWS {
            let d = Detents::new(row.points);
            let label = format!("{:?} @ {}", row.points, row.extent);
            assert_eq!(d.open_progress(row.extent), row.open, "open {label}");
            assert_eq!(d.targets(row.extent), row.targets, "targets {label}");
            for &(progress, velocity, sign, want) in row.releases {
                let got =
                    d.release_target(progress, velocity, sign, row.extent, DEFAULT_FLING_VELOCITY);
                assert_eq!(
                    got, want,
                    "release {label} ({progress}, {velocity}, {sign})"
                );
            }
        }
        for &(points, extent, from, delta, sign, want) in DRAGS {
            let got = Detents::new(points).drag_to(from, delta, sign, extent);
            assert_eq!(
                got, want,
                "drag {points:?} @ {extent} ({from}, {delta}, {sign})"
            );
        }
        for &(points, extent, target, policy, want) in SETTLES {
            let got = Detents::new(points).settle(target, policy, extent);
            assert_eq!(got, want, "settle {points:?} ({target}, {policy:?})");
        }
    }

    #[test]
    fn non_finite_points_are_dropped() {
        let d = Detents::new(&[NAN, 0.5, INF, -INF]);
        assert_eq!(d.points(), &[0.5]);
        assert_eq!(d.open_progress(400.0), 0.5);
        assert_eq!(d.targets(400.0), [0.0, 0.5]);
        assert_eq!(Detents::new(&[NAN]), Detents::default());
        assert_eq!(Detents::new(&[NAN]).targets(400.0), [0.0, 1.0]);
        // `normalize` is public, so it guards its own input too.
        assert_eq!(Detents::normalize(NAN, 400.0), 0.0);
        assert_eq!(Detents::normalize(INF, 400.0), 1.0);
        assert_eq!(Detents::normalize(-INF, 400.0), 0.0);
        assert_eq!(Detents::normalize(INF, INF), 0.0);
        assert_eq!(Detents::normalize(500.0, NAN), 1.0);
    }

    #[test]
    fn zero_first_point_opens_fully() {
        assert_eq!(Detents::new(&[0.0]).open_progress(400.0), 1.0);
        assert_eq!(Detents::new(&[0.0, 0.5]).open_progress(400.0), 1.0);
        assert_eq!(Detents::new(&[-0.2]).open_progress(400.0), 1.0);
        assert_eq!(Detents::new(&[PROGRESS_EPSILON]).open_progress(400.0), 1.0);
        assert_eq!(Detents::new(&[0.0]).targets(400.0), [0.0, 0.0]);
    }

    #[test]
    fn drag_clamp_is_nan_safe() {
        let d = Detents::new(&[0.25, 0.75]);
        assert_eq!(d.drag_to(NAN, 10.0, 1.0, 400.0), Some(0.0));
        assert_eq!(d.drag_to(0.5, NAN, 1.0, 400.0), Some(0.0));
        assert_eq!(d.drag_to(0.5, 10.0, NAN, 400.0), Some(0.0));
        assert_eq!(d.drag_to(0.5, INF, 1.0, 400.0), Some(0.0));
        assert_eq!(d.drag_to(0.5, -INF, 1.0, 400.0), Some(0.75));
        assert_eq!(d.drag_to(0.5, 10.0, 1.0, NAN), None);
        assert_eq!(d.drag_to(0.5, 10.0, 1.0, INF), Some(0.5));
        assert_eq!(d.drag_to(0.5, INF, 1.0, INF), Some(0.0));
        // Release resolution stays inside the target set on garbage input.
        assert_eq!(d.release_target(NAN, 0.0, 1.0, 400.0, 400.0), 0.0);
        assert_eq!(d.release_target(0.6, NAN, 1.0, 400.0, 400.0), 0.75);
        assert_eq!(d.release_target(0.6, 900.0, 1.0, 400.0, NAN), 0.75);
        assert_eq!(d.release_target(0.6, 900.0, NAN, 400.0, 400.0), 0.25);
    }

    #[test]
    fn settle_below_epsilon_dismisses_or_springs_back() {
        let d = Detents::new(&[0.5, 1.0]);
        let (dismiss, spring_back) = (DismissPolicy::Dismissable, DismissPolicy::SpringBackTo);
        for target in [0.0, PROGRESS_EPSILON / 2.0, PROGRESS_EPSILON, -0.3] {
            assert_eq!(d.settle(target, dismiss, 400.0), DetentRelease::Dismiss);
            assert_eq!(
                d.settle(target, spring_back, 400.0),
                DetentRelease::SnapTo(0.5)
            );
        }
        let above = PROGRESS_EPSILON * 2.0;
        assert_eq!(
            d.settle(above, dismiss, 400.0),
            DetentRelease::SnapTo(above)
        );
        for target in [NAN, INF, -INF] {
            assert_eq!(d.settle(target, dismiss, 400.0), DetentRelease::SnapTo(0.5));
        }
    }

    fn sheet() -> DetentController {
        DetentController::new(Detents::new(&[0.25, 0.75, 1.0]), 400.0)
    }

    /// Advance from `start_ms` in 16 ms frames until settled; panics if it never does.
    fn settle_from(c: &mut DetentController, start_ms: u64) {
        let settled = (0..400).any(|i| !c.advance(at(start_ms + i * 16)));
        assert!(settled, "the spring never settled");
    }

    #[test]
    fn begin_drag_interrupts_a_settle_without_a_jump() {
        let mut c = sheet();
        c.jump_to(0.25);
        c.begin_drag();
        c.drag_by(-150.0, 1.0);
        assert_eq!(c.progress(), 0.625);
        let release = c.release(0.0, 1.0, DismissPolicy::Dismissable, SPRING);
        assert_eq!(release, DetentRelease::SnapTo(0.75));
        for ms in [0, 16, 32] {
            assert!(c.advance(at(ms)));
        }
        let mid = c.progress();
        assert!(mid > 0.625 && mid < 0.75, "mid-settle progress {mid}");

        c.begin_drag();
        assert_eq!(c.progress(), mid);
        assert!(!c.is_settling());
        assert!(!c.advance(at(48)));
        assert_eq!(c.progress(), mid);
        c.drag_by(0.0, 1.0);
        assert_eq!(c.progress(), mid);
    }

    #[test]
    fn release_hands_velocity_to_the_spring() {
        // (close_sign, drag px, release px/s, target, progress velocity):
        // a bottom sheet flicked down, a top sheet flicked up, a bottom sheet
        // flicked up. Velocity is `-close_sign * px_s / extent`.
        let cases = [
            (1.0, 150.0, 800.0, 0.25, -2.0),
            (-1.0, -150.0, -800.0, 0.25, -2.0),
            (1.0, 150.0, -800.0, 0.75, 2.0),
        ];
        for (sign, drag, px_s, target, want) in cases {
            let mut c = sheet();
            c.jump_to(1.0);
            c.begin_drag();
            c.drag_by(drag, sign);
            assert_eq!(c.progress(), 0.625);
            let release = c.release(px_s, sign, DismissPolicy::Dismissable, SPRING);
            assert_eq!(release, DetentRelease::SnapTo(target));
            assert!((c.velocity() - want).abs() < 1e-9, "{}", c.velocity());
            assert!(c.advance(at(0)));
            assert!((c.velocity() - want).abs() < 1e-9, "{}", c.velocity());
            settle_from(&mut c, 16);
            assert_eq!(c.progress(), target);
        }
        // An unmeasured extent hands over no velocity rather than ∞ or NaN.
        let mut c = DetentController::new(Detents::default(), 0.0);
        c.jump_to(0.6);
        let release = c.release(-800.0, 1.0, DismissPolicy::Dismissable, SPRING);
        assert_eq!(release, DetentRelease::SnapTo(1.0));
        assert_eq!(c.velocity(), 0.0);
    }

    #[test]
    fn flick_skips_to_the_next_detent() {
        let open_flick = |px_s: f64| {
            let mut c = sheet();
            c.jump_to(0.75);
            c.begin_drag();
            c.drag_by(-25.0, 1.0);
            assert_eq!(c.progress(), 0.8125);
            let release = c.release(px_s, 1.0, DismissPolicy::Dismissable, SPRING);
            settle_from(&mut c, 0);
            (release, c.progress())
        };
        assert_eq!(open_flick(-399.0), (DetentRelease::SnapTo(0.75), 0.75));
        assert_eq!(open_flick(-400.0), (DetentRelease::SnapTo(1.0), 1.0));

        let mut c = sheet();
        c.jump_to(0.25);
        c.begin_drag();
        c.drag_by(25.0, 1.0);
        let release = c.release(400.0, 1.0, DismissPolicy::Dismissable, SPRING);
        assert_eq!(release, DetentRelease::Dismiss);
        assert!(!c.is_settling());
        assert_eq!(c.progress(), 0.1875);
        c.begin_drag();
        c.drag_by(25.0, 1.0);
        let release = c.release(400.0, 1.0, DismissPolicy::SpringBackTo, SPRING);
        assert_eq!(release, DetentRelease::SnapTo(0.25));
        assert!(c.is_settling());
    }
}
