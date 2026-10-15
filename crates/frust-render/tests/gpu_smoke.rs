//! GPU smoke tests: render Frust scenes offscreen through
//! [`frust_render::HeadlessRenderer`] and read the pixels back. These exercise
//! the real `frust-engine` / wgpu 30 pipeline end to end, so they are
//! `#[ignore]`d and run manually on hardware with a GPU:
//!
//! ```text
//! cargo test -p frust-render --test gpu_smoke -- --ignored --nocapture
//! ```
//!
//! CI sandboxes without a GPU are expected to skip them; a local pass on a
//! deliberately selected adapter is the gate. On a multi-GPU host, name the
//! adapter (`WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400`) — the harness routes
//! adapter selection through the environment-aware initializer, so that knob
//! is honoured, and `FRUST_GOLDEN_EXPECT_ADAPTER`/`FRUST_GOLDEN_EXPECT_BACKEND`
//! turn "which GPU did this run on?" into a refusal rather than a footnote.
//! `--nocapture` prints the resolved adapter metadata each test records.
//!
//! Every case renders through the same public seams an app does (the harness
//! drives `frust_engine::EngineRenderer` exactly as `SurfaceRenderer::submit`
//! does), so the actual scene-to-pixels path is what gets validated, not a
//! hand-written engine frame; the harness itself brackets each render in a
//! wgpu `Validation` error scope and fails on any captured error, so an
//! uncaptured-validation-error assertion is no longer each test's own job.
//!
//! Pixels come back PREMULTIPLIED (the engine's own convention — see
//! [`frust_render::HeadlessImage`]), which the assertions below are written
//! against; every content pixel they probe is opaque, where the two
//! conventions agree, apart from the fully-erased ones the hole-punch case
//! checks.

use frust_render::{HeadlessOptions, HeadlessRenderer, HeadlessSpec};
use frust_scene::{Scene, SceneBuilder};
use peniko::Brush;
use peniko::color::palette::css::{BLACK, BLUE, GREEN, RED};

const SIZE: u32 = 64;

/// Creates the offscreen renderer and prints the GPU it resolved — the
/// provenance (`docs/TESTING.md` § GPU Run Metadata) every promoted baseline
/// and every failing artifact has to record.
async fn headless() -> HeadlessRenderer {
    let renderer = HeadlessRenderer::new(HeadlessOptions::default())
        .await
        .expect("failed to create the headless renderer");
    println!("frust gpu_smoke: {}", renderer.meta());
    renderer
}

#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn red_fill_rect_produces_non_zero_pixels() {
    pollster::block_on(run());
}

async fn run() {
    let mut headless = headless().await;

    // Build a Frust scene with a full-surface red rect.
    let mut fk_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut fk_scene);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Solid(RED),
        );
    }
    let image = headless
        .render(&fk_scene, &HeadlessSpec::new(SIZE, SIZE))
        .await
        .expect("headless render failed");

    let non_zero = image.rgba8.iter().any(|&b| b != 0);
    assert!(non_zero, "expected non-zero pixels from a red FillRect");

    // Spot-check the centre pixel is dominated by red.
    let [r, g, b, _] = image.pixel(SIZE / 2, SIZE / 2);
    assert!(
        r > g && r > b,
        "centre pixel should be red-dominant, got rgb=({r},{g},{b})"
    );
}

/// Pixel-level regression test for the Mode B hole punch: a `clear_rect`
/// recorded INSIDE a
/// clip/opacity layer group must still erase an opaque backdrop painted
/// OUTSIDE the group (a group-local erase would be sealed in by the group
/// composite), and the erase must be PIXEL-EXACT at a 16-px-tile-UNALIGNED
/// edge (both defects were first caught on-device).
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn clear_rect_punches_pixel_exact_through_a_layer_group() {
    pollster::block_on(run_clear_probe());
}

async fn run_clear_probe() {
    let mut headless = headless().await;

    // Opaque backdrop at ROOT; the punch inside a full-surface layer group,
    // with its right edge one px past a 16-px tile boundary (SIZE/2 + 1).
    let punch_edge = (SIZE / 2 + 1) as f64;
    let mut fk_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut fk_scene);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Solid(RED),
        );
        builder.push_layer(kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64), 1.0);
        builder.clear_rect(kurbo::Rect::new(0.0, 0.0, punch_edge, SIZE as f64));
        builder.pop_layer();
    }
    let image = headless
        .render(
            &fk_scene,
            &HeadlessSpec {
                base_color: peniko::color::palette::css::TRANSPARENT,
                ..HeadlessSpec::new(SIZE, SIZE)
            },
        )
        .await
        .expect("headless render failed");

    let px = |x: u32| {
        let [r, g, b, a] = image.pixel(x, SIZE / 2);
        (r, g, b, a)
    };
    // Inside the punch: fully erased, through the group, over the backdrop.
    assert_eq!(px(SIZE / 4), (0, 0, 0, 0), "punch centre must be erased");
    assert_eq!(
        px(SIZE / 2),
        (0, 0, 0, 0),
        "last px inside the unaligned edge must be erased"
    );
    // Just past the edge, INSIDE the same 16-px tile: the backdrop must
    // survive untouched (the Compose::Clear tile bleed this test pins down).
    for x in [SIZE / 2 + 1, SIZE / 2 + 4, SIZE / 2 + 14] {
        let (r, _, _, a) = px(x);
        assert!(
            r > 200 && a > 200,
            "backdrop at x={x} (same tile as the punch edge) must stay opaque red, got {:?}",
            px(x)
        );
    }
}

