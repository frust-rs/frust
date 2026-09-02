//! A depth-writing pass a future 3D crate could own, and the engine's own 2D
//! frame, recorded into ONE command encoder against ONE depth attachment and
//! submitted once — with the occlusion between them coming out right in both
//! orderings.
//!
//! This is the claim `EngineTarget::depth` and
//! [`EngineRenderer::set_depth_pre_cleared`] exist for, and the one nothing
//! device-free can make. The engine's depth convention is
//! `wgpu::CompareFunction::LessEqual` against a `Depth24Plus` attachment
//! cleared to the far plane 1.0, with `strip.wgsl` mapping the backmost draw
//! to z = 1.0 and every draw in front of it to a smaller z. A pass sharing the
//! attachment therefore puts *nearer* geometry at a *smaller* z, and the two
//! orderings below are what that means in practice:
//!
//! - **Depth first, frame second.** The foreign pass writes 0.3 over the left
//!   half; the frame's own draw sits at the back of the engine's z range and
//!   is rejected there, surviving only where the foreign pass wrote nothing.
//!   `set_depth_pre_cleared(true)` is what keeps the frame from clearing that
//!   depth away before it tests against it.
//! - **Frame first, depth second.** The frame owns the clear
//!   (`set_depth_pre_cleared(false)`) and establishes its own depth; the
//!   foreign pass then draws at 0.3, passes against it, and takes the left
//!   half back.
//!
//! One consequence is worth stating because a caller will otherwise discover
//! it as a surprise: the frame *always* clears its colour target to the base
//! colour, so content drawn into that target ahead of the frame keeps its
//! depth and loses its pixels. A host compositing over content it has already
//! painted records that content after the frame, or into a target of its own
//! the frame composites — the shared attachment carries the ordering, not the
//! colour.
//!
//! The WGSL here is an inline test fixture standing in for a renderer this
//! crate knows nothing about, which is why it is not a file under `shaders/`:
//! the downlevel design-rule scan governs the shaders the engine ships, and a
//! stand-in for somebody else's pass is not one of them.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use frust_engine::gpu::depth::{DEPTH_COMPARE, DEPTH_FORMAT, depth_texture_descriptor};
use frust_engine::gpu::pipelines::EnginePipeline;
use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
use frust_gpu::{HeadlessTarget, PipelineCache, RenderPipelineDesc, ShaderLibrary, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::{Brush, Color};

/// Target extent every case renders at. Both axes are a multiple of two, so
/// the half-way split lands on a texel boundary rather than inside one.
const SIZE: u32 = 64;

/// The colour format the read-back is asserted in: `Rgba8Unorm` reads back R,
/// G, B, A in that order, so no swizzling stands between a pixel and its
/// assertion.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The frame's base colour, opaque so the frame's own alpha convention never
/// enters the comparison: the pixels the frame's draw is rejected on.
const BASE: Color = Color::from_rgb8(0, 0, 255);

/// The frame's one draw, opaque and covering the whole target so its interior
/// travels through the depth-writing opaque pass.
const DRAWN: Color = Color::from_rgb8(0, 255, 0);

/// [`BASE`] as the read-back reports it.
const BASE_PIXEL: [u8; 4] = [0, 0, 255, 255];

/// [`DRAWN`] as the read-back reports it.
const DRAWN_PIXEL: [u8; 4] = [0, 255, 0, 255];

/// The foreign pass's fragment colour, matching `fs_near`.
const NEAR_PIXEL: [u8; 4] = [255, 0, 0, 255];

/// Sample columns strictly inside the foreign pass's left half, clear of the
/// half-way boundary at x = 32.
const LEFT: &[u32] = &[4, 16, 28];

/// Sample columns strictly outside it, clear of the same boundary.
const RIGHT: &[u32] = &[36, 48, 60];

/// Sample rows, spread over the target's full height.
const ROWS: &[u32] = &[8, 32, 56];

/// The per-channel bar every colour assertion is made at — one 8-bit rounding
/// step, two orders of magnitude below the gap between any two of the three
/// colours in play.
const TOLERANCE: u8 = 2;

/// The stand-in 3D renderer's program: one depth-writing draw over the
/// target's left half at a depth well in front of the far plane, generating
/// its own vertices so it needs no vertex buffer, uniform or bind group.
const WGSL: &str = r#"
const NEAR_Z: f32 = 0.3;

@vertex
fn vs_near(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 0.0, -1.0),
        vec2<f32>( 0.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 0.0,  1.0),
        vec2<f32>(-1.0,  1.0)
    );
    return vec4<f32>(corners[index], NEAR_Z, 1.0);
}

