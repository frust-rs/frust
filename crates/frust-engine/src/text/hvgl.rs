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
//! run is routed to outlines by the collect walk — treated there like a
//! colour face — so it reserves no atlas slot.
//!
//! # Opening the face in CoreText
//!
//! By PostScript name (name ID 6), read from the face's own `name` table. The
//! two routes that would open the bytes themselves do not work for the face
//! this exists for: `CTFontManagerCreateFontDescriptorsFromData` returns no
//! descriptor at all for `PingFangUI.ttc`, and the file-URL route expands named
//! instances, so its indices are not the collection's. A name is refused when
//! it starts with a dot — CoreText resolves `.PingFang…`-style hidden names to
//! a substitute (Times) rather than the face — and when the face CoreText hands
//! back carries a different PostScript name from the one asked for. A refused
//! face draws none of its glyphs, and every one of them is counted in
//! [`CompiledFrame::skipped_glyphs`](crate::compile::CompiledFrame::skipped_glyphs):
//! missing, never drawn in a wrong face. A target without CoreText refuses
//! every face, so there an `hvgl` run is counted as skipped whole.
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
//! every axis (axis tag to user value). Each face keeps its few most recently
//! used instances open ([`MAX_INSTANCES_PER_FACE`]). A run with no coordinates
//! — every non-variable face, and a variable one at its default — draws from
//! the face as opened, exactly as before.
//!
//! # The cache
//!
//! Outlines are retained in a least-recently-used cache bounded by entry count
//! ([`MAX_CACHED_OUTLINES`]), keyed by the face's blob id and collection index
//! — the pair `glifo` keys on — plus the glyph id and a [`VariationKey`] built
//! from the run's coordinates, so one instance's outline is never served for
//! another's. A glyph CoreText cannot supply is remembered as missing too, so a
//! refusal is paid for once.

use std::collections::{BTreeMap, HashMap};

use frust_scene::GlyphRun;
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
/// `name`, on the same terms.
const NAME_TAG: [u8; 4] = *b"name";
/// `fvar`, the face's variation axes.
const FVAR_TAG: [u8; 4] = *b"fvar";
/// `avar`, the face's per-axis normalization remapping.
const AVAR_TAG: [u8; 4] = *b"avar";
/// An `fvar` header's fixed length, ahead of its axis records.
const FVAR_HEADER_LEN: usize = 16;
/// The shortest `fvar` axis record: tag, minimum, default, maximum, flags and
/// name id.
const FVAR_AXIS_MIN_LEN: usize = 20;
/// An `avar` header's fixed length, ahead of its segment maps.
const AVAR_HEADER_LEN: usize = 8;
/// One F2Dot14 unit: a normalized coordinate of 1.0.
const F2DOT14_ONE: f64 = 16384.0;
/// One 16.16 `Fixed` unit.
const FIXED_ONE: f64 = 65536.0;
/// The `name` table's PostScript-name record.
const POSTSCRIPT_NAME_ID: u16 = 6;
/// The longest PostScript name the OpenType `name` table allows.
const MAX_POSTSCRIPT_NAME_LEN: usize = 63;
/// A `name` table header's fixed length, ahead of its records.
const NAME_HEADER_LEN: usize = 6;
/// One `name` record: platform, encoding, language, name id, length, offset.
const NAME_RECORD_LEN: usize = 12;

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
    /// The face blob's process-unique id.
    font_id: u64,
    /// The face's index inside its collection.
    face_index: u32,
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
    let font = run.font.font();
    let coords = run.font.normalized_coords();
    let mut unavailable = 0_u32;
    for glyph in &run.glyphs {
        match outlines.outline(font, glyph.id, coords) {
            GlyphOutline::Path { path, units_per_em } => {
                sink.set_transform(glyph_transform(
                    transform,
                    glyph.x,
                    glyph.y,
                    run.font_size,
                    units_per_em,
                ));
                sink.fill_path(path);
            }
            GlyphOutline::Empty => {}
            GlyphOutline::Unavailable => unavailable = unavailable.saturating_add(1),
        }
    }
    unavailable
}

