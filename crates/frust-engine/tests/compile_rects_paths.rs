//! Scene-compiler behaviour over the geometry commands it lowers.
//!
//! Every case drives the public seam — a `frust_scene::Scene` recorded through
//! `SceneBuilder`, compiled by [`SceneCompiler`] — so the assertions pin the
//! contract a GPU pass consumes rather than the compiler's internals. No GPU,
//! device, or surface is involved: strip generation is pure CPU work.

use frust_engine::compile::paint::LutRequest;
use frust_engine::{EngineError, SceneCompiler};
use frust_scene::{DashPattern, Scene, SceneBuilder};
use kurbo::{Affine, BezPath, Point, Rect};
use peniko::color::palette::css::{BLUE, RED};
use peniko::{Brush, Color, Gradient};
use vello_common::encode::EncodedPaint;
use vello_common::paint::Paint;

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
fn a_zero_length_closed_subpath_changes_nothing_about_what_a_dashed_stroke_paints() {
    // The dash lowering drops a subpath that closes without ever leaving its
    // start point, because `kurbo::dash` emits such a subpath's closing element
    // ahead of the `MoveTo` that should open its output. Dropping it must be
    // exactly that — a removal of nothing — so the same path carrying one in
    // front, behind, and in the middle of its real geometry paints the diamond
    // and only the diamond.
    let dash = DashPattern::new(8.0, 4.0);
    let plain = scene_of(|b| b.stroke_path_dashed(diamond(), 3.0, dash, solid(RED)));
    let expected = compiler()
        .compile(&plain, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    assert!(
        !expected.alphas().is_empty(),
        "the reference dashed stroke must itself paint something"
    );

    let mut leading = BezPath::new();
    leading.move_to((10.0, 10.0));
    leading.close_path();
    leading.extend(diamond().iter());

    let mut trailing = diamond();
    trailing.move_to((10.0, 10.0));
    trailing.close_path();

    let mut doubled = diamond();
    doubled.close_path();

    for (name, path) in [
        ("leading", leading),
        ("trailing", trailing),
        ("doubled close", doubled),
    ] {
        let scene = scene_of(|b| b.stroke_path_dashed(path, 3.0, dash, solid(RED)));
        let frame = compiler()
            .compile(&scene, Affine::IDENTITY, VIEWPORT)
            .unwrap_or_else(|e| panic!("{name} should compile, got {e}"));

        // Alpha coverage alone would miss a change that shifted strips without
        // changing what they cover (a different draw count or a different
        // paint on an otherwise-identical strip range would not touch a single
        // alpha byte), so the draws and the strip buffer they index are pinned
        // too.
        assert_eq!(
            frame.alphas(),
            expected.alphas(),
            "a {name} zero-length closed subpath changed the dashed stroke's coverage"
        );
        assert_eq!(
            frame.strip_buf(),
            expected.strip_buf(),
            "a {name} zero-length closed subpath changed the dashed stroke's strip metadata"
        );
        assert_eq!(
            frame.draws().len(),
            expected.draws().len(),
            "a {name} zero-length closed subpath changed the dashed stroke's draw count"
        );
        for (index, (a, b)) in frame.draws().iter().zip(expected.draws()).enumerate() {
            assert_eq!(
                a.paint, b.paint,
                "a {name} zero-length closed subpath changed draw {index}'s paint"
            );
            assert_eq!(
                a.depth, b.depth,
                "a {name} zero-length closed subpath changed draw {index}'s depth"
            );
            assert_eq!(
                a.strip_range, b.strip_range,
                "a {name} zero-length closed subpath changed draw {index}'s strip range"
            );
        }
    }
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
    // Layers are recognised but not yet compiled; a clip is compiled but
    // records no draw of its own. The geometry between them must still record,
    // with depths dense over what survived.
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
        Err(EngineError::InvalidTransform)
    ));
}

#[test]
fn a_nan_root_transform_is_refused_rather_than_rasterized() {
    let scene = scene_of(|b| b.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), solid(RED)));

    assert!(matches!(
        compiler().compile(&scene, Affine::translate((f64::NAN, 0.0)), VIEWPORT),
        Err(EngineError::InvalidTransform)
    ));
}

