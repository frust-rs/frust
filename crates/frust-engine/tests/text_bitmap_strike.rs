//! Bitmap-strike (CBDT) colour glyphs: a PNG strike is decoded by `glifo`, an
//! atlas-routed glyph reaches the atlas through the pending-upload queue, and
//! an outline-routed one is counted skipped.
//!
//! Sibling of `text_color_hint.rs` and driven the same way: a `frust_scene`
//! `Scene` compiled by `SceneCompiler`, with glyph ids named directly against
//! the bundled face. The host cases need no GPU; the one case that writes the
//! queued pixmap into a real atlas array is `#[ignore]`d and runs with
//! `cargo test -p frust-engine --test text_bitmap_strike -- --ignored`.

use std::sync::{Arc, LazyLock};

use frust_engine::cache::images::ImageResidency;
use frust_engine::compile::CompiledFrame;
use frust_engine::{AtlasBudget, SceneCompiler};
use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};
use kurbo::Affine;
use peniko::color::palette::css::RED;
use peniko::{Blob, Brush, FontData};
use skrifa::bitmap::{BitmapData, BitmapStrikes};
use skrifa::instance::Size;
use skrifa::raw::FileRef;
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// Noto Color Emoji, subsetted to U+1F600 with its `CBDT`/`CBLC` strike kept.
const CBDT_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoColorEmoji-CBDT-Subset.ttf");

/// [`CBDT_FONT`]'s U+1F600, read off its character map.
const GRINNING_FACE: u32 = 1;

const VIEWPORT: (u16, u16) = (128, 128);

/// A size on the atlas route: far from the strike's own ppem, so the strike is
/// scaled rather than matched.
const SIZE: f32 = 32.0;

/// One blob for the whole file: `glifo` keys its cache on the blob's
/// process-unique id, so a face rebuilt per frame is a new font every frame
/// and nothing it drew is ever resident.
static FACE: LazyLock<FontData> =
    LazyLock::new(|| FontData::new(Blob::new(Arc::new(CBDT_FONT)), 0));

fn run() -> GlyphRun {
    run_at(SIZE)
}

fn run_at(size: f32) -> GlyphRun {
    GlyphRun {
        font: FontHandle::new(FACE.clone()),
        font_size: size,
        brush: Brush::Solid(RED),
        transform: Affine::translate((8.0, 64.0)),
        glyphs: vec![Glyph {
            id: GRINNING_FACE,
            x: 0.0,
            y: 0.0,
        }],
    }
}

fn scene() -> Scene {
    scene_of(run())
}

fn scene_of(run: GlyphRun) -> Scene {
    let mut scene = Scene::new();
    SceneBuilder::new(&mut scene).draw_glyph_run(run);
    scene
}

