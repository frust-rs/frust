//! Image lowering: what an image draw costs, what it uploads, and what it does
//! when the atlas cannot hold it.
//!
//! Every case drives the public seam — a `frust_scene::Scene` recorded through
//! `SceneBuilder`, compiled by [`SceneCompiler`] — and reads the result back
//! through the encoded-paint table and the frame's own upload/eviction plan, so
//! the assertions pin the residency decisions a GPU pass would act on rather
//! than the cache's internals.
//!
//! No GPU, device or surface is involved. The record the fragment shader reads
//! is checked by decoding it exactly as the WGSL accessors in
//! `shaders/helpers.wgsl` do, which pins the bit layout without a device; the
//! *pixel* comparison against the CPU reference renderer is a GPU-bound
//! concern and lives with the phase's golden corpus, not here — including the
//! unit-image case against `vello_cpu` and the after-eviction "no stale pixels"
//! render, both of which need the `frust-testing` corpus this crate does not
//! depend on. What is host-testable about eviction — that the freed rectangle
//! is reported, ordered ahead of the frame's uploads, and describes exactly the
//! texels a clear has to write — is pinned below.

use std::sync::Arc;

use frust_engine::cache::images::{
    ATLAS_PADDING, AtlasBudget, ImageResidency, ImageSkip, MAX_UNSEEN_FRAMES,
};
use frust_engine::compile::CompiledFrame;
use frust_engine::gpu::atlas::{MAX_ATLAS_INDEX, atlas_texture_descriptor, lower_encoded_image};
use frust_engine::{GpuEncodedPaint, SceneCompiler};
use frust_gpu::{DownlevelProfile, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::color::palette::css::RED;
use peniko::{Blob, Brush, ImageAlphaType, ImageBrush, ImageData, ImageFormat};
use vello_common::encode::EncodedPaint;
use vello_common::paint::Paint;

/// Viewport every case compiles against. Deliberately not square, so an axis
/// swapped somewhere in the lowering cannot pass by symmetry.
const VIEWPORT: (u16, u16) = (64, 48);

/// The atlas every case allocates in: small enough that exhaustion and layer
/// growth are reachable in a test, large enough to hold several images.
const TEST_BUDGET: AtlasBudget = AtlasBudget {
    atlas_size: (64, 64),
    max_atlases: 2,
};

/// A destination rectangle inside the viewport, on whole pixels.
const DEST: Rect = Rect::new(8.0, 8.0, 40.0, 32.0);

/// An opaque `width` x `height` RGBA8 image whose pixels are `fill` repeated,
/// so two images of the same extent can still be distinguished by blob
/// identity.
fn image_of(width: u32, height: u32, fill: [u8; 4]) -> ImageData {
    let mut data = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for _ in 0..(width as usize) * (height as usize) {
        data.extend_from_slice(&fill);
    }

    ImageData {
        data: Blob::new(Arc::new(data)),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    }
}

/// An opaque red `width` x `height` image.
fn image(width: u32, height: u32) -> ImageData {
    image_of(width, height, [255, 0, 0, 255])
}

/// A scene built by `record`, ready to compile.
fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    record(&mut builder);
    scene
}

fn compiler() -> SceneCompiler {
    SceneCompiler::with_atlas_budget(VIEWPORT.0, VIEWPORT.1, TEST_BUDGET)
}

fn compile(compiler: &mut SceneCompiler, scene: &Scene) -> CompiledFrame {
    compiler
        .compile(scene, Affine::IDENTITY, VIEWPORT)
        .expect("an image scene compiles")
}

/// The frame's single encoded image entry.
fn only_image_entry(frame: &CompiledFrame) -> &vello_common::encode::EncodedImage {
    assert_eq!(frame.encoded_paints.len(), 1, "exactly one encoded paint");
    match &frame.encoded_paints[0] {
        EncodedPaint::Image(image) => image,
        other => panic!("expected an image paint, got {other:?}"),
    }
}

