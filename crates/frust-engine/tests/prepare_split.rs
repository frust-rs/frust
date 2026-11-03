//! [`EngineRenderer::prepare`] and [`EngineRenderer::encode_prepared`]: the
//! two halves of [`EngineRenderer::encode`], called apart the way a host that
//! must know a frame is refused *before* it acquires its target calls them.
//!
//! What the split promises, and what each case below pins on real hardware:
//!
//! - **Every shape refusal is the plan's.** A frame the scheduler does not
//!   serve is refused by `prepare`, which takes no encoder and no target —
//!   and an `encode_prepared` that follows anyway re-plans, refuses the same
//!   way, and still leaves the caller's encoder exactly as it was found.
//! - **The halves are the whole.** `prepare` then `encode_prepared` records
//!   what `encode` records: the same passes and the same pixels.
//! - **The last plan wins.** A second `prepare` replaces the first, and the
//!   frame recorded is the second one's.
//! - **An abandoned plan leaves nothing behind.** The retained store is only
//!   committed by the encode, so a prepared frame that is never encoded — one
//!   re-recording a retained body with other content, here — changes no page
//!   and no ledger entry, and a later frame drawing that body draws it as it
//!   was recorded.
//! - **A stale plan is re-made, not misused.** A target of another extent than
//!   the one prepared for is planned again from the scene handed to the
//!   encode, and renders exactly as a plain `encode` at that extent does.
//!
//! The whole file runs under a validation error scope, so a wrong bind group
//! or an out-of-range draw surfaces as a named error rather than as a
//! plausible-looking image.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Mutex, MutexGuard, PoisonError};

use frust_engine::renderer::{FramePass, RegionCopyMode};
use frust_engine::{EngineError, EngineRenderer, EngineTarget, OutputAlpha};
use frust_gpu::{HeadlessTarget, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::color::palette::css::{BLACK, BLUE, GREEN, RED, WHITE};
use peniko::{Brush, Color};

/// Target extent the cases render at unless they say otherwise.
const SIZE: u32 = 256;

/// The target format, chosen for a read-back whose byte order needs no
/// swizzling: `Rgba8Unorm` reads back R, G, B, A in that order.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The colour the refusal case clears its target to itself, after the
/// refused frame — an arbitrary opaque value nothing else here produces.
const AFTER_REFUSAL_CLEAR: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};

/// Serializes every test in this binary that creates a GPU device — the same
/// guard the other GPU suites in this crate take (see `encode_contract.rs`):
/// the NVIDIA Vulkan driver serializes `vkDestroyDevice` against other Vulkan
/// work on a process-global mutex, so only one device is ever alive. Poison
/// is ignored deliberately: one test's failure must not cascade.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Blocks on `future` by polling it to completion; wgpu's native adapter and
/// device requests resolve without an executor driving them.
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
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        println!(
            "frust-engine prepare split adapter: {:?}",
            adapter.get_info()
        );
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine prepare split device"),
                required_features: wgpu::Features::empty(),
                required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// A renderer for `device`, built for [`FORMAT`].
fn renderer(device: &wgpu::Device, caps: &TierCaps) -> EngineRenderer {
    EngineRenderer::new(device, caps, FORMAT, None).expect("the engine builds on this device")
}

/// The engine target over the whole of `target`.
fn engine_target(target: &HeadlessTarget) -> EngineTarget<'_> {
    EngineTarget {
        view: target.view(),
        format: FORMAT,
        width: target.width(),
        height: target.height(),
        depth: None,
        output: OutputAlpha::Premultiplied,
    }
}

/// The texture usage `target` was created with — what a host knows about the
/// texture it will acquire before it acquires it.
fn usage(target: &HeadlessTarget) -> wgpu::TextureUsages {
    target.texture().usage()
}

/// A fresh encoder.
fn encoder(device: &wgpu::Device) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frust-engine prepare split frame"),
    })
}

