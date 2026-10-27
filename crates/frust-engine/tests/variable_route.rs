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
//! - **Faux oblique is off by default** (`set_synthetic_oblique`): an italic
//!   request on a static face draws upright and caches like any upright run.
//!   Switched on, it leans and is never cached: the run is routed to
//!   outlines, and its ink differs from the upright run's.
//! - **Faux bold is as heavy unhinted as hinted**: `glifo` sizes an unhinted
//!   stroke in font units, so the engine measures it against `unitsPerEm`.
//!
//! The GPU variant (ignored by default, needs Metal) repeats the weight
//! comparison on a headless target:
//! `cargo test -p frust-engine --test variable_route -- --ignored`.

use frust_engine::cache::images::ImageResidency;
use frust_engine::compile::CompiledFrame;
use frust_engine::{AtlasBudget, SceneCompiler};
use frust_scene::{FontHandle, GlyphRun, Scene, SceneBuilder};
use frust_text::{FontFamily, FontStyle, FontWeight, TextContext, TextStyle};
use kurbo::{Affine, Point};
use peniko::Color;
use std::time::Instant;

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

/// [`strip_compiler`] with synthetic oblique switched on.
fn oblique_strip_compiler() -> SceneCompiler {
    let mut compiler = strip_compiler();
    compiler.set_synthetic_oblique(true);
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
    let regular = hello(&mut ctx, &family);
    let bold = faux_bold(&regular);
    assert_eq!(regular[0].font.embolden(), 0.0);
    assert!(bold[0].font.embolden() > 0.0, "the run is synthetic bold");
    let (regular_ink, bold_ink) = (ink_of(&regular), ink_of(&bold));
    println!("static regular: {regular_ink}, faux bold: {bold_ink}");
    assert!(bold_ink > regular_ink, "{bold_ink} vs {regular_ink}");
}

/// The handle `laid_out` gives a 700 request on this face, applied to `runs`.
fn faux_bold(runs: &[GlyphRun]) -> Vec<GlyphRun> {
    runs.iter()
        .map(|run| GlyphRun {
            font: FontHandle::clone(&run.font).with_synthesis(faux_bold_fraction(), 0.0),
            ..run.clone()
        })
        .collect()
}

/// The em fraction `frust_text` synthesizes for bold on a face without one.
fn faux_bold_fraction() -> f32 {
    let (mut ctx, family) = context_with(STATIC_FONT);
    laid_out(&mut ctx, &family, FontWeight::BOLD)[0]
        .font
        .embolden()
}

/// Ink of `runs` at 1x with hinting `hint`, through the strip path.
fn hinted_ink(runs: &[GlyphRun], hint: bool) -> u64 {
    let mut compiler = strip_compiler();
    compiler.set_hint_text(hint);
    let frame = compile(&mut compiler, runs);
    assert!(frame.glyph_draws > 0, "the run drew nothing");
    ink(&frame)
}

/// `glifo` expands an unhinted outline at the face's `unitsPerEm` and a hinted
/// one at the device size; the engine's basis follows, so faux bold is as heavy
/// in both modes. The four ink numbers are printed.
#[test]
fn faux_bold_weight_is_comparable_hinted_and_unhinted() {
    let (mut ctx, family) = context_with(STATIC_FONT);
    let regular = hello(&mut ctx, &family);
    let bold = faux_bold(&regular);
    let (unhinted_regular, unhinted_bold) = (hinted_ink(&regular, false), hinted_ink(&bold, false));
    let (hinted_regular, hinted_bold) = (hinted_ink(&regular, true), hinted_ink(&bold, true));
    eprintln!(
        "ink unhinted regular {unhinted_regular} bold {unhinted_bold}; hinted regular {hinted_regular} bold {hinted_bold}"
    );
    let gain = |bold: u64, regular: u64| bold as f64 / regular as f64 - 1.0;
    let (unhinted_gain, hinted_gain) = (
        gain(unhinted_bold, unhinted_regular),
        gain(hinted_bold, hinted_regular),
    );
    eprintln!("gain unhinted {unhinted_gain:.3}, hinted {hinted_gain:.3}");
    assert!(unhinted_gain >= 0.25, "unhinted gain {unhinted_gain}");
    assert!(hinted_gain >= 0.25, "hinted gain {hinted_gain}");
    let ratio = unhinted_gain / hinted_gain;
    assert!(
        (0.5..=2.0).contains(&ratio),
        "unhinted {unhinted_gain} vs hinted {hinted_gain}"
    );
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
        compile(&mut oblique_strip_compiler(), &skewed),
    );
    assert!(skewed_frame.glyph_draws > 0);
    assert_ne!(upright_frame.alphas(), skewed_frame.alphas());

    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    compiler.set_glyph_quads(true);
    compiler.set_glyph_run_cache(true);
    compiler.set_synthetic_oblique(true);
    for _ in 0..6 {
        let frame = compile(&mut compiler, &skewed);
        assert_eq!(frame.resident_glyph_runs, 0);
        assert_eq!(frame.atlas_glyph_draws, 0, "a skewed run is not cached");
    }
}

