//! Glyph-run lowering: a `frust_scene::GlyphRun` becomes engine strips.
//!
//! Text is the one display-list primitive the compiler does not rasterize
//! itself. A glyph is a font outline that has to be fetched, scaled, hinted,
//! emboldened and cached before it is a path at all, and `glifo` — the same
//! crate the sparse-strip reference renderers drive their text through — is
//! where all of that already lives. This module is the adapter between the
//! two: it turns one display-list run into one
//! [`GlyphRunBuilder`](glifo::GlyphRunBuilder), points that builder at the
//! compiler's own strip generator through [`backend`], and hands back what the
//! run cost.
//!
//! # No conversion at the seam
//!
//! The display list's [`FontHandle`](frust_scene::FontHandle) wraps a
//! `peniko::FontData`, which is exactly what
//! [`GlyphRunBuilder::new`](glifo::GlyphRunBuilder::new) takes, and `glifo`
//! keys its caches on that blob's own process-unique id. So a run reaches
//! `glifo` by handing over the font it already carries — there is no font
//! registry in between, no re-parse, and no identity of the engine's own
//! invention that two paths could disagree about. A run's glyphs convert the
//! same way: `{id, x, y}` on both sides, mapped one for one.
//!
//! # Parity with the classic tier
//!
//! `frust-text` drops parley's synthesis when it builds a run — no synthetic
//! oblique, no synthetic bold, no variation coordinates — so the run this
//! module builds states that explicitly rather than inheriting whatever
//! `glifo`'s defaults happen to be: no normalized coordinates, and the default
//! (no-op) embolden. Hinting is the one knob set *against* the default:
//! `glifo` hints by default, the classic tier does not hint at all, and a
//! hinted outline lands on different pixels — so hinting is off here, and the
//! device-class policy that may turn it back on is a decision of its own, not
//! a default inherited by accident.
//!
//! # Cost
//!
//! The outline and hinting caches are retained across frames by the compiler
//! (they are the reason a steady-state frame of text costs no font-table work
//! at all), and maintained once per compiled frame. Every glyph is drawn as
//! strips, so a run costs one draw per glyph and no texture residency at all:
//! [`backend`]'s [`GlyphRunBackend`](glifo::GlyphRunBackend) hands `glifo`
//! [`AtlasCacher::Disabled`](glifo::AtlasCacher::Disabled) whichever way it is
//! called.
//!
//! [`atlas_policy`] is the decision half of turning that around — which glyphs
//! would earn a slot, keyed so an animated size cannot shred the cache, and in
//! what order a frame's one atlas pass services them. It is complete and
//! host-tested on its own; what it is still waiting for is the draw path that
//! consults it, since a cached glyph reaches the sink as an image paint and
//! [`backend`] has no encoding for one yet. Until then the two agree: no glyph
//! is cached, and the pixels are the outline path's.
//!
//! # The font gate
//!
//! `glifo` takes the font blob as read: it parses the face and reads its
//! `head` table with an `unwrap` on each, so a display list carrying a handle
//! whose blob is not a font — an unloaded resource, a truncated read, a
//! placeholder — would panic the frame path rather than return an
//! [`EngineError`](crate::EngineError) (E17). [`font_is_readable`] is the gate
//! that keeps it out: a run whose face fails it is refused whole, before its
//! brush is even encoded, and its glyphs are counted as skipped. The residual
//! `unwrap`s inside `glifo` — a corrupt outline that fails to draw, a colour
//! layer whose gradient carries no stops — are reachable only from a face that
//! parses and are shared with the reference renderers on the same pinned
//! version; the gate closes the class a plain display list can actually
//! deliver.

pub(crate) mod atlas_policy;
pub(crate) mod backend;
pub(crate) mod color;

use glifo::{AtlasConfig, FontEmbolden, Glyph, GlyphPrepCache, GlyphRunBuilder};
use kurbo::Affine;
use peniko::FontData;
use vello_common::paint::Paint;
use vello_common::strip_generator::StripGenerator;

use frust_gpu::TierCaps;
use frust_scene::GlyphRun;

use crate::cache::AtlasBudget;
use crate::compile::clip::ClipStack;
use crate::compile::{CompiledFrame, DepthCounter};
use crate::config;

use atlas_policy::AtlasPolicy;

pub(crate) use backend::{GlyphRunOutcome, context_paint};