/// The inline lowering of a `Command::PushRetained` bracket, pixel for pixel
/// — the only lowering this crate has for one.
///
/// Reached through the public headless seam alone: the bracket's
/// presentation scale as a transform correction and its sub-unity alpha as a
/// layer, which is what every render path and every tier produces. This pins
/// the absolute arithmetic that lowering must hit and the z-order of a command
/// recorded after the bracket.
///
/// The geometry keeps every content edge on a whole device pixel, so nothing
/// here is measuring rasterizer subpixel coverage.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn snapshot_bracket_inline_emulation_matches_the_composite_arithmetic() {
    pollster::block_on(run_snapshot());
}

/// Per-pixel tolerance: an 8-bit rounding step, no more.
const SNAPSHOT_TOLERANCE: u8 = 3;

async fn run_snapshot() {
    // The bracket: a 20x20 body in local space under a 2x device scale offset
    // by (8, 8) — device (8, 8)..(48, 48) — composited at 75% opacity.
    const ALPHA: f32 = 0.75;
    let bracket = kurbo::Affine::translate((8.0, 8.0)) * kurbo::Affine::scale(2.0);
    let rect = kurbo::Rect::new(0.0, 0.0, 20.0, 20.0);
    let overlay = peniko::Color::from_rgba8(255, 255, 0, 255);

    let mut fk_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut fk_scene);
        // An opaque black backdrop, which is what makes the composite
        // arithmetic below exact, plus a marker outside the bracket.
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Solid(BLACK),
        );
        builder.fill_rect(kurbo::Rect::new(52.0, 52.0, 60.0, 60.0), Brush::Solid(BLUE));
        builder.push_transform(bracket);
        builder.push_retained(1, rect, ALPHA, 1.0);
        // An opaque red block, a half-alpha green band over NOTHING (so the
        // bracket really carries partial alpha — a double premultiply is
        // invisible where alpha is 1), and a black bar standing in for a line
        // of text.
        builder.fill_rect(kurbo::Rect::new(0.0, 0.0, 20.0, 10.0), Brush::Solid(RED));
        builder.fill_rect(
            kurbo::Rect::new(0.0, 10.0, 20.0, 15.0),
            Brush::Solid(GREEN.with_alpha(0.5)),
        );
        builder.fill_rect(kurbo::Rect::new(5.0, 15.0, 15.0, 20.0), Brush::Solid(BLACK));
        builder.pop_retained();
        builder.pop_transform();
        // Recorded AFTER the bracket, so it must land on top of it.
        builder.fill_rect(
            kurbo::Rect::new(20.0, 20.0, 30.0, 30.0),
            Brush::Solid(overlay),
        );
    }

    let mut headless = headless().await;
    let image = headless
        .render(&fk_scene, &HeadlessSpec::new(SIZE, SIZE))
        .await
        .expect("headless render failed");

    // The expected values are the composite arithmetic over the black
    // backdrop, not observations:
    //  - the opaque red block composites at the bracket's 0.75 -> 191,
    //  - the half-alpha green band (CSS green is 0x008000) composites at
    //    128 * 0.5 * 0.75 -> 48, which is the number a doubly premultiplied
    //    page would halve again.
    let red = image.pixel(40, 12);
    assert!(
        red[0].abs_diff(191) <= SNAPSHOT_TOLERANCE,
        "the bracket's opaque block must composite at its alpha, got {red:?}"
    );
    let green = image.pixel(40, 32);
    assert!(
        green[1].abs_diff(48) <= SNAPSHOT_TOLERANCE,
        "the bracket's half-alpha band must composite at 128 * 0.5 * 0.75, got {green:?}"
    );
    assert_eq!(
        image.pixel(24, 24),
        [255, 255, 0, 255],
        "a command recorded after the bracket must land on top of it"
    );
    let marker = image.pixel(56, 56);
    assert!(
        marker[2] > 200,
        "content recorded before the bracket must survive, got {marker:?}"
    );
}