/// One frame of `scene` into `target` through the plain [`EngineRenderer::encode`],
/// submitted and closed out, answering the passes it recorded.
fn encode_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut EngineRenderer,
    scene: &Scene,
    target: &HeadlessTarget,
) -> Vec<FramePass> {
    let mut encoder = encoder(device);
    renderer
        .encode(
            device,
            queue,
            &mut encoder,
            scene,
            engine_target(target),
            Color::TRANSPARENT,
            Affine::IDENTITY,
        )
        .expect("a well-formed frame encodes");
    queue.submit([encoder.finish()]);
    renderer.end_frame(queue);
    renderer.last_frame_passes().to_vec()
}

/// One frame of `scene` into `target` through [`EngineRenderer::encode_prepared`]
/// alone — whatever the renderer holds prepared is what it is handed —
/// submitted and closed out, answering the passes it recorded.
fn encode_prepared_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut EngineRenderer,
    scene: &Scene,
    target: &HeadlessTarget,
) -> Vec<FramePass> {
    let mut encoder = encoder(device);
    renderer
        .encode_prepared(
            device,
            queue,
            &mut encoder,
            scene,
            engine_target(target),
            Color::TRANSPARENT,
            Affine::IDENTITY,
        )
        .expect("a well-formed prepared frame encodes");
    queue.submit([encoder.finish()]);
    renderer.end_frame(queue);
    renderer.last_frame_passes().to_vec()
}

/// The RGBA bytes of the pixel at `(x, y)` in a tightly packed read-back of a
/// `width`-wide target.
fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * width + x) * 4) as usize;
    pixels[index..index + 4]
        .try_into()
        .expect("a read-back row holds four bytes per pixel")
}

/// A target-filling opaque backdrop.
fn backdrop(builder: &mut SceneBuilder<'_>, width: u32, height: u32) {
    builder.fill_rect(
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        Brush::Solid(BLACK),
    );
}

/// The fan-inside-a-fan the scheduler refuses
/// (`tests/schedule.rs`'s `a_sibling_pair_nested_inside_a_sibling_pair_escalates`),
/// recorded through the scene builder: an outer translucent layer holding a
/// flat translucent child and a translucent child carrying two translucent
/// children of its own. The inner parent's round has to composite both of its
/// children while its own parent's page and theirs are live — a fourth page.
fn nested_fan_inside_a_fan() -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    backdrop(&mut builder, SIZE, SIZE);
    let outer = Rect::new(0.0, 0.0, 224.0, 192.0);
    builder.push_layer(outer, 0.5);
    {
        let flat = Rect::new(16.0, 16.0, 64.0, 64.0);
        builder.push_layer(flat, 0.5);
        builder.fill_rect(flat, Brush::Solid(RED));
        builder.pop_layer();

        let parent = Rect::new(80.0, 16.0, 208.0, 176.0);
        builder.push_layer(parent, 0.5);
        {
            let first = Rect::new(96.0, 32.0, 144.0, 96.0);
            builder.push_layer(first, 0.5);
            builder.fill_rect(first, Brush::Solid(GREEN));
            builder.pop_layer();

            let second = Rect::new(144.0, 96.0, 192.0, 160.0);
            builder.push_layer(second, 0.5);
            builder.fill_rect(second, Brush::Solid(BLUE));
            builder.pop_layer();
        }
        builder.pop_layer();
    }
    builder.pop_layer();
    scene
}

/// A representative layered frame: an opaque backdrop, a translucent layer
/// holding a nested translucent layer, a sibling translucent layer beside it,
/// and a retained body — page rounds, composites and a retained page in one
/// frame.
fn layered_scene() -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    backdrop(&mut builder, SIZE, SIZE);
    let outer = Rect::new(16.0, 16.0, 144.0, 144.0);
    builder.push_layer(outer, 0.75);
    builder.fill_rect(outer, Brush::Solid(WHITE));
    let inner = Rect::new(48.0, 48.0, 112.0, 112.0);
    builder.push_layer(inner, 0.5);
    builder.fill_rect(inner, Brush::Solid(RED));
    builder.pop_layer();
    builder.pop_layer();
    let sibling = Rect::new(160.0, 16.0, 240.0, 112.0);
    builder.push_layer(sibling, 0.5);
    builder.fill_rect(sibling, Brush::Solid(GREEN));
    builder.pop_layer();
    let body = Rect::new(32.0, 160.0, 224.0, 240.0);
    builder.push_retained(9, body, 0.6, 1.0);
    builder.fill_rect(body, Brush::Solid(BLUE));
    builder.pop_retained();
    scene
}

