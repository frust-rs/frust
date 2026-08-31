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
//! hinted outline lands on different pixels — so a run built here states its
//! own hinting choice explicitly too, rather than inheriting `glifo`'s.
//!
//! # The hinting policy, split in two
//!
//! Whether a run ends up hinted is two independent questions, answered on two
//! sides of the `glifo` boundary:
//!
//! - **Device class**, this crate's half: [`lower_glyph_run`]'s `hint`
//!   parameter, which [`SceneCompiler`](crate::compile::SceneCompiler) derives
//!   from the adapter's `TierCaps` in
//!   [`SceneCompiler::for_caps`](crate::compile::SceneCompiler::for_caps) —
//!   on for a desktop-class adapter, off for a mobile one, off by default
//!   until an adapter is known (the same caution
//!   [`AtlasBudget::MOBILE`](crate::cache::AtlasBudget::MOBILE) is chosen
//!   for). Nothing here decides *when* a hinted outline would actually help;
//!   it only decides whether the device is one hinting is worth paying for at
//!   all.
//! - **Transform shape**, `glifo`'s own half and not reimplemented here:
//!   [`GlyphRunBuilder::hint`] documents that hinting is applied only when the
//!   run's combined transform is a positive uniform scale with no vertical
//!   skew or rotation, and falls back to an unhinted direct draw otherwise —
//!   vertical-only hinting cannot answer for a transform it cannot express as
//!   a single vertical scale. A rotated or skewed run is therefore unhinted
//!   regardless of what `hint` this module passes, on `glifo`'s own terms.
//!
//! # Cost
//!
//! The outline and hinting caches are retained across frames by the compiler
//! (they are the reason a steady-state frame of text costs no font-table work
//! at all), and maintained once per compiled frame. What a *glyph* costs on top
//! of that is [`atlas_policy`]'s decision, made once per run before the run is
//! lowered and carried into `glifo` as the
//! [`AtlasCacher`](glifo::AtlasCacher) [`lower_glyph_run`] is handed:
//!
//! - **Routed to the atlas.** `glifo` resolves each glyph against the policy's
//!   [`GlyphAtlas`](glifo::GlyphAtlas), and a hit is one image draw sampling
//!   the slot — no outline fetched, no path flattened, no coverage generated.
//!   A miss allocates a slot out of the *shared* atlas allocator, records the
//!   fills that rasterize it into that page's command recorder, and draws the
//!   slot anyway; the recorded pages are replayed into the atlas by
//!   [`crate::gpu::atlas::AtlasRenderer`] before the frame's scene pass, which
//!   is the ordering that makes sampling a just-filled slot sound. So a page of
//!   static text pays its rasterization on one frame and nothing on the next
//!   thousand.
//! - **Routed to outlines.** Every glyph is fetched, scaled and rasterized as
//!   strips, exactly as before — one draw per glyph, no texture residency at
//!   all. This is what an animating size, an oversized run, an unusable size, a
//!   [colour face](font_has_color_glyphs), a transform `glifo` will not absorb
//!   a scale out of, a full glyph residency and `FRUST_ENGINE_NO_ATLAS` all
//!   get, and it is also where a glyph the atlas had no room for lands on its
//!   own. Correct pixels, slower — never wrong ones.
//!
//! The two paths are not pixel-identical by construction and are not claimed to
//! be: an atlas glyph is rasterized once at its quantized size and sampled,
//! an outline glyph is rasterized per frame at its exact transform.
//!
//! The *size* that quantization applies to is the device one — `font_size` with
//! the run's own transform scale absorbed into it, which is the quantity
//! `glifo` builds its key from. [`atlas_policy::device_font_size`] is where that
//! is restated, and its module doc has the whole reason a guard watching the
//! display list's `font_size` alone watches the wrong number.
//!
//! # Which glyphs never reach the atlas at all
//!
//! A colour (COLR) glyph is not merely uncacheable on this tier — cached, it is
//! *destructive*: `glifo` records it as a clip bracket around a colour-layer
//! stream, into the command recorder shared by every glyph on its atlas page,
//! and a replay that cannot lower a clip loses the whole page while its entries
//! stay resident pointing at texels nothing wrote. `glifo` 0.3.0 has no way to
//! withdraw them afterwards, so the run is refused the route beforehand:
//! [`font_has_color_glyphs`] reads the face's table directory once per run, and
//! a face carrying `COLR` draws every glyph through [`color`]'s layer
//! recombination instead.
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

