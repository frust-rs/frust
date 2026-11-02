//! The `EngineRenderer` passthroughs for two per-compiler switches a caller
//! reaches only through the renderer: faux oblique (`set_synthetic_oblique`)
//! and the resident glyph-run cache (`set_glyph_run_cache`).
//!
//! The renderer's compiler is private, and a compiled frame's counters
//! (`synthetic_oblique_runs`, `resident_glyph_runs`) are not on the renderer's
//! surface, so these cases observe each switch through the pixels the renderer
//! draws: an italic request leans only while faux oblique is on, and the
//! resident-run cache draws the same pixels as the path that resolves every
//! glyph through `glifo`.
//!
//! Every case needs a GPU adapter, so they are `#[ignore]`d like the other
//! engine device suites:
//!
//! ```text
//! cargo test -p frust-engine --test renderer_switches -- --ignored --test-threads=1
//! ```

#![cfg(not(target_arch = "wasm32"))]

use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
use frust_gpu::{HeadlessTarget, TierCaps};
use frust_scene::{FontHandle, GlyphRun, Scene, SceneBuilder};
use frust_text::{FontFamily, FontWeight, TextContext, TextStyle};
use kurbo::{Affine, Point};
use peniko::Color;

/// Noto Sans, subsetted to Latin plus combining marks — a static face with no
/// italic and no slant axis.
const STATIC_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

const TEXT: &str = "Hamburg";
const FONT_SIZE: f32 = 24.0;

/// Host and device extent.
const WIDTH: u32 = 160;
const HEIGHT: u32 = 64;

/// The read-back format: R, G, B, A in byte order, no swizzle.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Frames a renderer draws before its read-back is taken: past the atlas
/// policy's size-change guard, the frame that rasterizes every glyph, and the
/// frame the resident-run cache learns the run on.
const FRAMES: usize = 14;

/// The italic request: a skew of 14 degrees on the static face, the shape
/// `FontHandle::with_synthesis` gives a run the shaper asked italic for.
const FAUX_OBLIQUE_DEGREES: f32 = 14.0;

fn context_with(font: &[u8]) -> (TextContext, String) {
    let mut ctx = TextContext::new();
    let families = ctx
        .register_fonts(font.to_vec())
        .expect("the face registers");
    let family = families[0].name.clone();
    (ctx, family)
}

/// `TEXT` laid out upright in `family`.
fn upright_runs(ctx: &mut TextContext, family: &str) -> Vec<GlyphRun> {
    let mut style = TextStyle::new(FONT_SIZE, Color::BLACK);
    style.family = FontFamily::named(family);
    style.weight = FontWeight::REGULAR;
    ctx.layout(TEXT, &style, None)
        .to_scene_runs(Point::new(8.0, 8.0))
}

/// `runs` with every face asked for italic and given the faux-oblique skew.
fn italic_runs(runs: &[GlyphRun]) -> Vec<GlyphRun> {
    runs.iter()
        .map(|run| GlyphRun {
            font: FontHandle::clone(&run.font).with_synthesis(0.0, FAUX_OBLIQUE_DEGREES),
            ..run.clone()
        })
        .collect()
}

fn scene_of(runs: &[GlyphRun]) -> Scene {
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        for run in runs {
            builder.draw_glyph_run(run.clone());
        }
    }
    scene
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
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

fn gpu() -> (wgpu::Device, wgpu::Queue, TierCaps) {
    block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine renderer-switches device"),
                required_features: wgpu::Features::empty(),
                required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// Draws `scene` for [`FRAMES`] frames through a fresh renderer, configured by
/// `configure` before its first frame, and returns the last frame's read-back.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    caps: &TierCaps,
    scene: &Scene,
    configure: impl FnOnce(&mut EngineRenderer),
) -> Vec<u8> {
    let mut renderer =
        EngineRenderer::new(device, caps, FORMAT, None).expect("the engine builds on this device");
    configure(&mut renderer);
    renderer.finish_warm_up(device);
    let target = HeadlessTarget::new(device, WIDTH, HEIGHT, FORMAT);
    for _ in 0..FRAMES {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine renderer-switches frame"),
        });
        renderer
            .encode(
                device,
                queue,
                &mut encoder,
                scene,
                EngineTarget {
                    view: target.view(),
                    format: FORMAT,
                    width: WIDTH,
                    height: HEIGHT,
                    depth: None,
                    output: OutputAlpha::Premultiplied,
                },
                Color::TRANSPARENT,
                Affine::IDENTITY,
            )
            .expect("a well-formed frame encodes");
        queue.submit([encoder.finish()]);
        renderer.end_frame(queue);
    }
    target.read_back(device, queue)
}

