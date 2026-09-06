//! The `RoundedPolygon` engine: feature-point polygon construction with
//! per-corner rounding and smoothing, its cubic-curve outline, and the
//! [`kurbo::BezPath`] seam.
//!
//! Ported from `material_new_shapes` 1.0.0's `lib/src/shapes/`
//! (`corner_rounding.dart`, `cubic.dart`, `features.dart`, `point.dart`,
//! `rounded_polygon.dart`, `utils.dart`). See the [module docs](super) for
//! attribution, the Dart→Rust name map, and the list of deliberate deviations.

use std::f64::consts::PI;

use kurbo::{Affine, BezPath, Point, Rect, Vec2};

/// Distance below which two points count as the same one.
///
/// Upstream's `distanceEpsilon`: small enough that the round-off it forgives is
/// under a pixel on any reasonable display, large enough that a corner whose
/// rounding collapses to nothing is recognised as a plain vertex.
pub const DISTANCE_EPSILON: f64 = 1e-5;

/// Below this sine of the angle at a vertex, the two sides are treated as
/// collinear and the corner takes no round cut at all (upstream's inline
/// `1e-3` in `_RoundedCorner`).
const COLLINEAR_SIN_EPSILON: f64 = 1e-3;

// ---------------------------------------------------------------------------
// Scalar/vector helpers (utils.dart, point.dart)
// ---------------------------------------------------------------------------

/// Linear interpolation, in upstream's exact form — `start * (1 - fraction) +
/// stop * fraction`, **not** `start + (stop - start) * fraction`, which rounds
/// differently and would drift this port off its goldens.
fn lerp(start: f64, stop: f64, fraction: f64) -> f64 {
    start * (1.0 - fraction) + stop * fraction
}

/// Component-wise [`lerp`] between two points.
fn lerp_point(start: Point, stop: Point, fraction: f64) -> Point {
    Point::new(
        lerp(start.x, stop.x, fraction),
        lerp(start.y, stop.y, fraction),
    )
}

/// The vector rotated a quarter turn counter-clockwise.
fn rotate90(v: Vec2) -> Vec2 {
    Vec2::new(-v.y, v.x)
}

/// The unit vector along `v`.
///
/// # Panics
///
/// Debug-asserts that `v` is non-degenerate; a zero-length vector has no
/// direction and yields NaNs (upstream asserts the same).
fn direction(v: Vec2) -> Vec2 {
    let d = v.hypot();
    debug_assert!(d > 0.0, "cannot take the direction of a zero-length vector");
    v / d
}

/// The point at `radius` from the origin at `angle_radians`.
fn radial_to_cartesian(radius: f64, angle_radians: f64) -> Vec2 {
    Vec2::new(angle_radians.cos(), angle_radians.sin()) * radius
}

/// Whether the corner at `current` turns outward (convex) rather than inward.
///
/// A fast sign-of-cross-product test, not a reliable one for degenerate input —
/// upstream carries the same caveat.
fn convex(previous: Point, current: Point, next: Point) -> bool {
    (current - previous).cross(next - current) > 0.0
}

/// The average of the given points — upstream's `calculateCenter`.
fn calculate_center(vertices: &[Point]) -> Point {
    let n = vertices.len() as f64;
    let sum = vertices.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2());
    (sum / n).to_point()
}

// ---------------------------------------------------------------------------
// CornerRounding (corner_rounding.dart)
// ---------------------------------------------------------------------------

/// How much, and how smoothly, a single vertex is rounded.
///
/// A corner is one of three things:
///
/// 1. **unrounded** — [`CornerRounding::UNROUNDED`], the vertex itself;
/// 2. **circular** — `smoothing == 0`: one arc between the two adjacent edges;
/// 3. **smoothed** — `smoothing > 0`: an inner arc plus two symmetric flanking
///    curves carrying the arc out to the edges. At `smoothing == 1` there is no
///    arc left at all — the flanking curves meet in the middle.
///
/// `radius` is an absolute length in the polygon's own coordinate space, so a
/// shape authored in the canonical `(-1, -1)..(1, 1)` box wants radii relative
/// to that box, not to its eventual on-screen size. Transforming the polygon
/// scales the rounding with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CornerRounding {
    /// Radius of the circle defining the corner's inner arc; `0` is a sharp
    /// corner.
    pub radius: f64,
    /// How far the arc is extended toward the adjacent edges, in `0..=1`.
    pub smoothing: f64,
}

impl CornerRounding {
    /// A sharp corner: the vertex itself, no arc.
    pub const UNROUNDED: CornerRounding = CornerRounding {
        radius: 0.0,
        smoothing: 0.0,
    };

    /// A corner rounded by a circular arc of `radius`, with no smoothing.
    ///
    /// # Panics
    ///
    /// If `radius` is negative.
    #[must_use]
    pub fn radius(radius: f64) -> CornerRounding {
        CornerRounding::new(radius, 0.0)
    }

    /// A corner rounded by an arc of `radius`, smoothed by `smoothing`.
    ///
    /// # Panics
    ///
    /// If `radius` is negative or `smoothing` is outside `0..=1`.
    #[must_use]
    pub fn new(radius: f64, smoothing: f64) -> CornerRounding {
        assert!(radius >= 0.0, "corner radius must be >= 0, got {radius}");
        assert!(
            (0.0..=1.0).contains(&smoothing),
            "corner smoothing must be in 0..=1, got {smoothing}"
        );
        CornerRounding { radius, smoothing }
    }
}

impl Default for CornerRounding {
    fn default() -> Self {
        CornerRounding::UNROUNDED
    }
}

// ---------------------------------------------------------------------------
// Cubic (cubic.dart)
// ---------------------------------------------------------------------------

/// One cubic Bézier segment: two anchor points with two control points between
/// them.
///
/// Stored as the same flat eight-float array upstream uses
/// (`[a0x, a0y, c0x, c0y, c1x, c1y, a1x, a1y]`), which is also the layout a
/// morph's element-wise interpolation wants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cubic {
    points: [f64; 8],
}

impl Cubic {
    /// A cubic from its four points, in path order.
    #[must_use]
    pub const fn new(anchor0: Point, control0: Point, control1: Point, anchor1: Point) -> Cubic {
        Cubic {
            points: [
                anchor0.x, anchor0.y, control0.x, control0.y, control1.x, control1.y, anchor1.x,
                anchor1.y,
            ],
        }
    }

    /// A cubic from the raw `[a0x, a0y, c0x, c0y, c1x, c1y, a1x, a1y]` layout.
    #[must_use]
    pub const fn from_raw(points: [f64; 8]) -> Cubic {
        Cubic { points }
    }

    /// The raw `[a0x, a0y, c0x, c0y, c1x, c1y, a1x, a1y]` layout.
    #[must_use]
    pub const fn points(&self) -> &[f64; 8] {
        &self.points
    }

    /// A straight line between the anchors, with the controls placed a third
    /// and two thirds of the way along it.
    #[must_use]
    pub fn straight_line(p0: Point, p1: Point) -> Cubic {
        Cubic::new(
            p0,
            lerp_point(p0, p1, 1.0 / 3.0),
            lerp_point(p0, p1, 2.0 / 3.0),
            p1,
        )
    }

    /// A zero-length cubic at `p`.
    #[must_use]
    pub fn empty(p: Point) -> Cubic {
        Cubic::new(p, p, p, p)
    }

    /// A cubic approximating the circular arc from `p0` to `p1` around
    /// `center`, taking the shorter of the two ways round.
    ///
    /// `p0` and `p1` are expected to be equidistant from `center`; arcs beyond
    /// 180° need more than one segment. Near-coincident endpoints degrade to a
    /// straight line.
    #[must_use]
    pub fn circular_arc(center: Point, p0: Point, p1: Point) -> Cubic {
        let p0d = direction(p0 - center);
        let p1d = direction(p1 - center);
        let rotated_p0 = rotate90(p0d);
        let rotated_p1 = rotate90(p1d);
        let clockwise = rotated_p0.dot(p1 - center) >= 0.0;
        let cosa = p0d.dot(p1d);

        // p0 ~= p1: no meaningful arc left to approximate.
        if cosa > 0.999 {
            return Cubic::straight_line(p0, p1);
        }

        let k = (p0 - center).hypot() * 4.0 / 3.0
            * ((2.0 * (1.0 - cosa)).sqrt() - (1.0 - cosa * cosa).sqrt())
            / (1.0 - cosa)
            * if clockwise { 1.0 } else { -1.0 };

        Cubic::new(p0, p0 + rotated_p0 * k, p1 - rotated_p1 * k, p1)
    }

    /// The curve's starting anchor.
    #[must_use]
    pub const fn anchor0(&self) -> Point {
        Point::new(self.points[0], self.points[1])
    }

    /// The control point governing the slope at [`Cubic::anchor0`].
    #[must_use]
    pub const fn control0(&self) -> Point {
        Point::new(self.points[2], self.points[3])
    }

    /// The control point governing the slope at [`Cubic::anchor1`].
    #[must_use]
    pub const fn control1(&self) -> Point {
        Point::new(self.points[4], self.points[5])
    }

    /// The curve's ending anchor.
    #[must_use]
    pub const fn anchor1(&self) -> Point {
        Point::new(self.points[6], self.points[7])
    }

    /// The point at parameter `t`, where `0` is [`Cubic::anchor0`] and `1` is
    /// [`Cubic::anchor1`].
    #[must_use]
    pub fn point_on_curve(&self, t: f64) -> Point {
        let u = 1.0 - t;
        let (a0, c0, c1, a1) = (
            self.anchor0(),
            self.control0(),
            self.control1(),
            self.anchor1(),
        );
        Point::new(
            a0.x * (u * u * u)
                + c0.x * (3.0 * t * u * u)
                + c1.x * (3.0 * t * t * u)
                + a1.x * (t * t * t),
            a0.y * (u * u * u)
                + c0.y * (3.0 * t * u * u)
                + c1.y * (3.0 * t * t * u)
                + a1.y * (t * t * t),
        )
    }

    /// Whether both anchors sit on the same point, within
    /// [`DISTANCE_EPSILON`]. Such curves paint nothing and are dropped from a
    /// polygon's cubic list.
    #[must_use]
    pub fn zero_length(&self) -> bool {
        (self.points[0] - self.points[6]).abs() < DISTANCE_EPSILON
            && (self.points[1] - self.points[7]).abs() < DISTANCE_EPSILON
    }

    /// Splits the curve at parameter `t` into two curves covering the same
    /// path.
    #[must_use]
    pub fn split(&self, t: f64) -> (Cubic, Cubic) {
        let u = 1.0 - t;
        let point = self.point_on_curve(t);
        let (a0, c0, c1, a1) = (
            self.anchor0(),
            self.control0(),
            self.control1(),
            self.anchor1(),
        );

        (
            Cubic::new(
                a0,
                Point::new(a0.x * u + c0.x * t, a0.y * u + c0.y * t),
                Point::new(
                    a0.x * (u * u) + c0.x * (2.0 * u * t) + c1.x * (t * t),
                    a0.y * (u * u) + c0.y * (2.0 * u * t) + c1.y * (t * t),
                ),
                point,
            ),
            Cubic::new(
                point,
                Point::new(
                    c0.x * (u * u) + c1.x * (2.0 * u * t) + a1.x * (t * t),
                    c0.y * (u * u) + c1.y * (2.0 * u * t) + a1.y * (t * t),
                ),
                Point::new(c1.x * u + a1.x * t, c1.y * u + a1.y * t),
                a1,
            ),
        )
    }

    /// The same curve traversed in the opposite direction.
    #[must_use]
    pub fn reversed(&self) -> Cubic {
        Cubic::new(
            self.anchor1(),
            self.control1(),
            self.control0(),
            self.anchor0(),
        )
    }

    /// This curve with every anchor and control point mapped through `f`.
    #[must_use]
    pub fn transformed_with(&self, f: impl Fn(Point) -> Point) -> Cubic {
        Cubic::new(
            f(self.anchor0()),
            f(self.control0()),
            f(self.control1()),
            f(self.anchor1()),
        )
    }

