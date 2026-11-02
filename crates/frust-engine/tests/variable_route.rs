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

/// The frame's whole coverage, in 1/255 pixels: [`ink`] plus the solid spans
/// between strips. A strip carries alpha only along an edge; the interior of a
/// stem wider than a strip is a fill gap with no alpha, so [`ink`] grows with
/// a glyph's perimeter and under-counts it more the larger it is drawn. Share
/// comparisons across scales need the area.
fn area(frame: &CompiledFrame) -> u64 {
    let strips = frame.strip_buf();
    let mut total = ink(frame);
    for pair in strips.windows(2) {
        let (strip, next) = (&pair[0], &pair[1]);
        if strip.x == u16::MAX || next.alpha_idx() < strip.alpha_idx() {
            continue;
        }
        let end = u32::from(strip.x) + (next.alpha_idx() - strip.alpha_idx()) / 4;
        if next.fill_gap() && next.y == strip.y && u32::from(next.x) > end {
            total += u64::from(u32::from(next.x) - end) * 4 * 255;
        }
    }
    total
}

/// How far right the ink of the frame's top half sits of its bottom half, in
/// pixels: the centroid of the edge coverage in the rows above the ink's
/// vertical middle minus the centroid below it. Upright text reads a small
/// negative number (the glyph shapes), a forward lean pushes it up.
fn lean(frame: &CompiledFrame) -> f64 {
    let (strips, alphas) = (frame.strip_buf(), frame.alphas());
    // (y, x, coverage) of every covered pixel the strips carry.
    let mut samples = Vec::new();
    for pair in strips.windows(2) {
        let (strip, next) = (&pair[0], &pair[1]);
        if strip.x == u16::MAX || next.alpha_idx() < strip.alpha_idx() {
            continue;
        }
        let columns = (next.alpha_idx() - strip.alpha_idx()) as usize / 4;
        for column in 0..columns {
            for row in 0..4 {
                let coverage = alphas[strip.alpha_idx() as usize + column * 4 + row];
                if coverage != 0 {
                    samples.push((
                        f64::from(strip.y) + row as f64,
                        f64::from(strip.x) + column as f64,
                        f64::from(coverage),
                    ));
                }
            }
        }
    }
    assert!(!samples.is_empty(), "the frame carries no coverage");
    let (min, max) = samples.iter().fold((f64::MAX, f64::MIN), |(lo, hi), s| {
        (lo.min(s.0), hi.max(s.0))
    });
    let middle = (min + max) / 2.0;
    let (mut top, mut bottom) = ((0.0, 0.0), (0.0, 0.0));
    for (y, x, weight) in samples {
        let half = if y < middle { &mut top } else { &mut bottom };
        half.0 += weight * x;
        half.1 += weight;
    }
    top.0 / top.1 - bottom.0 / bottom.1
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
    // Measured on the strip path: 133888 at 400, 213160 at 700 (+0.592).
    let gain = bold_ink as f64 / light_ink as f64 - 1.0;
    assert!(
        (0.40..=0.80).contains(&gain),
        "700 inks {bold_ink}, 400 {light_ink}: gain {gain:.3}, measured 0.592, band 0.40..0.80"
    );
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
    // Measured on the strip path: 156101 regular, 227905 faux bold (+0.460).
    let gain = bold_ink as f64 / regular_ink as f64 - 1.0;
    assert!(
        (0.30..=0.65).contains(&gain),
        "faux bold {bold_ink} vs regular {regular_ink}: gain {gain:.3}, measured 0.460, band 0.30..0.65"
    );
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

/// `glifo` still hints a run under a horizontal-only shear (it hints
/// vertically only), so the faux-bold stroke is measured in device pixels
/// there too. A shear preserves area, so the sheared faux-bold run inks about
/// what the unsheared one does — not the blob a font-unit stroke would make.
#[test]
fn hinted_faux_bold_under_a_horizontal_shear_keeps_its_weight() {
    let (mut ctx, family) = context_with(STATIC_FONT);
    let regular = hello(&mut ctx, &family);
    let bold = faux_bold(&regular);
    let sheared = Affine::new([1.0, 0.0, 0.25, 1.0, 0.0, 0.0]);
    let ink_under = |runs: &[GlyphRun], transform: Affine| {
        let mut compiler = strip_compiler();
        compiler.set_hint_text(true);
        let frame = compiler
            .compile(&scene_of(runs), transform, VIEWPORT)
            .expect("an in-range scene compiles");
        assert!(frame.glyph_draws > 0, "the run drew nothing");
        ink(&frame)
    };
    let (upright_bold, sheared_bold) = (
        ink_under(&bold, Affine::IDENTITY),
        ink_under(&bold, sheared),
    );
    let sheared_regular = ink_under(&regular, sheared);
    eprintln!(
        "hinted faux bold ink upright {upright_bold}, sheared {sheared_bold}; sheared regular {sheared_regular}"
    );
    let ratio = sheared_bold as f64 / upright_bold as f64;
    assert!(
        (0.8..=1.25).contains(&ratio),
        "sheared/upright faux bold {ratio}"
    );
    assert!(
        sheared_bold as f64 >= sheared_regular as f64 * 1.25,
        "sheared faux bold {sheared_bold} vs regular {sheared_regular}"
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
    eprintln!(
        "resident regular runs {resident_regular} of {}",
        regular.len()
    );
    assert_eq!(
        resident_regular as usize,
        regular.len(),
        "every regular run settles resident (measured: all of them)"
    );
    // The bold run's first frame cannot be a resident hit.
    let first_bold = compile(&mut compiler, &bold);
    assert_eq!(first_bold.resident_glyph_runs, 0, "served the 400 record");
    // And the regular run is still its own record afterwards.
    let mut back = 0;
    for _ in 0..3 {
        back = compile(&mut compiler, &regular).resident_glyph_runs;
    }
    eprintln!("regular runs resident again after the bold frame: {back}");
    assert_eq!(
        back as usize,
        regular.len(),
        "the regular record survived the bold frame (measured: all runs resident)"
    );
    // The bold run then settles into a record of its own.
    let mut settled_bold = 0;
    for _ in 0..6 {
        settled_bold = compile(&mut compiler, &bold).resident_glyph_runs;
    }
    assert_eq!(
        settled_bold as usize,
        bold.len(),
        "the bold run settles with its own record"
    );
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
    // A 14 degree skew about the baseline moves the top of the ink right of
    // its bottom: the lean grows by roughly tan(14 deg) times half the ink's
    // height. Measured on the strip path (upright, skewed, growth) below.
    let (flat, leaning) = (lean(&upright_frame), lean(&skewed_frame));
    let growth = leaning - flat;
    eprintln!("lean upright {flat:.3}, 14 degree oblique {leaning:.3}, growth {growth:.3}");
    assert!(
        (1.0..=4.0).contains(&growth),
        "oblique lean {leaning:.3} vs upright {flat:.3}: growth {growth:.3}, measured 1.800 (upright -8.433, oblique -6.632), band 1.0..4.0 px"
    );

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

/// The cost of faux-italic text, in counters rather than clock time: on every
/// frame the italic paragraph draws all of its glyphs as outlines, where the
/// same paragraph upright, once settled, draws none of them as outlines and
/// all of them out of the atlas. This replaces a wall-clock ratio (a
/// machine-load-dependent bound that needed a TESTING.md quarantine record).
#[test]
fn an_italic_paragraph_compile_cost_is_bounded() {
    let (mut ctx, family) = context_with(STATIC_FONT);
    let italic = paragraph(&mut ctx, &family, FontStyle::Italic);
    let upright = paragraph(&mut ctx, &family, FontStyle::Normal);

    let mut compiler = screen_compiler();
    compiler.set_synthetic_oblique(true);
    let italic_frames: Vec<CompiledFrame> = (0..4)
        .map(|_| compile_screen(&mut compiler, &italic))
        .collect();
    let mut compiler = screen_compiler();
    let upright_frames: Vec<CompiledFrame> = (0..4)
        .map(|_| compile_screen(&mut compiler, &upright))
        .collect();
    let (italic_last, upright_last) = (&italic_frames[3], &upright_frames[3]);
    eprintln!(
        "italic: glyph_draws {} atlas {} oblique runs {} ; upright settled: glyph_draws {} atlas {} resident {}",
        italic_last.glyph_draws,
        italic_last.atlas_glyph_draws,
        italic_last.synthetic_oblique_runs,
        upright_last.glyph_draws,
        upright_last.atlas_glyph_draws,
        upright_last.resident_glyph_runs,
    );
    // `glyph_draws` counts every glyph drawn, atlas or outline; what is not an
    // atlas draw is an outline draw. Measured: 1215 glyphs in both paragraphs.
    let outline_draws = |frame: &CompiledFrame| frame.glyph_draws - frame.atlas_glyph_draws;
    // The outline cost is the same on every frame: it never amortises.
    for (index, frame) in italic_frames.iter().enumerate() {
        assert_eq!(
            outline_draws(frame),
            1215,
            "italic frame {index} outline draws (measured: all 1215 glyphs)"
        );
        assert_eq!(frame.atlas_glyph_draws, 0, "italic frame {index}");
    }
    assert_eq!(italic_last.synthetic_oblique_runs as usize, italic.len());
    // Upright text settles: outline draws fall to the atlas.
    assert!(
        upright_last.atlas_glyph_draws > 0,
        "upright text never settled"
    );
    assert_eq!(
        outline_draws(upright_last),
        0,
        "settled upright text still outline-draws (measured: 0 of 1215, all 1215 from the atlas)"
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

/// `runs` redrawn at twice the font size, twice the offsets and the same
/// transform: the 48 px run a 24 px run under a 2x transform stands for.
fn doubled_runs(runs: &[GlyphRun]) -> Vec<GlyphRun> {
    runs.iter()
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
        .collect()
}

/// A run that is both faux bold and faux oblique, drawn under a 2x uniform
/// transform, is emboldened by the amount of the size it is drawn at.
///
/// Hinted and unhinted alike the stroke is a fixed fraction of the drawn size,
/// so the share of area it adds holds as the scale doubles.
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
        let area_at = |runs: &[GlyphRun], scale: f64| {
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
            area(&frame)
        };
        let share = |scale: f64| {
            let (plain, bold) = (area_at(&plain, scale), area_at(&bold, scale));
            assert!(bold > plain, "hint {hint}: no embolden at {scale}x");
            (bold - plain) as f64 / plain as f64
        };
        let (share_1x, share_2x) = (share(1.0), share(2.0));
        eprintln!(
            "hint {hint}: embolden adds {share_1x:.4} of the area at 1x, {share_2x:.4} at 2x"
        );
        // Measured (oblique route, embolden 0.04, whole-area coverage):
        // hinted 0.5014 at 1x and 0.5242 at 2x; unhinted 0.5150 and 0.5100.
        // (Edge-only `ink` would read 0.3507 and 0.3168 at 2x: interior fill
        // gaps carry no alpha, which is why shares use `area`.)
        let ratio = share_2x / share_1x;
        for (share, scale) in [(share_1x, 1), (share_2x, 2)] {
            assert!(
                (0.40..=0.62).contains(&share),
                "hint {hint}: embolden share {share:.4} at {scale}x, band 0.40..0.62"
            );
        }
        assert!(
            (0.85..=1.15).contains(&ratio),
            "hint {hint}: share {share_1x:.4} at 1x, {share_2x:.4} at 2x, ratio {ratio:.3}, measured 0.5150/0.5100 unhinted and 0.5014/0.5242 hinted, band 0.85..1.15"
        );
        if hint {
            // The scale is absorbed into the font size, so the 24 px run under
            // 2x is the 48 px run under 1x: same stroke, same ink.
            let doubled = doubled_runs(&bold);
            let (at_2x, at_48) = (area_at(&bold, 2.0), area_at(&doubled, 1.0));
            eprintln!("hint {hint}: 24 px under 2x inks {at_2x}, 48 px under 1x {at_48}");
            let gap = at_2x.abs_diff(at_48) as f64 / at_48 as f64;
            assert!(gap < 0.02, "embolden did not follow the drawn size: {gap}");
        }
    }
}

/// Unhinted, `glifo` sizes the stroke in font units, so the engine's basis is
/// the face's `unitsPerEm`, which the transform scales with the outline: the
/// share of ink the stroke adds at 2x is comparable to the share at 1x. A
/// basis that ignored the scale would add the same absolute stroke to a
/// doubled outline and the share would fall by half or more.
#[test]
fn an_unhinted_embolden_share_holds_from_1x_to_2x() {
    let (mut ctx, family) = context_with(STATIC_FONT);
    let regular = hello(&mut ctx, &family);
    let bold = faux_bold(&regular);
    let ink_at = |runs: &[GlyphRun], scale: f64| {
        let size = (VIEWPORT.0 * 2, VIEWPORT.1 * 2);
        let mut compiler = SceneCompiler::new(size.0, size.1);
        compiler.set_image_residency(ImageResidency::disabled(AtlasBudget::MOBILE));
        compiler.set_hint_text(false);
        let frame = compiler
            .compile(&scene_of(runs), Affine::scale(scale), size)
            .expect("an in-range scene compiles");
        assert!(frame.glyph_draws > 0, "the run drew nothing");
        area(&frame)
    };
    let share = |scale: f64| {
        let (plain, bold) = (ink_at(&regular, scale), ink_at(&bold, scale));
        eprintln!("unhinted {scale}x: plain {plain}, bold {bold}");
        (bold - plain) as f64 / plain as f64
    };
    let (share_1x, share_2x) = (share(1.0), share(2.0));
    let ratio = share_2x / share_1x;
    eprintln!("unhinted share 1x {share_1x:.4}, 2x {share_2x:.4}, ratio {ratio:.3}");
    assert!(
        (0.35..=0.60).contains(&share_1x),
        "unhinted share at 1x {share_1x:.4}, measured 0.4600, band 0.35..0.60"
    );
    assert!(
        (0.85..=1.15).contains(&ratio),
        "unhinted share 1x {share_1x:.4}, 2x {share_2x:.4}, ratio {ratio:.3}, measured 0.4600 at 1x, 0.4525 at 2x (ratio 0.984), band 0.85..1.15"
    );
}

/// A faux-bold run under a 2x transform computes the same stroke in the route
/// key and in the resident identity: it settles into the resident cache, which
/// a disagreement between the two would prevent.
#[test]
fn a_faux_bold_run_under_a_2x_transform_becomes_resident() {
    let (mut ctx, family) = context_with(STATIC_FONT);
    let plain = hello(&mut ctx, &family);
    let bold = plain
        .iter()
        .map(|run| GlyphRun {
            font: FontHandle::clone(&run.font).with_synthesis(0.04, 0.0),
            ..run.clone()
        })
        .collect::<Vec<_>>();
    for hint in [false, true] {
        let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
        compiler.set_glyph_quads(true);
        compiler.set_glyph_run_cache(true);
        compiler.set_hint_text(hint);
        let mut at_2x = |runs: &[GlyphRun]| {
            compiler
                .compile(&scene_of(runs), Affine::scale(2.0), VIEWPORT)
                .expect("compiles")
                .resident_glyph_runs
        };
        // The run without the stroke settles first, so the bold run's key is
        // tested against a resident record that differs from it only in the
        // stroke.
        let mut plain_resident = 0;
        for _ in 0..6 {
            plain_resident = at_2x(&plain);
        }
        assert_eq!(
            plain_resident as usize,
            plain.len(),
            "hint {hint}: plain run"
        );
        let first_bold = at_2x(&bold);
        assert_eq!(
            first_bold, 0,
            "hint {hint}: the embolden run was served the unembolden record"
        );
        let mut resident = 0;
        for _ in 0..6 {
            resident = at_2x(&bold);
        }
        eprintln!("hint {hint}: plain resident {plain_resident}, bold resident {resident}");
        assert_eq!(
            resident as usize,
            bold.len(),
            "hint {hint}: never resident at 2x (measured: every run)"
        );
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
        pixels_under(runs, Affine::IDENTITY, false)
    }

    /// [`pixels`] under `transform`, with synthetic oblique switched on or off.
    fn pixels_under(runs: &[GlyphRun], transform: Affine, oblique: bool) -> Vec<u8> {
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
        renderer.set_synthetic_oblique(oblique);
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
                transform,
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
        // Measured on Metal: 816 pixels at 400, 1115 at 700 (+0.366).
        let gain = bold as f64 / regular as f64 - 1.0;
        assert!(
            (0.20..=0.60).contains(&gain),
            "700 inks {bold} pixels, 400 {regular}: gain {gain:.3}, measured 0.366, band 0.20..0.60"
        );
    }

    #[test]
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test variable_route -- --ignored`"]
    fn a_faux_bold_static_face_inks_more_through_the_gpu() {
        let (mut ctx, family) = context_with(STATIC_FONT);
        let regular_runs = hello(&mut ctx, &family);
        let regular = inked(&pixels(&regular_runs));
        let bold = inked(&pixels(&faux_bold(&regular_runs)));
        println!("gpu static regular: {regular}, faux bold: {bold}");
        // Measured on Metal: 933 pixels regular, 1274 faux bold (+0.365).
        let gain = bold as f64 / regular as f64 - 1.0;
        assert!(
            (0.20..=0.60).contains(&gain),
            "faux bold inks {bold} pixels, regular {regular}: gain {gain:.3}, measured 0.365, band 0.20..0.60"
        );
    }

    /// The faux-bold size, not just its presence: the ink gain is in a band
    /// both for a hinted run (the desktop default, an upright transform) and
    /// for an unhinted one (`glifo` cannot absorb a vertical shear, so it draws
    /// the outline at `unitsPerEm`).
    #[test]
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test variable_route -- --ignored`"]
    fn faux_bold_inks_a_calibrated_share_hinted_and_unhinted_through_the_gpu() {
        let (mut ctx, family) = context_with(STATIC_FONT);
        let regular = hello(&mut ctx, &family);
        let bold = faux_bold(&regular);
        let vertical_shear = Affine::new([1.0, 0.05, 0.0, 1.0, 0.0, 0.0]);
        for (label, transform) in [("hinted", Affine::IDENTITY), ("unhinted", vertical_shear)] {
            let plain = inked(&pixels_under(&regular, transform, false));
            let heavy = inked(&pixels_under(&bold, transform, false));
            let gain = heavy as f64 / plain as f64 - 1.0;
            println!("gpu {label}: regular {plain}, faux bold {heavy}, gain {gain:.3}");
            assert!(
                (0.25..=0.50).contains(&gain),
                "{label}: faux bold {heavy} pixels, regular {plain}: gain {gain:.3}, measured hinted 0.365, unhinted 0.309, band 0.25..0.50"
            );
        }
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
        // Measured on Metal: -7.183 for both (identical pixels).
        assert!(
            (leaning - flat).abs() < 0.25,
            "oblique {leaning}, upright {flat}: measured -7.183 for both, bound 0.25 px"
        );
        // Switched on, the same run leans.
        let leaning_on = lean(&pixels_under(&skewed, Affine::IDENTITY, true));
        let growth = leaning_on - flat;
        println!("gpu lean with the switch on: {leaning_on:.3}, growth {growth:.3}");
        assert!(
            (1.0..=5.0).contains(&growth),
            "switch on: lean {leaning_on:.3} vs upright {flat:.3}: growth {growth:.3}, measured upright -7.183, switch on -5.398, growth 1.785, band 1.0..5.0 px"
        );
    }
}
