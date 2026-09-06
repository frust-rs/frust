//! Paint encoding and gradient-ramp residency.
//!
//! Covers the two halves of the paint path against the public seam only: which
//! `peniko::Brush` becomes which encoded paint, and how the ramps those paints ask
//! for are packed, shared and reclaimed. The image and blurred-rounded-rect cases
//! at the bottom cover the GPU-side half too — lowering an `EncodedPaint` into the
//! record the strip shader reads, at the host level: record bytes, texel offsets,
//! and the paint descriptor a strip instance names a record by.

use std::sync::Arc;

use frust_engine::cache::images::{AtlasBudget, ImageResidency};
use frust_engine::cache::{GradientCache, GradientTextureLayout};
use frust_engine::compile::blur_rrect::encode_blurred_rounded_rect;
use frust_engine::compile::paint::{encode_brush, encode_image_command, resolve_lut_request};
use frust_engine::gpu::atlas::lower_encoded_image;
use frust_engine::gpu::paint_texture::lower_encoded_paint;
use frust_engine::gpu::strips::{PAINT_TEXTURE_INDEX_MASK, PaintType, pack_paint_descriptor};
use frust_engine::gpu::{GpuBlurredRoundedRect, GpuEncodedImage, GpuEncodedPaint};
use frust_scene::CornerRadii;
use peniko::color::palette::css::RED;
use peniko::color::{ColorSpaceTag, DynamicColor, HueDirection, PremulRgba8};
use peniko::{
    Blob, Brush, Color, ColorStop, ColorStops, Gradient, GradientKind, ImageAlphaType, ImageData,
    ImageFormat, LinearGradientPosition, RadialGradientPosition, SweepGradientPosition,
};
use vello_common::encode::{EncodedKind, EncodedPaint};
use vello_common::fearless_simd::Level;
use vello_common::kurbo::{Affine, Point, Rect};
use vello_common::paint::{Paint, PremulColor};

/// Three stops with the middle one at `offset`, so varying `offset` varies the
/// baked ramp — and therefore the cache key — while everything else stays equal.
fn stops(offset: f32) -> ColorStops {
    ColorStops(
        vec![
            ColorStop {
                offset: 0.0,
                color: DynamicColor::from_alpha_color(Color::from_rgb8(255, 0, 0)),
            },
            ColorStop {
                offset,
                color: DynamicColor::from_alpha_color(Color::from_rgb8(0, 255, 0)),
            },
            ColorStop {
                offset: 1.0,
                color: DynamicColor::from_alpha_color(Color::from_rgb8(0, 0, 255)),
            },
        ]
        .into(),
    )
}

fn gradient(kind: GradientKind, offset: f32) -> Gradient {
    Gradient {
        kind,
        stops: stops(offset),
        interpolation_cs: ColorSpaceTag::Srgb,
        hue_direction: HueDirection::Shorter,
        ..Default::default()
    }
}

fn linear_kind() -> GradientKind {
    LinearGradientPosition {
        start: Point::new(0.0, 0.0),
        end: Point::new(100.0, 0.0),
    }
    .into()
}

fn radial_kind() -> GradientKind {
    RadialGradientPosition {
        start_center: Point::new(0.0, 0.0),
        start_radius: 0.0,
        end_center: Point::new(0.0, 0.0),
        end_radius: 50.0,
    }
    .into()
}

fn sweep_kind() -> GradientKind {
    SweepGradientPosition {
        center: Point::new(50.0, 50.0),
        start_angle: 0.0,
        end_angle: std::f32::consts::TAU,
    }
    .into()
}

fn linear_brush(offset: f32) -> Brush {
    Brush::Gradient(gradient(linear_kind(), offset))
}

fn cache(capacity: u32) -> GradientCache {
    GradientCache::new(capacity, Level::new())
}

/// Encode `brush` and make its ramp resident, returning the encoded paints.
fn encode_resident(brushes: &[Brush], cache: &mut GradientCache) -> Vec<EncodedPaint> {
    let mut paints = Vec::new();
    for brush in brushes {
        let encoding = encode_brush(brush, Affine::IDENTITY, &mut paints);
        if let Some(request) = encoding.lut_request {
            resolve_lut_request(request, &paints, cache);
        }
    }
    paints
}

fn premul(paint: &Paint) -> PremulRgba8 {
    match paint {
        Paint::Solid(color) => color.as_premul_rgba8(),
        Paint::Indexed(_) => panic!("expected a solid paint"),
    }
}