    /// The bounding box of the four defining points — cheap, and never smaller
    /// than [`Cubic::exact_bounds`].
    #[must_use]
    pub fn approximate_bounds(&self) -> Rect {
        let xs = [
            self.points[0],
            self.points[2],
            self.points[4],
            self.points[6],
        ];
        let ys = [
            self.points[1],
            self.points[3],
            self.points[5],
            self.points[7],
        ];
        Rect::new(
            xs.iter().copied().fold(f64::INFINITY, f64::min),
            ys.iter().copied().fold(f64::INFINITY, f64::min),
            xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        )
    }

    /// The true bounding box of the curve, solving the derivative for its
    /// extrema on each axis.
    #[must_use]
    pub fn exact_bounds(&self) -> Rect {
        // A zero-length curve is just its own anchor point.
        if self.zero_length() {
            let p = self.anchor0();
            return Rect::new(p.x, p.y, p.x, p.y);
        }

        let (a0, c0, c1, a1) = (
            self.anchor0(),
            self.control0(),
            self.control1(),
            self.anchor1(),
        );
        let mut min = Point::new(a0.x.min(a1.x), a0.y.min(a1.y));
        let mut max = Point::new(a0.x.max(a1.x), a0.y.max(a1.y));

        // The derivative of a cubic is a quadratic; solve it per axis and fold
        // in whichever roots land inside 0..=1.
        for axis_x in [true, false] {
            let (p0, p1, p2, p3) = if axis_x {
                (a0.x, c0.x, c1.x, a1.x)
            } else {
                (a0.y, c0.y, c1.y, a1.y)
            };
            let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
            let b = 2.0 * p0 - 4.0 * p1 + 2.0 * p2;
            let c = -p0 + p1;

            let mut roots = [None, None];
            if a.abs() < DISTANCE_EPSILON {
                // Muller's method: with a ~ 0 there is at most a single root.
                if b != 0.0 {
                    roots[0] = Some(2.0 * c / (-2.0 * b));
                }
            } else {
                let s = b * b - 4.0 * a * c;
                if s >= 0.0 {
                    roots[0] = Some((-b + s.sqrt()) / (2.0 * a));
                    roots[1] = Some((-b - s.sqrt()) / (2.0 * a));
                }
            }

            for t in roots.into_iter().flatten() {
                if !(0.0..=1.0).contains(&t) {
                    continue;
                }
                let p = self.point_on_curve(t);
                if axis_x {
                    min.x = min.x.min(p.x);
                    max.x = max.x.max(p.x);
                } else {
                    min.y = min.y.min(p.y);
                    max.y = max.y.max(p.y);
                }
            }
        }

        Rect::new(min.x, min.y, max.x, max.y)
    }
}

// ---------------------------------------------------------------------------
// Feature (features.dart)
// ---------------------------------------------------------------------------

/// What a [`Feature`] contributes to the outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureKind {
    /// A stretch of outline between two corners, with no indentation of its
    /// own. Edges are ignored by the default morph mapping.
    Edge,
    /// A corner indenting outward.
    ConvexCorner,
    /// A corner indenting inward — a star's inner points, say.
    ConcaveCorner,
}

/// A run of contiguous cubics grouped into one semantic unit of the outline.
///
/// Features are what a morph matches on. A rounded rectangle's outline is a
/// dozen-odd cubics but only four corners plus four edges, and matching
/// corner-to-corner (convex to convex, concave to concave) is what keeps a
/// shape transition legible. Features built as
/// [`ignorable`](Feature::ignorable) — and every [`Edge`](FeatureKind::Edge) —
/// sit out that matching.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    cubics: Vec<Cubic>,
    kind: FeatureKind,
}

impl Feature {
    /// A single-cubic edge.
    #[must_use]
    pub fn edge(cubic: Cubic) -> Feature {
        Feature {
            cubics: vec![cubic],
            kind: FeatureKind::Edge,
        }
    }

    /// A feature of any indentation that the default morph mapping skips.
    ///
    /// Useful for steering a morph: marking a 12-pointed star's concave
    /// corners ignorable makes a morph consider only its outer points, which
    /// can cost the animation a whole rotation's worth of intersections.
    ///
    /// # Panics
    ///
    /// If `cubics` is empty or its curves are not contiguous.
    #[must_use]
    pub fn ignorable(cubics: Vec<Cubic>) -> Feature {
        Feature::validated(cubics, FeatureKind::Edge)
    }

    /// A corner indenting outward.
    ///
    /// # Panics
    ///
    /// If `cubics` is empty or its curves are not contiguous.
    #[must_use]
    pub fn convex_corner(cubics: Vec<Cubic>) -> Feature {
        Feature::validated(cubics, FeatureKind::ConvexCorner)
    }

    /// A corner indenting inward.
    ///
    /// # Panics
    ///
    /// If `cubics` is empty or its curves are not contiguous.
    #[must_use]
    pub fn concave_corner(cubics: Vec<Cubic>) -> Feature {
        Feature::validated(cubics, FeatureKind::ConcaveCorner)
    }

    fn validated(cubics: Vec<Cubic>, kind: FeatureKind) -> Feature {
        assert!(!cubics.is_empty(), "a feature needs at least one cubic");
        for pair in cubics.windows(2) {
            let (prev, next) = (pair[0].anchor1(), pair[1].anchor0());
            assert!(
                (next.x - prev.x).abs() <= DISTANCE_EPSILON
                    && (next.y - prev.y).abs() <= DISTANCE_EPSILON,
                "a feature must be continuous: {next:?} does not meet {prev:?}"
            );
        }
        Feature { cubics, kind }
    }

    /// Builds a feature without checking continuity — for the polygon builder,
    /// which produces contiguous runs by construction.
    fn unchecked(cubics: Vec<Cubic>, kind: FeatureKind) -> Feature {
        Feature { cubics, kind }
    }

    /// The curves making up this feature, in path order.
    #[must_use]
    pub fn cubics(&self) -> &[Cubic] {
        &self.cubics
    }

    /// What this feature contributes to the outline.
    #[must_use]
    pub const fn kind(&self) -> FeatureKind {
        self.kind
    }

    /// Whether the default morph mapping skips this feature.
    #[must_use]
    pub const fn is_ignorable(&self) -> bool {
        matches!(self.kind, FeatureKind::Edge)
    }

    /// Whether this feature is an edge.
    #[must_use]
    pub const fn is_edge(&self) -> bool {
        matches!(self.kind, FeatureKind::Edge)
    }

    /// Whether this feature is a corner of either indentation.
    #[must_use]
    pub const fn is_corner(&self) -> bool {
        !self.is_edge()
    }

    /// Whether this feature is an outward corner.
    #[must_use]
    pub const fn is_convex_corner(&self) -> bool {
        matches!(self.kind, FeatureKind::ConvexCorner)
    }

    /// Whether this feature is an inward corner.
    #[must_use]
    pub const fn is_concave_corner(&self) -> bool {
        matches!(self.kind, FeatureKind::ConcaveCorner)
    }

    /// This feature with every point mapped through `f`.
    #[must_use]
    pub fn transformed_with(&self, f: impl Fn(Point) -> Point + Copy) -> Feature {
        Feature {
            cubics: self.cubics.iter().map(|c| c.transformed_with(f)).collect(),
            kind: self.kind,
        }
    }

    /// This feature traversed in the opposite direction.
    ///
    /// A corner's indentation flips with the traversal, since a polygon's
    /// convexity flag is orientation-dependent (upstream carries the same
    /// negation, and the same note that it is really a property of the
    /// polygon's winding).
    #[must_use]
    pub fn reversed(&self) -> Feature {
        Feature {
            cubics: self.cubics.iter().rev().map(Cubic::reversed).collect(),
            kind: match self.kind {
                FeatureKind::Edge => FeatureKind::Edge,
                FeatureKind::ConvexCorner => FeatureKind::ConcaveCorner,
                FeatureKind::ConcaveCorner => FeatureKind::ConvexCorner,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// RoundedPolygon (rounded_polygon.dart)
// ---------------------------------------------------------------------------

/// Optional arguments to [`RoundedPolygon::star`], defaulted as upstream
/// defaults them.
#[derive(Clone, Copy, Debug)]
pub struct StarParams<'a> {
    /// How many vertices lie on each of the two radii.
    pub num_vertices_per_radius: usize,
    /// The outer radius; must be `> 0`.
    pub radius: f64,
    /// The inner radius; must be `> 0` and `< radius`.
    pub inner_radius: f64,
    /// Rounding applied to every vertex `per_vertex_rounding` does not cover.
    pub rounding: CornerRounding,
    /// Rounding for the inner vertices only. Ignored when
    /// `per_vertex_rounding` is set.
    pub inner_rounding: Option<CornerRounding>,
    /// Per-vertex rounding; must hold `2 * num_vertices_per_radius` entries.
    pub per_vertex_rounding: Option<&'a [CornerRounding]>,
    /// The centre all vertices are placed around.
    pub center: Point,
}

impl Default for StarParams<'_> {
    fn default() -> Self {
        StarParams {
            num_vertices_per_radius: 5,
            radius: 1.0,
            inner_radius: 0.5,
            rounding: CornerRounding::UNROUNDED,
            inner_rounding: None,
            per_vertex_rounding: None,
            center: Point::ZERO,
        }
    }
}

/// Optional arguments to [`RoundedPolygon::pill_star`], defaulted as upstream
/// defaults them.
#[derive(Clone, Copy, Debug)]
pub struct PillStarParams<'a> {
    /// Width of the underlying pill; must be `> 0`.
    pub width: f64,
    /// Height of the underlying pill; must be `> 0`.
    pub height: f64,
    /// How many vertices lie on each of the two radii.
    pub num_vertices_per_radius: usize,
    /// Inner radius as a fraction of the outer one, in `(0, 1]`.
    pub inner_radius_ratio: f64,
    /// Rounding applied to every vertex `per_vertex_rounding` does not cover.
    pub rounding: CornerRounding,
    /// Rounding for the inner vertices only. Ignored when
    /// `per_vertex_rounding` is set.
    pub inner_rounding: Option<CornerRounding>,
    /// Per-vertex rounding; must hold `2 * num_vertices_per_radius` entries.
    pub per_vertex_rounding: Option<&'a [CornerRounding]>,
    /// How the vertices on the curved end caps are spaced, in `0..=1`: `0`
    /// spaces the inner vertices like those on the straight edges (pushing the
    /// outer ones apart), `1` does the opposite, and the default `0.5` splits
    /// the difference.
    pub vertex_spacing: f64,
    /// Where on the perimeter the outline starts, in `0..=1`. Only visible to
    /// a caller stroking the path progressively.
    pub start_location: f64,
    /// The centre all vertices are placed around.
    pub center: Point,
}

impl Default for PillStarParams<'_> {
    fn default() -> Self {
        PillStarParams {
            width: 2.0,
            height: 1.0,
            num_vertices_per_radius: 8,
            inner_radius_ratio: 0.5,
            rounding: CornerRounding::UNROUNDED,
            inner_rounding: None,
            per_vertex_rounding: None,
            vertex_spacing: 0.5,
            start_location: 0.0,
            center: Point::ZERO,
        }
    }
}

/// A closed polygonal outline with optional per-corner rounding.
///
/// Built from vertices (supplied, or generated on a circle/rectangle/star/pill
/// outline) plus a [`CornerRounding`] per vertex, and carried around as both a
/// [`Feature`] list and the flat [`Cubic`] list those features flatten to. Paint
/// it with [`RoundedPolygon::to_path`].
///
/// Nothing here is in screen units: a polygon built with the default radius of
/// `1` lives in the `(-1, -1)..(1, 1)` box and wants
/// [`transformed`](RoundedPolygon::transformed) (or
/// [`normalized`](RoundedPolygon::normalized) plus a scale) before it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct RoundedPolygon {
    features: Vec<Feature>,
    center: Point,
    cubics: Vec<Cubic>,
}