/// The 16-byte texels a `GpuEncodedImage` record serializes to, as the shader
/// loads them.
fn record_texels(paint: &GpuEncodedPaint) -> Vec<[u32; 4]> {
    let bytes = paint.as_bytes();
    bytes
        .chunks_exact(16)
        .map(|texel| {
            let mut words = [0_u32; 4];
            for (word, chunk) in words.iter_mut().zip(texel.chunks_exact(4)) {
                *word = u32::from_le_bytes(chunk.try_into().expect("four bytes"));
            }
            words
        })
        .collect()
}

#[test]
fn an_image_command_records_one_draw_painted_by_an_indexed_image_paint() {
    let mut compiler = compiler();
    let scene = scene_of(|b| b.draw_image(&image(16, 16), DEST));

    let frame = compile(&mut compiler, &scene);

    assert_eq!(frame.draws().len(), 1);
    assert_eq!(frame.image_draws, 1);
    assert_eq!(frame.skipped_images, 0);
    assert!(matches!(frame.draws()[0].paint, Paint::Indexed(_)));
    assert!(
        matches!(frame.encoded_paints[0], EncodedPaint::Image(_)),
        "an image draw indexes an image entry"
    );
    // A whole-pixel destination under the identity takes the same fast
    // rectangle path a solid fill of the same rectangle would.
    assert_eq!(frame.fast_rect_draws, 1);
}

#[test]
fn sixty_frames_of_one_image_upload_it_exactly_once() {
    let mut compiler = compiler();
    let data = image(16, 16);
    let mut uploads = 0;

    for _ in 0..60 {
        let scene = scene_of(|b| b.draw_image(&data, DEST));
        let frame = compile(&mut compiler, &scene);

        assert_eq!(frame.image_draws, 1, "every frame still draws it");
        uploads += frame.image_uploads.len();
    }

    assert_eq!(uploads, 1, "residency survives every frame that draws it");
    assert_eq!(compiler.images().entry_count(), 1);
}

#[test]
fn the_same_image_drawn_twice_in_one_frame_uploads_once_and_encodes_twice() {
    let mut compiler = compiler();
    let data = image(16, 16);
    let scene = scene_of(|b| {
        b.draw_image(&data, DEST);
        b.draw_image(&data, Rect::new(0.0, 0.0, 8.0, 8.0));
    });

    let frame = compile(&mut compiler, &scene);

    assert_eq!(frame.image_draws, 2);
    assert_eq!(
        frame.encoded_paints.len(),
        2,
        "each draw carries its own transform, so each gets its own entry"
    );
    assert_eq!(
        frame.image_uploads.len(),
        1,
        "but both entries name one resident rectangle"
    );
    assert_eq!(compiler.images().entry_count(), 1);
}

#[test]
fn two_distinct_images_take_distinct_rectangles_and_upload_separately() {
    let mut compiler = compiler();
    let scene = scene_of(|b| {
        b.draw_image(&image_of(16, 16, [255, 0, 0, 255]), DEST);
        b.draw_image(
            &image_of(16, 16, [0, 0, 255, 255]),
            Rect::new(0.0, 0.0, 8.0, 8.0),
        );
    });

    let frame = compile(&mut compiler, &scene);

    assert_eq!(frame.image_uploads.len(), 2);
    assert_ne!(
        frame.image_uploads[0].region.offset,
        frame.image_uploads[1].region.offset
    );
    assert_eq!(compiler.images().entry_count(), 2);
}

#[test]
fn an_image_larger_than_u16_is_skipped_rather_than_panicking() {
    // The `from_peniko_image_data` trap: it asserts above `u16::MAX`, so the
    // extent is refused before the conversion is ever reached. The pixel buffer
    // is not allocated at that size — the refusal happens on the declared
    // extent, which is exactly what makes it cheap.
    let mut oversized = image(1, 1);
    oversized.width = u32::from(u16::MAX) + 1;
    oversized.height = 1;

    let mut compiler = compiler();
    let scene = scene_of(|b| b.draw_image(&oversized, DEST));

    let frame = compile(&mut compiler, &scene);

    assert_eq!(frame.skipped_images, 1);
    assert_eq!(frame.image_draws, 0);
    assert!(frame.draws().is_empty(), "the refused draw records nothing");
    assert!(
        frame.encoded_paints.is_empty(),
        "and leaves no orphan paint entry"
    );
    assert!(frame.strips.strips.is_empty(), "nor orphan coverage");
    assert!(frame.image_uploads.is_empty());
}

