//! The named shape vocabulary: 35 pre-built [`RoundedPolygon`]s, each
//! reachable by [`ShapeKind`] and cached after first construction (rounding
//! computation is not free — see [`ShapeKind::polygon`]).
//!
//! # Upstream
//!
//! Ported from **`material_new_shapes` 1.0.0** (MIT, Agbama Gifted)'s
//! `lib/src/material_shapes.dart` (the `MaterialShapes` class), itself a Dart
//! port of AndroidX Compose Material3's `MaterialShapes` —
//! <https://developer.android.com/images/reference/androidx/compose/material3/shapes.png>.
//! [`ShapeKind`]'s 35 variants and their declaration order mirror
//! `m3e_shape_kind.dart`'s `M3EShapeKind` enum, the catalog surface the M3
//! Expressive reference app built over this same package.
//!
//! Every shape below is a straight data transcription — vertex coordinates
//! and [`CornerRounding`] values copied verbatim from the Dart source, run
//! through the identical repeat/mirror/transform/normalize pipeline. The
//! table cites the exact source lines for each:
//!
//! | [`ShapeKind`] | `material_shapes.dart` |
//! |---|---|
//! | `Circle` | 25-30 |
//! | `Square` | 33-39 |
//! | `Slanted` | 42-54 |
//! | `Arch` | 57-69 |
//! | `SemiCircle` | 72-81 |
//! | `Oval` | 84-91 |
//! | `Pill` | 94-108 |
//! | `Triangle` | 111-117 |
//! | `Arrow` | 120-140 |
//! | `Fan` | 143-163 |
//! | `Diamond` | 166-178 |
//! | `ClamShell` | 181-197 |
//! | `Pentagon` | 200-217 |
//! | `Gem` | 220-241 |
//! | `Sunny` | 244-248 |
//! | `VerySunny` | 251-263 |
//! | `Cookie4Sided` | 266-278 |
//! | `Cookie6Sided` | 281-293 |
//! | `Cookie7Sided` | 296-304 |
//! | `Cookie9Sided` | 307-315 |
//! | `Cookie12Sided` | 318-326 |
//! | `Clover4Leaf` | 329-339 |
//! | `Clover8Leaf` | 342-351 |
//! | `Burst` | 354-366 |
//! | `SoftBurst` | 369-381 |
//! | `Boom` | 384-396 |
//! | `SoftBoom` | 399-417 |
//! | `Flower` | 420-434 |
//! | `Puffy` | 437-481 |
//! | `PuffyDiamond` | 484-498 |
//! | `Ghostish` | 501-522 |
//! | `PixelCircle` | 525-538 |
//! | `PixelTriangle` | 541-559 |
//! | `Bun` | 562-580 |
//! | `Heart` | 583-604 |
//!
//! ## Two upstream helpers this file also ports
//!
//! Most shapes aren't built from [`RoundedPolygon`]'s own named constructors
//! but from upstream's private `_customPolygon`/`_doRepeat`: a small vertex
//! list is repeated `reps` times around a center, optionally *mirrored*
//! (folded back on itself every other repetition rather than simply rotated)
//! to build shapes with reflective symmetry from an asymmetric seed list.
//! [`custom_polygon`]/[`repeat_points`] below are that port. A handful of
//! shapes additionally rotate or scale the result via a `Matrix4` chain
//! (`..rotateZ(a)..scale(sx, sy)`); [`rotate_z`]/[`scale`] reproduce that
//! exactly, scale-then-rotate, matching `Matrix4`'s right-multiply chaining.
//!
//! Full attribution ships in the crate's `NOTICE`.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, TAU};
use std::sync::OnceLock;

use kurbo::{Point, Vec2};

use super::rounded_polygon::{CornerRounding, RoundedPolygon, StarParams};

/// The center every catalog shape not built around the origin is authored
/// and normalized around (upstream's `_customPolygon`'s default `center:
/// const Point(0.5, 0.5)`).
const CENTER: Point = Point::new(0.5, 0.5);