/// The readback's row-padding path, on real hardware.
///
/// Every case above renders 64 px wide, where `64 * 4` is exactly wgpu's
/// 256-byte `copy_texture_to_buffer` row alignment and the padding arithmetic
/// is a no-op — which is why they were written at that size in the first
/// place. A width whose `width * 4` is NOT a multiple of 256 makes each copied
/// row carry trailing padding bytes, and a harness that failed to strip them
/// per row would return an image whose every row after the first is shifted:
/// still plausible pixels, silently wrong geometry. 97 px pads 388 bytes up to
/// 512, so a slip is visible on the very first row boundary.
///
/// The same renderer then renders a second, differently sized frame, covering
/// the device/engine-renderer/target reuse across renders.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn unaligned_width_reads_back_without_row_padding() {
    pollster::block_on(run_row_padding());
}

async fn run_row_padding() {
    // 97 * 4 = 388 bytes of pixels per row, padded to 512.
    const PAD_WIDTH: u32 = 97;
    const PAD_HEIGHT: u32 = 33;
    /// Right edge of the red block, on a whole device pixel so the column
    /// either side of it is exact rather than anti-aliased.
    const EDGE: u32 = 48;

    let mut headless = headless().await;

    let mut fk_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut fk_scene);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, EDGE as f64, PAD_HEIGHT as f64),
            Brush::Solid(RED),
        );
    }
    let image = headless
        .render(&fk_scene, &HeadlessSpec::new(PAD_WIDTH, PAD_HEIGHT))
        .await
        .expect("headless render failed");

    assert_eq!(
        image.rgba8.len(),
        (PAD_WIDTH * PAD_HEIGHT * 4) as usize,
        "the image must carry tightly packed rows, with no padding"
    );
    for y in 0..PAD_HEIGHT {
        let inside = image.pixel(EDGE - 1, y);
        assert!(
            inside[0] > 200 && inside[1] < 64 && inside[2] < 64,
            "row {y} must still be red up to the block's edge, got {inside:?}"
        );
        assert_eq!(
            image.pixel(EDGE, y),
            [0, 0, 0, 255],
            "row {y} must be the base color just past the block's edge"
        );
        assert_eq!(
            image.pixel(PAD_WIDTH - 1, y),
            [0, 0, 0, 255],
            "row {y}'s last pixel must be the base color, not a neighbouring row's"
        );
    }

    // A second render at a different size, through the same device and engine
    // renderer, must be just as correct.
    let mut square = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut square);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Solid(RED),
        );
    }
    let reused = headless
        .render(&square, &HeadlessSpec::new(SIZE, SIZE))
        .await
        .expect("headless render failed on a reused renderer");
    let [r, g, b, _] = reused.pixel(SIZE / 2, SIZE / 2);
    assert!(
        r > 200 && g < 64 && b < 64,
        "a reused renderer must still render, got rgb=({r},{g},{b})"
    );
}

