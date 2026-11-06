//! Bitmap-strike colour glyphs on the frust-side bitmap route: a bitmap-first
//! face (a `CBDT` strike with no outlines here; Apple Color Emoji's `sbix` on
//! Apple targets; a synthetic `sbix` face) never reaches `glifo`. Its PNG
//! record is decoded by the engine, inserted into the atlas through the
//! pending-upload queue and charged its real slot, and drawn from the atlas; a
//! glyph the atlas cannot serve is counted skipped. A face with outlines as
//! well as strikes is `glifo`'s, and `glifo` decodes none of its strikes: it
//! draws the outlines.
//!
//! Sibling of `text_color_hint.rs` and driven the same way: a `frust_scene`
//! `Scene` compiled by `SceneCompiler`, with glyph ids named directly against
//! the bundled face. The host cases need no GPU; the one case that writes the
//! queued pixmap into a real atlas array is `#[ignore]`d and runs with
//! `cargo test -p frust-engine --test text_bitmap_strike -- --ignored`. The
//! Apple Color Emoji cases skip when the face is not installed; the iOS ones
//! (CoreText) run in the simulator with
//! `CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER` set to a `xcrun simctl spawn`
//! wrapper and `--target aarch64-apple-ios-sim`.

use std::sync::{Arc, LazyLock};

use frust_engine::cache::images::ImageResidency;
use frust_engine::compile::CompiledFrame;
use frust_engine::{AtlasBudget, SceneCompiler};
use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};
use kurbo::Affine;
use peniko::color::palette::css::RED;
use peniko::{Blob, Brush, FontData};
use skrifa::bitmap::{BitmapData, BitmapStrikes};
use skrifa::instance::Size;
use skrifa::raw::FileRef;
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// Noto Color Emoji, subsetted to U+1F600 with its `CBDT`/`CBLC` strike kept.
const CBDT_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoColorEmoji-CBDT-Subset.ttf");

/// [`CBDT_FONT`]'s U+1F600, read off its character map.
const GRINNING_FACE: u32 = 1;

const VIEWPORT: (u16, u16) = (128, 128);

/// A size on the atlas route: far from the strike's own ppem, so the strike is
/// scaled rather than matched.
const SIZE: f32 = 32.0;

/// One blob for the whole file: `glifo` keys its cache on the blob's
/// process-unique id, so a face rebuilt per frame is a new font every frame
/// and nothing it drew is ever resident.
static FACE: LazyLock<FontData> =
    LazyLock::new(|| FontData::new(Blob::new(Arc::new(CBDT_FONT)), 0));

fn run() -> GlyphRun {
    run_at(SIZE)
}

fn run_at(size: f32) -> GlyphRun {
    GlyphRun {
        font: FontHandle::new(FACE.clone()),
        font_size: size,
        brush: Brush::Solid(RED),
        transform: Affine::translate((8.0, 64.0)),
        glyphs: vec![Glyph {
            id: GRINNING_FACE,
            x: 0.0,
            y: 0.0,
        }],
    }
}

fn scene() -> Scene {
    scene_of(run())
}

fn scene_of(run: GlyphRun) -> Scene {
    let mut scene = Scene::new();
    SceneBuilder::new(&mut scene).draw_glyph_run(run);
    scene
}