/// `-45°`, `-90°`, `-135°` in radians — upstream's `_negative45Radians` /
/// `_negative90Radians` / `_negative135Radians`.
const ROTATE_NEG_45: f64 = -FRAC_PI_4;
const ROTATE_NEG_90: f64 = -FRAC_PI_2;
const ROTATE_NEG_135: f64 = -3.0 * FRAC_PI_4;

/// Upstream's five named [`CornerRounding`] radii (`_cornerRound15` /
/// `_cornerRound20` / `_cornerRound30` / `_cornerRound50` / `_cornerRound100`),
/// reused across several shapes below.
const ROUND_15: CornerRounding = CornerRounding {
    radius: 0.15,
    smoothing: 0.0,
};
const ROUND_20: CornerRounding = CornerRounding {
    radius: 0.2,
    smoothing: 0.0,
};
const ROUND_30: CornerRounding = CornerRounding {
    radius: 0.3,
    smoothing: 0.0,
};
const ROUND_50: CornerRounding = CornerRounding {
    radius: 0.5,
    smoothing: 0.0,
};
const ROUND_100: CornerRounding = CornerRounding {
    radius: 1.0,
    smoothing: 0.0,
};

// ---------------------------------------------------------------------------
// _customPolygon / _doRepeat (material_shapes.dart:647-727)
// ---------------------------------------------------------------------------

/// One vertex of a hand-authored shape, plus its corner rounding — upstream's
/// private `_PointNRound(Point, [round = CornerRounding.unrounded])`.
#[derive(Clone, Copy)]
struct PointNRound {
    point: Point,
    rounding: CornerRounding,
}

/// An unrounded vertex.
fn pt(x: f64, y: f64) -> PointNRound {
    PointNRound {
        point: Point::new(x, y),
        rounding: CornerRounding::UNROUNDED,
    }
}

/// A vertex with explicit rounding.
fn pt_r(x: f64, y: f64, rounding: CornerRounding) -> PointNRound {
    PointNRound {
        point: Point::new(x, y),
        rounding,
    }
}

/// Builds a polygon from a short, hand-authored vertex list, repeated `reps`
/// times around `center` (optionally mirrored). Upstream's private
/// `_customPolygon`.
fn custom_polygon(
    points: &[PointNRound],
    reps: usize,
    center: Point,
    mirroring: bool,
) -> RoundedPolygon {
    let actual = repeat_points(points, reps, center, mirroring);
    let vertices: Vec<Point> = actual.iter().map(|p| p.point).collect();
    let rounding: Vec<CornerRounding> = actual.iter().map(|p| p.rounding).collect();
    RoundedPolygon::from_vertices(
        &vertices,
        CornerRounding::UNROUNDED,
        Some(&rounding),
        Some(center),
    )
}

/// Repeats a short vertex list around `center`. Upstream's private
/// `_doRepeat`.
///
/// Without mirroring, the list is rotated by `360 / reps` degrees `reps`
/// times — a plain radial repeat. With mirroring, each odd repetition is
/// additionally reflected (walked in reverse, with its first point dropped)
/// so consecutive repetitions read as mirror images rather than rotated
/// copies — how an asymmetric two-point seed becomes a symmetric petal or
/// cookie point.
fn repeat_points(
    points: &[PointNRound],
    reps: usize,
    center: Point,
    mirroring: bool,
) -> Vec<PointNRound> {
    let mut result = Vec::new();

    if mirroring {
        // Each seed point's polar coordinates relative to `center`, reused
        // every repetition rather than recomputed.
        let measures: Vec<(f64, f64)> = points
            .iter()
            .map(|p| {
                let off = p.point - center;
                (off.atan2(), off.hypot())
            })
            .collect();
        let actual_reps = reps * 2;
        let section_angle = TAU / actual_reps as f64;

        for r in 0..actual_reps {
            for index in 0..points.len() {
                let i = if r % 2 == 0 {
                    index
                } else {
                    points.len() - 1 - index
                };
                // Odd (reflected) repetitions drop their first point — it is
                // the same point the preceding repetition ended on.
                if i > 0 || r % 2 == 0 {
                    let angle = section_angle * r as f64
                        + if r % 2 == 0 {
                            measures[i].0
                        } else {
                            section_angle - measures[i].0 + 2.0 * measures[0].0
                        };
                    let point = center + Vec2::new(angle.cos(), angle.sin()) * measures[i].1;
                    result.push(PointNRound {
                        point,
                        rounding: points[i].rounding,
                    });
                }
            }
        }
    } else {
        let n = points.len();
        for i in 0..(n * reps) {
            let degrees = (i / n) as f64 * 360.0 / reps as f64;
            let point = rotate_around(points[i % n].point, degrees.to_radians(), center);
            result.push(PointNRound {
                point,
                rounding: points[i % n].rounding,
            });
        }
    }

    result
}

