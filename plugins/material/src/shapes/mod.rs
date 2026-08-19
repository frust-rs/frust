//! Feature-point rounded-polygon geometry: the substrate the Material 3
//! Expressive shape catalog and its feature-matched morphing are built on.
//!
//! A [`RoundedPolygon`] is a closed outline described **twice over**: as a flat
//! list of [`Cubic`] curves (what gets painted, via [`RoundedPolygon::to_path`])
//! and as an ordered list of [`Feature`]s — corners and edges — grouping those
//! cubics into the semantic units a morph matches on. Keeping both is the whole
//! point of the model: rounding a rectangle adds a dozen cubics but leaves it
//! with exactly four corners, and a morph that maps corner-to-corner (rather
//! than cubic-to-cubic) is what makes the M3 Expressive shape transitions read
//! as one shape becoming another instead of a soup of curves.
//!
//! # Upstream
//!
//! Ported from **`material_new_shapes` 1.0.0** (MIT, Agbama Gifted) — itself a
//! Dart port of AndroidX `graphics-shapes`. The files ported here are
//! `lib/src/shapes/`'s `corner_rounding.dart`, `cubic.dart`, `features.dart`,
//! `point.dart`, `rounded_polygon.dart`, and the polygon-relevant half of
//! `utils.dart`. The morph half (`morph.dart`, `feature_mapping.dart`,
//! `polygon_measure.dart`, `float_mapping.dart`) is ported alongside it as
//! [`Morph`], which layers on top of these types without reopening them and
//! carries its own name map and deviation list.
//!
//! ## Dart → Rust name map
//!
//! | Upstream (Dart) | Here (Rust) | Note |
//! |---|---|---|
//! | `Point` | [`kurbo::Point`] / [`kurbo::Vec2`] | upstream's `Point` is both position and vector; this port splits them the way kurbo does |
//! | `PointTransformer` | `impl Fn(Point) -> Point` | see `transformed_with`; [`kurbo::Affine`] covers the affine cases via `transformed` |
//! | `Cubic` | [`Cubic`] | `_points` → `points`, `Cubic.fromPoints` → [`Cubic::new`], `Cubic.empty` → [`Cubic::empty`] |
//! | `Cubic.reverse()` | [`Cubic::reversed`] | renamed for Rust's `-ed` convention |
//! | `Feature`/`EdgeFeature`/`CornerFeature` | [`Feature`] + [`FeatureKind`] | one struct with a kind tag instead of a class hierarchy |
//! | `Feature.buildEdge` / `buildIgnorableFeature` / `buildConvexCorner` / `buildConcaveCorner` | [`Feature::edge`] / [`Feature::ignorable`] / [`Feature::convex_corner`] / [`Feature::concave_corner`] | |
//! | `CornerRounding` | [`CornerRounding`] | `CornerRounding.unrounded` → [`CornerRounding::UNROUNDED`] |
//! | `RoundedPolygon.fromVertices` | [`RoundedPolygon::from_vertices`] | takes `&[Point]`, not a flat `List<double>` |
//! | `RoundedPolygon.fromVerticesNum` | [`RoundedPolygon::from_num_vertices`] | |
//! | `RoundedPolygon.fromFeatures` | [`RoundedPolygon::from_features`] | |
//! | `RoundedPolygon.circle/rectangle/star/pill/pillStar` | [`RoundedPolygon::circle`]/[`rectangle`](RoundedPolygon::rectangle)/[`star`](RoundedPolygon::star)/[`pill`](RoundedPolygon::pill)/[`pill_star`](RoundedPolygon::pill_star) | `star`/`pill_star` take a params struct (see below) |
//! | `calculateBounds(approximate: true/false)` | [`RoundedPolygon::bounds`] / [`RoundedPolygon::exact_bounds`] | split in two rather than taking a flag |
//! | `calculateMaxBounds` | [`RoundedPolygon::max_bounds`] | |
//! | `RoundedPolygonToPathExtension.toPath` | [`RoundedPolygon::to_path`] | emits a closed [`kurbo::BezPath`] |
//! | `distanceEpsilon` | [`DISTANCE_EPSILON`] | |
//!
//! ## Deliberate deviations
//!
//! Everything else is a structure-preserving port — same pipeline, same
//! formulas, same constants. The five places this port does **not** mirror
//! upstream, each for a stated reason:
//!
//! 1. **Named-parameter constructors.** Dart's optional named parameters have
//!    no Rust equivalent. Constructors up to five arguments take them
//!    positionally; the two parameter-rich ones take a `Default`-carrying
//!    params struct ([`StarParams`], [`PillStarParams`]) whose defaults are
//!    upstream's own.
//! 2. **`Option` instead of sentinel floats.** Upstream signals "no explicit
//!    centre" with `double.minPositive` (`fromVertices`) or `double.nan`
//!    (`fromFeatures`); here it is `Option<Point>`.
//! 3. **`±∞` bounds seeds.** [`RoundedPolygon::bounds`] seeds its running
//!    maximum with `f64::NEG_INFINITY`, where upstream seeds with
//!    `double.minPositive` (≈`5e-324`, a *positive* value) — which silently
//!    under-reports the bounds of a shape lying entirely in negative
//!    coordinates. This is a bug fix, and the only numeric divergence.
//! 4. **Contract violations panic** (with a documented `# Panics` section)
//!    rather than returning a typed error: upstream throws `ArgumentError`, and
//!    every argument involved is a shape-definition constant, not runtime
//!    input. Polygon contiguity is checked with `debug_assert!`, mirroring
//!    upstream's debug-only `assert`; feature continuity is checked
//!    unconditionally, mirroring upstream's unconditional throw.
//! 5. **Write-only fields dropped.** Upstream's private corner helper keeps
//!    `cosAngle`/`sinAngle`/`center` as fields that nothing ever reads back;
//!    they are locals here.
//!
//! Full attribution ships in the crate's `NOTICE`.

mod morph;
mod rounded_polygon;

pub use morph::Morph;
pub use rounded_polygon::{
    CornerRounding, Cubic, DISTANCE_EPSILON, Feature, FeatureKind, PillStarParams, RoundedPolygon,
    StarParams,
};
