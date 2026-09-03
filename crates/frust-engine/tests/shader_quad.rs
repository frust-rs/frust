//! `Command::ShaderQuad` on real hardware: a user-supplied WGSL fragment
//! program compiled at runtime, rendered into a pooled offscreen target, and
//! drawn into the frame that asked for it.
//!
//! Every claim here is one a device-free test cannot make. The pre-pass
//! compiles a shader the crate has never seen, allocates an attachment for it,
//! writes a uniform block, records a fullscreen-triangle pass, and registers the
//! result as a scene texture the frame's own pass then samples — a chain whose
//! failures are a validation error, an untouched target, or the wrong colour,
//! and only a real queue and a read-back tell those apart.
//!
//! What each case pins:
//!
//! - **A solid program fills its destination rectangle, and only that.** The
//!   whole chain in one assertion: compiled, rendered, registered, sampled,
//!   placed.
//! - **The target is sized to the quad's device extent.** The program reports
//!   the `resolution` uniform it was given as a colour, so a target sized from
//!   the wrong rectangle — or a uniform written with the requested rather than
//!   the created extent — fails here rather than merely looking soft.
//! - **A half-alpha program composites premultiplied.** The contract this work
//!   restates: the shader's output is taken as premultiplied and blended over
//!   what is behind it. A target treated as opaque, or one blended as straight
//!   alpha, produces a different number.
//! - **A quad whose program never rendered draws nothing.** A compiler driven
//!   without the pre-pass must leave the frame alone rather than sample
//!   whatever the external binding last held.
//! - **A program that fails to compile draws nothing and does not panic.** The
//!   FFI no-panic invariant, on the one input a user controls completely.
//! - **Two programs in one frame each draw their own pixels.** The strip
//!   pipeline has one external binding, so this holds only if the frame's
//!   instances were split into runs and the binding re-set between them.
//! - **A program the scene stops drawing is unregistered.** The age-based reap
//!   has to withdraw the registration too, or a reaped target stays bound to a
//!   view nothing owns.
//!
//! The whole file runs under a validation error scope, so a bind group built
//! against the wrong layout, or an attachment `wgpu` refuses, names itself
//! rather than showing up as a plausible-looking blank frame.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Mutex, MutexGuard, PoisonError};

use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha, ShaderQuadPass};
use frust_gpu::{HeadlessTarget, TierCaps};
use frust_scene::{Scene, SceneBuilder, ShaderProgram};
use kurbo::{Affine, Rect};
use peniko::Color;
use peniko::color::palette::css::{BLACK, BLUE};

/// Target extent every case renders at.
const SIZE: u32 = 64;

/// The target format. `Rgba8Unorm` reads back R, G, B, A in that order, so a
/// read-back needs no swizzling.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Where every quad is drawn: 32x32, offset from the origin on both axes, so a
/// dropped translation lands it at `(0, 0)` and fails rather than passing by
/// coincidence. Whole device pixels, so nothing here measures rasterizer
/// subpixel coverage.
const DEST: Rect = Rect::new(8.0, 8.0, 40.0, 40.0);

/// [`DEST`]'s device extent — the size the pre-pass must build the target at.
const DEST_EXTENT: f32 = 32.0;

/// The read-back bytes of the fully saturated green a shader writes as
/// `vec4(0.0, 1.0, 0.0, 1.0)`. Spelled out rather than taken from a named
/// palette colour, because the CSS `green` is half-intensity `#008000` and
/// would quietly compare a shader's output against the wrong number.
const SHADER_GREEN: [u8; 4] = [0, 255, 0, 255];

/// The read-back bytes of the fully saturated red a shader writes as
/// `vec4(1.0, 0.0, 0.0, 1.0)`.
const SHADER_RED: [u8; 4] = [255, 0, 0, 255];

/// Serializes every test in this binary that creates a GPU device: these tests
/// each build their own `wgpu::Device`, and a driver that serializes device
/// teardown on a process-global mutex deadlocks when two of them tear down at
/// once. Poison is ignored deliberately — one test's failure must not cascade
/// into every sibling.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A fragment source returning one constant premultiplied colour.
fn solid(r: f32, g: f32, b: f32, a: f32) -> String {
    format!(
        "@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {{ \
         return vec4<f32>({r:?}, {g:?}, {b:?}, {a:?}); }}"
    )
}

