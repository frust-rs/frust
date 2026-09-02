//! Two independent render passes, one borrowed encoder, one depth attachment:
//! the substrate half of the seam a 3D renderer and the 2D strip renderer meet
//! at.
//!
//! [`frust_gpu::CommandBuffer`]'s invariant already says a caller may record
//! its own passes before and after the ones it hands the encoder to, and
//! submit once. What it cannot say on its own is that the *depth* those passes
//! share behaves: that the second pass is rejected where the first wrote nearer
//! depth, that the reverse ordering wins the pixel back, and that one pass's
//! `StoreOp::Store` really is what the next pass's `LoadOp::Load` reads inside
//! a single command buffer. None of that produces a wrong *value* when it is
//! wrong — it produces a validation error, an untouched attachment, or a
//! plausible-looking image with the wrong half painted — so it takes a real
//! queue and a real read-back to tell those apart.
//!
//! Everything here is deliberately `frust-gpu` and `wgpu` only. The point of
//! the exercise is that a renderer this crate knows nothing about — a future 3D
//! crate — can drive the seam with a pipeline of its own, so reaching for the
//! strip renderer to prove it would prove something narrower. The engine half
//! of the same proof lives beside the other engine integration tests.
//!
//! The WGSL below is an inline test fixture standing in for that unknown
//! renderer's own program, which is why it is not a file under the engine's
//! `shaders/` directory: the downlevel design-rule scan governs the shaders
//! the engine ships, and a stand-in for somebody else's 3D pass is not one of
//! them (the same reasoning `frust_gpu::pipeline`'s own real-pipeline case
//! already follows).

use std::sync::Arc;

use frust_gpu::{
    ColorAttachment, CommandBuffer, DepthAttachment, HeadlessTarget, PipelineCache,
    RenderPipelineDesc, RenderTarget, ShaderLibrary,
};

/// Target extent every case renders at. Both axes are a multiple of two so the
/// half-way split lands on a texel boundary rather than inside one.
const SIZE: u32 = 64;

/// The colour format the read-back is asserted in: `Rgba8Unorm` reads back R,
/// G, B, A in that order, so no swizzling stands between a pixel and its
/// assertion.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The depth format the two passes share — the one the 2D renderer's own
/// attachment uses, so a caller allocating this buffer for both is allocating
/// exactly what the engine would have.
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

/// The far plane: the value a depth attachment is cleared to, and the value
/// every fragment therefore passes against on a freshly cleared buffer.
const FAR_PLANE: f32 = 1.0;

/// The colour neither pass writes, cleared once by whichever pass runs first.
const BACKDROP: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

/// Opaque red — the near pass's fragment colour, matching `fs_near`.
const NEAR_PIXEL: [u8; 4] = [255, 0, 0, 255];

/// Opaque green — the far pass's fragment colour, matching `fs_far`.
const FAR_PIXEL: [u8; 4] = [0, 255, 0, 255];

/// Sample columns strictly inside the near pass's left half, clear of the
/// half-way boundary at x = 32.
const LEFT: &[u32] = &[4, 16, 28];

/// Sample columns strictly outside it, clear of the same boundary.
const RIGHT: &[u32] = &[36, 48, 60];

/// Sample rows, spread over the target's full height — both passes cover
/// every row, so a row-stride slip shows up as a mismatch rather than as a
/// convincing image.
const ROWS: &[u32] = &[8, 32, 56];

/// The stand-in 3D renderer's program: two depth-writing draws with no
/// bindings at all, so the case is about the shared attachment and nothing
/// else.
///
/// `vs_near` covers the target's left half at a depth well in front of the far
/// plane; `vs_far` covers the whole target just behind it. Both generate their
/// own vertices from the vertex index, so neither needs a vertex buffer, a
/// uniform, or a bind group.
const WGSL: &str = r#"
const NEAR_Z: f32 = 0.3;
const FAR_Z: f32 = 0.9;

fn left_half(index: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 0.0, -1.0),
        vec2<f32>( 0.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 0.0,  1.0),
        vec2<f32>(-1.0,  1.0)
    );
    return corners[index];
}

fn whole_target(index: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0,  1.0)
    );
    return corners[index];
}

