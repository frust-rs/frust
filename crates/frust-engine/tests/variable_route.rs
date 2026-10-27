//! The `glifo` route draws a run at the weight it was shaped at.
//!
//! A run's handle carries the variation coordinates the shaper chose and, for
//! a static face asked for a weight it lacks, a faux-bold stroke. The engine
//! hands both to `glifo` and folds both into every cache key it builds:
//!
//! - **A variable face at weight 700 inks more than at 400**, through the
//!   strip path the CPU compiles to.
//! - **The two runs are different cache identities.** After the weight-400
//!   run is resident, the weight-700 run of the same blob and glyph ids is not
//!   served from its record.
//! - **A static face asked for 700 inks more than the same face at 400**
//!   (faux bold).
//! - **Faux oblique leans the right way and is never cached**: the run is
//!   routed to outlines, and its ink differs from the upright run's.
//!
//! The GPU variant (ignored by default, needs Metal) repeats the weight
//! comparison on a headless target:
//! `cargo test -p frust-engine --test variable_route -- --ignored`.

use frust_engine::cache::images::ImageResidency;
use frust_engine::compile::CompiledFrame;
use frust_engine::{AtlasBudget, SceneCompiler};
use frust_scene::{FontHandle, GlyphRun, Scene, SceneBuilder};
use frust_text::{FontFamily, FontWeight, TextContext, TextStyle};
use kurbo::{Affine, Point};
use peniko::Color;

const VIEWPORT: (u16, u16) = (160, 64);
const FONT_SIZE: f32 = 24.0;
const TEXT: &str = "Hamburg";

/// A variable `glyf` face (`wght` 100..900, default 400).
const VARIABLE_FONT: &[u8] = include_bytes!("../../../plugins/beui/fonts/geist/Geist-Variable.ttf");
/// A static face with no bold of its own.
const STATIC_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

fn context_with(font: &[u8]) -> (TextContext, String) {
    let mut ctx = TextContext::new();
    let families = ctx
        .register_fonts(font.to_vec())
        .expect("the face registers");
    let family = families[0].name.clone();
    (ctx, family)
}

fn laid_out(ctx: &mut TextContext, family: &str, weight: FontWeight) -> Vec<GlyphRun> {
    let mut style = TextStyle::new(FONT_SIZE, Color::BLACK);
    style.family = FontFamily::named(family);
    style.weight = weight;
    ctx.layout(TEXT, &style, None)
        .to_scene_runs(Point::new(8.0, 8.0))
}

fn scene_of(runs: &[GlyphRun]) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    for run in runs {
        builder.draw_glyph_run(run.clone());
    }
    scene
}

/// A compiler whose glyphs all take the strip path, so the compiled frame's
/// coverage is the run's ink.
fn strip_compiler() -> SceneCompiler {
    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    compiler.set_image_residency(ImageResidency::disabled(AtlasBudget::MOBILE));
    compiler
}

