//! Glyph runs in an `hvgl`-only face: Apple's PingFang, whose outlines live
//! in an `hvgl` table `glifo`'s `skrifa` cannot read.
//!
//! On Apple targets the engine fills such a run's glyphs from CoreText's own
//! outlines; everywhere else, and for a face CoreText cannot open faithfully,
//! every glyph is counted as skipped and nothing is drawn. What is pinned:
//!
//! - **A run is skipped whole when it cannot be opened.** A synthetic
//!   `hvgl`-only face (a `head` and an `hvgl` table, no outline table) whose
//!   handle names no face draws nothing and counts every glyph in
//!   `skipped_glyphs`, on every target; a target without CoreText cannot open
//!   any face, so there a named one is skipped whole too.
//! - **Identity is the text tier's call.** The engine opens the face by the
//!   PostScript name the handle carries and refuses nothing on identity: a
//!   handle with a hidden (dot-prefixed) name draws on Apple, and a name
//!   CoreText resolves to another face is counted once as an identity mismatch
//!   (and logged at `debug`), never refused.
//! - **On macOS the system PingFang draws.** A run of 中文爱 in
//!   `PingFangUI.ttc`'s `PingFangSC-Medium` face, glyph ids taken from
//!   CoreText's own character map, produces one draw per glyph with real
//!   coverage and skips nothing. The outline cache stays at its cap however
//!   many distinct glyphs are drawn. These cases print why and pass when the
//!   collection is not installed.
//! - **Through the GPU** (ignored by default, needs Metal): the same run read
//!   back from a headless target carries ink, and more of it at weight 600
//!   than at 300.
//!
//! And the variation coordinates a run is shaped at:
//!
//! - **`TextContext` carries them.** A variable `glyf` face laid out at
//!   weights 300 and 600 yields runs whose handles carry distinct
//!   coordinates, and none at the face's default weight. `glifo`'s route still
//!   draws such a run exactly as it did before coordinates were carried.
//! - **On macOS PingFang draws at the requested weight.** 中 at weights 300
//!   and 600 in PingFang SC compiles to different coverage. The name and the
//!   coordinates come from `TextContext` when its oracle resolves the `hvgl`
//!   face itself; otherwise the case prints so and names the face and derives
//!   the coordinates from its `fvar`/`avar`.

use std::sync::Arc;

use frust_engine::cache::images::ImageResidency;
use frust_engine::compile::CompiledFrame;
use frust_engine::{AtlasBudget, SceneCompiler};
use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};
use frust_text::{FontFamily, FontWeight, TextContext, TextStyle};
use kurbo::{Affine, Point};
use peniko::{Blob, Brush, Color, FontData};

/// Viewport every case compiles against.
const VIEWPORT: (u16, u16) = (128, 64);

/// Font size every case draws at.
const FONT_SIZE: f32 = 24.0;

/// A variable `glyf` face (`wght` 100..900, default 400) that fontique
/// accepts, so `TextContext` shapes it at a requested weight.
const VARIABLE_FONT: &[u8] = include_bytes!("../../../plugins/beui/fonts/geist/Geist-Variable.ttf");

/// A single-face sfnt holding `tables` (tag, bytes), table data 4-aligned.
fn sfnt(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let count = u16::try_from(tables.len()).expect("a handful of tables");
    let mut out = vec![0x00, 0x01, 0x00, 0x00];
    out.extend(count.to_be_bytes());
    out.extend([0; 6]);
    let mut offset = 12 + 16 * tables.len();
    let mut data = Vec::new();
    for (tag, bytes) in tables {
        out.extend(tag);
        out.extend([0; 4]);
        out.extend(u32::try_from(offset).expect("small").to_be_bytes());
        out.extend(u32::try_from(bytes.len()).expect("small").to_be_bytes());
        let padded = bytes.len().div_ceil(4) * 4;
        data.extend(bytes);
        data.resize(data.len() + padded - bytes.len(), 0);
        offset += padded;
    }
    out.extend(data);
    out
}

/// A 54-byte `head` table carrying `units_per_em`.
fn head(units_per_em: u16) -> Vec<u8> {
    let mut head = vec![0; 54];
    head[18..20].copy_from_slice(&units_per_em.to_be_bytes());
    head
}