use glifo::{AtlasCacher, FontEmbolden, Glyph, GlyphPrepCache, GlyphRunBuilder};
use kurbo::Affine;
use peniko::FontData;
use vello_common::paint::Paint;
use vello_common::strip_generator::StripGenerator;

use frust_scene::GlyphRun;

use crate::cache::ImageResidency;
use crate::compile::clip::ClipStack;
use crate::compile::{CompiledFrame, DepthCounter};
use crate::config;

pub(crate) use atlas_policy::AtlasPolicy;
pub(crate) use atlas_policy::{RunKey, RunRoute};

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
/// `glifo` reads back for a colour glyph's context colour. `hint` is the
/// device-class half of the hinting policy this module's doc splits out —
/// [`SceneCompiler`](crate::compile::SceneCompiler)'s own choice, passed
/// through unchanged; `glifo` still applies its transform-shape half on top
/// (see the module doc).
///
/// `cacher` is [`atlas_policy`]'s answer for this run, already decided:
/// [`AtlasCacher::Enabled`] over the policy's own entry map and the shared
/// atlas allocator for a run it routed to the atlas, [`AtlasCacher::Disabled`]
/// for one it routed to outlines. Nothing here re-decides it — the
/// classification happens once, before the brush is even encoded, so a run
/// cannot be routed one way by the policy and drawn the other.
///
/// Returns what the run cost: how many glyphs became draws, and how many were
/// refused.
pub(crate) fn lower_glyph_run(
    run: &GlyphRun,
    transform: Affine,
    paint: Paint,
    context_brush: &peniko::Brush,
    hint: bool,
    cacher: AtlasCacher<'_>,
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
        EngineTextBackend::new(&mut sink, prep, cacher),
    )
    .font_size(run.font_size)
    .font_embolden(FontEmbolden::default())
    .normalized_coords(&[])
    .hint(hint)
    .fill_glyphs(run.glyphs.iter().map(|glyph| Glyph {
        id: glyph.id,
        x: glyph.x,
        y: glyph.y,
    }));

    sink.outcome()
}