/// Rotates `p` by `radians` about `center` — upstream's `Point.rotate`.
fn rotate_around(p: Point, radians: f64, center: Point) -> Point {
    let off = p - center;
    let (sin, cos) = radians.sin_cos();
    center + Vec2::new(off.x * cos - off.y * sin, off.x * sin + off.y * cos)
}

/// Rotates `p` by `radians` about the origin — the `..rotateZ(a)` half of a
/// `Matrix4` chain applied through `asPointTransformer`.
fn rotate_z(radians: f64, p: Point) -> Point {
    let (sin, cos) = radians.sin_cos();
    Point::new(p.x * cos - p.y * sin, p.x * sin + p.y * cos)
}

/// Scales `p` by `(sx, sy)` about the origin — the `..scale(sx, sy)` half of
/// a `Matrix4` chain. `Matrix4`'s methods right-multiply (`M = M * op`), so a
/// chain of `..rotateZ(a)..scale(sx, sy)` applies scale first, then rotate —
/// callers below compose `rotate_z(a, scale(sx, sy, p))` to match.
fn scale(sx: f64, sy: f64, p: Point) -> Point {
    Point::new(p.x * sx, p.y * sy)
}

// ---------------------------------------------------------------------------
// ShapeKind
// ---------------------------------------------------------------------------

/// One entry in the Material 3 Expressive named-shape catalog.
///
/// Mirrors `m3e_shape_kind.dart`'s `M3EShapeKind` enum: 35 variants, in its
/// declaration order, each resolving to the [`RoundedPolygon`] the doc table
/// on [the module](self) cites.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShapeKind {
    Circle,
    Square,
    Slanted,
    Arch,
    SemiCircle,
    Oval,
    Pill,
    Triangle,
    Arrow,
    Fan,
    Diamond,
    ClamShell,
    Pentagon,
    Gem,
    Sunny,
    VerySunny,
    Cookie4Sided,
    Cookie6Sided,
    Cookie7Sided,
    Cookie9Sided,
    Cookie12Sided,
    Clover4Leaf,
    Clover8Leaf,
    Burst,
    SoftBurst,
    Boom,
    SoftBoom,
    Flower,
    Puffy,
    PuffyDiamond,
    Ghostish,
    PixelCircle,
    PixelTriangle,
    Bun,
    Heart,
}

