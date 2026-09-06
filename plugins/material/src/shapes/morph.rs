//! Feature-matched morphing between two [`RoundedPolygon`]s: the arc-length
//! measurer, the corner-to-corner feature mapping, and [`Morph`] itself.
//!
//! Ported from `material_new_shapes` 1.0.0's `lib/src/shapes/` (`morph.dart`,
//! `feature_mapping.dart`, `polygon_measure.dart`, `float_mapping.dart`) — the
//! half [`super::rounded_polygon`] deliberately left alone. Attribution and the
//! polygon-side name map live in the [parent module's docs](super).
//!
//! # How a morph is built
//!
//! Interpolating two outlines needs a curve-to-curve correspondence, and two
//! polygons rarely have the same number of curves in the same places. The
//! pipeline that produces one, in order:
//!
//! 1. **Measure.** Every cubic of each polygon gets an *outline progress*
//!    range: where it starts and ends as a fraction of the whole perimeter
//!    (arc length, approximated by [`MEASURE_SEGMENTS`] chords). Zero-length
//!    cubics — every unrounded corner is one — drop out here.
//! 2. **Map features.** Each corner ([`Feature::is_corner`]) carries the
//!    progress of its midpoint. Corners of the two shapes are paired greedily
//!    by proximity, closest first, never pairing a convex corner with a
//!    concave one and never introducing a crossing. The pairs define a
//!    [`DoubleMapper`]: a piecewise-linear, wrap-around bijection from
//!    progress on the start shape to progress on the end shape.
//! 3. **Cut and shift.** Progress `0` on the start shape maps to some progress
//!    on the end shape; the end shape is re-cut to begin there, so both
//!    outlines can be walked together from `0`.
//! 4. **Match.** Walking both, whichever curve ends first sets the boundary;
//!    the curve that overruns is split there. The result is a list of
//!    (start-curve, end-curve) pairs covering both outlines exactly.
//!
//! [`Morph::path`] then interpolates each pair element-wise on the flat
//! eight-float [`Cubic`] layout. Steps 1–4 all happen in [`Morph::new`], which
//! is why a morph is built once and kept, not rebuilt per frame.
//!
//! ## Dart → Rust name map
//!
//! | Upstream (Dart) | Here (Rust) | Note |
//! |---|---|---|
//! | `Morph` | [`Morph`] | `_start`/`_end`/`_morphMatch` → `start`/`end`/`pairs` |
//! | `Morph.asCubics` | [`Morph::as_cubics`] | |
//! | `MorphToPathExtension.toPath` | [`Morph::path`] | emits a closed [`kurbo::BezPath`]; the `startAngle`/`repeatPath` options are not ported (see deviations) |
//! | `Morph.calculateBounds(approximate:)` | [`Morph::bounds`] / [`Morph::exact_bounds`] | split in two rather than taking a flag, as the polygon's are |
//! | `Morph.calculateMaxBounds` | [`Morph::max_bounds`] | |
//! | `Measurer` / `LengthMeasurer` | [`measure_cubic`] + [`find_cubic_cut_point`] | one implementation, so the interface is collapsed (see deviations) |
//! | `MeasuredPolygon` / `MeasuredCubic` | [`MeasuredPolygon`] / [`MeasuredCubic`] | |
//! | `MeasuredPolygon.cutAndShift` | [`MeasuredPolygon::cut_and_shift`] | consumes and returns the polygon |
//! | `ProgressableFeature` | [`ProgressableFeature`] | holds the feature's kind and representative point rather than the feature itself |
//! | `featureMapper` / `doMapping` | [`feature_mapper`] / [`do_mapping`] | |
//! | `featureDistSquared` / `featureRepresentativePoint` | [`feature_dist_squared`] / [`feature_representative_point`] | |
//! | `DoubleMapper` / `linearMap` | [`DoubleMapper`] / [`linear_map`] | |
//! | `progressInRange` / `progressDistance` / `validateProgress` | [`progress_in_range`] / [`progress_distance`] / [`validate_progress`] | |
//! | `positiveModulo` | [`positive_modulo`] | |
//! | `angleEpsilon` | [`ANGLE_EPSILON`] | |
//!
//! ## Deliberate deviations
//!
//! Everything else is a structure-preserving port — same pipeline, same
//! formulas, same constants. Where this port does **not** mirror upstream:
//!
//! 1. **The `Measurer` interface is collapsed into its one implementation.**
//!    Upstream declares an interface and passes a `LengthMeasurer` instance
//!    everywhere; nothing else ever implements it and `Morph` hardcodes that
//!    choice, so the two methods are free functions here.
//! 2. **`Option` instead of a sentinel distance.** `featureDistSquared`
//!    returns `double.maxFinite` to mean "never map these two"; here it is
//!    [`None`].
//! 3. **A zero-length chord at a zero remaining measure cuts at that point,
//!    rather than dividing `0 / 0`.** Upstream's cut-point search evaluates
//!    `remainder / segment` with both zero when a cut lands exactly on a
//!    curve's start, yielding a `NaN` parameter that propagates into the split
//!    curve; the whole remaining measure is reached at that point, so the
//!    ratio is `1` and the cut is at the segment's own progress.
//! 4. **Contract violations panic** (with a documented `# Panics` section)
//!    rather than returning a typed error, exactly as the polygon side does —
//!    upstream throws `ArgumentError`/`StateError` at each of these points.
//!    Structural invariants upstream asserts (debug-only in Dart) are
//!    `debug_assert!`s here.
//! 5. **Ties in the feature-pair ordering resolve in generation order.** The
//!    candidate pairs are stably sorted by distance, so equidistant pairs keep
//!    their `(start feature, end feature)` enumeration order; Dart's
//!    `List.sort` is unstable, so a tie can resolve either way there. Only
//!    exactly-tied pairs are affected, and only in which of them is mapped
//!    first.
//! 6. **A morph's bounds inherit the polygon side's `±∞` seeding fix**
//!    (the parent module's deviation 3), so [`Morph::bounds`] can be tighter
//!    than upstream's for a shape lying in negative coordinates.
//! 7. **`toPath`'s `startAngle`/`repeatPath`/`closePath` options are not
//!    ported.** [`Morph::path`] always emits the whole closed outline, like
//!    [`RoundedPolygon::to_path`]; a caller wanting a rotation applies an
//!    [`kurbo::Affine`] to the path, and the progressive-stroke cases those
//!    flags serve have no consumer here.

use kurbo::{BezPath, Point, Rect};

use super::{Cubic, DISTANCE_EPSILON, Feature, FeatureKind, RoundedPolygon};

/// Below this difference in outline progress, two positions on an outline count
/// as the same one and no curve is cut between them (upstream's
/// `angleEpsilon`).
const ANGLE_EPSILON: f64 = 1e-6;

/// How many chords the arc length of one cubic is approximated by.
///
/// Upstream's `LengthMeasurer._segments`, with its measured claim: three chords
/// hold the error under 1.5% of the true arc length for a circular arc, which
/// is the worst case among the shapes this engine builds. The morph only ever
/// compares measurements against each other, so a consistent slight
/// under-measure costs nothing.
const MEASURE_SEGMENTS: usize = 3;

// ---------------------------------------------------------------------------
// Progress arithmetic (float_mapping.dart, utils.dart)
// ---------------------------------------------------------------------------

/// `value` modulo `modulus`, always in `0..modulus`.
///
/// [`f64::rem_euclid`] can return the modulus itself for a tiny negative input
/// (`-1e-18` rounds up to a bare `1.0`), which the `[0, 1)` progress contract
/// forbids; the extra fold matches what Dart's own double `%` yields there.
fn positive_modulo(value: f64, modulus: f64) -> f64 {
    let remainder = value.rem_euclid(modulus);
    if remainder >= modulus { 0.0 } else { remainder }
}

/// The distance between two progress values, the short way round: `0.99` and
/// `0.0` are `0.01` apart.
fn progress_distance(p1: f64, p2: f64) -> f64 {
    let diff = (p1 - p2).abs();
    diff.min(1.0 - diff)
}

/// Whether `progress` lies in the range running from `from` to `to`.
///
/// Progress wraps, so a range whose end is below its start is the one crossing
/// `0`: `0.7..0.2` holds both `0.8` and `0.1`, but not `0.5`.
fn progress_in_range(progress: f64, from: f64, to: f64) -> bool {
    if to >= from {
        progress >= from && progress <= to
    } else {
        progress >= from || progress <= to
    }
}