#[test]
fn solid_brush_encodes_to_a_premultiplied_paint_with_no_side_table_entry() {
    let mut paints = Vec::new();
    let brush = Brush::Solid(Color::from_rgba8(255, 0, 0, 128));

    let encoding = encode_brush(&brush, Affine::IDENTITY, &mut paints);

    assert!(paints.is_empty(), "a solid brush needs no encoded entry");
    assert_eq!(encoding.lut_request, None);
    let rgba = premul(&encoding.paint);
    assert_eq!(rgba.a, 128);
    assert!(
        rgba.r < 255,
        "colour should be premultiplied by alpha, got {rgba:?}"
    );
}

fn kind_label(kind: &EncodedKind) -> &'static str {
    match kind {
        EncodedKind::Linear(_) => "linear",
        EncodedKind::Radial(_) => "radial",
        EncodedKind::Sweep(_) => "sweep",
    }
}

#[test]
fn each_gradient_kind_encodes_to_its_encoded_kind() {
    for (kind, expected) in [
        (linear_kind(), "linear"),
        (radial_kind(), "radial"),
        (sweep_kind(), "sweep"),
    ] {
        let mut paints = Vec::new();
        let brush = Brush::Gradient(gradient(kind, 0.5));

        let encoding = encode_brush(&brush, Affine::IDENTITY, &mut paints);

        let index = match &encoding.paint {
            Paint::Indexed(indexed) => indexed.index(),
            Paint::Solid(_) => panic!("a valid gradient should encode to an indexed paint"),
        };
        assert_eq!(encoding.lut_request.map(|r| r.paint_index), Some(index));
        assert_eq!(paints.len(), 1);

        match &paints[index] {
            EncodedPaint::Gradient(encoded) => {
                assert_eq!(kind_label(&encoded.kind), expected);
            }
            other => panic!("expected a gradient paint, got {other:?}"),
        }
    }
}

#[test]
fn image_brush_falls_back_to_a_transparent_solid() {
    let mut paints = Vec::new();
    let image = peniko::ImageBrush::new(peniko::ImageData {
        data: peniko::Blob::new(std::sync::Arc::new(vec![255_u8, 0, 0, 255])),
        format: peniko::ImageFormat::Rgba8,
        alpha_type: peniko::ImageAlphaType::Alpha,
        width: 1,
        height: 1,
    });

    let encoding = encode_brush(&Brush::Image(image), Affine::IDENTITY, &mut paints);

    assert!(paints.is_empty(), "an image brush encodes no entry yet");
    assert_eq!(encoding.lut_request, None);
    assert_eq!(
        premul(&encoding.paint),
        PremulRgba8 {
            r: 0,
            g: 0,
            b: 0,
            a: 0
        }
    );
}

#[test]
fn degenerate_gradient_falls_back_to_a_solid_with_no_lut_request() {
    let mut paints = Vec::new();
    // A zero-length gradient line has no direction to interpolate along.
    let kind = LinearGradientPosition {
        start: Point::new(10.0, 10.0),
        end: Point::new(10.0, 10.0),
    }
    .into();

    let encoding = encode_brush(
        &Brush::Gradient(gradient(kind, 0.5)),
        Affine::IDENTITY,
        &mut paints,
    );

    assert!(matches!(encoding.paint, Paint::Solid(_)));
    assert!(paints.is_empty());
    assert_eq!(encoding.lut_request, None);
}

#[test]
fn a_repeated_gradient_bakes_exactly_one_lut() {
    let mut cache = cache(8);
    let brush = linear_brush(0.5);

    let paints = encode_resident(&[brush.clone(), brush.clone(), brush], &mut cache);

    assert_eq!(
        paints.len(),
        3,
        "each draw still gets its own encoded entry"
    );
    assert_eq!(cache.entry_count(), 1, "but they share one baked ramp");

    let ramps: Vec<_> = paints
        .iter()
        .map(|paint| match paint {
            EncodedPaint::Gradient(g) => cache.lookup(g).expect("ramp should be resident"),
            other => panic!("expected a gradient paint, got {other:?}"),
        })
        .collect();
    assert_eq!(ramps[0], ramps[1]);
    assert_eq!(ramps[1], ramps[2]);
    assert_eq!(cache.luts_size(), ramps[0].width as usize * 4);
}