impl ShapeKind {
    /// Every catalog entry, in declaration order — upstream's
    /// `M3EShapeKind.all`.
    pub const ALL: [ShapeKind; 35] = [
        ShapeKind::Circle,
        ShapeKind::Square,
        ShapeKind::Slanted,
        ShapeKind::Arch,
        ShapeKind::SemiCircle,
        ShapeKind::Oval,
        ShapeKind::Pill,
        ShapeKind::Triangle,
        ShapeKind::Arrow,
        ShapeKind::Fan,
        ShapeKind::Diamond,
        ShapeKind::ClamShell,
        ShapeKind::Pentagon,
        ShapeKind::Gem,
        ShapeKind::Sunny,
        ShapeKind::VerySunny,
        ShapeKind::Cookie4Sided,
        ShapeKind::Cookie6Sided,
        ShapeKind::Cookie7Sided,
        ShapeKind::Cookie9Sided,
        ShapeKind::Cookie12Sided,
        ShapeKind::Clover4Leaf,
        ShapeKind::Clover8Leaf,
        ShapeKind::Burst,
        ShapeKind::SoftBurst,
        ShapeKind::Boom,
        ShapeKind::SoftBoom,
        ShapeKind::Flower,
        ShapeKind::Puffy,
        ShapeKind::PuffyDiamond,
        ShapeKind::Ghostish,
        ShapeKind::PixelCircle,
        ShapeKind::PixelTriangle,
        ShapeKind::Bun,
        ShapeKind::Heart,
    ];

    /// This kind's [`RoundedPolygon`], built on first call and cached
    /// thereafter — construction runs the full corner-rounding pipeline per
    /// vertex, which is not free. Repeat calls for the same variant return
    /// the identical cached instance (pointer-equal).
    #[must_use]
    pub fn polygon(&self) -> &'static RoundedPolygon {
        match self {
            ShapeKind::Circle => circle(),
            ShapeKind::Square => square(),
            ShapeKind::Slanted => slanted(),
            ShapeKind::Arch => arch(),
            ShapeKind::SemiCircle => semi_circle(),
            ShapeKind::Oval => oval(),
            ShapeKind::Pill => pill(),
            ShapeKind::Triangle => triangle(),
            ShapeKind::Arrow => arrow(),
            ShapeKind::Fan => fan(),
            ShapeKind::Diamond => diamond(),
            ShapeKind::ClamShell => clam_shell(),
            ShapeKind::Pentagon => pentagon(),
            ShapeKind::Gem => gem(),
            ShapeKind::Sunny => sunny(),
            ShapeKind::VerySunny => very_sunny(),
            ShapeKind::Cookie4Sided => cookie_4_sided(),
            ShapeKind::Cookie6Sided => cookie_6_sided(),
            ShapeKind::Cookie7Sided => cookie_7_sided(),
            ShapeKind::Cookie9Sided => cookie_9_sided(),
            ShapeKind::Cookie12Sided => cookie_12_sided(),
            ShapeKind::Clover4Leaf => clover_4_leaf(),
            ShapeKind::Clover8Leaf => clover_8_leaf(),
            ShapeKind::Burst => burst(),
            ShapeKind::SoftBurst => soft_burst(),
            ShapeKind::Boom => boom(),
            ShapeKind::SoftBoom => soft_boom(),
            ShapeKind::Flower => flower(),
            ShapeKind::Puffy => puffy(),
            ShapeKind::PuffyDiamond => puffy_diamond(),
            ShapeKind::Ghostish => ghostish(),
            ShapeKind::PixelCircle => pixel_circle(),
            ShapeKind::PixelTriangle => pixel_triangle(),
            ShapeKind::Bun => bun(),
            ShapeKind::Heart => heart(),
        }
    }
}

// ---------------------------------------------------------------------------
// The 35 shapes (material_shapes.dart:25-604) — see the module doc's table
// for exact source-line citations.
// ---------------------------------------------------------------------------

fn circle() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| RoundedPolygon::circle(10, 0.5, CENTER))
}

fn square() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| RoundedPolygon::rectangle(1.0, 1.0, ROUND_30, None, CENTER))
}

fn slanted() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.926, 0.970, CornerRounding::new(0.189, 0.811)),
                pt_r(-0.021, 0.967, CornerRounding::new(0.187, 0.057)),
            ],
            2,
            CENTER,
            false,
        )
        .normalized()
    })
}