/// A retained page, on real hardware: frame N records a `PushRetained` body
/// into a page, frame N+1 draws it back with `DrawRetained` at alpha 0.5 and
/// presentation scale 0.9, and both frames are pixel-identical to the inline
/// emulation `frust_scene` defines (the body under the scale correction inside
/// a 0.5 layer) — the page holds the body exactly as the inline lowering
/// rasterizes it and is composited 1:1. The ledger lists the key after frame N
/// at the extent and scale it was rasterized at, and a `DrawRetained` naming a
/// key the engine does not hold refuses the frame and empties the ledger.
///
/// Driven through `frust_engine::EngineRenderer` directly rather than
/// `HeadlessRenderer`, because the claim spans frames of ONE renderer and
/// reads that renderer's ledger between them — exactly what
/// `SurfaceRenderer::submit` does for a surface, minus the swapchain.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn retained_page_redraws_identically_to_the_inline_emulation() {
    const KEY: u64 = 7;
    const ALPHA: f32 = 0.5;
    const SCALE: f64 = 0.9;
    let format = wgpu::TextureFormat::Rgba8Unorm;

    let (device, queue, caps) = pollster::block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        println!("frust gpu_smoke retained adapter: {:?}", adapter.get_info());
        let caps = frust_gpu::TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-render gpu_smoke retained"),
                required_features: wgpu::Features::empty(),
                required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                ..Default::default()
            })
            .await
            .expect("failed to create device");
        (device, queue, caps)
    });
    let target = frust_gpu::HeadlessTarget::new(&device, SIZE, SIZE, format);
    let mut engine = frust_engine::EngineRenderer::new(&device, &caps, format, None)
        .expect("failed to create the engine renderer");
    assert!(engine.retained_layers(), "a renderer retains by default");

    // A 20x20 body in local space under a 2x device scale offset by (8, 8),
    // with an opaque block, a half-alpha band over nothing and a bar.
    let bracket = kurbo::Affine::translate((8.0, 8.0)) * kurbo::Affine::scale(2.0);
    let rect = kurbo::Rect::new(0.0, 0.0, 20.0, 20.0);
    let body = |builder: &mut SceneBuilder<'_>| {
        builder.fill_rect(kurbo::Rect::new(0.0, 0.0, 20.0, 10.0), Brush::Solid(RED));
        builder.fill_rect(
            kurbo::Rect::new(0.0, 10.0, 20.0, 15.0),
            Brush::Solid(GREEN.with_alpha(0.5)),
        );
        builder.fill_rect(kurbo::Rect::new(5.0, 15.0, 15.0, 20.0), Brush::Solid(BLACK));
    };
    let frame = |middle: &dyn Fn(&mut SceneBuilder<'_>)| {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Solid(BLUE),
        );
        builder.push_transform(bracket);
        middle(&mut builder);
        builder.pop_transform();
        // Recorded after the bracket, so it lands on top of it.
        builder.fill_rect(
            kurbo::Rect::new(20.0, 20.0, 30.0, 30.0),
            Brush::Solid(BLACK),
        );
        scene
    };

    let inline = frame(&|builder| {
        builder.push_transform(kurbo::Affine::scale_about(SCALE, rect.center()));
        builder.push_layer(rect, ALPHA);
        body(builder);
        builder.pop_layer();
        builder.pop_transform();
    });
    let record = frame(&|builder| {
        builder.push_retained(KEY, rect, ALPHA, SCALE);
        body(builder);
        builder.pop_retained();
    });
    let redraw = frame(&|builder| builder.draw_retained(KEY, rect, ALPHA, SCALE));
    let missing = frame(&|builder| builder.draw_retained(KEY + 1, rect, ALPHA, SCALE));

    let render = |engine: &mut frust_engine::EngineRenderer,
                  scene: &Scene|
     -> Result<Vec<u8>, frust_engine::EngineError> {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let encoded = engine.encode(
            &device,
            &queue,
            &mut encoder,
            scene,
            frust_engine::EngineTarget {
                view: target.view(),
                format,
                width: SIZE,
                height: SIZE,
                depth: None,
                output: frust_engine::OutputAlpha::Premultiplied,
            },
            peniko::Color::BLACK,
            kurbo::Affine::IDENTITY,
        );
        queue.submit([encoder.finish()]);
        engine.end_frame(&queue);
        encoded.map(|()| target.read_back(&device, &queue))
    };

    let expected = render(&mut engine, &inline).expect("the inline frame renders");
    let recorded = render(&mut engine, &record).expect("frame N renders");
    let ledger: Vec<_> = engine.retained_ledger().collect();
    assert_eq!(ledger.len(), 1, "frame N leaves its page resident");
    assert_eq!(ledger[0].key, KEY);
    assert_eq!(ledger[0].extent_px, (36, 36), "20 x 2 x 0.9 on each axis");
    assert!((ledger[0].scale - 1.8).abs() < 1e-6, "{}", ledger[0].scale);

    let redrawn = render(&mut engine, &redraw).expect("frame N+1 renders");
    let differing =
        |a: &[u8], b: &[u8]| a.chunks(4).zip(b.chunks(4)).filter(|(p, q)| p != q).count();
    assert_eq!(
        differing(&recorded, &expected),
        0,
        "the frame that records the page draws exactly the inline emulation"
    );
    assert_eq!(
        differing(&redrawn, &expected),
        0,
        "the frame drawing the page back draws exactly the inline emulation"
    );

    let refused = render(&mut engine, &missing);
    assert!(
        matches!(
            refused,
            Err(frust_engine::EngineError::RetainedKeyMissing { key }) if key == KEY + 1
        ),
        "a DrawRetained of a key the engine does not hold refuses the frame: {:?}",
        refused.map(|_| ())
    );
    assert_eq!(engine.retained_ledger().count(), 0, "and empties the store");
    assert!(engine.take_retained_refusal());
}

/// A frosted-glass backdrop over a gradient, through the same headless seam
/// every case above takes: the panel's centre is blurred, and every pixel
/// outside the panel is exactly what the same frame without the backdrop
/// paints.
///
/// The gradient carries a hard stop — red to blue at the frame's middle — so a
/// blur has something to show: a smooth ramp is its own blur, a step is not.
/// The headless target is `Rgba8Unorm` and copyable, so the engine reads the
/// panel's region back by a texture copy; a swapchain frame takes the
/// root-via-intermediate route instead (`context::frame_renders_via_intermediate`).
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn a_backdrop_blur_softens_the_gradient_beneath_it_and_nothing_else() {
    pollster::block_on(run_backdrop());
}