#[test]
fn an_infinite_transform_is_refused_rather_than_rasterized() {
    let scene = scene_of(|b| b.fill_path(diamond(), solid(RED)));

    assert!(matches!(
        compiler().compile(&scene, Affine::scale(f64::INFINITY), VIEWPORT),
        Err(EngineError::InvalidTransform)
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

// ---------------------------------------------------------------------
// Paint encoding: which brush leaves which entry in the frame's side table
// ---------------------------------------------------------------------

/// A two-stop linear gradient running left to right across `rect`.
fn linear(rect: Rect) -> Brush {
    Brush::Gradient(
        Gradient::new_linear((rect.x0, rect.y0), (rect.x1, rect.y0)).with_stops([RED, BLUE]),
    )
}

/// The index of `draw`'s paint in the frame's encoded-paint table.
fn indexed(draw: &frust_engine::EngineDraw) -> usize {
    match &draw.paint {
        Paint::Indexed(indexed) => indexed.index(),
        Paint::Solid(color) => panic!("expected an indexed paint, got the solid {color:?}"),
    }
}

#[test]
fn a_solid_only_scene_encodes_no_paint_entry_at_all() {
    let scene = scene_of(|b| {
        b.fill_rect(Rect::new(10.0, 10.0, 60.0, 60.0), solid(RED));
        b.fill_path(diamond(), solid(BLUE));
    });

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert_eq!(frame.draws().len(), 2);
    assert!(
        frame.encoded_paints.is_empty(),
        "a solid colour travels inside the draw, costing no side-table entry"
    );
    assert!(frame.lut_requests.is_empty());
    for draw in frame.draws() {
        assert!(matches!(draw.paint, Paint::Solid(_)));
    }
}

#[test]
fn a_gradient_brush_encodes_an_indexed_paint_and_asks_for_its_ramp() {
    let rect = Rect::new(20.0, 20.0, 120.0, 80.0);
    let scene = scene_of(|b| b.fill_rect(rect, linear(rect)));

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert_eq!(frame.draws().len(), 1);
    let index = indexed(&frame.draws()[0]);
    assert_eq!(frame.encoded_paints.len(), 1);
    assert!(matches!(
        frame.encoded_paints[index],
        EncodedPaint::Gradient(_)
    ));
    assert_eq!(
        frame.lut_requests,
        vec![LutRequest { paint_index: index }],
        "the frame names the ramp the renderer must make resident"
    );
}

#[test]
fn every_gradient_draw_gets_its_own_entry_indexed_in_paint_order() {
    let first = Rect::new(10.0, 10.0, 90.0, 50.0);
    let second = Rect::new(10.0, 60.0, 90.0, 100.0);
    let scene = scene_of(|b| {
        b.fill_rect(first, linear(first));
        b.fill_rect(second, solid(RED));
        b.fill_path(diamond(), linear(second));
    });

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert_eq!(frame.draws().len(), 3);
    assert_eq!(frame.encoded_paints.len(), 2, "the solid draw adds nothing");
    assert_eq!(indexed(&frame.draws()[0]), 0);
    assert!(matches!(frame.draws()[1].paint, Paint::Solid(_)));
    assert_eq!(indexed(&frame.draws()[2]), 1);
    assert_eq!(
        frame.lut_requests,
        vec![LutRequest { paint_index: 0 }, LutRequest { paint_index: 1 }]
    );
}

#[test]
fn a_culled_gradient_draw_leaves_no_orphan_entry() {
    // Off-viewport geometry records no draw, so the brush it would have been
    // painted with must not reach the side table either — an entry nothing
    // indexes would still be lowered, uploaded and made resident.
    let offscreen = Rect::new(1000.0, 1000.0, 1100.0, 1100.0);
    let scene = scene_of(|b| b.fill_rect(offscreen, linear(offscreen)));

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert!(frame.draws().is_empty());
    assert!(frame.encoded_paints.is_empty());
    assert!(frame.lut_requests.is_empty());
}

#[test]
fn an_image_brush_encodes_an_indexed_paint_backed_by_atlas_residency() {
    let image = peniko::ImageBrush::new(peniko::ImageData {
        data: peniko::Blob::new(std::sync::Arc::new(vec![255_u8, 0, 0, 255])),
        format: peniko::ImageFormat::Rgba8,
        alpha_type: peniko::ImageAlphaType::Alpha,
        width: 1,
        height: 1,
    });
    let scene = scene_of(|b| {
        b.fill_rect(Rect::new(10.0, 10.0, 60.0, 60.0), Brush::Image(image));
    });

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert_eq!(frame.draws().len(), 1);
    assert_eq!(frame.image_draws, 1);
    assert_eq!(frame.skipped_images, 0);
    assert_eq!(frame.encoded_paints.len(), 1, "one image entry");
    assert!(matches!(
        &frame.encoded_paints[0],
        vello_common::encode::EncodedPaint::Image(_)
    ));
    // The brush's pixels become resident on this first frame, so the frame
    // carries exactly one upload for them.
    assert_eq!(frame.image_uploads.len(), 1);
    match &frame.draws()[0].paint {
        Paint::Indexed(indexed) => assert_eq!(indexed.index(), 0),
        Paint::Solid(_) => panic!("an image brush encodes an indexed paint"),
    }
}