/// An `hvgl`-only face with no outline table at all, identified as
/// `postscript_name` (`None`: the handle names no face) the way the text
/// tier's oracle identifies one.
fn synthetic_hvgl_face(postscript_name: Option<&str>, hidden: bool) -> FontHandle {
    let bytes = sfnt(&[(*b"head", head(1000)), (*b"hvgl", vec![0; 16])]);
    FontHandle::new(FontData::new(Blob::new(Arc::new(bytes)), 0))
        .with_identity(postscript_name.map(Arc::from), hidden)
}

/// A run of `ids` on one baseline, inside the viewport.
fn run(font: FontHandle, ids: &[u32]) -> GlyphRun {
    GlyphRun {
        font,
        font_size: FONT_SIZE,
        brush: Brush::Solid(Color::BLACK),
        transform: Affine::translate((8.0, 44.0)),
        glyphs: ids
            .iter()
            .enumerate()
            .map(|(index, id)| Glyph {
                id: *id,
                x: index as f32 * FONT_SIZE,
                y: 0.0,
            })
            .collect(),
    }
}

fn scene_of(runs: Vec<GlyphRun>) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    for glyph_run in runs {
        builder.draw_glyph_run(glyph_run);
    }
    scene
}

/// A compiler with the atlas left on: an `hvgl` run must take the outline
/// route on its own, not because the atlas happened to be off.
fn compiler() -> SceneCompiler {
    SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
}

fn compile(compiler: &mut SceneCompiler, runs: Vec<GlyphRun>) -> CompiledFrame {
    compiler
        .compile(&scene_of(runs), Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles")
}

/// Asserts `runs` draw from their faces with no identity refusal: every face
/// resolved to the name asked for, and none was nameless. Checked on a fresh
/// compiler, since the counters live there.
#[cfg(target_os = "macos")]
fn assert_identity_clean(runs: &[GlyphRun]) {
    let mut compiler = compiler();
    compile(&mut compiler, runs.to_vec());
    assert_eq!(
        compiler.hvgl_identity_mismatches(),
        0,
        "a face resolved to another PostScript name, so its glyphs were refused"
    );
    assert_eq!(
        compiler.hvgl_nameless_faces(),
        0,
        "a run's face carried no PostScript name"
    );
}

/// `text` laid out through a fresh `TextContext` in `family` at `weight`, as
/// scene runs inside the viewport.
fn laid_out(ctx: &mut TextContext, family: &str, weight: FontWeight, text: &str) -> Vec<GlyphRun> {
    let mut style = TextStyle::new(FONT_SIZE, Color::BLACK);
    style.family = FontFamily::named(family);
    style.weight = weight;
    ctx.layout(text, &style, None)
        .to_scene_runs(Point::new(8.0, 8.0))
}

/// The variable face registered into a fresh context, and its family name.
fn variable_font_context() -> (TextContext, String) {
    let mut ctx = TextContext::new();
    let families = ctx
        .register_fonts(VARIABLE_FONT.to_vec())
        .expect("the variable face registers");
    let family = families[0].name.clone();
    (ctx, family)
}

#[test]
fn text_context_runs_carry_the_variation_coordinates_they_were_shaped_at() {
    let (mut ctx, family) = variable_font_context();
    let mut coords = |weight| {
        let runs = laid_out(&mut ctx, &family, weight, "Hamburg");
        assert_eq!(runs.len(), 1, "one face, one run");
        runs[0].font.normalized_coords().to_vec()
    };
    let light = coords(FontWeight::LIGHT);
    let semibold = coords(FontWeight::SEMI_BOLD);
    assert!(!light.is_empty(), "weight 300 is not the default instance");
    assert!(
        !semibold.is_empty(),
        "weight 600 is not the default instance"
    );
    assert_ne!(light, semibold);
    assert!(light[0] < 0 && semibold[0] > 0, "{light:?} {semibold:?}");
    assert!(
        coords(FontWeight::REGULAR).is_empty(),
        "the face's default weight carries no coordinates"
    );
}

/// `glifo`'s route is not handed the coordinates: a run carrying them draws
/// exactly what the same run without them draws.
#[test]
fn a_glifo_run_draws_the_same_with_or_without_coordinates() {
    let (mut ctx, family) = variable_font_context();
    let varied = laid_out(&mut ctx, &family, FontWeight::SEMI_BOLD, "Hamburg");
    assert!(!varied[0].font.normalized_coords().is_empty());
    let plain: Vec<GlyphRun> = varied
        .iter()
        .map(|run| GlyphRun {
            font: FontHandle::new(run.font.font().clone()),
            ..run.clone()
        })
        .collect();
    let varied_frame = compile(&mut compiler(), varied);
    let plain_frame = compile(&mut compiler(), plain);
    assert!(varied_frame.glyph_draws > 0);
    assert_eq!(varied_frame.glyph_draws, plain_frame.glyph_draws);
    assert_eq!(varied_frame.skipped_glyphs, plain_frame.skipped_glyphs);
    assert_eq!(varied_frame.alphas(), plain_frame.alphas());
    assert_eq!(varied_frame.strip_buf(), plain_frame.strip_buf());
}

#[test]
fn the_compiler_stays_send_and_sync_with_the_hvgl_provider_inside() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SceneCompiler>();
}