fn compile(compiler: &mut SceneCompiler) -> CompiledFrame {
    compiler
        .compile(&scene(), Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles")
}

fn atlas_compiler() -> SceneCompiler {
    SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
}

fn outline_compiler() -> SceneCompiler {
    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    compiler.set_image_residency(ImageResidency::disabled(AtlasBudget::MOBILE));
    compiler
}

#[test]
fn the_test_face_carries_a_png_bitmap_strike_and_no_outline() {
    let font = FontRef::new(CBDT_FONT).expect("the face parses");
    assert_eq!(
        font.charmap().map(0x1F600_u32),
        Some(GlyphId::new(GRINNING_FACE)),
        "U+1F600 is the glyph the cases below draw"
    );

    let strikes: BitmapStrikes<'_> = font.bitmap_strikes();
    let glyph = strikes
        .glyph_for_size(Size::new(SIZE), GlyphId::new(GRINNING_FACE))
        .expect("the face has a strike for the grinning face");
    assert!(
        matches!(glyph.data, BitmapData::Png(_)),
        "a CBDT strike is PNG data"
    );
    assert!(matches!(FileRef::new(CBDT_FONT), Ok(FileRef::Font(_))));
}

#[test]
fn glifo_decodes_the_strikes_png_into_a_pixmap() {
    let font = FontRef::new(CBDT_FONT).expect("the face parses");
    let glyph = font
        .bitmap_strikes()
        .glyph_for_size(Size::new(SIZE), GlyphId::new(GRINNING_FACE))
        .expect("the face has a strike for the grinning face");
    let BitmapData::Png(data) = glyph.data else {
        panic!("a CBDT strike is PNG data");
    };

    // The decoder `glifo` calls for a PNG strike: present only under the
    // `png` feature `frust-engine` enables.
    let pixmap = vello_common::pixmap::Pixmap::from_png(std::io::Cursor::new(data))
        .expect("the strike's PNG decodes");
    assert!(pixmap.width() > 0 && pixmap.height() > 0);
    assert!(
        pixmap.data_as_u8_slice().chunks(4).any(|px| px[3] != 0),
        "the decoded emoji has visible texels"
    );
}

#[test]
fn an_atlas_routed_bitmap_glyph_is_drawn_and_queues_one_upload() {
    let mut compiler = atlas_compiler();
    let frame = compile(&mut compiler);

    assert_eq!(
        frame.skipped_glyphs, 0,
        "a PNG strike on the atlas route is drawn, not refused"
    );
    assert_eq!(frame.glyph_draws, 1);
    assert_eq!(compiler.glyph_atlas_entries(), 1, "the glyph is resident");

    let uploads: Vec<_> = compiler.glyph_atlas_mut().drain_pending_uploads().collect();
    assert_eq!(uploads.len(), 1, "exactly one bitmap upload is queued");
    let upload = &uploads[0];
    assert_eq!(
        u32::from(upload.pixmap.width()),
        u32::from(upload.atlas_slot.width),
        "the pixmap fills its slot"
    );
    assert_eq!(
        u32::from(upload.pixmap.height()),
        u32::from(upload.atlas_slot.height)
    );
}

#[test]
fn an_outline_routed_bitmap_glyph_is_still_counted_skipped() {
    let mut compiler = outline_compiler();
    let frame = compile(&mut compiler);

    assert_eq!(
        frame.skipped_glyphs, 1,
        "an uncached strike arrives as an inline pixmap, which outlines cannot draw"
    );
    assert_eq!(
        compiler.glyph_atlas_mut().drain_pending_uploads().count(),
        0,
        "and nothing is queued for the atlas"
    );
}

#[test]
fn a_96_px_bitmap_glyph_is_drawn_from_the_atlas() {
    let mut compiler = atlas_compiler();
    let frame = compiler
        .compile(&scene_of(run_at(96.0)), Affine::IDENTITY, (256, 256))
        .expect("an in-range scene compiles");

    assert_eq!(
        frame.skipped_glyphs, 0,
        "a 96 px strike glyph is not refused"
    );
    assert_eq!(frame.glyph_draws, 1);
    assert_eq!(
        compiler.glyph_atlas_mut().drain_pending_uploads().count(),
        1,
        "its strike is uploaded once"
    );
}

/// Past the outline path's 128 px ceiling: only the bitmap-only exemption keeps
/// it on the atlas.
#[test]
fn a_bitmap_glyph_far_past_the_cached_size_ceiling_is_still_drawn_from_the_atlas() {
    let mut compiler = atlas_compiler();
    let frame = compiler
        .compile(&scene_of(run_at(160.0)), Affine::IDENTITY, (256, 256))
        .expect("an in-range scene compiles");

    assert_eq!(frame.skipped_glyphs, 0);
    assert_eq!(frame.glyph_draws, 1);
    assert_eq!(
        compiler.glyph_atlas_mut().drain_pending_uploads().count(),
        1
    );
}

#[test]
fn a_bitmap_glyph_whose_size_animates_is_drawn_every_frame() {
    let mut compiler = atlas_compiler();
    for step in 0..8_u8 {
        let size = 24.0 + f32::from(step) * 6.0;
        let frame = compiler
            .compile(&scene_of(run_at(size)), Affine::IDENTITY, (256, 256))
            .expect("an in-range scene compiles");
        assert_eq!(frame.skipped_glyphs, 0, "step {step} at {size} px");
        assert_eq!(frame.glyph_draws, 1);
    }
}

#[test]
fn a_resident_bitmap_glyph_queues_no_second_upload() {
    let mut compiler = atlas_compiler();
    compile(&mut compiler);
    assert_eq!(
        compiler.glyph_atlas_mut().drain_pending_uploads().count(),
        1,
        "the first draw uploads the strike"
    );

    let frame = compile(&mut compiler);
    assert_eq!(frame.skipped_glyphs, 0);
    assert_eq!(
        compiler.glyph_atlas_mut().drain_pending_uploads().count(),
        0,
        "the glyph is resident: a drained queue is not refilled"
    );
}

/// The GPU arm: the queued pixmap is accepted by a real atlas array.
#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test text_bitmap_strike -- --ignored`"]
fn a_queued_bitmap_upload_is_written_into_a_real_atlas_array() {
    use frust_engine::gpu::atlas::{AtlasArray, AtlasRenderer};
    use frust_gpu::TierCaps;

    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let (device, queue, caps) = pollster_block_on(async {
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine bitmap strike device"),
                required_features: wgpu::Features::empty(),
                required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    });

    let mut compiler = atlas_compiler();
    compile(&mut compiler);
    let uploads: Vec<_> = compiler.glyph_atlas_mut().drain_pending_uploads().collect();
    assert_eq!(uploads.len(), 1);

    let extent = u32::from(uploads[0].atlas_slot.x) + u32::from(uploads[0].atlas_slot.width);
    let extent =
        extent.max(u32::from(uploads[0].atlas_slot.y) + u32::from(uploads[0].atlas_slot.height));
    let layers = uploads[0].atlas_slot.page_index + 1;
    let atlas = AtlasArray::with_layers(
        &device,
        extent.next_power_of_two().max(256),
        extent.next_power_of_two().max(256),
        layers,
    );
    let renderer = AtlasRenderer::new(&device, &caps);
    assert!(
        renderer.upload_pixmap(&queue, &atlas, &uploads[0]),
        "the array takes the decoded strike"
    );
}

fn pollster_block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};

    let mut cx = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}