impl RoundedPolygon {
    /// Builds a polygon from features that already describe a closed outline.
    ///
    /// Specifying features directly is what gives a caller control over how
    /// cubics are grouped, which is what a [`Feature`]-matched morph maps on.
    /// With `center` `None` the centre is the average of every cubic's starting
    /// anchor.
    ///
    /// # Panics
    ///
    /// If fewer than two features are given. Debug builds additionally assert
    /// that the resulting outline is contiguous (upstream asserts the same,
    /// also debug-only).
    #[must_use]
    pub fn from_features(features: Vec<Feature>, center: Option<Point>) -> RoundedPolygon {
        assert!(
            features.len() >= 2,
            "a polygon needs at least 2 features, got {}",
            features.len()
        );

        let center = center.unwrap_or_else(|| {
            let anchors: Vec<Point> = features
                .iter()
                .flat_map(|f| f.cubics().iter().map(Cubic::anchor0))
                .collect();
            calculate_center(&anchors)
        });

        let cubics = init_cubics(&features, center);

        debug_assert!(
            is_contiguous(&cubics),
            "a polygon must be contiguous: every cubic's start anchor must meet \
             the previous cubic's end anchor"
        );

        RoundedPolygon {
            features,
            center,
            cubics,
        }
    }

    /// Builds a polygon from an ordered vertex list, rounding each vertex as
    /// asked.
    ///
    /// The outline runs from each vertex to the next in list order; an
    /// unordered list gives undefined results. `per_vertex_rounding`, when
    /// given, overrides `rounding` for every vertex and must have one entry per
    /// vertex. With `center` `None` the centre is the average of the vertices.
    ///
    /// # Panics
    ///
    /// If fewer than three vertices are given, or `per_vertex_rounding` has a
    /// different length than `vertices`.
    #[must_use]
    pub fn from_vertices(
        vertices: &[Point],
        rounding: CornerRounding,
        per_vertex_rounding: Option<&[CornerRounding]>,
        center: Option<Point>,
    ) -> RoundedPolygon {
        let n = vertices.len();
        assert!(n >= 3, "a polygon needs at least 3 vertices, got {n}");
        if let Some(pvr) = per_vertex_rounding {
            assert!(
                pvr.len() == n,
                "per-vertex rounding must have one entry per vertex ({n}), got {}",
                pvr.len()
            );
        }

        let corners: Vec<RoundedCorner> = (0..n)
            .map(|i| {
                RoundedCorner::new(
                    vertices[(i + n - 1) % n],
                    vertices[i],
                    vertices[(i + 1) % n],
                    per_vertex_rounding.map_or(rounding, |pvr| pvr[i]),
                )
            })
            .collect();

        // For each side, work out how much of the cut its two corners asked for
        // actually fits. Rounding is served first for both corners; only what
        // is left over goes to smoothing. Each entry is (how much of the round
        // cut fits, how much of the smoothing cut fits), for the side running
        // from corner i to corner i + 1.
        let cut_adjusts: Vec<(f64, f64)> = (0..n)
            .map(|i| {
                let next = (i + 1) % n;
                let expected_round_cut =
                    corners[i].expected_round_cut + corners[next].expected_round_cut;
                let expected_cut = corners[i].expected_cut() + corners[next].expected_cut();
                let side_size = (vertices[i] - vertices[next]).hypot();

                if expected_round_cut > side_size {
                    // Not even the rounding fits; scale it down and drop
                    // smoothing entirely.
                    (side_size / expected_round_cut, 0.0)
                } else if expected_cut > side_size {
                    // Rounding fits, smoothing only partly.
                    (
                        1.0,
                        (side_size - expected_round_cut) / (expected_cut - expected_round_cut),
                    )
                } else {
                    (1.0, 1.0)
                }
            })
            .collect();

        let corner_cubics: Vec<Vec<Cubic>> = (0..n)
            .map(|i| {
                // allowed_cuts[0] covers the side coming in from the previous
                // corner, allowed_cuts[1] the side going out to the next one.
                let mut allowed_cuts = [0.0_f64; 2];
                for (delta, cut) in allowed_cuts.iter_mut().enumerate() {
                    let (round_cut_ratio, cut_ratio) = cut_adjusts[(i + n - 1 + delta) % n];
                    *cut = corners[i].expected_round_cut * round_cut_ratio
                        + (corners[i].expected_cut() - corners[i].expected_round_cut) * cut_ratio;
                }
                corners[i].cubics(allowed_cuts[0], allowed_cuts[1])
            })
            .collect();

        // Corner curves plus the straight edges joining them, in outline order.
        let mut features = Vec::with_capacity(2 * n);
        for i in 0..n {
            let next = (i + 1) % n;
            let cvx = convex(vertices[(i + n - 1) % n], vertices[i], vertices[next]);
            let edge = Cubic::straight_line(
                corner_cubics[i]
                    .last()
                    .expect("a corner always yields at least one cubic")
                    .anchor1(),
                corner_cubics[next]
                    .first()
                    .expect("a corner always yields at least one cubic")
                    .anchor0(),
            );
            features.push(Feature::unchecked(
                corner_cubics[i].clone(),
                if cvx {
                    FeatureKind::ConvexCorner
                } else {
                    FeatureKind::ConcaveCorner
                },
            ));
            features.push(Feature::unchecked(vec![edge], FeatureKind::Edge));
        }

        let center = center.unwrap_or_else(|| calculate_center(vertices));
        RoundedPolygon::from_features(features, Some(center))
    }

    /// Builds a regular polygon: `num_vertices` vertices equally spaced on a
    /// circle of `radius` around `center`.
    ///
    /// # Panics
    ///
    /// If `num_vertices` is below 3, or `per_vertex_rounding` has a different
    /// length than the vertex count.
    #[must_use]
    pub fn from_num_vertices(
        num_vertices: usize,
        radius: f64,
        center: Point,
        rounding: CornerRounding,
        per_vertex_rounding: Option<&[CornerRounding]>,
    ) -> RoundedPolygon {
        assert!(
            num_vertices >= 3,
            "a polygon needs at least 3 vertices, got {num_vertices}"
        );
        let vertices = vertices_from_num_vertices(num_vertices, radius, center);
        RoundedPolygon::from_vertices(&vertices, rounding, per_vertex_rounding, Some(center))
    }

    /// Approximates a circle of `radius` around `center` with a rounded
    /// `num_vertices`-gon whose rounding consumes its every side.
    ///
    /// # Panics
    ///
    /// If `num_vertices` is below 3.
    #[must_use]
    pub fn circle(num_vertices: usize, radius: f64, center: Point) -> RoundedPolygon {
        assert!(
            num_vertices >= 3,
            "a circle needs at least 3 vertices, got {num_vertices}"
        );
        // Half the angle between adjacent vertices; the underlying polygon's
        // circumradius has to exceed the circle's by that angle's cosine for
        // its rounded sides to land on the circle.
        let theta = PI / num_vertices as f64;
        let polygon_radius = radius / theta.cos();
        RoundedPolygon::from_num_vertices(
            num_vertices,
            polygon_radius,
            center,
            CornerRounding::radius(radius),
            None,
        )
    }

    /// Builds a `width` × `height` rectangle around `center`, optionally
    /// rounded.
    ///
    /// Vertices run bottom-right, bottom-left, top-left, top-right, which is
    /// the order `per_vertex_rounding`'s four entries are read in.
    ///
    /// # Panics
    ///
    /// If `per_vertex_rounding` is given and does not hold exactly four
    /// entries.
    #[must_use]
    pub fn rectangle(
        width: f64,
        height: f64,
        rounding: CornerRounding,
        per_vertex_rounding: Option<&[CornerRounding]>,
        center: Point,
    ) -> RoundedPolygon {
        let (left, right) = (center.x - width / 2.0, center.x + width / 2.0);
        let (top, bottom) = (center.y - height / 2.0, center.y + height / 2.0);
        let vertices = [
            Point::new(right, bottom),
            Point::new(left, bottom),
            Point::new(left, top),
            Point::new(right, top),
        ];
        RoundedPolygon::from_vertices(&vertices, rounding, per_vertex_rounding, Some(center))
    }

    /// Builds a star: a polygon whose vertices alternate between an outer and
    /// an inner radius.
    ///
    /// Equal radii would give a regular polygon with twice
    /// `num_vertices_per_radius` vertices — build that with
    /// [`RoundedPolygon::from_num_vertices`] instead.
    ///
    /// # Panics
    ///
    /// If either radius is not positive, if the inner radius is not smaller
    /// than the outer one, or if `per_vertex_rounding` does not hold
    /// `2 * num_vertices_per_radius` entries.
    #[must_use]
    pub fn star(params: StarParams<'_>) -> RoundedPolygon {
        let StarParams {
            num_vertices_per_radius,
            radius,
            inner_radius,
            rounding,
            inner_rounding,
            per_vertex_rounding,
            center,
        } = params;
        assert!(
            radius > 0.0 && inner_radius > 0.0,
            "star radii must both be greater than 0, got {radius} and {inner_radius}"
        );
        assert!(
            inner_radius < radius,
            "a star's inner radius ({inner_radius}) must be less than its outer one ({radius})"
        );

        let alternating = alternating_rounding(
            num_vertices_per_radius,
            rounding,
            inner_rounding,
            per_vertex_rounding,
        );
        let vertices =
            star_vertices_from_num_vertices(num_vertices_per_radius, radius, inner_radius, center);
        RoundedPolygon::from_vertices(
            &vertices,
            rounding,
            alternating.as_deref().or(per_vertex_rounding),
            Some(center),
        )
    }

    /// Builds a pill: a rectangle capped by a semicircle at each end of its
    /// longer axis.
    ///
    /// `smoothing` extends each end cap's arc toward the straight edges the way
    /// [`CornerRounding::smoothing`] does.
    ///
    /// # Panics
    ///
    /// If `width` or `height` is not positive.
    #[must_use]
    pub fn pill(width: f64, height: f64, smoothing: f64, center: Point) -> RoundedPolygon {
        assert!(
            width > 0.0 && height > 0.0,
            "a pill needs a positive width and height, got {width} and {height}"
        );
        let (w_half, h_half) = (width / 2.0, height / 2.0);
        let vertices = [
            Point::new(center.x + w_half, center.y + h_half),
            Point::new(center.x - w_half, center.y + h_half),
            Point::new(center.x - w_half, center.y - h_half),
            Point::new(center.x + w_half, center.y - h_half),
        ];
        RoundedPolygon::from_vertices(
            &vertices,
            CornerRounding::new(w_half.min(h_half), smoothing),
            None,
            Some(center),
        )
    }

    /// Builds a pill-star: a [`pill`](RoundedPolygon::pill) whose vertices
    /// alternate between an outer and an inner radius, the way a
    /// [`star`](RoundedPolygon::star)'s do around a circle.
    ///
    /// Vertices are spaced evenly along the underlying pill's perimeter, which
    /// leaves a choice on the curved end caps: outer vertices sitting on the
    /// outline crowd the inner ones together, and vice versa. That is what
    /// [`PillStarParams::vertex_spacing`] picks between.
    ///
    /// # Panics
    ///
    /// If `width` or `height` is not positive, if `inner_radius_ratio` is
    /// outside `(0, 1]`, if `vertex_spacing` or `start_location` is outside
    /// `0..=1`, or if `per_vertex_rounding` does not hold
    /// `2 * num_vertices_per_radius` entries.
    #[must_use]
    pub fn pill_star(params: PillStarParams<'_>) -> RoundedPolygon {
        let PillStarParams {
            width,
            height,
            num_vertices_per_radius,
            inner_radius_ratio,
            rounding,
            inner_rounding,
            per_vertex_rounding,
            vertex_spacing,
            start_location,
            center,
        } = params;
        assert!(
            width > 0.0 && height > 0.0,
            "a pill star needs a positive width and height, got {width} and {height}"
        );
        assert!(
            inner_radius_ratio > 0.0 && inner_radius_ratio <= 1.0,
            "a pill star's inner radius ratio must be in (0, 1], got {inner_radius_ratio}"
        );
        assert!(
            (0.0..=1.0).contains(&vertex_spacing),
            "vertex spacing must be in 0..=1, got {vertex_spacing}"
        );
        assert!(
            (0.0..=1.0).contains(&start_location),
            "start location must be in 0..=1, got {start_location}"
        );

        let alternating = alternating_rounding(
            num_vertices_per_radius,
            rounding,
            inner_rounding,
            per_vertex_rounding,
        );
        let vertices = pill_star_vertices_from_num_vertices(
            num_vertices_per_radius,
            width,
            height,
            inner_radius_ratio,
            vertex_spacing,
            start_location,
            center,
        );
        RoundedPolygon::from_vertices(
            &vertices,
            rounding,
            alternating.as_deref().or(per_vertex_rounding),
            Some(center),
        )
    }