fn compile(compiler: &mut SceneCompiler, runs: &[GlyphRun]) -> CompiledFrame {
    compiler
        .compile(&scene_of(runs), Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles")
}

/// The total coverage the frame's strips carry.
fn ink(frame: &CompiledFrame) -> u64 {
    frame.alphas().iter().map(|a| u64::from(*a)).sum()
}

fn ink_of(runs: &[GlyphRun]) -> u64 {
    let frame = compile(&mut strip_compiler(), runs);
    assert!(frame.glyph_draws > 0, "the run drew nothing");
    ink(&frame)
}

#[test]
fn a_variable_face_at_700_inks_more_than_at_400() {
    let (mut ctx, family) = context_with(VARIABLE_FONT);
    let regular = laid_out(&mut ctx, &family, FontWeight::REGULAR);
    let bold = laid_out(&mut ctx, &family, FontWeight::BOLD);
    assert!(regular[0].font.normalized_coords().is_empty());
    assert!(!bold[0].font.normalized_coords().is_empty());
    assert_eq!(bold[0].font.embolden(), 0.0, "a variable face is not faux");
    let (light_ink, bold_ink) = (ink_of(&regular), ink_of(&bold));
    println!("variable 400: {light_ink}, 700: {bold_ink}");
    assert!(bold_ink > light_ink, "700 inks {bold_ink}, 400 {light_ink}");
}

#[test]
fn a_faux_bold_static_face_inks_more_than_its_regular_run() {
    let (mut ctx, family) = context_with(STATIC_FONT);
    let regular = laid_out(&mut ctx, &family, FontWeight::REGULAR);
    let bold = laid_out(&mut ctx, &family, FontWeight::BOLD);
    assert_eq!(regular[0].font.embolden(), 0.0);
    assert!(
        bold[0].font.embolden() > 0.0,
        "the 700 request is synthetic"
    );
    let (regular_ink, bold_ink) = (ink_of(&regular), ink_of(&bold));
    println!("static regular: {regular_ink}, faux bold: {bold_ink}");
    assert!(bold_ink > regular_ink, "{bold_ink} vs {regular_ink}");
}

/// Two runs of one blob and one glyph sequence that differ only in weight are
/// two cache identities: the bold one is not drawn out of the regular one's
/// resident record, and each is resident with its own.
#[test]
fn weights_of_one_blob_have_distinct_resident_records() {
    let (mut ctx, family) = context_with(VARIABLE_FONT);
    let regular = laid_out(&mut ctx, &family, FontWeight::REGULAR);
    let bold = laid_out(&mut ctx, &family, FontWeight::BOLD);
    assert_eq!(
        regular[0].font.font().data.id(),
        bold[0].font.font().data.id()
    );
    let ids = |runs: &[GlyphRun]| runs[0].glyphs.iter().map(|g| g.id).collect::<Vec<_>>();
    assert_eq!(ids(&regular), ids(&bold), "the same glyph ids");

    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    compiler.set_glyph_quads(true);
    compiler.set_glyph_run_cache(true);
    // Settle the regular run into the resident cache.
    let mut resident_regular = 0;
    for _ in 0..6 {
        resident_regular = compile(&mut compiler, &regular).resident_glyph_runs;
    }
    assert!(
        resident_regular > 0,
        "the regular run never became resident"
    );
    // The bold run's first frame cannot be a resident hit.
    let first_bold = compile(&mut compiler, &bold);
    assert_eq!(first_bold.resident_glyph_runs, 0, "served the 400 record");
    // And the regular run is still its own record afterwards.
    let mut back = 0;
    for _ in 0..3 {
        back = compile(&mut compiler, &regular).resident_glyph_runs;
    }
    assert!(back > 0);
}

/// Faux oblique is its own code path: the run draws, differs from the upright
/// run, and is never cached or resident.
#[test]
fn a_faux_oblique_run_is_drawn_as_skewed_outlines_and_never_resident() {
    let (mut ctx, family) = context_with(STATIC_FONT);
    let upright = laid_out(&mut ctx, &family, FontWeight::REGULAR);
    let skewed: Vec<GlyphRun> = upright
        .iter()
        .map(|run| GlyphRun {
            font: FontHandle::clone(&run.font).with_synthesis(0.0, 14.0),
            ..run.clone()
        })
        .collect();
    let (upright_frame, skewed_frame) = (
        compile(&mut strip_compiler(), &upright),
        compile(&mut strip_compiler(), &skewed),
    );
    assert!(skewed_frame.glyph_draws > 0);
    assert_ne!(upright_frame.alphas(), skewed_frame.alphas());

    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    compiler.set_glyph_quads(true);
    compiler.set_glyph_run_cache(true);
    for _ in 0..6 {
        let frame = compile(&mut compiler, &skewed);
        assert_eq!(frame.resident_glyph_runs, 0);
        assert_eq!(frame.atlas_glyph_draws, 0, "a skewed run is not cached");
    }
}

#[cfg(target_os = "macos")]
mod gpu {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
    use frust_gpu::{HeadlessTarget, TierCaps};

    use super::*;

    static RENDER_LOCK: Mutex<()> = Mutex::new(());

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

    fn lock() -> MutexGuard<'static, ()> {
        RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The headless target's pixels (RGBA8) for `runs`, rendered through Metal.
    fn pixels(runs: &[GlyphRun]) -> Vec<u8> {
        let _guard = lock();
        let (device, queue, caps) = block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
                .await
                .expect("no compatible GPU adapter");
            let caps = TierCaps::probe(&adapter);
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-engine variable-route device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                    ..Default::default()
                })
                .await
                .expect("failed to create the device");
            (device, queue, caps)
        });
        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
        let scene = scene_of(runs);
        let mut renderer = EngineRenderer::with_image_sampler(&device, &caps, FORMAT, None, true)
            .expect("the engine builds on this device");
        renderer.finish_warm_up(&device);
        let (width, height) = (u32::from(VIEWPORT.0), u32::from(VIEWPORT.1));
        let target = HeadlessTarget::new(&device, width, height, FORMAT);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine variable-route frame"),
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
                    width,
                    height,
                    depth: None,
                    output: OutputAlpha::Premultiplied,
                },
                Color::TRANSPARENT,
                Affine::IDENTITY,
            )
            .expect("a well-formed frame encodes");
        queue.submit([encoder.finish()]);
        renderer.end_frame(&queue);
        target.read_back(&device, &queue)
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
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test variable_route -- --ignored`"]
    fn a_variable_face_at_700_inks_more_than_at_400_through_the_gpu() {
        let (mut ctx, family) = context_with(VARIABLE_FONT);
        let regular = inked(&pixels(&laid_out(&mut ctx, &family, FontWeight::REGULAR)));
        let bold = inked(&pixels(&laid_out(&mut ctx, &family, FontWeight::BOLD)));
        println!("gpu variable 400: {regular}, 700: {bold}");
        assert!(bold > regular, "700 inks {bold} pixels, 400 {regular}");
    }

    #[test]
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test variable_route -- --ignored`"]
    fn a_faux_bold_static_face_inks_more_through_the_gpu() {
        let (mut ctx, family) = context_with(STATIC_FONT);
        let regular = inked(&pixels(&laid_out(&mut ctx, &family, FontWeight::REGULAR)));
        let bold = inked(&pixels(&laid_out(&mut ctx, &family, FontWeight::BOLD)));
        println!("gpu static regular: {regular}, faux bold: {bold}");
        assert!(
            bold > regular,
            "faux bold inks {bold} pixels, regular {regular}"
        );
    }

    /// A faux-oblique run leans right: its ink's horizontal centre of mass is
    /// further right in the top half than in the bottom half, by more than the
    /// upright run's.
    #[test]
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test variable_route -- --ignored`"]
    fn a_faux_oblique_run_leans_right_through_the_gpu() {
        let (mut ctx, family) = context_with(STATIC_FONT);
        let upright = laid_out(&mut ctx, &family, FontWeight::REGULAR);
        let skewed: Vec<GlyphRun> = upright
            .iter()
            .map(|run| GlyphRun {
                font: FontHandle::clone(&run.font).with_synthesis(0.0, 14.0),
                ..run.clone()
            })
            .collect();
        let lean = |pixels: &[u8]| {
            let (mut top, mut bottom) = ((0.0, 0.0), (0.0, 0.0));
            for (index, px) in pixels.as_chunks::<4>().0.iter().enumerate() {
                let (x, y) = (
                    (index % VIEWPORT.0 as usize) as f64,
                    index / VIEWPORT.0 as usize,
                );
                let weight = f64::from(px[3]);
                // The text sits around y 8..40; split at its middle.
                let half = if y < 24 { &mut top } else { &mut bottom };
                half.0 += weight * x;
                half.1 += weight;
            }
            top.0 / top.1 - bottom.0 / bottom.1
        };
        let (flat, leaning) = (lean(&pixels(&upright)), lean(&pixels(&skewed)));
        println!("gpu lean upright: {flat:.3}, oblique: {leaning:.3}");
        assert!(leaning > flat + 1.0, "oblique {leaning}, upright {flat}");
    }
}
