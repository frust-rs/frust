//! Glyphs of an `hvgl`-only face, drawn from CoreText's outlines.
//!
//! Apple ships some system faces — PingFang, the iOS Han fallback, is the one
//! that matters — with their outlines only in an `hvgl` table and no `glyf`,
//! `CFF ` or `CFF2` table at all. `skrifa` cannot read `hvgl`, so `glifo`
//! finds no outline for any glyph of such a face and silently skips it: the
//! text comes out blank and is not even counted as skipped. CoreText can read
//! it, so a run in such a face never reaches `glifo`. Instead each glyph's
//! outline is asked of CoreText (`CTFontCreatePathForGlyph`) once, kept as a
//! `kurbo::BezPath` in font units, and filled per frame through the engine's
//! own glyph sink — the path every outline glyph takes, under the transform
//! `glifo` builds for an unhinted outline glyph ([`glyph_transform`]).
//!
//! # Which faces
//!
//! [`font_needs_hvgl_provider`] answers per face, from its table directory: an
//! `hvgl` table and none of the three outline tables `skrifa` reads. A face
//! carrying both keeps the ordinary route, since `glifo` can draw it. Such a
//! run is classified like any other and, when the atlas takes it, inserted
//! frust-side ([`super::hvgl_atlas`]): this module then supplies the outline
//! each new glyph is rasterized from, and the outline a glyph the atlas
//! refused is filled from. Off that route (`SceneCompiler::set_hvgl_atlas`,
//! or a face or brush the insert cannot take) every glyph is filled per frame
//! as below.
//!
//! # Opening the face in CoreText
//!
//! By the PostScript name the run's handle carries
//! ([`FontHandle::postscript_name`](frust_scene::FontHandle::postscript_name)):
//! the text tier's font oracle already resolved which face a run draws with,
//! so this module does not re-derive the name from the blob. The two routes
//! that would open the bytes themselves do not work for the face this exists
//! for: `CTFontManagerCreateFontDescriptorsFromData` returns no descriptor at
//! all for `PingFangUI.ttc`, and the file-URL route expands named instances, so
//! its indices are not the collection's. A handle with no name cannot be
//! opened, and neither can one whose name CoreText does not honour: its glyphs
//! are counted in
//! [`CompiledFrame::skipped_glyphs`](crate::compile::CompiledFrame::skipped_glyphs),
//! as are all of them on a target without CoreText.
//!
//! # Drawability
//!
//! A run's glyph ids index the face the shaper chose, so they are drawn only
//! from that face. The face CoreText hands back must carry exactly the
//! PostScript name asked for; a substitute (a hidden, dot-prefixed face
//! CoreText cannot resolve by name comes back as another font) is a refusal,
//! cached like a face, warned once with the names escaped and counted
//! ([`HvglOutlines::identity_mismatches`]). A nameless handle is refused the
//! same way ([`HvglOutlines::nameless_faces`]). A hidden handle therefore
//! draws only when CoreText honours its name.
//!
//! A name is not enough on its own: an app can register its own copy of a
//! face under a system face's PostScript name, and a copy whose glyph ids
//! differ from the installed face's would pass by name and draw the installed
//! outlines under the wrong ids. So before a face opened by name is used, the
//! handle's bytes and the CoreText face must agree on their glyph count
//! (`maxp.numGlyphs` against `CTFontGetGlyphCount`), their `unitsPerEm` and
//! their `fvar` axis tags, in order ([`FaceShape`]), and on the glyph each of
//! a sample of the bytes' `cmap` codepoints maps to (the middle codepoint of
//! `CMAP_SAMPLES` ranges spread over the best Unicode subtable, against
//! `CTFontGetGlyphsForCharacters`; [`cmap_disagreement`]). A disagreement is a
//! refusal like the others, warned once with the name escaped (a codepoint as
//! `U+XXXX`) and counted ([`HvglOutlines::bytes_mismatches`]). The handle does
//! not say whether its bytes are a registered copy or the installed file the
//! oracle mapped the name to, so every face is checked when it is first
//! opened: two getters, one copy of the installed face's `fvar` table and one
//! character-map lookup, once per open.
//!
//! Refusals are remembered apart from the open faces, in a set of at most
//! `MAX_REFUSED_FACES` (oldest out), so no refusal evicts a live face. One
//! warning and one count go to each face while the set holds it; a refused
//! face that has aged out of the set is counted again if a run asks for it.
//!
//! # Variation instances
//!
//! PingFang's faces are variable (`wght`, plus Apple's `WDTH` and `HGHT`), and
//! a run carries the normalized coordinates its shaper chose
//! ([`FontHandle::normalized_coords`](frust_scene::FontHandle::normalized_coords)).
//! Those are F2Dot14 values after the face's `avar` mapping, one per `fvar`
//! axis; CoreText wants user values (`wght` 600). So each is mapped back
//! through the inverse of the axis's `avar` segment map, then denormalized
//! against the axis's `fvar` minimum, default and maximum ([`user_value`]),
//! and the face is copied with a `kCTFontVariationAttribute` dictionary naming
//! every axis (axis tag to user value). The copy carries no failure signal, so
//! the instance is read back from it (`CTFontCopyVariationAxes`,
//! `CTFontCopyVariation`) and kept only when every axis holds the value asked
//! for within the axis's precision ([`unapplied_axis`]). Otherwise the
//! instance is refused — warned once, counted
//! ([`HvglOutlines::variation_mismatches`]), its glyphs unavailable and
//! nothing cached under its key — never drawn from another instance. Each
//! face keeps its few most recently used instances open
//! ([`MAX_INSTANCES_PER_FACE`]). A run with no coordinates — every
//! non-variable face, and a variable one at its default — draws from the face
//! as opened, exactly as before.
//!
//! # The cache
//!
//! Outlines are retained in a least-recently-used cache bounded by entry count
//! ([`MAX_CACHED_OUTLINES`]), keyed by the open face (one per PostScript name, blob
//! id and face index) plus the glyph id and a [`VariationKey`] built
//! from the run's coordinates, so one instance's outline is never served for
//! another's. A glyph CoreText cannot supply is remembered as missing too, so a
//! refusal is paid for once.

use std::collections::{BTreeMap, HashMap, VecDeque};

use frust_scene::{FontHandle, GlyphRun};
use glifo::DrawSink as _;
use kurbo::{Affine, BezPath, PathEl};
use peniko::FontData;

use super::backend::EngineGlyphSink;
use super::{head_units_per_em, read_u16, read_u32, table_directory, table_record};

/// `hvgl`, as a table record's tag reads.
const HVGL_TAG: [u8; 4] = *b"hvgl";
/// The outline tables `skrifa` reads: a face carrying any of them is one
/// `glifo` can draw on its own.
const OUTLINE_TAGS: [[u8; 4]; 3] = [*b"glyf", *b"CFF ", *b"CFF2"];
/// `fvar`, the face's variation axes.
const FVAR_TAG: [u8; 4] = *b"fvar";
/// `maxp`, whose `numGlyphs` is the face's glyph count.
const MAXP_TAG: [u8; 4] = *b"maxp";
/// Where `numGlyphs` sits in a `maxp` table (either version).
const MAXP_NUM_GLYPHS: usize = 4;
/// `avar`, the face's per-axis normalization remapping.
const AVAR_TAG: [u8; 4] = *b"avar";
/// An `fvar` header's fixed length, ahead of its axis records.
const FVAR_HEADER_LEN: usize = 16;
/// The shortest `fvar` axis record: tag, minimum, default, maximum, flags and
/// name id.
const FVAR_AXIS_MIN_LEN: usize = 20;
/// An `avar` header's fixed length, ahead of its segment maps.
const AVAR_HEADER_LEN: usize = 8;
/// `cmap`, the face's character map.
const CMAP_TAG: [u8; 4] = *b"cmap";
/// How many codepoints of the bytes' `cmap` are compared with CoreText's
/// character map when a face is opened.
const CMAP_SAMPLES: usize = 16;
/// One F2Dot14 unit: a normalized coordinate of 1.0.
const F2DOT14_ONE: f64 = 16384.0;
/// One 16.16 `Fixed` unit.
const FIXED_ONE: f64 = 65536.0;

/// How many glyph outlines the cache keeps.
///
/// Bounded by count rather than bytes because a glyph's cost is roughly
/// constant: PingFang's Han glyphs measured 57-133 path elements each (spike
/// h2-01), and a `kurbo::PathEl` is 56 bytes, so an entry is about 3-7.5 KiB
/// and a full cache about 3-7.5 MiB. 1024 distinct glyphs is a dense phone
/// page of Han text with room to spare; a page past it re-fetches its oldest
/// glyphs from CoreText (about 6 us each on an iPhone 13 mini) rather than
/// growing without bound.
pub(crate) const MAX_CACHED_OUTLINES: usize = 1024;

/// How many faces stay open in CoreText at once. A frame draws a handful of
/// fallback faces; one evicted is simply opened again by name.
const MAX_OPEN_FACES: usize = 16;

/// How many refused faces are remembered at once, oldest out. Kept apart from
/// the open faces, so no refusal evicts a live one.
const MAX_REFUSED_FACES: usize = 256;

/// How many variation instances of one face stay open at once. A screen uses
/// a few weights of a family (regular, medium, semibold); one evicted is
/// copied from the face again.
pub(crate) const MAX_INSTANCES_PER_FACE: usize = 8;

/// Which variation instance of a face an outline belongs to, derived from a
/// run's normalized coordinates.
///
/// Up to three coordinates — every face this module exists for: PingFang has
/// `WDTH`, `wght` and `HGHT` — are packed exactly, with their count, so two
/// distinct instances never share a key. A face with more axes keys on a
/// 64-bit hash of its coordinates, tagged by the top bit so it cannot collide
/// with a packed key. Coordinates that are all zero name the default instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct VariationKey(u64);

impl VariationKey {
    /// The face's default instance — no variation coordinates.
    pub(crate) const DEFAULT: Self = Self(0);

    /// The coordinates packed exactly when there are at most this many.
    const PACKED_AXES: usize = 3;

    /// The key for `coords`, normalized F2Dot14 values in `fvar` axis order.
    pub(crate) fn of(coords: &[i16]) -> Self {
        let coords = match coords.iter().rposition(|coord| *coord != 0) {
            Some(last) => &coords[..=last],
            None => return Self::DEFAULT,
        };
        if coords.len() <= Self::PACKED_AXES {
            let packed = coords.iter().fold(0_u64, |packed, coord| {
                (packed << 16) | u64::from(coord.cast_unsigned())
            });
            // The count (1..=3) sits above the 48 coordinate bits, so the key
            // is never zero and never has the hash tag bit.
            return Self(((coords.len() as u64) << 48) | packed);
        }
        // FNV-1a over the coordinates' bytes.
        let hash = coords
            .iter()
            .flat_map(|coord| coord.to_be_bytes())
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            });
        Self(hash | (1 << 63))
    }
}

/// One cached outline's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct OutlineKey {
    /// The open face's process-unique id.
    face: u64,
    /// The glyph.
    glyph_id: u32,
    /// The variation instance the outline was fetched at.
    variation: VariationKey,
}

/// What the provider learned about one glyph.
#[derive(Debug)]
enum Fetched {
    /// Its outline, in font units with y up.
    Path(BezPath),
    /// A glyph with no ink — a space. Drawn as nothing and not counted as
    /// skipped, exactly as an outline glyph with no ink is on `glifo`'s route.
    Empty,
    /// An inked glyph CoreText returned no outline for, or a glyph id outside
    /// the face.
    Missing,
}

/// What [`HvglOutlines::outline`] answers for one glyph.
#[derive(Debug)]
pub(crate) enum GlyphOutline<'a> {
    /// The glyph's outline in font units (y up), and the face's units-per-em.
    Path {
        /// The outline.
        path: &'a BezPath,
        /// The face's `head.unitsPerEm`, which scales the outline to a size.
        units_per_em: f64,
    },
    /// A glyph with no ink: nothing to draw, nothing skipped.
    Empty,
    /// The glyph cannot be drawn — its face was refused, or CoreText has no
    /// outline for it. Counted as skipped.
    Unavailable,
}

/// Whether `font`'s face keeps its outlines only in an `hvgl` table, so its
/// glyphs must come from this module rather than from `glifo`.
pub(crate) fn font_needs_hvgl_provider(font: &FontData) -> bool {
    let data = font.data.data();
    table_directory(data, font.index).is_some_and(|directory| {
        table_record(data, directory, HVGL_TAG).is_some()
            && OUTLINE_TAGS
                .iter()
                .all(|tag| table_record(data, directory, *tag).is_none())
    })
}

/// The draw transform of one glyph of an `hvgl` run: the run's device
/// transform, the glyph's run-space position, then font units scaled to the
/// run's size with y flipped — the unhinted outline transform `glifo` builds.
pub(crate) fn glyph_transform(
    run: Affine,
    x: f32,
    y: f32,
    font_size: f32,
    units_per_em: f64,
) -> Affine {
    let scale = f64::from(font_size) / units_per_em;
    run * Affine::translate((f64::from(x), f64::from(y))) * Affine::scale_non_uniform(scale, -scale)
}

/// Fill every glyph of `run` from `outlines` into `sink`, under the run's
/// device `transform`, answering how many glyphs could not be drawn.
///
/// The glyphs are drawn at the variation instance the run's handle carries.
pub(crate) fn draw_run(
    run: &GlyphRun,
    transform: Affine,
    sink: &mut EngineGlyphSink<'_>,
    outlines: &mut HvglOutlines,
) -> u32 {
    let mut unavailable = 0_u32;
    for glyph in &run.glyphs {
        if !draw_glyph(run, transform, glyph, sink, outlines) {
            unavailable = unavailable.saturating_add(1);
        }
    }
    unavailable
}

/// Fill one glyph of `run` from `outlines` into `sink` under the run's device
/// `transform`, answering `false` when it could not be drawn (a glyph with no
/// ink draws nothing and answers `true`).
pub(crate) fn draw_glyph(
    run: &GlyphRun,
    transform: Affine,
    glyph: &frust_scene::Glyph,
    sink: &mut EngineGlyphSink<'_>,
    outlines: &mut HvglOutlines,
) -> bool {
    match outlines.outline(&run.font, glyph.id) {
        GlyphOutline::Path { path, units_per_em } => {
            sink.set_transform(glyph_transform(
                transform,
                glyph.x,
                glyph.y,
                run.font_size,
                units_per_em,
            ));
            sink.fill_path(path);
            true
        }
        GlyphOutline::Empty => true,
        GlyphOutline::Unavailable => false,
    }
}