/// A fragment source answering green when the `resolution` uniform it was
/// handed is exactly `expected` on both axes, and red otherwise — the target's
/// own extent, reported as a colour a read-back can assert on.
fn reports_resolution(expected: f32) -> String {
    format!(
        "@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {{ \
         let want = vec2<f32>({expected:?}, {expected:?}); \
         if (all(frust_u.resolution == want)) {{ return vec4<f32>(0.0, 1.0, 0.0, 1.0); }} \
         return vec4<f32>(1.0, 0.0, 0.0, 1.0); }}"
    )
}

/// A fragment source encoding the `resolution` uniform it was handed
/// directly into its output colour (`resolution.x/255`, `resolution.y/255`)
/// rather than comparing it against a value baked into the WGSL source at
/// compile time. This lets one compiled program (one program id, one
/// pipeline) be reused across a resize — a read-back recovers the exact
/// extent the shader saw at each size from the pixel bytes themselves,
/// proving a shared (quantized) target renders the correct sub-rect after a
/// resize rather than being sized, or sampled, from the target's own true
/// (larger, quantized) extent.
fn reports_resolution_as_color() -> String {
    "@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> { \
     return vec4<f32>(frust_u.resolution.x / 255.0, frust_u.resolution.y / 255.0, 0.0, 1.0); }"
        .to_string()
}

/// A fragment source answering green when the `resolution` uniform's own
/// aspect ratio (`resolution.x / resolution.y`) is within `tolerance` of
/// `expected`, red otherwise — a device-verified proxy for "the target was
/// sized aspect-true from `dest`'s own dimensions, not the rotated
/// axis-aligned bounding box": a 40x20 `dest` (aspect 2.0) rotated 45 degrees
/// has a ~42.4x42.4 bbox (aspect ~1.0), so a target sized from the bbox
/// instead of `dest` would report the wrong aspect here. A direct
/// circle-stays-a-circle read-back would prove the same fact but is far more
/// sensitive to edge anti-aliasing and rotated-sampling error than an aspect
/// comparison is.
fn reports_aspect(expected: f32) -> String {
    format!(
        "@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {{ \
         let aspect = frust_u.resolution.x / frust_u.resolution.y; \
         if (abs(aspect - {expected:?}) < 0.05) {{ return vec4<f32>(0.0, 1.0, 0.0, 1.0); }} \
         return vec4<f32>(1.0, 0.0, 0.0, 1.0); }}"
    )
}

/// Blocks on `future` by polling it to completion.
///
/// wgpu's native adapter and device requests resolve without an executor
/// driving them, so a bare poll loop is enough; this crate has no async runtime
/// of its own.
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
        println!("frust-engine shader quad adapter: {:?}", adapter.get_info());
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine shader quad device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// One case's device, renderer, shader pre-pass and read-back target.
struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: EngineRenderer,
    shader_quads: ShaderQuadPass,
    target: HeadlessTarget,
}

impl Harness {
    fn new() -> Self {
        let (device, queue, caps) = gpu();
        let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
        let renderer = EngineRenderer::new(&device, &caps, FORMAT, None)
            .expect("the engine builds on this device");

        Self {
            device,
            queue,
            renderer,
            shader_quads: ShaderQuadPass::new(None),
            target,
        }
    }

