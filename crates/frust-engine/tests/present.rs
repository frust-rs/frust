//! The present pass on real hardware: what a straight-alpha swapchain actually
//! receives.
//!
//! The engine's own output convention is premultiplied and pinned elsewhere
//! (`frust-testing`'s alpha-polarity suite). What this file pins is the
//! conversion [`UnpremultiplyPass`] applies on top of it for a swapchain that
//! stores STRAIGHT alpha — iOS's `PostMultiplied` composite alpha mode — and the
//! two properties that conversion lives or dies by:
//!
//! - **The polarity really flips.** A 50%-alpha white fill leaves the engine's
//!   intermediate as `(128, 128, 128, 128)` and must reach the swapchain as
//!   `(255, 255, 255, 128)`. The two readings are equally plausible-looking
//!   bytes, and a compositor handed the premultiplied one presents every
//!   partial-alpha pixel at half brightness.
//! - **A zero-alpha pixel stays finite.** Un-premultiplying divides by alpha,
//!   and the frame is full of pixels whose alpha is exactly zero. The shader's
//!   `ALPHA_FLOOR` guard is what keeps `0 / 0` out of the swapchain; a NaN
//!   written there is undefined content, not a transparent pixel.
//!
//! A third case renders the same scene straight into the target with no pass at
//! all, so the premultiplied arm every other surface takes is proven unchanged
//! by the same frame in the same file, rather than assumed.
//!
//! The host-only case at the top needs no GPU: the program is validated through
//! the same `naga` a `wgpu::Device` would, so a syntax or type error surfaces in
//! the ordinary workspace test run instead of as a pipeline failure on a device.
//! Everything else runs under a validation error scope, so a wrong bind group or
//! a mismatched attachment names itself rather than producing a
//! plausible-looking image.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Mutex, MutexGuard, PoisonError};

use frust_engine::gpu::present::{UNPREMULTIPLY, UNPREMULTIPLY_NAME, UnpremultiplyPass};
use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
use frust_gpu::{HeadlessTarget, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::color::palette::css::WHITE;
use peniko::{Brush, Color};
use wgpu::naga;

/// Target extent every case renders at.
const SIZE: u32 = 64;

/// The swapchain format the pass writes. `Rgba8Unorm` reads back R, G, B, A in
/// that order, so a readback needs no swizzling.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The format of the intermediate the engine renders into on this arm — the
/// engine's own off-screen format, which is what a host configures its renderer
/// for when the swapchain is served through the pass rather than directly.
const INTERMEDIATE: wgpu::TextureFormat = frust_engine::gpu::pipelines::INTERMEDIATE_FORMAT;

/// The filled region: well inside the frame and integer-aligned on every edge,
/// so its interior carries no antialiased coverage and the only thing a sampled
/// pixel can be about is alpha.
const FILL: Rect = Rect::new(16.0, 16.0, 48.0, 48.0);

/// Interior sample points, all strictly inside [`FILL`].
const INTERIOR: &[(u32, u32)] = &[(20, 20), (32, 32), (44, 44)];

/// Sample points outside [`FILL`], where nothing is drawn and the frame's
/// transparent base colour is all there is — the pixels whose alpha is exactly
/// zero, and therefore the ones the divisor guard is for.
const OUTSIDE: &[(u32, u32)] = &[(4, 4), (60, 4), (4, 60), (60, 60)];

/// 50%-alpha white as the engine writes it: colour channels already scaled by
/// alpha (`1.0 * 0.5 -> 127.5 -> 128`).
const PREMULTIPLIED: [u8; 4] = [128, 128, 128, 128];

/// The same pixel after the conversion: colour channels restored to their own
/// value, alpha untouched.
const STRAIGHT: [u8; 4] = [255, 255, 255, 128];

/// The per-channel bar every assertion is made at. An 8-bit rounding step
/// either way, two orders of magnitude below the 127-level gap between the two
/// conventions, so it can never blur one into the other.
const TOLERANCE: u8 = 2;

/// Serializes every test in this binary that creates a GPU device — the same
/// guard the crate's other GPU suites take, for the same driver-teardown reason
/// (`tests/encode_contract.rs` carries the full note). Poison is ignored
/// deliberately: one test's failure must not cascade into every sibling.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
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
        println!("frust-engine present adapter: {:?}", adapter.get_info());
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine present device"),
                required_features: wgpu::Features::empty(),
                required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// The intermediate the engine renders into on the un-premultiplying arm:
/// `RENDER_ATTACHMENT | TEXTURE_BINDING`, exactly what a host configures — a
/// colour attachment for the frame, a sampled source for the pass, and no
/// `STORAGE_BINDING`, which this tier never has.
fn intermediate(device: &wgpu::Device) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust-engine present test intermediate"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: INTERMEDIATE,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The one scene every case renders: a single 50%-alpha white rectangle over a
/// transparent frame.
fn half_alpha_white() -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    builder.fill_rect(FILL, Brush::Solid(WHITE.with_alpha(0.5)));
    scene
}

/// The RGBA bytes of the pixel at `(x, y)` in a tightly packed readback.
fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let index = ((y * SIZE + x) * 4) as usize;
    pixels[index..index + 4]
        .try_into()
        .expect("a readback row holds four bytes per pixel")
}

/// Asserts every sampled pixel matches `expected` within [`TOLERANCE`].
fn assert_pixels(pixels: &[u8], at: &[(u32, u32)], expected: [u8; 4], what: &str) {
    for &(x, y) in at {
        let actual = pixel(pixels, x, y);
        let off = (0..4).any(|c| actual[c].abs_diff(expected[c]) > TOLERANCE);
        assert!(
            !off,
            "{what}: pixel ({x}, {y}) is {actual:?}, expected {expected:?} +/- {TOLERANCE}"
        );
    }
}