fn arch() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        RoundedPolygon::from_num_vertices(
            4,
            1.0,
            Point::ZERO,
            CornerRounding::UNROUNDED,
            Some(&[ROUND_100, ROUND_100, ROUND_20, ROUND_20]),
        )
        .transformed_with(|p| rotate_z(ROTATE_NEG_135, p))
        .normalized()
    })
}

fn semi_circle() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        RoundedPolygon::rectangle(
            1.6,
            1.0,
            CornerRounding::UNROUNDED,
            Some(&[ROUND_20, ROUND_20, ROUND_100, ROUND_100]),
            Point::ZERO,
        )
        .normalized()
    })
}

fn oval() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        RoundedPolygon::circle(8, 1.0, Point::ZERO)
            .transformed_with(|p| rotate_z(ROTATE_NEG_45, scale(1.0, 0.64, p)))
            .normalized()
    })
}

fn pill() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.961, 0.039, CornerRounding::radius(0.426)),
                pt(1.001, 0.428),
                pt_r(1.0, 0.609, ROUND_100),
            ],
            2,
            CENTER,
            true,
        )
        .normalized()
    })
}

fn triangle() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        RoundedPolygon::from_num_vertices(3, 1.0, Point::ZERO, ROUND_20, None)
            .transformed_with(|p| rotate_z(ROTATE_NEG_90, p))
            .normalized()
    })
}

fn arrow() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.5, 0.892, CornerRounding::radius(0.313)),
                pt_r(-0.216, 1.05, CornerRounding::radius(0.207)),
                pt_r(0.499, -0.16, CornerRounding::new(0.215, 1.0)),
                pt_r(1.225, 1.06, CornerRounding::radius(0.211)),
            ],
            1,
            CENTER,
            false,
        )
        .normalized()
    })
}

fn fan() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(1.004, 1.0, CornerRounding::new(0.148, 0.417)),
                pt_r(0.0, 1.0, CornerRounding::radius(0.151)),
                pt_r(0.0, -0.003, CornerRounding::radius(0.148)),
                pt_r(0.978, 0.02, CornerRounding::radius(0.803)),
            ],
            1,
            CENTER,
            false,
        )
        .normalized()
    })
}

fn diamond() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.5, 1.096, CornerRounding::new(0.151, 0.524)),
                pt_r(0.04, 0.5, CornerRounding::radius(0.159)),
            ],
            2,
            CENTER,
            false,
        )
        .normalized()
    })
}

fn clam_shell() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.171, 0.841, CornerRounding::radius(0.159)),
                pt_r(-0.02, 0.5, CornerRounding::radius(0.140)),
                pt_r(0.17, 0.159, CornerRounding::radius(0.159)),
            ],
            2,
            CENTER,
            false,
        )
        .normalized()
    })
}

fn pentagon() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.5, -0.009, CornerRounding::radius(0.172)),
                pt_r(1.03, 0.365, CornerRounding::radius(0.164)),
                pt_r(0.828, 0.97, CornerRounding::radius(0.169)),
            ],
            1,
            CENTER,
            true,
        )
        .normalized()
    })
}

fn gem() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.499, 1.023, CornerRounding::new(0.241, 0.778)),
                pt_r(-0.005, 0.792, CornerRounding::radius(0.208)),
                pt_r(0.073, 0.258, CornerRounding::radius(0.228)),
                pt_r(0.433, 0.0, CornerRounding::radius(0.491)),
            ],
            1,
            CENTER,
            true,
        )
        .normalized()
    })
}

fn sunny() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        RoundedPolygon::star(StarParams {
            num_vertices_per_radius: 8,
            inner_radius: 0.8,
            rounding: ROUND_15,
            ..StarParams::default()
        })
        .normalized()
    })
}

fn very_sunny() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.5, 1.080, CornerRounding::radius(0.085)),
                pt_r(0.358, 0.843, CornerRounding::radius(0.085)),
            ],
            8,
            CENTER,
            false,
        )
        .normalized()
    })
}