/// A source far larger than [`TEST_BUDGET`]'s layer, at a non-square aspect
/// ratio so a fit applied per axis rather than uniformly would show.
///
/// The corpus's own `adv-huge-image` is 5000 square; this is the same shape at
/// a size a host test can build and box-filter in the time a host test should
/// take, and the golden gate is where the literal case is measured.
const OVERSIZED: (u32, u32) = (512, 256);

#[test]
fn an_image_larger_than_the_atlas_is_minified_to_fit_rather_than_skipped() {
    // The `adv-huge-image` shape: well inside `u16::MAX`, so nothing upstream
    // objects, but far past any atlas layer this budget will create. It is
    // downsampled to fit and drawn, not dropped.
    let mut compiler = compiler();
    let scene = scene_of(|b| b.draw_image(&image(OVERSIZED.0, OVERSIZED.1), DEST));

    let frame = compile(&mut compiler, &scene);

    assert_eq!(frame.skipped_images, 0, "a big image is not a skipped one");
    assert_eq!(frame.image_draws, 1);
    assert_eq!(frame.draws().len(), 1);
    assert_eq!(compiler.images().entry_count(), 1);
    assert_eq!(compiler.images().minified(), 1);

    // Fitted to the layer, aspect preserved, with the source extent recorded
    // beside it — the upload carries exactly the region's texels, not the
    // source's.
    let upload = &frame.image_uploads[0];
    assert_eq!(upload.region.size, [64, 32]);
    assert_eq!(upload.natural, [OVERSIZED.0, OVERSIZED.1]);
    assert_eq!(
        (upload.pixels.width(), upload.pixels.height()),
        (64, 32),
        "the atlas write covers the region it was allocated for"
    );
    assert_eq!(
        upload.pixels.data_as_u8_slice().len(),
        upload.region.byte_len()
    );
}

#[test]
fn a_minified_records_transform_lands_on_the_resident_rectangle() {
    // The correction the renderer applies: the compiler composed the paint
    // transform against the SOURCE extent (it is all it knows before residency
    // is consulted), while the shader samples the smaller resident rectangle.
    // A record left uncorrected would address 512 texels of a 64-texel
    // rectangle and read nothing but its clamped edge.
    let mut compiler = compiler();
    let dest = Rect::new(8.0, 8.0, 40.0, 40.0);
    let frame = compile(
        &mut compiler,
        &scene_of(|b| b.draw_image(&image(OVERSIZED.0, OVERSIZED.1), dest)),
    );

    let resident = frame_resident(&frame);
    assert_eq!(resident.minify_scale(), Some((0.125, 0.125)));

    // Uncorrected, the entry maps the destination's far corner onto the
    // source's own extent.
    let entry = only_image_entry(&frame);
    let corner = entry.transform * kurbo::Point::new(dest.x1, dest.y1);
    assert!((corner.x - f64::from(OVERSIZED.0)).abs() < 1e-6);
    assert!((corner.y - f64::from(OVERSIZED.1)).abs() < 1e-6);

    // Corrected, it maps onto the resident rectangle instead — which is the
    // arithmetic `renderer::fit_minified` performs on the lowered record, so
    // this states the ratio that record has to be scaled by.
    let (scale_x, scale_y) = resident.minify_scale().expect("a minified image");
    assert!((corner.x * f64::from(scale_x) - 64.0).abs() < 1e-6);
    assert!((corner.y * f64::from(scale_y) - 32.0).abs() < 1e-6);
}