/// With the default (synthetic oblique off) an italic request on a static face
/// draws upright: the same pixels as the upright run, cached in the atlas by
/// frame 3, and never counted as a synthetic-oblique run.
#[test]
fn an_italic_request_draws_upright_and_caches_by_default() {
    let (mut ctx, family) = context_with(STATIC_FONT);
    let upright = hello(&mut ctx, &family);
    let skewed: Vec<GlyphRun> = upright
        .iter()
        .map(|run| GlyphRun {
            font: FontHandle::clone(&run.font).with_synthesis(0.0, 14.0),
            ..run.clone()
        })
        .collect();
    assert!(skewed.iter().all(|run| run.font.skew_degrees() > 0.0));
    assert!(!SceneCompiler::new(VIEWPORT.0, VIEWPORT.1).synthetic_oblique_enabled());
    assert_eq!(
        compile(&mut strip_compiler(), &upright).alphas(),
        compile(&mut strip_compiler(), &skewed).alphas(),
        "an obliqued face draws upright when the switch is off"
    );

    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    compiler.set_glyph_quads(true);
    compiler.set_glyph_run_cache(true);
    let mut frames = Vec::new();
    for _ in 0..3 {
        frames.push(compile(&mut compiler, &skewed));
    }
    assert!(frames.iter().all(|frame| frame.synthetic_oblique_runs == 0));
    assert!(
        frames[2].atlas_glyph_draws > 0,
        "the italic request never reached the atlas"
    );
}

/// A screen-sized paragraph: 45 lines of Latin text at 14 px.
const SCREEN: (u16, u16) = (480, 1000);

fn paragraph(ctx: &mut TextContext, family: &str, style: FontStyle) -> Vec<GlyphRun> {
    let text = (0..45)
        .map(|_| "Hello, Hello Hello, Hello Hello")
        .collect::<Vec<_>>()
        .join("\n");
    let mut text_style = TextStyle::new(14.0, Color::BLACK);
    text_style.family = FontFamily::named(family);
    text_style.style = style;
    ctx.layout(&text, &text_style, None)
        .to_scene_runs(Point::new(8.0, 8.0))
}

fn screen_compiler() -> SceneCompiler {
    let mut compiler = SceneCompiler::new(SCREEN.0, SCREEN.1);
    compiler.set_glyph_quads(true);
    compiler.set_glyph_run_cache(true);
    compiler
}

fn compile_screen(compiler: &mut SceneCompiler, runs: &[GlyphRun]) -> CompiledFrame {
    compiler
        .compile(&scene_of(runs), Affine::IDENTITY, SCREEN)
        .expect("an in-range scene compiles")
}

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

/// The cost of faux-italic text is observable and bounded: every skewed run
/// is counted as an outline run on every frame and none reaches the atlas,
/// while the same paragraph upright settles into it.
#[test]
fn an_italic_paragraph_is_counted_as_outline_runs_every_frame() {
    let (mut ctx, family) = context_with(STATIC_FONT);
    let italic = paragraph(&mut ctx, &family, FontStyle::Italic);
    let upright = paragraph(&mut ctx, &family, FontStyle::Normal);
    assert!(italic.len() >= 40, "{} runs", italic.len());
    assert!(
        italic.iter().all(|run| run.font.skew_degrees() > 0.0),
        "the italic request is synthesized as a skew"
    );
    assert!(upright.iter().all(|run| run.font.skew_degrees() == 0.0));

    let mut compiler = screen_compiler();
    compiler.set_synthetic_oblique(true);
    for frame_index in 0..3 {
        let frame = compile_screen(&mut compiler, &italic);
        assert_eq!(
            frame.synthetic_oblique_runs as usize,
            italic.len(),
            "frame {frame_index}"
        );
        assert_eq!(frame.atlas_glyph_draws, 0, "frame {frame_index}");
        assert_eq!(frame.resident_glyph_runs, 0, "frame {frame_index}");
        assert_eq!(frame.variation_axes_outline_runs, 0);
    }

    let mut compiler = screen_compiler();
    let mut settled = compile_screen(&mut compiler, &upright);
    for _ in 0..2 {
        settled = compile_screen(&mut compiler, &upright);
    }
    assert_eq!(settled.synthetic_oblique_runs, 0);
    assert!(settled.atlas_glyph_draws > 0, "upright text never settled");
}