/// The compiler's retained `hvgl` state: the faces open in CoreText and the
/// bounded outline cache.
pub(crate) struct HvglOutlines {
    /// The faces opened (or refused) most recently.
    faces: FaceTable,
    /// The outlines fetched, least recently used evicted first.
    outlines: OutlineCache,
    /// Glyphs the draw routes could not supply.
    unavailable: u64,
    /// Variation instances refused because CoreText's copy did not carry the
    /// user values asked for.
    variation_mismatches: u32,
}

impl Default for HvglOutlines {
    fn default() -> Self {
        Self::with_capacity(MAX_CACHED_OUTLINES)
    }
}

impl core::fmt::Debug for HvglOutlines {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HvglOutlines")
            .field("open_faces", &self.faces.open.len())
            .field("refused_faces", &self.faces.refused.len())
            .field("outlines", &self.outlines.entries.len())
            .field("capacity", &self.outlines.capacity)
            .finish()
    }
}

impl HvglOutlines {
    /// A provider whose outline cache keeps at most `capacity` glyphs (at
    /// least one).
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            faces: FaceTable::default(),
            outlines: OutlineCache::new(capacity),
            unavailable: 0,
            variation_mismatches: 0,
        }
    }

    /// Whether `handle`'s face can be drawn from CoreText: opened (or opened
    /// now) by its PostScript name with no refusal. Never on a target without
    /// CoreText. A refusal is counted and warned once, as on a first draw.
    pub(crate) fn face_drawable(&mut self, handle: &FontHandle) -> bool {
        self.faces.open(handle).is_some()
    }

    /// How many glyph outlines are cached.
    pub(crate) fn cached_outlines(&self) -> usize {
        self.outlines.entries.len()
    }

    /// How many glyph outlines the cache keeps at most.
    pub(crate) fn capacity(&self) -> usize {
        self.outlines.capacity
    }

    /// How many faces CoreText resolved to a different PostScript name than
    /// the one asked for, since the provider was built. Each was refused.
    pub(crate) fn identity_mismatches(&self) -> u32 {
        self.faces.mismatches
    }

    /// How many faces were refused for carrying no PostScript name, since the
    /// provider was built.
    pub(crate) fn nameless_faces(&self) -> u32 {
        self.faces.nameless
    }

    /// How many faces were refused because their bytes and the CoreText face
    /// their name opened disagree (glyph count, `unitsPerEm` or `fvar` axes),
    /// since the provider was built.
    pub(crate) fn bytes_mismatches(&self) -> u32 {
        self.faces.bytes_mismatches
    }

    /// How many variation instances were refused because the copy CoreText
    /// made of a face did not carry the user values asked for (read back from
    /// the copy), since the provider was built. Each instance counts once per
    /// open face; its glyphs are unavailable rather than drawn from another
    /// instance.
    pub(crate) fn variation_mismatches(&self) -> u32 {
        self.variation_mismatches
    }

    /// How many outline fetches the cache answered without asking CoreText,
    /// since the provider was built (saturating).
    ///
    /// Counted per fetch call, not per distinct glyph: on the atlas route an
    /// absent glyph is fetched twice (measured, then rasterized into the
    /// atlas), so one new inked glyph reads as one miss and one hit while the
    /// frame's new glyphs fit the cache; an inkless glyph is only measured,
    /// and a glyph resident in the atlas never fetches at all.
    pub(crate) fn outline_hits(&self) -> u64 {
        self.outlines.hits
    }

    /// How many outline fetches asked CoreText (a cache miss), since the
    /// provider was built (saturating). A miss costs a path read and build,
    /// plus a font copy for each new variation instance. Counted per fetch
    /// call, as [`Self::outline_hits`] explains.
    pub(crate) fn outline_misses(&self) -> u64 {
        self.outlines.misses
    }

    /// How many glyphs of `hvgl` runs could not be drawn, since the provider
    /// was built (saturating): the face refused, or CoreText had no path.
    /// Added by the draw routes through [`Self::note_unavailable`].
    pub(crate) fn unavailable_glyphs(&self) -> u64 {
        self.unavailable
    }

    /// Add `count` glyphs a draw route could not supply to
    /// [`Self::unavailable_glyphs`].
    pub(crate) fn note_unavailable(&mut self, count: u32) {
        self.unavailable = self.unavailable.saturating_add(u64::from(count));
    }

    /// `glyph_id`'s outline in `handle`'s face at the variation instance its
    /// coordinates name (normalized F2Dot14 values in `fvar` axis order; empty
    /// for the default instance), fetched from CoreText on a cache miss.
    ///
    /// Coordinates on a face with no `fvar` axes name nothing to vary, and
    /// draw from the face as opened. A refused instance answers
    /// [`GlyphOutline::Unavailable`] and caches nothing under its key.
    pub(crate) fn outline(&mut self, handle: &FontHandle, glyph_id: u32) -> GlyphOutline<'_> {
        let Self {
            faces,
            outlines,
            variation_mismatches,
            ..
        } = self;
        let coords = handle.normalized_coords();
        let Some(face) = faces.open(handle) else {
            return GlyphOutline::Unavailable;
        };
        let units_per_em = face.units_per_em;
        let variation = if face.axes.is_empty() {
            VariationKey::DEFAULT
        } else {
            VariationKey::of(coords)
        };
        let key = OutlineKey {
            face: face.id,
            glyph_id,
            variation,
        };
        let fetched = outlines.get_or_fetch(key, || {
            let name = handle.postscript_name().unwrap_or_default();
            face.instance(variation, coords, name, variation_mismatches)
                .map(|instance| fetch(instance, glyph_id))
        });
        match fetched {
            Some(Fetched::Path(path)) => GlyphOutline::Path { path, units_per_em },
            Some(Fetched::Empty) => GlyphOutline::Empty,
            Some(Fetched::Missing) | None => GlyphOutline::Unavailable,
        }
    }
}

/// A bounded least-recently-used map from glyph to outline.
///
/// Each entry carries the tick it was last used at, and `order` maps ticks
/// back to keys, so the oldest entry is the first key of `order` — a lookup,
/// a touch and an eviction are each a hash lookup plus a `BTreeMap` step.
#[derive(Debug)]
struct OutlineCache {
    /// The most entries kept.
    capacity: usize,
    /// Outline and last-use tick per glyph.
    entries: HashMap<OutlineKey, (Fetched, u64)>,
    /// Last-use tick to glyph, oldest first.
    order: BTreeMap<u64, OutlineKey>,
    /// The use counter ticks are drawn from.
    tick: u64,
    /// Fetch calls answered from the cache, saturating.
    hits: u64,
    /// Fetch calls that had to ask CoreText, saturating.
    misses: u64,
}

impl OutlineCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// `key`'s entry, marked most recently used, or `fetch`'s answer inserted
    /// in its place — evicting the least recently used entry when full. A
    /// `fetch` answering `None` (a refused instance) inserts and evicts
    /// nothing.
    ///
    /// Counts one hit or one miss per call.
    fn get_or_fetch(
        &mut self,
        key: OutlineKey,
        fetch: impl FnOnce() -> Option<Fetched>,
    ) -> Option<&Fetched> {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        if let Some((_, used)) = self.entries.get_mut(&key) {
            self.order.remove(used);
            *used = tick;
            self.hits = self.hits.saturating_add(1);
        } else {
            self.misses = self.misses.saturating_add(1);
            let fetched = fetch()?;
            if self.entries.len() >= self.capacity
                && let Some((_, oldest)) = self.order.pop_first()
            {
                self.entries.remove(&oldest);
            }
            self.entries.insert(key, (fetched, tick));
        }
        self.order.insert(tick, key);
        self.entries.get(&key).map(|(fetched, _)| fetched)
    }
}

/// One face open in CoreText.
struct Face {
    /// A process-unique id, which the outline cache keys on. A face evicted
    /// and opened again is a new id; its old outlines age out of the cache.
    id: u64,
    /// The CoreText face, opened at `units_per_em` points so its outlines are
    /// in font units — the face's default instance.
    ct: platform::CtFace,
    /// The face's `head.unitsPerEm`.
    units_per_em: f64,
    /// The face's variation axes, in `fvar` order; empty for a face that does
    /// not vary.
    axes: Vec<Axis>,
    /// The variation instances copied from `ct`, with their last use.
    instances: Vec<(VariationKey, platform::CtFace, u64)>,
    /// The instances refused (CoreText's copy did not carry them), oldest
    /// first, at most [`MAX_INSTANCES_PER_FACE`].
    refused: VecDeque<VariationKey>,
    /// CoreText's own axis defaults for `ct`, read on the first instance
    /// copied (an instance's read-back fills in the axes it leaves out).
    coretext_defaults: Option<Vec<(u32, f64)>>,
    /// A use counter, for the least-recently-used replacement of `instances`.
    uses: u64,
}

impl Face {
    /// The CoreText face to read `variation`'s outlines from: the face as
    /// opened for the default instance, else a copy at the user values
    /// `coords` maps to, copied on first use — or `None` when that copy is
    /// refused.
    ///
    /// A copy is kept only when the instance CoreText reads back from it
    /// carries every user value asked for ([`unapplied_axis`]); else drawing
    /// from it would cache another instance's outlines under this one's key.
    /// A refusal is counted in `refusals` and warned once (the face's `name`
    /// escaped), and remembered while the face stays open.
    fn instance(
        &mut self,
        variation: VariationKey,
        coords: &[i16],
        name: &str,
        refusals: &mut u32,
    ) -> Option<&platform::CtFace> {
        if variation == VariationKey::DEFAULT {
            return Some(&self.ct);
        }
        self.uses = self.uses.wrapping_add(1);
        let position = match self
            .instances
            .iter()
            .position(|(key, ..)| *key == variation)
        {
            Some(position) => position,
            None => {
                if self.refused.contains(&variation) {
                    return None;
                }
                let values = user_values(&self.axes, coords);
                let defaults = self
                    .coretext_defaults
                    .get_or_insert_with(|| self.ct.axis_defaults());
                let checked = match self.ct.varied(&values, self.units_per_em) {
                    Some(varied) => {
                        let applied = applied_variation(defaults, &varied);
                        match unapplied_axis(&self.axes, &values, &applied) {
                            None => Ok(varied),
                            Some(unapplied) => Err(unapplied.to_string()),
                        }
                    }
                    None => Err("CoreText made no copy".to_owned()),
                };
                let varied = match checked {
                    Ok(varied) => varied,
                    Err(why) => {
                        self.refuse(variation, name, &values, &why, refusals);
                        return None;
                    }
                };
                if self.instances.len() >= MAX_INSTANCES_PER_FACE
                    && let Some(oldest) = self
                        .instances
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, (.., used))| *used)
                        .map(|(position, _)| position)
                {
                    self.instances.swap_remove(oldest);
                }
                self.instances.push((variation, varied, 0));
                self.instances.len() - 1
            }
        };
        let entry = self.instances.get_mut(position)?;
        entry.2 = self.uses;
        Some(&entry.1)
    }

    /// Remembers `variation` as refused (oldest refusal out at the cap),
    /// counting it in `refusals` and warning once.
    fn refuse(
        &mut self,
        variation: VariationKey,
        name: &str,
        values: &[(u32, f64)],
        why: &str,
        refusals: &mut u32,
    ) {
        if self.refused.len() >= MAX_INSTANCES_PER_FACE {
            self.refused.pop_front();
        }
        self.refused.push_back(variation);
        *refusals = refusals.saturating_add(1);
        log::warn!(
            "frust-engine: hvgl face {:?} at [{}] refused: CoreText's copy is another instance \
             ({why}); its glyphs at that instance are counted as skipped",
            truncated(name),
            instance_list(values),
        );
    }
}

/// The instance CoreText reads back from `varied`, as `(axis tag, user
/// value)`: every axis of `defaults` (CoreText's own, from the face it was
/// copied from) at the value the copy reports, else at its default.
fn applied_variation(defaults: &[(u32, f64)], varied: &platform::CtFace) -> Vec<(u32, f64)> {
    let reported = varied.variation_values();
    let applied = defaults
        .iter()
        .map(|(tag, default)| {
            let value = reported
                .iter()
                .find(|(reported, _)| reported == tag)
                .map_or(*default, |(_, value)| *value);
            (*tag, value)
        })
        .collect();
    #[cfg(test)]
    let applied = tests::skewed(applied);
    applied
}

/// One axis CoreText's copy does not carry as asked.
#[derive(Debug, Clone, Copy, PartialEq)]
struct UnappliedAxis {
    /// The axis tag.
    tag: u32,
    /// The user value asked for.
    asked: f64,
    /// The value read back; `None` when CoreText reports no such axis.
    applied: Option<f64>,
}

impl core::fmt::Display for UnappliedAxis {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let tag = tag_list(&[self.tag]);
        match self.applied {
            Some(applied) => write!(f, "{tag} read back as {applied}, asked {}", self.asked),
            None => write!(f, "{tag} is not an axis of CoreText's copy"),
        }
    }
}

/// The first axis of `asked` (`(axis tag, user value)`, every axis of
/// `axes`) that `applied` — the instance CoreText reads back from its copy —
/// does not carry within the axis's precision ([`axis_precision`]), or `None`
/// when every axis was applied.
fn unapplied_axis(
    axes: &[Axis],
    asked: &[(u32, f64)],
    applied: &[(u32, f64)],
) -> Option<UnappliedAxis> {
    asked.iter().zip(axes).find_map(|((tag, asked), axis)| {
        let applied = applied
            .iter()
            .find(|(applied_tag, _)| applied_tag == tag)
            .map(|(_, value)| *value);
        let within = applied.is_some_and(|value| {
            (value - asked).abs() <= axis_precision(axis, *asked) + f64::EPSILON
        });
        (!within).then_some(UnappliedAxis {
            tag: *tag,
            asked: *asked,
            applied,
        })
    })
}

