//! A minimal shape-morphing primitive: interpolation between rounded regular
//! polygons, producing a closed [`kurbo::BezPath`].
//!
//! # Scope
//!
//! This is **deliberately not** the full M3 Expressive 35-shape library. It
//! covers exactly what the [`mod@super::loading_indicator`] and button-group
//! state morphs need: interpolate between two rounded polygons at a
//! parameter `t ∈ 0..1`. A future addition can replace [`RoundedPolygon`]'s
//! radial model with the real M3X feature-point curve set without changing
//! this module's `morph_path` seam.
//!
//! # Model
//!
//! A [`RoundedPolygon`] is described as a **radial function** `r(θ)` giving the
//! outline distance from the centre at each angle, for a unit circumradius. A
//! regular `n`-gon's exact radial function is `cos(π/n) / cos(a)` where `a` is
//! the angle within one sector measured from an edge midpoint (`1.0` at the
//! vertices, `cos(π/n)` — the apothem — at the edge midpoints). Rounding blends
//! that toward the circumscribed circle (`r ≡ 1.0`): `rounding = 1.0` is an
//! exact circle regardless of `sides`, `rounding = 0.0` is a sharp polygon.
//!
//! This is a cheap, monotonic rounding approximation — **not** geometrically
//! exact corner arcs — but it reads as a rounded polygon and, crucially, gives
//! well-defined, exactly-reproducible interpolation endpoints (see
//! [`morph_path`]). Both a single shape and a morph are sampled at [`SAMPLES`]
//! evenly-spaced angles and emitted as a closed polyline; at a 48dp indicator
//! size that reads as smooth.

use std::f64::consts::{PI, TAU};

use kurbo::{BezPath, Point};

/// Number of evenly-spaced angular samples taken around a shape's outline when
/// building its path. High enough that a full circle reads smooth at typical
/// indicator sizes (≤ 48dp); a morph samples both shapes at these same angles
/// so corresponding points always line up.
pub const SAMPLES: usize = 96;

/// A rounded regular polygon, described by its radial function (see the
/// [module docs](self)). Centre-agnostic and scale-agnostic: the concrete
/// centre and circumradius are supplied at path-build time
/// ([`RoundedPolygon::to_path`]/[`morph_path`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundedPolygon {
    /// Number of sides (clamped to `>= 3` when evaluated). Ignored when
    /// `rounding == 1.0` (a circle).
    sides: u32,
    /// Corner rounding in `0.0..=1.0`: `0.0` sharp polygon, `1.0` circle.
    rounding: f64,
    /// Orientation offset, radians (rotates the whole outline). Lets a cycle of
    /// shapes appear to spin as it morphs.
    rotation: f64,
}

impl RoundedPolygon {
    /// A rounded regular polygon with `sides` sides, `rounding ∈ 0.0..=1.0`
    /// (`0.0` sharp, `1.0` circle), rotated by `rotation` radians.
    pub const fn new(sides: u32, rounding: f64, rotation: f64) -> Self {
        RoundedPolygon {
            sides,
            rounding,
            rotation,
        }
    }

    /// A circle (radial function `r ≡ 1.0`, independent of `sides`/`rotation`).
    pub const fn circle() -> Self {
        RoundedPolygon::new(4, 1.0, 0.0)
    }

    /// The outline radius at angle `theta`, for a unit circumradius (see the
    /// [module docs](self)). Always `> 0` for a finite non-zero shape.
    fn unit_radius_at(&self, theta: f64) -> f64 {
        let n = self.sides.max(3) as f64;
        let seg = TAU / n;
        // Angle within one sector, measured from an edge midpoint: a ∈
        // [-π/n, π/n], where cos(a) is never zero (π/n < π/2 for n >= 3).
        let a = (theta - self.rotation).rem_euclid(seg) - seg / 2.0;
        let poly = (PI / n).cos() / a.cos();
        let round = self.rounding.clamp(0.0, 1.0);
        // Blend the polygon radius toward the circumscribed circle (r = 1.0).
        poly + (1.0 - poly) * round
    }

    /// Build this shape's closed outline path, centred at `center` with
    /// circumradius `radius`. Equivalent to [`morph_path`] against itself at
    /// any `t`.
    pub fn to_path(&self, center: Point, radius: f64) -> BezPath {
        sample_path(center, |theta| self.unit_radius_at(theta) * radius)
    }
}

