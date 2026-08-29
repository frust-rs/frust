//! Scene-compiler behaviour over the geometry commands it lowers.
//!
//! Every case drives the public seam — a `frust_scene::Scene` recorded through
//! `SceneBuilder`, compiled by [`SceneCompiler`] — so the assertions pin the
//! contract a GPU pass consumes rather than the compiler's internals. No GPU,
//! device, or surface is involved: strip generation is pure CPU work.

use frust_engine::{EngineError, SceneCompiler};
use frust_scene::{DashPattern, Scene, SceneBuilder};
use kurbo::{Affine, BezPath, Point, Rect};
use peniko::color::palette::css::{BLUE, RED};
use peniko::{Brush, Color};

/// Viewport every case compiles against, comfortably larger than the geometry
/// it records so nothing is culled by accident.
const VIEWPORT: (u16, u16) = (200, 200);

fn solid(color: Color) -> Brush {
    Brush::Solid(color)
}

fn compiler() -> SceneCompiler {
    SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
}

/// A scene built by `record`, ready to compile.
fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    record(&mut builder);
    scene
}

/// A four-point closed diamond — a path with curves would flatten to more
/// lines but the same strip contract.
fn diamond() -> BezPath {
    let mut path = BezPath::new();
    path.move_to((100.0, 40.0));
    path.line_to((160.0, 100.0));
    path.line_to((100.0, 160.0));
    path.line_to((40.0, 100.0));
    path.close_path();
    path
}

#[test]
fn a_pixel_aligned_fill_rect_takes_the_fast_rectangle_path() {
    let scene = scene_of(|b| b.fill_rect(Rect::new(20.0, 20.0, 100.0, 80.0), solid(RED)));

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles");

    assert_eq!(frame.fast_rect_draws, 1);
    assert_eq!(frame.draws().len(), 1);
    assert!(!frame.draws()[0].strip_range.is_empty());
}

#[test]
fn a_rotated_fill_rect_does_not_take_the_fast_rectangle_path() {
    let scene = scene_of(|b| b.fill_rect(Rect::new(60.0, 60.0, 140.0, 140.0), solid(RED)));
    // Rotated about the rect's own centre, so it stays inside the viewport and
    // the difference under test is the transform's shape, not culling.
    let root = Affine::rotate_about(0.4, Point::new(100.0, 100.0));

    let frame = compiler()
        .compile(&scene, root, VIEWPORT)
        .expect("an in-range scene compiles");

    assert_eq!(frame.fast_rect_draws, 0);
    assert_eq!(frame.draws().len(), 1);
    assert!(!frame.draws()[0].strip_range.is_empty());
}

#[test]
fn an_axis_aligned_but_fractional_fill_rect_does_not_take_the_fast_rectangle_path() {
    // Axis-aligned, so only the half-pixel edges disqualify it: the fast path
    // is admitted on pixel alignment, not merely on the absence of rotation.
    let scene = scene_of(|b| b.fill_rect(Rect::new(20.5, 20.5, 100.5, 80.5), solid(RED)));

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles");

    assert_eq!(frame.fast_rect_draws, 0);
    assert_eq!(frame.draws().len(), 1);
    assert!(!frame.draws()[0].strip_range.is_empty());
}

#[test]
fn a_non_integer_root_transform_disqualifies_an_integer_rect() {
    let scene = scene_of(|b| b.fill_rect(Rect::new(20.0, 20.0, 100.0, 80.0), solid(RED)));

    let frame = compiler()
        .compile(&scene, Affine::translate((0.25, 0.0)), VIEWPORT)
        .expect("an in-range scene compiles");

    assert_eq!(frame.fast_rect_draws, 0);
    assert_eq!(frame.draws().len(), 1);
}

#[test]
fn every_geometry_command_produces_strips() {
    let cases: Vec<(&str, Scene)> = vec![
        (
            "fill_rect",
            scene_of(|b| b.fill_rect(Rect::new(20.0, 20.0, 100.0, 80.0), solid(RED))),
        ),
        (
            "rounded_rect",
            scene_of(|b| b.fill_rounded_rect(Rect::new(20.0, 20.0, 100.0, 80.0), 12.0, solid(RED))),
        ),
        (
            "line",
            scene_of(|b| {
                b.stroke_line(
                    Point::new(20.0, 20.0),
                    Point::new(160.0, 120.0),
                    4.0,
                    solid(RED),
                )
            }),
        ),
        (
            "path_fill",
            scene_of(|b| b.fill_path(diamond(), solid(RED))),
        ),
        (
            "path_stroke",
            scene_of(|b| b.stroke_path(diamond(), 3.0, solid(RED))),
        ),
        (
            "path_stroke_dashed",
            scene_of(|b| {
                b.stroke_path_dashed(diamond(), 3.0, DashPattern::new(8.0, 4.0), solid(RED))
            }),
        ),
    ];

    for (name, scene) in cases {
        let frame = compiler()
            .compile(&scene, Affine::IDENTITY, VIEWPORT)
            .unwrap_or_else(|e| panic!("{name} should compile, got {e}"));

        assert_eq!(frame.draws().len(), 1, "{name} should record one draw");
        assert!(
            !frame.draws()[0].strip_range.is_empty(),
            "{name} should produce strips"
        );
        assert!(!frame.alphas().is_empty(), "{name} should produce alphas");
    }
}