#[test]
fn gradients_differing_only_in_geometry_share_one_ramp() {
    let mut cache = cache(8);
    let far = Brush::Gradient(gradient(
        LinearGradientPosition {
            start: Point::new(200.0, 200.0),
            end: Point::new(400.0, 260.0),
        }
        .into(),
        0.5,
    ));

    // Same stops, different placement, and a paint transform on top of that.
    let mut paints = Vec::new();
    let near = encode_brush(&linear_brush(0.5), Affine::IDENTITY, &mut paints);
    let far = encode_brush(&far, Affine::scale(2.0), &mut paints);
    for encoding in [&near, &far] {
        let request = encoding
            .lut_request
            .expect("gradient should request a ramp");
        resolve_lut_request(request, &paints, &mut cache);
    }

    assert_eq!(paints.len(), 2, "geometry is per encoded paint");
    assert_eq!(cache.entry_count(), 1, "colour ramp is shared");
}

#[test]
fn capacity_eviction_is_lru() {
    let mut cache = cache(3);
    let brushes: Vec<Brush> = (1..=4).map(|i| linear_brush(i as f32 / 10.0)).collect();

    let mut paints = Vec::new();
    let resolve = |brush: &Brush, paints: &mut Vec<EncodedPaint>, cache: &mut GradientCache| {
        let encoding = encode_brush(brush, Affine::IDENTITY, paints);
        let request = encoding
            .lut_request
            .expect("gradient should request a ramp");
        resolve_lut_request(request, paints, cache);
        request.paint_index
    };

    let first = resolve(&brushes[0], &mut paints, &mut cache);
    let second = resolve(&brushes[1], &mut paints, &mut cache);
    let third = resolve(&brushes[2], &mut paints, &mut cache);
    assert_eq!(cache.entry_count(), 3);

    // Touch the first and third, leaving the second least recently used.
    resolve(&brushes[0], &mut paints, &mut cache);
    resolve(&brushes[2], &mut paints, &mut cache);
    let fourth = resolve(&brushes[3], &mut paints, &mut cache);

    assert_eq!(cache.entry_count(), 4, "over capacity until maintained");
    cache.maintain();
    assert_eq!(cache.entry_count(), 3);

    let resident = |index: usize, cache: &GradientCache| match &paints[index] {
        EncodedPaint::Gradient(g) => cache.lookup(g).is_some(),
        other => panic!("expected a gradient paint, got {other:?}"),
    };
    assert!(resident(first, &cache), "recently used, should survive");
    assert!(!resident(second, &cache), "least recently used, should go");
    assert!(resident(third, &cache), "recently used, should survive");
    assert!(resident(fourth, &cache), "newest, should survive");

    // Eviction compacts the packed buffer: survivors stay contiguous from zero and
    // the buffer holds exactly their bytes.
    let mut ramps: Vec<_> = [first, third, fourth]
        .into_iter()
        .map(|index| match &paints[index] {
            EncodedPaint::Gradient(g) => cache.lookup(g).expect("resident"),
            other => panic!("expected a gradient paint, got {other:?}"),
        })
        .collect();
    ramps.sort_by_key(|ramp| ramp.lut_start);

    assert_eq!(ramps[0].lut_start, 0);
    for pair in ramps.windows(2) {
        assert_eq!(pair[1].lut_start, pair[0].lut_start + pair[0].width);
    }
    let total: u32 = ramps.iter().map(|ramp| ramp.width).sum();
    assert_eq!(cache.luts_size(), total as usize * 4);
}

#[test]
fn maintain_below_capacity_evicts_nothing() {
    let mut cache = cache(5);
    let brushes: Vec<Brush> = (1..=5).map(|i| linear_brush(i as f32 / 10.0)).collect();

    encode_resident(&brushes, &mut cache);
    let packed = cache.luts_size();
    cache.maintain();

    assert_eq!(cache.entry_count(), 5);
    assert_eq!(cache.luts_size(), packed);
}

#[test]
fn upload_pads_to_the_texture_footprint_and_restores_the_buffer() {
    let mut cache = cache(8);
    encode_resident(&[linear_brush(0.5)], &mut cache);

    let packed = cache.luts_size();
    assert!(cache.has_changed());

    let layout = GradientTextureLayout::square(2048);
    {
        let upload = cache.begin_upload(layout).expect("ramps are packed");
        assert_eq!(upload.len(), layout.byte_capacity());
        assert_eq!(upload.logical_len(), packed);
        assert_eq!(upload.bytes_per_row(), 2048 * 4);
        assert!(
            upload[packed..].iter().all(|byte| *byte == 0),
            "padding must be zeroed"
        );
    }

    assert_eq!(cache.luts_size(), packed, "buffer restored on drop");
    cache.mark_synced();
    assert!(!cache.has_changed());
}

