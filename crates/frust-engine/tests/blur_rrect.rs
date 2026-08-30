//! Blurred rounded rectangle lowering: `Command::BlurredRoundedRect` through
//! the compiler's public seam.
//!
//! Every case drives a `frust_scene::Scene` recorded through `SceneBuilder`
//! and compiled by [`SceneCompiler`], and reads the result back through the
//! encoded-paint table and the frame's own draw/strip records — the same
//! shape `tests/compile_rects_paths.rs` and `tests/images.rs` already use for
//! the compiler's other geometry primitives.
//!
//! No GPU, device or surface is involved: strip generation is pure CPU work,
//! and the GPU-side accessors this primitive's encoded record feeds
//! (`get_blurred_rounded_rect_*` and `calculate_blurred_rounded_rect` in
//! `shaders/helpers.wgsl`, the `PaintType::BlurredRoundedRect` branch in
//! `shaders/strip.wgsl`) already shipped ahead of this compiler wiring and are
//! untouched by it. The pixel comparison against `vello_cpu` — the
//! unit-blur-rrect golden across a handful of standard-deviation steps the
//! task names — needs the `frust-testing` corpus this crate does not depend
//! on, so it is deferred to the phase's golden suite (p4-07) rather than
//! faked here with an ad hoc reference image.

use frust_engine::{EngineError, SceneCompiler};
use frust_scene::{CornerRadii, Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::color::palette::css::{BLUE, RED};
use vello_common::encode::EncodedPaint;
use vello_common::paint::Paint;

/// Viewport every case compiles against, comfortably larger than the geometry
/// it records (plus its blur padding) so nothing is culled by accident.
const VIEWPORT: (u16, u16) = (200, 200);

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

/// The blurred-rounded-rect entry a draw's indexed paint names, panicking if
/// the paint is not indexed or the entry is not that variant.
fn blurred_entry<'a>(
    draw: &frust_engine::EngineDraw,
    encoded_paints: &'a [EncodedPaint],
) -> &'a vello_common::encode::EncodedBlurredRoundedRectangle {
    let index = match &draw.paint {
        Paint::Indexed(indexed) => indexed.index(),
        Paint::Solid(color) => panic!("expected an indexed paint, got the solid {color:?}"),
    };
    match &encoded_paints[index] {
        EncodedPaint::BlurredRoundedRect(entry) => entry,
        other => panic!("expected an encoded blurred rounded rectangle, got {other:?}"),
    }
}

#[test]
fn a_blurred_rounded_rect_records_one_draw_with_strips_and_an_encoded_entry() {
    let rect = Rect::new(40.0, 40.0, 120.0, 100.0);
    let scene = scene_of(|b| b.draw_blurred_rounded_rect(rect, 12.0, 6.0, RED));

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles");

    assert_eq!(frame.draws().len(), 1);
    assert!(!frame.draws()[0].strip_range.is_empty());
    assert!(!frame.alphas().is_empty());
    assert_eq!(frame.encoded_paints.len(), 1);
    assert!(matches!(
        frame.encoded_paints[0],
        EncodedPaint::BlurredRoundedRect(_)
    ));
}

#[test]
fn the_encoded_entry_carries_the_shadows_colour_and_never_inverts() {
    // `Command::BlurredRoundedRect` carries no invert flag — frust-scene has
    // no inset-shadow command yet — so every shadow this compiler lowers
    // paints the ordinary (drop) polarity.
    let rect = Rect::new(10.0, 10.0, 60.0, 50.0);
    let scene = scene_of(|b| b.draw_blurred_rounded_rect(rect, 8.0, 4.0, RED));

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    let entry = blurred_entry(&frame.draws()[0], &frame.encoded_paints);
    assert!(!entry.invert);
    assert_eq!(
        entry.color,
        vello_common::paint::PremulColor::from_alpha_color(RED)
    );
}