fn cookie_4_sided() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(1.237, 1.236, CornerRounding::radius(0.258)),
                pt_r(0.5, 0.918, CornerRounding::radius(0.233)),
            ],
            4,
            CENTER,
            false,
        )
        .normalized()
    })
}

fn cookie_6_sided() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.723, 0.884, CornerRounding::radius(0.394)),
                pt_r(0.5, 1.099, CornerRounding::radius(0.398)),
            ],
            6,
            CENTER,
            false,
        )
        .normalized()
    })
}

fn cookie_7_sided() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        RoundedPolygon::star(StarParams {
            num_vertices_per_radius: 7,
            inner_radius: 0.75,
            rounding: ROUND_50,
            ..StarParams::default()
        })
        .transformed_with(|p| rotate_z(ROTATE_NEG_90, p))
        .normalized()
    })
}

fn cookie_9_sided() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        RoundedPolygon::star(StarParams {
            num_vertices_per_radius: 9,
            inner_radius: 0.8,
            rounding: ROUND_50,
            ..StarParams::default()
        })
        .transformed_with(|p| rotate_z(ROTATE_NEG_90, p))
        .normalized()
    })
}

fn cookie_12_sided() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        RoundedPolygon::star(StarParams {
            num_vertices_per_radius: 12,
            inner_radius: 0.8,
            rounding: ROUND_50,
            ..StarParams::default()
        })
        .transformed_with(|p| rotate_z(ROTATE_NEG_90, p))
        .normalized()
    })
}

fn clover_4_leaf() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt(0.5, 0.074),
                pt_r(0.725, -0.099, CornerRounding::radius(0.476)),
            ],
            4,
            CENTER,
            true,
        )
        .normalized()
    })
}

fn clover_8_leaf() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt(0.5, 0.036),
                pt_r(0.758, -0.101, CornerRounding::radius(0.209)),
            ],
            8,
            CENTER,
            false,
        )
        .normalized()
    })
}

fn burst() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.5, -0.006, CornerRounding::radius(0.006)),
                pt_r(0.592, 0.158, CornerRounding::radius(0.006)),
            ],
            12,
            CENTER,
            false,
        )
        .normalized()
    })
}

fn soft_burst() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.193, 0.277, CornerRounding::radius(0.053)),
                pt_r(0.176, 0.055, CornerRounding::radius(0.053)),
            ],
            10,
            CENTER,
            false,
        )
        .normalized()
    })
}

fn boom() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.457, 0.296, CornerRounding::radius(0.007)),
                pt_r(0.5, -0.051, CornerRounding::radius(0.007)),
            ],
            15,
            CENTER,
            false,
        )
        .normalized()
    })
}

fn soft_boom() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt(0.733, 0.454),
                pt_r(0.839, 0.437, CornerRounding::radius(0.532)),
                pt_r(0.949, 0.449, CornerRounding::new(0.439, 1.0)),
                pt_r(0.998, 0.478, CornerRounding::radius(0.174)),
            ],
            16,
            CENTER,
            true,
        )
        .normalized()
    })
}

fn flower() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt(0.370, 0.187),
                pt_r(0.416, 0.049, CornerRounding::radius(0.381)),
                pt_r(0.479, 0.001, CornerRounding::radius(0.095)),
            ],
            8,
            CENTER,
            true,
        )
        .normalized()
    })
}

fn puffy() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt(0.5, 0.053),
                pt_r(0.545, -0.04, CornerRounding::radius(0.405)),
                pt_r(0.670, -0.035, CornerRounding::radius(0.426)),
                pt_r(0.717, 0.066, CornerRounding::radius(0.574)),
                pt(0.722, 0.128),
                pt_r(0.777, 0.002, CornerRounding::radius(0.36)),
                pt_r(0.914, 0.149, CornerRounding::radius(0.66)),
                pt_r(0.926, 0.289, CornerRounding::radius(0.66)),
                pt(0.881, 0.346),
                pt_r(0.940, 0.344, CornerRounding::radius(0.126)),
                pt_r(1.003, 0.437, CornerRounding::radius(0.255)),
            ],
            2,
            CENTER,
            true,
        )
        .transformed_with(|p| scale(1.0, 0.742, p))
        .normalized()
    })
}

