//! Which face a run's glyph ids are drawn from: the face CoreText opens by
//! the handle's PostScript name, checked against the handle's bytes before its
//! outlines are used ([`FaceShape`], [`cmap_disagreement`]), and the refusals of
//! a face that fails, remembered apart from the open faces, oldest out. Also
//! the `sfnt` table reads those checks and the variation instances rest on
//! (`fvar`, `avar`, `maxp`, `cmap`).

use std::collections::VecDeque;

use frust_scene::FontHandle;
use peniko::FontData;

use super::super::{head_units_per_em, read_u16, read_u32, table_directory, table_record};
use super::ffi::{CtFace, UNAVAILABLE};
use super::outline::{Face, Lru};
use super::{F2DOT14_ONE, FIXED_ONE, MAX_INSTANCES_PER_FACE, MAX_OPEN_FACES, MAX_REFUSED_FACES};

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

/// One `fvar` axis, with its `avar` segment map.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Axis {
    /// The axis tag as a big-endian integer — CoreText's axis identifier.
    pub(super) tag: u32,
    /// The axis minimum, in user units.
    pub(super) min: f64,
    /// The axis default, in user units.
    pub(super) default: f64,
    /// The axis maximum, in user units.
    pub(super) max: f64,
    /// The axis's `avar` map as `(from, to)` normalized pairs; empty when the
    /// face has no `avar` (the identity).
    pub(super) avar: Vec<(f64, f64)>,
}

/// The face's `fvar` axes with their `avar` maps, or none when it has no
/// readable `fvar`. An `avar` whose axis count disagrees with `fvar`'s is
/// ignored (the identity), as is any part of one that does not fit the blob.
pub(super) fn variation_axes(data: &[u8], directory: usize) -> Vec<Axis> {
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
    fn of_installed(face: &CtFace) -> Self {
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
fn cmap_disagreement(font: &FontData, face: &CtFace) -> Option<String> {
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
pub(super) fn tag_list(tags: &[u32]) -> String {
    tags.iter()
        .map(|tag| tag.to_be_bytes().escape_ascii().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// What identifies one face the table has looked at: the PostScript name
/// CoreText is asked for, plus the blob and collection index the glyph ids
/// were shaped against. Two handles sharing a name but not a blob are two faces.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct FaceKey {
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
pub(super) fn truncated(name: &str) -> &str {
    let mut end = name.len().min(LOGGED_NAME_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

/// The longest name a log line carries.
const LOGGED_NAME_BYTES: usize = 63;

/// The faces looked at most recently. Open faces and refusals are kept apart:
/// the open faces are an [`Lru`] bounded by `MAX_OPEN_FACES`, the refusals a
/// bounded set of their own, so churn among refusals never evicts a live face.
pub(super) struct FaceTable {
    /// The open faces, least recently used evicted first.
    pub(super) open: Lru<FaceKey, Face>,
    /// The keys of refused faces, oldest first, at most `MAX_REFUSED_FACES`.
    pub(super) refused: VecDeque<FaceKey>,
    /// The next face id.
    next_id: u64,
    /// Faces whose CoreText PostScript name differed from the one asked for;
    /// each was refused.
    pub(super) mismatches: u32,
    /// Faces whose handle carried no PostScript name; each was refused.
    pub(super) nameless: u32,
    /// Faces whose bytes disagreed with the CoreText face their name opened;
    /// each was refused.
    pub(super) bytes_mismatches: u32,
}

impl Default for FaceTable {
    fn default() -> Self {
        Self {
            open: Lru::new(MAX_OPEN_FACES),
            refused: VecDeque::new(),
            next_id: 0,
            mismatches: 0,
            nameless: 0,
            bytes_mismatches: 0,
        }
    }
}

impl FaceTable {
    /// `handle`'s face open in CoreText, opening it on first sight, or `None`
    /// when the face was refused (no name, a name CoreText does not honour
    /// exactly, or one it could not open). A refusal is remembered until it
    /// ages out of the refused set.
    ///
    /// A lookup allocates nothing: keys are compared in place.
    pub(super) fn open(&mut self, handle: &FontHandle) -> Option<&mut Face> {
        if self.open.touch_matching(|key| key.matches(handle)) {
            return self.open.newest_mut();
        }
        if self.refused.iter().any(|key| key.matches(handle)) {
            return None;
        }
        self.next_id = self.next_id.wrapping_add(1);
        let Some(face) = self.open_face(handle) else {
            self.refuse(FaceKey::of(handle));
            return None;
        };
        Some(self.open.insert(FaceKey::of(handle), face))
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
    let (ct, resolved) = CtFace::open(name, units_per_em).ok_or(UNAVAILABLE)?;
    Ok((
        Face {
            id,
            ct,
            units_per_em,
            axes: variation_axes(data, directory),
            instances: Lru::new(MAX_INSTANCES_PER_FACE),
            refused: VecDeque::new(),
            coretext_defaults: None,
        },
        resolved,
    ))
}

#[cfg(test)]
pub(super) mod tests {
    use std::sync::Arc;

    use peniko::Blob;

    #[cfg(target_os = "macos")]
    use super::super::font_needs_hvgl_provider;
    use super::super::outline::user_value;
    use super::super::{GlyphOutline, HVGL_TAG, HvglOutlines};
    use super::*;

    /// A single-face sfnt holding `tables` (tag, bytes), table data 4-aligned.
    pub(in super::super) fn sfnt(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
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

    pub(in super::super) fn head(units_per_em: u16) -> Vec<u8> {
        let mut head = vec![0; 54];
        head[18..20].copy_from_slice(&units_per_em.to_be_bytes());
        head
    }

    pub(in super::super) fn font(bytes: Vec<u8>) -> FontData {
        FontData::new(Blob::new(Arc::new(bytes)), 0)
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
        let live_id = outlines.faces.open.newest_mut().expect("open").id;
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
            outlines.faces.open.newest_mut().expect("open").id,
            live_id,
            "and was not opened again"
        );
        assert_eq!(outlines.identity_mismatches(), 0);
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
    pub(in super::super) fn system_pingfang_sc() -> Option<FontData> {
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
        let installed = CtFace::open("PingFangSC-Medium", 1000.0)
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
        let (installed, _) = CtFace::open("PingFangSC-Medium", 1000.0).expect("installed");
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
}