fn compile(compiler: &mut SceneCompiler) -> CompiledFrame {
    compiler
        .compile(&scene(), Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles")
}

fn atlas_compiler() -> SceneCompiler {
    SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
}

fn outline_compiler() -> SceneCompiler {
    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    compiler.set_image_residency(ImageResidency::disabled(AtlasBudget::MOBILE));
    compiler
}

#[test]
fn the_test_face_carries_a_png_bitmap_strike_and_no_outline() {
    let font = FontRef::new(CBDT_FONT).expect("the face parses");
    assert_eq!(
        font.charmap().map(0x1F600_u32),
        Some(GlyphId::new(GRINNING_FACE)),
        "U+1F600 is the glyph the cases below draw"
    );

    let strikes: BitmapStrikes<'_> = font.bitmap_strikes();
    let glyph = strikes
        .glyph_for_size(Size::new(SIZE), GlyphId::new(GRINNING_FACE))
        .expect("the face has a strike for the grinning face");
    assert!(
        matches!(glyph.data, BitmapData::Png(_)),
        "a CBDT strike is PNG data"
    );
    assert!(matches!(FileRef::new(CBDT_FONT), Ok(FileRef::Font(_))));
}

#[test]
fn the_strikes_png_decodes_into_a_pixmap() {
    let font = FontRef::new(CBDT_FONT).expect("the face parses");
    let glyph = font
        .bitmap_strikes()
        .glyph_for_size(Size::new(SIZE), GlyphId::new(GRINNING_FACE))
        .expect("the face has a strike for the grinning face");
    let BitmapData::Png(data) = glyph.data else {
        panic!("a CBDT strike is PNG data");
    };

    // The decoder the bitmap route calls for a PNG strike: present only under
    // the `png` feature `frust-engine` enables on `vello_common`.
    let pixmap = vello_common::pixmap::Pixmap::from_png(std::io::Cursor::new(data))
        .expect("the strike's PNG decodes");
    assert!(pixmap.width() > 0 && pixmap.height() > 0);
    assert!(
        pixmap.data_as_u8_slice().chunks(4).any(|px| px[3] != 0),
        "the decoded emoji has visible texels"
    );
}

#[test]
fn an_atlas_routed_bitmap_glyph_is_drawn_and_queues_one_upload() {
    let mut compiler = atlas_compiler();
    let frame = compile(&mut compiler);

    assert_eq!(
        frame.skipped_glyphs, 0,
        "a PNG strike on the atlas route is drawn, not refused"
    );
    assert_eq!(frame.glyph_draws, 1);
    assert_eq!(compiler.glyph_atlas_entries(), 1, "the glyph is resident");

    let uploads: Vec<_> = compiler.glyph_atlas_mut().drain_pending_uploads().collect();
    assert_eq!(uploads.len(), 1, "exactly one bitmap upload is queued");
    let upload = &uploads[0];
    assert_eq!(
        u32::from(upload.pixmap.width()),
        u32::from(upload.atlas_slot.width),
        "the pixmap fills its slot"
    );
    assert_eq!(
        u32::from(upload.pixmap.height()),
        u32::from(upload.atlas_slot.height)
    );
}

#[test]
fn a_bitmap_glyph_with_the_atlas_off_is_counted_skipped() {
    let mut compiler = outline_compiler();
    let frame = compile(&mut compiler);

    assert_eq!(
        frame.skipped_glyphs, 1,
        "a strike is drawn only from the atlas, which this compiler has none of"
    );
    assert_eq!(
        compiler.glyph_atlas_mut().drain_pending_uploads().count(),
        0,
        "and nothing is queued for the atlas"
    );
}

#[test]
fn a_96_px_bitmap_glyph_is_drawn_from_the_atlas() {
    let mut compiler = atlas_compiler();
    let frame = compiler
        .compile(&scene_of(run_at(96.0)), Affine::IDENTITY, (256, 256))
        .expect("an in-range scene compiles");

    assert_eq!(
        frame.skipped_glyphs, 0,
        "a 96 px strike glyph is not refused"
    );
    assert_eq!(frame.glyph_draws, 1);
    assert_eq!(
        compiler.glyph_atlas_mut().drain_pending_uploads().count(),
        1,
        "its strike is uploaded once"
    );
}

/// Past the outline path's 128 px ceiling: only the bitmap-first exemption
/// keeps it on the atlas.
#[test]
fn a_bitmap_glyph_far_past_the_cached_size_ceiling_is_still_drawn_from_the_atlas() {
    let mut compiler = atlas_compiler();
    let frame = compiler
        .compile(&scene_of(run_at(160.0)), Affine::IDENTITY, (256, 256))
        .expect("an in-range scene compiles");

    assert_eq!(frame.skipped_glyphs, 0);
    assert_eq!(frame.glyph_draws, 1);
    assert_eq!(
        compiler.glyph_atlas_mut().drain_pending_uploads().count(),
        1
    );
}

#[test]
fn a_bitmap_glyph_whose_size_animates_is_drawn_every_frame() {
    let mut compiler = atlas_compiler();
    for step in 0..8_u8 {
        let size = 24.0 + f32::from(step) * 6.0;
        let frame = compiler
            .compile(&scene_of(run_at(size)), Affine::IDENTITY, (256, 256))
            .expect("an in-range scene compiles");
        assert_eq!(frame.skipped_glyphs, 0, "step {step} at {size} px");
        assert_eq!(frame.glyph_draws, 1);
    }
}

#[test]
fn a_resident_bitmap_glyph_queues_no_second_upload() {
    let mut compiler = atlas_compiler();
    compile(&mut compiler);
    assert_eq!(
        compiler.glyph_atlas_mut().drain_pending_uploads().count(),
        1,
        "the first draw uploads the strike"
    );

    let frame = compile(&mut compiler);
    assert_eq!(frame.skipped_glyphs, 0);
    assert_eq!(
        compiler.glyph_atlas_mut().drain_pending_uploads().count(),
        0,
        "the glyph is resident: a drained queue is not refilled"
    );
}

/// A 24 px emoji is charged the slot its 109 ppem strike really occupies —
/// the image plus `glifo`'s padding on every side — not a 24 px glyph's.
#[test]
fn a_24_px_bitmap_glyph_is_charged_its_real_padded_slot() {
    let mut compiler = atlas_compiler();
    let before = compiler.glyph_resident_texels();
    let frame = compiler
        .compile(&scene_of(run_at(24.0)), Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles");
    assert_eq!(frame.skipped_glyphs, 0);

    let uploads: Vec<_> = compiler.glyph_atlas_mut().drain_pending_uploads().collect();
    assert_eq!(uploads.len(), 1);
    let pad = u64::from(glifo::GLYPH_PADDING) * 2;
    let slot = uploads[0].atlas_slot;
    let padded = (u64::from(slot.width) + pad) * (u64::from(slot.height) + pad);
    assert_eq!(compiler.glyph_resident_texels() - before, padded);
    assert!(
        padded > 20 * 24 * 24,
        "a strike slot is far larger than a 24 px glyph's em square: {padded}"
    );
}

/// [`CBDT_FONT`] with its strike's PNG header claiming a 4096 px square: the
/// bitmap route refuses it on the header alone.
static OVERSIZED: LazyLock<FontData> = LazyLock::new(|| {
    let mut bytes = CBDT_FONT.to_vec();
    let ihdr = bytes
        .windows(4)
        .position(|window| window == b"IHDR")
        .expect("the strike carries a PNG");
    bytes[ihdr + 4..ihdr + 8].copy_from_slice(&4096_u32.to_be_bytes());
    bytes[ihdr + 8..ihdr + 12].copy_from_slice(&4096_u32.to_be_bytes());
    FontData::new(Blob::new(Arc::new(bytes)), 0)
});

#[test]
fn a_strike_whose_png_header_is_oversized_is_refused_without_an_upload() {
    let mut compiler = atlas_compiler();
    let run = GlyphRun {
        font: FontHandle::new(OVERSIZED.clone()),
        ..run()
    };
    let frame = compiler
        .compile(&scene_of(run), Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles");
    assert_eq!(frame.skipped_glyphs, 1, "the oversized strike is refused");
    assert_eq!(compiler.glyph_atlas_entries(), 0, "nothing was allocated");
    assert_eq!(
        compiler.glyph_atlas_mut().drain_pending_uploads().count(),
        0
    );
}

/// The tables of the single-font file `font`, in directory order.
fn tables_of(font: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    let read_u32 = |at: usize| u32::from_be_bytes(font[at..at + 4].try_into().unwrap()) as usize;
    let count = usize::from(u16::from_be_bytes([font[4], font[5]]));
    (0..count)
        .map(|index| {
            let record = 12 + 16 * index;
            let tag: [u8; 4] = font[record..record + 4].try_into().unwrap();
            let (offset, length) = (read_u32(record + 8), read_u32(record + 12));
            (tag, font[offset..offset + length].to_vec())
        })
        .collect()
}

/// The table `tag` of `tables`.
fn table_in<'a>(tables: &'a [([u8; 4], Vec<u8>)], tag: &[u8; 4]) -> &'a [u8] {
    &tables
        .iter()
        .find(|(found, _)| found == tag)
        .unwrap_or_else(|| panic!("no {tag:?} table"))
        .1
}

/// A single-font file holding `tables`, its directory sorted by tag (the
/// parsers search it) and every table four-byte aligned.
fn sfnt(mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by_key(|(tag, _)| *tag);
    let mut out = vec![0x00, 0x01, 0x00, 0x00];
    out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    let mut offset = 12 + 16 * tables.len();
    for (tag, bytes) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        offset += bytes.len().next_multiple_of(4);
    }
    for (_, bytes) in &tables {
        out.extend_from_slice(bytes);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    out
}

/// [`CBDT_FONT`]'s one strike image: the PNG file from its signature through
/// its `IEND` chunk.
fn grinning_png() -> Vec<u8> {
    let start = CBDT_FONT
        .windows(8)
        .position(|window| window == b"\x89PNG\r\n\x1a\n")
        .expect("the strike carries a PNG");
    let end = CBDT_FONT
        .windows(4)
        .position(|window| window == b"IEND")
        .expect("the PNG ends")
        + 8;
    CBDT_FONT[start..end].to_vec()
}

/// [`CBDT_FONT`] with TrueType outlines added — Noto Sans's `H` as glyph
/// [`GRINNING_FACE`]'s outline — so it is no longer bitmap-first and its runs
/// reach `glifo`; with its strike's PNG header claiming `claimed` px square
/// when given.
fn cbdt_with_outlines(claimed: Option<u32>) -> FontData {
    let mut tables = tables_of(CBDT_FONT);
    if let Some(side) = claimed {
        let cbdt = &mut tables.iter_mut().find(|(tag, _)| tag == b"CBDT").unwrap().1;
        let ihdr = cbdt
            .windows(4)
            .position(|window| window == b"IHDR")
            .expect("the strike carries a PNG");
        cbdt[ihdr + 4..ihdr + 8].copy_from_slice(&side.to_be_bytes());
        cbdt[ihdr + 8..ihdr + 12].copy_from_slice(&side.to_be_bytes());
    }
    let latin = tables_of(LATIN_FONT);
    let (latin_loca, latin_glyf) = (table_in(&latin, b"loca"), table_in(&latin, b"glyf"));
    let short = |glyph: usize| {
        usize::from(u16::from_be_bytes([
            latin_loca[glyph * 2],
            latin_loca[glyph * 2 + 1],
        ])) * 2
    };
    const LATIN_H: usize = 5;
    let outline = latin_glyf[short(LATIN_H)..short(LATIN_H + 1)].to_vec();
    assert!(!outline.is_empty(), "Noto Sans's H has an outline");

    let maxp = table_in(&tables, b"maxp");
    let glyphs = usize::from(u16::from_be_bytes([maxp[4], maxp[5]]));
    let mut loca = Vec::new();
    let mut glyf = Vec::new();
    for glyph in 0..=glyphs {
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        if glyph == GRINNING_FACE as usize {
            glyf.extend_from_slice(&outline);
        }
    }
    let head = &mut tables.iter_mut().find(|(tag, _)| tag == b"head").unwrap().1;
    head[50..52].copy_from_slice(&1_u16.to_be_bytes());
    tables.push((*b"loca", loca));
    tables.push((*b"glyf", glyf));
    FontData::new(Blob::new(Arc::new(sfnt(tables))), 0)
}

/// A PNG strike of a face that also carries outlines is `glifo`'s, and
/// `glifo` decodes none: the glyph draws its outline on every frame — the
/// animating first frames and the settled, atlas-routed ones alike — and no
/// bitmap is ever queued. With the strike's header claiming a 20000 px square
/// the frames compile just the same, and as quickly: nothing sizes a decode
/// from that header.
#[test]
fn a_png_strike_of_a_face_with_outlines_is_never_decoded() {
    for claimed in [None, Some(20_000)] {
        let face = FontHandle::new(cbdt_with_outlines(claimed));
        let font = FontRef::new(face.font().data.data()).expect("the face parses");
        let strike = font
            .bitmap_strikes()
            .glyph_for_size(Size::new(SIZE), GlyphId::new(GRINNING_FACE))
            .expect("fixture precondition: the strike survives the rebuild");
        assert!(matches!(strike.data, BitmapData::Png(_)));

        let mut compiler = atlas_compiler();
        let started = std::time::Instant::now();
        for frame_index in 0..12 {
            let frame = compiler
                .compile(
                    &scene_of(GlyphRun {
                        font: face.clone(),
                        ..run()
                    }),
                    Affine::IDENTITY,
                    VIEWPORT,
                )
                .expect("an in-range scene compiles");
            let case = format!("{claimed:?} px header, frame {frame_index}");
            assert_eq!(frame.skipped_glyphs, 0, "{case}: the outline is drawn");
            assert_eq!(frame.glyph_draws, 1, "{case}");
            assert_eq!(
                compiler.glyph_atlas_mut().drain_pending_uploads().count(),
                0,
                "{case}: no strike is decoded, so no bitmap is queued"
            );
            compiler.acknowledge_glyph_replay();
            compiler.acknowledge_glyph_clears();
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "{claimed:?}: twelve frames took {:?}",
            started.elapsed()
        );
    }
}

/// The `sbix` ppem the synthetic face's one strike is drawn at: the PNG it
/// carries is [`CBDT_FONT`]'s 109 ppem image.
const SBIX_PPEM: u16 = 109;

/// The synthetic `sbix` face's glyph that carries a `png ` record.
const SBIX_IMAGE: u32 = 1;

/// Its glyph that no strike holds a record for.
const SBIX_STRIKELESS: u32 = 2;

/// A bitmap-first `sbix` face (`DRAW_OUTLINES` clear) of three glyphs and one
/// strike: `.notdef` and [`SBIX_STRIKELESS`] without records,
/// [`SBIX_IMAGE`] with [`grinning_png`] as its `png ` record.
fn sbix_face() -> FontData {
    let source = tables_of(CBDT_FONT);
    let mut maxp = table_in(&source, b"maxp").to_vec();
    maxp[4..6].copy_from_slice(&3_u16.to_be_bytes());
    let mut hhea = table_in(&source, b"hhea").to_vec();
    hhea[34..36].copy_from_slice(&1_u16.to_be_bytes());
    // One long metric (advance 2550, lsb 0) and two short lsbs.
    let mut hmtx = 2550_u16.to_be_bytes().to_vec();
    hmtx.extend_from_slice(&[0; 6]);

    let png = grinning_png();
    let mut record = 0_i16.to_be_bytes().to_vec();
    record.extend_from_slice(&0_i16.to_be_bytes());
    record.extend_from_slice(b"png ");
    record.extend_from_slice(&png);
    // The strike: ppem, ppi, four glyph-data offsets (three glyphs plus the
    // end), then glyph 1's record.
    let records_at = 4 + 4 * 4_u32;
    let mut strike = SBIX_PPEM.to_be_bytes().to_vec();
    strike.extend_from_slice(&72_u16.to_be_bytes());
    let end = records_at + record.len() as u32;
    for offset in [records_at, records_at, end, end] {
        strike.extend_from_slice(&offset.to_be_bytes());
    }
    strike.extend_from_slice(&record);
    let mut sbix = 1_u16.to_be_bytes().to_vec();
    sbix.extend_from_slice(&1_u16.to_be_bytes());
    sbix.extend_from_slice(&1_u32.to_be_bytes());
    sbix.extend_from_slice(&12_u32.to_be_bytes());
    sbix.extend_from_slice(&strike);

    let tables = vec![
        (*b"head", table_in(&source, b"head").to_vec()),
        (*b"hhea", hhea),
        (*b"hmtx", hmtx),
        (*b"maxp", maxp),
        (*b"sbix", sbix),
    ];
    FontData::new(Blob::new(Arc::new(sfnt(tables))), 0)
}

/// One frame of `glyph` of `face` at [`SIZE`] on a fresh atlas compiler, the
/// compiler kept for a second frame.
fn sbix_frame(face: &FontHandle, glyph: u32) -> (SceneCompiler, CompiledFrame) {
    let mut compiler = atlas_compiler();
    let frame = compiler
        .compile(
            &scene_of(GlyphRun {
                font: face.clone(),
                glyphs: vec![Glyph {
                    id: glyph,
                    x: 0.0,
                    y: 0.0,
                }],
                ..run()
            }),
            Affine::IDENTITY,
            VIEWPORT,
        )
        .expect("an in-range scene compiles");
    (compiler, frame)
}

/// In a bitmap-first `sbix` face a glyph no strike holds a record for is a
/// missing glyph — such a face never falls back to its outlines — and is
/// counted skipped; its sibling with a `png ` record is decoded and drawn.
#[test]
fn an_sbix_glyph_no_strike_holds_is_counted_skipped() {
    let face = FontHandle::new(sbix_face());
    let (_, frame) = sbix_frame(&face, SBIX_STRIKELESS);
    assert_eq!(frame.skipped_glyphs, 1, "the strike-less glyph is counted");
    assert_eq!(frame.glyph_draws, 0);

    let (mut compiler, frame) = sbix_frame(&face, SBIX_IMAGE);
    assert_eq!(frame.skipped_glyphs, 0, "the png record draws");
    assert_eq!(frame.glyph_draws, 1);
    let uploads: Vec<_> = compiler.glyph_atlas_mut().drain_pending_uploads().collect();
    assert_eq!(uploads.len(), 1);
    assert_eq!(
        (uploads[0].atlas_slot.width, uploads[0].atlas_slot.height),
        (136, 128)
    );
}

/// On iOS a named `sbix` face CoreText cannot open — an app-bundled face it
/// was never given — falls back to the engine's own decode of the same
/// strike's `png ` record instead of being counted skipped, and the resident
/// slot keeps drawing on the next frame without a second upload.
#[cfg(target_os = "ios")]
#[test]
fn an_sbix_face_core_text_cannot_open_draws_through_the_engines_decode() {
    let face = FontHandle::new(sbix_face())
        .with_identity(Some(Arc::from("FrustTestNoSuchSbixFace")), false);
    let (mut compiler, frame) = sbix_frame(&face, SBIX_IMAGE);
    assert_eq!(frame.skipped_glyphs, 0, "the record is decoded instead");
    assert_eq!(frame.glyph_draws, 1);
    let uploads: Vec<_> = compiler.glyph_atlas_mut().drain_pending_uploads().collect();
    assert_eq!(uploads.len(), 1);
    assert_eq!(
        (uploads[0].atlas_slot.width, uploads[0].atlas_slot.height),
        (136, 128)
    );

    let again = compiler
        .compile(
            &scene_of(GlyphRun {
                font: face.clone(),
                glyphs: vec![Glyph {
                    id: SBIX_IMAGE,
                    x: 0.0,
                    y: 0.0,
                }],
                ..run()
            }),
            Affine::IDENTITY,
            VIEWPORT,
        )
        .expect("an in-range scene compiles");
    assert_eq!(again.skipped_glyphs, 0);
    assert_eq!(again.glyph_draws, 1);
    assert_eq!(
        compiler.glyph_atlas_mut().drain_pending_uploads().count(),
        0,
        "the decoded slot is resident"
    );

    let (_, strikeless) = sbix_frame(&face, SBIX_STRIKELESS);
    assert_eq!(strikeless.skipped_glyphs, 1);
}

/// Noto Sans, subsetted to Latin: the outline text that spends the budget.
const LATIN_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

/// Faces of [`LATIN_FONT`], each its own blob and so its own atlas identity.
static LATIN_FACES: LazyLock<Vec<FontData>> = LazyLock::new(|| {
    (0..3)
        .map(|_| FontData::new(Blob::new(Arc::new(LATIN_FONT)), 0))
        .collect()
});

/// A 120 px run of every glyph of `face` in all four subpixel buckets: 84
/// entries at a 120 px glyph's price, far more than the 21 its distinct ids
/// are charged up front.
fn latin_run(face: &FontData, line: usize) -> GlyphRun {
    let glyphs = (1..22_u32)
        .flat_map(|id| {
            (0..4).map(move |bucket| Glyph {
                id,
                x: (id as f32) * 8.0 + bucket as f32 * 0.25,
                y: 0.0,
            })
        })
        .collect();
    GlyphRun {
        font: FontHandle::new(face.clone()),
        font_size: 120.0,
        brush: Brush::Solid(RED),
        transform: Affine::translate((0.0, 200.0 + 130.0 * line as f64)),
        glyphs,
    }
}

/// On the mobile atlas geometry, a 160 px emoji drawn ahead of enough
/// outline text to take residency past its budget is served from the atlas on
/// every frame — across an eviction pass — rather than blinking out once the
/// budget refuses insertions.
#[test]
fn a_large_bitmap_glyph_is_served_every_frame_while_the_budget_refuses() {
    let mut compiler = SceneCompiler::with_atlas_budget(512, 640, AtlasBudget::MOBILE);
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.draw_glyph_run(run_at(160.0));
        for (line, face) in LATIN_FACES.iter().enumerate() {
            builder.draw_glyph_run(latin_run(face, line));
        }
    }
    let strike = FontRef::new(CBDT_FONT)
        .expect("the face parses")
        .bitmap_strikes()
        .glyph_for_size(Size::new(160.0), GlyphId::new(GRINNING_FACE))
        .expect("the face has a strike for the grinning face");
    let strike_size = [strike.width, strike.height];

    let mut over_budget = 0;
    for frame_index in 0..100 {
        let frame = compiler
            .compile(&scene, Affine::IDENTITY, (512, 640))
            .expect("an in-range scene compiles");
        assert_eq!(frame.skipped_glyphs, 0, "frame {frame_index}");
        assert!(
            frame
                .glyph_slots
                .iter()
                .any(|slot| slot.region.size == strike_size),
            "frame {frame_index}: the emoji is drawn from its strike's slot"
        );
        if compiler.glyph_resident_texels() > compiler.glyph_texel_budget() {
            over_budget += 1;
        }
        compiler
            .glyph_atlas_mut()
            .drain_pending_uploads()
            .for_each(drop);
        compiler.acknowledge_glyph_replay();
        compiler.acknowledge_glyph_clears();
    }
    assert!(
        over_budget >= 90,
        "precondition: residency sat over its budget ({over_budget} of 100 frames)"
    );
}