/// Renders [`half_alpha_white`] into an intermediate and converts it into the
/// returned target's pixels through [`UnpremultiplyPass`] — the whole
/// straight-alpha arm, in one encoder and one submit exactly as a host records
/// it.
fn through_the_present_pass() -> Vec<u8> {
    let (device, queue, caps) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let source = intermediate(&device);
    // Warmed for the INTERMEDIATE's format, not the swapchain's: on this arm
    // the engine's own passes never touch the swapchain.
    let mut renderer = EngineRenderer::new(&device, &caps, INTERMEDIATE, None)
        .expect("the engine builds on this device");
    let present = UnpremultiplyPass::new(&device, FORMAT, None);
    assert_eq!(present.format(), FORMAT);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frust-engine present frame"),
    });
    renderer
        .encode(
            &device,
            &queue,
            &mut encoder,
            &half_alpha_white(),
            EngineTarget {
                view: &source,
                format: INTERMEDIATE,
                width: SIZE,
                height: SIZE,
                depth: None,
                // The INTERMEDIATE really does hold premultiplied pixels — the
                // pipelines are fixed premultiplied, so this is what keeps the
                // frame's base colour in the same convention as everything
                // drawn over it. `Straight` describes the swapchain the pass
                // below writes, not this buffer.
                output: OutputAlpha::Premultiplied,
            },
            Color::TRANSPARENT,
            Affine::IDENTITY,
        )
        .expect("a well-formed frame encodes");
    // Into the SAME encoder, after the frame: one command buffer carries the
    // frame's passes and the conversion, in order.
    present.record(&device, &mut encoder, &source, target.view(), None);
    queue.submit([encoder.finish()]);
    renderer.end_frame(&queue);

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the frame raised {error:?}");

    target.read_back(&device, &queue)
}

/// The same scene rendered straight into the target, with no present pass — the
/// arm every premultiplied-expecting surface takes.
fn without_the_present_pass() -> Vec<u8> {
    let (device, queue, caps) = gpu();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let mut renderer = EngineRenderer::new(&device, &caps, FORMAT, None)
        .expect("the engine builds on this device");

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frust-engine premultiplied frame"),
    });
    renderer
        .encode(
            &device,
            &queue,
            &mut encoder,
            &half_alpha_white(),
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
    queue.submit([encoder.finish()]);
    renderer.end_frame(&queue);

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the frame raised {error:?}");

    target.read_back(&device, &queue)
}

/// Parses and validates `src` the way a `wgpu::Device` would.
///
/// `wgpu` re-exports the same `naga` its own WGSL front end uses, so this is
/// the device's validation path minus the device — the same host-side guard the
/// engine's pipeline modules keep.
fn validate(name: &str, src: &str) {
    let module = naga::front::wgsl::parse_str(src)
        .unwrap_or_else(|err| panic!("{name} failed to parse: {err:?}"));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap_or_else(|err| panic!("{name} failed validation: {err:?}"));
}

#[test]
fn the_present_program_parses_and_validates() {
    validate(UNPREMULTIPLY_NAME, UNPREMULTIPLY);
    // The guard the fragment stage divides by has to be a real declaration in
    // the source that was just validated, not a comment about one. (Its value
    // is pinned against the crate's own constant by `gpu::present`'s tests.)
    assert!(
        UNPREMULTIPLY
            .lines()
            .any(|line| line.trim_start().starts_with("const ALPHA_FLOOR")),
        "the validated program must declare the divisor guard"
    );
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test present -- --ignored`"]
fn a_half_alpha_fill_reaches_a_straight_alpha_target_unpremultiplied() {
    let _serialized = render_lock();
    let pixels = through_the_present_pass();

    assert_pixels(
        &pixels,
        INTERIOR,
        STRAIGHT,
        "a 50%-alpha white fill must reach a straight-alpha swapchain with its colour channels \
         restored",
    );
    // The negative half of the same claim: the bytes the intermediate held are
    // exactly what must NOT reach the swapchain, and they differ from the
    // expected ones by 127 levels — far outside the tolerance above.
    let inside = pixel(&pixels, INTERIOR[0].0, INTERIOR[0].1);
    assert_ne!(
        inside, PREMULTIPLIED,
        "the swapchain received the engine's premultiplied bytes unconverted"
    );
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test present -- --ignored`"]
fn zero_alpha_pixels_stay_finite_and_transparent_black() {
    let _serialized = render_lock();
    let pixels = through_the_present_pass();

    // `0 / 0` would be a NaN, which a `unorm` target resolves to an
    // implementation-defined byte rather than to zero — so this asserts the
    // exact value, not merely "transparent".
    assert_pixels(
        &pixels,
        OUTSIDE,
        [0, 0, 0, 0],
        "an alpha-zero pixel must stay transparent black through the divisor guard",
    );
}

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test present -- --ignored`"]
fn the_premultiplied_arm_is_unchanged_by_the_pass_existing() {
    let _serialized = render_lock();
    let pixels = without_the_present_pass();

    assert_pixels(
        &pixels,
        INTERIOR,
        PREMULTIPLIED,
        "a target the engine writes directly must still receive premultiplied bytes",
    );
    assert_pixels(
        &pixels,
        OUTSIDE,
        [0, 0, 0, 0],
        "the transparent base colour is unchanged on the direct arm",
    );
}