fn puffy_diamond() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.87, 0.13, CornerRounding::radius(0.146)),
                pt(0.818, 0.357),
                pt_r(1.0, 0.332, CornerRounding::radius(0.853)),
            ],
            4,
            CENTER,
            true,
        )
        .normalized()
    })
}

fn ghostish() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.5, 0.0, ROUND_100),
                pt_r(1.0, 0.0, ROUND_100),
                pt_r(1.0, 1.14, CornerRounding::new(0.254, 0.106)),
                pt_r(0.575, 0.906, CornerRounding::radius(0.253)),
            ],
            1,
            CENTER,
            true,
        )
        .normalized()
    })
}

fn pixel_circle() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt(0.5, 0.0),
                pt(0.704, 0.0),
                pt(0.704, 0.065),
                pt(0.843, 0.065),
                pt(0.843, 0.148),
                pt(0.926, 0.148),
                pt(0.926, 0.296),
                pt(1.0, 0.296),
            ],
            2,
            CENTER,
            true,
        )
        .normalized()
    })
}

fn pixel_triangle() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt(0.11, 0.5),
                pt(0.113, 0.0),
                pt(0.287, 0.0),
                pt(0.287, 0.087),
                pt(0.421, 0.087),
                pt(0.421, 0.17),
                pt(0.56, 0.17),
                pt(0.56, 0.265),
                pt(0.674, 0.265),
                pt(0.675, 0.344),
                pt(0.789, 0.344),
                pt(0.789, 0.439),
                pt(0.888, 0.439),
            ],
            1,
            CENTER,
            true,
        )
        .normalized()
    })
}

fn bun() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt(0.796, 0.5),
                pt_r(0.853, 0.518, ROUND_100),
                pt_r(0.992, 0.631, ROUND_100),
                pt_r(0.968, 1.0, ROUND_100),
            ],
            2,
            CENTER,
            true,
        )
        .normalized()
    })
}