/// Interpolates `x` through the piecewise-linear, wrapping map that sends each
/// `x_values[i]` to `y_values[i]`.
///
/// # Panics
///
/// If `x` falls in no segment, which cannot happen for a validated mapping
/// (the segments cover the whole circle). Debug-asserts `x` is in `0..=1`.
fn linear_map(x_values: &[f64], y_values: &[f64], x: f64) -> f64 {
    debug_assert!((0.0..=1.0).contains(&x), "progress out of range: {x}");

    let start = (0..x_values.len())
        .find(|&i| progress_in_range(x, x_values[i], x_values[(i + 1) % x_values.len()]))
        .expect("a validated mapping covers every progress value");
    let end = (start + 1) % x_values.len();

    let segment_size_x = positive_modulo(x_values[end] - x_values[start], 1.0);
    let segment_size_y = positive_modulo(y_values[end] - y_values[start], 1.0);
    // A segment too short to divide by: take its midpoint rather than amplify
    // the round-off.
    let position_in_segment = if segment_size_x < 0.001 {
        0.5
    } else {
        positive_modulo(x - x_values[start], 1.0) / segment_size_x
    };

    positive_modulo(y_values[start] + segment_size_y * position_in_segment, 1.0)
}

/// A bijection between two `0..1` progress spaces, given by a finite list of
/// corresponding values and extended by linear interpolation with wrap-around.
///
/// Given the pairs `(0.2, 0.5)` and `(0.4, 0.6)`, source `0.3` — halfway along
/// the source interval — maps to `0.55`, halfway along the target one.
#[derive(Clone, Debug)]
struct DoubleMapper {
    source: Vec<f64>,
    target: Vec<f64>,
}

impl DoubleMapper {
    /// The mapper defined by `mappings`, each pair a (source, target) value.
    ///
    /// # Panics
    ///
    /// If either side is empty, holds a value outside `0..1`, repeats a value,
    /// or wraps more than once (see [`validate_progress`]).
    fn new(mappings: &[(f64, f64)]) -> DoubleMapper {
        let source: Vec<f64> = mappings.iter().map(|m| m.0).collect();
        let target: Vec<f64> = mappings.iter().map(|m| m.1).collect();
        validate_progress(&source, "source");
        validate_progress(&target, "target");
        DoubleMapper { source, target }
    }

    /// The target-space value corresponding to source-space `x`.
    fn map(&self, x: f64) -> f64 {
        linear_map(&self.source, &self.target, x)
    }

    /// The source-space value corresponding to target-space `x`.
    fn map_back(&self, x: f64) -> f64 {
        linear_map(&self.target, &self.source, x)
    }
}

/// Checks that progress values are in `0..1`, distinct, and monotonically
/// increasing with at most one wrap around `0`.
///
/// # Panics
///
/// If any of that fails — a mapping that repeats or double-wraps has no
/// well-defined inverse.
fn validate_progress(values: &[f64], side: &str) {
    assert!(
        !values.is_empty(),
        "a {side} mapping needs at least one value"
    );

    let mut previous = values[values.len() - 1];
    let mut wraps = 0_u32;
    for &current in values {
        assert!(
            (0.0..1.0).contains(&current),
            "{side} progress outside 0..1: {values:?}"
        );
        assert!(
            progress_distance(current, previous) > DISTANCE_EPSILON,
            "{side} progress repeats a value: {values:?}"
        );
        if current < previous {
            wraps += 1;
            assert!(
                wraps <= 1,
                "{side} progress wraps more than once: {values:?}"
            );
        }
        previous = current;
    }
}

// ---------------------------------------------------------------------------
// Measuring (polygon_measure.dart)
// ---------------------------------------------------------------------------

/// The length of `cubic`, approximated by [`MEASURE_SEGMENTS`] chords.
fn measure_cubic(cubic: &Cubic) -> f64 {
    closest_progress_to(cubic, f64::INFINITY).1
}

/// The parameter `t` at which `cubic` has covered `measure` of its length,
/// capped at its full length.
fn find_cubic_cut_point(cubic: &Cubic, measure: f64) -> f64 {
    closest_progress_to(cubic, measure).0
}

/// Walks `cubic` in [`MEASURE_SEGMENTS`] chords, returning the parameter at
/// which `threshold` length is reached and the threshold itself — or, if the
/// curve is shorter than that, `(1, its length)`.
///
/// One walk answers both of the measurer's questions: `f64::INFINITY` is never
/// reached, so it returns the total length.
fn closest_progress_to(cubic: &Cubic, threshold: f64) -> (f64, f64) {
    let segments = MEASURE_SEGMENTS as f64;
    let mut total = 0.0;
    let mut remainder = threshold;
    let mut previous = cubic.anchor0();

    for i in 0..=MEASURE_SEGMENTS {
        let progress = i as f64 / segments;
        let point = cubic.point_on_curve(progress);
        let segment = (point - previous).hypot();

        if segment >= remainder {
            // A zero-length chord is only ever "reached" with nothing left to
            // measure, and then the cut is exactly here.
            let covered = if segment > 0.0 {
                remainder / segment
            } else {
                1.0
            };
            return (progress - (1.0 - covered) / segments, threshold);
        }

        remainder -= segment;
        total += segment;
        previous = point;
    }

    (1.0, total)
}

/// One cubic of an outline, with the stretch of that outline it covers.
///
/// Outline progress is the fraction of the whole perimeter travelled, so two
/// shapes' curves can be compared by where they sit rather than by index.
#[derive(Clone, Copy, Debug)]
struct MeasuredCubic {
    cubic: Cubic,
    measured_size: f64,
    start_outline_progress: f64,
    end_outline_progress: f64,
}

impl MeasuredCubic {
    fn new(cubic: Cubic, start_outline_progress: f64, end_outline_progress: f64) -> MeasuredCubic {
        debug_assert!(
            (0.0..=1.0).contains(&start_outline_progress)
                && (0.0..=1.0).contains(&end_outline_progress)
                && end_outline_progress >= start_outline_progress,
            "outline progress range must be ordered and within 0..=1, got \
             {start_outline_progress}..{end_outline_progress}"
        );
        MeasuredCubic {
            cubic,
            measured_size: measure_cubic(&cubic),
            start_outline_progress,
            end_outline_progress,
        }
    }

    /// Splits this curve at an outline progress value, into the part before it
    /// and the part after.
    ///
    /// The cut is clamped into this curve's own range first: round-off further
    /// up can put it a hair outside, and compounding that error is worse than
    /// cutting at the very end.
    ///
    /// # Panics
    ///
    /// If the cut lands outside the curve's parameter range, which a clamped
    /// cut cannot.
    fn cut_at_progress(&self, cut_outline_progress: f64) -> (MeasuredCubic, MeasuredCubic) {
        let bounded =
            cut_outline_progress.clamp(self.start_outline_progress, self.end_outline_progress);
        let progress_size = self.end_outline_progress - self.start_outline_progress;
        let progress_from_start = bounded - self.start_outline_progress;

        // Empty curves are filtered out before any cut, so this cannot divide
        // by zero.
        let relative_progress = progress_from_start / progress_size;
        let t = find_cubic_cut_point(&self.cubic, relative_progress * self.measured_size);
        assert!(
            (0.0..=1.0).contains(&t),
            "a cubic cut point must be in 0..=1, got {t}"
        );

        let (before, after) = self.cubic.split(t);
        (
            MeasuredCubic::new(before, self.start_outline_progress, bounded),
            MeasuredCubic::new(after, bounded, self.end_outline_progress),
        )
    }
}

/// A polygon's corner, reduced to what the feature mapping needs: where it sits
/// along the outline, where it sits in space, and which way it indents.
#[derive(Clone, Copy, Debug)]
struct ProgressableFeature {
    progress: f64,
    representative: Point,
    kind: FeatureKind,
}

/// The point standing in for a whole feature when measuring how far apart two
/// of them are: the midpoint of its first and last anchors.
fn feature_representative_point(feature: &Feature) -> Point {
    let cubics = feature.cubics();
    let first = cubics[0].anchor0();
    let last = cubics[cubics.len() - 1].anchor1();
    Point::new((first.x + last.x) / 2.0, (first.y + last.y) / 2.0)
}

/// How far apart two features are, squared — or [`None`] when they must never
/// be mapped to each other.
///
/// Convex maps only to convex and concave only to concave: pairing a star's
/// outer point with another star's inner one turns the morph inside out.
fn feature_dist_squared(f1: &ProgressableFeature, f2: &ProgressableFeature) -> Option<f64> {
    if f1.kind != f2.kind {
        return None;
    }
    Some((f1.representative - f2.representative).hypot2())
}

/// A polygon's outline, cut into curves that each know where they sit along it,
/// plus the progress of every corner.
#[derive(Clone, Debug)]
struct MeasuredPolygon {
    cubics: Vec<MeasuredCubic>,
    features: Vec<ProgressableFeature>,
}