/// A frame painting one rectangle in `color` inside a translucent layer.
fn tinted_scene(color: Color) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    backdrop(&mut builder, SIZE, SIZE);
    let rect = Rect::new(64.0, 64.0, 192.0, 192.0);
    builder.push_layer(rect, 0.5);
    builder.fill_rect(rect, Brush::Solid(color));
    builder.pop_layer();
    scene
}

/// The rectangle the retained-store case records its body into.
const RETAINED_RECT: Rect = Rect::new(32.0, 32.0, 160.0, 160.0);

/// The key the retained-store case records its body under.
const RETAINED_KEY: u64 = 5;

/// A frame recording the retained body under [`RETAINED_KEY`], filled `color`.
fn record_retained(color: Color) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    backdrop(&mut builder, SIZE, SIZE);
    builder.push_retained(RETAINED_KEY, RETAINED_RECT, 1.0, 1.0);
    builder.fill_rect(RETAINED_RECT, Brush::Solid(color));
    builder.pop_retained();
    scene
}

/// A frame compositing the retained body from its page at 0.75.
fn draw_retained() -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    backdrop(&mut builder, SIZE, SIZE);
    builder.draw_retained(RETAINED_KEY, RETAINED_RECT, 0.75, 1.0);
    scene
}

#[test]
fn of_usage_is_the_single_sampled_answer_for_every_usage_and_format() {
    use wgpu::TextureUsages as U;
    let attach = U::RENDER_ATTACHMENT;
    for usage in [
        attach,
        attach | U::COPY_SRC,
        attach | U::TEXTURE_BINDING,
        attach | U::TEXTURE_BINDING | U::COPY_SRC,
    ] {
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ] {
            assert_eq!(
                RegionCopyMode::of_usage(usage, format),
                RegionCopyMode::for_usage(usage, format, 1),
                "{usage:?} in {format:?}"
            );
        }
    }
    // The swapchain shape this exists for: attachment-only reads nothing back.
    assert_eq!(
        RegionCopyMode::of_usage(attach, wgpu::TextureFormat::Bgra8Unorm),
        RegionCopyMode::Unavailable
    );
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test prepare_split -- --ignored --test-threads=1`"]
fn prepare_refuses_a_sibling_pair_nested_inside_a_sibling_pair_and_leaves_the_encoder_untouched() {
    let _serialized = render_lock();
    let (device, queue, caps) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut renderer = renderer(&device, &caps);
    let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let scene = nested_fan_inside_a_fan();

    // A frame that drew before it, so a refusal that kept the last frame's
    // record would be visible.
    let _ = encode_frame(&device, &queue, &mut renderer, &tinted_scene(RED), &target);
    assert!(!renderer.last_frame_passes().is_empty());
    let ledger_generation = renderer.retained_generation();

    // The encoder exists before the plan, as a host's would — and the plan
    // never sees it.
    let mut encoder = encoder(&device);
    let refused = renderer.prepare(&scene, SIZE, SIZE, usage(&target), Affine::IDENTITY);
    assert!(
        matches!(refused, Err(EngineError::SchedulerEscalation { .. })),
        "prepare must refuse the shape before anything is acquired: {refused:?}"
    );
    assert!(
        renderer.last_frame_passes().is_empty(),
        "a refused plan reports no pass"
    );

    // An encode that follows anyway holds no plan, re-plans, and refuses the
    // same way, recording nothing into the encoder.
    let again = renderer.encode_prepared(
        &device,
        &queue,
        &mut encoder,
        &scene,
        engine_target(&target),
        Color::TRANSPARENT,
        Affine::IDENTITY,
    );
    assert!(
        matches!(again, Err(EngineError::SchedulerEscalation { .. })),
        "the re-plan refuses too: {again:?}"
    );
    assert!(renderer.last_frame_passes().is_empty());
    assert_eq!(
        renderer.retained_generation(),
        ledger_generation,
        "a refused plan changes no ledger entry"
    );

    // The encoder is exactly as found: a pass begun on it now is legal, and
    // its clear is the only thing the target shows once it is submitted.
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("pass after the refused frame"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target.view(),
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(AFTER_REFUSAL_CLEAR),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    queue.submit([encoder.finish()]);
    renderer.end_frame(&queue);
    let pixels = target.read_back(&device, &queue);
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|px| *px == [0, 255, 255, 255]),
        "nothing of the refused frame reached the target"
    );

    // And the renderer is unharmed: the next frame draws.
    let passes = encode_frame(&device, &queue, &mut renderer, &tinted_scene(RED), &target);
    assert!(!passes.is_empty());

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the frames raised {error:?}");
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test prepare_split -- --ignored --test-threads=1`"]
fn prepare_then_encode_prepared_records_exactly_what_encode_records() {
    let _serialized = render_lock();
    let (device, queue, caps) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let scene = layered_scene();

    // Two renderers on one device, each over two frames: the recording frame
    // and the steady one after it, which composites the retained page.
    let mut whole = renderer(&device, &caps);
    let mut split = renderer(&device, &caps);
    let whole_target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let split_target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    for frame in 0..2 {
        let whole_passes = encode_frame(&device, &queue, &mut whole, &scene, &whole_target);
        split
            .prepare(&scene, SIZE, SIZE, usage(&split_target), Affine::IDENTITY)
            .expect("the layered frame plans");
        let split_passes =
            encode_prepared_frame(&device, &queue, &mut split, &scene, &split_target);

        assert!(
            whole_passes.len() > 2,
            "frame {frame}: the scene takes page rounds: {whole_passes:?}"
        );
        assert_eq!(whole_passes, split_passes, "frame {frame}: the same passes");
        let whole_pixels = whole_target.read_back(&device, &queue);
        let split_pixels = split_target.read_back(&device, &queue);
        assert!(
            whole_pixels == split_pixels,
            "frame {frame}: prepare + encode_prepared drew other pixels than encode"
        );
        let whole_keys: Vec<u64> = whole.retained_ledger().map(|entry| entry.key).collect();
        let split_keys: Vec<u64> = split.retained_ledger().map(|entry| entry.key).collect();
        assert_eq!(whole_keys, split_keys, "frame {frame}: the same ledger");
        assert_eq!(split_keys, [9], "frame {frame}: the body is resident");
    }

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the frames raised {error:?}");
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test prepare_split -- --ignored --test-threads=1`"]
fn a_second_prepare_replaces_the_first() {
    let _serialized = render_lock();
    let (device, queue, caps) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let red = tinted_scene(RED);
    let blue = tinted_scene(BLUE);

    let reference_target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let mut reference = renderer(&device, &caps);
    let _ = encode_frame(&device, &queue, &mut reference, &blue, &reference_target);
    let expected = reference_target.read_back(&device, &queue);

    let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let mut renderer = renderer(&device, &caps);
    renderer
        .prepare(&red, SIZE, SIZE, usage(&target), Affine::IDENTITY)
        .expect("the red frame plans");
    renderer
        .prepare(&blue, SIZE, SIZE, usage(&target), Affine::IDENTITY)
        .expect("the blue frame plans");
    let _ = encode_prepared_frame(&device, &queue, &mut renderer, &blue, &target);
    let pixels = target.read_back(&device, &queue);

    // A translucent blue over black: blue at half intensity, no red at all.
    let inside = pixel(&pixels, SIZE, 128, 128);
    assert_eq!(
        inside[0], 0,
        "nothing of the first plan is drawn: {inside:?}"
    );
    assert!(
        (i32::from(inside[2]) - 128).abs() <= 1,
        "the second plan is: {inside:?}"
    );
    assert!(
        pixels == expected,
        "the second plan draws as a plain encode of it"
    );

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the frames raised {error:?}");
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test prepare_split -- --ignored --test-threads=1`"]
fn an_abandoned_prepare_leaves_the_retained_store_as_it_was() {
    let _serialized = render_lock();
    let (device, queue, caps) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    // The reference: the body recorded green, then drawn from its page, with
    // nothing in between.
    let reference_target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let mut reference = renderer(&device, &caps);
    let _ = encode_frame(
        &device,
        &queue,
        &mut reference,
        &record_retained(GREEN),
        &reference_target,
    );
    let _ = encode_frame(
        &device,
        &queue,
        &mut reference,
        &draw_retained(),
        &reference_target,
    );
    let expected = reference_target.read_back(&device, &queue);

    let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let mut renderer = renderer(&device, &caps);
    let _ = encode_frame(
        &device,
        &queue,
        &mut renderer,
        &record_retained(GREEN),
        &target,
    );
    let ledger: Vec<_> = renderer.retained_ledger().collect();
    assert!(
        ledger.iter().any(|entry| entry.key == RETAINED_KEY),
        "the body is resident after its recording frame"
    );
    let generation = renderer.retained_generation();

    // A frame re-recording the same key in red, planned and then abandoned —
    // a host that skipped it. Had its decisions been applied, the page would
    // be re-keyed for a recording that never reached it.
    renderer
        .prepare(
            &record_retained(RED),
            SIZE,
            SIZE,
            usage(&target),
            Affine::IDENTITY,
        )
        .expect("the re-recording frame plans");
    assert_eq!(
        renderer.retained_generation(),
        generation,
        "a plan changes no ledger entry"
    );
    assert_eq!(
        renderer.retained_ledger().collect::<Vec<_>>(),
        ledger,
        "a plan changes no page"
    );

    // Another frame's prepare and encode: it composites the page the green
    // recording left, as the reference does.
    let draw = draw_retained();
    renderer
        .prepare(&draw, SIZE, SIZE, usage(&target), Affine::IDENTITY)
        .expect("the draw frame plans: the key is still resident");
    let _ = encode_prepared_frame(&device, &queue, &mut renderer, &draw, &target);
    let pixels = target.read_back(&device, &queue);

    let inside = pixel(&pixels, SIZE, 96, 96);
    assert_eq!(
        inside[0], 0,
        "no red recording reached the page: {inside:?}"
    );
    // CSS green is 128 on its green channel; composited at 0.75 over black.
    assert!(
        (i32::from(inside[1]) - 96).abs() <= 1,
        "the green page composited at 0.75: {inside:?}"
    );
    assert!(
        pixels == expected,
        "the retained draw renders as it does with no abandoned plan before it"
    );
    assert!(
        renderer
            .retained_ledger()
            .any(|entry| entry.key == RETAINED_KEY),
        "the body stays resident"
    );

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the frames raised {error:?}");
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test prepare_split -- --ignored --test-threads=1`"]
fn encode_prepared_at_another_extent_re_plans_and_renders_that_extent() {
    let _serialized = render_lock();
    let (device, queue, caps) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let (width, height) = (192_u32, 160_u32);
    let scene = layered_scene();

    let reference_target = HeadlessTarget::new(&device, width, height, FORMAT);
    let mut reference = renderer(&device, &caps);
    let expected_passes = encode_frame(&device, &queue, &mut reference, &scene, &reference_target);
    let expected = reference_target.read_back(&device, &queue);

    // Prepared at the full extent, then handed a smaller target — a resize
    // between the host's prepare and its encode.
    let target = HeadlessTarget::new(&device, width, height, FORMAT);
    let mut renderer = renderer(&device, &caps);
    renderer
        .prepare(&scene, SIZE, SIZE, usage(&target), Affine::IDENTITY)
        .expect("the frame plans at the full extent");
    let passes = encode_prepared_frame(&device, &queue, &mut renderer, &scene, &target);
    let pixels = target.read_back(&device, &queue);

    assert_eq!(
        passes, expected_passes,
        "the re-plan's passes are the extent's"
    );
    assert!(
        pixels == expected,
        "the re-planned frame draws as a plain encode at that extent"
    );
    // The last row and column are inside the target, and the backdrop reached
    // them: nothing was clipped to the extent the plan was first made at.
    assert_eq!(pixel(&pixels, width, width - 1, height - 1), [0, 0, 0, 255]);

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the frames raised {error:?}");
}