@vertex
fn vs_near(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(left_half(index), NEAR_Z, 1.0);
}

@vertex
fn vs_far(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(whole_target(index), FAR_Z, 1.0);
}

@fragment
fn fs_near() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 0.0, 1.0);
}

@fragment
fn fs_far() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
"#;

/// How many vertices each draw expands to: two triangles, no index buffer.
const VERTICES: std::ops::Range<u32> = 0..6;

/// Blocks on `future` by polling it to completion.
///
/// wgpu's native adapter and device requests resolve without an executor
/// driving them, so a bare poll loop is enough.
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

/// A device and queue off the adapter the environment selects.
fn gpu() -> (wgpu::Device, wgpu::Queue) {
    block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        // The environment-aware initializer, so `WGPU_ADAPTER_NAME` picks the
        // GPU on a multi-adapter host instead of the run silently landing on
        // whichever one enumerates first.
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        println!("frust-gpu shared encoder adapter: {:?}", adapter.get_info());
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-gpu shared encoder device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("failed to create the device")
    })
}

/// A `width` x `height` depth attachment, render-attachment only — nothing
/// samples or copies it.
fn depth_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust-gpu shared encoder depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The two depth-writing pipelines, near first, built through the crate's own
/// cache so the desc really is what a caller would hand it.
fn pipelines(device: &wgpu::Device) -> (wgpu::RenderPipeline, wgpu::RenderPipeline, PipelineCache) {
    let mut library = ShaderLibrary::new();
    let shader = library.insert_wgsl(device, "shared-encoder-depth", WGSL);
    let mut cache = PipelineCache::new(Arc::new(library), None);

    // Nearer geometry carries the smaller z, so the comparison is `LessEqual`
    // — the convention the 2D renderer above this crate encodes its painter
    // order in, and the one a pass sharing its attachment has to agree with.
    let depth = wgpu::DepthStencilState {
        format: DEPTH,
        depth_write_enabled: Some(true),
        depth_compare: Some(wgpu::CompareFunction::LessEqual),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    };
    let near = cache
        .get_or_create(
            device,
            &RenderPipelineDesc {
                depth: Some(depth.clone()),
                ..RenderPipelineDesc::new(shader, "vs_near", "fs_near", FORMAT)
            },
        )
        .clone();
    let far = cache
        .get_or_create(
            device,
            &RenderPipelineDesc {
                depth: Some(depth),
                ..RenderPipelineDesc::new(shader, "vs_far", "fs_far", FORMAT)
            },
        )
        .clone();
    (near, far, cache)
}

/// Records `passes` in order through ONE [`CommandBuffer`] against one colour
/// target and one depth view, submits once, and reads the target back.
///
/// The first pass owns both clears — colour to [`BACKDROP`], depth to
/// [`FAR_PLANE`] — and every pass after it loads what the previous one stored.
/// That split is the depth-clear-ownership rule the seam's contract states,
/// written out as the only thing that varies between the two orderings below.
fn render_through_one_encoder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &HeadlessTarget,
    depth: &wgpu::TextureView,
    passes: [(&str, &wgpu::RenderPipeline); 2],
) -> Vec<u8> {
    let mut buffer = CommandBuffer::new(device, Some("frust-gpu shared encoder"));
    for (index, (label, pipeline)) in passes.iter().enumerate() {
        let first = index == 0;
        let render_target = RenderTarget {
            color: vec![ColorAttachment {
                view: target.view(),
                load: if first {
                    wgpu::LoadOp::Clear(BACKDROP)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            }],
            depth: Some(DepthAttachment {
                view: depth,
                load: if first {
                    wgpu::LoadOp::Clear(FAR_PLANE)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            }),
        };
        let mut pass = buffer.render_pass(&render_target, Some(label));
        pass.set_pipeline(pipeline);
        pass.draw(VERTICES, 0..1);
    }
    // One submit for both passes — the whole point of handing an encoder over
    // rather than letting each renderer submit its own.
    queue.submit([buffer.finish()]);
    target.read_back(device, queue)
}

/// The RGBA bytes at `(x, y)` of a tightly packed read-back.
fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE + x) * 4) as usize;
    pixels[at..at + 4]
        .try_into()
        .expect("a read-back row holds four bytes per pixel")
}