/// Prints the per-frame CPU compile cost of italic against upright text, and
/// asserts only a generous ceiling.
#[test]
fn an_italic_paragraph_compile_cost_is_bounded() {
    let (mut ctx, family) = context_with(STATIC_FONT);
    let italic = paragraph(&mut ctx, &family, FontStyle::Italic);
    let upright = paragraph(&mut ctx, &family, FontStyle::Normal);
    let measure = |runs: &[GlyphRun]| {
        let mut compiler = screen_compiler();
        compiler.set_synthetic_oblique(true);
        // Settle first, so upright is timed on its steady-state path.
        for _ in 0..4 {
            compile_screen(&mut compiler, runs);
        }
        let samples = (0..24)
            .map(|_| {
                let start = Instant::now();
                compile_screen(&mut compiler, runs);
                start.elapsed().as_secs_f64() * 1000.0
            })
            .collect::<Vec<_>>();
        median(samples)
    };
    let (italic_ms, upright_ms) = (measure(&italic), measure(&upright));
    eprintln!(
        "per-frame compile median over 24 frames: italic {italic_ms:.3} ms, upright {upright_ms:.3} ms ({:.1}x)",
        italic_ms / upright_ms
    );
    assert!(
        italic_ms < upright_ms * 20.0,
        "italic {italic_ms} ms vs upright {upright_ms} ms"
    );
}

/// One short run of glyphs the static face really has, so no system fallback
/// face (machine-dependent) joins the measurement.
fn hello(ctx: &mut TextContext, family: &str) -> Vec<GlyphRun> {
    let mut style = TextStyle::new(FONT_SIZE, Color::BLACK);
    style.family = FontFamily::named(family);
    ctx.layout("Hello, Hello", &style, None)
        .to_scene_runs(Point::new(8.0, 8.0))
}

/// A run that is both faux bold and faux oblique, drawn under a 2x uniform
/// transform, is emboldened by the amount of the size it is drawn at.
///
/// Hinted, the stroke is a fixed fraction of the drawn size, so the share of
/// ink it adds holds as the scale doubles. Unhinted, the stroke is asserted
/// only to be present here; its weight is `faux_bold_weight_is_comparable_*`.
#[test]
fn faux_bold_and_oblique_under_a_2x_transform_is_emboldened() {
    let (mut ctx, family) = context_with(STATIC_FONT);
    let upright = hello(&mut ctx, &family);
    assert_eq!(upright.len(), 1, "the face covers the text");
    let with = |embolden: f32| -> Vec<GlyphRun> {
        upright
            .iter()
            .map(|run| GlyphRun {
                font: FontHandle::clone(&run.font).with_synthesis(embolden, 14.0),
                ..run.clone()
            })
            .collect()
    };
    let (plain, bold) = (with(0.0), with(0.04));
    for hint in [false, true] {
        let ink_at = |runs: &[GlyphRun], scale: f64| {
            let mut compiler = SceneCompiler::new(VIEWPORT.0 * 2, VIEWPORT.1 * 2);
            compiler.set_synthetic_oblique(true);
            compiler.set_image_residency(ImageResidency::disabled(AtlasBudget::MOBILE));
            compiler.set_hint_text(hint);
            let frame = compiler
                .compile(
                    &scene_of(runs),
                    Affine::scale(scale),
                    (VIEWPORT.0 * 2, VIEWPORT.1 * 2),
                )
                .expect("compiles");
            assert!(frame.glyph_draws > 0);
            ink(&frame)
        };
        let share = |scale: f64| {
            let (plain, bold) = (ink_at(&plain, scale), ink_at(&bold, scale));
            assert!(bold > plain, "hint {hint}: no embolden at {scale}x");
            (bold - plain) as f64 / plain as f64
        };
        let (share_1x, share_2x) = (share(1.0), share(2.0));
        eprintln!("hint {hint}: embolden adds {share_1x:.4} of the ink at 1x, {share_2x:.4} at 2x");
        if hint {
            // The scale is absorbed into the font size, so the 24 px run under
            // 2x is the 48 px run under 1x: same stroke, same ink.
            let doubled: Vec<GlyphRun> = bold
                .iter()
                .map(|run| {
                    let [a, b, c, d, e, f] = run.transform.as_coeffs();
                    GlyphRun {
                        font_size: run.font_size * 2.0,
                        transform: Affine::new([a, b, c, d, e * 2.0, f * 2.0]),
                        glyphs: run
                            .glyphs
                            .iter()
                            .map(|glyph| frust_scene::Glyph {
                                id: glyph.id,
                                x: glyph.x * 2.0,
                                y: glyph.y * 2.0,
                            })
                            .collect(),
                        ..run.clone()
                    }
                })
                .collect();
            let (at_2x, at_48) = (ink_at(&bold, 2.0), ink_at(&doubled, 1.0));
            eprintln!("hint {hint}: 24 px under 2x inks {at_2x}, 48 px under 1x {at_48}");
            let gap = at_2x.abs_diff(at_48) as f64 / at_48 as f64;
            assert!(gap < 0.02, "embolden did not follow the drawn size: {gap}");
        }
    }
}