    /// Renders `scene` over `base`, exactly as a surface does: the frame's
    /// shader quads into the frame's own encoder first, then the frame itself,
    /// then one submit.
    fn render_over(&mut self, scene: &Scene, base: Color) -> Vec<u8> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-engine shader quad frame"),
            });
        self.shader_quads.prepare(
            &self.device,
            &self.queue,
            &mut encoder,
            scene,
            Affine::IDENTITY,
            &mut self.renderer,
        );
        self.encode_into(&mut encoder, scene, base);
        self.queue.submit([encoder.finish()]);
        self.renderer.end_frame(&self.queue);

        let error = drain_error_scope(&self.device, scope);
        assert!(error.is_none(), "the frame raised {error:?}");

        self.target.read_back(&self.device, &self.queue)
    }

    /// Renders `scene` over black WITHOUT the pre-pass — a host driving the
    /// engine directly, which is what leaves a quad's program unrendered.
    fn render_without_prepass(&mut self, scene: &Scene) -> Vec<u8> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-engine shader quad frame, no pre-pass"),
            });
        self.encode_into(&mut encoder, scene, BLACK);
        self.queue.submit([encoder.finish()]);
        self.renderer.end_frame(&self.queue);

        self.target.read_back(&self.device, &self.queue)
    }

    fn encode_into(&mut self, encoder: &mut wgpu::CommandEncoder, scene: &Scene, base: Color) {
        self.renderer
            .encode(
                &self.device,
                &self.queue,
                encoder,
                scene,
                EngineTarget {
                    view: self.target.view(),
                    format: FORMAT,
                    width: SIZE,
                    height: SIZE,
                    depth: None,
                    output: OutputAlpha::Premultiplied,
                },
                base,
                Affine::IDENTITY,
            )
            .expect("a well-formed frame encodes");
    }

    /// Drives `count` frames of `scene` through the pre-pass alone — no encode,
    /// no submit. The aging clock a program is reaped against ticks once per
    /// `prepare`, so this is how a scene that stopped drawing a program is
    /// fast-forwarded past the reap window.
    fn age(&mut self, scene: &Scene, count: usize) {
        for _ in 0..count {
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("frust-engine shader quad aging"),
                });
            self.shader_quads.prepare(
                &self.device,
                &self.queue,
                &mut encoder,
                scene,
                Affine::IDENTITY,
                &mut self.renderer,
            );
            self.queue.submit([encoder.finish()]);
        }
    }
}

/// A scene built by `record`, ready to render.
fn scene_of(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    record(&mut builder);
    scene
}

/// A scene drawing `program` over `dest` at time zero.
fn scene_drawing(program: &ShaderProgram, dest: Rect) -> Scene {
    scene_of(|builder| builder.draw_shader(program, dest, 0.0))
}

/// The RGBA the read-back holds at device pixel `(x, y)`.
fn pixel(frame: &[u8], x: u32, y: u32) -> [u8; 4] {
    let start = ((y * SIZE + x) * 4) as usize;
    frame[start..start + 4]
        .try_into()
        .expect("a read-back pixel is four bytes")
}

fn rgba(color: Color) -> [u8; 4] {
    color.to_rgba8().to_u8_array()
}

/// Asserts every pixel inside [`DEST`] is `expected`, sampled away from the
/// edges so nothing here depends on edge filtering.
fn assert_dest_is(frame: &[u8], expected: [u8; 4]) {
    for probe in [(12, 12), (20, 20), (36, 36), (12, 36), (36, 12)] {
        assert_eq!(
            pixel(frame, probe.0, probe.1),
            expected,
            "the destination rectangle at {probe:?}"
        );
    }
}

/// Asserts nothing outside [`DEST`] was painted over `base`.
fn assert_only_dest_painted(frame: &[u8], base: Color) {
    for probe in [(1, 1), (SIZE - 2, 1), (1, SIZE - 2), (SIZE - 2, SIZE - 2)] {
        assert_eq!(
            pixel(frame, probe.0, probe.1),
            rgba(base),
            "the frame outside the destination rectangle is the base colour at {probe:?}"
        );
    }
}

/// Asserts `frame` holds nothing but `base`.
fn assert_frame_is_base(frame: &[u8], base: Color) {
    assert!(
        frame.chunks_exact(4).all(|texel| texel == rgba(base)),
        "the frame is the base colour everywhere"
    );
}