#[test]
fn a_rounded_rect_uses_each_corner_radius() {
    // A rect rounded on one corner only must differ from the same rect rounded
    // on all four — proof the per-corner radii reach the shape rather than
    // being collapsed to a single value.
    let rect = Rect::new(20.0, 20.0, 120.0, 120.0);
    let one_corner = scene_of(|b| {
        b.fill_rounded_rect_radii(
            rect,
            frust_scene::CornerRadii::new(40.0, 0.0, 0.0, 0.0),
            solid(RED),
        )
    });
    let all_corners = scene_of(|b| b.fill_rounded_rect(rect, 40.0, solid(RED)));

    let a = compiler()
        .compile(&one_corner, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    let b = compiler()
        .compile(&all_corners, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert_ne!(a.alphas(), b.alphas());
}

#[test]
fn a_dashed_stroke_differs_from_the_solid_stroke_of_the_same_path() {
    let solid_stroke = scene_of(|b| b.stroke_path(diamond(), 3.0, solid(RED)));
    let dashed =
        scene_of(|b| b.stroke_path_dashed(diamond(), 3.0, DashPattern::new(8.0, 8.0), solid(RED)));

    let a = compiler()
        .compile(&solid_stroke, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    let b = compiler()
        .compile(&dashed, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert_ne!(
        a.alphas(),
        b.alphas(),
        "the dash pattern must be expanded before stroking"
    );
}

#[test]
fn a_degenerate_dash_pattern_strokes_solid() {
    // `DashPattern::is_effective` rejects a zero-length `on`; such a pattern
    // must fall back to the plain stroke rather than expanding into nothing.
    let solid_stroke = scene_of(|b| b.stroke_path(diamond(), 3.0, solid(RED)));
    let degenerate =
        scene_of(|b| b.stroke_path_dashed(diamond(), 3.0, DashPattern::new(0.0, 4.0), solid(RED)));

    let a = compiler()
        .compile(&solid_stroke, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    let b = compiler()
        .compile(&degenerate, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert_eq!(a.alphas(), b.alphas());
}

#[test]
fn draws_carry_monotone_depths_from_the_back_most() {
    let scene = scene_of(|b| {
        b.fill_rect(Rect::new(10.0, 10.0, 60.0, 60.0), solid(RED));
        b.fill_rect(Rect::new(30.0, 30.0, 80.0, 80.0), solid(BLUE));
        b.fill_path(diamond(), solid(RED));
        b.stroke_line(
            Point::new(10.0, 190.0),
            Point::new(190.0, 10.0),
            2.0,
            solid(BLUE),
        );
    });

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    let depths: Vec<u32> = frame.draws().iter().map(|d| d.depth).collect();
    assert_eq!(depths, vec![0, 1, 2, 3]);
}

#[test]
fn strip_ranges_partition_the_shared_strip_buffer_in_order() {
    let scene = scene_of(|b| {
        b.fill_rect(Rect::new(10.0, 10.0, 60.0, 60.0), solid(RED));
        b.fill_rounded_rect(Rect::new(70.0, 70.0, 150.0, 150.0), 10.0, solid(BLUE));
        b.fill_path(diamond(), solid(RED));
    });

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    let mut cursor = 0;
    for draw in frame.draws() {
        assert_eq!(draw.strip_range.start, cursor, "ranges must be contiguous");
        assert!(draw.strip_range.end > draw.strip_range.start);
        cursor = draw.strip_range.end;
    }
    assert_eq!(cursor, frame.strip_buf().len());
}

#[test]
fn an_empty_scene_compiles_to_nothing() {
    let frame = compiler()
        .compile(&Scene::new(), Affine::IDENTITY, VIEWPORT)
        .expect("an empty scene compiles");

    assert!(frame.draws().is_empty());
    assert!(frame.strip_buf().is_empty());
    assert!(frame.encoded_paints.is_empty());
    assert_eq!(frame.fast_rect_draws, 0);
}

#[test]
fn commands_outside_the_compiler_scope_are_skipped_without_disturbing_depths() {
    // Clips and layers are recognised but not yet compiled; the geometry
    // between them must still record, with depths dense over what survived.
    let scene = scene_of(|b| {
        b.push_clip(Rect::new(0.0, 0.0, 200.0, 200.0));
        b.fill_rect(Rect::new(10.0, 10.0, 60.0, 60.0), solid(RED));
        b.push_layer(Rect::new(0.0, 0.0, 200.0, 200.0), 0.5);
        b.fill_path(diamond(), solid(BLUE));
        b.pop_layer();
        b.pop_clip();
    });

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    let depths: Vec<u32> = frame.draws().iter().map(|d| d.depth).collect();
    assert_eq!(depths, vec![0, 1]);
}

#[test]
fn a_root_transform_composes_onto_each_command() {
    let scene = scene_of(|b| b.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), solid(RED)));

    let untranslated = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    let translated = compiler()
        .compile(&scene, Affine::translate((60.0, 60.0)), VIEWPORT)
        .expect("compiles");

    let a = untranslated.strip_buf()[0];
    let b = translated.strip_buf()[0];
    assert_ne!((a.x, a.y), (b.x, b.y));
}

#[test]
fn a_builder_transform_composes_under_the_root() {
    let scene = scene_of(|b| {
        b.push_transform(Affine::translate((50.0, 0.0)));
        b.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), solid(RED));
        b.pop_transform();
    });

    let shifted = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    let plain = scene_of(|b| b.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), solid(RED)));
    let plain = compiler()
        .compile(&plain, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert_ne!(shifted.strip_buf()[0].x, plain.strip_buf()[0].x);
}

#[test]
fn a_nan_command_transform_is_refused_rather_than_rasterized() {
    let scene = scene_of(|b| {
        b.push_transform(Affine::scale(f64::NAN));
        b.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), solid(RED));
        b.pop_transform();
    });

    assert!(matches!(
        compiler().compile(&scene, Affine::IDENTITY, VIEWPORT),
        Err(EngineError::TargetTooLarge)
    ));
}