#[test]
fn an_atlas_with_no_room_left_is_still_a_skip() {
    // Minification fits an image to a LAYER, not to the space left in one: a
    // budget already exhausted refuses the next allocation exactly as before.
    let full = AtlasBudget {
        atlas_size: (64, 64),
        max_atlases: 1,
    };
    let mut residency = ImageResidency::new(full);
    residency.begin_frame();
    residency
        .resolve(&image(64, 64))
        .expect("the first image fills the one layer");

    assert!(matches!(
        residency.resolve(&image_of(32, 32, [0, 0, 255, 255])),
        Err(ImageSkip::NoAtlasSpace { .. })
    ));
    assert_eq!(residency.skipped(), 1);
}

#[test]
fn an_unsupported_or_malformed_image_is_skipped_with_the_frame_still_drawing() {
    let mut malformed = image(4, 4);
    malformed.data = Blob::new(Arc::new(vec![0_u8; 8]));

    let mut compiler = compiler();
    let scene = scene_of(|b| {
        b.draw_image(&malformed, DEST);
        b.fill_rect(Rect::new(0.0, 0.0, 16.0, 16.0), Brush::Solid(RED));
    });

    let frame = compile(&mut compiler, &scene);

    assert_eq!(frame.skipped_images, 1);
    assert_eq!(
        frame.draws().len(),
        1,
        "a refused image draws less, never blanking the frame"
    );
}

#[test]
fn a_refused_image_leaves_the_frames_depths_dense() {
    let mut oversized = image(1, 1);
    oversized.width = u32::from(u16::MAX) + 1;

    let mut compiler = compiler();
    let scene = scene_of(|b| {
        b.fill_rect(Rect::new(0.0, 0.0, 16.0, 16.0), Brush::Solid(RED));
        b.draw_image(&oversized, DEST);
        b.fill_rect(Rect::new(16.0, 0.0, 32.0, 16.0), Brush::Solid(RED));
    });

    let frame = compile(&mut compiler, &scene);

    let depths: Vec<u32> = frame.draws().iter().map(|draw| draw.depth).collect();
    assert_eq!(depths, vec![0, 1], "the skipped image consumes no depth");
}

#[test]
fn an_unseen_image_is_reaped_and_its_rectangle_reported_for_clearing() {
    let mut compiler = compiler();
    let data = image(16, 16);

    let drawn = compile(&mut compiler, &scene_of(|b| b.draw_image(&data, DEST)));
    let region = drawn.image_uploads[0].region;

    // Inside the window nothing is reclaimed, however many frames draw nothing.
    let empty = scene_of(|_| {});
    for _ in 0..MAX_UNSEEN_FRAMES {
        let frame = compile(&mut compiler, &empty);
        assert!(frame.image_evictions.is_empty());
    }

    let reaped = compile(&mut compiler, &empty);
    assert_eq!(
        reaped.image_evictions,
        vec![region],
        "the padded rectangle the image held is handed back for clearing"
    );
    assert_eq!(compiler.images().entry_count(), 0);

    // Drawing it again re-allocates and re-uploads: residency is genuinely
    // gone, not merely unreferenced.
    let redrawn = compile(&mut compiler, &scene_of(|b| b.draw_image(&data, DEST)));
    assert_eq!(redrawn.image_uploads.len(), 1);
}

#[test]
fn an_evicted_regions_clear_covers_exactly_the_texels_it_freed() {
    // The host-testable half of "render after eviction shows no stale pixels":
    // the reported region's byte footprint and row stride are exactly what a
    // transparent write into it has to cover, so a clear built from it cannot
    // under-cover the rectangle the next allocation may only partly reuse.
    let mut compiler = compiler();
    let data = image(16, 8);
    let drawn = compile(&mut compiler, &scene_of(|b| b.draw_image(&data, DEST)));
    let region = drawn.image_uploads[0].region;

    let empty = scene_of(|_| {});
    let mut evicted = None;
    for _ in 0..=MAX_UNSEEN_FRAMES {
        let frame = compile(&mut compiler, &empty);
        if let Some(first) = frame.image_evictions.first() {
            evicted = Some(*first);
        }
    }
    let evicted = evicted.expect("the image is reaped inside the window plus one");

    assert_eq!(
        evicted.size,
        [
            16 + 2 * u32::from(ATLAS_PADDING),
            8 + 2 * u32::from(ATLAS_PADDING)
        ]
    );
    assert_eq!(evicted.bytes_per_row(), evicted.size[0] * 4);
    assert_eq!(
        evicted.byte_len(),
        (evicted.size[0] * evicted.size[1] * 4) as usize
    );
    assert_eq!(region.layer, evicted.layer);
}