/// The installed Apple Color Emoji (collection index 0), once per test run.
#[cfg(target_os = "macos")]
static APPLE_EMOJI: LazyLock<Option<FontData>> = LazyLock::new(|| {
    let bytes = std::fs::read("/System/Library/Fonts/Apple Color Emoji.ttc").ok()?;
    Some(FontData::new(Blob::new(Arc::new(bytes)), 0))
});

/// One frame of `glyph` of `face` at `size`, and the uploads it queued.
#[cfg(target_vendor = "apple")]
fn draw_one(
    face: FontHandle,
    glyph: u32,
    size: f32,
) -> (CompiledFrame, Vec<glifo::atlas::PendingBitmapUpload>) {
    let mut compiler = SceneCompiler::new(256, 256);
    let run = GlyphRun {
        font: face,
        font_size: size,
        brush: Brush::Solid(RED),
        transform: Affine::translate((8.0, 200.0)),
        glyphs: vec![Glyph {
            id: glyph,
            x: 0.0,
            y: 0.0,
        }],
    };
    let frame = compiler
        .compile(&scene_of(run), Affine::IDENTITY, (256, 256))
        .expect("an in-range scene compiles");
    let uploads = compiler.glyph_atlas_mut().drain_pending_uploads().collect();
    (frame, uploads)
}