#[test]
fn a_nan_root_transform_is_refused_rather_than_rasterized() {
    let scene = scene_of(|b| b.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), solid(RED)));

    assert!(matches!(
        compiler().compile(&scene, Affine::translate((f64::NAN, 0.0)), VIEWPORT),
        Err(EngineError::TargetTooLarge)
    ));
}

#[test]
fn an_infinite_transform_is_refused_rather_than_rasterized() {
    let scene = scene_of(|b| b.fill_path(diamond(), solid(RED)));

    assert!(matches!(
        compiler().compile(&scene, Affine::scale(f64::INFINITY), VIEWPORT),
        Err(EngineError::TargetTooLarge)
    ));
}

#[test]
fn a_refused_frame_records_nothing_at_all() {
    // The whole scene is validated before any strip is generated, so a frame
    // rejected for its last command leaves no partial recording behind.
    let scene = scene_of(|b| {
        b.fill_rect(Rect::new(10.0, 10.0, 60.0, 60.0), solid(RED));
        b.push_transform(Affine::scale(f64::NAN));
        b.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), solid(BLUE));
        b.pop_transform();
    });

    let mut compiler = compiler();
    assert!(
        compiler
            .compile(&scene, Affine::IDENTITY, VIEWPORT)
            .is_err()
    );

    // The same compiler still works for a well-formed frame afterwards.
    let good = scene_of(|b| b.fill_rect(Rect::new(10.0, 10.0, 60.0, 60.0), solid(RED)));
    let frame = compiler
        .compile(&good, Affine::IDENTITY, VIEWPORT)
        .expect("a clean frame compiles after a refused one");
    assert_eq!(frame.draws().len(), 1);
}

#[test]
fn a_viewport_that_cannot_be_tile_aligned_is_refused() {
    let scene = scene_of(|b| b.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), solid(RED)));

    assert!(matches!(
        compiler().compile(&scene, Affine::IDENTITY, (u16::MAX, 64)),
        Err(EngineError::TargetTooLarge)
    ));
    assert!(matches!(
        compiler().compile(&scene, Affine::IDENTITY, (64, u16::MAX)),
        Err(EngineError::TargetTooLarge)
    ));
}

#[test]
fn a_compiler_reused_across_frames_starts_each_one_empty() {
    let mut compiler = compiler();
    let scene = scene_of(|b| b.fill_rect(Rect::new(10.0, 10.0, 60.0, 60.0), solid(RED)));

    let first = compiler
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    let second = compiler
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert_eq!(first.draws().len(), second.draws().len());
    assert_eq!(first.strip_buf().len(), second.strip_buf().len());
    assert_eq!(first.alphas().len(), second.alphas().len());
    assert_eq!(first.draws()[0].strip_range, second.draws()[0].strip_range);
}

#[test]
fn geometry_entirely_outside_the_viewport_records_no_draw() {
    let scene = scene_of(|b| {
        b.fill_rect(Rect::new(1000.0, 1000.0, 1100.0, 1100.0), solid(RED));
    });

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert!(frame.draws().is_empty());
}