/// A handle that names no face cannot be opened: the engine does not derive a
/// name from the blob. Its glyphs are counted as skipped, whole, on every
/// target.
#[test]
fn an_hvgl_handle_without_an_identity_is_skipped_whole_and_draws_nothing() {
    let mut compiler = compiler();
    let face = synthetic_hvgl_face(None, false);
    for frame_index in 0..2 {
        let frame = compile(&mut compiler, vec![run(face.clone(), &[1, 2, 3])]);
        assert_eq!(frame.skipped_glyphs, 3, "frame {frame_index}");
        assert_eq!(frame.glyph_draws, 0, "frame {frame_index}");
        assert!(frame.draws().is_empty(), "frame {frame_index}");
        assert_eq!(frame.atlas_glyph_draws, 0, "frame {frame_index}");
    }
    assert_eq!(compiler.hvgl_cached_outlines(), 0);
    assert_eq!(compiler.hvgl_identity_mismatches(), 0);
    assert_eq!(
        compiler.hvgl_nameless_faces(),
        1,
        "counted once, not per frame"
    );
}

/// A dot-prefixed name is the shape of Apple's hidden system faces. CoreText
/// cannot resolve a hidden name to the face (it hands back a substitute), so
/// the engine refuses it: skipped, and counted once.
#[test]
fn a_hidden_named_hvgl_handle_is_refused_when_coretext_substitutes() {
    let mut compiler = compiler();
    let frame = compile(
        &mut compiler,
        vec![run(
            synthetic_hvgl_face(Some(".PingFangUITextSC-Default"), true),
            &[10, 11],
        )],
    );
    if cfg!(target_vendor = "apple") {
        assert_eq!(compiler.hvgl_identity_mismatches(), 1);
    } else {
        assert_eq!(compiler.hvgl_identity_mismatches(), 0);
    }
    assert_eq!(frame.skipped_glyphs, 2, "never drawn from a substitute");
    assert_eq!(frame.glyph_draws, 0);
    assert!(frame.draws().is_empty());
}

/// An unknown name CoreText substitutes for is refused: the mismatch is
/// counted once however many frames ask, and every glyph is skipped.
#[cfg(target_vendor = "apple")]
#[test]
fn a_name_coretext_resolves_differently_is_counted_once_and_skipped() {
    let mut compiler = compiler();
    let face = synthetic_hvgl_face(Some("FrustNoSuchFace-Regular"), false);
    for _ in 0..3 {
        let frame = compile(&mut compiler, vec![run(face.clone(), &[10, 11])]);
        assert_eq!(frame.skipped_glyphs, 2);
    }
    assert_eq!(compiler.hvgl_identity_mismatches(), 1);
}

/// The cap is a fixed constant, not zero: a compiler that kept nothing would
/// pass the eviction case trivially.
#[test]
fn the_hvgl_outline_cache_has_a_real_cap() {
    let compiler = compiler();
    assert!(compiler.hvgl_outline_capacity() >= 256);
    assert_eq!(compiler.hvgl_cached_outlines(), 0);
}