/// Apple Color Emoji's `sbix` `png ` strikes are decoded by the engine on
/// macOS (CoreText is never asked to draw them there), at 32 px from the
/// 32 ppem strike and at 160 px from the 160 ppem one.
#[cfg(target_os = "macos")]
#[test]
fn apple_color_emoji_draws_through_the_engines_png_decode() {
    let Some(face) = APPLE_EMOJI.as_ref() else {
        eprintln!("Apple Color Emoji is not installed here; skipping");
        return;
    };
    let font = FontRef::from_index(face.data.data(), 0).expect("the face parses");
    let grinning = font
        .charmap()
        .map(0x1F600_u32)
        .expect("U+1F600 is mapped")
        .to_u32();
    for (size, ppem) in [(32.0, 32), (160.0, 160)] {
        let (frame, uploads) = draw_one(FontHandle::new(face.clone()), grinning, size);
        assert_eq!(frame.skipped_glyphs, 0, "{size} px");
        assert_eq!(frame.glyph_draws, 1, "{size} px");
        assert_eq!(uploads.len(), 1, "{size} px queues one upload");
        assert_eq!(
            (uploads[0].atlas_slot.width, uploads[0].atlas_slot.height),
            (ppem, ppem),
            "{size} px is drawn from the {ppem} ppem strike"
        );
    }
}