/// Asserts every `(column, row)` of the cross product reads exactly `expected`.
fn assert_columns(pixels: &[u8], columns: &[u32], expected: [u8; 4], what: &str) {
    for &x in columns {
        for &y in ROWS {
            assert_eq!(
                pixel(pixels, x, y),
                expected,
                "{what}: pixel ({x}, {y}) is wrong"
            );
        }
    }
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with \
            `cargo test -p frust-gpu --test shared_encoder -- --ignored` \
            (pin the adapter on a multi-GPU host with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn a_pass_is_rejected_where_an_earlier_pass_in_the_same_encoder_wrote_nearer_depth() {
    let (device, queue) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let depth = depth_texture(&device, SIZE, SIZE);
    let (near, far, _cache) = pipelines(&device);

    let pixels = render_through_one_encoder(
        &device,
        &queue,
        &target,
        &depth,
        [("near half", &near), ("whole target", &far)],
    );

    // Where the near pass wrote 0.3, the later whole-target draw at 0.9 fails
    // the comparison and never reaches the colour attachment.
    assert_columns(&pixels, LEFT, NEAR_PIXEL, "the near half must survive");
    assert_columns(
        &pixels,
        RIGHT,
        FAR_PIXEL,
        "the far draw must reach the untouched half",
    );

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the frame raised {error:?}");
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with \
            `cargo test -p frust-gpu --test shared_encoder -- --ignored` \
            (pin the adapter on a multi-GPU host with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn a_pass_drawing_nearer_than_what_the_encoder_already_holds_takes_the_pixel_back() {
    let (device, queue) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let depth = depth_texture(&device, SIZE, SIZE);
    let (near, far, _cache) = pipelines(&device);

    let pixels = render_through_one_encoder(
        &device,
        &queue,
        &target,
        &depth,
        [("whole target", &far), ("near half", &near)],
    );

    // Same two draws, opposite order: 0.3 passes against the 0.9 already
    // stored, so the near half overwrites what the first pass left there.
    assert_columns(&pixels, LEFT, NEAR_PIXEL, "the near half must win");
    assert_columns(
        &pixels,
        RIGHT,
        FAR_PIXEL,
        "the half the near pass does not cover must keep the far draw",
    );

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the frame raised {error:?}");
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with \
            `cargo test -p frust-gpu --test shared_encoder -- --ignored` \
            (pin the adapter on a multi-GPU host with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn a_depth_view_whose_extent_differs_from_the_colour_target_is_refused() {
    let (device, queue) = gpu();

    let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let depth = depth_texture(&device, SIZE / 2, SIZE / 2);
    let (near, _far, _cache) = pipelines(&device);

    // Its own scope, and nothing submitted: the pass below is expected to be
    // rejected, and a command buffer carrying a rejected pass is not something
    // to hand the queue.
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut buffer = CommandBuffer::new(&device, Some("frust-gpu mismatched depth"));
    {
        let render_target = RenderTarget {
            color: vec![ColorAttachment {
                view: target.view(),
                load: wgpu::LoadOp::Clear(BACKDROP),
                store: wgpu::StoreOp::Store,
            }],
            depth: Some(DepthAttachment {
                view: &depth,
                load: wgpu::LoadOp::Clear(FAR_PLANE),
                store: wgpu::StoreOp::Store,
            }),
        };
        let mut pass = buffer.render_pass(&render_target, Some("mismatched extents"));
        pass.set_pipeline(&near);
        pass.draw(VERTICES, 0..1);
    }
    // Finishing is what surfaces a rejected pass; the buffer it produces is
    // deliberately dropped rather than submitted.
    drop(buffer.finish());
    let error = drain_error_scope(&device, scope);

    // The extent-equality rule is wgpu's, not this crate's, which is exactly
    // why it is pinned here: a caller sharing one depth buffer between two
    // renderers has to size it against the colour target, and getting it wrong
    // is a hard refusal rather than a subtly wrong image.
    assert!(
        error.is_some(),
        "a depth attachment sized differently from the colour target must be refused"
    );
    drop(queue);
}