#[test]
fn an_upload_carries_exactly_its_regions_premultiplied_texels() {
    let mut compiler = compiler();
    // Half-transparent white in straight alpha premultiplies to half grey.
    let data = image_of(4, 2, [255, 255, 255, 128]);
    let frame = compile(&mut compiler, &scene_of(|b| b.draw_image(&data, DEST)));

    let upload = &frame.image_uploads[0];
    assert_eq!(upload.region.size, [4, 2]);
    assert_eq!(upload.pixels.width(), 4);
    assert_eq!(upload.pixels.height(), 2);
    assert_eq!(
        upload.pixels.data_as_u8_slice().len(),
        upload.region.byte_len(),
        "the pixel payload is exactly the region's footprint"
    );

    let first = upload.pixels.sample(0, 0);
    assert_eq!(first.a, 128);
    assert_eq!(
        (first.r, first.g, first.b),
        (128, 128, 128),
        "straight alpha is premultiplied on the way into the atlas"
    );
    assert!(
        upload.pixels.may_have_transparency(),
        "and the transparency hint follows the actual pixels"
    );
}

#[test]
fn the_encoded_transform_maps_the_destination_back_onto_the_natural_pixels() {
    let mut compiler = compiler();
    let dest = Rect::new(10.0, 20.0, 42.0, 36.0);
    let frame = compile(
        &mut compiler,
        &scene_of(|b| b.draw_image(&image(16, 8), dest)),
    );

    // The entry stores the inverse mapping the shader applies: a device point
    // at the destination's origin lands on the image's first texel, and one at
    // its far corner on the image's own extent.
    let transform = only_image_entry(&frame).transform;
    let origin = transform * kurbo::Point::new(dest.x0, dest.y0);
    let corner = transform * kurbo::Point::new(dest.x1, dest.y1);

    assert!((origin.x).abs() < 1e-9 && (origin.y).abs() < 1e-9);
    assert!((corner.x - 16.0).abs() < 1e-9 && (corner.y - 8.0).abs() < 1e-9);
}

#[test]
fn the_lowered_record_decodes_the_way_the_wgsl_accessors_read_it() {
    let mut compiler = compiler();
    let frame = compile(
        &mut compiler,
        &scene_of(|b| b.draw_image(&image(16, 8), DEST)),
    );

    let region = frame.image_uploads[0].region;
    let entry = only_image_entry(&frame);
    let resident = frame_resident(&frame);
    let record = lower_encoded_image(entry, &resident).expect("the residency matches the entry");
    let texels = record_texels(&record);

    // `get_image_size` / `get_image_offset`: width and x in the high half.
    assert_eq!(texels[0][1] >> 16, region.size[0]);
    assert_eq!(texels[0][1] & 0xFFFF, region.size[1]);
    assert_eq!(texels[0][2] >> 16, region.offset[0]);
    assert_eq!(texels[0][2] & 0xFFFF, region.offset[1]);
    // `get_image_atlas_index` / `get_image_source_kind`.
    assert_eq!((texels[0][0] >> 6) & 0xFF, region.layer);
    assert_eq!((texels[0][0] >> 14) & 1, 0, "an atlas source, not external");
    // `get_image_padding`, and the identity tint the shader always multiplies.
    assert_eq!(texels[2][3], u32::from(ATLAS_PADDING));
    assert_eq!(texels[2][1], u32::MAX);
}

