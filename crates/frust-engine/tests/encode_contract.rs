//! The encode contract on real hardware: what [`EngineRenderer::encode`]
//! promises about the encoder it is handed, and that a frame actually reaches
//! the target.
//!
//! Every other test in this crate is device-free — layouts, sizing, pool
//! keying, compilation. This one exists because the contract it checks cannot
//! be checked without a GPU:
//!
//! - **Nothing is left open.** After `encode` returns, the caller records a
//!   second, entirely unrelated pass into the *same* encoder. A pass the
//!   engine failed to end would make that illegal and `finish` would raise a
//!   validation error.
//! - **Nothing is submitted.** That second pass is recorded *after* `encode`
//!   and its result is read back, so it can only have executed because the one
//!   command buffer the test submits still carried both. An engine-side
//!   `queue.submit` would have shipped the frame's passes early, in a separate
//!   buffer.
//! - **The frame is really drawn.** The read-back pixels are checked against
//!   the scene, not merely for being non-empty, so a pass that ran but drew
//!   nothing fails.
//!
//! The whole file runs under a validation error scope, so a wrong bind group,
//! a mismatched attachment or an out-of-range draw surfaces as a named error
//! rather than as a plausible-looking image.

#![cfg(not(target_arch = "wasm32"))]

use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
use frust_gpu::{HeadlessTarget, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, BezPath, Point, Rect};
use peniko::color::palette::css::{BLUE, GREEN, RED};
use peniko::{Brush, Color};

/// Target extent every case renders at.
const SIZE: u32 = 256;

/// The target format, chosen for a read-back whose byte order needs no
/// swizzling: `Rgba8Unorm` reads back R, G, B, A in that order.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The colour the second, unrelated pass clears its own target to — an
/// arbitrary opaque value nothing else in the test produces.
const SECOND_PASS_CLEAR: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 1.0,
    a: 1.0,
};

/// A pixel-aligned rectangle, taking the compiler's fast rectangle path.
const FILL_RECT: Rect = Rect::new(32.0, 32.0, 96.0, 96.0);

/// Blocks on `future` by polling it to completion.
///
/// wgpu's native adapter and device requests resolve without an executor
/// driving them, so a bare poll loop is enough; this crate has no async
/// runtime of its own.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

/// Pops a validation error scope, pumping the device until the pop resolves.
fn drain_error_scope(device: &wgpu::Device, scope: wgpu::ErrorScopeGuard) -> Option<wgpu::Error> {
    use std::task::{Context, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = std::pin::pin!(scope.pop());
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(error) => return error,
            Poll::Pending => {
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
            }
        }
    }
}

/// A device plus the capabilities probed off the adapter it came from.
fn gpu() -> (wgpu::Device, wgpu::Queue, TierCaps) {
    block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        // The environment-aware initializer, so `WGPU_ADAPTER_NAME` picks the
        // GPU on a multi-adapter host instead of the run silently landing on
        // whichever one enumerates first.
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        println!(
            "frust-engine encode contract adapter: {:?}",
            adapter.get_info()
        );
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine encode contract device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// A four-point closed diamond, well clear of [`FILL_RECT`].
fn diamond() -> BezPath {
    let mut path = BezPath::new();
    path.move_to((176.0, 116.0));
    path.line_to((236.0, 176.0));
    path.line_to((176.0, 236.0));
    path.line_to((116.0, 176.0));
    path.close_path();
    path
}

/// The scene every case encodes: a fast-path rectangle, an arbitrary filled
/// path, and a stroke — the three geometry kinds the compiler lowers.
fn scene() -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    builder.fill_rect(FILL_RECT, Brush::Solid(RED));
    builder.fill_path(diamond(), Brush::Solid(BLUE));
    builder.stroke_line(
        Point::new(16.0, 200.0),
        Point::new(96.0, 200.0),
        6.0,
        Brush::Solid(GREEN),
    );
    scene
}

/// Finishes the renderer's background pipeline warm-up before the test drops
/// the device.
///
/// Not a nicety. Warm-up runs on a worker holding its own handle on the
/// `wgpu::Device`, and a test that returns while it is still compiling drops
/// the device out from under it — which the driver reports as a heap
/// corruption at process exit, long after every assertion has passed. Forcing
/// the warm-up to completion keeps a teardown race from failing a run whose
/// assertions all succeeded, or from being read as a rendering fault.
fn settle(renderer: &mut EngineRenderer, device: &wgpu::Device) {
    renderer.finish_warm_up(device);
    assert!(
        renderer.compiled_pipelines() > 0,
        "warm-up compiled no pipeline at all"
    );
}