async fn run_backdrop() {
    use peniko::{ColorStop, Gradient};

    const PANEL: kurbo::Rect = kurbo::Rect::new(16.0, 16.0, 48.0, 48.0);
    let mut headless = headless().await;
    let scene = |backdrop: bool| {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let step = Gradient::new_linear((0.0, 0.0), (SIZE as f64, 0.0)).with_stops([
            ColorStop::from((0.0, RED)),
            ColorStop::from((0.5, RED)),
            ColorStop::from((0.5, BLUE)),
            ColorStop::from((1.0, BLUE)),
        ]);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Gradient(step),
        );
        if backdrop {
            builder.backdrop_blur(PANEL, 3.0);
        }
        scene
    };
    let spec = HeadlessSpec::new(SIZE, SIZE);
    let plain = headless
        .render(&scene(false), &spec)
        .await
        .expect("the plain frame renders");
    let blurred = headless
        .render(&scene(true), &spec)
        .await
        .expect("the backdrop frame renders");

    // Outside the panel: untouched, to the byte.
    for y in 0..SIZE {
        for x in 0..SIZE {
            let inside = PANEL.contains(kurbo::Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5));
            if !inside {
                assert_eq!(
                    blurred.pixel(x, y),
                    plain.pixel(x, y),
                    "pixel ({x}, {y}) outside the panel changed"
                );
            }
        }
    }

    // At the centre, the step is blurred: the two pixels either side of it mix
    // red and blue, where the plain frame keeps them pure.
    let [r_plain, _, b_plain, _] = plain.pixel(SIZE / 2 - 1, SIZE / 2);
    assert!(r_plain > 200 && b_plain < 20, "the plain step is sharp");
    for x in [SIZE / 2 - 1, SIZE / 2] {
        let [r, _, b, a] = blurred.pixel(x, SIZE / 2);
        assert!(
            r > 60 && b > 60 && a == 255,
            "the panel's centre column {x} mixes the two sides of the step: rgb=({r},_,{b}) a={a}"
        );
    }
    // Deep inside the panel, away from the step, the blur of a flat colour is
    // that colour.
    let [r, g, b, _] = blurred.pixel(20, SIZE / 2);
    assert!(
        r > 200 && g < 20 && b < 60,
        "flat red stays red: ({r},{g},{b})"
    );
}

// ---------------------------------------------------------------------------
// The partial-repaint spike (`FRUST_PARTIAL_REPAINT`), driven through the
// engine seams `SurfaceRenderer::submit` drives it through: a full frame
// rendered straight into a persistent accumulator, then a frame confined to a
// scissor rendered into a readable root, its applied rectangle copied into the
// accumulator, and the whole accumulator presented into a swapchain stand-in
// by `CopyPresentPass`.
// ---------------------------------------------------------------------------

/// The swapchain's stand-in format — a plain copy needs nothing of it.
const PARTIAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A device on the environment's adapter (`WGPU_ADAPTER_NAME` honoured),
/// with that adapter's capabilities.
fn partial_device() -> (wgpu::Device, wgpu::Queue, frust_gpu::TierCaps) {
    pollster::block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        println!("frust gpu_smoke partial adapter: {:?}", adapter.get_info());
        let caps = frust_gpu::TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust gpu_smoke partial repaint"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .expect("failed to create device");
        (device, queue, caps)
    })
}