/// A `flip` record draws the glyph it names, mirrored: the upload is the
/// named glyph's own upload with every row reversed.
#[cfg(target_os = "macos")]
#[test]
fn an_apple_color_emoji_flip_record_draws_mirrored() {
    use skrifa::raw::TableProvider;
    use skrifa::raw::types::Tag;

    let Some(face) = APPLE_EMOJI.as_ref() else {
        eprintln!("Apple Color Emoji is not installed here; skipping");
        return;
    };
    let font = FontRef::from_index(face.data.data(), 0).expect("the face parses");
    let sbix = font.sbix().expect("the face carries sbix");
    let glyphs = font.maxp().expect("the face carries maxp").num_glyphs();
    let strike = (0..sbix.strikes().len())
        .filter_map(|index| sbix.strikes().get(index).ok())
        .find(|strike| strike.ppem() == 160)
        .expect("the face has a 160 ppem strike");
    let Some((flip, named)) = (0..u32::from(glyphs)).find_map(|id| {
        let data = strike.glyph_data(GlyphId::new(id)).ok()??;
        (data.graphic_type() == Tag::new(b"flip")).then(|| {
            let target = u16::from_be_bytes([data.data()[0], data.data()[1]]);
            (id, u32::from(target))
        })
    }) else {
        eprintln!("this Apple Color Emoji carries no flip record; skipping");
        return;
    };

    let (frame, flipped) = draw_one(FontHandle::new(face.clone()), flip, 160.0);
    assert_eq!(frame.skipped_glyphs, 0, "glyph {flip} flips glyph {named}");
    assert_eq!(flipped.len(), 1);
    let (_, plain) = draw_one(FontHandle::new(face.clone()), named, 160.0);
    let (flipped, plain) = (&flipped[0].pixmap, &plain[0].pixmap);
    assert_eq!(
        (flipped.width(), flipped.height()),
        (plain.width(), plain.height())
    );
    let width = usize::from(plain.width());
    for (mirrored, row) in flipped
        .data()
        .chunks_exact(width)
        .zip(plain.data().chunks_exact(width))
    {
        assert!(
            mirrored.iter().eq(row.iter().rev()),
            "every row is reversed"
        );
    }
}