/// The finest user-value distinction `axis` makes at `value`: one F2Dot14
/// step of the normalized range on that side of the default (the resolution
/// of a run's coordinates), and no finer than one 16.16 `Fixed` unit (the
/// resolution of `fvar` itself).
fn axis_precision(axis: &Axis, value: f64) -> f64 {
    let side = if value < axis.default {
        axis.default - axis.min
    } else {
        axis.max - axis.default
    };
    (side.abs() / F2DOT14_ONE).max(1.0 / FIXED_ONE)
}

/// `values` as `tag=value` pairs, for a log line.
fn instance_list(values: &[(u32, f64)]) -> String {
    values
        .iter()
        .map(|(tag, value)| format!("{}={value}", tag_list(&[*tag])))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One `fvar` axis, with its `avar` segment map.
#[derive(Debug, Clone, PartialEq)]
struct Axis {
    /// The axis tag as a big-endian integer — CoreText's axis identifier.
    tag: u32,
    /// The axis minimum, in user units.
    min: f64,
    /// The axis default, in user units.
    default: f64,
    /// The axis maximum, in user units.
    max: f64,
    /// The axis's `avar` map as `(from, to)` normalized pairs; empty when the
    /// face has no `avar` (the identity).
    avar: Vec<(f64, f64)>,
}

/// Every axis of `coords`'s instance as `(axis tag, user value)`: each
/// coordinate mapped back through its axis's `avar`, then denormalized
/// against the axis's range. An axis `coords` stops short of sits at its
/// default, so the instance is absolute rather than relative to the face's
/// own named instance.
fn user_values(axes: &[Axis], coords: &[i16]) -> Vec<(u32, f64)> {
    axes.iter()
        .enumerate()
        .map(|(index, axis)| {
            let coord = coords.get(index).copied().unwrap_or(0);
            (axis.tag, user_value(axis, f64::from(coord) / F2DOT14_ONE))
        })
        .collect()
}

/// The user value an `avar`-mapped normalized coordinate stands for on `axis`.
fn user_value(axis: &Axis, normalized: f64) -> f64 {
    let normalized = unmap_avar(&axis.avar, normalized.clamp(-1.0, 1.0));
    if normalized < 0.0 {
        axis.default + normalized * (axis.default - axis.min)
    } else {
        axis.default + normalized * (axis.max - axis.default)
    }
}

/// The pre-`avar` coordinate `mapped` came from, through the inverse of the
/// segment map `map` (`(from, to)` pairs, ascending in both): linear between
/// the two pairs whose `to` values bracket it. A map with fewer than two
/// pairs is the identity, as the `avar` spec treats it. Where several `from`
/// values share a `to` (a flat segment), the first is answered.
fn unmap_avar(map: &[(f64, f64)], mapped: f64) -> f64 {
    if map.len() < 2 {
        return mapped;
    }
    for pair in map.windows(2) {
        let [(from0, to0), (from1, to1)] = [pair[0], pair[1]];
        if mapped >= to0 && mapped <= to1 {
            if to1 - to0 <= f64::EPSILON {
                return from0;
            }
            return from0 + (mapped - to0) * (from1 - from0) / (to1 - to0);
        }
    }
    mapped
}

/// The face's `fvar` axes with their `avar` maps, or none when it has no
/// readable `fvar`. An `avar` whose axis count disagrees with `fvar`'s is
/// ignored (the identity), as is any part of one that does not fit the blob.
fn variation_axes(data: &[u8], directory: usize) -> Vec<Axis> {
    let Some(mut axes) = fvar_axes(data, directory) else {
        return Vec::new();
    };
    if let Some(maps) = avar_maps(data, directory).filter(|maps| maps.len() == axes.len()) {
        for (axis, map) in axes.iter_mut().zip(maps) {
            axis.avar = map;
        }
    }
    axes
}

/// Where the table `tag` starts in `data`, when its record and its whole
/// extent fit.
fn table_start(data: &[u8], directory: usize, tag: [u8; 4]) -> Option<usize> {
    let record = table_record(data, directory, tag)?;
    let offset = usize::try_from(read_u32(data, record.checked_add(8)?)?).ok()?;
    let length = usize::try_from(read_u32(data, record.checked_add(12)?)?).ok()?;
    (offset.checked_add(length)? <= data.len()).then_some(offset)
}

/// The `fvar` axis records, without their `avar` maps.
fn fvar_axes(data: &[u8], directory: usize) -> Option<Vec<Axis>> {
    fvar_axes_at(data, table_start(data, directory, FVAR_TAG)?)
}

/// The axis records of the `fvar` table starting at `table` in `data` — a
/// face's, or a table CoreText copied out on its own (at 0).
fn fvar_axes_at(data: &[u8], table: usize) -> Option<Vec<Axis>> {
    if data.len() < table.checked_add(FVAR_HEADER_LEN)? {
        return None;
    }
    let first = table.checked_add(usize::from(read_u16(data, table.checked_add(4)?)?))?;
    let count = usize::from(read_u16(data, table.checked_add(8)?)?);
    let size = usize::from(read_u16(data, table.checked_add(10)?)?);
    if size < FVAR_AXIS_MIN_LEN {
        return None;
    }
    let fixed = |at: usize| -> Option<f64> {
        Some(f64::from(read_u32(data, at)?.cast_signed()) / FIXED_ONE)
    };
    (0..count)
        .map(|index| {
            let at = first.checked_add(index.checked_mul(size)?)?;
            Some(Axis {
                tag: read_u32(data, at)?,
                min: fixed(at.checked_add(4)?)?,
                default: fixed(at.checked_add(8)?)?,
                max: fixed(at.checked_add(12)?)?,
                avar: Vec::new(),
            })
        })
        .collect()
}

/// The `avar` segment maps, one per axis, as normalized `(from, to)` pairs.
/// Version 2's extra variation data is not read; its segment maps are.
fn avar_maps(data: &[u8], directory: usize) -> Option<Vec<Vec<(f64, f64)>>> {
    let table = table_start(data, directory, AVAR_TAG)?;
    let major = read_u16(data, table)?;
    if major != 1 && major != 2 {
        return None;
    }
    let count = usize::from(read_u16(data, table.checked_add(6)?)?);
    let f2dot14 = |at: usize| -> Option<f64> {
        Some(f64::from(read_u16(data, at)?.cast_signed()) / F2DOT14_ONE)
    };
    let mut at = table.checked_add(AVAR_HEADER_LEN)?;
    let mut maps = Vec::with_capacity(count);
    for _ in 0..count {
        let pairs = usize::from(read_u16(data, at)?);
        at = at.checked_add(2)?;
        let map = (0..pairs)
            .map(|pair| {
                let pair_at = at.checked_add(pair.checked_mul(4)?)?;
                Some((f2dot14(pair_at)?, f2dot14(pair_at.checked_add(2)?)?))
            })
            .collect::<Option<Vec<_>>>()?;
        at = at.checked_add(pairs.checked_mul(4)?)?;
        maps.push(map);
    }
    Some(maps)
}

/// `maxp.numGlyphs`: how many glyphs the face's bytes hold.
fn maxp_num_glyphs(data: &[u8], directory: usize) -> Option<usize> {
    let table = table_start(data, directory, MAXP_TAG)?;
    read_u16(data, table.checked_add(MAXP_NUM_GLYPHS)?).map(usize::from)
}

/// What a face's bytes and the CoreText face opened by its name must agree
/// on before the bytes' glyph ids are drawn from CoreText's outlines.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FaceShape {
    /// The glyph count; `None` when the bytes carry no readable `maxp`.
    glyph_count: Option<usize>,
    /// `unitsPerEm`; `None` when the bytes carry no usable `head`.
    units_per_em: Option<u32>,
    /// The `fvar` axis tags, in axis order; empty for a face that does not
    /// vary.
    axis_tags: Vec<u32>,
}

impl FaceShape {
    /// The shape `font`'s bytes declare.
    fn of_bytes(font: &FontData) -> Self {
        let data = font.data.data();
        let Some(directory) = table_directory(data, font.index) else {
            return Self {
                glyph_count: None,
                units_per_em: None,
                axis_tags: Vec::new(),
            };
        };
        Self {
            glyph_count: maxp_num_glyphs(data, directory),
            units_per_em: head_units_per_em(data, directory).map(u32::from),
            axis_tags: axis_tags(fvar_axes(data, directory)),
        }
    }

    /// The shape of the face CoreText opened.
    fn of_installed(face: &platform::CtFace) -> Self {
        let fvar = face.table(FVAR_TAG);
        Self {
            glyph_count: Some(face.glyph_count()),
            units_per_em: Some(face.units_per_em()),
            axis_tags: axis_tags(fvar.as_deref().and_then(|table| fvar_axes_at(table, 0))),
        }
    }

    /// Where `installed` disagrees with these bytes, for the warning; `None`
    /// when the two agree on every field.
    fn disagreement(&self, installed: &Self) -> Option<String> {
        if self.glyph_count != installed.glyph_count {
            return Some(format!(
                "glyph count {} in the bytes, {} in CoreText's face",
                or_none(self.glyph_count),
                or_none(installed.glyph_count),
            ));
        }
        if self.units_per_em != installed.units_per_em {
            return Some(format!(
                "unitsPerEm {} in the bytes, {} in CoreText's face",
                or_none(self.units_per_em),
                or_none(installed.units_per_em),
            ));
        }
        if self.axis_tags != installed.axis_tags {
            return Some(format!(
                "fvar axes [{}] in the bytes, [{}] in CoreText's face",
                tag_list(&self.axis_tags),
                tag_list(&installed.axis_tags),
            ));
        }
        None
    }
}

/// How one range of a Unicode `cmap` subtable maps its codepoints to glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeMap {
    /// Format 12: the first codepoint's glyph, the rest following in order.
    Sequential(u32),
    /// Format 4 without an `idRangeOffset`: the codepoint plus a delta,
    /// modulo 65536.
    Delta(u16),
    /// Format 4 through its `glyphIdArray`: where the range's first entry
    /// sits in the blob, and the delta added to a non-zero entry.
    Indexed {
        /// The blob offset of the first codepoint's entry.
        at: usize,
        /// Added to a non-zero entry, modulo 65536.
        delta: u16,
    },
}

/// One run of consecutive codepoints of a Unicode `cmap` subtable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CmapRange {
    /// The first codepoint.
    first: u32,
    /// The last codepoint, inclusive.
    last: u32,
    /// How the range's codepoints map to glyphs.
    map: RangeMap,
}

impl CmapRange {
    /// `codepoint`'s glyph in `data`, or `None` when it falls outside the
    /// range or its entry does not fit the blob.
    fn glyph(&self, data: &[u8], codepoint: u32) -> Option<u16> {
        let offset = codepoint.checked_sub(self.first)?;
        match self.map {
            RangeMap::Sequential(first) => u16::try_from(first.checked_add(offset)?).ok(),
            RangeMap::Delta(delta) => Some(u16::try_from(codepoint).ok()?.wrapping_add(delta)),
            RangeMap::Indexed { at, delta } => {
                let entry = read_u16(data, at.checked_add(usize::try_from(offset).ok()? * 2)?)?;
                Some(if entry == 0 {
                    0
                } else {
                    entry.wrapping_add(delta)
                })
            }
        }
    }
}

/// A face's best Unicode `cmap` subtable: format 12 (full repertoire) over
/// format 4 (BMP), as a shaper prefers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnicodeCmap {
    /// Where the subtable starts in the blob.
    at: usize,
    /// Its format, 4 or 12.
    format: u16,
    /// How many ranges (format 4 segments, format 12 groups) it declares.
    ranges: usize,
}

impl UnicodeCmap {
    /// The best Unicode subtable of the `cmap` table starting at `table`, or
    /// `None` when it has none this module reads.
    fn find(data: &[u8], table: usize) -> Option<Self> {
        let count = usize::from(read_u16(data, table.checked_add(2)?)?);
        let mut best: Option<(u8, usize, u16)> = None;
        for index in 0..count {
            let record = table.checked_add(4)?.checked_add(index.checked_mul(8)?)?;
            let platform = read_u16(data, record)?;
            let encoding = read_u16(data, record.checked_add(2)?)?;
            let offset = usize::try_from(read_u32(data, record.checked_add(4)?)?).ok()?;
            let at = table.checked_add(offset)?;
            let format = read_u16(data, at)?;
            let rank = match (platform, encoding, format) {
                (3, 10, 12) | (0, 4 | 6, 12) => 2,
                (3, 1, 4) | (0, 0..=3, 4) => 1,
                _ => continue,
            };
            if best.is_none_or(|(best_rank, ..)| rank > best_rank) {
                best = Some((rank, at, format));
            }
        }
        let (_, at, format) = best?;
        let ranges = if format == 12 {
            usize::try_from(read_u32(data, at.checked_add(12)?)?).ok()?
        } else {
            usize::from(read_u16(data, at.checked_add(6)?)? / 2)
        };
        Some(Self { at, format, ranges })
    }

    /// Range `index` (`< self.ranges`), or `None` when it does not fit the
    /// blob, is inverted, or is format 4's terminating `0xFFFF` segment.
    fn range(&self, data: &[u8], index: usize) -> Option<CmapRange> {
        if self.format == 12 {
            let group = self
                .at
                .checked_add(16)?
                .checked_add(index.checked_mul(12)?)?;
            let first = read_u32(data, group)?;
            let last = read_u32(data, group.checked_add(4)?)?;
            let map = RangeMap::Sequential(read_u32(data, group.checked_add(8)?)?);
            return (first <= last).then_some(CmapRange { first, last, map });
        }
        let segments_x2 = self.ranges.checked_mul(2)?;
        let segment = index.checked_mul(2)?;
        let ends = self.at.checked_add(14)?;
        let starts = ends.checked_add(segments_x2)?.checked_add(2)?;
        let deltas = starts.checked_add(segments_x2)?;
        let offsets = deltas.checked_add(segments_x2)?.checked_add(segment)?;
        let last = read_u16(data, ends.checked_add(segment)?)?;
        let first = read_u16(data, starts.checked_add(segment)?)?;
        let delta = read_u16(data, deltas.checked_add(segment)?)?;
        let range_offset = usize::from(read_u16(data, offsets)?);
        if first > last || first == 0xFFFF {
            return None;
        }
        let map = if range_offset == 0 {
            RangeMap::Delta(delta)
        } else {
            RangeMap::Indexed {
                at: offsets.checked_add(range_offset)?,
                delta,
            }
        };
        Some(CmapRange {
            first: u32::from(first),
            last: u32::from(last),
            map,
        })
    }
}