@fragment
fn fs_near() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 0.0, 1.0);
}
"#;

/// How many vertices the foreign draw expands to: two triangles, no index
/// buffer.
const VERTICES: std::ops::Range<u32> = 0..6;

/// Serializes every test in this binary that creates a GPU device — the same
/// guard the crate's other GPU suites take, for the same driver-teardown
/// reason. Poison is ignored deliberately: one test's failure must not cascade
/// into every sibling.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Blocks on `future` by polling it to completion.
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
            "frust-engine shared encoder adapter: {:?}",
            adapter.get_info()
        );
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine shared encoder device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// The shared depth attachment, built from the engine's own descriptor so the
/// format, extent, sample count and usage are the ones the frame expects
/// rather than a caller's guess at them.
fn shared_depth(device: &wgpu::Device) -> wgpu::TextureView {
    let texture = device.create_texture(&depth_texture_descriptor(SIZE, SIZE));
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The foreign pass's pipeline, depth-writing under the engine's own
/// comparison so both renderers read one attachment the same way.
fn near_pipeline(device: &wgpu::Device) -> (wgpu::RenderPipeline, PipelineCache) {
    let mut library = ShaderLibrary::new();
    let shader = library.insert_wgsl(device, "shared-encoder-near", WGSL);
    let mut cache = PipelineCache::new(Arc::new(library), None);
    let pipeline = cache
        .get_or_create(
            device,
            &RenderPipelineDesc {
                depth: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(DEPTH_COMPARE),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                ..RenderPipelineDesc::new(shader, "vs_near", "fs_near", FORMAT)
            },
        )
        .clone();
    (pipeline, cache)
}

/// Records the foreign pass into `encoder`, clearing both attachments when it
/// runs first and loading them when the frame has already written them.
fn record_near(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    view: &wgpu::TextureView,
    depth: &wgpu::TextureView,
    first: bool,
) {
    let color_load = if first {
        wgpu::LoadOp::Clear(wgpu::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        })
    } else {
        wgpu::LoadOp::Load
    };
    let depth_load = if first {
        wgpu::LoadOp::Clear(1.0)
    } else {
        wgpu::LoadOp::Load
    };
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("shared encoder foreign depth pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: color_load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load: depth_load,
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.draw(VERTICES, 0..1);
}

/// The one scene every case renders: a single opaque rectangle covering the
/// whole target, so its interior travels through the depth-testing,
/// depth-writing opaque pass.
fn covering_rect() -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    builder.fill_rect(
        Rect::new(0.0, 0.0, f64::from(SIZE), f64::from(SIZE)),
        Brush::Solid(DRAWN),
    );
    scene
}

/// The RGBA bytes at `(x, y)` of a tightly packed read-back.
fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE + x) * 4) as usize;
    pixels[at..at + 4]
        .try_into()
        .expect("a read-back row holds four bytes per pixel")
}

/// Asserts every `(column, row)` of the cross product matches `expected`
/// within [`TOLERANCE`].
fn assert_columns(pixels: &[u8], columns: &[u32], expected: [u8; 4], what: &str) {
    for &x in columns {
        for &y in ROWS {
            let actual = pixel(pixels, x, y);
            let off = (0..4).any(|c| actual[c].abs_diff(expected[c]) > TOLERANCE);
            assert!(
                !off,
                "{what}: pixel ({x}, {y}) is {actual:?}, expected {expected:?} +/- {TOLERANCE}"
            );
        }
    }
}

