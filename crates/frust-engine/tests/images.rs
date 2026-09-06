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
//!
//! A full atlas is the other half of that. Residency is bounded by population
//! as well as by age — an allocation the packer refuses displaces the
//! least-recently-seen image rather than dropping the draw — so the cases below
//! pin both ends of the trade: a working set larger than the atlas keeps every
//! draw and pays in re-uploads, while a working set larger than the atlas
//! *within one frame* is still a skip, because everything resident belongs to
//! the frame that is about to sample it.
//!
//! One thing here is *not* about lowering: the plan's own lifetime. A compiled
//! frame can still be refused before it reaches a queue, so residency is only
//! committed when a consumer acknowledges having serviced the plan, and the
//! cases below hold both halves of that — an unacknowledged plan is re-offered
//! identically rather than lost or duplicated, and every region it names lies
//! inside the atlas the same frame asks to be grown to, so a consumer recording
//! where an image lives can never come to sample a rectangle nothing wrote.

use std::sync::Arc;

use frust_engine::cache::images::{
    ATLAS_PADDING, AtlasBudget, ImageResidency, ImageSkip, MAX_UNSEEN_FRAMES,
};
#[cfg(feature = "perf-trace")]
use frust_engine::cache::images::{TierLogOnce, atlas_tier_line};
use frust_engine::compile::CompiledFrame;
#[cfg(feature = "perf-trace")]
use frust_engine::compile::{image_pressure_line, image_pressure_reported};
use frust_engine::gpu::atlas::{MAX_ATLAS_INDEX, atlas_texture_descriptor, lower_encoded_image};
use frust_engine::schedule::{PageConfig, Schedule};
use frust_engine::{EngineError, GpuEncodedPaint, SceneCompiler};
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
        .as_chunks::<16>()
        .0
        .iter()
        .map(|texel| {
            let mut words = [0_u32; 4];
            for (word, chunk) in words.iter_mut().zip(texel.as_chunks::<4>().0) {
                *word = u32::from_le_bytes(*chunk);
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
        // Every frame here stands for one that reached the atlas; the case
        // below is what happens when one does not.
        compiler.acknowledge_image_plan();
    }

    assert_eq!(uploads, 1, "residency survives every frame that draws it");
    assert_eq!(compiler.images().entry_count(), 1);
}

/// The engine's own scheduler capabilities, as the escalation case below needs
/// them.
fn caps() -> TierCaps {
    TierCaps::fake(DownlevelProfile::Full)
}

/// A half-opacity layer holding a flat isolated child and then a nesting one
/// that holds an isolated *pair*, over a scene that also draws `data`.
///
/// The smallest shape three live pages cannot serve — the outer parent takes a
/// group of its own once its round is cut after the first child, the inner
/// parent's two children take the other group and the spill page between them,
/// and the inner parent's own round, which composites both of them, would need
/// a fourth live page — so `Schedule::build` refuses it. Recorded around a real
/// image draw, so the refusal lands on a frame that made an image resident.
///
/// Neither a plain sibling fan nor a single nesting child is a counterexample
/// any more: the scheduler serves a fan of any width by cutting the parent's
/// round, and a chain hanging off a later child on its one spill page.
fn branching_layers_with_an_image(data: &ImageData) -> Scene {
    scene_of(|b| {
        b.draw_image(data, DEST);
        b.push_layer(Rect::new(0.0, 0.0, 56.0, 24.0), 0.5);
        b.push_layer(Rect::new(0.0, 0.0, 24.0, 24.0), 0.5);
        b.fill_rect(Rect::new(2.0, 2.0, 20.0, 20.0), Brush::Solid(RED));
        b.pop_layer();
        b.push_layer(Rect::new(28.0, 0.0, 52.0, 24.0), 0.5);
        b.push_layer(Rect::new(30.0, 2.0, 38.0, 22.0), 0.5);
        b.fill_rect(Rect::new(31.0, 4.0, 37.0, 20.0), Brush::Solid(RED));
        b.pop_layer();
        b.push_layer(Rect::new(40.0, 2.0, 50.0, 22.0), 0.5);
        b.fill_rect(Rect::new(41.0, 4.0, 49.0, 20.0), Brush::Solid(RED));
        b.pop_layer();
        b.pop_layer();
        b.pop_layer();
    })
}

#[test]
fn a_frame_refused_after_compiling_keeps_its_uploads_for_the_next_frame() {
    // The defect this pins: residency used to be committed at compile time, so
    // a frame refused afterwards dropped its uploads while the entry map went
    // on reporting the image resident. Every later frame then resolved a hit,
    // scheduled no upload, and the draw was lost for the life of the process.
    let mut compiler = compiler();
    let data = image(16, 16);

    let refused = compile(&mut compiler, &branching_layers_with_an_image(&data));
    assert_eq!(refused.image_uploads.len(), 1, "the image became resident");
    let region = refused.image_uploads[0].region;

    // The refusal itself: this frame never reaches an atlas, so nothing may
    // acknowledge its plan.
    assert!(
        matches!(
            Schedule::build(&refused.recorder, &caps(), &PageConfig::default()),
            Err(EngineError::SchedulerEscalation { .. })
        ),
        "the counterexample must really be a refused frame"
    );
    drop(refused);

    // The next frame draws the same image and is scheduled without complaint.
    let retry = compile(&mut compiler, &scene_of(|b| b.draw_image(&data, DEST)));
    assert!(Schedule::build(&retry.recorder, &caps(), &PageConfig::default()).is_ok());
    assert_eq!(
        retry.image_uploads.len(),
        1,
        "the unserviced upload is offered again rather than lost"
    );
    assert_eq!(
        retry.image_uploads[0].region, region,
        "and to the same place"
    );
    assert_eq!(retry.image_draws, 1, "the draw is not dropped");

    // Serviced. From here the steady state resumes: exactly one upload has been
    // handed over for this image, and no further frame offers another.
    compiler.acknowledge_image_plan();
    for _ in 0..4 {
        let frame = compile(&mut compiler, &scene_of(|b| b.draw_image(&data, DEST)));
        assert_eq!(frame.image_draws, 1);
        assert!(
            frame.image_uploads.is_empty(),
            "a serviced upload is never offered twice"
        );
        compiler.acknowledge_image_plan();
    }
}

#[test]
fn a_refused_frames_evictions_survive_it_too() {
    // The eviction half of the same invariant: a clear that never ran must not
    // be forgotten, or the reaped rectangle keeps its old tenant's texels for a
    // later, smaller allocation to sample past the edge of.
    let mut compiler = compiler();
    let data = image(16, 8);

    let drawn = compile(&mut compiler, &scene_of(|b| b.draw_image(&data, DEST)));
    let region = drawn.image_uploads[0].region;
    compiler.acknowledge_image_plan();

    let empty = scene_of(|_| {});
    let mut evicted = None;
    for _ in 0..=MAX_UNSEEN_FRAMES {
        let frame = compile(&mut compiler, &empty);
        if let Some(first) = frame.image_evictions.first() {
            evicted = Some(*first);
        }
    }
    let evicted = evicted.expect("the image is reaped inside the window plus one");
    assert_eq!(evicted.layer, region.layer);

    // Nothing acknowledged any of those frames, so the clear is still owed.
    let next = compile(&mut compiler, &empty);
    assert_eq!(
        next.image_evictions,
        vec![evicted],
        "an unserviced clear is offered again, and only once"
    );

    compiler.acknowledge_image_plan();
    let after = compile(&mut compiler, &empty);
    assert!(after.image_evictions.is_empty());
}

#[test]
fn a_capacity_refusal_leaves_no_region_recorded_for_an_image_it_refused() {
    // The second face of the same defect: a consumer records where each upload
    // lives so a later frame's paint can name it. A region outside the atlas
    // the frame asks to be grown to could never be written, so recording it
    // would point the shader at texels nothing ever set. The plan is what that
    // consumer reads, and it can only ever name regions the budget admits.
    let full = AtlasBudget {
        atlas_size: (64, 64),
        max_atlases: 1,
    };
    let mut compiler = SceneCompiler::with_atlas_budget(VIEWPORT.0, VIEWPORT.1, full);

    let fills = image(64, 64);
    let refused = image_of(32, 32, [0, 0, 255, 255]);
    let frame = compile(
        &mut compiler,
        &scene_of(|b| {
            b.draw_image(&fills, DEST);
            b.draw_image(&refused, Rect::new(0.0, 0.0, 8.0, 8.0));
        }),
    );

    assert_eq!(frame.skipped_images, 1, "the second image finds no room");
    assert_eq!(frame.image_draws, 1);
    assert_eq!(
        frame.encoded_paints.len(),
        1,
        "a refused image leaves no paint entry to resolve"
    );
    assert_eq!(
        frame.image_uploads.len(),
        1,
        "and no upload to record a region from"
    );

    for upload in &frame.image_uploads {
        assert!(
            full.contains(upload.region, frame.atlas_layers),
            "{:?} lies outside the {}-layer atlas this frame asks for",
            upload.region,
            frame.atlas_layers
        );
    }
}

#[test]
fn every_uploads_region_fits_the_atlas_the_same_frame_asks_to_be_grown_to() {
    // Stated over a frame that genuinely spans layers, since `atlas_layers` is
    // the depth the array is grown to before these regions are written: an
    // upload naming a layer past it would be refused by the array and its image
    // sampled from whatever the texture held.
    let mut compiler = compiler();
    let frame = compile(
        &mut compiler,
        &scene_of(|b| {
            b.draw_image(&image_of(64, 48, [255, 0, 0, 255]), DEST);
            b.draw_image(
                &image_of(64, 48, [0, 0, 255, 255]),
                Rect::new(0.0, 0.0, 16.0, 16.0),
            );
        }),
    );

    assert_eq!(frame.image_uploads.len(), 2);
    assert_eq!(frame.atlas_layers, 2, "the two images need a layer each");
    for upload in &frame.image_uploads {
        assert!(TEST_BUDGET.contains(upload.region, frame.atlas_layers));
    }
    for region in &frame.image_evictions {
        assert!(TEST_BUDGET.contains(*region, frame.atlas_layers));
    }
}

#[test]
fn an_upload_names_the_handle_its_own_paint_carries() {
    // What lets a consumer register an upload without pairing it positionally
    // against the frame's draws — a pairing a re-offered plan breaks, since an
    // unserviced upload is reported again on a frame with no draw of its own to
    // pair it with.
    let mut compiler = compiler();
    let frame = compile(
        &mut compiler,
        &scene_of(|b| b.draw_image(&image(16, 8), DEST)),
    );

    let vello_common::paint::ImageSource::OpaqueId { id, .. } = only_image_entry(&frame).source
    else {
        panic!("an engine-encoded image names a residency handle");
    };
    assert_eq!(frame.image_uploads[0].id, id);
    assert!(!frame.image_uploads[0].may_have_transparency);
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

/// An atlas of exactly four 32-square rectangles: one layer, two by two, so
/// "the atlas is full" needs no reasoning about how a packer splits free space.
const FOUR_SLOT_BUDGET: AtlasBudget = AtlasBudget {
    atlas_size: (64, 64),
    max_atlases: 1,
};

#[test]
fn a_working_set_larger_than_the_atlas_re_uploads_rather_than_dropping_draws() {
    // The scrolling-list shape, scaled to a host test: a hundred distinct
    // images cycling past a window four cells wide, against an atlas that holds
    // thirty-two of them. Before residency was bounded by population, every
    // draw past the last free rectangle painted nothing — a hole on a screen
    // whose solid fills all still landed. It must now cost a re-upload instead.
    let mut compiler = compiler();
    let images: Vec<ImageData> = (0..100).map(|_| image(16, 16)).collect();

    let mut skipped = 0_u32;
    let mut drawn = 0_u32;
    for frame_index in 0..100_usize {
        let window: Vec<&ImageData> = (0..4)
            .map(|cell| &images[(frame_index * 4 + cell) % images.len()])
            .collect();
        let frame = compile(
            &mut compiler,
            &scene_of(|b| {
                for data in window {
                    b.draw_image(data, DEST);
                }
            }),
        );

        skipped += frame.skipped_images;
        drawn += frame.image_draws;
        compiler.acknowledge_image_plan();
    }

    assert_eq!(drawn, 400, "every cell of every frame painted");
    assert_eq!(skipped, 0, "and none of them was dropped for want of room");
    assert_eq!(compiler.images().skipped(), 0);
    assert!(
        compiler.images().pressure_evictions() > 0,
        "fixture precondition: the working set really outgrew the atlas"
    );
    assert!(
        compiler.images().entry_count() <= 32,
        "residency stays inside the budget it was given: {} entries",
        compiler.images().entry_count()
    );
}

#[test]
fn an_atlas_filled_inside_one_frame_still_skips_and_schedules_no_clear() {
    // The boundary the eviction must not cross. The frame path clears the
    // evicted rectangles before it writes the new ones, so freeing a rectangle
    // this same frame resolved would zero texels the frame is about to sample —
    // painting exactly the hole the eviction exists to prevent. Everything
    // resident here belongs to this frame, so the overflow is honestly a skip.
    let mut compiler = SceneCompiler::with_atlas_budget(VIEWPORT.0, VIEWPORT.1, FOUR_SLOT_BUDGET);
    let images: Vec<ImageData> = (0..8).map(|_| image(32, 32)).collect();

    let frame = compile(
        &mut compiler,
        &scene_of(|b| {
            for data in &images {
                b.draw_image(data, DEST);
            }
        }),
    );

    assert!(
        frame.skipped_images > 0,
        "fixture precondition: eight 32-square images cannot share four slots"
    );
    assert_eq!(
        frame.image_draws + frame.skipped_images,
        8,
        "every draw is accounted for as either painted or skipped"
    );
    assert_eq!(
        compiler.images().pressure_evictions(),
        0,
        "nothing this frame drew was evicted for something else this frame drew"
    );
    assert!(
        frame.image_evictions.is_empty(),
        "and no clear was scheduled over a rectangle the frame is sampling"
    );
}

#[test]
fn a_request_no_eviction_could_satisfy_costs_no_eviction_at_all() {
    // The storm. One image the atlas can never hold, drawn every frame in the
    // middle of a working set that *does* fit: `[A, Big, B, C, D]`, where the
    // four 32-squares take the four slots and the 48-square fits in none of the
    // gaps between them. Freeing B, C and D would not help — a 48-square needs
    // a contiguous rectangle no combination of those holes makes while A is
    // resident — so an eviction loop that only stops when the candidate list
    // runs out clears and re-uploads three images per frame, forever, and skips
    // `Big` anyway. The scene must instead settle: one skip, and nothing moves.
    let mut compiler = SceneCompiler::with_atlas_budget(VIEWPORT.0, VIEWPORT.1, FOUR_SLOT_BUDGET);
    let a = image(32, 32);
    let big = image(48, 48);
    let rest: Vec<ImageData> = (0..3).map(|_| image(32, 32)).collect();

    for index in 0..8_u32 {
        let frame = compile(
            &mut compiler,
            &scene_of(|b| {
                b.draw_image(&a, DEST);
                b.draw_image(&big, DEST);
                for data in &rest {
                    b.draw_image(data, DEST);
                }
            }),
        );

        assert_eq!(
            frame.skipped_images, 1,
            "frame {index}: only the 48-square is refused"
        );
        assert_eq!(frame.image_draws, 4, "frame {index}: the rest all painted");

        if index == 0 {
            assert_eq!(
                frame.image_uploads.len(),
                4,
                "the first frame makes the four that fit resident"
            );
        } else {
            assert!(
                frame.image_uploads.is_empty(),
                "frame {index}: a settled scene re-uploads nothing — {} uploads",
                frame.image_uploads.len()
            );
            assert!(
                frame.image_evictions.is_empty(),
                "frame {index}: and clears nothing — {} clears",
                frame.image_evictions.len()
            );
        }
        compiler.acknowledge_image_plan();
    }

    assert_eq!(
        compiler.images().pressure_evictions(),
        0,
        "nothing was ever displaced for a request that could not be placed"
    );
    assert_eq!(
        compiler.images().entry_count(),
        4,
        "and the four that fit are all still resident"
    );
}

#[test]
fn a_request_larger_than_everything_evictable_leaves_the_residency_alone() {
    // The same invariant reached by area rather than by shape: a whole-layer
    // image asked for while one of the four slots belongs to this frame. Even
    // freeing every candidate leaves a quarter of the layer occupied, so the
    // request is hopeless before the first rectangle is handed back — and a
    // skip that costs nothing is exactly what it cost before eviction existed.
    let mut compiler = SceneCompiler::with_atlas_budget(VIEWPORT.0, VIEWPORT.1, FOUR_SLOT_BUDGET);
    let resident: Vec<ImageData> = (0..4).map(|_| image(32, 32)).collect();
    compile(
        &mut compiler,
        &scene_of(|b| {
            for data in &resident {
                b.draw_image(data, DEST);
            }
        }),
    );
    compiler.acknowledge_image_plan();

    let whole_layer = image(64, 64);
    let frame = compile(
        &mut compiler,
        &scene_of(|b| {
            // One of the four redrawn first, so it is this frame's and cannot
            // be evicted whatever the rest of the atlas holds.
            b.draw_image(&resident[0], DEST);
            b.draw_image(&whole_layer, DEST);
        }),
    );

    assert_eq!(frame.skipped_images, 1, "the whole-layer image is refused");
    assert!(
        frame.image_evictions.is_empty(),
        "and nothing was scheduled for clearing to try"
    );
    assert!(frame.image_uploads.is_empty());
    assert_eq!(
        compiler.images().pressure_evictions(),
        0,
        "a hopeless request evicts nothing"
    );
    assert_eq!(
        compiler.images().entry_count(),
        4,
        "the residency is exactly as it was found"
    );
}

#[test]
fn one_blob_drawn_at_two_extents_in_a_frame_keeps_the_first_draws_texels() {
    // The extent-mismatch release, held to the same rule as the pressure path.
    // A caller that rebuilds an `ImageData` around the same buffer at a new
    // extent gives the old rectangle back — but the plan clears before it
    // uploads, so doing that on a frame whose first draw already resolved that
    // rectangle would zero the texels that draw is about to sample. The second
    // extent is skipped for this frame instead.
    let square = image(16, 8);
    let transposed = ImageData {
        data: square.data.clone(),
        format: square.format,
        alpha_type: square.alpha_type,
        width: 8,
        height: 16,
    };

    let mut compiler = compiler();
    let frame = compile(
        &mut compiler,
        &scene_of(|b| {
            b.draw_image(&square, DEST);
            b.draw_image(&transposed, DEST);
        }),
    );

    assert_eq!(frame.image_draws, 1, "the first extent painted");
    assert_eq!(
        frame.skipped_images, 1,
        "the second is skipped, not swapped"
    );
    assert_eq!(
        frame.image_uploads.len(),
        1,
        "one upload, for the extent that resolved"
    );
    assert!(
        frame.image_evictions.is_empty(),
        "and no clear over the rectangle the first draw is sampling"
    );
    compiler.acknowledge_image_plan();

    // On a later frame the release is ordinary again: nothing has sampled the
    // rectangle yet, so the new extent takes it and the old one is cleared.
    let next = compile(
        &mut compiler,
        &scene_of(|b| b.draw_image(&transposed, DEST)),
    );
    assert_eq!(next.skipped_images, 0, "the new extent resolves");
    assert_eq!(next.image_uploads.len(), 1);
    assert_eq!(
        next.image_evictions.len(),
        1,
        "and the rectangle the old extent held is reported for clearing"
    );
}

#[test]
fn a_pressure_eviction_reports_its_rectangle_once_and_hands_it_to_the_new_upload() {
    // The consumer contract eviction has always kept, now reached by the second
    // route: `image_evictions` names each freed rectangle once, and the plan is
    // serviced clears-first, so the rectangle can be re-used by an upload in
    // the very same plan without the clear erasing it.
    let mut compiler = SceneCompiler::with_atlas_budget(VIEWPORT.0, VIEWPORT.1, FOUR_SLOT_BUDGET);
    let resident: Vec<ImageData> = (0..4).map(|_| image(32, 32)).collect();

    let filled = compile(
        &mut compiler,
        &scene_of(|b| {
            for data in &resident {
                b.draw_image(data, DEST);
            }
        }),
    );
    assert_eq!(filled.skipped_images, 0, "the four slots take four images");
    assert_eq!(filled.image_uploads.len(), 4);
    assert!(filled.image_evictions.is_empty());
    // The first image allocated is the one displaced below: every entry was
    // last seen on the same frame, and the slot index breaks that tie.
    let displaced = filled.image_uploads[0].region;
    compiler.acknowledge_image_plan();

    let arrival = image(32, 32);
    let frame = compile(&mut compiler, &scene_of(|b| b.draw_image(&arrival, DEST)));

    assert_eq!(frame.skipped_images, 0, "the fifth image still paints");
    assert_eq!(
        frame.image_evictions,
        vec![displaced],
        "exactly one rectangle, reported exactly once"
    );
    assert_eq!(frame.image_uploads.len(), 1);
    assert_eq!(
        frame.image_uploads[0].region, displaced,
        "the freed rectangle is what the new occupant took, which is why the \
         plan has to be serviced clears-first"
    );
    assert!(
        FOUR_SLOT_BUDGET.contains(frame.image_uploads[0].region, frame.atlas_layers),
        "and it still lies inside the atlas this frame asks for"
    );
}

#[test]
fn a_displaced_image_becomes_resident_again_on_the_frame_that_draws_it() {
    // What "degrades to re-uploads" means at the seam: the image that lost its
    // rectangle is not lost, it is merely no longer resident, and the next
    // frame that draws it schedules a fresh upload rather than a skipped draw.
    let mut compiler = SceneCompiler::with_atlas_budget(VIEWPORT.0, VIEWPORT.1, FOUR_SLOT_BUDGET);
    let first = image(32, 32);
    let rest: Vec<ImageData> = (0..3).map(|_| image(32, 32)).collect();

    compile(
        &mut compiler,
        &scene_of(|b| {
            b.draw_image(&first, DEST);
            for data in &rest {
                b.draw_image(data, DEST);
            }
        }),
    );
    compiler.acknowledge_image_plan();

    let arrival = image(32, 32);
    compile(&mut compiler, &scene_of(|b| b.draw_image(&arrival, DEST)));
    compiler.acknowledge_image_plan();
    assert_eq!(compiler.images().pressure_evictions(), 1);

    let redrawn = compile(&mut compiler, &scene_of(|b| b.draw_image(&first, DEST)));
    assert_eq!(redrawn.skipped_images, 0, "the displaced image draws again");
    assert_eq!(redrawn.image_draws, 1);
    assert_eq!(
        redrawn.image_uploads.len(),
        1,
        "at the cost of one upload — the trade the eviction made"
    );
}

/// The `frust-perf img` line exists only in a `perf-trace` build, like every
/// other `frust-perf` line: a release-lean binary must carry none of its bytes.
/// `cargo test -p frust-engine --features perf-trace` is where this runs; the
/// absence half is `the_residency_line_is_absent_without_perf_trace` below.
#[cfg(feature = "perf-trace")]
#[test]
fn the_per_frame_residency_line_is_written_only_when_something_happened() {
    // The line a benchmark capture is graded by: its absence means the atlas
    // held the scene, `skipped>0` means draws were lost, and `evicted>0` alone
    // means the working set is larger than the atlas and the screen is still
    // complete. Field names and order are pinned here because a capture is
    // read by grepping them.
    let mut compiler = SceneCompiler::with_atlas_budget(VIEWPORT.0, VIEWPORT.1, FOUR_SLOT_BUDGET);
    let resident: Vec<ImageData> = (0..4).map(|_| image(32, 32)).collect();

    let quiet = compile(
        &mut compiler,
        &scene_of(|b| {
            for data in &resident {
                b.draw_image(data, DEST);
            }
        }),
    );
    assert!(
        !image_pressure_reported(&quiet, compiler.images()),
        "a frame the atlas held reports nothing at all"
    );
    compiler.acknowledge_image_plan();

    let arrival = image(32, 32);
    let evicting = compile(&mut compiler, &scene_of(|b| b.draw_image(&arrival, DEST)));
    assert!(image_pressure_reported(&evicting, compiler.images()));
    assert_eq!(
        image_pressure_line(&evicting, compiler.images()),
        "frust-perf img skipped=0 evicted=1 resident=4 budget=64x64x1",
    );

    // And a frame that genuinely lost draws says so in the same line.
    let mut full = SceneCompiler::with_atlas_budget(VIEWPORT.0, VIEWPORT.1, FOUR_SLOT_BUDGET);
    let crowd: Vec<ImageData> = (0..6).map(|_| image(32, 32)).collect();
    let skipping = compile(
        &mut full,
        &scene_of(|b| {
            for data in &crowd {
                b.draw_image(data, DEST);
            }
        }),
    );
    assert!(image_pressure_reported(&skipping, full.images()));
    assert_eq!(
        image_pressure_line(&skipping, full.images()),
        "frust-perf img skipped=2 evicted=0 resident=4 budget=64x64x1",
    );
}

/// The other half of M1/M3: without the feature the route is not merely quiet,
/// it is not compiled. A `use` of either symbol here would fail to resolve, so
/// the check is that the *counters* the line reads stay available in every
/// build — tests and the residency's own bookkeeping depend on them — while the
/// formatter and its `frust-perf` literal do not exist to be linked.
#[cfg(not(feature = "perf-trace"))]
#[test]
fn the_residency_line_is_absent_without_perf_trace() {
    let mut compiler = SceneCompiler::with_atlas_budget(VIEWPORT.0, VIEWPORT.1, FOUR_SLOT_BUDGET);
    let resident: Vec<ImageData> = (0..4).map(|_| image(32, 32)).collect();
    compile(
        &mut compiler,
        &scene_of(|b| {
            for data in &resident {
                b.draw_image(data, DEST);
            }
        }),
    );
    compiler.acknowledge_image_plan();

    let arrival = image(32, 32);
    let evicting = compile(&mut compiler, &scene_of(|b| b.draw_image(&arrival, DEST)));
    assert_eq!(evicting.skipped_images, 0);
    assert_eq!(
        compiler.images().frame_pressure_evictions(),
        1,
        "the counters a report would read are compiled in every build"
    );
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
    compiler.acknowledge_image_plan();

    // Inside the window nothing is reclaimed, however many frames draw nothing.
    let empty = scene_of(|_| {});
    for _ in 0..MAX_UNSEEN_FRAMES {
        let frame = compile(&mut compiler, &empty);
        assert!(frame.image_evictions.is_empty());
        compiler.acknowledge_image_plan();
    }

    let reaped = compile(&mut compiler, &empty);
    assert_eq!(
        reaped.image_evictions,
        vec![region],
        "the padded rectangle the image held is handed back for clearing"
    );
    assert_eq!(compiler.images().entry_count(), 0);
    compiler.acknowledge_image_plan();

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

/// `perf-trace`-only, with the line itself — see
/// `the_per_frame_residency_line_is_written_only_when_something_happened`.
#[cfg(feature = "perf-trace")]
#[test]
fn the_resolved_atlas_tier_is_reported_once_in_a_line_a_capture_keeps() {
    // Which of the two tiers a device takes is otherwise an inference from
    // whether its images went missing. The line is `frust-perf`-prefixed
    // because that is what a benchmark capture keeps, and it names both signals
    // `is_mobile_tier` reads so a surprising tier can be explained from the log
    // alone rather than re-derived from the adapter.
    let caps = TierCaps::fake(DownlevelProfile::WebGl2);
    assert_eq!(
        atlas_tier_line(&caps, AtlasBudget::for_caps(&caps)),
        "frust-perf atlas tier=mobile budget=1024x1024x4 downlevel=WebGl2 \
         transient_saves_memory=false adapter=fake-webgl2"
    );

    let desktop = TierCaps::fake(DownlevelProfile::Full);
    let line = atlas_tier_line(&desktop, AtlasBudget::for_caps(&desktop));
    assert!(line.starts_with("frust-perf atlas tier=desktop budget=2048x2048x8"));

    // Once, not once per surface: the question is about the device.
    let latch = TierLogOnce::new();
    assert!(
        latch.emit(&caps, AtlasBudget::MOBILE),
        "the first call writes it"
    );
    assert!(
        !latch.emit(&caps, AtlasBudget::MOBILE),
        "no later one repeats it"
    );
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
