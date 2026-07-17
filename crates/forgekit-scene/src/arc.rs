//! Arc-to-path helper (task 05, PLAN.md D2b).
//!
//! Circular progress indicators (task 10) and `CupertinoActivityIndicator`
//! (task 13) both need to paint a stroked arc — this is the one non-rect,
//! non-glyph shape the baseline widget set needs, so a tiny helper lives here
//! rather than being duplicated in each widget. It's a thin wrapper over
//! `kurbo::Arc::to_path` (`kurbo::Shape`'s tessellation into a `BezPath`),
//! kept in `forgekit-scene` (not `forgekit-core`/`forgekit-widgets`) since it
//! only touches `kurbo` types and every widget that needs it already depends
//! on this crate.

use kurbo::{Arc, BezPath, Point, Shape, Vec2};

/// Default flattening tolerance (in local px) for [`arc_path`]'s Bézier
/// tessellation — small enough that the arc reads as smooth at typical
/// progress-indicator/spinner sizes (16-48px), without generating an
/// excessive segment count.
const ARC_TOLERANCE: f64 = 0.1;

/// Builds a `kurbo::BezPath` tracing a circular arc centered at `center` with
/// the given `radius`, from `start_angle` sweeping `sweep_angle` (both in
/// radians, matching `kurbo::Arc`'s convention: 0 = positive x-axis, positive
/// = clockwise in a y-down coordinate space).
///
/// The returned path is a bare arc outline — no `MoveTo` back to `center` and
/// no `ClosePath` — the shape a stroked progress ring/spinner segment needs;
/// pass it to [`crate::SceneBuilder::stroke_path`] (via
/// `forgekit-core`'s `PaintScene::stroke_path`) rather than `fill_path`.
pub fn arc_path(center: Point, radius: f64, start_angle: f64, sweep_angle: f64) -> BezPath {
    let arc = Arc::new(
        center,
        Vec2::new(radius, radius),
        start_angle,
        sweep_angle,
        0.0,
    );
    arc.to_path(ARC_TOLERANCE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::PathEl;
    use std::f64::consts::PI;

    #[test]
    fn arc_path_starts_with_a_move_to() {
        let path = arc_path(Point::new(10.0, 10.0), 5.0, 0.0, PI / 2.0);
        assert!(matches!(path.elements().first(), Some(PathEl::MoveTo(_))));
    }

    #[test]
    fn arc_path_produces_curve_segments_for_a_quarter_turn() {
        let path = arc_path(Point::new(0.0, 0.0), 10.0, 0.0, PI / 2.0);
        let curve_count = path
            .elements()
            .iter()
            .filter(|el| matches!(el, PathEl::CurveTo(..) | PathEl::QuadTo(..)))
            .count();
        assert!(curve_count >= 1, "expected at least one curve segment");
    }

    #[test]
    fn full_circle_produces_more_segments_than_a_quarter_turn() {
        let quarter = arc_path(Point::new(0.0, 0.0), 10.0, 0.0, PI / 2.0);
        let full = arc_path(Point::new(0.0, 0.0), 10.0, 0.0, 2.0 * PI);

        let count = |p: &BezPath| {
            p.elements()
                .iter()
                .filter(|el| matches!(el, PathEl::CurveTo(..) | PathEl::QuadTo(..)))
                .count()
        };
        assert!(count(&full) > count(&quarter));
    }

    #[test]
    fn arc_path_endpoints_lie_on_the_circle() {
        let center = Point::new(3.0, 4.0);
        let radius = 7.0;
        let path = arc_path(center, radius, 0.0, PI);

        let start = match path.elements().first() {
            Some(PathEl::MoveTo(p)) => *p,
            other => panic!("expected MoveTo, got {other:?}"),
        };
        let end = match path.elements().last() {
            Some(PathEl::CurveTo(_, _, p)) => *p,
            Some(PathEl::QuadTo(_, p)) => *p,
            Some(PathEl::LineTo(p)) => *p,
            other => panic!("expected a terminal segment, got {other:?}"),
        };

        let dist = |p: Point| ((p.x - center.x).powi(2) + (p.y - center.y).powi(2)).sqrt();
        assert!((dist(start) - radius).abs() < 1e-6);
        assert!((dist(end) - radius).abs() < 1e-6);
    }
}