use backend::{EngineGlyphSink, EngineTextBackend};

/// The pieces of a compile in progress a glyph run is drawn against.
///
/// Grouped into one value rather than passed as five arguments because they
/// are borrowed disjointly out of the compiler and the frame it is filling,
/// and naming that split in one place is what keeps the lowering free of
/// borrow gymnastics at the call site.
#[derive(Debug)]
pub(crate) struct GlyphRunTargets<'a> {
    /// The compiler's retained strip generator.
    pub(crate) generator: &'a mut StripGenerator,
    /// The clip stack the run is drawn under.
    pub(crate) clips: &'a mut ClipStack,
    /// The compiler's retained outline/hinting caches.
    pub(crate) prep: &'a mut GlyphPrepCache,
    /// The frame being compiled.
    pub(crate) frame: &'a mut CompiledFrame,
    /// The frame's painter-order depth counter.
    pub(crate) depth: &'a mut DepthCounter,
}

/// Draw `run` into `targets`, painted with the already-encoded `paint`.
///
/// `transform` is the run's own transform composed with the frame root — the
/// full device-space mapping — and is what `glifo` derives every glyph's draw
/// transform from. `paint` is the run's brush as the compiler encoded it once
/// against that same transform (see [`backend`]'s module doc for why once is
/// enough), and `context_brush` is the brush it was encoded from, which
/// `glifo` reads back for a colour glyph's context colour.
///
/// Returns what the run cost: how many glyphs became draws, and how many were
/// refused.
pub(crate) fn lower_glyph_run(
    run: &GlyphRun,
    transform: Affine,
    paint: Paint,
    context_brush: &peniko::Brush,
    targets: GlyphRunTargets<'_>,
) -> GlyphRunOutcome {
    let GlyphRunTargets {
        generator,
        clips,
        prep,
        frame,
        depth,
    } = targets;

    let mut sink = EngineGlyphSink::new(
        generator,
        clips,
        frame,
        depth,
        paint,
        context_paint(context_brush),
    );

    GlyphRunBuilder::new(
        run.font.font().clone(),
        transform,
        // The brush's own transform relative to the run: the display list has
        // no per-run paint transform, so a run's paint is placed by the run's
        // transform alone.
        Affine::IDENTITY,
        EngineTextBackend::new(&mut sink, prep),
    )
    .font_size(run.font_size)
    .font_embolden(FontEmbolden::default())
    .normalized_coords(&[])
    .hint(false)
    .fill_glyphs(run.glyphs.iter().map(|glyph| Glyph {
        id: glyph.id,
        x: glyph.x,
        y: glyph.y,
    }));

    sink.outcome()
}

/// The glyph-atlas policy `caps`' adapter gets.
///
/// The page geometry is [`AtlasBudget::for_caps`]' — mobile `(1024, 1024)` x4
/// layers, desktop `(2048, 2048)` x8, `FRUST_ENGINE_ATLAS_SIZE` redistributing
/// that tier's own allowance between extent and depth, then clamped to what the
/// adapter can actually create. Shared with the image atlas because the tier
/// question is the same question: the two arrays are separate textures sized by
/// one budget, not one texture holding both.
///
/// `FRUST_ENGINE_NO_ATLAS` is read here, once per policy, rather than per run —
/// it is a process-global kill switch ([`config::atlas_disabled`]), and a run
/// that consulted it separately could not be told from one the size tracker
/// refused.
#[allow(
    dead_code,
    reason = "constructs the policy for the glyph-draw path that consults it; \
              that path is what turns atlas caching on in `backend`"
)]
#[must_use]
pub(crate) fn glyph_atlas_policy(caps: &TierCaps) -> AtlasPolicy {
    let pages: AtlasConfig = AtlasBudget::for_caps(caps).config();
    AtlasPolicy::new(pages, config::atlas_disabled())
}

/// The OpenType table directory's own fixed header length.
const TABLE_DIRECTORY_LEN: usize = 12;
/// One table record: tag, checksum, offset, length.
const TABLE_RECORD_LEN: usize = 16;
/// The `head` table's fixed length.
const HEAD_TABLE_LEN: usize = 54;
/// Byte offset of `unitsPerEm` inside the `head` table.
const HEAD_UNITS_PER_EM: usize = 18;
/// `head`, as a table record's tag reads.
const HEAD_TAG: [u8; 4] = *b"head";
/// A font collection's own file tag.
const TTC_TAG: [u8; 4] = *b"ttcf";
/// Byte offset of a collection header's font count.
const TTC_NUM_FONTS: usize = 12;
/// Byte offset of a collection header's first table-directory offset.
const TTC_OFFSETS: usize = 16;
/// The three single-font file tags: TrueType outlines, CFF outlines, and the
/// legacy Apple TrueType tag.
const SFNT_TAGS: [[u8; 4]; 3] = [[0x00, 0x01, 0x00, 0x00], *b"OTTO", *b"true"];