#[test]
fn an_empty_cache_has_nothing_to_upload() {
    let mut cache = cache(8);

    assert!(cache.is_empty());
    assert!(
        cache
            .begin_upload(GradientTextureLayout::square(256))
            .is_none()
    );
}

#[test]
fn texture_capacity_bounds_the_packed_buffer() {
    // Worst case is every ramp baked at the maximum LUT size, so the packed bytes
    // can never exceed the texture the cache was sized for.
    let layout = GradientTextureLayout::square(2048);
    let cache = GradientCache::for_texture(layout, Level::new());

    assert_eq!(cache.capacity(), 2048 * 2048 / 4096);
    assert_eq!(
        cache.capacity() as usize * 4096 * 4,
        layout.byte_capacity(),
        "worst-case residency exactly fills the texture"
    );
}

// ---------------------------------------------------------------------
// p4-05b: lowering an image and a blurred rounded rect into the records
// the strip shader reads, at the host level (no GPU).
// ---------------------------------------------------------------------

/// An opaque `width` x `height` RGBA8 image, straight alpha.
fn image_data(width: u32, height: u32) -> ImageData {
    let len = (width as usize) * (height as usize) * 4;
    ImageData {
        data: Blob::new(Arc::new(vec![255_u8; len])),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    }
}

/// An atlas small enough that a test allocation is easy to reason about.
fn test_budget() -> AtlasBudget {
    AtlasBudget {
        atlas_size: (64, 64),
        max_atlases: 1,
    }
}

#[test]
fn an_image_paint_lowers_to_a_record_naming_its_atlas_rectangle() {
    let mut images = ImageResidency::new(test_budget());
    images.begin_frame();
    let mut paints = Vec::new();

    let data = image_data(16, 8);
    let dest = Rect::new(0.0, 0.0, 32.0, 16.0);
    let encoding = encode_image_command(&data, dest, Affine::IDENTITY, &mut paints, &mut images)
        .expect("a 16x8 image fits a 64x64 atlas");

    let EncodedPaint::Image(image) = &paints[encoding.paint_index] else {
        panic!("expected an image paint");
    };
    let record = lower_encoded_image(image, &encoding.resident)
        .expect("the residency `encode_image_command` returned names this same entry");

    assert_eq!(record.paint_type(), PaintType::Image);
    assert_eq!(record.byte_len(), 48, "a `GpuEncodedImage` is 48 bytes");
    assert_eq!(record.texel_len(), 3);

    match record {
        GpuEncodedPaint::Image(GpuEncodedImage {
            image_params,
            image_size,
            image_offset,
            image_padding,
            ..
        }) => {
            let region = encoding.resident.region;
            assert_eq!(image_size >> 16, region.size[0], "width in the high half");
            assert_eq!(image_size & 0xFFFF, region.size[1]);
            assert_eq!(image_offset >> 16, region.offset[0], "x in the high half");
            assert_eq!(image_offset & 0xFFFF, region.offset[1]);
            assert_eq!(
                (image_params >> 6) & 0xFF,
                region.layer,
                "the atlas layer, packed at bits 6-13"
            );
            assert_eq!(
                (image_params >> 14) & 1,
                0,
                "an atlas-resident image is not an external source"
            );
            assert_eq!(image_padding, encoding.resident.padding);
        }
        other => panic!("expected an image record, got {other:?}"),
    }
}

#[test]
fn an_image_paint_with_no_matching_residency_does_not_lower() {
    // Both images share one residency, so their `ImageId`s are guaranteed
    // distinct (sequential slot indices) rather than merely likely to be —
    // two residencies each starting fresh would both mint id zero for their
    // own first image, which would pass this guard by coincidence rather
    // than exercise it.
    let mut images = ImageResidency::new(test_budget());
    images.begin_frame();
    let mut paints = Vec::new();

    let first = encode_image_command(
        &image_data(16, 8),
        Rect::new(0.0, 0.0, 32.0, 16.0),
        Affine::IDENTITY,
        &mut paints,
        &mut images,
    )
    .expect("fits the atlas");
    let second = encode_image_command(
        &image_data(4, 4),
        Rect::new(0.0, 0.0, 4.0, 4.0),
        Affine::IDENTITY,
        &mut paints,
        &mut images,
    )
    .expect("fits the atlas");
    assert_ne!(first.resident.id, second.resident.id);

    // The first entry's own paint, lowered against the *second* image's
    // residency — a mismatch [`lower_encoded_image`] guards against the
    // same way a gradient lowered against the wrong ramp would be.
    let EncodedPaint::Image(image) = &paints[first.paint_index] else {
        panic!("expected an image paint");
    };
    assert!(
        lower_encoded_image(image, &second.resident).is_none(),
        "a record naming a different image's rectangle would sample the wrong texels"
    );
}

