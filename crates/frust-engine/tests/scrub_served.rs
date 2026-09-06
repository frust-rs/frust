//! A navigation scrub, frame by frame: forty transition frames of the shape a
//! held full-screen page layer records, every one of them served.
//!
//! The shape is the one a swipe-back or a shared-axis transition holds for the
//! whole of a drag: a full-target layer at a fractional opacity — the page
//! being scrubbed — holding its own dark backdrop, a chip beside it, and a
//! later chip carrying a nested translucent chip of its own. The outer layer
//! takes a page group of its own the moment its round is
//! [cut](frust_engine::schedule) after the first chip, the nested chip takes
//! the second, and the chip *between* them needs a third: this is exactly the
//! shape the scheduler serves on its one bounded spill page, and the shape it
//! used to refuse.
//!
//! Why forty frames on one renderer rather than one frame on a fresh one. A
//! refusal is a skipped frame with no fallback renderer (see
//! `docs/LIMITATIONS.md`'s `engine-scheduler-escalation`), so a shape that
//! refuses while an opacity is fractional refuses for the whole gesture and
//! freezes the surface at whatever it last presented — which reads on a device
//! as a scrub that holds instead of snapping back, not as a wrong pixel. One
//! frame cannot show that. A sweep across the opacities a transition really
//! passes through, driven through one persistent renderer exactly as a live
//! surface drives it (one pool, one atlas, one instance buffer carried across
//! frames), can: the assertion is that *every* frame of it is served.
//!
//! The pixel spot-checks are the second half of the claim — serving a frame
//! wrong would satisfy the first half. They are computed rather than compared
//! against a reference rasterizer, for the reason `desktop_stress.rs`'s own
//! header gives: every draw here is an axis-aligned, pixel-aligned, solid
//! rectangle sampled well inside its own interior, where source-over at a
//! known alpha has one arithmetic answer and antialiasing has no say. The
//! colours are deliberately far apart so a page sampled out of the wrong group
//! — the failure a third live page could plausibly introduce — reads as a
//! colour error rather than as a rounding one.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Mutex, MutexGuard, PoisonError};