/// Whether `font`'s blob is a face this backend can hand to `glifo` without
/// tripping one of its `unwrap`s (see this module's doc).
///
/// Answers the two questions `glifo` asks and does not check: does the blob
/// parse as a font at `font.index`, and does that font carry a `head` table
/// with a usable units-per-em? A zero units-per-em passes the parse and then
/// divides every glyph transform by nothing, so it is refused here alongside a
/// missing table rather than allowed to become a non-finite transform later.
///
/// Deliberately stricter than the parser it stands in front of, never looser:
/// it accepts only a known file tag whose table directory and named records
/// all lie inside the blob, which is a subset of what the parser itself will
/// take. A face this refuses but the parser would have accepted loses its
/// text; a face this accepted but the parser would have rejected would lose
/// the frame, which is the failure worth being conservative about.
pub(crate) fn font_is_readable(font: &FontData) -> bool {
    let data = font.data.data();
    table_directory(data, font.index)
        .and_then(|directory| head_units_per_em(data, directory))
        .is_some_and(|units_per_em| units_per_em > 0)
}

/// Where `index`'s table directory starts inside `data`, or `None` when the
/// blob is not a font file this index names one in.
fn table_directory(data: &[u8], index: u32) -> Option<usize> {
    let tag: [u8; 4] = data.get(0..4)?.try_into().ok()?;

    let start = if tag == TTC_TAG {
        let count = read_u32(data, TTC_NUM_FONTS)?;
        if index >= count {
            return None;
        }
        let entry = TTC_OFFSETS.checked_add(usize::try_from(index).ok()?.checked_mul(4)?)?;
        usize::try_from(read_u32(data, entry)?).ok()?
    } else {
        // A single font file holds exactly one face, so any other index names
        // nothing in it.
        if index != 0 || !SFNT_TAGS.contains(&tag) {
            return None;
        }
        0
    };

    // The directory's own header plus every record it claims must be inside
    // the blob; a truncated one reads back as no records at all rather than as
    // an error, which would lose the `head` table silently.
    let records = read_u16(data, start.checked_add(4)?)?;
    let end = start
        .checked_add(TABLE_DIRECTORY_LEN)?
        .checked_add(usize::from(records).checked_mul(TABLE_RECORD_LEN)?)?;

    (end <= data.len()).then_some(start)
}

/// The units-per-em of the `head` table named by the directory at
/// `directory`, or `None` when there is no readable one.
fn head_units_per_em(data: &[u8], directory: usize) -> Option<u16> {
    let records = read_u16(data, directory.checked_add(4)?)?;

    for index in 0..usize::from(records) {
        let record = directory
            .checked_add(TABLE_DIRECTORY_LEN)?
            .checked_add(index.checked_mul(TABLE_RECORD_LEN)?)?;
        let tag: [u8; 4] = data.get(record..record.checked_add(4)?)?.try_into().ok()?;
        if tag != HEAD_TAG {
            continue;
        }

        // The first record carrying the tag, which is the one the parser
        // resolves it to whether or not the directory is sorted.
        let offset = usize::try_from(read_u32(data, record.checked_add(8)?)?).ok()?;
        let length = usize::try_from(read_u32(data, record.checked_add(12)?)?).ok()?;
        let fits = offset != 0
            && length >= HEAD_TABLE_LEN
            && offset
                .checked_add(length)
                .is_some_and(|end| end <= data.len());
        if !fits {
            return None;
        }

        return read_u16(data, offset.checked_add(HEAD_UNITS_PER_EM)?);
    }

    None
}

/// The big-endian `u16` at `at`, or `None` when it does not fit.
fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    let bytes: [u8; 2] = data.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_be_bytes(bytes))
}

/// The big-endian `u32` at `at`, or `None` when it does not fit.
fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    let bytes: [u8; 4] = data.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_be_bytes(bytes))
}