impl MeasuredPolygon {
    /// Measures `polygon`'s outline.
    ///
    /// The curves come from the polygon's [`Feature`]s rather than its flat
    /// cubic list, since a feature's cubics are what a corner's progress is
    /// measured against; the two describe the same closed outline.
    ///
    /// # Panics
    ///
    /// If the outline has no measurable length at all (a fully degenerate
    /// polygon), since every progress value would then be `0/0`.
    fn measure(polygon: &RoundedPolygon) -> MeasuredPolygon {
        let mut cubics: Vec<Cubic> = Vec::new();
        // Each corner is represented by the cubic in the middle of its run —
        // the inner arc of a rounded corner.
        let mut corner_cubic_indices: Vec<(usize, &Feature)> = Vec::new();

        for feature in polygon.features() {
            let representative = feature.cubics().len() / 2;
            for (i, cubic) in feature.cubics().iter().enumerate() {
                if feature.is_corner() && i == representative {
                    corner_cubic_indices.push((cubics.len(), feature));
                }
                cubics.push(*cubic);
            }
        }

        let mut measures = vec![0.0; cubics.len() + 1];
        let mut total_measure = 0.0;
        for (i, cubic) in cubics.iter().enumerate() {
            let measure = measure_cubic(cubic);
            debug_assert!(measure >= 0.0, "a measured cubic cannot be negative");
            total_measure += measure;
            measures[i + 1] = total_measure;
        }
        assert!(
            total_measure > 0.0,
            "a polygon with no measurable outline cannot be morphed"
        );

        let outline_progress: Vec<f64> = measures.iter().map(|m| m / total_measure).collect();
        let features = corner_cubic_indices
            .iter()
            .map(|&(index, feature)| ProgressableFeature {
                progress: positive_modulo(
                    (outline_progress[index] + outline_progress[index + 1]) / 2.0,
                    1.0,
                ),
                representative: feature_representative_point(feature),
                kind: feature.kind(),
            })
            .collect();

        MeasuredPolygon::assemble(features, &cubics, &outline_progress)
    }

    /// Builds the measured outline from curves and the progress boundaries
    /// between them, dropping the empty ones.
    ///
    /// # Panics
    ///
    /// If every curve is empty. Debug-asserts that the boundaries run from `0`
    /// to `1` with one more of them than there are curves.
    fn assemble(
        features: Vec<ProgressableFeature>,
        cubics: &[Cubic],
        outline_progress: &[f64],
    ) -> MeasuredPolygon {
        debug_assert!(
            outline_progress.len() == cubics.len() + 1,
            "expected one more outline-progress value than curves"
        );
        debug_assert!(
            outline_progress[0] == 0.0 && outline_progress[outline_progress.len() - 1] == 1.0,
            "outline progress must run from 0 to 1"
        );

        let mut measured: Vec<MeasuredCubic> = Vec::with_capacity(cubics.len());
        let mut start = 0.0;
        for (i, cubic) in cubics.iter().enumerate() {
            if outline_progress[i + 1] - outline_progress[i] > DISTANCE_EPSILON {
                measured.push(MeasuredCubic::new(*cubic, start, outline_progress[i + 1]));
                // The next curve starts exactly where this one ends, closing
                // the gaps the dropped curves left.
                start = outline_progress[i + 1];
            }
        }

        // Trailing curves may have been dropped; the outline still has to end
        // at 1.
        measured
            .last_mut()
            .expect("a measurable outline has at least one non-empty curve")
            .end_outline_progress = 1.0;

        MeasuredPolygon {
            cubics: measured,
            features,
        }
    }

    /// The same outline, re-cut to start at `cutting_point`.
    ///
    /// Given curves over `[0, 0.2] [0.2, 0.5] [0.5, 1]` and a cut at `0.4`, the
    /// second curve splits and the result runs `[0, 0.1] [0.1, 0.6] [0.6, 0.8]
    /// [0.8, 1]` — the same closed outline, walked from a different point.
    ///
    /// # Panics
    ///
    /// If `cutting_point` is outside `0..=1`, or lands in none of the curves.
    fn cut_and_shift(self, cutting_point: f64) -> MeasuredPolygon {
        assert!(
            (0.0..=1.0).contains(&cutting_point),
            "a cutting point must be in 0..=1, got {cutting_point}"
        );
        if cutting_point < DISTANCE_EPSILON {
            return self;
        }

        let target_index = self
            .cubics
            .iter()
            .position(|c| {
                cutting_point >= c.start_outline_progress && cutting_point <= c.end_outline_progress
            })
            .expect("the cutting point lies on the outline");
        let (before, after) = self.cubics[target_index].cut_at_progress(cutting_point);

        // The part of the target curve after the cut, then everything from
        // there round to the target again, then the part before the cut.
        let mut cubics = Vec::with_capacity(self.cubics.len() + 2);
        cubics.push(after.cubic);
        for i in 1..self.cubics.len() {
            cubics.push(self.cubics[(i + target_index) % self.cubics.len()].cubic);
        }
        cubics.push(before.cubic);

        // Every boundary moves back by the cut, wrapping; the two ends are the
        // cut itself.
        let mut outline_progress = Vec::with_capacity(cubics.len() + 1);
        for i in 0..=cubics.len() {
            if i == 0 {
                outline_progress.push(0.0);
            } else if i == cubics.len() {
                outline_progress.push(1.0);
            } else {
                let index = (target_index + i - 1) % self.cubics.len();
                outline_progress.push(positive_modulo(
                    self.cubics[index].end_outline_progress - cutting_point,
                    1.0,
                ));
            }
        }

        let features = self
            .features
            .iter()
            .map(|f| ProgressableFeature {
                progress: positive_modulo(f.progress - cutting_point, 1.0),
                ..*f
            })
            .collect();

        MeasuredPolygon::assemble(features, &cubics, &outline_progress)
    }

    fn get(&self, index: usize) -> Option<MeasuredCubic> {
        self.cubics.get(index).copied()
    }
}

// ---------------------------------------------------------------------------
// Feature mapping (feature_mapping.dart)
// ---------------------------------------------------------------------------

/// The progress mapping between two shapes, built from their corners alone.
fn feature_mapper(
    features1: &[ProgressableFeature],
    features2: &[ProgressableFeature],
) -> DoubleMapper {
    let corners = |features: &[ProgressableFeature]| -> Vec<ProgressableFeature> {
        features
            .iter()
            .copied()
            .filter(|f| f.kind != FeatureKind::Edge)
            .collect()
    };
    DoubleMapper::new(&do_mapping(&corners(features1), &corners(features2)))
}