/// The compiler's retained `hvgl` state: the faces open in CoreText and the
/// bounded outline cache.
pub(crate) struct HvglOutlines {
    /// The faces opened (or refused) most recently.
    faces: FaceTable,
    /// The outlines fetched, least recently used evicted first.
    outlines: OutlineCache,
}

impl Default for HvglOutlines {
    fn default() -> Self {
        Self::with_capacity(MAX_CACHED_OUTLINES)
    }
}

impl core::fmt::Debug for HvglOutlines {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HvglOutlines")
            .field("faces", &self.faces.faces.len())
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
        }
    }

    /// How many glyph outlines are cached.
    pub(crate) fn cached_outlines(&self) -> usize {
        self.outlines.entries.len()
    }

    /// How many glyph outlines the cache keeps at most.
    pub(crate) fn capacity(&self) -> usize {
        self.outlines.capacity
    }

    /// `glyph_id`'s outline in `font`'s face at the variation instance
    /// `coords` names (normalized F2Dot14 values in `fvar` axis order; empty
    /// for the default instance), fetched from CoreText on a cache miss.
    ///
    /// Coordinates on a face with no `fvar` axes name nothing to vary, and
    /// draw from the face as opened.
    pub(crate) fn outline(
        &mut self,
        font: &FontData,
        glyph_id: u32,
        coords: &[i16],
    ) -> GlyphOutline<'_> {
        let Self { faces, outlines } = self;
        let Some(face) = faces.open(font) else {
            return GlyphOutline::Unavailable;
        };
        let units_per_em = face.units_per_em;
        let variation = if face.axes.is_empty() {
            VariationKey::DEFAULT
        } else {
            VariationKey::of(coords)
        };
        let key = OutlineKey {
            font_id: font.data.id(),
            face_index: font.index,
            glyph_id,
            variation,
        };
        match outlines.get_or_fetch(key, || fetch(face.instance(variation, coords), glyph_id)) {
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
}

impl OutlineCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
        }
    }

    /// `key`'s entry, marked most recently used, or `fetch`'s answer inserted
    /// in its place — evicting the least recently used entry when full.
    fn get_or_fetch(
        &mut self,
        key: OutlineKey,
        fetch: impl FnOnce() -> Fetched,
    ) -> Option<&Fetched> {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        if let Some((_, used)) = self.entries.get_mut(&key) {
            self.order.remove(used);
            *used = tick;
        } else {
            if self.entries.len() >= self.capacity
                && let Some((_, oldest)) = self.order.pop_first()
            {
                self.entries.remove(&oldest);
            }
            self.entries.insert(key, (fetch(), tick));
        }
        self.order.insert(tick, key);
        self.entries.get(&key).map(|(fetched, _)| fetched)
    }
}

/// One face open in CoreText.
struct Face {
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
    /// A use counter, for the least-recently-used replacement of `instances`.
    uses: u64,
}