/// A `SIZE` x `SIZE` texture of the stand-in format with `usage`.
fn partial_texture(device: &wgpu::Device, usage: wgpu::TextureUsages) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("frust gpu_smoke partial texture"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PARTIAL_FORMAT,
            usage,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// The surface-side half of the spike, as `SurfaceRenderer` owns it: the
/// persistent accumulator, the readable root a confined frame renders into,
/// the depth attachment, the present pass and the swapchain stand-in.
struct PartialSurface {
    device: wgpu::Device,
    queue: wgpu::Queue,
    engine: frust_engine::EngineRenderer,
    accumulator: wgpu::TextureView,
    root: wgpu::TextureView,
    depth: frust_engine::DepthTexture,
    present: frust_engine::gpu::present::CopyPresentPass,
    swapchain: frust_gpu::HeadlessTarget,
}

impl PartialSurface {
    fn new() -> Self {
        let (device, queue, caps) = partial_device();
        let engine = frust_engine::EngineRenderer::new(&device, &caps, PARTIAL_FORMAT, None)
            .expect("failed to create the engine renderer");
        let readable = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        Self {
            accumulator: partial_texture(&device, readable | wgpu::TextureUsages::COPY_DST),
            root: partial_texture(&device, readable),
            depth: frust_engine::DepthTexture::new(&device, SIZE, SIZE),
            present: frust_engine::gpu::present::CopyPresentPass::new(
                &device,
                PARTIAL_FORMAT,
                None,
            ),
            swapchain: frust_gpu::HeadlessTarget::new(&device, SIZE, SIZE, PARTIAL_FORMAT),
            engine,
            device,
            queue,
        }
    }

    /// One presented frame of `scene`: full (`scissor: None`, rendered into
    /// the accumulator) or confined to `scissor` — then the accumulator
    /// presented whole. Returns the presented pixels and the engine's pass
    /// list.
    fn frame(
        &mut self,
        scene: &Scene,
        scissor: Option<frust_engine::renderer::ScissorRect>,
    ) -> (Vec<u8>, Vec<frust_engine::renderer::FramePass>) {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.engine.set_frame_scissor(scissor);
        let view = if scissor.is_some() {
            &self.root
        } else {
            &self.accumulator
        };
        self.engine
            .encode(
                &self.device,
                &self.queue,
                &mut encoder,
                scene,
                frust_engine::EngineTarget {
                    view,
                    format: PARTIAL_FORMAT,
                    width: SIZE,
                    height: SIZE,
                    depth: Some(self.depth.view()),
                    output: frust_engine::OutputAlpha::Premultiplied,
                },
                peniko::Color::BLACK,
                kurbo::Affine::IDENTITY,
            )
            .expect("the engine refused the frame");
        if let Some(applied) = self.engine.last_frame_scissor() {
            let origin = wgpu::Origin3d {
                x: applied.x,
                y: applied.y,
                z: 0,
            };
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: self.root.texture(),
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: self.accumulator.texture(),
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: applied.width,
                    height: applied.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.present.record(
            &self.device,
            &self.queue,
            &mut encoder,
            &self.accumulator,
            self.swapchain.view(),
            (SIZE, SIZE),
            None,
        );
        self.queue.submit([encoder.finish()]);
        self.engine.end_frame(&self.queue);
        // The pop resolves only once the queue has been pumped, so it is
        // polled against the device rather than blocked on.
        let error = {
            use std::task::{Context, Poll, Waker};
            let mut cx = Context::from_waker(Waker::noop());
            let mut pop = std::pin::pin!(scope.pop());
            loop {
                match pop.as_mut().poll(&mut cx) {
                    Poll::Ready(error) => break error,
                    Poll::Pending => {
                        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
                    }
                }
            }
        };
        assert!(
            error.is_none(),
            "the frame raised a validation error: {error:?}"
        );
        let passes = self.engine.last_frame_passes().to_vec();
        (self.swapchain.read_back(&self.device, &self.queue), passes)
    }
}

/// The RGBA bytes of pixel (`x`, `y`) of a tightly packed `SIZE`-wide image.
fn rgba_at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let index = ((y * SIZE + x) * 4) as usize;
    pixels[index..index + 4]
        .try_into()
        .expect("four bytes per pixel")
}

/// A full-surface `background` with a 16-px `BLUE` square at `square`.
fn square_scene(background: peniko::Color, square: (f64, f64)) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    builder.fill_rect(
        kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
        Brush::Solid(background),
    );
    builder.fill_rect(
        kurbo::Rect::new(square.0, square.1, square.0 + 16.0, square.1 + 16.0),
        Brush::Solid(BLUE),
    );
    scene
}

#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn a_partial_frame_repaints_its_scissor_and_presents_the_whole_accumulator() {
    let mut surface = PartialSurface::new();
    let (first, full_passes) = surface.frame(&square_scene(RED, (8.0, 8.0)), None);
    assert_eq!(rgba_at(&first, 4, 4), [255, 0, 0, 255], "red background");
    assert_eq!(rgba_at(&first, 12, 12), [0, 0, 255, 255], "blue square");

    // The second scene is GREEN with the square moved into the bottom-right
    // quadrant, but only that quadrant is repainted.
    let quadrant = frust_engine::renderer::ScissorRect::from_corners(32, 32, SIZE, SIZE);
    let second_scene = square_scene(GREEN, (40.0, 40.0));
    let (second, partial_passes) = surface.frame(&second_scene, Some(quadrant));
    assert_eq!(
        surface.engine.last_frame_scissor(),
        Some(quadrant),
        "a scissor touching no backdrop is applied as asked"
    );
    assert_eq!(
        partial_passes, full_passes,
        "a scissor changes no pass: the frame records exactly the passes an unscissored one does"
    );
    for y in 0..SIZE {
        for x in 0..SIZE {
            let inside = x >= 32 && y >= 32;
            let expected = if inside {
                // The repainted quadrant: the second scene.
                if (40..56).contains(&x) && (40..56).contains(&y) {
                    [0, 0, 255, 255]
                } else {
                    [0, 128, 0, 255]
                }
            } else {
                // Everywhere else: the first frame, untouched to the byte.
                rgba_at(&first, x, y)
            };
            assert_eq!(
                rgba_at(&second, x, y),
                expected,
                "pixel ({x}, {y}) of the presented partial frame"
            );
        }
    }
}