    /// The features describing this outline, in path order.
    #[must_use]
    pub fn features(&self) -> &[Feature] {
        &self.features
    }

    /// The features flattened into one contiguous, closed run of curves — what
    /// gets painted.
    #[must_use]
    pub fn cubics(&self) -> &[Cubic] {
        &self.cubics
    }

    /// The centre all of this polygon's vertices were placed around.
    #[must_use]
    pub const fn center(&self) -> Point {
        self.center
    }

    /// This polygon with every point mapped through `f`.
    ///
    /// A low-level seam; reach for [`RoundedPolygon::transformed`] for the
    /// affine cases.
    #[must_use]
    pub fn transformed_with(&self, f: impl Fn(Point) -> Point + Copy) -> RoundedPolygon {
        RoundedPolygon::from_features(
            self.features
                .iter()
                .map(|feat| feat.transformed_with(f))
                .collect(),
            Some(f(self.center)),
        )
    }

    /// This polygon transformed by an affine map.
    #[must_use]
    pub fn transformed(&self, transform: Affine) -> RoundedPolygon {
        self.transformed_with(|p| transform * p)
    }

    /// This polygon moved and scaled to sit inside the `(0, 0)..(1, 1)` box,
    /// centred along whichever axis has room to spare.
    #[must_use]
    pub fn normalized(&self) -> RoundedPolygon {
        let bounds = self.bounds();
        let (width, height) = (bounds.width(), bounds.height());
        let side = width.max(height);

        // Centre the shape along the shorter axis.
        let offset_x = (side - width) / 2.0 - bounds.x0;
        let offset_y = (side - height) / 2.0 - bounds.y0;

        self.transformed_with(|p| Point::new((p.x + offset_x) / side, (p.y + offset_y) / side))
    }

    /// The axis-aligned bounds of every anchor and control point — cheap, and
    /// never tighter than [`RoundedPolygon::exact_bounds`].
    #[must_use]
    pub fn bounds(&self) -> Rect {
        self.fold_bounds(Cubic::approximate_bounds)
    }

    /// The true axis-aligned bounds of the outline, solving each curve for its
    /// extrema.
    #[must_use]
    pub fn exact_bounds(&self) -> Rect {
        self.fold_bounds(Cubic::exact_bounds)
    }

    /// The smallest square centred on [`RoundedPolygon::center`] that holds
    /// this shape in *any* rotation — the size a UI element hosting a spinning
    /// shape needs to reserve.
    ///
    /// Measured from the centre to each curve's start and midpoint, so it is an
    /// approximation, matching upstream.
    #[must_use]
    pub fn max_bounds(&self) -> Rect {
        let mut max_dist_squared = 0.0_f64;
        for cubic in &self.cubics {
            let anchor = (cubic.anchor0() - self.center).hypot2();
            let middle = (cubic.point_on_curve(0.5) - self.center).hypot2();
            max_dist_squared = max_dist_squared.max(anchor.max(middle));
        }
        let distance = max_dist_squared.sqrt();
        Rect::new(
            self.center.x - distance,
            self.center.y - distance,
            self.center.x + distance,
            self.center.y + distance,
        )
    }

    fn fold_bounds(&self, per_cubic: impl Fn(&Cubic) -> Rect) -> Rect {
        let mut min = Point::new(f64::INFINITY, f64::INFINITY);
        let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for cubic in &self.cubics {
            let b = per_cubic(cubic);
            min.x = min.x.min(b.x0);
            min.y = min.y.min(b.y0);
            max.x = max.x.max(b.x1);
            max.y = max.y.max(b.y1);
        }
        Rect::new(min.x, min.y, max.x, max.y)
    }

    /// This outline as a closed [`kurbo::BezPath`].
    ///
    /// The path is one `MoveTo`, one `CurveTo` per cubic, and a `ClosePath`.
    /// The final curve's end point is bit-identical to the `MoveTo` point (see
    /// `init_cubics`) — a shape whose ends miss each other by a fraction of a
    /// pixel shows seam artifacts when filled.
    #[must_use]
    pub fn to_path(&self) -> BezPath {
        let mut path = BezPath::new();
        for (i, cubic) in self.cubics.iter().enumerate() {
            if i == 0 {
                path.move_to(cubic.anchor0());
            }
            path.curve_to(cubic.control0(), cubic.control1(), cubic.anchor1());
        }
        if !self.cubics.is_empty() {
            path.close_path();
        }
        path
    }
}

/// Flattens features into the polygon's contiguous cubic list.
///
/// Two things happen here beyond concatenation. Zero-length curves are dropped
/// — they paint nothing and can trigger rendering artifacts — but their end
/// anchor is folded into the previous curve, so a run of several near-zero
/// curves cannot accumulate into a visible discontinuity. And the outline is
/// rotated to start halfway through the first feature when that feature is a
/// three-curve (smoothed) corner, so the closing curve can be rebuilt to land
/// *exactly* on the starting anchor rather than merely near it.
fn init_cubics(features: &[Feature], center: Point) -> Vec<Cubic> {
    let mut cubics: Vec<Cubic> = Vec::new();
    let mut first_cubic: Option<Cubic> = None;
    let mut last_cubic: Option<Cubic> = None;
    let mut split_start: Option<[Cubic; 2]> = None;
    let mut split_end: Option<[Cubic; 2]> = None;

    if let Some(first) = features.first()
        && first.cubics().len() == 3
    {
        let (start, end) = first.cubics()[1].split(0.5);
        split_start = Some([first.cubics()[0], start]);
        split_end = Some([end, first.cubics()[2]]);
    }

    // One past the feature count, so the leading half of a split first feature
    // can be appended at the end.
    for i in 0..=features.len() {
        let feature_cubics: &[Cubic] = if i == features.len() {
            match split_start.as_ref() {
                Some(s) => s.as_slice(),
                None => break,
            }
        } else if i == 0 {
            match split_end.as_ref() {
                Some(s) => s.as_slice(),
                None => features[0].cubics(),
            }
        } else {
            features[i].cubics()
        };

        for cubic in feature_cubics {
            if cubic.zero_length() {
                if let Some(last) = last_cubic.as_mut() {
                    // Keep the latest anchor so dropped curves cannot add up to
                    // a discontinuity.
                    last.points[6] = cubic.points[6];
                    last.points[7] = cubic.points[7];
                }
            } else {
                if let Some(last) = last_cubic {
                    cubics.push(last);
                }
                last_cubic = Some(*cubic);
                first_cubic.get_or_insert(*cubic);
            }
        }
    }

    match (last_cubic, first_cubic) {
        (Some(last), Some(first)) => cubics.push(Cubic::new(
            last.anchor0(),
            last.control0(),
            last.control1(),
            first.anchor0(),
        )),
        // An empty or zero-sized polygon: one degenerate curve at the centre.
        _ => cubics.push(Cubic::empty(center)),
    }

    cubics
}

/// Whether each curve starts where the previous one ended, wrapping around.
fn is_contiguous(cubics: &[Cubic]) -> bool {
    let Some(mut prev) = cubics.last().copied() else {
        return true;
    };
    for cubic in cubics {
        let (start, end) = (cubic.anchor0(), prev.anchor1());
        if (start.x - end.x).abs() > DISTANCE_EPSILON || (start.y - end.y).abs() > DISTANCE_EPSILON
        {
            return false;
        }
        prev = *cubic;
    }
    true
}

/// Expands a star's outer/inner rounding pair into a per-vertex list, or `None`
/// when the caller supplied its own list (or asked for no inner rounding).
fn alternating_rounding(
    num_vertices_per_radius: usize,
    rounding: CornerRounding,
    inner_rounding: Option<CornerRounding>,
    per_vertex_rounding: Option<&[CornerRounding]>,
) -> Option<Vec<CornerRounding>> {
    match (per_vertex_rounding, inner_rounding) {
        (None, Some(inner)) => Some(
            (0..num_vertices_per_radius)
                .flat_map(|_| [rounding, inner])
                .collect(),
        ),
        _ => None,
    }
}

/// The vertices of a regular polygon: `num_vertices` points equally spaced on a
/// circle of `radius` around `center`.
fn vertices_from_num_vertices(num_vertices: usize, radius: f64, center: Point) -> Vec<Point> {
    (0..num_vertices)
        .map(|i| center + radial_to_cartesian(radius, PI / num_vertices as f64 * 2.0 * i as f64))
        .collect()
}

/// The vertices of a star, alternating between the two radii.
fn star_vertices_from_num_vertices(
    num_vertices_per_radius: usize,
    radius: f64,
    inner_radius: f64,
    center: Point,
) -> Vec<Point> {
    let n = num_vertices_per_radius as f64;
    let mut result = Vec::with_capacity(num_vertices_per_radius * 2);
    for i in 0..num_vertices_per_radius {
        result.push(center + radial_to_cartesian(radius, PI / n * 2.0 * i as f64));
        result.push(center + radial_to_cartesian(inner_radius, PI / n * (2.0 * i as f64 + 1.0)));
    }
    result
}

/// The vertices of a pill-star, walked evenly around the underlying pill's
/// perimeter.
///
/// The perimeter splits into nine sections — the two half-lengths of the
/// right-hand vertical edge, four quarter-circle end caps, and the horizontal
/// and vertical edges between them — of which the vertical or the horizontal
/// ones are always zero-length (whichever dimension is smaller gets pure
/// curvature). Each vertex's distance along the perimeter says which section it
/// lands in and where inside it.
fn pill_star_vertices_from_num_vertices(
    num_vertices_per_radius: usize,
    width: f64,
    height: f64,
    inner_radius: f64,
    vertex_spacing: f64,
    start_location: f64,
    center: Point,
) -> Vec<Point> {
    let endcap_radius = width.min(height);
    let v_seg_len = (height - width).max(0.0);
    let h_seg_len = (width - height).max(0.0);
    let v_seg_half = v_seg_len / 2.0;
    let h_seg_half = h_seg_len / 2.0;

    let circle_perimeter = 2.0 * PI * endcap_radius * lerp(inner_radius, 1.0, vertex_spacing);
    let perimeter = 2.0 * h_seg_len + 2.0 * v_seg_len + circle_perimeter;

    // Where each section of the outline starts, as a distance along it.
    let mut sections = [0.0_f64; 11];
    sections[1] = v_seg_len / 2.0;
    sections[2] = sections[1] + circle_perimeter / 4.0;
    sections[3] = sections[2] + h_seg_len;
    sections[4] = sections[3] + circle_perimeter / 4.0;
    sections[5] = sections[4] + v_seg_len;
    sections[6] = sections[5] + circle_perimeter / 4.0;
    sections[7] = sections[6] + h_seg_len;
    sections[8] = sections[7] + circle_perimeter / 4.0;
    sections[9] = sections[8] + v_seg_len / 2.0;
    sections[10] = perimeter;

    let t_per_vertex = perimeter / (2.0 * num_vertices_per_radius as f64);
    let mut inner = false;
    let mut curr_sec_index = 0_usize;
    let mut sec_start = 0.0;
    let mut sec_end = sections[1];
    // 0 is on the positive x axis, heading into section 0; start_location
    // shifts that anywhere around the perimeter.
    let mut t = start_location * perimeter;

    let rect_br = Vec2::new(h_seg_half, v_seg_half);
    let rect_bl = Vec2::new(-h_seg_half, v_seg_half);
    let rect_tl = Vec2::new(-h_seg_half, -v_seg_half);
    let rect_tr = Vec2::new(h_seg_half, -v_seg_half);

    let mut result = Vec::with_capacity(num_vertices_per_radius * 2);
    for _ in 0..(num_vertices_per_radius * 2) {
        // t may start (and so end) past 0; wrapping keeps the section walk
        // right when it crosses back over.
        let bounded_t = t.rem_euclid(perimeter);
        if bounded_t < sec_start {
            curr_sec_index = 0;
        }
        while bounded_t >= sections[(curr_sec_index + 1) % sections.len()] {
            curr_sec_index = (curr_sec_index + 1) % sections.len();
            sec_start = sections[curr_sec_index];
            sec_end = sections[(curr_sec_index + 1) % sections.len()];
        }

        let t_in_section = bounded_t - sec_start;
        let t_proportion = t_in_section / (sec_end - sec_start);

        // On an end cap, the proportion is an angle along that quarter circle;
        // on a straight edge it is a plain linear position.
        let curr_radius = if inner {
            endcap_radius * inner_radius
        } else {
            endcap_radius
        };
        let vertex = match curr_sec_index {
            0 => Vec2::new(curr_radius, t_proportion * v_seg_half),
            1 => radial_to_cartesian(curr_radius, t_proportion * PI / 2.0) + rect_br,
            2 => Vec2::new(h_seg_half - t_proportion * h_seg_len, curr_radius),
            3 => radial_to_cartesian(curr_radius, PI / 2.0 + (t_proportion * PI / 2.0)) + rect_bl,
            4 => Vec2::new(-curr_radius, v_seg_half - t_proportion * v_seg_len),
            5 => radial_to_cartesian(curr_radius, PI + (t_proportion * PI / 2.0)) + rect_tl,
            6 => Vec2::new(-h_seg_half + t_proportion * h_seg_len, -curr_radius),
            7 => radial_to_cartesian(curr_radius, PI * 1.5 + (t_proportion * PI / 2.0)) + rect_tr,
            _ => Vec2::new(curr_radius, -v_seg_half + t_proportion * v_seg_half),
        };

        result.push(center + vertex);
        t += t_per_vertex;
        inner = !inner;
    }

    result
}