impl Face {
    /// The CoreText face to read `variation`'s outlines from: the face as
    /// opened for the default instance, else a copy at the user values
    /// `coords` maps to, copied on first use.
    fn instance(&mut self, variation: VariationKey, coords: &[i16]) -> &platform::CtFace {
        if variation == VariationKey::DEFAULT {
            return &self.ct;
        }
        self.uses = self.uses.wrapping_add(1);
        let position = match self
            .instances
            .iter()
            .position(|(key, ..)| *key == variation)
        {
            Some(position) => position,
            None => {
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
                let values = user_values(&self.axes, coords);
                let Some(varied) = self.ct.varied(&values, self.units_per_em) else {
                    return &self.ct;
                };
                self.instances.push((variation, varied, 0));
                self.instances.len() - 1
            }
        };
        let Some(entry) = self.instances.get_mut(position) else {
            return &self.ct;
        };
        entry.2 = self.uses;
        &entry.1
    }
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
    let table = table_start(data, directory, FVAR_TAG)?;
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

/// A face the provider has looked at: open, or refused for good.
enum FaceSlot {
    /// Opened in CoreText.
    Open(Face),
    /// Refused; its glyphs are counted as skipped.
    Refused,
}

/// The faces looked at most recently, keyed like `glifo`'s caches.
#[derive(Default)]
struct FaceTable {
    /// `(blob id, index, slot, last use)` per face.
    faces: Vec<(u64, u32, FaceSlot, u64)>,
    /// A use counter, for the least-recently-used replacement.
    uses: u64,
}

impl FaceTable {
    /// `font`'s face open in CoreText, opening it on first sight, or `None`
    /// when it was refused.
    fn open(&mut self, font: &FontData) -> Option<&mut Face> {
        let id = font.data.id();
        self.uses = self.uses.wrapping_add(1);
        let position = match self
            .faces
            .iter()
            .position(|(face_id, index, ..)| *face_id == id && *index == font.index)
        {
            Some(position) => position,
            None => {
                if self.faces.len() >= MAX_OPEN_FACES
                    && let Some(oldest) = self
                        .faces
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, (.., used))| *used)
                        .map(|(position, _)| position)
                {
                    self.faces.swap_remove(oldest);
                }
                self.faces.push((id, font.index, open_face(font), 0));
                self.faces.len().checked_sub(1)?
            }
        };
        let entry = self.faces.get_mut(position)?;
        entry.3 = self.uses;
        match &mut entry.2 {
            FaceSlot::Open(face) => Some(face),
            FaceSlot::Refused => None,
        }
    }
}

/// `font`'s face opened in CoreText, or refused with a warning naming why.
fn open_face(font: &FontData) -> FaceSlot {
    match try_open_face(font) {
        Ok(face) => FaceSlot::Open(face),
        Err(reason) => {
            log::warn!(
                "frust-engine: hvgl face {} of font blob {} refused ({reason}); its glyphs are \
                 counted as skipped",
                font.index,
                font.data.id(),
            );
            FaceSlot::Refused
        }
    }
}

fn try_open_face(font: &FontData) -> Result<Face, String> {
    let data = font.data.data();
    let directory = table_directory(data, font.index).ok_or("no table directory")?;
    let units_per_em = head_units_per_em(data, directory)
        .filter(|units| *units > 0)
        .map(f64::from)
        .ok_or("no usable unitsPerEm")?;
    let name = postscript_name(data, directory).ok_or("no PostScript name")?;
    if name.starts_with('.') {
        return Err(format!(
            "'{name}' is a hidden system name, which CoreText resolves to a substitute face"
        ));
    }
    if !is_postscript_name(&name) {
        return Err(format!("'{name}' is not a valid PostScript name"));
    }
    let ct = platform::CtFace::open(&name, units_per_em)
        .ok_or_else(|| format!("'{name}': {}", platform::UNAVAILABLE))?;
    Ok(Face {
        ct,
        units_per_em,
        axes: variation_axes(data, directory),
        instances: Vec::new(),
        uses: 0,
    })
}

/// Whether `name` is a PostScript name as the `name` table defines one:
/// 1-63 printable ASCII characters.
fn is_postscript_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_POSTSCRIPT_NAME_LEN
        && name.bytes().all(|byte| (b'!'..=b'~').contains(&byte))
}