/// The installed Apple Color Emoji on iOS: the device's own path, or the
/// simulator runtime's under `SIMULATOR_ROOT`.
#[cfg(target_os = "ios")]
static IOS_EMOJI: LazyLock<Option<FontData>> = LazyLock::new(|| {
    const FILE: &str = "System/Library/Fonts/CoreAddition/AppleColorEmoji-160px.ttc";
    let roots = ["SIMULATOR_ROOT", "IPHONE_SIMULATOR_ROOT"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .chain([String::from("/")]);
    roots
        .map(|root| std::path::Path::new(&root).join(FILE))
        .find_map(|path| std::fs::read(path).ok())
        .map(|bytes| FontData::new(Blob::new(Arc::new(bytes)), 0))
});

/// On iOS an `sbix` face is drawn by CoreText (its strikes are `emjc`
/// records the engine does not decode): U+1F600 at 32 px and at 160 px
/// compiles with nothing skipped and queues one coloured upload.
#[cfg(target_os = "ios")]
#[test]
fn apple_color_emoji_draws_through_core_text_on_ios() {
    let Some(face) = IOS_EMOJI.as_ref() else {
        eprintln!("Apple Color Emoji is not found here; skipping");
        return;
    };
    let font = FontRef::from_index(face.data.data(), 0).expect("the face parses");
    let grinning = font
        .charmap()
        .map(0x1F600_u32)
        .expect("U+1F600 is mapped")
        .to_u32();
    for size in [32.0, 160.0] {
        let handle =
            FontHandle::new(face.clone()).with_identity(Some(Arc::from("AppleColorEmoji")), false);
        let (frame, uploads) = draw_one(handle, grinning, size);
        assert_eq!(frame.skipped_glyphs, 0, "{size} px");
        assert_eq!(frame.glyph_draws, 1, "{size} px");
        assert_eq!(uploads.len(), 1, "{size} px queues one upload");
        assert!(
            uploads[0]
                .pixmap
                .data()
                .iter()
                .any(|texel| texel.a == 255 && texel.r != texel.b),
            "{size} px: the raster is opaque colour"
        );
    }
}

/// The GPU arm: the queued pixmap is accepted by a real atlas array.
#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine --test text_bitmap_strike -- --ignored`"]
fn a_queued_bitmap_upload_is_written_into_a_real_atlas_array() {
    use frust_engine::gpu::atlas::{AtlasArray, AtlasRenderer};
    use frust_gpu::TierCaps;

    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let (device, queue, caps) = pollster_block_on(async {
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine bitmap strike device"),
                required_features: wgpu::Features::empty(),
                required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    });

    let mut compiler = atlas_compiler();
    compile(&mut compiler);
    let uploads: Vec<_> = compiler.glyph_atlas_mut().drain_pending_uploads().collect();
    assert_eq!(uploads.len(), 1);

    let extent = u32::from(uploads[0].atlas_slot.x) + u32::from(uploads[0].atlas_slot.width);
    let extent =
        extent.max(u32::from(uploads[0].atlas_slot.y) + u32::from(uploads[0].atlas_slot.height));
    let layers = uploads[0].atlas_slot.page_index + 1;
    let atlas = AtlasArray::with_layers(
        &device,
        extent.next_power_of_two().max(256),
        extent.next_power_of_two().max(256),
        layers,
    );
    let renderer = AtlasRenderer::new(&device, &caps);
    assert!(
        renderer.upload_pixmap(&queue, &atlas, &uploads[0]),
        "the array takes the decoded strike"
    );
}

fn pollster_block_on<F: std::future::Future>(future: F) -> F::Output {
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