/// The RGBA bytes of the pixel at `(x, y)` in a tightly packed read-back.
fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let index = ((y * SIZE + x) * 4) as usize;
    pixels[index..index + 4]
        .try_into()
        .expect("a read-back row holds four bytes per pixel")
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test encode_contract -- --ignored`"]
fn encode_records_into_the_callers_encoder_without_submitting_or_leaving_a_pass_open() {
    let (device, queue, caps) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    // The second pass's target: entirely unrelated to the frame, so the only
    // thing it shares with it is the encoder.
    let unrelated = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);

    let mut renderer = EngineRenderer::new(&device, &caps, FORMAT, None)
        .expect("the engine builds on this device");
    let scene = scene();

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frust-engine encode contract"),
    });

    renderer
        .encode(
            &device,
            &queue,
            &mut encoder,
            &scene,
            EngineTarget {
                view: target.view(),
                format: FORMAT,
                width: SIZE,
                height: SIZE,
                depth: None,
                output: OutputAlpha::Premultiplied,
            },
            Color::TRANSPARENT,
            Affine::IDENTITY,
        )
        .expect("a well-formed frame encodes");

    // The contract, exercised: a pass left open by the engine makes this
    // illegal, and a submit inside the engine would have already shipped the
    // frame's passes without this one.
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("unrelated pass after encode"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: unrelated.view(),
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(SECOND_PASS_CLEAR),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));

    // One submit, for both the engine's passes and the caller's own.
    queue.submit([encoder.finish()]);
    renderer.end_frame(&queue);

    // Drained before the pixels are read: a validation error names what went
    // wrong, where a pixel mismatch only says that something did.
    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "encoding raised {error:?}");

    let pixels = target.read_back(&device, &queue);
    assert_eq!(pixels.len(), (SIZE * SIZE * 4) as usize);
    assert!(
        pixels.iter().any(|byte| *byte != 0),
        "the frame drew nothing at all"
    );

    // The rectangle's interior: opaque red, written by the opaque pass.
    assert_eq!(pixel(&pixels, 64, 64), [255, 0, 0, 255]);
    // The filled path's interior.
    assert_eq!(pixel(&pixels, 176, 176), [0, 0, 255, 255]);
    // The stroke, six pixels wide about y = 200.
    assert_eq!(pixel(&pixels, 56, 200), [0, 128, 0, 255]);
    // Untouched background, proving the clear ran and the frame did not simply
    // fill the whole target.
    assert_eq!(pixel(&pixels, 4, 4), [0, 0, 0, 0]);
    assert_eq!(pixel(&pixels, 250, 8), [0, 0, 0, 0]);

    // The unrelated pass executed, from the same command buffer.
    let unrelated_pixels = unrelated.read_back(&device, &queue);
    assert_eq!(pixel(&unrelated_pixels, 128, 128), [0, 0, 255, 255]);

    settle(&mut renderer, &device);
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test encode_contract -- --ignored`"]
fn a_renderer_encodes_frame_after_frame_and_survives_a_resize() {
    let (device, queue, caps) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let mut renderer = EngineRenderer::new(&device, &caps, FORMAT, None)
        .expect("the engine builds on this device");
    let scene = scene();

    // Three frames at the original extent, then a resize and one more at the
    // new one: the resize drops the size-derived resources, so a frame after
    // it re-establishes them rather than reusing a stale attachment.
    for extent in [SIZE, SIZE, SIZE, SIZE / 2] {
        if extent != SIZE {
            renderer.resize(&device, extent, extent);
        }
        let target = HeadlessTarget::new(&device, extent, extent, FORMAT);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine repeat frame"),
        });
        renderer
            .encode(
                &device,
                &queue,
                &mut encoder,
                &scene,
                EngineTarget {
                    view: target.view(),
                    format: FORMAT,
                    width: extent,
                    height: extent,
                    depth: None,
                    output: OutputAlpha::Premultiplied,
                },
                Color::TRANSPARENT,
                Affine::IDENTITY,
            )
            .expect("a well-formed frame encodes");
        queue.submit([encoder.finish()]);
        renderer.end_frame(&queue);

        let pixels = target.read_back(&device, &queue);
        let index = (((extent / 4) * extent + extent / 4) * 4) as usize;
        assert_eq!(
            &pixels[index..index + 4],
            [255, 0, 0, 255],
            "the rectangle is drawn at every extent (extent {extent})"
        );
    }

    settle(&mut renderer, &device);

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "repeat encoding raised {error:?}");
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test encode_contract -- --ignored`"]
fn a_caller_supplied_depth_attachment_is_used_instead_of_an_engine_owned_one() {
    let (device, queue, caps) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let depth = device.create_texture(&frust_engine::gpu::depth::depth_texture_descriptor(
        SIZE, SIZE,
    ));
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

    let mut renderer = EngineRenderer::new(&device, &caps, FORMAT, None)
        .expect("the engine builds on this device");
    let scene = scene();

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frust-engine caller depth"),
    });
    renderer
        .encode(
            &device,
            &queue,
            &mut encoder,
            &scene,
            EngineTarget {
                view: target.view(),
                format: FORMAT,
                width: SIZE,
                height: SIZE,
                depth: Some(&depth_view),
                output: OutputAlpha::Premultiplied,
            },
            Color::TRANSPARENT,
            Affine::IDENTITY,
        )
        .expect("a frame with a caller-supplied depth attachment encodes");
    queue.submit([encoder.finish()]);
    renderer.end_frame(&queue);

    let pixels = target.read_back(&device, &queue);
    assert_eq!(pixel(&pixels, 64, 64), [255, 0, 0, 255]);

    settle(&mut renderer, &device);

    let error = drain_error_scope(&device, scope);
    assert!(
        error.is_none(),
        "encoding against a caller depth raised {error:?}"
    );
}