#[test]
fn a_blurred_rounded_rect_paint_always_lowers_since_its_record_needs_no_external_residency() {
    let mut paints = Vec::new();
    let rect = Rect::new(0.0, 0.0, 40.0, 30.0);

    let paint = encode_blurred_rounded_rect(
        rect,
        CornerRadii::uniform(6.0),
        3.0,
        RED,
        Affine::IDENTITY,
        &mut paints,
    );
    let index = match paint {
        Paint::Indexed(indexed) => indexed.index(),
        Paint::Solid(_) => panic!("a blurred rounded rectangle always encodes indexed"),
    };

    // Unlike a gradient, the record needs no ramp — `None` still lowers.
    let record = lower_encoded_paint(&paints[index], None)
        .expect("a blurred rounded rectangle carries everything its record needs inline");

    assert_eq!(record.paint_type(), PaintType::BlurredRoundedRect);
    assert_eq!(
        record.byte_len(),
        80,
        "a `GpuBlurredRoundedRect` is 80 bytes"
    );
    assert_eq!(record.texel_len(), 5);

    match record {
        GpuEncodedPaint::BlurredRoundedRect(GpuBlurredRoundedRect {
            color,
            invert,
            size,
            ..
        }) => {
            assert_eq!(
                color,
                PremulColor::from_alpha_color(RED)
                    .as_premul_rgba8()
                    .to_u32()
            );
            assert_eq!(invert, 0, "the display list carries no inset-shadow flag");
            assert_eq!(size, [40.0, 30.0]);
        }
        other => panic!("expected a blurred-rounded-rect record, got {other:?}"),
    }
}

#[test]
fn an_image_and_a_blurred_rect_record_serialize_back_to_back_at_the_offsets_a_draw_names_them_by() {
    let mut images = ImageResidency::new(test_budget());
    images.begin_frame();
    let mut paints = Vec::new();
    let data = image_data(16, 8);
    let image_encoding = encode_image_command(
        &data,
        Rect::new(0.0, 0.0, 32.0, 16.0),
        Affine::IDENTITY,
        &mut paints,
        &mut images,
    )
    .expect("fits");
    let EncodedPaint::Image(image) = &paints[image_encoding.paint_index] else {
        panic!("expected an image paint");
    };
    let image_record =
        lower_encoded_image(image, &image_encoding.resident).expect("residency matches");

    let mut blur_paints = Vec::new();
    let blur_paint = encode_blurred_rounded_rect(
        Rect::new(0.0, 0.0, 20.0, 20.0),
        CornerRadii::uniform(4.0),
        1.0,
        RED,
        Affine::IDENTITY,
        &mut blur_paints,
    );
    let blur_index = match blur_paint {
        Paint::Indexed(indexed) => indexed.index(),
        Paint::Solid(_) => panic!("always indexed"),
    };
    let blur_record = lower_encoded_paint(&blur_paints[blur_index], None).expect("always lowers");

    let records = [image_record, blur_record];
    assert_eq!(
        GpuEncodedPaint::texel_offsets(&records),
        vec![0, 3],
        "the image's 48 bytes occupy three texels before the blurred rect starts"
    );
    assert_eq!(GpuEncodedPaint::serialized_len(&records), 48 + 80);

    // The paint descriptor a strip instance would carry for each, matching
    // the texel each record actually starts at.
    let image_descriptor = pack_paint_descriptor(PaintType::Image, 0);
    let blur_descriptor = pack_paint_descriptor(PaintType::BlurredRoundedRect, 3);
    assert_ne!(image_descriptor, blur_descriptor);
    assert_eq!(image_descriptor & PAINT_TEXTURE_INDEX_MASK, 0);
    assert_eq!(blur_descriptor & PAINT_TEXTURE_INDEX_MASK, 3);
}