/// Up to `count` `(codepoint, glyph)` pairs of the face's Unicode `cmap`:
/// the middle codepoint of `count` ranges spread evenly over the subtable
/// (each the middle of an equal share of its ranges), so the cost is
/// `count` range reads however large the `cmap`. A pick that maps to glyph 0,
/// is not a scalar value or does not fit the blob is left out; none at all
/// when the face has no Unicode subtable.
fn cmap_sample(data: &[u8], directory: usize, count: usize) -> Vec<(char, u16)> {
    let Some(cmap) =
        table_start(data, directory, CMAP_TAG).and_then(|table| UnicodeCmap::find(data, table))
    else {
        return Vec::new();
    };
    let picks = count.min(cmap.ranges);
    (0..picks)
        .filter_map(|pick| {
            let index = (2 * pick + 1).checked_mul(cmap.ranges)? / (2 * picks);
            let range = cmap.range(data, index)?;
            let codepoint = range.first + (range.last - range.first) / 2;
            let glyph = range.glyph(data, codepoint).filter(|glyph| *glyph != 0)?;
            Some((char::from_u32(codepoint)?, glyph))
        })
        .collect()
}

/// Where CoreText's character map disagrees with `font`'s bytes on a sample
/// of their `cmap` ([`cmap_sample`], [`CMAP_SAMPLES`] codepoints), for the
/// warning; `None` when every sampled codepoint maps to the same glyph in
/// both. Bytes with no mapped Unicode codepoint never agree. Glyph
/// substitutions (`GSUB`) are not compared.
fn cmap_disagreement(font: &FontData, face: &platform::CtFace) -> Option<String> {
    let data = font.data.data();
    let sample = table_directory(data, font.index)
        .map(|directory| cmap_sample(data, directory, CMAP_SAMPLES))
        .unwrap_or_default();
    if sample.is_empty() {
        return Some("no mapped Unicode codepoint in the bytes' cmap".to_owned());
    }
    let characters: Vec<char> = sample.iter().map(|(character, _)| *character).collect();
    let installed = face.glyphs_for(&characters);
    if installed.len() != sample.len() {
        return Some("CoreText mapped none of the sampled codepoints".to_owned());
    }
    sample
        .iter()
        .zip(installed)
        .find(|((_, bytes), installed)| bytes != installed)
        .map(|((character, bytes), installed)| {
            format!(
                "U+{:04X} is glyph {bytes} in the bytes, {installed} in CoreText's face",
                u32::from(*character),
            )
        })
}

/// `value` for a log line, `none` when absent.
fn or_none(value: Option<impl core::fmt::Display>) -> String {
    value.map_or_else(|| "none".to_owned(), |value| value.to_string())
}

/// The tags of `axes`, in axis order (none for a face without `fvar`).
fn axis_tags(axes: Option<Vec<Axis>>) -> Vec<u32> {
    axes.unwrap_or_default()
        .iter()
        .map(|axis| axis.tag)
        .collect()
}