/// One vertex's worth of rounding state, held across the two passes the
/// polygon builder needs: first each corner says how much it *wants* to cut off
/// its two sides, then it is told how much it may actually cut (its neighbours
/// are competing for the same side) and produces its curves.
///
/// With no rounding the corner is the vertex itself, expressed as a zero-length
/// curve there. With rounding it is a circular arc; with smoothing on top, that
/// arc plus a flanking curve on each side carrying it out to the edges.
struct RoundedCorner {
    /// The vertex before the one being rounded.
    p0: Point,
    /// The vertex being rounded.
    p1: Point,
    /// The vertex after the one being rounded.
    p2: Point,
    /// Unit vector from `p1` toward `p0`.
    d1: Vec2,
    /// Unit vector from `p1` toward `p2`.
    d2: Vec2,
    corner_radius: f64,
    smoothing: f64,
    /// How far along each side the rounding alone wants to cut.
    expected_round_cut: f64,
}

impl RoundedCorner {
    fn new(p0: Point, p1: Point, p2: Point, rounding: CornerRounding) -> RoundedCorner {
        let v01 = p0 - p1;
        let v21 = p2 - p1;
        let d01 = v01.hypot();
        let d21 = v21.hypot();

        if d01 <= 0.0 || d21 <= 0.0 {
            // A side of zero length leaves nothing to round.
            return RoundedCorner {
                p0,
                p1,
                p2,
                d1: Vec2::ZERO,
                d2: Vec2::ZERO,
                corner_radius: 0.0,
                smoothing: 0.0,
                expected_round_cut: 0.0,
            };
        }

        let d1 = v01 / d01;
        let d2 = v21 / d21;
        // The cosine of the angle at p1 is the dot product of the unit vectors
        // to the other two vertices; sin follows from sin² + cos² = 1.
        let cos_angle = d1.dot(d2);
        let sin_angle = (1.0 - cos_angle * cos_angle).sqrt();
        // How far along a side the rounding circle meets it, via the identity
        // tan(A/2) = sinA / (1 + cosA), with tan(A/2) = radius / cut.
        let expected_round_cut = if sin_angle > COLLINEAR_SIN_EPSILON {
            rounding.radius * (cos_angle + 1.0) / sin_angle
        } else {
            0.0
        };

        RoundedCorner {
            p0,
            p1,
            p2,
            d1,
            d2,
            corner_radius: rounding.radius,
            smoothing: rounding.smoothing,
            expected_round_cut,
        }
    }

    /// How far along each side the corner wants to cut in total. Smoothing
    /// stretches the cut: `0` leaves it at the round cut, `1` doubles it.
    fn expected_cut(&self) -> f64 {
        (1.0 + self.smoothing) * self.expected_round_cut
    }

    /// This corner's curves, given how much each of its two sides will allow it
    /// to cut.
    fn cubics(&self, allowed_cut0: f64, allowed_cut1: f64) -> Vec<Cubic> {
        // The radius is bounded by the tighter of the two sides; a side with
        // room to spare can still spend it on smoothing.
        let allowed_cut = allowed_cut0.min(allowed_cut1);

        // Nothing to round: the corner is the vertex.
        if self.expected_round_cut < DISTANCE_EPSILON
            || allowed_cut < DISTANCE_EPSILON
            || self.corner_radius < DISTANCE_EPSILON
        {
            return vec![Cubic::straight_line(self.p1, self.p1)];
        }

        let actual_round_cut = allowed_cut.min(self.expected_round_cut);
        // Rounding is served first on each side; smoothing gets what is left.
        let actual_smoothing0 = self.actual_smoothing(allowed_cut0);
        let actual_smoothing1 = self.actual_smoothing(allowed_cut1);
        let actual_r = self.corner_radius * actual_round_cut / self.expected_round_cut;
        let center_distance = (actual_r * actual_r + actual_round_cut * actual_round_cut).sqrt();
        let center = self.p1 + direction((self.d1 + self.d2) / 2.0) * center_distance;
        let circle_intersection0 = self.p1 + self.d1 * actual_round_cut;
        let circle_intersection2 = self.p1 + self.d2 * actual_round_cut;

        let flanking0 = self.flanking_curve(
            actual_round_cut,
            actual_smoothing0,
            self.p0,
            circle_intersection0,
            circle_intersection2,
            center,
            actual_r,
        );
        let flanking2 = self
            .flanking_curve(
                actual_round_cut,
                actual_smoothing1,
                self.p2,
                circle_intersection2,
                circle_intersection0,
                center,
                actual_r,
            )
            .reversed();

        vec![
            flanking0,
            Cubic::circular_arc(center, flanking0.anchor1(), flanking2.anchor0()),
            flanking2,
        ]
    }

    /// How much smoothing survives a given allowed cut: full smoothing if the
    /// side has room for it, a proportional share if it only has some room
    /// beyond the rounding, none if the rounding already ate the side.
    fn actual_smoothing(&self, allowed_cut: f64) -> f64 {
        let expected_cut = self.expected_cut();
        if allowed_cut > expected_cut {
            self.smoothing
        } else if allowed_cut > self.expected_round_cut {
            self.smoothing * (allowed_cut - self.expected_round_cut)
                / (expected_cut - self.expected_round_cut)
        } else {
            0.0
        }
    }

    /// The curve joining one straight side to the corner's circular arc.
    ///
    /// It starts on the (cut) side and ends on the circle, meeting the circle
    /// tangentially so the join reads smooth.
    ///
    /// * `side_start` — the far end of the straight side (`p0` or `p2`).
    /// * `circle_intersection` — where this side meets the circle.
    /// * `other_circle_intersection` — where the opposing side meets it.
    #[expect(
        clippy::too_many_arguments,
        reason = "a faithful port of upstream's _computeFlankingCurve; every \
                  argument is one of its own, minus the two carried on self"
    )]
    fn flanking_curve(
        &self,
        actual_round_cut: f64,
        actual_smoothing: f64,
        side_start: Point,
        circle_intersection: Point,
        other_circle_intersection: Point,
        circle_center: Point,
        actual_r: f64,
    ) -> Cubic {
        let side_direction = direction(side_start - self.p1);
        let curve_start = self.p1 + side_direction * actual_round_cut * (1.0 + actual_smoothing);

        // Cut away a share of the circular section proportional to
        // 1 - smoothing: none of it at smoothing 0, all of it at 1. An
        // approximation, and one that gets shaky as the corner approaches 180°.
        let midpoint = lerp_point(circle_intersection, other_circle_intersection, 0.5);
        let p = lerp_point(circle_intersection, midpoint, actual_smoothing);

        // The flanking curve ends on the circle.
        let curve_end = circle_center + direction(p - circle_center) * actual_r;

        // Its anchor on the circle side is where the tangent at that end point
        // crosses the straight side.
        let circle_tangent = rotate90(curve_end - circle_center);
        let anchor_end = line_intersection(side_start, side_direction, curve_end, circle_tangent)
            .unwrap_or(circle_intersection);

        // Of what remains, two thirds of the way to that anchor — the ratio
        // design tools converge on.
        let anchor_start = ((curve_start.to_vec2() + anchor_end.to_vec2() * 2.0) / 3.0).to_point();

        Cubic::new(curve_start, anchor_start, anchor_end, curve_end)
    }
}