/// The glyph-atlas policy for `images`' allocator.
///
/// The policy packs into the residency's own
/// [`ImageCache`](vello_common::image_cache::ImageCache) — the process's single
/// atlas allocator — rather than one of its own, so a glyph slot and an image
/// slot can never be handed the same `ImageId` or overlapping texels of the
/// same layer. [`crate::cache::images`] states why that has to be one cache;
/// here it is simply where the policy's pages come from.
///
/// The tier decision therefore arrives already made. That geometry is
/// [`AtlasBudget::for_caps`](crate::cache::AtlasBudget::for_caps)' — mobile
/// `(1024, 1024)` x4 layers, desktop `(2048, 2048)` x8,
/// `FRUST_ENGINE_ATLAS_SIZE` redistributing that tier's own allowance between
/// extent and depth, then clamped to what the adapter can actually create —
/// applied when the residency was built, and read back off the allocator rather
/// than derived a second time from the same `TierCaps`. Deriving it twice is
/// exactly the shape that let the two halves disagree.
///
/// `FRUST_ENGINE_NO_ATLAS` is read here, once per policy, rather than per run —
/// it is a process-global kill switch ([`config::atlas_disabled`]), and a run
/// that consulted it separately could not be told from one the size tracker
/// refused. A residency that is disabled for any other reason takes the glyphs
/// with it: the two classes share one array, and caching glyphs into an atlas
/// the images were denied would be one class living in a texture the other was
/// told does not exist.
#[must_use]
pub(crate) fn glyph_atlas_policy(images: &ImageResidency) -> AtlasPolicy {
    AtlasPolicy::new(
        images.allocator(),
        images.is_disabled() || config::atlas_disabled(),
    )
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
/// `COLR`, on the same terms — the colour-glyph table
/// [`font_has_color_glyphs`] looks for.
const COLR_TAG: [u8; 4] = *b"COLR";
/// A font collection's own file tag.
const TTC_TAG: [u8; 4] = *b"ttcf";
/// Byte offset of a collection header's major version. Only versions 1 and 2
/// are defined; anything else is a collection this backend does not
/// understand and refuses rather than guesses at.
const TTC_MAJOR_VERSION: usize = 4;
/// Byte offset of a collection header's font count.
const TTC_NUM_FONTS: usize = 8;
/// Byte offset of a collection header's first table-directory offset.
const TTC_OFFSETS: usize = 12;
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
        let major_version = read_u16(data, TTC_MAJOR_VERSION)?;
        if major_version != 1 && major_version != 2 {
            return None;
        }
        let count = read_u32(data, TTC_NUM_FONTS)?;
        // The full offsets array `count` implies — not just the one entry this
        // index names — must fit inside the blob before any entry in it is
        // trusted. `read-fonts`' own `TTCHeader::table_directory_offsets()`
        // makes the same check, but fails open on a miss: a `numFonts` too
        // large for the blob does not error, it silently yields an *empty*
        // array, so `CollectionRef::get` returns `InvalidCollectionIndex` and
        // `glifo`'s `FontRef::from_index(..).unwrap()` aborts the process
        // (panic=abort) rather than returning an `EngineError`. Checking the
        // array's own fit here — before `index >= count` even runs — is what
        // keeps this gate from accepting a blob whose one requested entry
        // happens to lie inside a header `read-fonts` itself would refuse.
        if TTC_OFFSETS.checked_add(usize::try_from(count).ok()?.checked_mul(4)?)? > data.len() {
            return None;
        }
        if index >= count {
            return None;
        }
        let entry = TTC_OFFSETS.checked_add(usize::try_from(index).ok()?.checked_mul(4)?)?;
        let inner = usize::try_from(read_u32(data, entry)?).ok()?;
        // The inner table directory is itself a single-font header, so it
        // must carry the same known sfnt tag the single-font branch checks
        // at offset 0 — otherwise the bound check above validates a
        // directory `glifo` would refuse to parse.
        let inner_tag: [u8; 4] = data.get(inner..inner.checked_add(4)?)?.try_into().ok()?;
        if !SFNT_TAGS.contains(&inner_tag) {
            return None;
        }
        inner
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

/// Whether `font`'s face carries a `COLR` table, and so could hand `glifo` a
/// colour glyph.
///
/// The gate that keeps colour glyphs off the atlas route — see
/// [`atlas_policy`]'s module doc for why a cached COLR glyph voids the whole
/// atlas page it lands on, and why `glifo` 0.3.0 offers no way to undo that
/// after the fact. A face this answers `true` for has every one of its runs
/// routed to outlines, which is where `text::color` recombines a colour glyph's
/// layers into engine shapes.
///
/// `COLR` alone, because `COLR` alone is what `glifo` looks at: it resolves
/// colour glyphs through `skrifa`'s `color_glyphs()`, which reads COLRv0/v1 and
/// nothing else. A bitmap strike (`CBDT`/`sbix`) reaches the atlas as a
/// pre-rasterized pixmap through the upload queue rather than as recorded
/// commands, so it dirties no page and needs no gate; an `SVG ` table is not a
/// path this backend takes at all.
///
/// Answers per face, not per glyph: `glifo` decides colour-glyph caching from
/// the presence of a cacher, so there is no way to offer a run's outlines the
/// atlas while holding its colour glyphs back. Refusing the whole face is
/// conservative in the direction that costs speed rather than pixels — an
/// emoji font's Latin glyphs, if it has any, rasterize per frame.
pub(crate) fn font_has_color_glyphs(font: &FontData) -> bool {
    let data = font.data.data();
    table_directory(data, font.index)
        .and_then(|directory| table_record(data, directory, COLR_TAG))
        .is_some()
}

/// Where the record for `tag` starts inside the table directory at
/// `directory`, or `None` when the directory names no such table.
///
/// The first record carrying the tag, which is the one the parser resolves it
/// to whether or not the directory is sorted.
fn table_record(data: &[u8], directory: usize, tag: [u8; 4]) -> Option<usize> {
    let records = read_u16(data, directory.checked_add(4)?)?;

    for index in 0..usize::from(records) {
        let record = directory
            .checked_add(TABLE_DIRECTORY_LEN)?
            .checked_add(index.checked_mul(TABLE_RECORD_LEN)?)?;
        let found: [u8; 4] = data.get(record..record.checked_add(4)?)?.try_into().ok()?;
        if found == tag {
            return Some(record);
        }
    }

    None
}

/// The units-per-em of the `head` table named by the directory at
/// `directory`, or `None` when there is no readable one.
fn head_units_per_em(data: &[u8], directory: usize) -> Option<u16> {
    let record = table_record(data, directory, HEAD_TAG)?;

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

    read_u16(data, offset.checked_add(HEAD_UNITS_PER_EM)?)
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