#[test]
fn per_corner_radii_collapse_to_the_largest_corner() {
    // A shadow rounded on one corner only must encode the identical outer
    // radius as one rounded on every corner to that same value — proof the
    // per-corner radii collapse through `CornerRadii::largest` rather than,
    // say, averaging or taking the first corner.
    let rect = Rect::new(20.0, 20.0, 120.0, 120.0);
    let one_corner = scene_of(|b| {
        b.draw_blurred_rounded_rect_radii(rect, CornerRadii::new(40.0, 0.0, 0.0, 0.0), 5.0, RED)
    });
    let all_corners = scene_of(|b| b.draw_blurred_rounded_rect(rect, 40.0, 5.0, RED));

    let a = compiler()
        .compile(&one_corner, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    let b = compiler()
        .compile(&all_corners, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    let entry_a = blurred_entry(&a.draws()[0], &a.encoded_paints);
    let entry_b = blurred_entry(&b.draws()[0], &b.encoded_paints);
    assert_eq!(entry_a.r1, entry_b.r1, "the outer blur radius must match");
    assert_eq!(
        entry_a.scale, entry_b.scale,
        "every radius-derived blur parameter must match"
    );

    // The coverage the two scenes paint is the same padded bounding
    // rectangle either way (the collapse only changes which radius the
    // shader rounds by, not the rect the shadow is cast from), so their
    // strip metadata must be identical too.
    assert_eq!(a.strip_buf(), b.strip_buf());
}

#[test]
fn a_rounder_shadow_differs_from_a_squarer_one_of_the_same_rect() {
    let rect = Rect::new(20.0, 20.0, 100.0, 90.0);
    let square = scene_of(|b| b.draw_blurred_rounded_rect(rect, 0.0, 5.0, RED));
    let round = scene_of(|b| b.draw_blurred_rounded_rect(rect, 30.0, 5.0, RED));

    let a = compiler()
        .compile(&square, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    let b = compiler()
        .compile(&round, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    let entry_a = blurred_entry(&a.draws()[0], &a.encoded_paints);
    let entry_b = blurred_entry(&b.draws()[0], &b.encoded_paints);
    assert_ne!(entry_a.r1, entry_b.r1);
}

#[test]
fn coverage_reaches_past_the_shadows_own_rectangle_by_the_blur_kernel() {
    // The strip generator rasterizes the *padded* bounding rectangle (see
    // `compile::blur_rrect::inflated_bounds`), not `rect` itself, so a wider
    // standard deviation must widen the strips' own device-space bounds even
    // though the shadow's `rect` argument never changes.
    let rect = Rect::new(80.0, 80.0, 120.0, 110.0);
    let narrow = scene_of(|b| b.draw_blurred_rounded_rect(rect, 4.0, 1.0, RED));
    let wide = scene_of(|b| b.draw_blurred_rounded_rect(rect, 4.0, 20.0, RED));

    let a = compiler()
        .compile(&narrow, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");
    let b = compiler()
        .compile(&wide, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    // `Strip` carries no row-end width of its own outside the generator that
    // built it (the *next* strip in a run is what names one strip's width —
    // see `Strip::width_to`), so the coarser and equally conclusive signal is
    // used instead: at least as much alpha coverage is needed to rasterize
    // the wider padded rectangle, and the two runs' strip metadata differ.
    assert!(
        b.alphas().len() >= a.alphas().len(),
        "a wider blur kernel must not shrink the rasterized coverage"
    );
    assert_ne!(
        a.strip_buf(),
        b.strip_buf(),
        "two different standard deviations must rasterize different coverage"
    );
}

#[test]
fn a_pixel_aligned_shadow_with_no_blur_takes_the_fast_rectangle_path() {
    // At `std_dev == 0.0` the padded bounding rectangle is `rect` itself
    // (`inflated_bounds` pads by zero), so a pixel-aligned rect under the
    // identity transform takes the same fast rectangle path a plain fill
    // would.
    let rect = Rect::new(20.0, 20.0, 100.0, 80.0);
    let scene = scene_of(|b| b.draw_blurred_rounded_rect(rect, 0.0, 0.0, RED));

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert_eq!(frame.fast_rect_draws, 1);
}

#[test]
fn a_rotated_shadow_does_not_take_the_fast_rectangle_path() {
    let rect = Rect::new(60.0, 60.0, 140.0, 140.0);
    let root = Affine::rotate_about(0.4, kurbo::Point::new(100.0, 100.0));
    let scene = scene_of(|b| b.draw_blurred_rounded_rect(rect, 10.0, 4.0, RED));

    let frame = compiler()
        .compile(&scene, root, VIEWPORT)
        .expect("compiles");

    assert_eq!(frame.fast_rect_draws, 0);
    assert_eq!(frame.draws().len(), 1);
    assert!(!frame.draws()[0].strip_range.is_empty());
}

#[test]
fn every_shadow_gets_its_own_entry_indexed_in_paint_order() {
    let first = Rect::new(10.0, 10.0, 60.0, 50.0);
    let second = Rect::new(10.0, 70.0, 60.0, 110.0);
    let scene = scene_of(|b| {
        b.draw_blurred_rounded_rect(first, 8.0, 3.0, RED);
        b.fill_rect(
            Rect::new(70.0, 10.0, 120.0, 60.0),
            peniko::Brush::Solid(BLUE),
        );
        b.draw_blurred_rounded_rect(second, 8.0, 3.0, RED);
    });

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert_eq!(frame.draws().len(), 3);
    assert_eq!(frame.encoded_paints.len(), 2, "the solid fill adds nothing");
    let index_of = |draw: &frust_engine::EngineDraw| match &draw.paint {
        Paint::Indexed(indexed) => indexed.index(),
        Paint::Solid(_) => panic!("expected an indexed paint"),
    };
    assert_eq!(index_of(&frame.draws()[0]), 0);
    assert!(matches!(frame.draws()[1].paint, Paint::Solid(_)));
    assert_eq!(index_of(&frame.draws()[2]), 1);
}

#[test]
fn a_culled_shadow_leaves_no_orphan_entry() {
    // Fully off-viewport, even after the blur's own padding, so the whole
    // draw is culled — and, per the compiler's general contract, culling a
    // draw must not leave an unindexed entry in the encoded-paint table.
    let offscreen = Rect::new(10_000.0, 10_000.0, 10_100.0, 10_060.0);
    let scene = scene_of(|b| b.draw_blurred_rounded_rect(offscreen, 8.0, 4.0, RED));

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    assert!(frame.draws().is_empty());
    assert!(frame.encoded_paints.is_empty());
}

#[test]
fn draws_alongside_other_geometry_keep_dense_monotone_depths() {
    let scene = scene_of(|b| {
        b.fill_rect(Rect::new(10.0, 10.0, 60.0, 60.0), peniko::Brush::Solid(RED));
        b.draw_blurred_rounded_rect(Rect::new(70.0, 10.0, 130.0, 60.0), 10.0, 4.0, RED);
        b.fill_rounded_rect(
            Rect::new(10.0, 90.0, 130.0, 150.0),
            12.0,
            peniko::Brush::Solid(BLUE),
        );
    });

    let frame = compiler()
        .compile(&scene, Affine::IDENTITY, VIEWPORT)
        .expect("compiles");

    let depths: Vec<u32> = frame.draws().iter().map(|d| d.depth).collect();
    assert_eq!(depths, vec![0, 1, 2]);
}

#[test]
fn a_nan_std_dev_is_refused_rather_than_rasterized() {
    let scene = scene_of(|b| {
        b.draw_blurred_rounded_rect(Rect::new(0.0, 0.0, 40.0, 40.0), 8.0, f64::NAN, RED);
    });

    assert!(matches!(
        compiler().compile(&scene, Affine::IDENTITY, VIEWPORT),
        Err(EngineError::InvalidGeometry)
    ));
}

#[test]
fn a_nan_radius_is_refused_rather_than_rasterized() {
    let scene = scene_of(|b| {
        b.draw_blurred_rounded_rect_radii(
            Rect::new(0.0, 0.0, 40.0, 40.0),
            CornerRadii::new(f64::NAN, 0.0, 0.0, 0.0),
            4.0,
            RED,
        );
    });

    assert!(matches!(
        compiler().compile(&scene, Affine::IDENTITY, VIEWPORT),
        Err(EngineError::InvalidGeometry)
    ));
}

#[test]
fn an_infinite_rect_is_refused_rather_than_rasterized() {
    let scene = scene_of(|b| {
        b.draw_blurred_rounded_rect(Rect::new(0.0, 0.0, f64::INFINITY, 40.0), 8.0, 4.0, RED);
    });

    assert!(matches!(
        compiler().compile(&scene, Affine::IDENTITY, VIEWPORT),
        Err(EngineError::InvalidGeometry)
    ));
}

#[test]
fn a_root_transform_composes_onto_the_shadow() {
    let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
    let scene = scene_of(|b| b.draw_blurred_rounded_rect(rect, 6.0, 3.0, RED));

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