/// A faux-bold run under a 2x transform computes the same stroke in the route
/// key and in the resident identity: it settles into the resident cache, which
/// a disagreement between the two would prevent.
#[test]
fn a_faux_bold_run_under_a_2x_transform_becomes_resident() {
    let (mut ctx, family) = context_with(STATIC_FONT);
    let bold = hello(&mut ctx, &family)
        .into_iter()
        .map(|run| GlyphRun {
            font: FontHandle::clone(&run.font).with_synthesis(0.04, 0.0),
            ..run
        })
        .collect::<Vec<_>>();
    for hint in [false, true] {
        let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
        compiler.set_glyph_quads(true);
        compiler.set_glyph_run_cache(true);
        compiler.set_hint_text(hint);
        let mut resident = 0;
        for _ in 0..6 {
            resident = compiler
                .compile(&scene_of(&bold), Affine::scale(2.0), VIEWPORT)
                .expect("compiles")
                .resident_glyph_runs;
        }
        assert!(resident > 0, "hint {hint}: never resident at 2x");
    }
}

/// A face reporting more coordinates than the key holds is drawn as outlines
/// and counted.
#[test]
fn a_face_with_more_than_eight_axes_is_counted_as_an_outline_run() {
    let (mut ctx, family) = context_with(VARIABLE_FONT);
    let runs: Vec<GlyphRun> = laid_out(&mut ctx, &family, FontWeight::REGULAR)
        .into_iter()
        .map(|run| GlyphRun {
            font: FontHandle::clone(&run.font)
                .with_normalized_coords(&[0, 0, 0, 0, 0, 0, 0, 0, 1024]),
            ..run
        })
        .collect();
    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    compiler.set_glyph_quads(true);
    compiler.set_glyph_run_cache(true);
    for _ in 0..3 {
        let frame = compile(&mut compiler, &runs);
        assert_eq!(frame.variation_axes_outline_runs as usize, runs.len());
        assert_eq!(frame.atlas_glyph_draws, 0);
        assert!(frame.glyph_draws > 0);
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
        let regular_runs = hello(&mut ctx, &family);
        let regular = inked(&pixels(&regular_runs));
        let bold = inked(&pixels(&faux_bold(&regular_runs)));
        println!("gpu static regular: {regular}, faux bold: {bold}");
        assert!(
            bold > regular,
            "faux bold inks {bold} pixels, regular {regular}"
        );
    }

    /// Synthetic oblique is off by default, and the renderer exposes no switch
    /// for its compiler, so an obliqued run renders upright: its ink's lean
    /// matches the upright run's.
    #[test]
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test variable_route -- --ignored`"]
    fn a_faux_oblique_run_renders_upright_through_the_gpu_by_default() {
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
        assert!(
            (leaning - flat).abs() < 0.25,
            "oblique {leaning}, upright {flat}"
        );
    }
}