/// The face's PostScript name (name ID 6), from a Unicode or Windows-Unicode
/// record (UTF-16BE), else a Macintosh-Roman one.
fn postscript_name(data: &[u8], directory: usize) -> Option<String> {
    let record = table_record(data, directory, NAME_TAG)?;
    let table = usize::try_from(read_u32(data, record.checked_add(8)?)?).ok()?;
    let count = usize::from(read_u16(data, table.checked_add(2)?)?);
    let strings = table.checked_add(usize::from(read_u16(data, table.checked_add(4)?)?))?;
    let mut mac_roman = None;
    for index in 0..count {
        let at = table
            .checked_add(NAME_HEADER_LEN)?
            .checked_add(index.checked_mul(NAME_RECORD_LEN)?)?;
        let platform = read_u16(data, at)?;
        let encoding = read_u16(data, at.checked_add(2)?)?;
        if read_u16(data, at.checked_add(6)?)? != POSTSCRIPT_NAME_ID {
            continue;
        }
        let length = usize::from(read_u16(data, at.checked_add(8)?)?);
        let start = strings.checked_add(usize::from(read_u16(data, at.checked_add(10)?)?))?;
        let bytes = data.get(start..start.checked_add(length)?)?;
        match (platform, encoding) {
            (0, _) | (3, 1) => {
                if bytes.len() % 2 != 0 {
                    return None;
                }
                let units = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_be_bytes(*pair));
                return char::decode_utf16(units)
                    .collect::<Result<String, _>>()
                    .ok();
            }
            // Only ASCII survives `is_postscript_name`, and Mac Roman agrees
            // with ASCII there.
            (1, 0) => mac_roman = Some(bytes.iter().copied().map(char::from).collect()),
            _ => {}
        }
    }
    mac_roman
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
    use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};
    use objc2_core_graphics::{CGPath, CGPathElement, CGPathElementType};
    use objc2_core_text::{CTFont, CTFontDescriptor, CTFontOrientation, kCTFontVariationAttribute};

    /// Why [`CtFace::open`] refuses a face here.
    pub(super) const UNAVAILABLE: &str =
        "CoreText resolves that name to a different face, or to none";

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
        /// The installed face whose PostScript name is `name`, at
        /// `units_per_em` points, or `None` when CoreText hands back any other
        /// face for it (its substitute for a name it does not know).
        pub(super) fn open(name: &str, units_per_em: f64) -> Option<Self> {
            let requested = CFString::from_str(name);
            // SAFETY: a null matrix is the documented "identity" argument.
            let font = unsafe { CTFont::with_name(&requested, units_per_em, core::ptr::null()) };
            // SAFETY: a getter on a live font.
            let resolved = unsafe { font.post_script_name() }.to_string();
            if resolved != name {
                return None;
            }
            // SAFETY: a getter on a live font.
            let glyph_count = usize::try_from(unsafe { font.glyph_count() }).unwrap_or(0);
            Some(Self { font, glyph_count })
        }

        /// A copy of this face at the variation instance `axes` names — each
        /// `(axis tag, user value)` — at `units_per_em` points.
        ///
        /// Built from a `kCTFontVariationAttribute` dictionary (axis
        /// identifier to value, both `CFNumber`s), which CoreText honours on
        /// an `hvgl` face as on any variable one. Always `Some` here; the
        /// `Option` is the shape the target without CoreText shares.
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
        pub(super) fn open(_name: &str, _units_per_em: f64) -> Option<Self> {
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

    /// A `name` table with one PostScript-name record on `platform`/`encoding`.
    fn name(platform: u16, encoding: u16, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(0_u16.to_be_bytes());
        out.extend(1_u16.to_be_bytes());
        out.extend(18_u16.to_be_bytes());
        for field in [platform, encoding, 0, POSTSCRIPT_NAME_ID] {
            out.extend(field.to_be_bytes());
        }
        out.extend(u16::try_from(bytes.len()).unwrap().to_be_bytes());
        out.extend(0_u16.to_be_bytes());
        out.extend(bytes);
        out
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_be_bytes).collect()
    }

    fn font(bytes: Vec<u8>) -> FontData {
        FontData::new(Blob::new(Arc::new(bytes)), 0)
    }

    fn key(glyph_id: u32) -> OutlineKey {
        OutlineKey {
            font_id: 1,
            face_index: 0,
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

    #[test]
    fn the_postscript_name_is_read_from_a_unicode_or_mac_roman_record() {
        for (platform, encoding, bytes) in [
            (3, 1, utf16("PingFangSC-Medium")),
            (0, 3, utf16("PingFangSC-Medium")),
            (1, 0, b"PingFangSC-Medium".to_vec()),
        ] {
            let data = sfnt(&[
                (*b"head", head(1000)),
                (NAME_TAG, name(platform, encoding, &bytes)),
            ]);
            let directory = table_directory(&data, 0).unwrap();
            assert_eq!(
                postscript_name(&data, directory).as_deref(),
                Some("PingFangSC-Medium"),
                "platform {platform} encoding {encoding}",
            );
        }
        let odd = sfnt(&[
            (*b"head", head(1000)),
            (NAME_TAG, name(3, 1, &[0, b'A', 0])),
        ]);
        assert_eq!(
            postscript_name(&odd, table_directory(&odd, 0).unwrap()),
            None
        );
        // A record pointing past the blob reads as no name, not as a panic.
        let mut truncated = name(3, 1, &utf16("Name"));
        truncated.truncate(20);
        let short = sfnt(&[(*b"head", head(1000)), (NAME_TAG, truncated)]);
        assert_eq!(
            postscript_name(&short, table_directory(&short, 0).unwrap()),
            None
        );
    }

    #[test]
    fn only_printable_ascii_up_to_63_bytes_is_a_postscript_name() {
        assert!(is_postscript_name("PingFangSC-Medium"));
        assert!(!is_postscript_name(""));
        assert!(!is_postscript_name("Ping Fang"));
        assert!(!is_postscript_name("平方"));
        assert!(!is_postscript_name(&"A".repeat(64)));
    }

    #[test]
    fn hidden_and_nameless_faces_are_refused() {
        let hidden = font(sfnt(&[
            (*b"head", head(1000)),
            (HVGL_TAG, vec![0; 8]),
            (NAME_TAG, name(3, 1, &utf16(".PingFangUITextSC-Default"))),
        ]));
        let error = try_open_face(&hidden).err().unwrap();
        assert!(error.contains("hidden"), "{error}");
        let nameless = font(sfnt(&[(*b"head", head(1000)), (HVGL_TAG, vec![0; 8])]));
        assert_eq!(
            try_open_face(&nameless).err().as_deref(),
            Some("no PostScript name")
        );
        let mut outlines = HvglOutlines::default();
        assert!(matches!(
            outlines.outline(&hidden, 1, &[]),
            GlyphOutline::Unavailable
        ));
        assert_eq!(outlines.cached_outlines(), 0);
    }

    #[test]
    fn the_outline_cache_evicts_the_least_recently_used_entry_at_its_cap() {
        let mut cache = OutlineCache::new(3);
        for glyph in 0..3 {
            cache.get_or_fetch(key(glyph), || Fetched::Empty);
        }
        // Touch glyph 0, so glyph 1 is now the oldest.
        let mut fetched = false;
        cache.get_or_fetch(key(0), || {
            fetched = true;
            Fetched::Missing
        });
        assert!(!fetched, "a cached glyph is not fetched again");
        cache.get_or_fetch(key(3), || Fetched::Empty);
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

    /// The face the macOS cases read: `PingFangSC-Medium` from the system
    /// collection, or `None` when it is not installed.
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
        (0..count).find_map(|index| {
            let font = FontData::new(blob.clone(), index);
            let data = font.data.data();
            let directory = table_directory(data, index)?;
            (postscript_name(data, directory).as_deref() == Some("PingFangSC-Medium"))
                .then_some(font)
        })
    }

    /// The light and semibold instances of PingFang SC's `中` have different
    /// outline bounds: the coordinates reach CoreText.
    #[cfg(target_os = "macos")]
    #[test]
    fn pingfang_outlines_follow_the_requested_weight() {
        let Some(font) = system_pingfang_sc() else {
            println!("skipped: PingFangUI.ttc's PingFangSC-Medium is not installed");
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
            match outlines.outline(&font, glyph, &coords) {
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