/// `tags` as escaped four-character codes, for a log line: the bytes come
/// from a font, so nothing in them is trusted to be printable.
fn tag_list(tags: &[u32]) -> String {
    tags.iter()
        .map(|tag| tag.to_be_bytes().escape_ascii().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// What identifies one face the table has looked at: the PostScript name
/// CoreText is asked for, plus the blob and collection index the glyph ids
/// were shaped against. Two handles sharing a name but not a blob are two faces.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FaceKey {
    /// The handle's PostScript name; `None` for a nameless handle.
    name: Option<String>,
    /// The font blob's id.
    blob: u64,
    /// The face's index within the blob.
    index: u32,
}

impl FaceKey {
    /// The key for `handle`, owning a copy of its name. Built only when a face
    /// is first stored, never on a lookup.
    fn of(handle: &FontHandle) -> Self {
        let font = handle.font();
        Self {
            name: handle.postscript_name().map(str::to_owned),
            blob: font.data.id(),
            index: font.index,
        }
    }

    /// Whether this key names `handle`'s face, compared in place without
    /// allocating.
    fn matches(&self, handle: &FontHandle) -> bool {
        let font = handle.font();
        self.blob == font.data.id()
            && self.index == font.index
            && self.name.as_deref() == handle.postscript_name()
    }
}

/// `name` cut to at most [`LOGGED_NAME_BYTES`] bytes on a character boundary,
/// for a log line (a name can come from the on-disk fallback cache).
fn truncated(name: &str) -> &str {
    let mut end = name.len().min(LOGGED_NAME_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

/// [`truncated`] for the compiler's log lines.
pub(crate) fn truncated_name(name: &str) -> &str {
    truncated(name)
}

/// The longest name a log line carries.
const LOGGED_NAME_BYTES: usize = 63;

/// The faces looked at most recently. Open faces and refusals are kept apart:
/// the open faces are an LRU bounded by `MAX_OPEN_FACES`, the refusals a
/// bounded set of their own, so churn among refusals never evicts a live face.
#[derive(Default)]
struct FaceTable {
    /// `(key, face, last use)` per open face.
    open: Vec<(FaceKey, Face, u64)>,
    /// The keys of refused faces, oldest first, at most `MAX_REFUSED_FACES`.
    refused: VecDeque<FaceKey>,
    /// A use counter, for the least-recently-used replacement.
    uses: u64,
    /// The next face id.
    next_id: u64,
    /// Faces whose CoreText PostScript name differed from the one asked for;
    /// each was refused.
    mismatches: u32,
    /// Faces whose handle carried no PostScript name; each was refused.
    nameless: u32,
    /// Faces whose bytes disagreed with the CoreText face their name opened;
    /// each was refused.
    bytes_mismatches: u32,
}

impl FaceTable {
    /// `handle`'s face open in CoreText, opening it on first sight, or `None`
    /// when the face was refused (no name, a name CoreText does not honour
    /// exactly, or one it could not open). A refusal is remembered until it
    /// ages out of the refused set.
    ///
    /// A lookup allocates nothing: keys are compared in place.
    fn open(&mut self, handle: &FontHandle) -> Option<&mut Face> {
        self.uses = self.uses.wrapping_add(1);
        if let Some(position) = self.open.iter().position(|(key, ..)| key.matches(handle)) {
            let entry = self.open.get_mut(position)?;
            entry.2 = self.uses;
            return Some(&mut entry.1);
        }
        if self.refused.iter().any(|key| key.matches(handle)) {
            return None;
        }
        self.next_id = self.next_id.wrapping_add(1);
        let Some(face) = self.open_face(handle) else {
            self.refuse(FaceKey::of(handle));
            return None;
        };
        if self.open.len() >= MAX_OPEN_FACES
            && let Some(oldest) = self
                .open
                .iter()
                .enumerate()
                .min_by_key(|(_, (.., used))| *used)
                .map(|(position, _)| position)
        {
            self.open.swap_remove(oldest);
        }
        self.open.push((FaceKey::of(handle), face, self.uses));
        Some(&mut self.open.last_mut()?.1)
    }

    /// Remembers `key` as refused, dropping the oldest refusal when the set is
    /// full.
    fn refuse(&mut self, key: FaceKey) {
        if self.refused.len() >= MAX_REFUSED_FACES {
            self.refused.pop_front();
        }
        self.refused.push_back(key);
    }

    /// `handle`'s face opened in CoreText, or `None` with one warning.
    ///
    /// The glyph ids of a run are indices into the face the shaper chose, so
    /// they may only be drawn from that face: the face CoreText hands back
    /// must carry exactly the PostScript name asked for. A substitute (a
    /// hidden, dot-prefixed face CoreText cannot resolve by name comes back as
    /// another font) is a counted refusal, never drawn. So is a face whose
    /// bytes disagree with the face CoreText opened by that name
    /// ([`FaceShape`]): the name matched, the glyph ids may not.
    fn open_face(&mut self, handle: &FontHandle) -> Option<Face> {
        let Some(name) = handle.postscript_name() else {
            self.nameless = self.nameless.saturating_add(1);
            log::warn!(
                "frust-engine: hvgl face {} carries no PostScript name; its glyphs are counted \
                 as skipped",
                handle.font().index,
            );
            return None;
        };
        let shown = truncated(name);
        match try_open_face(handle.font(), name, self.next_id) {
            Ok((_, resolved)) if resolved != name => {
                self.mismatches = self.mismatches.saturating_add(1);
                log::warn!(
                    "frust-engine: hvgl face {shown:?} (hidden: {}) resolved by CoreText to \
                     {:?}; its glyphs are counted as skipped",
                    handle.is_hidden(),
                    truncated(&resolved),
                );
                None
            }
            Ok((face, _)) => {
                let installed = FaceShape::of_installed(&face.ct);
                let disagreement = FaceShape::of_bytes(handle.font())
                    .disagreement(&installed)
                    .or_else(|| cmap_disagreement(handle.font(), &face.ct));
                match disagreement {
                    None => Some(face),
                    Some(disagreement) => {
                        self.bytes_mismatches = self.bytes_mismatches.saturating_add(1);
                        log::warn!(
                            "frust-engine: hvgl face {shown:?} does not match the face CoreText \
                             opens by that name ({disagreement}); its glyphs are counted as \
                             skipped",
                        );
                        None
                    }
                }
            }
            Err(reason) => {
                log::warn!(
                    "frust-engine: hvgl face {shown:?} refused ({reason}); its glyphs are \
                     counted as skipped",
                );
                None
            }
        }
    }
}

/// `name`'s face opened in CoreText over `font`'s metrics and axes, with the
/// PostScript name CoreText resolved it to. The error names no face: the caller
/// logs the name once, escaped and truncated.
fn try_open_face(font: &FontData, name: &str, id: u64) -> Result<(Face, String), &'static str> {
    let data = font.data.data();
    let directory = table_directory(data, font.index).ok_or("no table directory")?;
    let units_per_em = head_units_per_em(data, directory)
        .filter(|units| *units > 0)
        .map(f64::from)
        .ok_or("no usable unitsPerEm")?;
    let (ct, resolved) = platform::CtFace::open(name, units_per_em).ok_or(platform::UNAVAILABLE)?;
    Ok((
        Face {
            id,
            ct,
            units_per_em,
            axes: variation_axes(data, directory),
            instances: Vec::new(),
            refused: VecDeque::new(),
            coretext_defaults: None,
            uses: 0,
        },
        resolved,
    ))
}

/// What CoreText supplies for `glyph_id` of `face`.
fn fetch(face: &platform::CtFace, glyph_id: u32) -> Fetched {
    if !face.contains(glyph_id) {
        return Fetched::Missing;
    }
    match face.path(glyph_id) {
        Some(elements) => checked_path(elements),
        // CoreText answers no path both for a glyph with no outline (a space)
        // and for one it cannot draw; the glyph's bounding box tells them
        // apart.
        None if face.has_ink(glyph_id) => Fetched::Missing,
        None => Fetched::Empty,
    }
}

/// A CoreText path, from the path elements it applied: missing when it does
/// not begin with a move (which `kurbo` requires of a path), empty when it
/// applied none.
fn checked_path(mut elements: Vec<PathEl>) -> Fetched {
    match elements.first() {
        None => Fetched::Empty,
        Some(PathEl::MoveTo(_)) => {
            elements.shrink_to_fit();
            Fetched::Path(BezPath::from_vec(elements))
        }
        Some(_) => Fetched::Missing,
    }
}

#[cfg(target_vendor = "apple")]
mod platform {
    use core::ffi::c_void;
    use core::ptr::NonNull;

    use kurbo::{PathEl, Point};
    use objc2_core_foundation::{
        CFArray, CFDictionary, CFIndex, CFNumber, CFRetained, CFString, CFType,
    };
    use objc2_core_graphics::{CGPath, CGPathElement, CGPathElementType};
    use objc2_core_text::{
        CTFont, CTFontDescriptor, CTFontOrientation, CTFontTableOptions, kCTFontVariationAttribute,
        kCTFontVariationAxisDefaultValueKey, kCTFontVariationAxisIdentifierKey,
    };

    /// Why [`CtFace::open`] refuses a face here.
    pub(super) const UNAVAILABLE: &str = "CoreText cannot open that name";

    /// One face opened in CoreText, read-only.
    pub(super) struct CtFace {
        /// The face, at `unitsPerEm` points.
        font: CFRetained<CTFont>,
        /// How many glyphs the face holds.
        glyph_count: usize,
    }

    // SAFETY: Apple documents CoreText as thread-safe: "All individual
    // functions in Core Text are thread-safe. Font objects (CTFont,
    // CTFontDescriptor, and associated objects) can be used simultaneously by
    // multiple operations, work queues, or threads." (Core Text Programming
    // Guide, "Core Text Objects Are C-Language Opaque Types"). A `CtFace` is
    // never mutated after `open`: it only ever reads through the font
    // (`CTFontCreatePathForGlyph`, `CTFontGetBoundingRectsForGlyphs`), and the
    // `CFRetained` release on drop is `CFRelease`, which is thread-safe too.
    // `SceneCompiler`, which holds these, has to be `Send + Sync`.
    unsafe impl Send for CtFace {}
    // SAFETY: as above — shared access is only ever the immutable reads named
    // there.
    unsafe impl Sync for CtFace {}

    impl CtFace {
        /// The face CoreText resolves `name` to, at `units_per_em` points, with
        /// the PostScript name it actually carries (which differs from `name`
        /// when CoreText substituted a face for a name it does not know).
        pub(super) fn open(name: &str, units_per_em: f64) -> Option<(Self, String)> {
            let requested = CFString::from_str(name);
            // SAFETY: a null matrix is the documented "identity" argument.
            let font = unsafe { CTFont::with_name(&requested, units_per_em, core::ptr::null()) };
            // SAFETY: a getter on a live font.
            let resolved = unsafe { font.post_script_name() }.to_string();
            // SAFETY: a getter on a live font.
            let glyph_count = usize::try_from(unsafe { font.glyph_count() }).unwrap_or(0);
            Some((Self { font, glyph_count }, resolved))
        }

        /// A copy of this face at the variation instance `axes` names — each
        /// `(axis tag, user value)` — at `units_per_em` points.
        ///
        /// Built from a `kCTFontVariationAttribute` dictionary (axis
        /// identifier to value, both `CFNumber`s). The copy carries no failure
        /// signal of its own, so this is always `Some`: whether CoreText
        /// applied the instance is read back from the copy
        /// ([`Self::variation_values`]).
        pub(super) fn varied(&self, axes: &[(u32, f64)], units_per_em: f64) -> Option<Self> {
            let tags: Vec<CFRetained<CFNumber>> = axes
                .iter()
                .map(|(tag, _)| CFNumber::new_i64(i64::from(*tag)))
                .collect();
            let values: Vec<CFRetained<CFNumber>> = axes
                .iter()
                .map(|(_, value)| CFNumber::new_f64(*value))
                .collect();
            let tags: Vec<&CFNumber> = tags.iter().map(|tag| &**tag).collect();
            let values: Vec<&CFNumber> = values.iter().map(|value| &**value).collect();
            let variation = CFDictionary::<CFNumber, CFNumber>::from_slices(&tags, &values);
            // SAFETY: reading an immutable CoreText constant.
            let key: &CFString = unsafe { kCTFontVariationAttribute };
            let variation: &CFType = variation.as_ref();
            let attributes = CFDictionary::<CFString, CFType>::from_slices(&[key], &[variation]);
            // SAFETY: the attribute dictionary maps a CoreText attribute name
            // to the dictionary type that attribute documents.
            let descriptor = unsafe { CTFontDescriptor::with_attributes(attributes.as_opaque()) };
            // SAFETY: the font is live and a null matrix keeps the original's.
            let font = unsafe {
                self.font
                    .copy_with_attributes(units_per_em, core::ptr::null(), Some(&descriptor))
            };
            Some(Self {
                font,
                glyph_count: self.glyph_count,
            })
        }

        /// Whether `glyph_id` names a glyph of the face.
        pub(super) fn contains(&self, glyph_id: u32) -> bool {
            u16::try_from(glyph_id).is_ok_and(|glyph| usize::from(glyph) < self.glyph_count)
        }

        /// Every axis `CTFontCopyVariationAxes` lists, as `(axis identifier,
        /// default user value)`; empty for a face that does not vary. Costly
        /// (about 130 us on macOS 27: CoreText localizes every axis name), so
        /// read once per face.
        pub(super) fn axis_defaults(&self) -> Vec<(u32, f64)> {
            // SAFETY: a copy from a live font; `CFRetained` releases it.
            let Some(axes) = (unsafe { self.font.variation_axes() }) else {
                return Vec::new();
            };
            // SAFETY: every element is a CF object, which `CFType` stands for;
            // each is downcast checked before it is read as anything narrower.
            let axes: &CFArray<CFType> = unsafe { axes.cast_unchecked() };
            // SAFETY: reading immutable CoreText constants.
            let (identifier_key, default_key): (&CFType, &CFType) = unsafe {
                (
                    kCTFontVariationAxisIdentifierKey,
                    kCTFontVariationAxisDefaultValueKey,
                )
            };
            axes.iter()
                .filter_map(|axis| {
                    // SAFETY: as above, a dictionary of CF objects.
                    let axis: &CFDictionary<CFType, CFType> =
                        unsafe { axis.downcast_ref::<CFDictionary>()?.cast_unchecked() };
                    let tag = axis_tag(&*number(axis, identifier_key)?)?;
                    Some((tag, number(axis, default_key)?.as_f64()?))
                })
                .collect()
        }

        /// The values `CTFontCopyVariation` reports for the face, as `(axis
        /// identifier, user value)`: only those off the axis default.
        pub(super) fn variation_values(&self) -> Vec<(u32, f64)> {
            // SAFETY: a copy from a live font; `CFRetained` releases it.
            let Some(applied) = (unsafe { self.font.variation() }) else {
                return Vec::new();
            };
            // SAFETY: every key and value is a CF object, which `CFType`
            // stands for; each is downcast checked before it is read.
            let applied: &CFDictionary<CFType, CFType> = unsafe { applied.cast_unchecked() };
            let (keys, values) = applied.to_vecs();
            keys.iter()
                .zip(&values)
                .filter_map(|(key, value)| {
                    Some((
                        axis_tag(key.downcast_ref::<CFNumber>()?)?,
                        value.downcast_ref::<CFNumber>()?.as_f64()?,
                    ))
                })
                .collect()
        }

        /// CoreText's nominal glyph for each of `characters`
        /// (`CTFontGetGlyphsForCharacters`, the face's own character map), 0
        /// for one the face does not map.
        pub(super) fn glyphs_for(&self, characters: &[char]) -> Vec<u16> {
            let mut units: Vec<u16> = Vec::with_capacity(characters.len() * 2);
            let mut firsts = Vec::with_capacity(characters.len());
            for character in characters {
                firsts.push(units.len());
                units.extend_from_slice(character.encode_utf16(&mut [0; 2]));
            }
            let mut glyphs = vec![0_u16; units.len()];
            let (Some(units_at), Some(glyphs_at), Ok(count)) = (
                NonNull::new(units.as_mut_ptr()),
                NonNull::new(glyphs.as_mut_ptr()),
                CFIndex::try_from(units.len()),
            ) else {
                return Vec::new();
            };
            // SAFETY: both buffers hold `count` elements. A non-BMP character
            // takes two units, its glyph in the first slot.
            unsafe { self.font.glyphs_for_characters(units_at, glyphs_at, count) };
            firsts.into_iter().map(|first| glyphs[first]).collect()
        }

        /// How many glyphs the face holds (`CTFontGetGlyphCount`).
        pub(super) fn glyph_count(&self) -> usize {
            self.glyph_count
        }

        /// The face's design units per em (`CTFontGetUnitsPerEm`).
        pub(super) fn units_per_em(&self) -> u32 {
            // SAFETY: a getter on a live font.
            unsafe { self.font.units_per_em() }
        }

        /// A copy of the face's table `tag` (`CTFontCopyTable`); `None` when
        /// the face has no such table.
        pub(super) fn table(&self, tag: [u8; 4]) -> Option<Vec<u8>> {
            // SAFETY: a copy from a live font; the returned data follows the
            // Create rule and `CFRetained` releases it.
            let data = unsafe {
                self.font
                    .table(u32::from_be_bytes(tag), CTFontTableOptions::NoOptions)
            }?;
            Some(data.to_vec())
        }

        /// `glyph_id`'s outline in font units, y up, as path elements; `None`
        /// when CoreText returns no path for it.
        pub(super) fn path(&self, glyph_id: u32) -> Option<Vec<PathEl>> {
            let glyph = u16::try_from(glyph_id).ok()?;
            // SAFETY: the font is live and a null matrix is the documented
            // "identity" argument.
            let path = unsafe { self.font.path_for_glyph(glyph, core::ptr::null()) }?;
            let mut elements: Vec<PathEl> = Vec::new();
            // SAFETY: `push_element` writes only through `info`, which points
            // at `elements` for the duration of this call and nothing else
            // borrows it meanwhile.
            unsafe {
                CGPath::apply(
                    Some(&path),
                    (&raw mut elements).cast::<c_void>(),
                    Some(push_element),
                );
            }
            Some(elements)
        }

        /// Whether `glyph_id` has a non-empty bounding box.
        pub(super) fn has_ink(&self, glyph_id: u32) -> bool {
            let Ok(mut glyph) = u16::try_from(glyph_id) else {
                return false;
            };
            // SAFETY: `glyphs` points at one valid `CGGlyph` and `count` is 1;
            // a null `bounding_rects` asks for the overall box alone.
            let bounds = unsafe {
                self.font.bounding_rects_for_glyphs(
                    CTFontOrientation::Default,
                    NonNull::from(&mut glyph),
                    core::ptr::null_mut(),
                    1,
                )
            };
            bounds.size.width > 0.0 && bounds.size.height > 0.0
        }
    }

    /// The axis tag a `CFNumber` axis identifier carries.
    fn axis_tag(identifier: &CFNumber) -> Option<u32> {
        u32::try_from(identifier.as_i64()?).ok()
    }

    /// `dictionary`'s value for `key`, when it is a `CFNumber`.
    fn number(
        dictionary: &CFDictionary<CFType, CFType>,
        key: &CFType,
    ) -> Option<CFRetained<CFNumber>> {
        dictionary.get(key)?.downcast::<CFNumber>().ok()
    }

    /// `CGPathApply`'s callback: append one element to the `Vec<PathEl>`
    /// `info` points at.
    ///
    /// # Safety
    ///
    /// `info` must point at a live, otherwise unborrowed `Vec<PathEl>`, and
    /// `element` must be a valid element whose `points` hold as many points as
    /// its type carries — which `CGPathApply` guarantees for the call.
    unsafe extern "C-unwind" fn push_element(info: *mut c_void, element: NonNull<CGPathElement>) {
        // SAFETY: `info` is the `&mut Vec<PathEl>` `outline` passed in.
        let out = unsafe { &mut *info.cast::<Vec<PathEl>>() };
        // SAFETY: CoreGraphics hands a valid element for the call's duration.
        let element = unsafe { element.as_ref() };
        let points = element.points.as_ptr();
        let point = |index: usize| {
            // SAFETY: every element type read below carries at least
            // `index + 1` points.
            let p = unsafe { *points.add(index) };
            Point::new(p.x, p.y)
        };
        let el = match element.r#type {
            CGPathElementType::MoveToPoint => PathEl::MoveTo(point(0)),
            CGPathElementType::AddLineToPoint => PathEl::LineTo(point(0)),
            CGPathElementType::AddQuadCurveToPoint => PathEl::QuadTo(point(0), point(1)),
            CGPathElementType::AddCurveToPoint => PathEl::CurveTo(point(0), point(1), point(2)),
            CGPathElementType::CloseSubpath => PathEl::ClosePath,
            _ => return,
        };
        out.push(el);
    }
}

#[cfg(not(target_vendor = "apple"))]
mod platform {
    use kurbo::PathEl;

    /// Why [`CtFace::open`] refuses a face here.
    pub(super) const UNAVAILABLE: &str = "this target has no CoreText to read hvgl outlines with";

    /// No face can be opened without CoreText.
    pub(super) enum CtFace {}

    impl CtFace {
        /// Always `None`: there is no CoreText here.
        pub(super) fn open(_name: &str, _units_per_em: f64) -> Option<(Self, String)> {
            None
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(super) fn varied(&self, _axes: &[(u32, f64)], _units_per_em: f64) -> Option<Self> {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(super) fn contains(&self, _glyph_id: u32) -> bool {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(super) fn axis_defaults(&self) -> Vec<(u32, f64)> {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(super) fn variation_values(&self) -> Vec<(u32, f64)> {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(super) fn glyphs_for(&self, _characters: &[char]) -> Vec<u16> {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(super) fn glyph_count(&self) -> usize {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(super) fn units_per_em(&self) -> u32 {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(super) fn table(&self, _tag: [u8; 4]) -> Option<Vec<u8>> {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(super) fn path(&self, _glyph_id: u32) -> Option<Vec<PathEl>> {
            match *self {}
        }

        /// Unreachable — no `CtFace` exists on this target.
        pub(super) fn has_ink(&self, _glyph_id: u32) -> bool {
            match *self {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use peniko::Blob;

    use super::*;

    /// A single-face sfnt holding `tables` (tag, bytes), table data 4-aligned.
    fn sfnt(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let count = u16::try_from(tables.len()).unwrap();
        let mut out = vec![0x00, 0x01, 0x00, 0x00];
        out.extend(count.to_be_bytes());
        out.extend([0; 6]);
        let mut offset = 12 + 16 * tables.len();
        let mut data = Vec::new();
        for (tag, bytes) in tables {
            out.extend(tag);
            out.extend([0; 4]);
            out.extend(u32::try_from(offset).unwrap().to_be_bytes());
            out.extend(u32::try_from(bytes.len()).unwrap().to_be_bytes());
            let padded = bytes.len().div_ceil(4) * 4;
            data.extend(bytes);
            data.resize(data.len() + padded - bytes.len(), 0);
            offset += padded;
        }
        out.extend(data);
        out
    }

    fn head(units_per_em: u16) -> Vec<u8> {
        let mut head = vec![0; 54];
        head[18..20].copy_from_slice(&units_per_em.to_be_bytes());
        head
    }

    fn font(bytes: Vec<u8>) -> FontData {
        FontData::new(Blob::new(Arc::new(bytes)), 0)
    }

    thread_local! {
        /// Added to every user value CoreText reads back on this thread, so a
        /// case can force a read-back mismatch.
        static READ_BACK_SKEW: core::cell::Cell<f64> = const { core::cell::Cell::new(0.0) };
    }

    /// The read-back hook: `applied` with this thread's skew added.
    pub(super) fn skewed(applied: Vec<(u32, f64)>) -> Vec<(u32, f64)> {
        let skew = READ_BACK_SKEW.get();
        applied
            .into_iter()
            .map(|(tag, value)| (tag, value + skew))
            .collect()
    }

    fn key(glyph_id: u32) -> OutlineKey {
        OutlineKey {
            face: 1,
            glyph_id,
            variation: VariationKey::DEFAULT,
        }
    }

    #[test]
    fn only_an_hvgl_face_without_another_outline_table_needs_the_provider() {
        let hvgl_only = font(sfnt(&[(*b"head", head(1000)), (HVGL_TAG, vec![0; 8])]));
        assert!(font_needs_hvgl_provider(&hvgl_only));
        for tag in OUTLINE_TAGS {
            let both = font(sfnt(&[
                (*b"head", head(1000)),
                (HVGL_TAG, vec![0; 8]),
                (tag, vec![0; 8]),
            ]));
            assert!(!font_needs_hvgl_provider(&both), "{tag:?}");
        }
        let plain = font(sfnt(&[(*b"head", head(1000)), (*b"glyf", vec![0; 8])]));
        assert!(!font_needs_hvgl_provider(&plain));
        assert!(!font_needs_hvgl_provider(&font(b"not a font".to_vec())));
    }

    fn handle(bytes: Vec<u8>, name: Option<&str>) -> FontHandle {
        FontHandle::new(font(bytes)).with_identity(name.map(Arc::from), false)
    }

    #[test]
    fn a_handle_with_no_name_is_not_opened_and_a_name_is_not_derived_from_the_blob() {
        // The blob carries no `name` table, and none is looked for.
        let nameless = handle(
            sfnt(&[(*b"head", head(1000)), (HVGL_TAG, vec![0; 8])]),
            None,
        );
        let mut outlines = HvglOutlines::default();
        assert!(matches!(
            outlines.outline(&nameless, 1),
            GlyphOutline::Unavailable
        ));
        assert_eq!(outlines.cached_outlines(), 0);
        assert_eq!(outlines.identity_mismatches(), 0);
    }

    /// A nameless handle is one cached refusal however often it is asked.
    #[test]
    fn a_nameless_handle_is_one_cached_refusal_with_stable_counters() {
        let nameless = handle(
            sfnt(&[(*b"head", head(1000)), (HVGL_TAG, vec![0; 8])]),
            None,
        );
        let mut outlines = HvglOutlines::default();
        for _ in 0..3 {
            assert!(matches!(
                outlines.outline(&nameless, 1),
                GlyphOutline::Unavailable
            ));
        }
        assert!(outlines.faces.open.is_empty());
        assert_eq!(outlines.faces.refused.len(), 1);
        assert_eq!(outlines.nameless_faces(), 1);
        assert_eq!(outlines.identity_mismatches(), 0);
    }

    /// Two blobs under one name are two faces.
    #[test]
    fn two_blobs_under_one_name_are_two_faces() {
        let one = handle(sfnt(&[(HVGL_TAG, vec![0; 8])]), Some("SameName-Regular"));
        let two = handle(sfnt(&[(HVGL_TAG, vec![1; 8])]), Some("SameName-Regular"));
        let mut outlines = HvglOutlines::default();
        outlines.outline(&one, 1);
        outlines.outline(&two, 1);
        outlines.outline(&one, 1);
        assert_eq!(outlines.faces.refused.len(), 2);
    }

    #[test]
    fn a_logged_name_is_cut_on_a_character_boundary() {
        let long = "é".repeat(100);
        let cut = truncated(&long);
        assert!(cut.len() <= LOGGED_NAME_BYTES);
        assert!(long.starts_with(cut));
        assert_eq!(truncated("short"), "short");
    }

    #[test]
    fn a_blob_without_a_head_table_is_refused_once_per_name() {
        let broken = handle(sfnt(&[(HVGL_TAG, vec![0; 8])]), Some("PingFangSC-Medium"));
        let mut outlines = HvglOutlines::default();
        for _ in 0..2 {
            assert!(matches!(
                outlines.outline(&broken, 1),
                GlyphOutline::Unavailable
            ));
        }
        assert_eq!(outlines.faces.refused.len(), 1);
    }

    /// Refusals sit outside the open-face LRU and are bounded: each refused
    /// face is counted once while the set remembers it, and the set never
    /// grows past its cap.
    #[test]
    fn refusals_are_remembered_outside_the_open_faces_and_bounded() {
        let refused_face = || {
            handle(
                sfnt(&[(*b"head", head(1000)), (HVGL_TAG, vec![0; 8])]),
                None,
            )
        };
        let faces: Vec<FontHandle> = (0..MAX_REFUSED_FACES + 8).map(|_| refused_face()).collect();
        let mut outlines = HvglOutlines::default();
        for face in &faces[..MAX_REFUSED_FACES] {
            outlines.outline(face, 1);
        }
        let remembered = u32::try_from(MAX_REFUSED_FACES).expect("small");
        assert_eq!(outlines.nameless_faces(), remembered);
        for face in &faces[..MAX_REFUSED_FACES] {
            outlines.outline(face, 1);
        }
        assert_eq!(
            outlines.nameless_faces(),
            remembered,
            "asked again, counted again"
        );
        assert_eq!(outlines.faces.refused.len(), MAX_REFUSED_FACES);
        assert!(outlines.faces.open.is_empty());
        for face in &faces[MAX_REFUSED_FACES..] {
            outlines.outline(face, 1);
        }
        assert_eq!(outlines.faces.refused.len(), MAX_REFUSED_FACES);
        assert_eq!(
            outlines.nameless_faces(),
            u32::try_from(faces.len()).expect("small"),
            "each face past the cap is one new refusal"
        );
    }

    /// An open face survives a churn of refusals: the refusals never evict it,
    /// so it is not opened again and the counters do not move for it.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_churn_of_refusals_never_evicts_an_open_face() {
        let Some(font) = system_pingfang_sc() else {
            println!("skipped: PingFangUI.ttc is not installed");
            return;
        };
        let live = FontHandle::new(font).with_identity(Some(Arc::from("PingFangSC-Medium")), false);
        let mut outlines = HvglOutlines::default();
        assert!(matches!(
            outlines.outline(&live, 289),
            GlyphOutline::Path { .. }
        ));
        let live_id = outlines.faces.open[0].1.id;
        for _ in 0..MAX_REFUSED_FACES + MAX_OPEN_FACES {
            let refused = handle(
                sfnt(&[(*b"head", head(1000)), (HVGL_TAG, vec![0; 8])]),
                None,
            );
            outlines.outline(&refused, 1);
        }
        assert!(matches!(
            outlines.outline(&live, 289),
            GlyphOutline::Path { .. }
        ));
        assert_eq!(outlines.faces.open.len(), 1, "the live face is still open");
        assert_eq!(
            outlines.faces.open[0].1.id, live_id,
            "and was not opened again"
        );
        assert_eq!(outlines.identity_mismatches(), 0);
    }

    #[test]
    fn the_outline_cache_evicts_the_least_recently_used_entry_at_its_cap() {
        let mut cache = OutlineCache::new(3);
        for glyph in 0..3 {
            cache.get_or_fetch(key(glyph), || Some(Fetched::Empty));
        }
        assert_eq!((cache.hits, cache.misses), (0, 3));
        // Touch glyph 0, so glyph 1 is now the oldest.
        let mut fetched = false;
        cache.get_or_fetch(key(0), || {
            fetched = true;
            Some(Fetched::Missing)
        });
        assert!(!fetched, "a cached glyph is not fetched again");
        assert_eq!((cache.hits, cache.misses), (1, 3));
        cache.get_or_fetch(key(3), || Some(Fetched::Empty));
        assert_eq!((cache.hits, cache.misses), (1, 4));
        assert_eq!(cache.entries.len(), 3);
        assert_eq!(cache.order.len(), 3);
        assert!(cache.entries.contains_key(&key(0)));
        assert!(
            !cache.entries.contains_key(&key(1)),
            "the oldest entry is evicted"
        );
        assert!(cache.entries.contains_key(&key(2)));
        assert!(cache.entries.contains_key(&key(3)));
        assert!(matches!(OutlineCache::new(0).capacity, 1));
    }

    #[test]
    fn cycling_one_more_glyph_than_the_capacity_never_hits() {
        let mut cache = OutlineCache::new(3);
        for _ in 0..5 {
            for glyph in 0..4 {
                cache.get_or_fetch(key(glyph), || Some(Fetched::Empty));
            }
        }
        assert_eq!((cache.hits, cache.misses), (0, 20), "every fetch thrashes");
        // A working set within the capacity settles to hits.
        let mut cache = OutlineCache::new(3);
        for _ in 0..5 {
            for glyph in 0..3 {
                cache.get_or_fetch(key(glyph), || Some(Fetched::Empty));
            }
        }
        assert_eq!((cache.hits, cache.misses), (12, 3));
    }

    #[test]
    fn the_outline_counters_saturate() {
        let mut cache = OutlineCache::new(2);
        cache.hits = u64::MAX;
        cache.misses = u64::MAX;
        cache.get_or_fetch(key(0), || Some(Fetched::Empty));
        cache.get_or_fetch(key(0), || Some(Fetched::Empty));
        assert_eq!((cache.hits, cache.misses), (u64::MAX, u64::MAX));
    }

    #[test]
    fn unavailable_glyphs_accumulate_and_saturate() {
        let mut outlines = HvglOutlines::with_capacity(2);
        outlines.note_unavailable(3);
        outlines.note_unavailable(4);
        assert_eq!(outlines.unavailable_glyphs(), 7);
        outlines.unavailable = u64::MAX;
        outlines.note_unavailable(1);
        assert_eq!(outlines.unavailable_glyphs(), u64::MAX);
    }

    #[test]
    fn a_coretext_path_must_begin_with_a_move() {
        assert!(matches!(checked_path(Vec::new()), Fetched::Empty));
        assert!(matches!(
            checked_path(vec![PathEl::LineTo((1.0, 1.0).into())]),
            Fetched::Missing
        ));
        assert!(matches!(
            checked_path(vec![
                PathEl::MoveTo((0.0, 0.0).into()),
                PathEl::LineTo((1.0, 1.0).into()),
                PathEl::ClosePath,
            ]),
            Fetched::Path(_)
        ));
    }

    /// An `fvar` table of `(tag, min, default, max)` axes.
    fn fvar(axes: &[(&[u8; 4], f64, f64, f64)]) -> Vec<u8> {
        let mut out = Vec::new();
        let count = u16::try_from(axes.len()).unwrap();
        for field in [1_u16, 0, 16, 2, count, 20, 0, 0] {
            out.extend(field.to_be_bytes());
        }
        for (tag, min, default, max) in axes {
            out.extend(*tag);
            for value in [min, default, max] {
                out.extend(((value * FIXED_ONE) as i32).to_be_bytes());
            }
            out.extend([0; 4]);
        }
        out
    }

    /// An `avar` table of one segment map of `(from, to)` pairs per axis.
    fn avar(maps: &[&[(f64, f64)]]) -> Vec<u8> {
        let mut out = Vec::new();
        let count = u16::try_from(maps.len()).unwrap();
        for field in [1_u16, 0, 0, count] {
            out.extend(field.to_be_bytes());
        }
        for map in maps {
            out.extend(u16::try_from(map.len()).unwrap().to_be_bytes());
            for (from, to) in *map {
                out.extend(((from * F2DOT14_ONE) as i16).to_be_bytes());
                out.extend(((to * F2DOT14_ONE) as i16).to_be_bytes());
            }
        }
        out
    }

    fn axis(tag: &[u8; 4], min: f64, default: f64, max: f64) -> Axis {
        Axis {
            tag: u32::from_be_bytes(*tag),
            min,
            default,
            max,
            avar: Vec::new(),
        }
    }

    #[test]
    fn variation_keys_separate_every_instance_and_fold_the_default() {
        assert_eq!(VariationKey::of(&[]), VariationKey::DEFAULT);
        assert_eq!(VariationKey::of(&[0, 0, 0]), VariationKey::DEFAULT);
        // Trailing zeros are default axes: the same instance.
        assert_eq!(VariationKey::of(&[0, 100]), VariationKey::of(&[0, 100, 0]));
        let keys = [
            VariationKey::of(&[100]),
            VariationKey::of(&[0, 100]),
            VariationKey::of(&[0, 0, 100]),
            VariationKey::of(&[-100]),
            VariationKey::of(&[0, -9830, 0]),
            VariationKey::of(&[0, 16384, 0]),
            VariationKey::of(&[1, 2, 3, 4]),
            VariationKey::of(&[1, 2, 3, 5]),
        ];
        for (index, key) in keys.iter().enumerate() {
            assert_ne!(*key, VariationKey::DEFAULT, "{index}");
            for other in &keys[index + 1..] {
                assert_ne!(key, other);
            }
        }
        // Packed keys never carry the hashed keys' tag bit.
        assert_eq!(VariationKey::of(&[-1, -1, -1]).0 >> 63, 0);
        assert_eq!(VariationKey::of(&[1, 2, 3, 4]).0 >> 63, 1);
    }

    #[test]
    fn a_normalized_coordinate_denormalizes_against_its_axis_range() {
        let wght = axis(b"wght", 100.0, 500.0, 600.0);
        assert_eq!(user_value(&wght, 0.0), 500.0);
        assert_eq!(user_value(&wght, -1.0), 100.0);
        assert_eq!(user_value(&wght, -0.5), 300.0);
        assert_eq!(user_value(&wght, 1.0), 600.0);
        assert_eq!(user_value(&wght, 2.0), 600.0, "clamped to the axis");
        let values = user_values(&[axis(b"WDTH", 1.0, 500.0, 1000.0), wght], &[0]);
        assert_eq!(
            values,
            vec![
                (u32::from_be_bytes(*b"WDTH"), 500.0),
                (u32::from_be_bytes(*b"wght"), 500.0)
            ],
            "an axis past the coordinates sits at its default"
        );
    }

    #[test]
    fn an_avar_mapped_coordinate_is_mapped_back_before_denormalizing() {
        // PingFang SC's `wght` map (macOS 27): the light half is bent, and
        // the heavy half maps 0..1 onto 0..0.2 with a jump to 1.0 at the end.
        let map = [
            (-1.0, -1.0),
            (-0.5, -0.6),
            (0.0, 0.0),
            (1.0, 0.2),
            (1.0, 1.0),
        ];
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(unmap_avar(&map, -0.6), -0.5));
        assert!(close(unmap_avar(&map, -0.8), -0.75));
        assert!(close(unmap_avar(&map, 0.1), 0.5));
        assert!(close(unmap_avar(&map, 0.2), 1.0));
        assert!(close(unmap_avar(&map, 0.6), 1.0), "the flat top is the max");
        assert!(close(unmap_avar(&map, 1.0), 1.0));
        assert!(close(unmap_avar(&[], 0.3), 0.3), "no map is the identity");
        let mut wght = axis(b"wght", 100.0, 500.0, 600.0);
        wght.avar = map.to_vec();
        assert!(close(user_value(&wght, -0.6), 300.0));
        assert!(close(user_value(&wght, 0.2), 600.0));
    }

    #[test]
    fn fvar_axes_and_their_avar_maps_are_read_from_the_face() {
        let tables = [
            (*b"head", head(1000)),
            (
                FVAR_TAG,
                fvar(&[
                    (b"WDTH", 1.0, 500.0, 1000.0),
                    (b"wght", 100.0, 500.0, 600.0),
                ]),
            ),
            (
                AVAR_TAG,
                avar(&[
                    &[(-1.0, -1.0), (0.0, 0.0), (1.0, 1.0)],
                    &[(-1.0, -1.0), (-0.5, -0.625), (0.0, 0.0), (1.0, 1.0)],
                ]),
            ),
        ];
        let data = sfnt(&tables);
        let axes = variation_axes(&data, table_directory(&data, 0).unwrap());
        assert_eq!(axes.len(), 2);
        assert_eq!(axes[1].tag, u32::from_be_bytes(*b"wght"));
        assert_eq!(
            (axes[1].min, axes[1].default, axes[1].max),
            (100.0, 500.0, 600.0)
        );
        assert_eq!(axes[1].avar[1], (-0.5, -0.625));
        assert_eq!(user_value(&axes[1], -0.625), 300.0);

        // An `avar` for the wrong number of axes is ignored, not misapplied.
        let short = sfnt(&[
            tables[0].clone(),
            tables[1].clone(),
            (AVAR_TAG, avar(&[&[(-1.0, -1.0), (0.0, 0.0), (1.0, 1.0)]])),
        ]);
        let axes = variation_axes(&short, table_directory(&short, 0).unwrap());
        assert!(axes.iter().all(|axis| axis.avar.is_empty()));

        // No `fvar`, or one that does not fit, reads as a face that does not
        // vary.
        let plain = sfnt(&[(*b"head", head(1000))]);
        assert!(variation_axes(&plain, table_directory(&plain, 0).unwrap()).is_empty());
        let mut truncated = fvar(&[(b"wght", 100.0, 500.0, 600.0)]);
        truncated.truncate(24);
        let broken = sfnt(&[(*b"head", head(1000)), (FVAR_TAG, truncated)]);
        assert!(variation_axes(&broken, table_directory(&broken, 0).unwrap()).is_empty());
    }

    /// The blob the macOS cases read: the variable `hvgl` face of the system
    /// PingFang collection that `PingFangSC-Medium` is (its bytes agree with
    /// the face CoreText opens by that name; else the first variable `hvgl`
    /// face), or `None` when the collection is not installed.
    #[cfg(target_os = "macos")]
    fn system_pingfang_sc() -> Option<FontData> {
        let data = [
            "/System/Library/PrivateFrameworks/FontServices.framework/Versions/A/Resources/Reserved/PingFangUI.ttc",
            "/System/Library/Fonts/PingFangUI.ttc",
        ]
        .iter()
        .find_map(|path| std::fs::read(path).ok())?;
        let count = read_u32(&data, 8)?;
        let blob = Blob::new(Arc::new(data));
        let variable: Vec<FontData> = (0..count)
            .filter_map(|index| {
                let font = FontData::new(blob.clone(), index);
                let data = font.data.data();
                let directory = table_directory(data, index)?;
                (font_needs_hvgl_provider(&font) && !variation_axes(data, directory).is_empty())
                    .then_some(font)
            })
            .collect();
        let installed = platform::CtFace::open("PingFangSC-Medium", 1000.0)
            .map(|(face, _)| FaceShape::of_installed(&face));
        variable
            .iter()
            .find(|font| {
                installed
                    .as_ref()
                    .is_some_and(|shape| FaceShape::of_bytes(font).disagreement(shape).is_none())
            })
            .or(variable.first())
            .cloned()
    }

    /// A copy of `font`'s blob with its `maxp.numGlyphs` lowered by one.
    #[cfg(target_os = "macos")]
    fn with_one_glyph_fewer(font: &FontData) -> FontData {
        let mut data = font.data.data().to_vec();
        let directory = table_directory(&data, font.index).expect("a directory");
        let at = table_start(&data, directory, MAXP_TAG).expect("a maxp") + MAXP_NUM_GLYPHS;
        let count = read_u16(&data, at).expect("numGlyphs");
        data[at..at + 2].copy_from_slice(&(count - 1).to_be_bytes());
        FontData::new(Blob::new(Arc::new(data)), font.index)
    }

    /// A face whose bytes disagree with the CoreText face its name opens is
    /// refused: counted once as a bytes mismatch, not as an identity one,
    /// and never drawn. The unpatched bytes draw.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_face_whose_bytes_disagree_with_coretext_is_counted_once_and_refused() {
        let Some(font) = system_pingfang_sc() else {
            println!("skipped: PingFangUI.ttc is not installed");
            return;
        };
        let name = Some(Arc::from("PingFangSC-Medium"));
        let mut outlines = HvglOutlines::default();
        let genuine = FontHandle::new(font.clone()).with_identity(name.clone(), false);
        assert!(matches!(
            outlines.outline(&genuine, 289),
            GlyphOutline::Path { .. }
        ));
        assert_eq!(outlines.bytes_mismatches(), 0, "the installed bytes agree");
        let patched = FontHandle::new(with_one_glyph_fewer(&font)).with_identity(name, false);
        for _ in 0..3 {
            assert!(matches!(
                outlines.outline(&patched, 289),
                GlyphOutline::Unavailable
            ));
        }
        assert_eq!(outlines.bytes_mismatches(), 1);
        assert_eq!(outlines.identity_mismatches(), 0);
    }

    /// The light and semibold instances of PingFang SC's `中` have different
    /// outline bounds: the coordinates reach CoreText.
    #[cfg(target_os = "macos")]
    #[test]
    fn pingfang_outlines_follow_the_requested_weight() {
        let Some(font) = system_pingfang_sc() else {
            println!("skipped: PingFangUI.ttc is not installed");
            return;
        };
        let data = font.data.data();
        let axes = variation_axes(data, table_directory(data, font.index).unwrap());
        let wght = axes
            .iter()
            .position(|axis| axis.tag == u32::from_be_bytes(*b"wght"))
            .expect("PingFang SC varies its weight");
        // Glyph 289 is `中` in this face (CoreText's own character map, as in
        // the integration tests); any inked glyph shows the same.
        let glyph = 289;
        let mut outlines = HvglOutlines::default();
        let mut bounds_at = |normalized: i16| {
            let mut coords = vec![0_i16; axes.len()];
            coords[wght] = normalized;
            let handle = FontHandle::new(font.clone())
                .with_normalized_coords(&coords)
                .with_identity(Some(Arc::from("PingFangSC-Medium")), false);
            match outlines.outline(&handle, glyph) {
                GlyphOutline::Path { path, .. } => kurbo::Shape::bounding_box(path),
                other => panic!("no outline at {normalized}: {other:?}"),
            }
        };
        let light = bounds_at(-16384);
        let heavy = bounds_at(16384);
        let default = bounds_at(0);
        assert_ne!(light, heavy);
        assert_ne!(light, default);
        assert!(heavy.width() > light.width(), "{light:?} vs {heavy:?}");
        assert_eq!(outlines.cached_outlines(), 3, "one entry per instance");
        assert_eq!(
            outlines.identity_mismatches(),
            0,
            "the name resolved as asked"
        );
    }

    /// A name CoreText substitutes another face for is refused: counted once
    /// per face, never drawn from the substitute.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_face_coretext_resolves_differently_is_counted_once_and_refused() {
        let Some(font) = system_pingfang_sc() else {
            println!("skipped: PingFangUI.ttc is not installed");
            return;
        };
        let handle = FontHandle::new(font)
            .with_identity(Some(Arc::from(".FrustNoSuchHiddenFace-Regular")), true);
        let mut outlines = HvglOutlines::default();
        for _ in 0..3 {
            assert!(
                matches!(outlines.outline(&handle, 1), GlyphOutline::Unavailable),
                "a substituted face is refused, not drawn"
            );
        }
        assert_eq!(outlines.identity_mismatches(), 1);
    }

    /// A `maxp` table (version 0.5) of `num_glyphs`.
    fn maxp(num_glyphs: u16) -> Vec<u8> {
        let mut maxp = vec![0x00, 0x00, 0x50, 0x00];
        maxp.extend(num_glyphs.to_be_bytes());
        maxp
    }

    #[test]
    fn a_face_shape_is_read_from_maxp_head_and_fvar() {
        let tables = [
            (*b"head", head(2048)),
            (
                FVAR_TAG,
                fvar(&[(b"wght", 100.0, 400.0, 900.0), (b"HGHT", 0.0, 0.0, 1.0)]),
            ),
            (HVGL_TAG, vec![0; 8]),
            (MAXP_TAG, maxp(300)),
        ];
        let shape = FaceShape::of_bytes(&font(sfnt(&tables)));
        assert_eq!(
            shape,
            FaceShape {
                glyph_count: Some(300),
                units_per_em: Some(2048),
                axis_tags: vec![u32::from_be_bytes(*b"wght"), u32::from_be_bytes(*b"HGHT")],
            }
        );
        // The `fvar` CoreText copies out on its own reads the same tags.
        let fvar_table = &tables[1].1;
        assert_eq!(
            axis_tags(fvar_axes_at(fvar_table, 0)),
            shape.axis_tags,
            "a standalone fvar parses like the face's"
        );
        let bare = FaceShape::of_bytes(&font(sfnt(&[
            (*b"head", head(1000)),
            (HVGL_TAG, vec![0; 8]),
        ])));
        assert_eq!(bare.glyph_count, None, "no maxp, no glyph count");
        assert!(bare.axis_tags.is_empty());
        assert_eq!(
            FaceShape::of_bytes(&font(b"not a font".to_vec())).units_per_em,
            None
        );
    }

    #[test]
    fn faces_agree_only_on_every_field() {
        let bytes = FaceShape {
            glyph_count: Some(300),
            units_per_em: Some(1000),
            axis_tags: vec![u32::from_be_bytes(*b"wght")],
        };
        assert_eq!(bytes.disagreement(&bytes.clone()), None);
        let fewer = FaceShape {
            glyph_count: Some(299),
            ..bytes.clone()
        };
        assert!(
            bytes
                .disagreement(&fewer)
                .is_some_and(|why| why.contains("glyph count 300") && why.contains("299")),
            "{:?}",
            bytes.disagreement(&fewer)
        );
        let unknown = FaceShape {
            glyph_count: None,
            ..bytes.clone()
        };
        assert!(
            unknown.disagreement(&bytes).is_some(),
            "a missing maxp never agrees"
        );
        let units = FaceShape {
            units_per_em: Some(2048),
            ..bytes.clone()
        };
        assert!(
            bytes
                .disagreement(&units)
                .is_some_and(|why| why.contains("unitsPerEm"))
        );
        let reordered = FaceShape {
            axis_tags: vec![u32::from_be_bytes(*b"wdth"), u32::from_be_bytes(*b"wght")],
            ..bytes.clone()
        };
        assert!(
            bytes
                .disagreement(&reordered)
                .is_some_and(|why| why.contains("[wght]") && why.contains("[wdth wght]"))
        );
        let flat = FaceShape {
            axis_tags: Vec::new(),
            ..bytes.clone()
        };
        assert!(bytes.disagreement(&flat).is_some(), "axis counts differ");
        // An unprintable tag is escaped, not written raw.
        assert_eq!(tag_list(&[0x0a00_6869]), "\\n\\x00hi");
    }

    /// A `cmap` table of `(platform, encoding, subtable)` records.
    fn cmap(subtables: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
        let count = u16::try_from(subtables.len()).unwrap();
        let mut out = vec![0, 0];
        out.extend(count.to_be_bytes());
        let mut offset = 4 + 8 * subtables.len();
        for (platform, encoding, subtable) in subtables {
            out.extend(platform.to_be_bytes());
            out.extend(encoding.to_be_bytes());
            out.extend(u32::try_from(offset).unwrap().to_be_bytes());
            offset += subtable.len();
        }
        for (.., subtable) in subtables {
            out.extend(subtable);
        }
        out
    }

    /// A format 12 subtable of `(first, last, first glyph)` groups.
    fn cmap12(groups: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut out = vec![0, 12, 0, 0];
        out.extend(u32::try_from(16 + 12 * groups.len()).unwrap().to_be_bytes());
        out.extend([0; 4]);
        out.extend(u32::try_from(groups.len()).unwrap().to_be_bytes());
        for (first, last, glyph) in groups {
            for field in [first, last, glyph] {
                out.extend(field.to_be_bytes());
            }
        }
        out
    }

    /// One format 4 segment: first and last codepoint, delta, and the
    /// `glyphIdArray` entries (none for a delta-only segment).
    type Segment = (u16, u16, u16, Vec<u16>);

    /// A format 4 subtable of `(first, last, delta, glyphs)` segments, the
    /// terminating `0xFFFF` segment appended; a segment with `glyphs` maps
    /// through the `glyphIdArray`.
    fn cmap4(segments: &[Segment]) -> Vec<u8> {
        let count = segments.len() + 1;
        let x2 = u16::try_from(count * 2).unwrap();
        let mut out = vec![0, 4, 0, 0, 0, 0];
        out.extend(x2.to_be_bytes());
        out.extend([0; 6]);
        let field = |pick: &dyn Fn(&Segment) -> u16, last: u16| {
            segments
                .iter()
                .map(pick)
                .chain([last])
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<u8>>()
        };
        out.extend(field(&|segment| segment.1, 0xFFFF));
        out.extend([0; 2]);
        out.extend(field(&|segment| segment.0, 0xFFFF));
        out.extend(field(&|segment| segment.2, 1));
        // Each indexed segment's entries follow every earlier segment's.
        let mut array: Vec<u16> = Vec::new();
        let mut offsets: Vec<u16> = Vec::new();
        for (index, (.., glyphs)) in segments.iter().enumerate() {
            if glyphs.is_empty() {
                offsets.push(0);
            } else {
                let to_array = (count - index) * 2 + array.len() * 2;
                offsets.push(u16::try_from(to_array).unwrap());
                array.extend(glyphs);
            }
        }
        offsets.push(0);
        out.extend(offsets.iter().flat_map(|offset| offset.to_be_bytes()));
        out.extend(array.iter().flat_map(|glyph| glyph.to_be_bytes()));
        out
    }

    fn sample_of(tables: &[([u8; 4], Vec<u8>)], count: usize) -> Vec<(char, u16)> {
        let data = sfnt(tables);
        cmap_sample(&data, table_directory(&data, 0).unwrap(), count)
    }

    #[test]
    fn a_cmap_sample_reads_format_12_ahead_of_format_4() {
        let groups = cmap12(&[
            (0x41, 0x43, 10),
            (0x4E2D, 0x4E2D, 289),
            (0x20000, 0x20002, 500),
        ]);
        let bmp = cmap4(&[(0x41, 0x43, 100, Vec::new())]);
        let both = [(CMAP_TAG, cmap(&[(3, 1, bmp.clone()), (3, 10, groups)]))];
        assert_eq!(
            sample_of(&both, 16),
            vec![('B', 11), ('中', 289), ('\u{20001}', 501)],
            "one pick per range at most, each its middle codepoint"
        );
        // Format 4 alone: a delta segment, an indexed one (whose zero entry
        // is no glyph), and the terminating segment never picked.
        let indexed = cmap4(&[
            (0x41, 0x43, 100, Vec::new()),
            (0x61, 0x63, 5, vec![7, 0, 9]),
            (0x30, 0x30, 0xFFF0, Vec::new()),
        ]);
        let data = sfnt(&[(CMAP_TAG, cmap(&[(3, 1, indexed.clone())]))]);
        let table = table_start(&data, table_directory(&data, 0).unwrap(), CMAP_TAG).unwrap();
        let subtable = UnicodeCmap::find(&data, table).unwrap();
        assert_eq!((subtable.format, subtable.ranges), (4, 4));
        let glyphs: Vec<Option<u16>> = (0x61..=0x63)
            .map(|codepoint| subtable.range(&data, 1)?.glyph(&data, codepoint))
            .collect();
        assert_eq!(glyphs, vec![Some(12), Some(0), Some(14)]);
        assert_eq!(
            subtable.range(&data, 2).unwrap().glyph(&data, 0x30),
            Some(0x30_u16.wrapping_add(0xFFF0)),
            "a delta wraps modulo 65536"
        );
        assert_eq!(subtable.range(&data, 3), None, "the terminating segment");
        assert_eq!(
            sample_of(&[(CMAP_TAG, cmap(&[(3, 1, indexed)]))], 16),
            vec![('B', 166), ('0', 32)],
            "an unmapped middle codepoint is left out"
        );
        // A symbol subtable alone, or no cmap at all, samples nothing.
        assert!(sample_of(&[(CMAP_TAG, cmap(&[(3, 0, bmp)]))], 16).is_empty());
        assert!(sample_of(&[(*b"head", head(1000))], 16).is_empty());
    }

    #[test]
    fn a_cmap_sample_spreads_its_picks_over_the_ranges() {
        let groups: Vec<(u32, u32, u32)> = (0..64)
            .map(|index| (index * 4, index * 4, index + 1))
            .collect();
        let sample = sample_of(&[(CMAP_TAG, cmap(&[(3, 10, cmap12(&groups))]))], 4);
        assert_eq!(
            sample
                .iter()
                .map(|(character, _)| u32::from(*character))
                .collect::<Vec<_>>(),
            vec![32, 96, 160, 224],
            "the middle of each quarter"
        );
    }

    #[test]
    fn an_instance_is_applied_only_within_each_axis_precision() {
        let axes = [
            axis(b"wght", 100.0, 500.0, 600.0),
            axis(b"WDTH", 1.0, 500.0, 1000.0),
        ];
        let [wght, wdth] = [axes[0].tag, axes[1].tag];
        let asked = [(wght, 300.0), (wdth, 500.0)];
        assert_eq!(unapplied_axis(&axes, &asked, &asked), None);
        // One F2Dot14 step below the default is 400 / 16384 user units.
        let step = 400.0 / F2DOT14_ONE;
        assert_eq!(axis_precision(&axes[0], 300.0), step);
        assert_eq!(axis_precision(&axes[0], 550.0), 100.0 / F2DOT14_ONE);
        let close = [(wght, 300.0 + step), (wdth, 500.0)];
        assert_eq!(unapplied_axis(&axes, &asked, &close), None, "within a step");
        let off = [(wdth, 500.0), (wght, 300.0 + 2.0 * step)];
        assert_eq!(
            unapplied_axis(&axes, &asked, &off),
            Some(UnappliedAxis {
                tag: wght,
                asked: 300.0,
                applied: Some(300.0 + 2.0 * step)
            }),
            "matched by tag, not by position"
        );
        let missing = unapplied_axis(&axes, &asked, &[(wght, 300.0)]).unwrap();
        assert_eq!(missing.applied, None);
        assert_eq!(
            missing.to_string(),
            "WDTH is not an axis of CoreText's copy"
        );
        assert_eq!(
            unapplied_axis(&axes, &asked, &[(wght, 500.0), (wdth, 500.0)])
                .unwrap()
                .to_string(),
            "wght read back as 500, asked 300"
        );
        // A degenerate axis still allows one `Fixed` unit.
        assert_eq!(
            axis_precision(&axis(b"flat", 5.0, 5.0, 5.0), 5.0),
            1.0 / FIXED_ONE
        );
        assert_eq!(instance_list(&asked), "wght=300 WDTH=500");
    }

    #[test]
    fn a_refused_fetch_caches_nothing_and_evicts_nothing() {
        let mut cache = OutlineCache::new(1);
        cache.get_or_fetch(key(0), || Some(Fetched::Empty));
        assert!(cache.get_or_fetch(key(1), || None).is_none());
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.entries.contains_key(&key(0)), "nothing evicted");
        assert!(!cache.entries.contains_key(&key(1)), "nothing cached");
        assert_eq!(cache.order.len(), 1);
        assert_eq!((cache.hits, cache.misses), (0, 2));
    }

    /// The F2Dot14 coordinate a shaper hands a run of `axis` at the user
    /// value `user`: normalized against the axis range, then through its
    /// `avar` map (the first segment holding the value, as a shaper reads it).
    #[cfg(target_os = "macos")]
    fn normalized(axis: &Axis, user: f64) -> i16 {
        let user = user.clamp(axis.min, axis.max);
        let mut normalized = if user < axis.default {
            (user - axis.default) / (axis.default - axis.min)
        } else if axis.max > axis.default {
            (user - axis.default) / (axis.max - axis.default)
        } else {
            0.0
        };
        if let Some(pair) = axis
            .avar
            .windows(2)
            .find(|pair| normalized >= pair[0].0 && normalized <= pair[1].0)
        {
            let [(from0, to0), (from1, to1)] = [pair[0], pair[1]];
            normalized = to0 + (normalized - from0) * (to1 - to0) / (from1 - from0);
        }
        (normalized * F2DOT14_ONE).round() as i16
    }

    /// A known answer from CoreText itself: PingFang SC's named instances
    /// (`PingFangSC-Light`, `-Semibold`) report their user values through
    /// `CTFontCopyVariation`. Those values, turned into the coordinates a
    /// shaper hands a run, map back to the same user values, and the outline
    /// drawn at them from `PingFangSC-Medium` is the named instance's own.
    #[cfg(target_os = "macos")]
    #[test]
    fn pingfang_coordinates_draw_coretexts_named_instance() {
        let Some(font) = system_pingfang_sc() else {
            println!("skipped: PingFangUI.ttc is not installed");
            return;
        };
        let data = font.data.data();
        let axes = variation_axes(data, table_directory(data, font.index).unwrap());
        let glyph = 289;
        let mut outlines = HvglOutlines::default();
        for named in ["PingFangSC-Light", "PingFangSC-Semibold"] {
            let (instance, resolved) = platform::CtFace::open(named, 1000.0).expect("installed");
            assert_eq!(resolved, named);
            let reported = applied_variation(&instance.axis_defaults(), &instance);
            let user = |tag: u32| {
                reported
                    .iter()
                    .find(|(reported, _)| *reported == tag)
                    .map(|(_, value)| *value)
                    .expect("CoreText reports every axis")
            };
            let coords: Vec<i16> = axes
                .iter()
                .map(|axis| normalized(axis, user(axis.tag)))
                .collect();
            assert!(
                coords.iter().any(|coord| *coord != 0),
                "{named} is not the default"
            );
            for (axis, (tag, value)) in axes.iter().zip(user_values(&axes, &coords)) {
                let expected = user(tag);
                assert!(
                    (value - expected).abs() <= axis_precision(axis, expected),
                    "{named}: {value} for {expected}"
                );
            }
            let handle = FontHandle::new(font.clone())
                .with_normalized_coords(&coords)
                .with_identity(Some(Arc::from("PingFangSC-Medium")), false);
            let GlyphOutline::Path { path, .. } = outlines.outline(&handle, glyph) else {
                panic!("{named}: no outline");
            };
            let expected = instance.path(glyph).expect("the named instance's outline");
            assert_eq!(path.elements().len(), expected.len(), "{named}");
            let farthest = path
                .elements()
                .iter()
                .zip(&expected)
                .flat_map(|(drawn, named)| {
                    let points = |el: &PathEl| match *el {
                        PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
                        PathEl::QuadTo(a, b) => vec![a, b],
                        PathEl::CurveTo(a, b, c) => vec![a, b, c],
                        PathEl::ClosePath => Vec::new(),
                    };
                    assert_eq!(points(drawn).len(), points(named).len());
                    points(drawn)
                        .into_iter()
                        .zip(points(named))
                        .map(|(a, b)| (a - b).hypot())
                        .collect::<Vec<_>>()
                })
                .fold(0.0_f64, f64::max);
            assert!(
                farthest < 0.01,
                "{named}: a point {farthest} font units off"
            );
        }
        assert_eq!(outlines.variation_mismatches(), 0);
    }

    /// A read-back that disagrees with the values asked for refuses the
    /// instance: counted once, its glyphs unavailable, nothing cached under
    /// its key, and the default instance still drawn.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_forced_read_back_mismatch_refuses_the_instance_without_caching() {
        let Some(font) = system_pingfang_sc() else {
            println!("skipped: PingFangUI.ttc is not installed");
            return;
        };
        let name = Some(Arc::from("PingFangSC-Medium"));
        let data = font.data.data();
        let wght = variation_axes(data, table_directory(data, font.index).unwrap())
            .iter()
            .position(|axis| axis.tag == u32::from_be_bytes(*b"wght"))
            .expect("PingFang SC varies its weight");
        let mut coords = vec![0_i16; wght + 1];
        coords[wght] = 3276;
        let heavy = FontHandle::new(font.clone())
            .with_normalized_coords(&coords)
            .with_identity(name.clone(), false);
        let mut outlines = HvglOutlines::default();
        READ_BACK_SKEW.set(1.0);
        for _ in 0..3 {
            assert!(matches!(
                outlines.outline(&heavy, 289),
                GlyphOutline::Unavailable
            ));
        }
        READ_BACK_SKEW.set(0.0);
        assert_eq!(outlines.variation_mismatches(), 1, "counted once");
        assert_eq!(outlines.cached_outlines(), 0, "nothing cached");
        let face = &outlines.faces.open[0].1;
        assert!(face.instances.is_empty(), "no instance kept");
        assert_eq!(face.refused.len(), 1);
        assert_eq!(
            outlines.bytes_mismatches() + outlines.identity_mismatches(),
            0
        );
        let default = FontHandle::new(font.clone()).with_identity(name, false);
        assert!(matches!(
            outlines.outline(&default, 289),
            GlyphOutline::Path { .. }
        ));
        assert!(
            matches!(outlines.outline(&heavy, 289), GlyphOutline::Unavailable),
            "the refusal is remembered while the face is open"
        );
        assert_eq!(outlines.variation_mismatches(), 1);
        assert_eq!(outlines.cached_outlines(), 1, "the default instance only");
        // Without the skew the same instance draws on a fresh provider.
        let mut fresh = HvglOutlines::default();
        assert!(matches!(
            fresh.outline(&heavy, 289),
            GlyphOutline::Path { .. }
        ));
        assert_eq!(fresh.variation_mismatches(), 0);
    }

    /// `font`'s blob with every format 12 group of its Unicode `cmap` mapped
    /// one glyph further on (wrapping inside the face): the glyph count, units
    /// and axes still agree with the installed face; the character map does
    /// not.
    #[cfg(target_os = "macos")]
    fn with_shifted_cmap(font: &FontData) -> FontData {
        let mut data = font.data.data().to_vec();
        let directory = table_directory(&data, font.index).expect("a directory");
        let glyphs = u32::try_from(maxp_num_glyphs(&data, directory).expect("a maxp")).unwrap();
        let table = table_start(&data, directory, CMAP_TAG).expect("a cmap");
        let cmap = UnicodeCmap::find(&data, table).expect("a Unicode cmap");
        assert_eq!(cmap.format, 12);
        for group in 0..cmap.ranges {
            let at = cmap.at + 16 + 12 * group + 8;
            let shifted = (read_u32(&data, at).unwrap() + 1) % glyphs;
            data[at..at + 4].copy_from_slice(&shifted.to_be_bytes());
        }
        FontData::new(Blob::new(Arc::new(data)), font.index)
    }

    /// The installed bytes agree with CoreText's character map on every
    /// sampled codepoint; bytes whose `cmap` maps elsewhere are refused as a
    /// bytes mismatch naming the first differing codepoint.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_face_whose_cmap_disagrees_with_coretext_is_refused() {
        let Some(font) = system_pingfang_sc() else {
            println!("skipped: PingFangUI.ttc is not installed");
            return;
        };
        let (installed, _) =
            platform::CtFace::open("PingFangSC-Medium", 1000.0).expect("installed");
        let data = font.data.data();
        let sample = cmap_sample(
            data,
            table_directory(data, font.index).unwrap(),
            CMAP_SAMPLES,
        );
        assert_eq!(sample.len(), CMAP_SAMPLES);
        assert_eq!(cmap_disagreement(&font, &installed), None);
        let shifted = with_shifted_cmap(&font);
        assert_eq!(
            FaceShape::of_bytes(&shifted).disagreement(&FaceShape::of_installed(&installed)),
            None,
            "only the character map differs"
        );
        let why = cmap_disagreement(&shifted, &installed).expect("refused");
        let first = u32::from(sample[0].0);
        assert!(
            why.starts_with(&format!("U+{first:04X} is glyph ")),
            "{why}"
        );
        let mut outlines = HvglOutlines::default();
        let handle =
            FontHandle::new(shifted).with_identity(Some(Arc::from("PingFangSC-Medium")), false);
        for _ in 0..2 {
            assert!(matches!(
                outlines.outline(&handle, 289),
                GlyphOutline::Unavailable
            ));
        }
        assert_eq!(outlines.bytes_mismatches(), 1);
        assert_eq!(outlines.identity_mismatches(), 0);
    }

    #[test]
    fn a_glyph_is_scaled_to_the_run_size_with_y_flipped() {
        let transform = glyph_transform(Affine::translate((10.0, 20.0)), 5.0, 0.0, 20.0, 1000.0);
        let origin = transform * kurbo::Point::new(0.0, 0.0);
        assert_eq!((origin.x, origin.y), (15.0, 20.0));
        let up = transform * kurbo::Point::new(0.0, 1000.0);
        assert!((up.y - 0.0).abs() < 1e-9, "{up:?}");
    }
}