/// Asserts each channel is within `tolerance` of `expected` — the allowance for
/// one 8-bit rounding step in the target and one in the blend.
fn assert_close(actual: [u8; 4], expected: [u8; 4], tolerance: u8, at: &str) {
    let close = actual
        .iter()
        .zip(expected.iter())
        .all(|(a, e)| a.abs_diff(*e) <= tolerance);
    assert!(close, "{at}: got {actual:?}, expected ~{expected:?}");
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test shader_quad -- --ignored`"]
fn a_solid_program_fills_its_destination_rectangle() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    let program = ShaderProgram::new(solid(0.0, 1.0, 0.0, 1.0));

    let frame = harness.render_over(&scene_drawing(&program, DEST), BLACK);

    assert_dest_is(&frame, SHADER_GREEN);
    assert_only_dest_painted(&frame, BLACK);
    assert_eq!(
        harness.shader_quads.registered_len(),
        1,
        "the program's target stays registered for the next frame"
    );
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test shader_quad -- --ignored`"]
fn a_targets_resolution_uniform_is_the_quads_device_extent() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    // Green only if the uniform the shader read is exactly the destination's
    // device extent — the target was sized from the quad, and the uniform was
    // written with the extent the texture was actually created at.
    let program = ShaderProgram::new(reports_resolution(DEST_EXTENT));

    let frame = harness.render_over(&scene_drawing(&program, DEST), BLACK);

    assert_dest_is(&frame, SHADER_GREEN);
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test shader_quad -- --ignored`"]
fn a_half_alpha_program_composites_premultiplied_over_the_base() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    // Red at half alpha, already premultiplied: rgb 0.5 with alpha 0.5 is the
    // premultiplied form of opaque red drawn at 50%.
    let program = ShaderProgram::new(solid(0.5, 0.0, 0.0, 0.5));

    let frame = harness.render_over(&scene_drawing(&program, DEST), BLUE);

    // Source-over with a premultiplied source: `src.rgb + (1 - src.a) * dst`.
    // 0.5 red over opaque blue is (128, 0, 127) — a target treated as opaque
    // would read (128, 0, 0), and one blended as straight alpha (64, 0, 127).
    for probe in [(12, 12), (20, 20), (36, 36)] {
        assert_close(
            pixel(&frame, probe.0, probe.1),
            [128, 0, 127, 255],
            3,
            &format!("the composited quad at {probe:?}"),
        );
    }
    assert_only_dest_painted(&frame, BLUE);
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test shader_quad -- --ignored`"]
fn a_fully_transparent_program_leaves_the_frame_alone() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    // The premultiplied identity of "nothing": zero colour, zero alpha. Under
    // the retired opaque-only reading this would have punched a black square.
    let program = ShaderProgram::new(solid(0.0, 0.0, 0.0, 0.0));

    let frame = harness.render_over(&scene_drawing(&program, DEST), BLUE);

    assert_frame_is_base(&frame, BLUE);
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test shader_quad -- --ignored`"]
fn a_quad_whose_program_never_rendered_draws_nothing() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    let program = ShaderProgram::new(solid(0.0, 1.0, 0.0, 1.0));

    let frame = harness.render_without_prepass(&scene_drawing(&program, DEST));

    assert_frame_is_base(&frame, BLACK);
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test shader_quad -- --ignored`"]
fn a_program_that_fails_to_compile_draws_nothing() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    // Not WGSL at all: the compile is caught in an error scope one layer down
    // and recorded as failed, so nothing here may panic and no target exists
    // to register.
    let program = ShaderProgram::new("this is not a shader");
    let scene = scene_drawing(&program, DEST);

    // The pre-pass runs under this harness's own error scope, so the recorded
    // compile failure must not escape as an uncaptured device error either.
    let frame = harness.render_over(&scene, BLACK);

    assert_frame_is_base(&frame, BLACK);
    assert_eq!(
        harness.shader_quads.registered_len(),
        0,
        "a program with no pipeline registers no target"
    );
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test shader_quad -- --ignored`"]
fn two_programs_in_one_frame_each_draw_their_own_pixels() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    let first = ShaderProgram::new(solid(1.0, 0.0, 0.0, 1.0));
    let second = ShaderProgram::new(solid(0.0, 1.0, 0.0, 1.0));

    // Side by side, so one binding leaking into the other run shows up as the
    // wrong colour rather than as nothing at all.
    let left = Rect::new(0.0, 16.0, 32.0, 48.0);
    let right = Rect::new(32.0, 16.0, 64.0, 48.0);
    let scene = scene_of(|builder| {
        builder.draw_shader(&first, left, 0.0);
        builder.draw_shader(&second, right, 0.0);
    });

    let frame = harness.render_over(&scene, BLACK);

    assert_eq!(harness.shader_quads.registered_len(), 2);
    assert_eq!(
        pixel(&frame, 8, 32),
        SHADER_RED,
        "the left rectangle draws the first program"
    );
    assert_eq!(
        pixel(&frame, 40, 32),
        SHADER_GREEN,
        "the right rectangle draws the second program"
    );
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test shader_quad -- --ignored`"]
fn one_program_drawn_twice_in_a_frame_renders_once_and_draws_both_quads() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    let program = ShaderProgram::new(solid(0.0, 1.0, 0.0, 1.0));
    let left = Rect::new(0.0, 16.0, 32.0, 48.0);
    let right = Rect::new(32.0, 16.0, 64.0, 48.0);
    let scene = scene_of(|builder| {
        builder.draw_shader(&program, left, 0.0);
        builder.draw_shader(&program, right, 0.0);
    });

    let frame = harness.render_over(&scene, BLACK);

    assert_eq!(
        harness.shader_quads.registered_len(),
        1,
        "both quads share one target"
    );
    assert_eq!(pixel(&frame, 8, 32), SHADER_GREEN);
    assert_eq!(pixel(&frame, 40, 32), SHADER_GREEN);
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test shader_quad -- --ignored`"]
fn a_program_the_scene_stops_drawing_is_reaped_and_unregistered() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    let program = ShaderProgram::new(solid(0.0, 1.0, 0.0, 1.0));

    let frame = harness.render_over(&scene_drawing(&program, DEST), BLACK);
    assert_dest_is(&frame, SHADER_GREEN);
    assert_eq!(harness.shader_quads.registered_len(), 1);

    // Frames drawing nothing at all: the reap clock ticks on every one of
    // them, which is what ages a vanished program out.
    harness.age(
        &Scene::new(),
        frust_gpu::effects::MAX_UNSEEN_FRAMES as usize + 1,
    );

    assert_eq!(
        harness.shader_quads.registered_len(),
        0,
        "a reaped program's registration is withdrawn with its target"
    );
    assert_eq!(
        harness.renderer.bound_texture_count(),
        0,
        "the renderer holds no view for a reaped program"
    );
    // And the same scene now draws nothing rather than a stale target.
    let after = harness.render_without_prepass(&scene_drawing(&program, DEST));
    assert_frame_is_base(&after, BLACK);
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test shader_quad -- --ignored`"]
fn a_resized_quad_reads_back_the_correct_exact_size_from_the_shared_quantized_target() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    let program = ShaderProgram::new(reports_resolution_as_color());

    let first = harness.render_over(&scene_drawing(&program, DEST), BLACK);
    let before = pixel(&first, 20, 20);
    assert_eq!(before[0], 32, "the shader reads back the exact 32px width");
    assert_eq!(before[1], 32, "the shader reads back the exact 32px height");
    assert_eq!(harness.shader_quads.registered_len(), 1);

    // Resized within the same 256x256 quantized target (see
    // `frust_gpu::effects::quantized_target_key`): the same pooled texture is
    // reused underneath, but the pass renders — and the shader reads back —
    // the new exact device size, 48x48, never the old 32x32 or the target's
    // true (quantized) extent.
    let resized_dest = Rect::new(8.0, 8.0, 56.0, 56.0);
    let second = harness.render_over(&scene_drawing(&program, resized_dest), BLACK);
    let after = pixel(&second, 30, 30);
    assert_eq!(
        after[0], 48,
        "after the resize the shader reads back 48px width"
    );
    assert_eq!(
        after[1], 48,
        "after the resize the shader reads back 48px height"
    );
    assert_eq!(
        harness.shader_quads.registered_len(),
        1,
        "still one registration: the same program, a new exact demanded size"
    );
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test shader_quad -- --ignored`"]
fn a_rotated_quad_sizes_its_target_from_dest_not_the_rotated_bbox() {
    let _guard = render_lock();
    let mut harness = Harness::new();
    // A 40x20 dest (aspect 2.0), recorded rotated 45 degrees around its own
    // centre so it stays inside the 64x64 frame: the axis-aligned bbox of a
    // 40x20 rectangle rotated 45 degrees is ~42.4x42.4 (aspect ~1.0), which
    // is what an un-fixed bbox-based sizing would report here instead.
    let program = ShaderProgram::new(reports_aspect(2.0));
    let local_dest = Rect::new(-20.0, -10.0, 20.0, 10.0);
    let scene = scene_of(|builder| {
        builder.push_transform(
            Affine::translate((32.0, 32.0)) * Affine::rotate(std::f64::consts::FRAC_PI_4),
        );
        builder.draw_shader(&program, local_dest, 0.0);
        builder.pop_transform();
    });

    let frame = harness.render_over(&scene, BLACK);

    // Probe near the centre of the rotated quad, away from its edges (which
    // an aspect check does not depend on rotated-sampling precision at all —
    // every rendered texel already carries the same colour).
    assert_eq!(pixel(&frame, 32, 32), SHADER_GREEN);
}