/// Pairs the corners of two shapes, returning (start progress, end progress)
/// pairs sorted by the first element.
///
/// Every possible pair is scored by distance and taken in ascending order,
/// skipping any that would map an already-mapped corner, sit within
/// [`DISTANCE_EPSILON`] of an existing pair (which would make the mapping
/// unstable), or cross an existing pair.
///
/// # Panics
///
/// If two corners of the same shape share an outline progress value.
fn do_mapping(
    features1: &[ProgressableFeature],
    features2: &[ProgressableFeature],
) -> Vec<(f64, f64)> {
    let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
    for (i1, f1) in features1.iter().enumerate() {
        for (i2, f2) in features2.iter().enumerate() {
            if let Some(distance) = feature_dist_squared(f1, f2) {
                candidates.push((distance, i1, i2));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));

    // Nothing to go on: map progress straight through.
    let Some(&(_, first1, first2)) = candidates.first() else {
        return vec![(0.0, 0.0), (0.5, 0.5)];
    };
    // One pair fixes an offset but not a scale; the opposite point pins it.
    if candidates.len() == 1 {
        let (p1, p2) = (features1[first1].progress, features2[first2].progress);
        return vec![(p1, p2), ((p1 + 0.5) % 1.0, (p2 + 0.5) % 1.0)];
    }

    let mut helper = MappingHelper::new(features1.len(), features2.len());
    for &(_, i1, i2) in &candidates {
        helper.add(features1[i1].progress, features2[i2].progress, i1, i2);
    }
    helper.mapping
}

/// Accumulates the corner pairing, keeping the mapping sorted by start progress
/// and free of crossings.
struct MappingHelper {
    mapping: Vec<(f64, f64)>,
    used1: Vec<bool>,
    used2: Vec<bool>,
}

impl MappingHelper {
    fn new(len1: usize, len2: usize) -> MappingHelper {
        MappingHelper {
            mapping: Vec::new(),
            used1: vec![false; len1],
            used2: vec![false; len2],
        }
    }

    fn add(&mut self, p1: f64, p2: f64, i1: usize, i2: usize) {
        // A corner is mapped once.
        if self.used1[i1] || self.used2[i2] {
            return;
        }

        let insertion = match self
            .mapping
            .binary_search_by(|probe| probe.0.total_cmp(&p1))
        {
            Ok(_) => panic!("two features cannot share the outline progress {p1}"),
            Err(index) => index,
        };

        let n = self.mapping.len();
        if n >= 1 {
            let (before1, before2) = self.mapping[(insertion + n - 1) % n];
            let (after1, after2) = self.mapping[insertion % n];

            // Pairs packed too closely together make the mapper unstable.
            if progress_distance(p1, before1) < DISTANCE_EPSILON
                || progress_distance(p1, after1) < DISTANCE_EPSILON
                || progress_distance(p2, before2) < DISTANCE_EPSILON
                || progress_distance(p2, after2) < DISTANCE_EPSILON
            {
                return;
            }

            // With two or more pairs already placed, the new one has to land
            // between its neighbours on the *target* side too, or the outlines
            // would cross while morphing.
            if n > 1 && !progress_in_range(p2, before2, after2) {
                return;
            }
        }

        self.mapping.insert(insertion, (p1, p2));
        self.used1[i1] = true;
        self.used2[i2] = true;
    }
}

// ---------------------------------------------------------------------------
// Morph (morph.dart)
// ---------------------------------------------------------------------------

/// An interpolation between two [`RoundedPolygon`]s: at any progress `t` it
/// yields one closed outline, the start shape at `0` and the end shape at `1`.
///
/// # Cost
///
/// **Building one is the expensive step** — measuring both outlines, pairing
/// their corners and re-cutting the curves all happen in [`Morph::new`]. Build
/// a morph once and keep it (in widget state, a `OnceLock`, a cache keyed by
/// the shape pair); [`Morph::path`] is the per-frame call and does nothing but
/// interpolate the stored curve pairs into the path it returns.
#[derive(Clone, Debug)]
pub struct Morph {
    start: RoundedPolygon,
    end: RoundedPolygon,
    /// The two shapes' outlines cut into matching curves, one pair per curve of
    /// the morphed outline. Interpolating every pair by the same progress is
    /// the whole of the animation.
    pairs: Vec<(Cubic, Cubic)>,
}

impl Morph {
    /// Builds the morph from `start` to `end`.
    ///
    /// This measures both outlines, pairs their corners and re-cuts their
    /// curves into matching pairs — see [`Morph`]'s note on cost for why that
    /// belongs outside the frame loop.
    ///
    /// # Panics
    ///
    /// If either polygon has no measurable outline, or if two corners of one
    /// shape sit at the same point along its outline (upstream throws in both
    /// cases; every input here is a shape definition, not runtime data).
    #[must_use]
    pub fn new(start: RoundedPolygon, end: RoundedPolygon) -> Morph {
        let pairs = Morph::match_polygons(&start, &end);
        Morph { start, end, pairs }
    }

    /// The shape at progress `0`.
    #[must_use]
    pub const fn start(&self) -> &RoundedPolygon {
        &self.start
    }

    /// The shape at progress `1`.
    #[must_use]
    pub const fn end(&self) -> &RoundedPolygon {
        &self.end
    }

    /// Matches the two outlines curve for curve, cutting where they disagree.
    fn match_polygons(p1: &RoundedPolygon, p2: &RoundedPolygon) -> Vec<(Cubic, Cubic)> {
        let measured1 = MeasuredPolygon::measure(p1);
        let measured2 = MeasuredPolygon::measure(p2);

        let mapper = feature_mapper(&measured1.features, &measured2.features);

        // Progress 0 on the first shape belongs at this point on the second,
        // so that is where the second shape is re-cut to start.
        let cut_point = mapper.map(0.0);
        let bs1 = measured1;
        let bs2 = measured2.cut_and_shift(cut_point);

        let mut pairs: Vec<(Cubic, Cubic)> = Vec::new();
        // The curve from each shape waiting to be matched, and the index the
        // one after it will come from.
        let mut curve1 = bs1.get(0);
        let mut curve2 = bs2.get(0);
        let mut i1 = 1;
        let mut i2 = 1;

        while let (Some(b1), Some(b2)) = (curve1, curve2) {
            // Where each curve ends, in the *first* shape's progress space.
            // The last curve of either shape ends at 1 by definition, whatever
            // round-off says.
            let b1_end = if i1 == bs1.cubics.len() {
                1.0
            } else {
                b1.end_outline_progress
            };
            let b2_end = if i2 == bs2.cubics.len() {
                1.0
            } else {
                mapper.map_back(positive_modulo(b2.end_outline_progress + cut_point, 1.0))
            };

            // Whichever curve ends first sets the boundary; a curve running
            // past it is cut there, and its remainder matched next time round.
            let boundary = b1_end.min(b2_end);

            let (segment1, remainder1) = if b1_end > boundary + ANGLE_EPSILON {
                let (cut, rest) = b1.cut_at_progress(boundary);
                (cut, Some(rest))
            } else {
                let next = bs1.get(i1);
                i1 += 1;
                (b1, next)
            };

            let (segment2, remainder2) = if b2_end > boundary + ANGLE_EPSILON {
                let (cut, rest) =
                    b2.cut_at_progress(positive_modulo(mapper.map(boundary) - cut_point, 1.0));
                (cut, Some(rest))
            } else {
                let next = bs2.get(i2);
                i2 += 1;
                (b2, next)
            };

            pairs.push((segment1.cubic, segment2.cubic));
            curve1 = remainder1;
            curve2 = remainder2;
        }

        debug_assert!(
            curve1.is_none() && curve2.is_none(),
            "both outlines must be fully matched"
        );

        pairs
    }

    /// The morphed outline at `progress`, as its curves.
    ///
    /// Allocates the returned list; [`Morph::path`] is the cheaper route when
    /// the caller only wants to paint it.
    #[must_use]
    pub fn as_cubics(&self, progress: f64) -> Vec<Cubic> {
        let mut cubics = Vec::with_capacity(self.pairs.len());
        self.for_each_cubic(progress, |cubic| cubics.push(cubic));
        cubics
    }

    /// The morphed outline at `progress` as a closed [`kurbo::BezPath`]: one
    /// `MoveTo`, one `CurveTo` per curve, and a `ClosePath`.
    ///
    /// `progress` is not clamped — a caller feeding a spring value may pass a
    /// little past `0`/`1` to keep the spring's overshoot, as long as it stays
    /// close (far outside the range the outline is undefined).
    ///
    /// This is the per-frame call: it allocates the path it returns and nothing
    /// else.
    #[must_use]
    pub fn path(&self, progress: f64) -> BezPath {
        let mut path = BezPath::new();
        let mut started = false;
        self.for_each_cubic(progress, |cubic| {
            if !started {
                path.move_to(cubic.anchor0());
                started = true;
            }
            path.curve_to(cubic.control0(), cubic.control1(), cubic.anchor1());
        });
        if started {
            path.close_path();
        }
        path
    }

    /// Walks the morphed outline at `progress`, curve by curve.
    ///
    /// The last curve is rebuilt to end exactly on the first one's start
    /// anchor: even a sub-pixel gap between the two shows as a seam when the
    /// shape is filled (the same closure [`RoundedPolygon`] applies to its own
    /// cubic list).
    fn for_each_cubic(&self, progress: f64, mut emit: impl FnMut(Cubic)) {
        let Some((first_start, first_end)) = self.pairs.first() else {
            return;
        };
        let first_anchor = interpolate(first_start, first_end, progress).anchor0();

        let last = self.pairs.len() - 1;
        for (i, (start, end)) in self.pairs.iter().enumerate() {
            let cubic = interpolate(start, end, progress);
            if i == last {
                emit(Cubic::new(
                    cubic.anchor0(),
                    cubic.control0(),
                    cubic.control1(),
                    first_anchor,
                ));
            } else {
                emit(cubic);
            }
        }
    }

    /// The axis-aligned bounds of every anchor and control point of both
    /// shapes — cheap, and never tighter than [`Morph::exact_bounds`].
    ///
    /// This is the size to reserve for a morph: an interpolated control point
    /// is a blend of the two shapes' own, so for any progress in `0..=1` the
    /// outline stays inside the hull these bounds describe (an overshooting
    /// progress can leave it).
    #[must_use]
    pub fn bounds(&self) -> Rect {
        self.start.bounds().union(self.end.bounds())
    }

    /// The true axis-aligned bounds of both shapes' outlines.
    ///
    /// Holds the outline at progress `0` and `1` exactly; in between, an
    /// interpolated control point can pull a curve marginally outside it, which
    /// is why [`Morph::bounds`] is the safer reservation.
    #[must_use]
    pub fn exact_bounds(&self) -> Rect {
        self.start.exact_bounds().union(self.end.exact_bounds())
    }

    /// The smallest square holding either shape in *any* rotation — what a UI
    /// element hosting a spinning morph needs to reserve.
    #[must_use]
    pub fn max_bounds(&self) -> Rect {
        self.start.max_bounds().union(self.end.max_bounds())
    }
}

/// Interpolates two curves element-wise on the flat eight-float layout.
///
/// Upstream's exact `lerp` form — `start * (1 - fraction) + stop * fraction` —
/// which is also what makes the endpoints exact: at `0` every term of the
/// second curve is multiplied by zero, at `1` every term of the first is.
fn interpolate(start: &Cubic, end: &Cubic, progress: f64) -> Cubic {
    let (a, b) = (start.points(), end.points());
    let mut points = [0.0_f64; 8];
    for (i, point) in points.iter_mut().enumerate() {
        *point = a[i] * (1.0 - progress) + b[i] * progress;
    }
    Cubic::from_raw(points)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::{CornerRounding, StarParams};
    use kurbo::{CubicBez, ParamCurve, ParamCurveNearest, PathEl, Point, Shape};

    /// Tolerance for the transcribed Dart goldens. The Dart source prints
    /// shortest-round-trip doubles, so every literal parses back to the exact
    /// `f64` upstream held; anything above pure operation-ordering noise here
    /// is a real divergence.
    const GOLDEN_EPSILON: f64 = 1e-12;

    /// Upstream's own test epsilon (`test/test_utils.dart`), used wherever an
    /// assertion is analytic rather than a recorded value.
    const EPSILON: f64 = 1e-4;

    fn triangle() -> RoundedPolygon {
        RoundedPolygon::from_num_vertices(3, 1.0, Point::ZERO, CornerRounding::UNROUNDED, None)
    }

    fn square() -> RoundedPolygon {
        RoundedPolygon::from_num_vertices(4, 1.0, Point::ZERO, CornerRounding::UNROUNDED, None)
    }

    fn octagon() -> RoundedPolygon {
        RoundedPolygon::from_num_vertices(8, 1.0, Point::ZERO, CornerRounding::UNROUNDED, None)
    }

    fn circle8() -> RoundedPolygon {
        RoundedPolygon::circle(8, 1.0, Point::ZERO)
    }

    fn rounded_square() -> RoundedPolygon {
        RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::new(0.3, 0.5), None, Point::ZERO)
    }

    fn star5() -> RoundedPolygon {
        RoundedPolygon::star(StarParams {
            num_vertices_per_radius: 5,
            inner_radius: 0.5,
            rounding: CornerRounding::radius(0.2),
            ..StarParams::default()
        })
    }

    /// The curves of a path, as `kurbo` segments.
    fn segments(path: &BezPath) -> Vec<CubicBez> {
        let mut start = Point::ZERO;
        let mut previous = Point::ZERO;
        let mut out = Vec::new();
        for element in path.elements() {
            match *element {
                PathEl::MoveTo(p) => {
                    start = p;
                    previous = p;
                }
                PathEl::CurveTo(c0, c1, p) => {
                    out.push(CubicBez::new(previous, c0, c1, p));
                    previous = p;
                }
                PathEl::ClosePath => previous = start,
                other => panic!("unexpected path element {other:?}"),
            }
        }
        out
    }

    /// The distance from `point` to the nearest point of `path`.
    fn distance_to(path: &BezPath, point: Point) -> f64 {
        segments(path)
            .iter()
            .map(|segment| segment.nearest(point, 1e-12).distance_sq.sqrt())
            .fold(f64::INFINITY, f64::min)
    }

    /// Asserts both outlines trace the same curve, by sampling each against the
    /// other (one direction alone would pass for a path covering only part of
    /// the other).
    fn assert_same_outline(a: &BezPath, b: &BezPath, tolerance: f64, what: &str) {
        for (from, to, direction) in [(a, b, "a->b"), (b, a, "b->a")] {
            for segment in segments(from) {
                for i in 0..=8 {
                    let point = segment.eval(f64::from(i) / 8.0);
                    let distance = distance_to(to, point);
                    assert!(
                        distance < tolerance,
                        "{what} ({direction}): {point:?} is {distance} off the other outline"
                    );
                }
            }
        }
    }

    fn assert_cubics_match_golden(cubics: &[Cubic], golden: &[[f64; 8]], name: &str) {
        assert_eq!(cubics.len(), golden.len(), "{name}: curve count drifted");
        for (i, (cubic, want)) in cubics.iter().zip(golden).enumerate() {
            for (j, (got, want)) in cubic.points().iter().zip(want).enumerate() {
                assert!(
                    (got - want).abs() < GOLDEN_EPSILON,
                    "{name}: curve {i} coordinate {j}: expected {want}, got {got}"
                );
            }
        }
    }

    fn assert_close(expected: f64, actual: f64, what: &str) {
        assert!(
            (expected - actual).abs() < EPSILON,
            "{what}: expected {expected}, got {actual}"
        );
    }

    // -----------------------------------------------------------------------
    // Progress arithmetic
    // -----------------------------------------------------------------------

    #[test]
    fn positive_modulo_stays_below_the_modulus() {
        assert_close(1.0, positive_modulo(4.0, 3.0), "4 mod 3");
        assert_close(2.0, positive_modulo(-4.0, 3.0), "-4 mod 3");
        // The case rem_euclid alone gets wrong: a tiny negative rounds up to
        // the modulus itself.
        assert_eq!(positive_modulo(-1e-18, 1.0), 0.0);
    }

    #[test]
    fn progress_range_wraps_around_zero() {
        assert!(progress_in_range(0.5, 0.2, 0.7));
        assert!(!progress_in_range(0.1, 0.2, 0.7));
        // A range whose end is below its start is the one crossing 0.
        assert!(progress_in_range(0.8, 0.7, 0.2));
        assert!(progress_in_range(0.1, 0.7, 0.2));
        assert!(!progress_in_range(0.5, 0.7, 0.2));
    }

    #[test]
    fn progress_distance_takes_the_short_way_round() {
        assert_close(0.01, progress_distance(0.99, 0.0), "0.99 to 0");
        assert_close(0.2, progress_distance(0.3, 0.5), "0.3 to 0.5");
    }

    /// Ported from upstream's `FloatMapping` suite: every mapper must agree
    /// with its closed form in both directions, everywhere.
    fn validate_mapping(mapper: &DoubleMapper, expected: impl Fn(f64) -> f64) {
        for i in 0..10_000 {
            let source = f64::from(i) / 10_000.0;
            let target = expected(source);
            assert_close(target, mapper.map(source), "map");
            assert_close(source, mapper.map_back(target), "map back");
        }
    }

    #[test]
    fn identity_mapping() {
        validate_mapping(&DoubleMapper::new(&[(0.0, 0.0), (0.5, 0.5)]), |x| x);
    }

    #[test]
    fn simple_mapping() {
        // The first half of the source becomes the first quarter of the target.
        validate_mapping(&DoubleMapper::new(&[(0.0, 0.0), (0.5, 0.25)]), |x| {
            if x < 0.5 {
                x / 2.0
            } else {
                (3.0 * x - 1.0) / 2.0
            }
        });
    }

    #[test]
    fn wrapping_mappings_are_still_bijections() {
        // All three are the same "+ 0.5" or identity function, expressed with
        // the wrap on the target, on the source, and on both.
        validate_mapping(&DoubleMapper::new(&[(0.0, 0.5), (0.1, 0.6)]), |x| {
            (x + 0.5) % 1.0
        });
        validate_mapping(&DoubleMapper::new(&[(0.5, 0.0), (0.1, 0.6)]), |x| {
            (x + 0.5) % 1.0
        });
        validate_mapping(
            &DoubleMapper::new(&[(0.5, 0.5), (0.75, 0.75), (0.1, 0.1), (0.49, 0.49)]),
            |x| x,
        );
    }

    #[test]
    fn multiple_point_mapping() {
        validate_mapping(
            &DoubleMapper::new(&[(0.4, 0.2), (0.5, 0.22), (0.0, 0.8)]),
            |x| {
                if x < 0.4 {
                    (0.8 + x) % 1.0
                } else if x < 0.5 {
                    0.2 + (x - 0.4) / 5.0
                } else {
                    // 0.5 of source maps to 0.58 of target, hence the 1.16.
                    0.22 + (x - 0.5) * 1.16
                }
            },
        );
    }

    #[test]
    #[should_panic(expected = "wraps more than once")]
    fn a_target_wrapping_twice_is_rejected() {
        let _ = DoubleMapper::new(&[(0.0, 0.0), (0.3, 0.6), (0.6, 0.3), (0.9, 0.9)]);
    }

    #[test]
    #[should_panic(expected = "wraps more than once")]
    fn a_source_wrapping_twice_is_rejected() {
        let _ = DoubleMapper::new(&[(0.0, 0.0), (0.6, 0.3), (0.3, 0.6), (0.9, 0.9)]);
    }

    // -----------------------------------------------------------------------
    // Measuring
    // -----------------------------------------------------------------------

    /// Upstream's `irregularPolygonMeasure`: the measured curves have to tile
    /// `0..1` in order, and every corner's progress has to land in `0..1`.
    fn assert_measures_cleanly(polygon: &RoundedPolygon) -> MeasuredPolygon {
        let measured = MeasuredPolygon::measure(polygon);
        assert_eq!(measured.cubics[0].start_outline_progress, 0.0);
        assert_eq!(
            measured.cubics[measured.cubics.len() - 1].end_outline_progress,
            1.0
        );
        for (i, cubic) in measured.cubics.iter().enumerate() {
            if i > 0 {
                assert_eq!(
                    measured.cubics[i - 1].end_outline_progress,
                    cubic.start_outline_progress,
                    "curve {i} does not start where {} ended",
                    i - 1
                );
            }
            assert!(cubic.end_outline_progress >= cubic.start_outline_progress);
        }
        for (i, feature) in measured.features.iter().enumerate() {
            assert!(
                (0.0..1.0).contains(&feature.progress),
                "corner {i} has invalid progress {}",
                feature.progress
            );
        }
        measured
    }

    #[test]
    fn a_regular_polygon_measures_one_curve_per_side() {
        for sides in [3, 5, 8, 12, 20] {
            let polygon = RoundedPolygon::from_num_vertices(
                sides,
                1.0,
                Point::ZERO,
                CornerRounding::UNROUNDED,
                None,
            );
            let measured = assert_measures_cleanly(&polygon);
            // The zero-length corner curves drop out, leaving the sides.
            assert_eq!(measured.cubics.len(), sides, "{sides}-gon");
            for (i, cubic) in measured.cubics.iter().enumerate() {
                assert_close(
                    i as f64 / sides as f64,
                    cubic.start_outline_progress,
                    "side start",
                );
            }
        }
    }

    #[test]
    fn rounded_shapes_measure_cleanly() {
        for rounding in [0.15, 0.5, 1.0] {
            let hexagon = RoundedPolygon::from_num_vertices(
                6,
                1.0,
                Point::ZERO,
                CornerRounding::radius(rounding),
                None,
            );
            assert_measures_cleanly(&hexagon);
        }
        assert_measures_cleanly(&circle8());
        assert_measures_cleanly(&rounded_square());
        assert_measures_cleanly(&star5());
    }

    #[test]
    fn the_measurer_approximates_a_circle_within_one_and_a_half_percent() {
        // White box: the measurer walks chords, so a circle — the worst case —
        // comes out short. Upstream pins the same 1.5%.
        let polygon = RoundedPolygon::circle(4, 1.0, Point::ZERO);
        let measured: f64 = polygon.cubics().iter().map(measure_cubic).sum();
        let expected = 2.0 * std::f64::consts::PI;
        assert!(
            (expected - measured).abs() < 0.015 * expected,
            "circle perimeter: expected ~{expected}, measured {measured}"
        );
    }

    #[test]
    fn measured_progress_matches_the_dart_goldens() {
        // Recorded from `MeasuredPolygon.measurePolygon` upstream.
        let measured = MeasuredPolygon::measure(&rounded_square());
        let golden_ends = [
            0.032907413764029836,
            0.06996739408559419,
            0.10287480784962404,
            0.25,
            0.28290741376402984,
            0.3199673940855942,
            0.352874807849624,
            0.5,
            0.5329074137640298,
            0.5699673940855942,
            0.6028748078496241,
            0.75,
            0.7829074137640298,
            0.8199673940855942,
            0.8528748078496241,
            1.0,
        ];
        assert_eq!(measured.cubics.len(), golden_ends.len());
        for (i, (cubic, want)) in measured.cubics.iter().zip(&golden_ends).enumerate() {
            assert!(
                (cubic.end_outline_progress - want).abs() < GOLDEN_EPSILON,
                "curve {i}: expected end {want}, got {}",
                cubic.end_outline_progress
            );
        }

        let golden_corners = [
            0.051437403924812,
            0.301437403924812,
            0.551437403924812,
            0.801437403924812,
        ];
        assert_eq!(measured.features.len(), golden_corners.len());
        for (i, (feature, want)) in measured.features.iter().zip(&golden_corners).enumerate() {
            assert!(
                (feature.progress - want).abs() < GOLDEN_EPSILON,
                "corner {i}: expected {want}, got {}",
                feature.progress
            );
            assert_eq!(feature.kind, FeatureKind::ConvexCorner);
        }
    }

    #[test]
    fn a_star_alternates_convex_and_concave_corners() {
        let measured = MeasuredPolygon::measure(&star5());
        let kinds: Vec<FeatureKind> = measured.features.iter().map(|f| f.kind).collect();
        assert_eq!(kinds.len(), 10);
        for (i, kind) in kinds.iter().enumerate() {
            let want = if i % 2 == 0 {
                FeatureKind::ConvexCorner
            } else {
                FeatureKind::ConcaveCorner
            };
            assert_eq!(*kind, want, "corner {i}");
        }
    }

    #[test]
    fn cutting_and_shifting_keeps_the_same_outline() {
        let measured = MeasuredPolygon::measure(&circle8());
        let before = measured.clone();
        let shifted = measured.cut_and_shift(0.4);

        assert_eq!(shifted.cubics[0].start_outline_progress, 0.0);
        assert_eq!(
            shifted.cubics[shifted.cubics.len() - 1].end_outline_progress,
            1.0
        );
        // The outline is one curve longer (the cut one, in two halves) and
        // still contiguous.
        assert_eq!(shifted.cubics.len(), before.cubics.len() + 1);
        for pair in shifted.cubics.windows(2) {
            let (previous, next) = (pair[0].cubic.anchor1(), pair[1].cubic.anchor0());
            assert!((next - previous).hypot() < 1e-12);
        }
        // It starts where the cut was: 40% of the way round the old outline.
        let old_start = before
            .cubics
            .iter()
            .find(|c| c.start_outline_progress <= 0.4 && c.end_outline_progress >= 0.4)
            .expect("some curve spans 0.4")
            .cut_at_progress(0.4)
            .1
            .cubic
            .anchor0();
        assert!((shifted.cubics[0].cubic.anchor0() - old_start).hypot() < 1e-12);
    }

    // -----------------------------------------------------------------------
    // Feature mapping
    // -----------------------------------------------------------------------

    fn corners(polygon: &RoundedPolygon) -> Vec<ProgressableFeature> {
        MeasuredPolygon::measure(polygon)
            .features
            .into_iter()
            .filter(|f| f.kind != FeatureKind::Edge)
            .collect()
    }

    fn assert_mapping_matches_golden(
        p1: &RoundedPolygon,
        p2: &RoundedPolygon,
        golden: &[(f64, f64)],
        name: &str,
    ) {
        let mapping = do_mapping(&corners(p1), &corners(p2));
        assert_eq!(mapping.len(), golden.len(), "{name}: pair count drifted");
        for (i, (got, want)) in mapping.iter().zip(golden).enumerate() {
            assert!(
                (got.0 - want.0).abs() < GOLDEN_EPSILON && (got.1 - want.1).abs() < GOLDEN_EPSILON,
                "{name}: pair {i}: expected {want:?}, got {got:?}"
            );
        }
    }

    #[test]
    fn corner_pairing_matches_the_dart_goldens() {
        assert_mapping_matches_golden(
            &triangle(),
            &square(),
            &[
                (0.0, 0.0),
                (0.3333333333333335, 0.25),
                (0.6666666666666667, 0.75),
            ],
            "triangle -> square",
        );
        assert_mapping_matches_golden(
            &triangle(),
            &octagon(),
            &[
                (0.0, 0.0),
                (0.3333333333333335, 0.375),
                (0.6666666666666667, 0.625),
            ],
            "triangle -> octagon",
        );
        assert_mapping_matches_golden(
            &circle8(),
            &square(),
            &[(0.0625, 0.0), (0.3125, 0.25), (0.5625, 0.5), (0.8125, 0.75)],
            "circle -> square",
        );
        assert_mapping_matches_golden(
            &star5(),
            &circle8(),
            &[
                (0.0465277905440431, 0.0625),
                (0.24652779054404328, 0.3125),
                (0.44652779054404323, 0.4375),
                (0.6465277905440434, 0.6875),
                (0.8465277905440434, 0.8125),
            ],
            "star -> circle",
        );
    }

    #[test]
    fn every_paired_corner_is_a_near_neighbour() {
        // Upstream's `feature mapping triangle to square`: one exact match
        // (both shapes have a vertex at 0°) and two close ones.
        let (c1, c2) = (corners(&triangle()), corners(&square()));
        let mut distances: Vec<f64> = do_mapping(&c1, &c2)
            .iter()
            .map(|&(p1, p2)| {
                let f1 = c1.iter().find(|f| f.progress == p1).expect("mapped corner");
                let f2 = c2.iter().find(|f| f.progress == p2).expect("mapped corner");
                feature_dist_squared(f1, f2).expect("corners of the same kind")
            })
            .collect();
        distances.sort_by(f64::total_cmp);

        assert_eq!(distances.len(), 3);
        assert!(distances[0] < 1e-6, "the exact match: {}", distances[0]);
        assert_close(distances[1], distances[2], "the two symmetric matches");
        assert!(distances[2] < 0.3, "the worst match: {}", distances[2]);
    }

    #[test]
    fn a_convex_corner_never_maps_to_a_concave_one() {
        let star = corners(&star5());
        let circle = corners(&circle8());
        let concave = star
            .iter()
            .find(|f| f.kind == FeatureKind::ConcaveCorner)
            .expect("a star has concave corners");
        assert!(feature_dist_squared(concave, &circle[0]).is_none());

        // So a star's inner points sit out the mapping entirely: only its five
        // outer ones are paired.
        assert_eq!(do_mapping(&star, &circle).len(), 5);
    }

    #[test]
    fn mapping_a_complicated_pair_does_not_crash() {
        // Upstream's regression case: a checkmark against a many-pointed star
        // used to crash the mapper.
        let checkmark = RoundedPolygon::from_vertices(
            &[
                Point::new(400.0, -304.0),
                Point::new(240.0, -464.0),
                Point::new(296.0, -520.0),
                Point::new(400.0, -416.0),
                Point::new(664.0, -680.0),
                Point::new(720.0, -624.0),
            ],
            CornerRounding::UNROUNDED,
            None,
            None,
        )
        .normalized();
        let very_sunny = RoundedPolygon::star(StarParams {
            num_vertices_per_radius: 8,
            inner_radius: 0.65,
            rounding: CornerRounding::radius(0.15),
            ..StarParams::default()
        })
        .normalized();

        let mapping = do_mapping(&corners(&checkmark), &corners(&very_sunny));
        assert!(!mapping.is_empty());
        let _ = Morph::new(checkmark, very_sunny);
    }

    // -----------------------------------------------------------------------
    // Morph: endpoints
    // -----------------------------------------------------------------------

    #[test]
    fn the_endpoints_are_the_source_shapes_point_for_point() {
        // Recorded from `Morph.asCubics` upstream: a triangle morphing into a
        // square, at both ends. The four curves at progress 0 trace the
        // triangle (one side is cut in two, where the square has a corner the
        // triangle does not); at progress 1 they trace the square.
        let morph = Morph::new(triangle(), square());
        assert_cubics_match_golden(
            &morph.as_cubics(0.0),
            &[
                [
                    1.0,
                    0.0,
                    0.5000000000000001,
                    0.28867513459481287,
                    1.6653345369377348e-16,
                    0.5773502691896257,
                    -0.49999999999999983,
                    0.8660254037844387,
                ],
                [
                    -0.49999999999999983,
                    0.8660254037844387,
                    -0.4999999999999999,
                    0.5773502691896258,
                    -0.5,
                    0.288675134594813,
                    -0.5000000000000001,
                    2.7755575615628914e-17,
                ],
                [
                    -0.5000000000000001,
                    2.7755575615628914e-17,
                    -0.5000000000000002,
                    -0.28867513459481275,
                    -0.5000000000000003,
                    -0.5773502691896255,
                    -0.5000000000000004,
                    -0.8660254037844384,
                ],
                [
                    -0.5000000000000004,
                    -0.8660254037844384,
                    -3.3306690738754696e-16,
                    -0.5773502691896256,
                    0.4999999999999998,
                    -0.2886751345948128,
                    1.0,
                    0.0,
                ],
            ],
            "triangle -> square at 0",
        );
        assert_cubics_match_golden(
            &morph.as_cubics(1.0),
            &[
                [
                    1.0,
                    0.0,
                    0.6666666666666667,
                    0.3333333333333333,
                    0.3333333333333334,
                    0.6666666666666666,
                    6.123233995736766e-17,
                    1.0,
                ],
                [
                    6.123233995736766e-17,
                    1.0,
                    -0.33333333333333326,
                    0.6666666666666667,
                    -0.6666666666666666,
                    0.3333333333333334,
                    -1.0,
                    1.2246467991473532e-16,
                ],
                [
                    -1.0,
                    1.2246467991473532e-16,
                    -0.6666666666666669,
                    -0.33333333333333326,
                    -0.3333333333333335,
                    -0.6666666666666666,
                    -1.8369701987210297e-16,
                    -1.0,
                ],
                [
                    -1.8369701987210297e-16,
                    -1.0,
                    0.3333333333333332,
                    -0.6666666666666667,
                    0.6666666666666665,
                    -0.33333333333333337,
                    1.0,
                    0.0,
                ],
            ],
            "triangle -> square at 1",
        );
    }

    #[test]
    fn the_endpoints_trace_the_source_outlines() {
        for (name, start, end) in [
            ("triangle -> square", triangle(), square()),
            ("triangle -> octagon", triangle(), octagon()),
            ("circle -> square", circle8(), square()),
            ("rounded square -> circle", rounded_square(), circle8()),
            ("star -> circle", star5(), circle8()),
        ] {
            let (start_path, end_path) = (start.to_path(), end.to_path());
            let morph = Morph::new(start.clone(), end.clone());

            assert_same_outline(&morph.path(0.0), &start_path, 1e-9, &format!("{name} at 0"));
            assert_same_outline(&morph.path(1.0), &end_path, 1e-9, &format!("{name} at 1"));

            // The area a fill covers is the same to within round-off, which no
            // partial or doubled cover could be.
            assert!(
                (morph.path(0.0).area() - start_path.area()).abs() < 1e-9,
                "{name}: filled area differs at 0"
            );
            assert!(
                (morph.path(1.0).area() - end_path.area()).abs() < 1e-9,
                "{name}: filled area differs at 1"
            );
        }
    }

    #[test]
    fn a_shape_morphed_into_itself_never_moves() {
        for shape in [triangle(), circle8(), rounded_square(), star5()] {
            let path = shape.to_path();
            let morph = Morph::new(shape.clone(), shape.clone());
            for step in 0..=10 {
                let progress = f64::from(step) / 10.0;
                assert_same_outline(
                    &morph.path(progress),
                    &path,
                    1e-9,
                    &format!("self-morph at {progress}"),
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Morph: in between
    // -----------------------------------------------------------------------

    #[test]
    fn the_halfway_outline_matches_the_dart_goldens() {
        // A circle and a square: eight curves against four, so the matching
        // cuts both into twelve.
        assert_cubics_match_golden(
            &Morph::new(circle8(), square()).as_cubics(0.5),
            &[
                [
                    0.8369397662556435,
                    -0.31634171618254486,
                    0.9039799220852145,
                    -0.2134180278540675,
                    0.9583333333333333,
                    -0.10670901392703358,
                    1.0,
                    3.0878077872387166e-16,
                ],
                [
                    1.0,
                    3.0878077872387166e-16,
                    0.9583333333333335,
                    0.10670901392703386,
                    0.9039799220852147,
                    0.21341802785406744,
                    0.8369397662556439,
                    0.3163417161825445,
                ],
                [
                    0.8369397662556439,
                    0.3163417161825445,
                    0.7028594545965018,
                    0.522189092839499,
                    0.5221890928395,
                    0.7028594545965007,
                    0.3163417161825455,
                    0.8369397662556429,
                ],
                [
                    0.3163417161825455,
                    0.836939766255643,
                    0.21341802785406783,
                    0.9039799220852143,
                    0.10670901392703366,
                    0.9583333333333334,
                    -4.967397667182656e-16,
                    1.0,
                ],
                [
                    -4.967397667182656e-16,
                    1.0,
                    -0.10670901392703397,
                    0.9583333333333336,
                    -0.2134180278540674,
                    0.9039799220852148,
                    -0.31634171618254436,
                    0.836939766255644,
                ],
                [
                    -0.31634171618254436,
                    0.8369397662556439,
                    -0.522189092839499,
                    0.7028594545965017,
                    -0.7028594545965008,
                    0.5221890928395001,
                    -0.836939766255643,
                    0.31634171618254536,
                ],
                [
                    -0.8369397662556431,
                    0.31634171618254536,
                    -0.9039799220852144,
                    0.21341802785406774,
                    -0.9583333333333335,
                    0.10670901392703358,
                    -1.0,
                    -5.563292174903757e-16,
                ],
                [
                    -1.0,
                    -5.563292174903757e-16,
                    -0.9583333333333335,
                    -0.10670901392703397,
                    -0.9039799220852148,
                    -0.21341802785406735,
                    -0.836939766255644,
                    -0.3163417161825443,
                ],
                [
                    -0.8369397662556439,
                    -0.3163417161825443,
                    -0.7028594545965017,
                    -0.522189092839499,
                    -0.5221890928395001,
                    -0.7028594545965007,
                    -0.3163417161825455,
                    -0.836939766255643,
                ],
                [
                    -0.3163417161825455,
                    -0.836939766255643,
                    -0.213418027854068,
                    -0.9039799220852143,
                    -0.10670901392703391,
                    -0.9583333333333334,
                    1.3366554194093344e-16,
                    -1.0,
                ],
                [
                    1.3366554194093344e-16,
                    -1.0,
                    0.10670901392703383,
                    -0.9583333333333334,
                    0.2134180278540675,
                    -0.9039799220852145,
                    0.3163417161825447,
                    -0.8369397662556435,
                ],
                [
                    0.31634171618254475,
                    -0.8369397662556435,
                    0.5221890928394994,
                    -0.7028594545965013,
                    0.7028594545965011,
                    -0.5221890928394997,
                    0.8369397662556435,
                    -0.31634171618254486,
                ],
            ],
            "circle -> square at 0.5",
        );
    }

    #[test]
    fn morphing_across_differing_corner_counts_matches_the_dart_golden() {
        // Three corners against eight: the triangle's sides are cut so both
        // outlines walk as eight curves.
        assert_cubics_match_golden(
            &Morph::new(triangle(), octagon()).as_cubics(0.5),
            &[
                [
                    1.0,
                    0.0,
                    0.867851130197758,
                    0.16596365263022672,
                    0.7357022603955158,
                    0.33192730526045344,
                    0.6035533905932737,
                    0.4978909578906803,
                ],
                [
                    0.6035533905932737,
                    0.4978909578906803,
                    0.40236892706218247,
                    0.5948190167920578,
                    0.2011844635310912,
                    0.6917470756934354,
                    -1.1510060200336796e-16,
                    0.788675134594813,
                ],
                [
                    -1.1510060200336796e-16,
                    0.788675134594813,
                    -0.2011844635310913,
                    0.7879721205583732,
                    -0.4023689270621824,
                    0.787269106521933,
                    -0.6035533905932736,
                    0.7865660924854931,
                ],
                [
                    -0.6035533905932736,
                    0.7865660924854931,
                    -0.6523689270621824,
                    0.5243773949903291,
                    -0.7011844635310912,
                    0.26218869749516494,
                    -0.75,
                    7.481828364441833e-16,
                ],
                [
                    -0.75,
                    7.481828364441833e-16,
                    -0.7011844635310914,
                    -0.2621886974951638,
                    -0.6523689270621827,
                    -0.5243773949903283,
                    -0.6035533905932741,
                    -0.7865660924854929,
                ],
                [
                    -0.6035533905932741,
                    -0.7865660924854929,
                    -0.40236892706218286,
                    -0.7872691065219329,
                    -0.20118446353109162,
                    -0.7879721205583728,
                    -3.971598417079695e-16,
                    -0.7886751345948129,
                ],
                [
                    -3.971598417079695e-16,
                    -0.7886751345948129,
                    0.2011844635310909,
                    -0.6917470756934354,
                    0.4023689270621822,
                    -0.5948190167920578,
                    0.6035533905932735,
                    -0.4978909578906803,
                ],
                [
                    0.6035533905932735,
                    -0.4978909578906803,
                    0.7357022603955157,
                    -0.33192730526045355,
                    0.8678511301977578,
                    -0.16596365263022678,
                    1.0,
                    0.0,
                ],
            ],
            "triangle -> octagon at 0.5",
        );
    }

    #[test]
    fn morphing_a_shape_with_concave_corners_matches_the_dart_golden() {
        // A star's inner points sit out the corner pairing, so its curves are
        // carried along by the mapping of the outer ones — the case a mistake
        // in the concave path shows up in. Recorded from upstream; the start
        // anchors alone pin the whole walk, since each curve begins where the
        // one before it ended.
        let cubics = Morph::new(star5(), circle8()).as_cubics(0.5);
        let golden = [
            (0.785330199718972, -0.2682354889504666),
            (0.785327440564177, 0.2682427047337134),
            (0.7729607641303604, 0.28460842940814013),
            (0.6552523839451216, 0.4157139686894965),
            (0.486649700963247, 0.6048840395563428),
            (0.37615440549992074, 0.7289173135928099),
            (0.36220709983559274, 0.7420407503987456),
            (-0.0776730888836798, 0.8221309185364376),
            (-0.19387659974157886, 0.7513254080021173),
            (-0.2726062554084669, 0.7120463007873984),
            (-0.35736358560669745, 0.6914924591704735),
            (-0.4885340974522692, 0.6713420091019238),
            (-0.7667404101680318, 0.31848023965843286),
            (-0.7683818377075402, 0.29870091587759084),
            (-0.7679462219911052, 0.12701645351874605),
            (-0.7679304833770698, -0.12711291114449055),
            (-0.7683818377075398, -0.29870091587759007),
            (-0.7667216169895471, -0.31852230735243764),
            (-0.48831593733694323, -0.6714922788437413),
            (-0.3571801349513464, -0.6915865496117093),
            (-0.27260625540846845, -0.7120463007873981),
            (-0.19368034855012445, -0.7513885952154867),
            (-0.07741256963330226, -0.8221789242799227),
            (0.3622501349712382, -0.7420242928309926),
            (0.37615440549992063, -0.7289173135928098),
            (0.4867290356852201, -0.6048269625957917),
            (0.6553094609056724, -0.415634633967524),
            (0.7729607641303599, -0.2846084294081404),
        ];

        assert_eq!(cubics.len(), golden.len(), "curve count drifted");
        for (i, (cubic, (x, y))) in cubics.iter().zip(&golden).enumerate() {
            let anchor = cubic.anchor0();
            assert!(
                (anchor.x - x).abs() < GOLDEN_EPSILON && (anchor.y - y).abs() < GOLDEN_EPSILON,
                "curve {i}: expected start ({x}, {y}), got {anchor:?}"
            );
        }
    }

    #[test]
    fn the_halfway_outline_is_closed_simple_and_inside_both_shapes() {
        let (start, end) = (circle8(), square());
        let union = Morph::new(start.clone(), end.clone()).bounds();
        let morph = Morph::new(start, end);
        let path = morph.path(0.5);

        // Closed: the last curve lands exactly on the opening move, and the
        // path says so.
        let elements = path.elements();
        let Some(PathEl::MoveTo(first)) = elements.first().copied() else {
            panic!("a path opens with a MoveTo");
        };
        assert!(matches!(elements.last(), Some(PathEl::ClosePath)));
        let curves = segments(&path);
        assert_eq!(
            curves[curves.len() - 1].p3,
            first,
            "the outline must close exactly"
        );
        for pair in curves.windows(2) {
            assert_eq!(pair[0].p3, pair[1].p0, "the outline must be contiguous");
        }

        // Inside the union of both shapes' bounds.
        for curve in &curves {
            for i in 0..=8 {
                let point = curve.eval(f64::from(i) / 8.0);
                assert!(
                    union.inflate(1e-9, 1e-9).contains(point),
                    "{point:?} escapes the union bounds {union:?}"
                );
            }
        }

        // Simple (non-self-intersecting): a point can only be inside a closed
        // curve once, so no winding number may exceed 1 in magnitude, and every
        // interior point has to agree on the sign — a crossing shows up as a
        // lobe wound the other way, or wound twice.
        let mut inside = 0_u32;
        let mut sign = 0_i32;
        for row in 0..48 {
            for column in 0..48 {
                let point = Point::new(
                    union.x0 + union.width() * (f64::from(column) + 0.5) / 48.0,
                    union.y0 + union.height() * (f64::from(row) + 0.5) / 48.0,
                );
                let winding = path.winding(point);
                assert!(
                    (-1..=1).contains(&winding),
                    "winding {winding} at {point:?}: the outline crosses itself"
                );
                if winding != 0 {
                    assert!(
                        sign == 0 || sign == winding,
                        "winding {winding} at {point:?} disagrees with {sign} elsewhere"
                    );
                    sign = winding;
                    inside += 1;
                }
            }
        }
        assert!(inside > 1000, "only {inside} of 2304 samples landed inside");
    }

    #[test]
    fn the_bounds_hold_both_shapes() {
        let morph = Morph::new(rounded_square(), circle8());
        let bounds = morph.bounds();
        for polygon in [morph.start(), morph.end()] {
            let shape = polygon.bounds();
            assert!(
                bounds.union(shape) == bounds,
                "{shape:?} escapes the union {bounds:?}"
            );
        }
        assert!(morph.exact_bounds().area() <= bounds.area());
        // A shape spun about its centre needs the square, which is at least as
        // wide as the shape itself.
        let max = morph.max_bounds();
        assert!(max.width() >= morph.exact_bounds().width() - EPSILON);
        assert_close(max.width(), max.height(), "max bounds is a square");
    }

    #[test]
    fn a_hundred_samples_of_each_pair_are_well_formed() {
        for (name, start, end) in [
            ("triangle -> octagon", triangle(), octagon()),
            ("circle -> square", circle8(), square()),
            ("star -> rounded square", star5(), rounded_square()),
        ] {
            let morph = Morph::new(start, end);
            let curve_count = morph.as_cubics(0.0).len();
            for step in 0..100 {
                let progress = f64::from(step) / 99.0;
                let path = morph.path(progress);
                let curves = segments(&path);
                assert_eq!(curves.len(), curve_count, "{name} at {progress}");
                assert!(matches!(path.elements().last(), Some(PathEl::ClosePath)));
                for curve in &curves {
                    for value in [curve.p0, curve.p1, curve.p2, curve.p3] {
                        assert!(
                            value.x.is_finite() && value.y.is_finite(),
                            "{name} at {progress}: non-finite point {value:?}"
                        );
                    }
                }
            }
        }
    }
}