#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn a_partial_frame_touching_a_backdrop_blurs_exactly_as_a_full_frame_does() {
    use peniko::{ColorStop, Gradient};

    const PANEL: kurbo::Rect = kurbo::Rect::new(16.0, 16.0, 48.0, 48.0);
    let scene = |marker: bool| {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let step = Gradient::new_linear((0.0, 0.0), (SIZE as f64, 0.0)).with_stops([
            ColorStop::from((0.0, RED)),
            ColorStop::from((0.5, RED)),
            ColorStop::from((0.5, BLUE)),
            ColorStop::from((1.0, BLUE)),
        ]);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Gradient(step),
        );
        if marker {
            // Beneath the glass, so the blur has to read it.
            builder.fill_rect(
                kurbo::Rect::new(40.0, 40.0, 44.0, 44.0),
                Brush::Solid(GREEN),
            );
        }
        builder.backdrop_blur(PANEL, 3.0);
        scene
    };

    // The reference: the marked scene as one full frame.
    let mut reference = PartialSurface::new();
    let (expected, _) = reference.frame(&scene(true), None);
    drop(reference);

    // The same scene reached by a partial frame confined to the marker alone,
    // over a full frame of the unmarked one.
    let mut surface = PartialSurface::new();
    surface.frame(&scene(false), None);
    let marker = frust_engine::renderer::ScissorRect::from_corners(40, 40, 44, 44);
    let (partial, _) = surface.frame(&scene(true), Some(marker));
    let applied = surface
        .engine
        .last_frame_scissor()
        .expect("a confined frame reports its scissor");
    assert!(
        applied.x <= 16 && applied.y <= 16 && applied.x1() >= 48 && applied.y1() >= 48,
        "the scissor grew over the backdrop region it touched: {applied:?}"
    );
    for y in 0..SIZE {
        for x in 0..SIZE {
            assert_eq!(
                rgba_at(&partial, x, y),
                rgba_at(&expected, x, y),
                "pixel ({x}, {y}): the partial frame must blur exactly as the full one"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The spike's host measurement probe (not a pass/fail gate): what one frame
// of a busy page costs on this adapter today (straight into the swapchain),
// through the partial-repaint accumulator at full damage, and confined to a
// centred damage rectangle of a few sizes. Wall-clock per frame, median of
// `PROBE_FRAMES`: `gpu` is submit-to-idle (the GPU's execution, an upper
// bound), `frame` is encode + submit + idle. Prints a table; run with
// `cargo test -p frust-render --test gpu_smoke partial_repaint_cost_probe --
// --ignored --nocapture --test-threads=1`.
// ---------------------------------------------------------------------------

/// Frames timed per mode, after `PROBE_WARMUP` untimed ones.
const PROBE_FRAMES: usize = 60;
const PROBE_WARMUP: usize = 60;

/// A busy page at `width` x `height`: a full background, a grid of
/// gradient-filled rounded cards and a scatter of translucent circles over
/// them — strip-heavy, glyph-free (the probe carries no font).
fn probe_scene(width: u32, height: u32, frame: usize) -> Scene {
    use peniko::{ColorStop, Gradient};
    let (w, h) = (f64::from(width), f64::from(height));
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    builder.fill_rect(kurbo::Rect::new(0.0, 0.0, w, h), Brush::Solid(BLACK));
    let (cols, rows) = (6, 16);
    let (cw, ch) = (w / f64::from(cols), h / f64::from(rows));
    for row in 0..rows {
        for col in 0..cols {
            let x = f64::from(col) * cw;
            let y = f64::from(row) * ch;
            let gradient = Gradient::new_linear((x, y), (x + cw, y + ch))
                .with_stops([ColorStop::from((0.0, RED)), ColorStop::from((1.0, BLUE))]);
            builder.fill_rounded_rect(
                kurbo::Rect::new(x + 4.0, y + 4.0, x + cw - 4.0, y + ch - 4.0),
                12.0,
                Brush::Gradient(gradient),
            );
        }
    }
    for i in 0..60 {
        let t = (i as f64 + frame as f64 * 0.25) * 0.37;
        let center =
            kurbo::Point::new((t.sin() * 0.5 + 0.5) * w, ((t * 1.3).cos() * 0.5 + 0.5) * h);
        let path = kurbo::Shape::to_path(&kurbo::Circle::new(center, w * 0.05), 0.1);
        builder.fill_path(path, Brush::Solid(GREEN.with_alpha(0.5)));
    }
    scene
}

/// One probe surface at an arbitrary extent: the swapchain stand-in, the
/// accumulator, the readable root, the depth attachment and the present pass.
struct ProbeSurface {
    width: u32,
    height: u32,
    accumulator: wgpu::TextureView,
    root: wgpu::TextureView,
    depth: frust_engine::DepthTexture,
    present: frust_engine::gpu::present::CopyPresentPass,
    swapchain: frust_gpu::HeadlessTarget,
}

/// How a probe frame is rendered.
#[derive(Clone, Copy)]
enum ProbeMode {
    /// Today: straight into the swapchain, no present pass.
    Direct,
    /// The accumulator at full damage, then presented.
    PartialFull,
    /// Confined to a centred rectangle of this share of the surface.
    PartialConfined(f64),
}

fn probe_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("frust gpu_smoke probe texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PARTIAL_FORMAT,
            usage,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// Median of `samples` in microseconds.
fn median_us(samples: &mut [std::time::Duration]) -> f64 {
    samples.sort_unstable();
    samples[samples.len() / 2].as_secs_f64() * 1e6
}

#[test]
#[ignore = "a measurement probe, not a gate: requires a GPU and prints a table"]
fn partial_repaint_cost_probe() {
    use std::time::Instant;

    let (device, queue, caps) = partial_device();
    println!("| extent | mode | passes/frame | transfers/frame | gpu_us p50 | frame_us p50 |");
    println!("|---|---|---|---|---|---|");
    for (width, height) in [(1080, 2400), (1920, 1080)] {
        let readable = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        let surface = ProbeSurface {
            width,
            height,
            accumulator: probe_texture(
                &device,
                width,
                height,
                readable | wgpu::TextureUsages::COPY_DST,
            ),
            root: probe_texture(&device, width, height, readable),
            depth: frust_engine::DepthTexture::new(&device, width, height),
            present: frust_engine::gpu::present::CopyPresentPass::new(
                &device,
                PARTIAL_FORMAT,
                None,
            ),
            swapchain: frust_gpu::HeadlessTarget::new(&device, width, height, PARTIAL_FORMAT),
        };
        let mut engine = frust_engine::EngineRenderer::new(&device, &caps, PARTIAL_FORMAT, None)
            .expect("failed to create the engine renderer");
        engine.finish_warm_up(&device);
        let modes = [
            ("direct (today)", ProbeMode::Direct),
            ("partial, full damage", ProbeMode::PartialFull),
            ("partial, 50% damage", ProbeMode::PartialConfined(0.5)),
            ("partial, 10% damage", ProbeMode::PartialConfined(0.1)),
            ("partial, 1% damage", ProbeMode::PartialConfined(0.01)),
        ];
        for (name, mode) in modes {
            let mut gpu = Vec::with_capacity(PROBE_FRAMES);
            let mut whole = Vec::with_capacity(PROBE_FRAMES);
            let mut passes = 0;
            let mut transfers = 0;
            for frame in 0..PROBE_WARMUP + PROBE_FRAMES {
                let scene = probe_scene(width, height, frame);
                let start = Instant::now();
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                let scissor = match mode {
                    ProbeMode::PartialConfined(share) => {
                        let side = share.sqrt();
                        let (w, h) = (
                            (f64::from(width) * side) as u32,
                            (f64::from(height) * side) as u32,
                        );
                        let (x, y) = ((width - w) / 2, (height - h) / 2);
                        Some(frust_engine::renderer::ScissorRect::from_corners(
                            x,
                            y,
                            x + w,
                            y + h,
                        ))
                    }
                    _ => None,
                };
                engine.set_frame_scissor(scissor);
                let view = match mode {
                    ProbeMode::Direct => surface.swapchain.view(),
                    ProbeMode::PartialFull => &surface.accumulator,
                    ProbeMode::PartialConfined(_) => &surface.root,
                };
                engine
                    .encode(
                        &device,
                        &queue,
                        &mut encoder,
                        &scene,
                        frust_engine::EngineTarget {
                            view,
                            format: PARTIAL_FORMAT,
                            width,
                            height,
                            depth: Some(surface.depth.view()),
                            output: frust_engine::OutputAlpha::Premultiplied,
                        },
                        peniko::Color::BLACK,
                        kurbo::Affine::IDENTITY,
                    )
                    .expect("the engine refused a probe frame");
                passes = engine.last_frame_passes().len();
                transfers = 0;
                if let Some(applied) = engine.last_frame_scissor() {
                    let origin = wgpu::Origin3d {
                        x: applied.x,
                        y: applied.y,
                        z: 0,
                    };
                    encoder.copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: surface.root.texture(),
                            mip_level: 0,
                            origin,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture: surface.accumulator.texture(),
                            mip_level: 0,
                            origin,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::Extent3d {
                            width: applied.width,
                            height: applied.height,
                            depth_or_array_layers: 1,
                        },
                    );
                    transfers = 1;
                }
                if !matches!(mode, ProbeMode::Direct) {
                    surface.present.record(
                        &device,
                        &queue,
                        &mut encoder,
                        &surface.accumulator,
                        surface.swapchain.view(),
                        (surface.width, surface.height),
                        None,
                    );
                    passes += 1;
                }
                let submitted = Instant::now();
                queue.submit([encoder.finish()]);
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
                let done = Instant::now();
                engine.end_frame(&queue);
                if frame >= PROBE_WARMUP {
                    gpu.push(done - submitted);
                    whole.push(done - start);
                }
            }
            println!(
                "| {width}x{height} | {name} | {passes} | {transfers} | {:.0} | {:.0} |",
                median_us(&mut gpu),
                median_us(&mut whole)
            );
        }
    }
}