use frust_engine::schedule::{MAX_LIVE_PAGES, PageConfig, PageParity, Round, Schedule};
use frust_engine::{
    EngineError, EngineRenderer, EngineTarget, OutputAlpha, SceneCompiler, compile::CompiledFrame,
};
use frust_gpu::{ContextOptions, DownlevelProfile, HeadlessTarget, RenderContext, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::{Brush, Color};

/// The surface every frame is recorded and rendered against — a phone in
/// logical points, which is the surface the scrub shape belongs to.
const SCRUB: (u32, u32) = (375, 667);

/// The one target format, chosen for the reason `desktop_stress.rs` chooses
/// it: `Rgba8Unorm` reads back R, G, B, A in that order, so a readback needs
/// no swizzling.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The per-channel bar every pixel assertion is made at.
///
/// One level wider than `desktop_stress.rs`'s three, because the deepest
/// sample here passes through three composites rather than two — the nested
/// chip into its host chip, the chip into the scrubbing page, the page onto
/// the surface — and each one rounds an `Rgba8Unorm` intermediate. Still two
/// orders of magnitude below the distances the assertions are about: the
/// colours below sit a hundred levels or more apart.
const TOLERANCE: u8 = 4;

/// The opaque surface every frame composites onto, and the base colour the
/// frame is cleared to. Nothing in the scene paints it, so a pixel that reads
/// as this is a layer that never arrived.
const BACKDROP: Color = Color::from_rgba8(0, 0, 255, 255);

/// The scrubbing page's own backdrop, inside the outer layer.
const PAGE: Color = Color::from_rgba8(28, 28, 30, 255);

/// A chip's opaque body.
const CHIP: Color = Color::from_rgba8(0, 200, 60, 255);

/// The translucent wash each chip carries over its own body.
const TINT: Color = Color::from_rgba8(220, 40, 240, 255);

/// The nested translucent chip the later chip carries.
const NESTED: Color = Color::from_rgba8(255, 255, 255, 255);

/// The opacity a chip's tint layer composites at — fractional, so the layer
/// is isolated and costs a page.
const TINT_ALPHA: f32 = 0.35;

/// The opacity the nested chip composites at: low, as a ripple or a pressed
/// overlay is, and the layer whose page is the third one live.
const NESTED_ALPHA: f32 = 0.12;

/// Serializes every test in this binary that creates a GPU device — the same
/// guard the crate's other GPU suites take, for the same driver-teardown
/// reason. Poison is ignored deliberately: one case's failure must not cascade
/// into its siblings.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Blocks on `future` by polling it to completion.
///
/// wgpu's native adapter and device requests resolve without an executor
/// driving them, so a bare poll loop is enough; this crate has no async
/// runtime of its own.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context as TaskContext, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = TaskContext::from_waker(waker);
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
    use std::task::{Context as TaskContext, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = TaskContext::from_waker(waker);
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

/// The device, queue and capabilities one GPU context hands out.
fn gpu() -> (RenderContext, wgpu::Device, wgpu::Queue, TierCaps) {
    let mut context = RenderContext::with_options(ContextOptions {
        device_label: "frust-engine scrub device".to_string(),
        backends: None,
    });
    let handle = block_on(context.device()).expect("no compatible GPU adapter");
    println!(
        "frust-engine scrub adapter: {:?}",
        handle.adapter.get_info()
    );
    let (device, queue, caps) = (
        handle.device.clone(),
        handle.queue.clone(),
        handle.caps.clone(),
    );
    (context, device, queue, caps)
}

/// The opacities the scrub sweeps through: up from barely-there to nearly
/// opaque, then back part of the way, as a gesture that is driven forward and
/// then released does.
///
/// Forty of them, and every one strictly between transparent and opaque —
/// which is what keeps the outer layer isolated, and so keeps the shape under
/// test present on every frame rather than on some of them.
fn scrub_alphas() -> Vec<f32> {
    let up = (0..25).map(|step| 0.04 + 0.86 * (step as f32) / 24.0);
    let down = (0..15).map(|step| 0.9 - 0.8 * (step as f32) / 14.0);
    up.chain(down).collect()
}

/// The rectangle chip `index` occupies.
fn chip_rect(index: u32) -> Rect {
    let x = 24.0 + 168.0 * f64::from(index);
    Rect::new(x, 120.0, x + 152.0, 420.0)
}

/// The rectangle the nested chip inside chip `index` occupies — well inside
/// its host, so a sample taken in it is unambiguously inside both.
fn nested_rect(index: u32) -> Rect {
    let chip = chip_rect(index);
    Rect::new(
        chip.x0 + 32.0,
        chip.y0 + 32.0,
        chip.x0 + 120.0,
        chip.y0 + 200.0,
    )
}

/// Records chip `index`: an opaque body under a translucent tint layer, which
/// carries a nested translucent chip of its own when `nested`.
fn chip(builder: &mut SceneBuilder<'_>, index: u32, nested: bool) {
    let rect = chip_rect(index);
    builder.fill_rect(rect, Brush::Solid(CHIP));
    builder.push_layer(rect, TINT_ALPHA);
    builder.fill_rect(rect, Brush::Solid(TINT));
    if nested {
        let inner = nested_rect(index);
        builder.push_layer(inner, NESTED_ALPHA);
        builder.fill_rect(inner, Brush::Solid(NESTED));
        builder.pop_layer();
    }
    builder.pop_layer();
}

/// One frame of the scrub at `alpha`: the whole page inside one full-target
/// layer at that opacity, holding its backdrop, a plain chip, and a later chip
/// that nests one.
///
/// The order is what makes the shape: the plain chip's composite is what the
/// outer layer's round is cut around, which is what leaves the outer layer
/// holding a group for the rest of the frame — so by the time the nested chip
/// has taken the second group, the chip hosting it has none.
fn scrub_scene(alpha: f32) -> Scene {
    let full = Rect::new(0.0, 0.0, f64::from(SCRUB.0), f64::from(SCRUB.1));
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);

    builder.push_layer(full, alpha);
    builder.fill_rect(full, Brush::Solid(PAGE));
    chip(&mut builder, 0, false);
    chip(&mut builder, 1, true);
    builder.pop_layer();

    scene
}

/// `scene` compiled against the scrub surface.
fn compile(scene: &Scene) -> CompiledFrame {
    let size = (
        u16::try_from(SCRUB.0).expect("the scrub surface is inside the device grid"),
        u16::try_from(SCRUB.1).expect("the scrub surface is inside the device grid"),
    );
    SceneCompiler::new(size.0, size.1)
        .compile(scene, Affine::IDENTITY, size)
        .expect("an in-range scene compiles")
}

/// The rounds `frame` schedules as, or the refusal it was answered with.
fn rounds(frame: &CompiledFrame) -> Result<Vec<Round>, EngineError> {
    Schedule::build(
        &frame.recorder,
        &TierCaps::fake(DownlevelProfile::Full),
        &PageConfig::default(),
    )
}

/// `source` composited over `dest` at `alpha`, both operands opaque.
///
/// Chained rather than rounded per step, so a sample taken through several
/// layers is computed once in float and rounded once at the end; the tolerance
/// is what covers the 8-bit intermediates the real path rounds at each stage.
fn over(source: Color, dest: Color, alpha: f32) -> Color {
    let (src, dst) = (source.components, dest.components);
    Color::new([
        alpha * src[0] + (1.0 - alpha) * dst[0],
        alpha * src[1] + (1.0 - alpha) * dst[1],
        alpha * src[2] + (1.0 - alpha) * dst[2],
        1.0,
    ])
}

/// `color` as the eight-bit pixel a readback holds.
fn bytes(color: Color) -> [u8; 4] {
    color
        .components
        .map(|channel| (channel * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// The RGBA bytes at `(x, y)` of a tightly packed readback of the scrub
/// surface.
fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let index = ((y * SCRUB.0 + x) * 4) as usize;
    pixels[index..index + 4]
        .try_into()
        .expect("a readback row holds four bytes per pixel")
}

/// Asserts the pixel at `(x, y)` matches `expected` within [`TOLERANCE`].
fn assert_pixel(pixels: &[u8], (x, y): (u32, u32), expected: Color, what: &str) {
    let actual = pixel(pixels, x, y);
    let expected = bytes(expected);
    let off = (0..4).any(|channel| actual[channel].abs_diff(expected[channel]) > TOLERANCE);
    assert!(
        !off,
        "{what}: pixel ({x}, {y}) is {actual:?}, expected {expected:?} +/- {TOLERANCE}"
    );
}

/// A point inside `rect` but outside `hole`, as whole device pixels.
fn point_in(rect: Rect, offset: (f64, f64)) -> (u32, u32) {
    ((rect.x0 + offset.0) as u32, (rect.y0 + offset.1) as u32)
}

/// The three samples one scrubbed frame at `alpha` is checked at, each with
/// the colour the recording says it must hold.
///
/// One per depth the shape reaches: the page's own backdrop under the outer
/// layer alone, a chip's tint over its body under it, and the nested chip
/// inside that — which is the layer whose page is the third one live.
fn samples(alpha: f32) -> Vec<((u32, u32), Color, &'static str)> {
    let tinted = over(TINT, CHIP, TINT_ALPHA);
    let nested = over(over(NESTED, TINT, NESTED_ALPHA), CHIP, TINT_ALPHA);
    vec![
        (
            (40, 560),
            over(PAGE, BACKDROP, alpha),
            "the scrubbing page's own backdrop",
        ),
        (
            point_in(chip_rect(0), (16.0, 240.0)),
            over(tinted, BACKDROP, alpha),
            "the plain chip's tint over its body",
        ),
        (
            point_in(nested_rect(1), (24.0, 80.0)),
            over(nested, BACKDROP, alpha),
            "the nested chip inside the later chip",
        ),
    ]
}

// ---------------------------------------------------------------------------
// Host-only: every frame of the scrub schedules, and the third page is what
// serves it.
// ---------------------------------------------------------------------------

#[test]
fn every_frame_of_the_scrub_schedules_without_refusing() {
    let mut refused: Vec<String> = Vec::new();
    let mut spilled = 0_usize;

    for (frame, alpha) in scrub_alphas().iter().enumerate() {
        let compiled = compile(&scrub_scene(*alpha));
        match rounds(&compiled) {
            Err(error) => refused.push(format!("frame {frame} at alpha {alpha:.3}: {error}")),
            Ok(rounds) => {
                assert!(
                    rounds
                        .iter()
                        .filter_map(Round::page)
                        .any(|page| page.parity == PageParity::Spill),
                    "frame {frame} at alpha {alpha:.3} is served without the spill page, so this \
                     recording no longer holds the shape the case is about: {rounds:?}"
                );
                spilled += 1;
            }
        }
    }

    assert!(
        refused.is_empty(),
        "{} of {} scrub frames were refused:\n{}",
        refused.len(),
        scrub_alphas().len(),
        refused.join("\n")
    );
    assert_eq!(spilled, scrub_alphas().len());
}

#[test]
fn a_scrubbed_frame_never_holds_more_pages_live_than_the_bound() {
    // The spill is one page, not an unbounded third group: whatever the
    // opacity, no round of this shape may hold more than the scheduler's own
    // bound live at once — its own page plus everything it samples.
    for alpha in scrub_alphas() {
        let compiled = compile(&scrub_scene(alpha));
        let rounds = rounds(&compiled).expect("a scrub frame is served");

        for round in &rounds {
            let live = usize::from(round.page().is_some()) + round.released.len();
            assert!(
                live <= MAX_LIVE_PAGES,
                "alpha {alpha:.3} holds {live} pages live in {round:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// On a device: forty frames through one renderer, every one of them served.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test scrub_served -- --ignored`"]
fn a_forty_frame_scrub_on_one_renderer_serves_every_frame_and_paints_the_analytic_answer() {
    let _guard = render_lock();
    let (_context, device, queue, caps) = gpu();
    let (width, height) = SCRUB;

    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = HeadlessTarget::new(&device, width, height, FORMAT);
    // One renderer for the whole sweep, as a live surface has: its pool, its
    // atlas and its instance buffers carry from frame to frame, which is the
    // state a per-frame renderer would hide.
    let mut renderer = EngineRenderer::new(&device, &caps, FORMAT, None)
        .expect("the engine builds on this device");

    let alphas = scrub_alphas();
    let mut refused: Vec<String> = Vec::new();
    let mut served = 0_usize;

    for (frame, alpha) in alphas.iter().enumerate() {
        let scene = scrub_scene(*alpha);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine scrub frame"),
        });
        let encoded = renderer.encode(
            &device,
            &queue,
            &mut encoder,
            &scene,
            EngineTarget {
                view: target.view(),
                format: FORMAT,
                width,
                height,
                depth: None,
                output: OutputAlpha::Premultiplied,
            },
            BACKDROP,
            Affine::IDENTITY,
        );
        queue.submit([encoder.finish()]);
        renderer.end_frame(&queue);

        if let Err(error) = encoded {
            // A refused frame presents nothing at all, so there is no readback
            // to check: record it and carry on, because how many of the sweep
            // refuse is the measurement.
            refused.push(format!("frame {frame} at alpha {alpha:.3}: {error}"));
            continue;
        }
        served += 1;

        // Every fifth frame, plus both ends of the sweep, is read back: a
        // readback is a device round trip, and the frames between two checked
        // ones still have to be *encoded* — which is what a schedule that fell
        // apart mid-sweep would fail at.
        if frame % 5 == 0 || frame + 1 == alphas.len() {
            let pixels = target.read_back(&device, &queue);
            for (at, expected, what) in samples(*alpha) {
                assert_pixel(
                    &pixels,
                    at,
                    expected,
                    &format!("frame {frame} at alpha {alpha:.3}: {what}"),
                );
            }
        }
    }

    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the scrub raised {error:?}");

    println!(
        "frust-engine scrub: {served}/{} frames served",
        alphas.len()
    );
    assert!(
        refused.is_empty(),
        "{} of {} scrub frames were refused, so the surface would have frozen for the whole \
         gesture:\n{}",
        refused.len(),
        alphas.len(),
        refused.join("\n")
    );
    assert_eq!(served, alphas.len());
}