/// The residency the frame's single upload describes, rebuilt from the frame
/// itself so the record test needs no accessor into the cache.
fn frame_resident(frame: &CompiledFrame) -> frust_engine::ResidentImage {
    let upload = &frame.image_uploads[0];
    let EncodedPaint::Image(entry) = &frame.encoded_paints[0] else {
        panic!("expected an image paint");
    };
    let vello_common::paint::ImageSource::OpaqueId {
        id,
        may_have_transparency,
    } = entry.source
    else {
        panic!("an engine-encoded image names a residency handle");
    };

    frust_engine::ResidentImage {
        id,
        region: upload.region,
        natural: upload.natural,
        padding: u32::from(ATLAS_PADDING),
        may_have_transparency,
    }
}

#[test]
fn a_disabled_atlas_skips_every_image_without_disturbing_the_rest_of_the_frame() {
    let mut compiler = SceneCompiler::with_atlas_budget(VIEWPORT.0, VIEWPORT.1, TEST_BUDGET);
    compiler.set_image_residency(ImageResidency::disabled(TEST_BUDGET));

    let scene = scene_of(|b| {
        b.draw_image(&image(16, 16), DEST);
        b.fill_rect(Rect::new(0.0, 0.0, 16.0, 16.0), Brush::Solid(RED));
    });
    let frame = compile(&mut compiler, &scene);

    assert_eq!(frame.skipped_images, 1);
    assert_eq!(frame.image_draws, 0);
    assert_eq!(frame.draws().len(), 1, "the solid fill still paints");
    assert!(frame.image_uploads.is_empty());
    assert_eq!(frame.atlas_layers, 0, "no atlas layer is created at all");
}

#[test]
fn a_brush_image_is_drawn_at_its_natural_size_under_the_paint_transform() {
    let mut compiler = compiler();
    let brush = ImageBrush::new(image(16, 8));
    let rect = Rect::new(4.0, 4.0, 36.0, 28.0);
    let frame = compile(
        &mut compiler,
        &scene_of(|b| b.fill_rect(rect, Brush::Image(brush))),
    );

    assert_eq!(frame.image_draws, 1);

    // Unlike a `Command::Image`, a brush is not scaled to the shape: under the
    // identity its natural pixels land one-for-one, so the device point at the
    // rectangle's origin maps to that same point in image space.
    let transform = only_image_entry(&frame).transform;
    let origin = transform * kurbo::Point::new(rect.x0, rect.y0);
    assert!((origin.x - rect.x0).abs() < 1e-9 && (origin.y - rect.y0).abs() < 1e-9);
}

#[test]
fn the_mobile_budget_is_chosen_for_a_downlevel_adapter_and_never_the_vello_default() {
    let mobile = AtlasBudget::for_caps(&TierCaps::fake(DownlevelProfile::WebGl2));
    assert_eq!(mobile.atlas_size, AtlasBudget::MOBILE.atlas_size);
    assert_eq!(mobile.max_atlases, AtlasBudget::MOBILE.max_atlases);

    let desktop = AtlasBudget::for_caps(&TierCaps::fake(DownlevelProfile::Full));
    assert_eq!(desktop.atlas_size, AtlasBudget::DESKTOP.atlas_size);
    assert_eq!(desktop.max_atlases, AtlasBudget::DESKTOP.max_atlases);

    // Never the crate default of 4096 square over eight layers.
    for budget in [mobile, desktop] {
        assert!(budget.atlas_size.0 <= 2048 && budget.atlas_size.1 <= 2048);
        assert!(budget.max_atlases <= usize::try_from(MAX_ATLAS_INDEX + 1).expect("fits"));
    }
}

#[test]
fn the_atlas_texture_is_declared_the_way_the_shaders_binding_expects() {
    let descriptor = atlas_texture_descriptor(
        AtlasBudget::MOBILE.atlas_size.0,
        AtlasBudget::MOBILE.atlas_size.1,
        4,
    );

    assert_eq!(descriptor.format, wgpu::TextureFormat::Rgba8Unorm);
    assert_eq!(descriptor.dimension, wgpu::TextureDimension::D2);
    assert_eq!(descriptor.size.depth_or_array_layers, 4);
    assert!(
        descriptor
            .usage
            .contains(wgpu::TextureUsages::TEXTURE_BINDING)
    );
    assert!(descriptor.usage.contains(wgpu::TextureUsages::COPY_DST));
}