/// Renders the foreign pass and the frame into one encoder, in the order
/// `frame_last` names, and reads the target back.
fn one_encoder(frame_last: bool) -> Vec<u8> {
    let _guard = render_lock();
    let (device, queue, caps) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let depth = shared_depth(&device);
    let (near, _cache) = near_pipeline(&device);
    let mut renderer = EngineRenderer::new(&device, &caps, FORMAT, None)
        .expect("the engine builds on this device");
    // The whole statement the caller makes about the shared buffer: whoever
    // draws first owns the clear, and the other one loads.
    renderer.set_depth_pre_cleared(frame_last);
    assert_eq!(renderer.depth_pre_cleared(), frame_last);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frust-engine shared encoder frame"),
    });
    if frame_last {
        record_near(&mut encoder, &near, target.view(), &depth, true);
    }
    renderer
        .encode(
            &device,
            &queue,
            &mut encoder,
            &covering_rect(),
            EngineTarget {
                view: target.view(),
                format: FORMAT,
                width: SIZE,
                height: SIZE,
                depth: Some(&depth),
                output: OutputAlpha::Premultiplied,
            },
            BASE,
            Affine::IDENTITY,
        )
        .expect("a well-formed frame encodes");
    if !frame_last {
        // Straight after `encode` returned, on the same encoder: legal only
        // because the frame left no pass open, which is the half of the encode
        // contract a caller cannot check for itself.
        record_near(&mut encoder, &near, target.view(), &depth, false);
    }
    // One submit for the foreign pass and the whole frame together.
    queue.submit([encoder.finish()]);
    renderer.end_frame(&queue);

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the frame raised {error:?}");

    target.read_back(&device, &queue)
}

#[test]
fn the_shared_comparison_is_the_one_every_depth_testing_engine_pipeline_uses() {
    // `DEPTH_COMPARE` is what a renderer sharing the attachment builds its own
    // pipeline against, so it has to keep agreeing with the pipelines the
    // engine actually draws through — a device-free tripwire against the two
    // drifting apart.
    for pipeline in [
        EnginePipeline::StripOpaque,
        EnginePipeline::StripDepthAlpha,
        EnginePipeline::StripDepthDestOut,
    ] {
        let state = pipeline
            .depth()
            .expect("a depth-testing variant carries depth state");
        assert_eq!(state.format, DEPTH_FORMAT, "{pipeline:?}");
        assert_eq!(state.depth_compare, Some(DEPTH_COMPARE), "{pipeline:?}");
    }
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with \
            `cargo test -p frust-engine --test shared_encoder -- --ignored` \
            (pin the adapter on a multi-GPU host with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn a_frame_is_occluded_where_an_earlier_pass_in_the_same_encoder_wrote_nearer_depth() {
    let pixels = one_encoder(true);

    // Left: the foreign pass wrote 0.3, so the frame's draw fails the
    // comparison there and the frame's own clear is all that reaches the
    // target — the foreign pass's colour is gone, its depth is not.
    assert_columns(
        &pixels,
        LEFT,
        BASE_PIXEL,
        "the frame's draw must be rejected where nearer depth was already written",
    );
    // Right: the depth the foreign pass left untouched is still the far plane,
    // so the frame's draw passes.
    assert_columns(
        &pixels,
        RIGHT,
        DRAWN_PIXEL,
        "the frame's draw must reach the half the foreign pass left alone",
    );
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with \
            `cargo test -p frust-engine --test shared_encoder -- --ignored` \
            (pin the adapter on a multi-GPU host with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn a_pass_after_the_frame_draws_over_it_where_it_is_nearer() {
    let pixels = one_encoder(false);

    // The same two draws in the opposite order: the frame owned the clear and
    // established its own depth at the back of its range, and 0.3 passes
    // against that.
    assert_columns(
        &pixels,
        LEFT,
        NEAR_PIXEL,
        "a nearer pass recorded after the frame must take the pixel",
    );
    assert_columns(
        &pixels,
        RIGHT,
        DRAWN_PIXEL,
        "the half that pass does not cover must keep the frame's draw",
    );
}