fn heart() -> &'static RoundedPolygon {
    static CACHE: OnceLock<RoundedPolygon> = OnceLock::new();
    CACHE.get_or_init(|| {
        custom_polygon(
            &[
                pt_r(0.5, 0.268, CornerRounding::radius(0.016)),
                pt_r(0.792, -0.066, CornerRounding::radius(0.958)),
                pt_r(1.064, 0.276, ROUND_100),
                pt_r(0.501, 0.946, CornerRounding::radius(0.129)),
            ],
            1,
            CENTER,
            true,
        )
        .normalized()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{CubicBez, ParamCurveNearest, PathEl};

    #[test]
    fn all_has_every_variant_exactly_once() {
        assert_eq!(ShapeKind::ALL.len(), 35, "35-shape catalog count drifted");
        let mut seen = std::collections::HashSet::new();
        for kind in ShapeKind::ALL {
            assert!(seen.insert(kind), "{kind:?} listed more than once");
        }
    }

    #[test]
    fn every_shape_is_a_closed_non_empty_outline() {
        for kind in ShapeKind::ALL {
            let polygon = kind.polygon();
            assert!(!polygon.cubics().is_empty(), "{kind:?} has no cubics");

            let path = polygon.to_path();
            let elements: Vec<PathEl> = path.elements().to_vec();
            let Some(PathEl::MoveTo(start)) = elements.first().copied() else {
                panic!("{kind:?}: path must open with a MoveTo");
            };
            assert!(
                matches!(elements.last(), Some(PathEl::ClosePath)),
                "{kind:?}: path must end with a ClosePath"
            );

            let curves: Vec<PathEl> = elements
                .iter()
                .copied()
                .filter(|el| matches!(el, PathEl::CurveTo(..)))
                .collect();
            assert!(!curves.is_empty(), "{kind:?}: path has no curves");
            let Some(PathEl::CurveTo(_, _, end)) = curves.last().copied() else {
                unreachable!("filtered to CurveTo above");
            };
            assert_eq!(end, start, "{kind:?}: outline does not close exactly");
        }
    }

    #[test]
    fn polygon_accessor_is_cached() {
        for kind in ShapeKind::ALL {
            let a = kind.polygon();
            let b = kind.polygon();
            assert!(
                std::ptr::eq(a, b),
                "{kind:?}: polygon() returned a different instance on repeat call"
            );
        }
    }

    /// Every vertex a [`RoundedPolygon::from_vertices`]-built shape carries
    /// yields exactly one corner [`super::Feature`] and one edge one, so a
    /// shape's known vertex count (derivable from its `reps`/mirroring
    /// parameters without touching curve geometry) pins its feature count.
    /// Spot-checks upstream's `_doRepeat` output size for one shape of each
    /// distinctive construction family this catalog uses.
    #[test]
    fn parameter_fidelity_spot_checks() {
        // sunny: RoundedPolygon::star, 8 vertices per radius -> 16 vertices.
        assert_eq!(ShapeKind::Sunny.polygon().features().len(), 32);

        // cookie9Sided: RoundedPolygon::star, 9 vertices per radius -> 18
        // vertices; also the catalog's 9-fold-symmetry pin (see the test
        // below), so this doubles as its structural half.
        assert_eq!(ShapeKind::Cookie9Sided.polygon().features().len(), 36);

        // clover4Leaf: custom_polygon, 2-point seed, reps=4, mirrored ->
        // reps * (2 * seed_len - 1) = 4 * 3 = 12 vertices.
        assert_eq!(ShapeKind::Clover4Leaf.polygon().features().len(), 24);

        // heart: custom_polygon, 4-point seed, reps=1, mirrored ->
        // 1 * (2 * 4 - 1) = 7 vertices.
        assert_eq!(ShapeKind::Heart.polygon().features().len(), 14);

        // pentagon: custom_polygon, 3-point seed, reps=1, mirrored ->
        // 1 * (2 * 3 - 1) = 5 vertices — matching its name.
        assert_eq!(ShapeKind::Pentagon.polygon().features().len(), 10);
    }

    /// `cookie9Sided` is a 9-vertices-per-radius star (dihedral D9 symmetry
    /// pre-transform) taken through a pure rotation and a uniform-scale
    /// normalize — both preserve rotational symmetry about the correspondingly
    /// transformed center (upstream carries the same guarantee for every
    /// `RoundedPolygon.star` derivative it rotates/normalizes). Sampled here
    /// by checking that rotating points off the outline by a ninth turn about
    /// [`RoundedPolygon::center`] lands back on the outline.
    #[test]
    fn cookie_9_sided_has_nine_fold_rotational_symmetry() {
        let cookie = ShapeKind::Cookie9Sided.polygon();
        let curves: Vec<CubicBez> = cookie
            .cubics()
            .iter()
            .map(|c| CubicBez::new(c.anchor0(), c.control0(), c.control1(), c.anchor1()))
            .collect();
        let center = cookie.center();
        let ninth_turn = TAU / 9.0;

        for cubic in cookie.cubics() {
            for i in 0..4 {
                let sample = cubic.point_on_curve(f64::from(i) / 4.0);
                let rotated = rotate_around(sample, ninth_turn, center);
                let nearest = curves
                    .iter()
                    .map(|curve| curve.nearest(rotated, 1e-12).distance_sq)
                    .fold(f64::INFINITY, f64::min)
                    .sqrt();
                assert!(nearest < 1e-8, "{rotated:?} is {nearest} off the outline");
            }
        }
    }
}