/// Sample a radial outline into a closed polyline path.
fn sample_path(center: Point, radius_at: impl Fn(f64) -> f64) -> BezPath {
    let mut path = BezPath::new();
    for i in 0..SAMPLES {
        let theta = TAU * (i as f64) / (SAMPLES as f64);
        let r = radius_at(theta);
        let p = Point::new(center.x + r * theta.cos(), center.y + r * theta.sin());
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    path.close_path();
    path
}

/// Interpolate between rounded polygons `a` (at `t = 0`) and `b` (at `t = 1`),
/// returning the morphed outline centred at `center` with circumradius
/// `radius`.
///
/// Interpolation is per-sample-angle radial lerp, so the endpoints are exact:
/// `morph_path(a, b, 0.0, ..)` reproduces `a.to_path(..)` and
/// `morph_path(a, b, 1.0, ..)` reproduces `b.to_path(..)`, bit-for-bit. `t` is
/// **not** clamped — a caller feeding a spring value may pass `t` slightly past
/// `0.0`/`1.0` to preserve the spring's overshoot (see
/// [`mod@super::loading_indicator`]).
pub fn morph_path(
    a: &RoundedPolygon,
    b: &RoundedPolygon,
    t: f64,
    center: Point,
    radius: f64,
) -> BezPath {
    sample_path(center, |theta| {
        let ra = a.unit_radius_at(theta) * radius;
        let rb = b.unit_radius_at(theta) * radius;
        ra + (rb - ra) * t
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::PathEl;

    const CENTER: Point = Point::new(50.0, 50.0);
    const R: f64 = 24.0;

    /// The `(x, y)` of every on-path point (MoveTo/LineTo) in order.
    fn points(path: &BezPath) -> Vec<Point> {
        path.elements()
            .iter()
            .filter_map(|el| match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
                _ => None,
            })
            .collect()
    }

    fn assert_paths_close(a: &BezPath, b: &BezPath) {
        let (pa, pb) = (points(a), points(b));
        assert_eq!(pa.len(), pb.len(), "sample counts differ");
        for (x, y) in pa.iter().zip(pb.iter()) {
            assert!(
                (x.x - y.x).abs() < 1e-9 && (x.y - y.y).abs() < 1e-9,
                "point mismatch: {x:?} vs {y:?}"
            );
        }
    }

    #[test]
    fn morph_endpoints_reproduce_the_source_shapes() {
        let a = RoundedPolygon::circle();
        let b = RoundedPolygon::new(6, 0.3, 0.0);
        assert_paths_close(&morph_path(&a, &b, 0.0, CENTER, R), &a.to_path(CENTER, R));
        assert_paths_close(&morph_path(&a, &b, 1.0, CENTER, R), &b.to_path(CENTER, R));
    }

    #[test]
    fn morph_midpoint_lies_between_the_shapes() {
        // At an edge-midpoint angle a hexagon is smaller than the circle, so
        // the halfway morph radius there sits strictly between the two.
        let circle = RoundedPolygon::circle();
        let hex = RoundedPolygon::new(6, 0.0, 0.0);
        let theta = PI / 6.0; // an edge midpoint of the (unrotated) hexagon
        let rc = circle.unit_radius_at(theta) * R;
        let rh = hex.unit_radius_at(theta) * R;
        assert!(rh < rc, "hexagon apothem is inside the circle");
        // Rebuild the halfway radius the way morph_path does.
        let rm = rc + (rh - rc) * 0.5;
        assert!(rm > rh && rm < rc);
    }

    #[test]
    fn same_shape_morph_is_the_shape_at_every_t() {
        let s = RoundedPolygon::new(5, 0.4, 0.7);
        let base = s.to_path(CENTER, R);
        for t in [0.0, 0.25, 0.5, 0.9, 1.0] {
            assert_paths_close(&morph_path(&s, &s, t, CENTER, R), &base);
        }
    }

    #[test]
    fn zero_size_collapses_every_point_to_the_center() {
        let a = RoundedPolygon::circle();
        let b = RoundedPolygon::new(3, 0.5, 0.0);
        for path in [
            a.to_path(CENTER, 0.0),
            b.to_path(CENTER, 0.0),
            morph_path(&a, &b, 0.5, CENTER, 0.0),
        ] {
            for p in points(&path) {
                assert!((p.x - CENTER.x).abs() < 1e-12 && (p.y - CENTER.y).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn circle_is_rounding_independent_of_sides() {
        // rounding == 1.0 collapses the radial function to a constant, so any
        // side count yields the same circle.
        let c4 = RoundedPolygon::new(4, 1.0, 0.0);
        let c9 = RoundedPolygon::new(9, 1.0, 1.3);
        assert_paths_close(&c4.to_path(CENTER, R), &c9.to_path(CENTER, R));
    }

    #[test]
    fn path_is_closed_with_the_expected_sample_count() {
        let path = RoundedPolygon::new(7, 0.3, 0.0).to_path(CENTER, R);
        assert_eq!(points(&path).len(), SAMPLES);
        assert!(matches!(path.elements().last(), Some(PathEl::ClosePath)));
    }
}