/// An image residency switched off must not change an `hvgl` run's answer:
/// it never asked the atlas for anything.
#[test]
fn an_hvgl_run_is_skipped_identically_with_the_atlas_off() {
    let mut compiler = compiler();
    compiler.set_image_residency(ImageResidency::disabled(AtlasBudget::MOBILE));
    let frame = compile(
        &mut compiler,
        vec![run(synthetic_hvgl_face(None, false), &[4, 5])],
    );
    assert_eq!(frame.skipped_glyphs, 2);
    assert_eq!(frame.glyph_draws, 0);
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use frust_engine::compile::CompiledFrame;
    use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
    use frust_gpu::{HeadlessTarget, TierCaps};
    use frust_scene::{FontHandle, GlyphRun};
    use frust_text::{FontWeight, TextContext};
    use kurbo::Affine;
    use objc2_core_foundation::CFString;
    use objc2_core_text::CTFont;
    use peniko::{Blob, Color, FontData};

    use super::{VIEWPORT, assert_identity_clean, compile, compiler, laid_out, run, scene_of};

    /// Where macOS keeps the PingFang collection. It moved into a private
    /// framework's reserved fonts; older releases kept it in `Fonts`.
    const PINGFANG_PATHS: [&str; 2] = [
        "/System/Library/PrivateFrameworks/FontServices.framework/Versions/A/Resources/Reserved/PingFangUI.ttc",
        "/System/Library/Fonts/PingFangUI.ttc",
    ];

    /// The collection face drawn: a public (non-hidden) PostScript name.
    const FACE: &str = "PingFangSC-Medium";

    /// The text drawn — three Han characters PingFang covers.
    const TEXT: &str = "中文爱";

    /// The system collection's bytes, or `None` (with the reason printed)
    /// when it is not installed.
    fn pingfang() -> Option<Arc<Vec<u8>>> {
        let found = PINGFANG_PATHS
            .iter()
            .find_map(|path| std::fs::read(path).ok());
        if found.is_none() {
            println!("skipped: PingFangUI.ttc is not installed at any of {PINGFANG_PATHS:?}");
        }
        found.map(Arc::new)
    }

    fn be_u16(data: &[u8], at: usize) -> u16 {
        u16::from_be_bytes([data[at], data[at + 1]])
    }

    fn be_u32(data: &[u8], at: usize) -> u32 {
        u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
    }

    /// A variable `hvgl` face of the collection `data`, identified as `name`
    /// the way the oracle identifies it. The blob supplies the face's metrics
    /// and axes; the name alone picks the face CoreText draws.
    fn face_named(data: &Arc<Vec<u8>>, name: &str, hidden: bool) -> FontHandle {
        let index = (0..be_u32(data, 8))
            .find(|index| {
                table(data, *index, b"hvgl").is_some() && table(data, *index, b"fvar").is_some()
            })
            .expect("PingFangUI.ttc carries a variable hvgl face");
        FontHandle::new(FontData::new(Blob::new(data.clone()), index))
            .with_identity(Some(Arc::from(name)), hidden)
    }

    /// The public `PingFangSC-Medium` face of `data`.
    fn face(data: &Arc<Vec<u8>>, name: &str) -> FontHandle {
        face_named(data, name, false)
    }

    /// The table `tag` of the face at `index` in the font file `data` (a
    /// collection or a single face), as `(offset, length)`.
    fn table(data: &[u8], index: u32, tag: &[u8; 4]) -> Option<(usize, usize)> {
        let directory = if &data[0..4] == b"ttcf" {
            be_u32(data, 12 + 4 * index as usize) as usize
        } else {
            0
        };
        (0..be_u16(data, directory + 4) as usize)
            .map(|table| directory + 12 + 16 * table)
            .find(|record| &data[*record..*record + 4] == tag)
            .map(|record| {
                (
                    be_u32(data, record + 8) as usize,
                    be_u32(data, record + 12) as usize,
                )
            })
    }

    /// The normalized coordinates — `fvar` normalization, then the `avar`
    /// map, in F2Dot14 — of the instance at `weight` (every other axis at its
    /// default): what a shaper hands a run of `font` at that weight.
    fn coords_at_weight(font: &FontData, weight: f64) -> Vec<i16> {
        let data = font.data.data();
        let (fvar, _) = table(data, font.index, b"fvar").expect("PingFang varies");
        let first = fvar + be_u16(data, fvar + 4) as usize;
        let (count, size) = (
            be_u16(data, fvar + 8) as usize,
            be_u16(data, fvar + 10) as usize,
        );
        let fixed = |at: usize| f64::from(be_u32(data, at).cast_signed()) / 65536.0;
        let f2dot14 = |at: usize| f64::from(be_u16(data, at).cast_signed()) / 16384.0;
        let mut maps: Vec<Vec<(f64, f64)>> = vec![Vec::new(); count];
        if let Some((avar, _)) = table(data, font.index, b"avar") {
            let mut at = avar + 8;
            for map in &mut maps {
                let pairs = be_u16(data, at) as usize;
                *map = (0..pairs)
                    .map(|pair| (f2dot14(at + 2 + 4 * pair), f2dot14(at + 4 + 4 * pair)))
                    .collect();
                at += 2 + 4 * pairs;
            }
        }
        (0..count)
            .map(|axis| {
                let record = first + axis * size;
                if &data[record..record + 4] != b"wght" {
                    return 0;
                }
                let (min, default, max) =
                    (fixed(record + 4), fixed(record + 8), fixed(record + 12));
                let weight = weight.clamp(min, max);
                let mut normalized = if weight < default {
                    (weight - default) / (default - min)
                } else {
                    (weight - default) / (max - default)
                };
                if let Some(pair) = maps[axis]
                    .windows(2)
                    .find(|pair| normalized >= pair[0].0 && normalized <= pair[1].0)
                {
                    let [(from0, to0), (from1, to1)] = [pair[0], pair[1]];
                    normalized = to0 + (normalized - from0) * (to1 - to0) / (from1 - from0);
                }
                (normalized * 16384.0).round() as i16
            })
            .collect()
    }

    /// Runs of 中 in PingFang SC at `weight`, with the coordinates
    /// `TextContext` shapes them at when it resolves the `hvgl` face itself,
    /// else the face's own `fvar`/`avar`
    /// coordinates for that weight on a run of CoreText's glyph.
    fn pingfang_runs_at(
        data: &Arc<Vec<u8>>,
        ctx: Option<&mut TextContext>,
        weight: FontWeight,
        user_weight: f64,
    ) -> Vec<GlyphRun> {
        if let Some(ctx) = ctx {
            let runs = laid_out(ctx, "PingFang SC", weight, "中");
            if runs.iter().all(|run| {
                let font = run.font.font();
                table(font.data.data(), font.index, b"hvgl").is_some()
            }) {
                return runs;
            }
        }
        let font = face(data, FACE);
        let coords = coords_at_weight(font.font(), user_weight);
        vec![run(
            font.with_normalized_coords(&coords),
            &coretext_glyphs(FACE, "中"),
        )]
    }

    /// A `TextContext` that resolves PingFang SC to its `hvgl` face through
    /// the system font oracle, with the face's identity on every run. Fonts
    /// the caller registers carry no oracle identity, so none is registered:
    /// the system face is the one the engine must draw by name.
    fn text_context_resolves_pingfang() -> Option<TextContext> {
        let mut ctx = TextContext::new();
        let resolved = laid_out(&mut ctx, "PingFang SC", FontWeight::LIGHT, "中")
            .iter()
            .all(|run| {
                let font = run.font.font();
                table(font.data.data(), font.index, b"hvgl").is_some()
                    && run.font.postscript_name().is_some()
            });
        if !resolved {
            println!(
                "TextContext leg skipped: PingFang SC does not resolve to an identified hvgl \
                 face here; the coordinates come from PingFang's fvar/avar"
            );
        }
        resolved.then_some(ctx)
    }

    /// The oracle's identity reaches the run: the PostScript name of a public
    /// face, not hidden.
    #[test]
    fn text_context_pingfang_runs_carry_the_oracles_identity() {
        let Some(mut ctx) = text_context_resolves_pingfang() else {
            return;
        };
        let runs = laid_out(&mut ctx, "PingFang SC", FontWeight::SEMI_BOLD, "中");
        assert_identity_clean(&runs);
        for run in runs {
            let name = run.font.postscript_name().expect("identified");
            assert!(name.starts_with("PingFang"), "{name}");
            assert!(!run.font.is_hidden());
        }
    }

    fn pingfang_frame_at(data: &Arc<Vec<u8>>, weight: FontWeight, user: f64) -> CompiledFrame {
        let mut ctx = text_context_resolves_pingfang();
        let runs = pingfang_runs_at(data, ctx.as_mut(), weight, user);
        assert!(
            runs.iter()
                .all(|run| !run.font.normalized_coords().is_empty()),
            "weight {user} is not PingFang SC's default instance"
        );
        compile(&mut compiler(), runs)
    }

    #[test]
    fn pingfang_han_draws_at_the_requested_weight() {
        let Some(data) = pingfang() else { return };
        let light = pingfang_frame_at(&data, FontWeight::LIGHT, 300.0);
        let semibold = pingfang_frame_at(&data, FontWeight::SEMI_BOLD, 600.0);
        for frame in [&light, &semibold] {
            assert_eq!(frame.skipped_glyphs, 0);
            assert_eq!(frame.glyph_draws, 1);
            assert!(frame.alphas().iter().any(|alpha| *alpha != 0));
        }
        assert_ne!(
            light.alphas(),
            semibold.alphas(),
            "weights 300 and 600 draw different outlines"
        );
    }

    /// CoreText's own glyph ids for `text` in the installed face `name`.
    fn coretext_glyphs(name: &str, text: &str) -> Vec<u32> {
        let mut characters: Vec<u16> = text.encode_utf16().collect();
        let mut glyphs = vec![0_u16; characters.len()];
        // SAFETY: a null matrix is the documented identity argument.
        let font = unsafe { CTFont::with_name(&CFString::from_str(name), 24.0, std::ptr::null()) };
        // SAFETY: both buffers hold `characters.len()` elements.
        let mapped = unsafe {
            font.glyphs_for_characters(
                NonNull::new(characters.as_mut_ptr()).expect("non-empty"),
                NonNull::new(glyphs.as_mut_ptr()).expect("non-empty"),
                characters.len() as isize,
            )
        };
        assert!(
            mapped,
            "CoreText maps every character of {text:?} in {name}"
        );
        glyphs.into_iter().map(u32::from).collect()
    }

    #[test]
    fn pingfang_han_glyphs_draw_from_coretext_outlines() {
        let Some(data) = pingfang() else { return };
        let glyphs = coretext_glyphs(FACE, TEXT);
        assert!(glyphs.iter().all(|glyph| *glyph != 0), "{glyphs:?}");
        // One handle for every frame: the cache is keyed on the blob's
        // identity, as a shaped run's font is from frame to frame.
        let font = face(&data, FACE);
        let mut compiler = compiler();
        for frame_index in 0..2 {
            let frame = compile(&mut compiler, vec![run(font.clone(), &glyphs)]);
            assert_eq!(frame.skipped_glyphs, 0, "frame {frame_index}");
            assert_eq!(frame.glyph_draws, 3, "frame {frame_index}");
            assert_eq!(
                frame.atlas_glyph_draws, 0,
                "an hvgl run never samples the atlas"
            );
            assert!(
                frame.alphas().iter().any(|alpha| *alpha != 0),
                "frame {frame_index}: the glyphs carry coverage"
            );
            assert!(!frame.strip_buf().is_empty(), "frame {frame_index}");
        }
        assert_eq!(compiler.hvgl_cached_outlines(), 3, "cached once, reused");
    }

    /// CoreText returns no path for a space; its empty bounding box marks it
    /// as a glyph with no ink rather than one CoreText could not supply.
    #[test]
    fn a_pingfang_space_is_neither_drawn_nor_skipped() {
        let Some(data) = pingfang() else { return };
        let glyphs = coretext_glyphs(FACE, "中 文");
        let mut compiler = compiler();
        let frame = compile(&mut compiler, vec![run(face(&data, FACE), &glyphs)]);
        assert_eq!(frame.skipped_glyphs, 0);
        assert_eq!(frame.glyph_draws, 2);
    }

    /// A hidden name CoreText substitutes a face for is refused: skipped and
    /// counted once, never drawn from the substitute.
    #[test]
    fn a_hidden_named_pingfang_handle_is_refused_when_coretext_substitutes() {
        let Some(data) = pingfang() else { return };
        let glyphs = coretext_glyphs(FACE, TEXT);
        let mut compiler = compiler();
        let hidden = face_named(&data, ".PingFangUITextSC-Default", true);
        let frame = compile(&mut compiler, vec![run(hidden, &glyphs)]);
        assert_eq!(frame.skipped_glyphs, glyphs.len() as u32);
        assert_eq!(frame.glyph_draws, 0);
        assert_eq!(compiler.hvgl_identity_mismatches(), 1);
    }

    /// The identity check is silent when CoreText resolves the name asked for.
    #[test]
    fn a_resolved_pingfang_name_logs_no_identity_mismatch() {
        let Some(data) = pingfang() else { return };
        let mut compiler = compiler();
        compile(
            &mut compiler,
            vec![run(face(&data, FACE), &coretext_glyphs(FACE, TEXT))],
        );
        assert_eq!(compiler.hvgl_identity_mismatches(), 0);
    }

    #[test]
    fn the_hvgl_outline_cache_evicts_at_its_cap() {
        let Some(data) = pingfang() else { return };
        let mut compiler = compiler();
        let capacity = compiler.hvgl_outline_capacity();
        let font = face(&data, FACE);
        // Glyph ids 1.. are all inside PingFang (tens of thousands of glyphs),
        // drawn 32 distinct ones a frame until the cap is passed.
        let ids: Vec<u32> = (1..=u32::try_from(capacity + 64).expect("small")).collect();
        for (chunk_index, chunk) in ids.chunks(32).enumerate() {
            compile(&mut compiler, vec![run(font.clone(), chunk)]);
            let drawn = (chunk_index + 1) * 32;
            assert_eq!(
                compiler.hvgl_cached_outlines(),
                drawn.min(capacity),
                "after {drawn} distinct glyphs"
            );
        }
        assert_eq!(compiler.hvgl_cached_outlines(), capacity);
        // Drawing the newest glyphs again fetches nothing new and keeps the
        // population at the cap.
        compile(&mut compiler, vec![run(font, &ids[ids.len() - 32..])]);
        assert_eq!(compiler.hvgl_cached_outlines(), capacity);
    }

    static RENDER_LOCK: Mutex<()> = Mutex::new(());

    fn render_lock() -> MutexGuard<'static, ()> {
        RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
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
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
                .await
                .expect("no compatible GPU adapter");
            let caps = TierCaps::probe(&adapter);
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-engine hvgl device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                    ..Default::default()
                })
                .await
                .expect("failed to create the device");
            (device, queue, caps)
        })
    }

    /// How many pixels of `runs`, rendered headless through Metal, carry ink.
    fn inked_pixels(runs: Vec<GlyphRun>) -> usize {
        let _guard = render_lock();
        let (device, queue, caps) = gpu();
        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
        let scene = scene_of(runs);
        let mut renderer = EngineRenderer::with_image_sampler(&device, &caps, FORMAT, None, true)
            .expect("the engine builds on this device");
        renderer.finish_warm_up(&device);
        let (width, height) = (u32::from(VIEWPORT.0), u32::from(VIEWPORT.1));
        let target = HeadlessTarget::new(&device, width, height, FORMAT);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine hvgl frame"),
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
        let pixels = target.read_back(&device, &queue);
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] != 0)
            .count()
    }

    #[test]
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test hvgl_provider -- --ignored`"]
    fn pingfang_han_glyphs_render_ink_through_the_gpu() {
        let Some(data) = pingfang() else { return };
        let glyphs = coretext_glyphs(FACE, TEXT);
        let runs = vec![run(face(&data, FACE), &glyphs)];
        assert_identity_clean(&runs);
        let inked = inked_pixels(runs);
        // Three 24 px Han glyphs cover well over a hundred pixels between them.
        assert!(inked > 100, "only {inked} pixels carry ink");
    }

    /// Strokes thicken with weight: 中 at 600 inks more pixels than at 300.
    #[test]
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test hvgl_provider -- --ignored`"]
    fn pingfang_semibold_inks_more_than_light_through_the_gpu() {
        let Some(data) = pingfang() else { return };
        let mut ctx = text_context_resolves_pingfang();
        let light_runs = pingfang_runs_at(&data, ctx.as_mut(), FontWeight::LIGHT, 300.0);
        assert_identity_clean(&light_runs);
        let light = inked_pixels(light_runs);
        let semibold_runs = pingfang_runs_at(&data, ctx.as_mut(), FontWeight::SEMI_BOLD, 600.0);
        assert_identity_clean(&semibold_runs);
        let semibold = inked_pixels(semibold_runs);
        assert!(
            semibold > light,
            "weight 600 inks {semibold} pixels, weight 300 {light}"
        );
    }
}