/// Where the lines `p0 + t * d0` and `p1 + t * d1` cross, or `None` when they
/// are parallel enough not to.
fn line_intersection(p0: Point, d0: Vec2, p1: Point, d1: Vec2) -> Option<Point> {
    let rotated_d1 = rotate90(d1);
    let den = d0.dot(rotated_d1);

    if den.abs() < DISTANCE_EPSILON {
        return None;
    }

    let num = (p1 - p0).dot(rotated_d1);

    // The same check relative to the numerator — equivalent to
    // `(den / num).abs() < DISTANCE_EPSILON` without the division.
    if den.abs() < DISTANCE_EPSILON * num.abs() {
        return None;
    }

    Some(p0 + d0 * (num / den))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{CubicBez, ParamCurveNearest, PathEl};

    /// Upstream's own test epsilon (`test/test_utils.dart`), used wherever a
    /// ported assertion is analytic rather than a recorded value.
    const EPSILON: f64 = 1e-4;

    /// Tolerance for the transcribed Dart goldens below. The Dart source prints
    /// shortest-round-trip doubles, so every literal parses back to the exact
    /// `f64` upstream held; anything above pure operation-ordering noise here is
    /// a real divergence.
    const GOLDEN_EPSILON: f64 = 1e-12;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn assert_close(expected: f64, actual: f64, what: &str) {
        assert!(
            (expected - actual).abs() < EPSILON,
            "{what}: expected {expected}, got {actual}"
        );
    }

    /// Asserts a polygon's cubics match a golden dumped from the upstream Dart
    /// implementation (see the crate's parity note in the module docs).
    fn assert_matches_golden(polygon: &RoundedPolygon, golden: &[[f64; 8]], name: &str) {
        assert_eq!(
            polygon.cubics().len(),
            golden.len(),
            "{name}: cubic count drifted from upstream"
        );
        for (i, (cubic, want)) in polygon.cubics().iter().zip(golden).enumerate() {
            for (j, (got, want)) in cubic.points().iter().zip(want).enumerate() {
                assert!(
                    (got - want).abs() < GOLDEN_EPSILON,
                    "{name}: cubic {i} coordinate {j}: expected {want}, got {got}"
                );
            }
        }
    }

    fn assert_bounds(bounds: Rect, want: [f64; 4], name: &str) {
        let got = [bounds.x0, bounds.y0, bounds.x1, bounds.y1];
        for (i, (got, want)) in got.iter().zip(&want).enumerate() {
            assert!(
                (got - want).abs() < GOLDEN_EPSILON,
                "{name}: bound {i}: expected {want}, got {got}"
            );
        }
    }

    fn kinds(polygon: &RoundedPolygon) -> Vec<FeatureKind> {
        polygon.features().iter().map(Feature::kind).collect()
    }

    // -----------------------------------------------------------------------
    // Cubic
    // -----------------------------------------------------------------------

    #[test]
    fn straight_line_puts_its_controls_at_the_thirds() {
        let line = Cubic::straight_line(p(1.0, 0.0), p(0.0, 1.0));
        assert_eq!(line.anchor0(), p(1.0, 0.0));
        assert_eq!(line.anchor1(), p(0.0, 1.0));
        assert_close(line.control0().x, 2.0 / 3.0, "control0.x");
        assert_close(line.control0().y, 1.0 / 3.0, "control0.y");
        assert_close(line.control1().x, 1.0 / 3.0, "control1.x");
        assert_close(line.control1().y, 2.0 / 3.0, "control1.y");
    }

    #[test]
    fn circular_arc_keeps_its_endpoints_and_degrades_to_a_line() {
        let arc = Cubic::circular_arc(Point::ZERO, p(1.0, 0.0), p(0.0, 1.0));
        assert_eq!(arc.anchor0(), p(1.0, 0.0));
        assert_eq!(arc.anchor1(), p(0.0, 1.0));
        // Every sampled point sits on the unit circle.
        for i in 0..=20 {
            let on_curve = arc.point_on_curve(f64::from(i) / 20.0);
            assert!((on_curve.to_vec2().hypot() - 1.0).abs() < 3e-4);
        }

        // Near-coincident endpoints have no arc left to approximate.
        let degenerate = Cubic::circular_arc(Point::ZERO, p(1.0, 0.0), p(1.0, 0.001));
        assert_eq!(degenerate, Cubic::straight_line(p(1.0, 0.0), p(1.0, 0.001)));
    }

    #[test]
    fn split_covers_the_same_curve() {
        let cubic = Cubic::new(p(1.0, 0.0), p(1.0, 0.5), p(0.5, 1.0), p(0.0, 1.0));
        let (a, b) = cubic.split(0.5);
        assert_eq!(a.anchor0(), cubic.anchor0());
        assert_eq!(b.anchor1(), cubic.anchor1());
        assert_eq!(a.anchor1(), b.anchor0());
        for i in 0..=10 {
            let t = f64::from(i) / 10.0;
            let whole = cubic.point_on_curve(t / 2.0);
            let half = a.point_on_curve(t);
            assert!((whole - half).hypot() < 1e-12, "t = {t}");
        }
    }

    #[test]
    fn reversed_swaps_the_ends() {
        let cubic = Cubic::new(p(1.0, 0.0), p(1.0, 0.5), p(0.5, 1.0), p(0.0, 1.0));
        let back = cubic.reversed();
        assert_eq!(back.anchor0(), cubic.anchor1());
        assert_eq!(back.control0(), cubic.control1());
        assert_eq!(back.control1(), cubic.control0());
        assert_eq!(back.anchor1(), cubic.anchor0());
    }

    #[test]
    fn empty_cubic_has_zero_length() {
        assert!(Cubic::empty(p(10.0, 10.0)).zero_length());
        assert!(!Cubic::straight_line(p(0.0, 0.0), p(1.0, 0.0)).zero_length());
    }

    #[test]
    fn exact_bounds_are_tighter_than_approximate_ones() {
        let cubic = Cubic::new(p(0.0, 0.0), p(0.0, 1.0), p(1.0, 1.0), p(1.0, 0.0));
        let exact = cubic.exact_bounds();
        let approximate = cubic.approximate_bounds();
        assert_bounds(approximate, [0.0, 0.0, 1.0, 1.0], "approximate");
        // The curve never reaches y = 1; its peak is at 3/4.
        assert_bounds(exact, [0.0, 0.0, 1.0, 0.75], "exact");
    }

    // -----------------------------------------------------------------------
    // Feature
    // -----------------------------------------------------------------------

    #[test]
    fn feature_kinds_report_themselves() {
        let edge = Feature::edge(Cubic::straight_line(p(0.0, 0.0), p(1.0, 0.0)));
        assert!(edge.is_edge() && edge.is_ignorable() && !edge.is_corner());

        let convex = Feature::convex_corner(vec![Cubic::empty(p(1.0, 0.0))]);
        assert!(convex.is_corner() && convex.is_convex_corner() && !convex.is_ignorable());

        let concave = Feature::concave_corner(vec![Cubic::empty(p(1.0, 0.0))]);
        assert!(concave.is_concave_corner() && !concave.is_convex_corner());

        // An ignorable feature is an edge of any indentation.
        assert!(Feature::ignorable(vec![Cubic::empty(p(0.0, 0.0))]).is_ignorable());
    }

    #[test]
    fn reversing_a_corner_flips_its_indentation() {
        let corner = Feature::convex_corner(vec![
            Cubic::straight_line(p(0.0, 0.0), p(1.0, 0.0)),
            Cubic::straight_line(p(1.0, 0.0), p(1.0, 1.0)),
        ]);
        let back = corner.reversed();
        assert_eq!(back.kind(), FeatureKind::ConcaveCorner);
        assert_eq!(back.cubics()[0].anchor0(), p(1.0, 1.0));
        assert_eq!(back.cubics()[1].anchor1(), p(0.0, 0.0));
        assert_eq!(corner.reversed().reversed(), corner);
    }

    #[test]
    #[should_panic(expected = "must be continuous")]
    fn a_discontinuous_feature_is_rejected() {
        let _ = Feature::convex_corner(vec![
            Cubic::straight_line(p(0.0, 0.0), p(1.0, 0.0)),
            Cubic::straight_line(p(10.0, 10.0), p(20.0, 20.0)),
        ]);
    }

    #[test]
    #[should_panic(expected = "at least one cubic")]
    fn an_empty_feature_is_rejected() {
        let _ = Feature::convex_corner(Vec::new());
    }

    // -----------------------------------------------------------------------
    // Construction invariants
    // -----------------------------------------------------------------------

    #[test]
    fn an_unrounded_polygon_reproduces_its_vertex_polyline() {
        let vertices = [
            p(1.0, 0.0),
            p(0.0, 1.0),
            p(-1.0, 0.0),
            p(0.0, -1.0),
            p(0.5, -0.5),
        ];
        let polygon =
            RoundedPolygon::from_vertices(&vertices, CornerRounding::UNROUNDED, None, None);

        // One straight-line cubic per side, in vertex order, with the controls
        // at the thirds — i.e. exactly the polyline through the vertices.
        assert_eq!(polygon.cubics().len(), vertices.len());
        for (i, cubic) in polygon.cubics().iter().enumerate() {
            let from = vertices[i];
            let to = vertices[(i + 1) % vertices.len()];
            assert_eq!(cubic.anchor0(), from, "cubic {i} start");
            assert_eq!(cubic.anchor1(), to, "cubic {i} end");
            assert_eq!(cubic.control0(), lerp_point(from, to, 1.0 / 3.0));
            assert_eq!(cubic.control1(), lerp_point(from, to, 2.0 / 3.0));
        }
    }

    #[test]
    fn to_path_is_closed_and_lands_exactly_on_its_start() {
        for polygon in [
            RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::UNROUNDED, None, Point::ZERO),
            RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::new(0.5, 0.2), None, Point::ZERO),
            RoundedPolygon::circle(8, 1.0, Point::ZERO),
            RoundedPolygon::star(StarParams::default()),
        ] {
            let path = polygon.to_path();
            let elements: Vec<PathEl> = path.elements().to_vec();

            let Some(PathEl::MoveTo(start)) = elements.first().copied() else {
                panic!("a path must open with a MoveTo, got {:?}", elements.first());
            };
            assert_eq!(start, polygon.cubics()[0].anchor0());
            assert!(matches!(elements.last(), Some(PathEl::ClosePath)));

            let curves: Vec<PathEl> = elements
                .iter()
                .copied()
                .filter(|el| matches!(el, PathEl::CurveTo(..)))
                .collect();
            assert_eq!(curves.len(), polygon.cubics().len());

            // Bit-exact closure: a sub-pixel gap here shows up as a seam.
            let Some(PathEl::CurveTo(_, _, end)) = curves.last().copied() else {
                unreachable!("filtered to CurveTo above");
            };
            assert_eq!(end, start, "the outline must close exactly on its start");
        }
    }

    #[test]
    fn the_circle_constructor_is_round() {
        let radius = 3.0;
        let center = p(1.0, -2.0);
        // A circle here is n cubic-approximated arcs, one per corner, each
        // spanning a turn of 2π/n — so how round it is depends on n. The
        // tolerances below are relative to the radius: the eight-vertex
        // default's ~4e-6 is the cubic arc approximation's own error over a 45°
        // sweep, not port drift, and twelve vertices already buys an order of
        // magnitude.
        for (num_vertices, tolerance) in [(8, 5e-6), (12, 1e-6), (20, 1e-6), (32, 1e-6)] {
            let circle = RoundedPolygon::circle(num_vertices, radius, center);
            let mut worst = 0.0_f64;
            for cubic in circle.cubics() {
                for i in 0..=16 {
                    let on_curve = cubic.point_on_curve(f64::from(i) / 16.0);
                    worst = worst.max(((on_curve - center).hypot() - radius).abs() / radius);
                }
            }
            assert!(
                worst < tolerance,
                "{num_vertices}-gon circle deviates by {worst} of its radius (> {tolerance})"
            );
        }
    }

    #[test]
    fn rectangle_bounds_are_exact() {
        let rectangle =
            RoundedPolygon::rectangle(4.0, 2.0, CornerRounding::UNROUNDED, None, p(1.0, 3.0));
        assert_bounds(rectangle.bounds(), [-1.0, 2.0, 3.0, 4.0], "approximate");
        assert_bounds(rectangle.exact_bounds(), [-1.0, 2.0, 3.0, 4.0], "exact");
        assert_eq!(rectangle.center(), p(1.0, 3.0));
    }

    #[test]
    fn a_rounded_square_is_symmetric_under_a_quarter_turn() {
        let square =
            RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::new(0.5, 0.3), None, Point::ZERO);
        let turned = square.transformed(Affine::rotate(std::f64::consts::FRAC_PI_2));

        // Every point sampled off the turned outline lands back on the original
        // outline. Measured against the curves themselves rather than against
        // sampled points: a quarter turn maps the outline onto itself as a
        // *set*, but not parameter-for-parameter (the cubic list starts halfway
        // through a corner arc, so the two runs are offset).
        let curves: Vec<CubicBez> = square
            .cubics()
            .iter()
            .map(|c| CubicBez::new(c.anchor0(), c.control0(), c.control1(), c.anchor1()))
            .collect();
        for cubic in turned.cubics() {
            for i in 0..8 {
                let want = cubic.point_on_curve(f64::from(i) / 8.0);
                let nearest = curves
                    .iter()
                    .map(|curve| curve.nearest(want, 1e-12).distance_sq)
                    .fold(f64::INFINITY, f64::min)
                    .sqrt();
                assert!(nearest < 1e-9, "{want:?} is {nearest} off the outline");
            }
        }
    }

    #[test]
    fn the_center_defaults_to_the_vertex_average() {
        let polygon = RoundedPolygon::from_vertices(
            &[p(0.0, 0.0), p(1.0, 0.0), p(0.0, 1.0), p(1.0, 1.0)],
            CornerRounding::UNROUNDED,
            None,
            None,
        );
        assert_close(polygon.center().x, 0.5, "center.x");
        assert_close(polygon.center().y, 0.5, "center.y");
    }

    #[test]
    fn normalized_fits_the_unit_box() {
        for polygon in [
            RoundedPolygon::star(StarParams::default()),
            RoundedPolygon::pill(2.0, 1.0, 0.0, Point::ZERO),
            RoundedPolygon::circle(10, 4.0, p(-7.0, 3.0)),
        ] {
            let bounds = polygon.normalized().bounds();
            assert!(bounds.x0 >= -EPSILON && bounds.y0 >= -EPSILON, "{bounds:?}");
            assert!(
                bounds.x1 <= 1.0 + EPSILON && bounds.y1 <= 1.0 + EPSILON,
                "{bounds:?}"
            );
            // The longer axis fills the box exactly.
            assert_close(bounds.width().max(bounds.height()), 1.0, "longer axis");
        }
    }

    #[test]
    fn rebuilding_from_features_reproduces_the_polygon() {
        let mut shapes = vec![
            RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::UNROUNDED, None, Point::ZERO),
            RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::new(0.5, 0.2), None, Point::ZERO),
            RoundedPolygon::pill(2.0, 1.0, 0.0, Point::ZERO),
            RoundedPolygon::pill_star(PillStarParams {
                rounding: CornerRounding::new(0.5, 0.2),
                ..PillStarParams::default()
            }),
        ];
        for n in 3..=20 {
            shapes.push(RoundedPolygon::circle(n, 1.0, Point::ZERO));
            shapes.push(RoundedPolygon::star(StarParams {
                num_vertices_per_radius: n,
                ..StarParams::default()
            }));
            shapes.push(RoundedPolygon::star(StarParams {
                num_vertices_per_radius: n,
                rounding: CornerRounding::new(0.5, 0.2),
                ..StarParams::default()
            }));
        }

        for base in shapes {
            let rebuilt = RoundedPolygon::from_features(base.features().to_vec(), None);
            assert_eq!(kinds(&rebuilt), kinds(&base));
            assert_eq!(rebuilt.cubics().len(), base.cubics().len());
            for (got, want) in rebuilt.cubics().iter().zip(base.cubics()) {
                for (got, want) in got.points().iter().zip(want.points()) {
                    assert_close(*want, *got, "rebuilt cubic");
                }
            }
        }
    }

    #[test]
    fn a_full_size_shape_matches_its_canonical_one() {
        // Upstream's "full size creation": building at UI scale and scaling
        // down must land on the same curves as building canonically.
        let radius = 400.0;
        let inner_radius_factor = 0.35;
        let rounding_factor = 0.32;

        let full_size = RoundedPolygon::star(StarParams {
            num_vertices_per_radius: 4,
            radius,
            inner_radius: radius * inner_radius_factor,
            rounding: CornerRounding::radius(radius * rounding_factor),
            inner_rounding: Some(CornerRounding::radius(radius * rounding_factor)),
            per_vertex_rounding: None,
            center: p(radius, radius),
        })
        .transformed_with(|point| {
            Point::new((point.x - radius) / radius, (point.y - radius) / radius)
        });

        let canonical = RoundedPolygon::star(StarParams {
            num_vertices_per_radius: 4,
            radius: 1.0,
            inner_radius: inner_radius_factor,
            rounding: CornerRounding::radius(rounding_factor),
            inner_rounding: Some(CornerRounding::radius(rounding_factor)),
            per_vertex_rounding: None,
            center: Point::ZERO,
        });

        assert_eq!(full_size.cubics().len(), canonical.cubics().len());
        for (got, want) in full_size.cubics().iter().zip(canonical.cubics()) {
            for (got, want) in got.points().iter().zip(want.points()) {
                assert_close(*want, *got, "full-size vs canonical");
            }
        }
    }

    // -----------------------------------------------------------------------
    // Cut competition between neighbouring corners (upstream's own tests)
    // -----------------------------------------------------------------------

    #[test]
    fn rounding_takes_space_before_smoothing_does() {
        // The p0 -> p1 side has no room even for the two roundings, so
        // smoothing gets nothing and both corners end at the side's midpoint —
        // collapsing that edge to a zero-length one.
        let polygon = RoundedPolygon::from_vertices(
            &[p(0.0, 0.0), p(1.0, 0.0), p(0.5, 1.0)],
            CornerRounding::UNROUNDED,
            Some(&[
                CornerRounding::radius(1.0),
                CornerRounding::new(1.0, 1.0),
                CornerRounding::UNROUNDED,
            ]),
            None,
        );

        let lower_edge = polygon
            .features()
            .iter()
            .find(|f| f.is_edge())
            .expect("a polygon has edges");
        assert_eq!(lower_edge.cubics().len(), 1);
        let cubic = lower_edge.cubics()[0];
        assert_close(0.5, cubic.anchor0().x, "edge start x");
        assert_close(0.0, cubic.anchor0().y, "edge start y");
        assert_close(0.5, cubic.anchor1().x, "edge end x");
        assert_close(0.0, cubic.anchor1().y, "edge end y");
    }

    /// Upstream's `doUnevenSmoothTest`: two corners of a 5x1 rectangle compete
    /// for the same short left-hand side.
    ///
    /// ```text
    ///   vertex 0              vertex 1
    ///      *---------------------*
    ///      |                     |
    ///      *---------------------*
    ///   vertex 3              vertex 2
    /// ```
    fn uneven_smooth_case(
        rounding0: CornerRounding,
        rounding3: CornerRounding,
        expected_v0_sx: f64,
        expected_v0_sy: f64,
        expected_v3_sy: f64,
    ) {
        let polygon = RoundedPolygon::from_vertices(
            &[p(0.0, 0.0), p(5.0, 0.0), p(5.0, 1.0), p(0.0, 1.0)],
            CornerRounding::UNROUNDED,
            Some(&[
                rounding0,
                CornerRounding::UNROUNDED,
                CornerRounding::UNROUNDED,
                rounding3,
            ]),
            None,
        );

        let edges: Vec<&Feature> = polygon.features().iter().filter(|f| f.is_edge()).collect();
        let (e01, e30) = (edges[0], edges[3]);
        let context = format!("r0 = {rounding0:?}, r3 = {rounding3:?}");

        assert_close(expected_v0_sx, e01.cubics()[0].anchor0().x, &context);
        assert_close(expected_v0_sy, e30.cubics()[0].anchor1().y, &context);
        assert_close(expected_v3_sy, 1.0 - e30.cubics()[0].anchor0().y, &context);
    }

    #[test]
    fn uneven_smoothing_against_a_plain_rounded_neighbour() {
        // Vertex 3 has the default 0.5 radius and no smoothing; vertex 0 has a
        // 0.4 radius and smoothing sweeping 0 -> 1.
        for i in 0..=20 {
            let smooth = f64::from(i) / 20.0;
            uneven_smooth_case(
                CornerRounding::new(0.4, smooth),
                CornerRounding::radius(0.5),
                0.4 * (1.0 + smooth),
                (0.4 * (1.0 + smooth)).min(0.5),
                0.5,
            );
        }
    }

    #[test]
    fn uneven_smoothing_when_both_neighbours_want_smoothing() {
        // Vertex 3 takes at most 0.4; past a smoothing of 0.5 vertex 0 starts
        // competing with it for the leftover 0.4 of side.
        for i in 0..=20 {
            let smooth = f64::from(i) / 20.0;
            let wanted_v0 = 0.4 * smooth;
            let wanted_v3 = 0.2;
            let factor = (0.4 / (wanted_v0 + wanted_v3)).min(1.0);
            uneven_smooth_case(
                CornerRounding::new(0.4, smooth),
                CornerRounding::new(0.2, 1.0),
                0.4 * (1.0 + smooth),
                0.4 + factor * wanted_v0,
                0.2 + factor * wanted_v3,
            );
        }
    }

    #[test]
    fn uneven_smoothing_with_no_room_left_on_the_shared_side() {
        // Vertex 3's 0.6 radius leaves the shared side full, but vertex 0 can
        // still smooth along the top.
        for i in 0..=20 {
            let smooth = f64::from(i) / 20.0;
            uneven_smooth_case(
                CornerRounding::new(0.4, smooth),
                CornerRounding::radius(0.6),
                0.4 * (1.0 + smooth),
                0.4,
                0.6,
            );
        }
    }

    // -----------------------------------------------------------------------
    // Dart -> Rust parity goldens
    //
    // Control points dumped straight out of `material_new_shapes` 1.0.0 running
    // under `flutter test`; the constructor calls below are the same ones that
    // produced them. These pin the port's numeric behaviour, not merely its
    // shape.
    // -----------------------------------------------------------------------

    #[rustfmt::skip]
    const TRIANGLE_SPACE_USAGE: [[f64; 8]; 5] = [
        [0.2371344439404332, 0.14655714625849423, 0.2903596433552479, 0.06043696454733296, 0.38514116108158136, -5.551115123125783e-17, 0.5, -5.551115123125783e-17],
        [0.5, -5.551115123125783e-17, 0.7297176778368372, -5.551115123125783e-17, 0.879126070905334, 0.241747858189332, 0.7763932022500211, 0.44721359549995787],
        [0.7763932022500211, 0.44721359549995787, 0.6842621348333474, 0.6314757303333053, 0.5921310674166738, 0.8157378651666526, 0.5, 1.0],
        [0.5, 1.0, 0.40786893258332635, 0.8157378651666527, 0.31573786516665264, 0.6314757303333053, 0.223606797749979, 0.44721359549995787],
        [0.223606797749979, 0.44721359549995787, 0.17224036342232252, 0.3444807268446449, 0.18390924452561846, 0.23267732796965546, 0.2371344439404332, 0.14655714625849423],
    ];

    #[rustfmt::skip]
    const SQUARE_UNROUNDED: [[f64; 8]; 4] = [
        [1.0, 1.0, 0.3333333333333334, 1.0, -0.33333333333333326, 1.0, -1.0, 1.0],
        [-1.0, 1.0, -1.0, 0.3333333333333334, -1.0, -0.33333333333333326, -1.0, -1.0],
        [-1.0, -1.0, -0.3333333333333334, -1.0, 0.33333333333333326, -1.0, 1.0, -1.0],
        [1.0, -1.0, 1.0, -0.3333333333333334, 1.0, 0.33333333333333326, 1.0, 1.0],
    ];

    #[rustfmt::skip]
    const SQUARE_ROUNDED_SMOOTHED: [[f64; 8]; 17] = [
        [0.8535533905932737, 0.8535533905932736, 0.7754029376043261, 0.9317038435822215, 0.6714276261466594, 0.9840294381024499, 0.5552157630374233, 0.9969418673368095],
        [0.5552157630374233, 0.9969418673368095, 0.5276925690687082, 1.0, 0.4851283793791388, 1.0, 0.4, 1.0],
        [0.4, 1.0, 0.1333333333333334, 1.0, -0.1333333333333333, 1.0, -0.4, 1.0],
        [-0.4, 1.0, -0.4851283793791388, 1.0, -0.5276925690687082, 1.0, -0.5552157630374233, 0.9969418673368095],
        [-0.5552157630374233, 0.9969418673368095, -0.7876394892558956, 0.9711170088680903, -0.9711170088680903, 0.7876394892558956, -0.9969418673368095, 0.5552157630374233],
        [-0.9969418673368095, 0.5552157630374233, -1.0, 0.5276925690687082, -1.0, 0.4851283793791388, -1.0, 0.4],
        [-1.0, 0.4, -1.0, 0.1333333333333334, -1.0, -0.1333333333333333, -1.0, -0.4],
        [-1.0, -0.4, -1.0, -0.4851283793791388, -1.0, -0.5276925690687082, -0.9969418673368095, -0.5552157630374233],
        [-0.9969418673368095, -0.5552157630374233, -0.9711170088680903, -0.7876394892558956, -0.7876394892558956, -0.9711170088680903, -0.5552157630374233, -0.9969418673368095],
        [-0.5552157630374233, -0.9969418673368095, -0.5276925690687082, -1.0, -0.4851283793791388, -1.0, -0.4, -1.0],
        [-0.4, -1.0, -0.1333333333333334, -1.0, 0.1333333333333333, -1.0, 0.4, -1.0],
        [0.4, -1.0, 0.4851283793791388, -1.0, 0.5276925690687082, -1.0, 0.5552157630374233, -0.9969418673368095],
        [0.5552157630374233, -0.9969418673368095, 0.7876394892558956, -0.9711170088680903, 0.9711170088680903, -0.7876394892558956, 0.9969418673368095, -0.5552157630374233],
        [0.9969418673368095, -0.5552157630374233, 1.0, -0.5276925690687082, 1.0, -0.4851283793791388, 1.0, -0.4],
        [1.0, -0.4, 1.0, -0.1333333333333334, 1.0, 0.1333333333333333, 1.0, 0.4],
        [1.0, 0.4, 1.0, 0.4851283793791388, 1.0, 0.5276925690687082, 0.9969418673368095, 0.5552157630374233],
        [0.9969418673368095, 0.5552157630374233, 0.9840294381024499, 0.6714276261466594, 0.9317038435822214, 0.7754029376043262, 0.8535533905932737, 0.8535533905932736],
    ];

    #[rustfmt::skip]
    const CIRCLE_8: [[f64; 8]; 9] = [
        [1.0, 1.3877787807814457e-17, 1.0, 0.1300846945207343, 0.9746265108370957, 0.2601693890414686, 0.9238795325112868, 0.3826834323650898],
        [0.9238795325112868, 0.3826834323650898, 0.8223855758596692, 0.6277115190123321, 0.6277115190123322, 0.822385575859669, 0.38268343236508984, 0.9238795325112871],
        [0.38268343236508984, 0.9238795325112871, 0.13765534571784743, 1.0253734891629047, -0.13765534571784727, 1.0253734891629047, -0.3826834323650897, 0.9238795325112868],
        [-0.3826834323650897, 0.9238795325112868, -0.6277115190123324, 0.8223855758596691, -0.8223855758596692, 0.6277115190123326, -0.9238795325112871, 0.38268343236508984],
        [-0.9238795325112871, 0.38268343236508984, -1.0253734891629047, 0.13765534571784738, -1.0253734891629047, -0.1376553457178471, -0.9238795325112867, -0.3826834323650896],
        [-0.9238795325112867, -0.3826834323650896, -0.8223855758596691, -0.6277115190123324, -0.6277115190123326, -0.822385575859669, -0.38268343236508995, -0.9238795325112871],
        [-0.38268343236508995, -0.9238795325112871, -0.1376553457178475, -1.0253734891629047, 0.137655345717847, -1.0253734891629047, 0.38268343236508956, -0.9238795325112871],
        [0.38268343236508956, -0.9238795325112871, 0.6277115190123322, -0.8223855758596694, 0.822385575859669, -0.6277115190123328, 0.9238795325112867, -0.3826834323650897],
        [0.9238795325112867, -0.3826834323650897, 0.9746265108370956, -0.2601693890414685, 1.0, -0.1300846945207343, 1.0, 1.3877787807814457e-17],
    ];

    #[rustfmt::skip]
    const STAR_5: [[f64; 8]; 11] = [
        [0.6480743810254861, -4.85722573273506e-17, 0.6480743810254861, 0.0996540589356331, 0.5961576437278834, 0.19930811787126626, 0.4923241691326778, 0.2505530011089714],
        [0.4923241691326778, 0.2505530011089714, 0.43734769396117146, 0.27768551364375393, 0.39924248711635457, 0.3301328314207743, 0.3904265993855176, 0.39080297384891327],
        [0.3904265993855176, 0.39080297384891327, 0.3571256555953457, 0.6199770586860098, 0.07549264735137257, 0.7114851701738012, -0.08615352937847076, 0.5456532445175477],
        [-0.08615352937847076, 0.5456532445175477, -0.12894674734696346, 0.501751917036087, -0.19060226717022227, 0.48171882426403184, -0.25102726060038916, 0.4920825218521362],
        [-0.25102726060038916, 0.4920825218521362, -0.4792753249105761, 0.5312301291467034, -0.6533340963592308, 0.2916587830253511, -0.5455699785393353, 0.08667925007453033],
        [-0.5455699785393353, 0.08667925007453033, -0.5170411665603402, 0.032414225004965255, -0.5170411665603402, -0.032414225004965144, -0.5455699785393353, -0.08667925007453012],
        [-0.5455699785393353, -0.08667925007453012, -0.6533340963592309, -0.29165878302535075, -0.47927532491057623, -0.5312301291467032, -0.2510272606003892, -0.4920825218521361],
        [-0.2510272606003892, -0.4920825218521361, -0.19060226717022244, -0.48171882426403173, -0.12894674734696351, -0.501751917036087, -0.08615352937847082, -0.5456532445175477],
        [-0.08615352937847082, -0.5456532445175477, 0.07549264735137243, -0.7114851701738012, 0.3571256555953456, -0.6199770586860099, 0.39042659938551744, -0.3908029738489133],
        [0.39042659938551744, -0.3908029738489133, 0.3992424871163545, -0.33013283142077454, 0.43734769396117146, -0.27768551364375416, 0.49232416913267785, -0.25055300110897155],
        [0.49232416913267785, -0.25055300110897155, 0.5961576437278834, -0.19930811787126637, 0.6480743810254861, -0.09965405893563323, 0.6480743810254861, -4.85722573273506e-17],
    ];

    #[rustfmt::skip]
    const PILL: [[f64; 8]; 11] = [
        [0.8830446015004401, 0.3213671315821969, 0.8096338575801304, 0.4088670230999255, 0.7063570243187614, 0.47130983399174997, 0.5868926669545238, 0.4923917794089684],
        [0.5868926669545238, 0.4923917794089684, 0.5437794169386783, 0.5, 0.4791862779591189, 0.5, 0.35, 0.5],
        [0.35, 0.5, 0.11666666666666668, 0.5, -0.11666666666666664, 0.5, -0.35, 0.5],
        [-0.35, 0.5, -0.4791862779591189, 0.5, -0.5437794169386783, 0.5, -0.5868926669545238, 0.4923917794089684],
        [-0.5868926669545238, 0.4923917794089684, -0.8258213816829989, 0.4502278885745316, -1.0, 0.2426205358416706, -1.0, 0.0],
        [-1.0, 0.0, -1.0, -0.2426205358416706, -0.8258213816829989, -0.4502278885745316, -0.5868926669545238, -0.4923917794089684],
        [-0.5868926669545238, -0.4923917794089684, -0.5437794169386783, -0.5, -0.4791862779591189, -0.5, -0.35, -0.5],
        [-0.35, -0.5, -0.11666666666666668, -0.5, 0.11666666666666664, -0.5, 0.35, -0.5],
        [0.35, -0.5, 0.4791862779591189, -0.5, 0.5437794169386783, -0.5, 0.5868926669545238, -0.4923917794089684],
        [0.5868926669545238, -0.4923917794089684, 0.8258213816829989, -0.4502278885745316, 1.0, -0.2426205358416706, 1.0, 0.0],
        [1.0, 0.0, 1.0, 0.1213102679208353, 0.9564553454207497, 0.23386724006446818, 0.8830446015004401, 0.3213671315821969],
    ];

    #[rustfmt::skip]
    const NORMALIZED_ARCH: [[f64; 8]; 8] = [
        [0.9798474571599122, 0.5468644038342292, 0.9798474571599124, 0.6525169448765878, 0.9395423714797368, 0.7581694859189465, 0.8589322001193854, 0.8387796572792977],
        [0.8589322001193854, 0.8387796572792977, 0.6977118573986831, 1.0, 0.4363220359499508, 1.0, 0.27510169322924843, 0.8387796572792977],
        [0.27510169322924843, 0.8387796572792977, 0.20531864277993192, 0.7689966068299814, 0.13553559233061546, 0.6992135563806647, 0.06575254188129889, 0.6294305059313483],
        [0.06575254188129889, 0.6294305059313483, 0.020152542840087787, 0.583830506890137, 0.020152542840087787, 0.5098983007783213, 0.06575254188129889, 0.4642983017371102],
        [0.06575254188129889, 0.4642983017371102, 0.20531864277993184, 0.32473220083847715, 0.3448847436785648, 0.18516609993984418, 0.4844508445771978, 0.045599999041211176],
        [0.4844508445771978, 0.045599999041211176, 0.5300508436184089, 6.481820712280062e-17, 0.6039830497302248, 0.0, 0.649583048771436, 0.04559999904121111],
        [0.649583048771436, 0.04559999904121111, 0.7193660992207523, 0.11538304949052762, 0.789149149670069, 0.18516609993984412, 0.8589322001193855, 0.25494915038916066],
        [0.8589322001193855, 0.25494915038916066, 0.9395423714797366, 0.3355593217495118, 0.9798474571599123, 0.4412118627918704, 0.9798474571599122, 0.5468644038342292],
    ];

    #[test]
    fn parity_uneven_cut_competition() {
        let polygon = RoundedPolygon::from_vertices(
            &[p(0.0, 0.0), p(1.0, 0.0), p(0.5, 1.0)],
            CornerRounding::UNROUNDED,
            Some(&[
                CornerRounding::radius(1.0),
                CornerRounding::new(1.0, 1.0),
                CornerRounding::UNROUNDED,
            ]),
            None,
        );
        assert_matches_golden(&polygon, &TRIANGLE_SPACE_USAGE, "triangle");
        assert_close(0.5, polygon.center().x, "center.x");
        assert_close(1.0 / 3.0, polygon.center().y, "center.y");
        assert_bounds(
            polygon.bounds(),
            [
                0.17224036342232252,
                -5.551115123125783e-17,
                0.879126070905334,
                1.0,
            ],
            "triangle bounds",
        );
    }

    #[test]
    fn parity_square() {
        let unrounded =
            RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::UNROUNDED, None, Point::ZERO);
        assert_matches_golden(&unrounded, &SQUARE_UNROUNDED, "square");
        // The golden here is the unit square's own circumradius.
        let r = std::f64::consts::SQRT_2;
        assert_bounds(unrounded.max_bounds(), [-r, -r, r, r], "square max bounds");

        let rounded =
            RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::new(0.5, 0.2), None, Point::ZERO);
        assert_matches_golden(&rounded, &SQUARE_ROUNDED_SMOOTHED, "rounded square");
        assert_eq!(
            kinds(&rounded),
            [
                FeatureKind::ConvexCorner,
                FeatureKind::Edge,
                FeatureKind::ConvexCorner,
                FeatureKind::Edge,
                FeatureKind::ConvexCorner,
                FeatureKind::Edge,
                FeatureKind::ConvexCorner,
                FeatureKind::Edge,
            ]
        );
    }

    #[test]
    fn parity_circle() {
        let circle = RoundedPolygon::circle(8, 1.0, Point::ZERO);
        assert_matches_golden(&circle, &CIRCLE_8, "circle");
    }

    #[test]
    fn parity_star() {
        let star = RoundedPolygon::star(StarParams {
            rounding: CornerRounding::new(0.3, 0.4),
            inner_rounding: Some(CornerRounding::radius(0.2)),
            ..StarParams::default()
        });
        assert_matches_golden(&star, &STAR_5, "star");
        // Outer points are convex, inner ones concave, edges between.
        let want: Vec<FeatureKind> = (0..5)
            .flat_map(|_| {
                [
                    FeatureKind::ConvexCorner,
                    FeatureKind::Edge,
                    FeatureKind::ConcaveCorner,
                    FeatureKind::Edge,
                ]
            })
            .collect();
        assert_eq!(kinds(&star), want);
    }

    #[test]
    fn parity_pill_and_pill_star() {
        let pill = RoundedPolygon::pill(2.0, 1.0, 0.3, Point::ZERO);
        assert_matches_golden(&pill, &PILL, "pill");
        assert_bounds(pill.bounds(), [-1.0, -0.5, 1.0, 0.5], "pill bounds");

        // The pill star's 49 cubics are pinned structurally rather than
        // point-by-point: feature kinds, cubic count, and bounds together move
        // if any part of the perimeter walk drifts.
        let pill_star = RoundedPolygon::pill_star(PillStarParams {
            rounding: CornerRounding::new(0.2, 0.1),
            inner_rounding: Some(CornerRounding::radius(0.1)),
            ..PillStarParams::default()
        });
        assert_eq!(pill_star.cubics().len(), 49);
        assert_eq!(pill_star.features().len(), 32);
        assert_bounds(
            pill_star.bounds(),
            [
                -1.258_127_058_178_750_3,
                -0.910_050_473_510_705_7,
                1.221_825_670_243_362_5,
                0.910_050_473_510_705_7,
            ],
            "pill star bounds",
        );
    }

    #[test]
    fn parity_normalized_arch() {
        let round_100 = CornerRounding::radius(1.0);
        let round_20 = CornerRounding::radius(0.2);
        let arch = RoundedPolygon::from_num_vertices(
            4,
            1.0,
            Point::ZERO,
            CornerRounding::UNROUNDED,
            Some(&[round_100, round_100, round_20, round_20]),
        )
        .normalized();

        assert_matches_golden(&arch, &NORMALIZED_ARCH, "normalized arch");
        assert_bounds(
            arch.bounds(),
            [0.020_152_542_840_087_787, 0.0, 0.979_847_457_159_912_4, 1.0],
            "arch bounds",
        );
        assert!((arch.center().x - 0.567_016_946_674_317).abs() < GOLDEN_EPSILON);
        assert!((arch.center().y - 0.546_864_403_834_229_2).abs() < GOLDEN_EPSILON);
    }
}