fn differing(a: &[u8], b: &[u8]) -> usize {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(x, y)| x != y)
        .count()
}

fn inked(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[3] != 0)
        .count()
}

#[test]
#[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test renderer_switches -- --ignored`"]
fn the_renderer_reports_the_switches_it_was_set_to() {
    let (device, _queue, caps) = gpu();
    let mut renderer = EngineRenderer::new(&device, &caps, FORMAT, None)
        .expect("the engine builds on this device");

    assert!(
        !renderer.synthetic_oblique(),
        "faux oblique is off by default"
    );
    renderer.set_synthetic_oblique(true);
    assert!(renderer.synthetic_oblique());
    renderer.set_synthetic_oblique(false);
    assert!(!renderer.synthetic_oblique());

    renderer.set_glyph_run_cache(false);
    assert!(!renderer.glyph_run_cache());
    renderer.set_glyph_run_cache(true);
    assert!(renderer.glyph_run_cache());
}

#[test]
#[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test renderer_switches -- --ignored`"]
fn faux_oblique_on_the_renderer_leans_an_italic_request_and_off_restores_upright() {
    let (device, queue, caps) = gpu();
    let (mut ctx, family) = context_with(STATIC_FONT);
    let upright = upright_runs(&mut ctx, &family);
    let upright_scene = scene_of(&upright);
    let italic_scene = scene_of(&italic_runs(&upright));

    let upright_pixels = render(&device, &queue, &caps, &upright_scene, |_| {});
    assert!(inked(&upright_pixels) > 0, "the upright frame drew nothing");

    // Off by default: the italic request draws upright, the same pixels as
    // the upright run.
    let default_off = render(&device, &queue, &caps, &italic_scene, |_| {});
    assert_eq!(
        default_off, upright_pixels,
        "an italic request on a face without one draws upright by default"
    );

    let leaned = render(&device, &queue, &caps, &italic_scene, |renderer| {
        renderer.set_synthetic_oblique(true);
    });
    let leaned_pixels = differing(&leaned, &upright_pixels);
    assert!(
        leaned_pixels > 0,
        "with faux oblique on, the italic request must lean ({leaned_pixels} pixels differ)"
    );

    let switched_back = render(&device, &queue, &caps, &italic_scene, |renderer| {
        renderer.set_synthetic_oblique(true);
        renderer.set_synthetic_oblique(false);
    });
    assert_eq!(
        switched_back, default_off,
        "switching faux oblique back off restores the upright pixels"
    );
}

#[test]
#[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test renderer_switches -- --ignored`"]
fn the_glyph_run_cache_switch_draws_the_same_pixels_as_glyph_by_glyph() {
    let (device, queue, caps) = gpu();
    let (mut ctx, family) = context_with(STATIC_FONT);
    let scene = scene_of(&upright_runs(&mut ctx, &family));

    let cached = render(&device, &queue, &caps, &scene, |renderer| {
        renderer.set_glyph_quads(true);
        renderer.set_glyph_run_cache(true);
    });
    let glyph_by_glyph = render(&device, &queue, &caps, &scene, |renderer| {
        renderer.set_glyph_quads(true);
        renderer.set_glyph_run_cache(false);
    });
    assert!(inked(&cached) > 0, "the cached arm drew nothing");
    assert_eq!(
        differing(&cached, &glyph_by_glyph),
        0,
        "the resident-run cache must draw what glifo draws glyph by glyph"
    );
}
