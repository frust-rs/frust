//! The frust-side route for bitmap-first faces (colour emoji strikes), which
//! never reach `glifo`.
//!
//! A face is bitmap-first ([`strike_table`]) when its `sbix` table leaves
//! `DRAW_OUTLINES` clear, or when it carries `CBDT`/`EBDT` strikes and no
//! outlines. Apple Color Emoji is the first kind: it carries `glyf`
//! placeholders beside `sbix`, and on iOS its strikes are `emjc` records that
//! `glifo`'s `skrifa` cannot decode. Each glyph of such a run is resolved here:
//! its strike chosen by `skrifa`'s size rule, keyed exactly as `glifo` keys a
//! bitmap ([`bitmap_key`]), and on a miss produced, inserted and charged its
//! real slot ([`AtlasPolicy::insert_bitmap`]). On iOS an `sbix` glyph is drawn
//! by CoreText ([`super::apple_raster`]), which resolves every record type;
//! elsewhere the record is read here — `png ` decoded behind a
//! [`MAX_STRIKE_DIM`] header check, `dupe` and `flip` followed — and anything
//! else is counted skipped. When CoreText refuses a glyph (it cannot open or
//! verify the face, or draws nothing), the same strike's record is decoded
//! here before the glyph is counted skipped, so an app-bundled `sbix` face
//! with `png ` records still draws; its slot is marked ([`DECODED_BEARINGS`])
//! so it is placed as a decoded strike is. A resident glyph is drawn from its
//! slot with `glifo`'s own placement ([`bitmap_transform`]). Such runs never
//! take the resident-run cache or glyph quads: a strike is sampled bilinearly.
//!
//! A production that fails — a record that does not decode, a header over the
//! cap, a CoreText refusal with nothing decodable behind it — is remembered
//! under the glyph's key ([`AtlasPolicy::note_bitmap_production`]) and not
//! tried again: the glyph is counted skipped each frame without the work. An
//! `sbix` glyph no strike holds a record for is counted skipped too: a
//! bitmap-first face never falls back to outlines, so it is a missing glyph.
//! (A `CBDT`/`EBDT` bitmap-first face has no outlines to miss, and such a
//! glyph draws nothing.)
//!
//! Every offset read out of a `CBLC`/`EBLC` index subtable is computed in the
//! table's own 32-bit offset space with checked arithmetic, so a crafted
//! subtable is refused rather than wrapped into another record on a 32-bit
//! target.
//!
//! # Pin-bound
//!
//! The key, the strike rule and the placement restate `glifo` 0.3.0 and
//! `skrifa` 0.44 by hand (neither exports them), each pointing at its source;
//! a bump of either re-reads them.

use std::sync::Arc;

use frust_scene::GlyphRun;
use glifo::{AtlasSlot, DrawSink as _, GlyphRenderer as _, ImageCache};
use kurbo::{Affine, Point, Rect, Vec2};
use peniko::{Brush, Extend, FontData, ImageSampler};
use vello_common::paint::{Image, Paint};
use vello_common::pixmap::Pixmap;

use super::atlas_policy::{
    AtlasPolicy, KeyedSlot, MAX_BITMAP_STRIKES, StrikePpems, absorbed_scale, bitmap_key,
    prefer_strike,
};
use super::backend::{EngineGlyphSink, GlyphRunOutcome, context_paint};
use super::{GlyphRunTargets, OUTLINE_TAGS, read_u16, read_u32, table_directory, table_record};

/// The largest strike image, in pixels on either side, the route will
/// allocate for; a record claiming more is refused before decoding.
pub(crate) const MAX_STRIKE_DIM: u32 = 512;

/// The most `dupe`/`flip` references followed for one glyph.
const MAX_REFERENCE_HOPS: usize = 4;

/// `sbix` header flag: draw outlines in addition to the bitmaps.
const SBIX_DRAW_OUTLINES: u16 = 0x0002;

/// A PNG file's eight-byte signature.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// The slot bearings that mark an `sbix` strike decoded here on the CoreText
/// route (iOS), which is placed by [`bitmap_transform`] rather than by its
/// raster origin. No CoreText raster is accepted with this origin.
const DECODED_BEARINGS: (i16, i16) = (i16::MIN, i16::MIN);

/// Which table a bitmap-first face's strikes are read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StrikeTable {
    /// Apple's `sbix`.
    Sbix,
    /// `CBDT` located by `CBLC`.
    Cbdt,
    /// `EBDT` located by `EBLC`.
    Ebdt,
}

/// The table at `tag` in the directory at `directory`, bounds-checked.
fn table(data: &[u8], directory: usize, tag: [u8; 4]) -> Option<&[u8]> {
    let record = table_record(data, directory, tag)?;
    let offset = usize::try_from(read_u32(data, record.checked_add(8)?)?).ok()?;
    let length = usize::try_from(read_u32(data, record.checked_add(12)?)?).ok()?;
    data.get(offset..offset.checked_add(length)?)
}

/// Whether the face whose directory is at `directory` is bitmap-first, and so
/// which table its strikes come from (`sbix` first, as `skrifa` prefers it).
pub(crate) fn strike_table(data: &[u8], directory: usize) -> Option<StrikeTable> {
    let has = |tag: [u8; 4]| table_record(data, directory, tag).is_some();
    if let Some(sbix) = table(data, directory, *b"sbix")
        && read_u16(sbix, 2).is_some_and(|flags| flags & SBIX_DRAW_OUTLINES == 0)
    {
        return Some(StrikeTable::Sbix);
    }
    if OUTLINE_TAGS.into_iter().any(has) {
        return None;
    }
    if has(*b"CBDT") && has(*b"CBLC") {
        Some(StrikeTable::Cbdt)
    } else if has(*b"EBDT") && has(*b"EBLC") {
        Some(StrikeTable::Ebdt)
    } else {
        None
    }
}

/// The strikes of `kind`'s table, in table order (see [`StrikePpems`]).
pub(crate) fn strike_ppems(data: &[u8], directory: usize, kind: StrikeTable) -> StrikePpems {
    let mut strikes = StrikePpems::NONE;
    let Some(face) = Face::read(data, directory, kind) else {
        return strikes;
    };
    for index in 0..face.strike_count() {
        if let Some((choice, key)) = face.strike_ppem(index) {
            strikes.push(choice, key);
        }
    }
    strikes
}

/// One glyph's image in its chosen strike, with what `glifo` places it by.
#[derive(Debug, Clone, Copy)]
struct StrikeGlyph<'a> {
    /// The strike's ppem the entry is keyed at, and `skrifa`'s `ppem_x`/`ppem_y`.
    ppem: (u16, u16),
    /// The PNG record.
    png: &'a [u8],
    /// `skrifa`'s inner bearings, in strike pixels.
    inner: (f32, f32),
    /// `skrifa`'s outer bearings, in font units (zero for `CBDT`/`EBDT`).
    outer: (f32, f32),
    /// Whether the image is drawn mirrored horizontally (an `sbix` `flip`).
    mirrored: bool,
}

/// What a glyph's lookup found.
#[derive(Debug, Clone, Copy)]
enum Lookup<'a> {
    /// An image the route can decode, in the strike `skrifa` would choose.
    Image(StrikeGlyph<'a>),
    /// No strike holds a record for the glyph: nothing to draw.
    Absent,
    /// A record exists but none decodes here: counted skipped.
    Undecodable,
}

/// A bitmap-first face's tables, located once per run.
#[derive(Debug, Clone, Copy)]
struct Face<'a> {
    kind: StrikeTable,
    /// `sbix`, or `CBLC`/`EBLC`.
    strikes: &'a [u8],
    /// `CBDT`/`EBDT`; empty for `sbix`.
    images: &'a [u8],
    glyphs: u16,
    upem: u16,
    hmtx: &'a [u8],
    long_metrics: u16,
    /// `(loca, glyf, long offsets)` when the face has TrueType outlines.
    outlines: Option<(&'a [u8], &'a [u8], bool)>,
}

impl<'a> Face<'a> {
    fn read(data: &'a [u8], directory: usize, kind: StrikeTable) -> Option<Self> {
        let get = |tag: [u8; 4]| table(data, directory, tag);
        let (strikes, images) = match kind {
            StrikeTable::Sbix => (get(*b"sbix")?, &[][..]),
            StrikeTable::Cbdt => (get(*b"CBLC")?, get(*b"CBDT")?),
            StrikeTable::Ebdt => (get(*b"EBLC")?, get(*b"EBDT")?),
        };
        let head = get(*b"head")?;
        let outlines = match (get(*b"loca"), get(*b"glyf")) {
            (Some(loca), Some(glyf)) => Some((loca, glyf, read_u16(head, 50)? != 0)),
            _ => None,
        };
        Some(Self {
            kind,
            strikes,
            images,
            glyphs: get(*b"maxp")
                .and_then(|maxp| read_u16(maxp, 4))
                .unwrap_or(0),
            upem: read_u16(head, 18)?,
            hmtx: get(*b"hmtx").unwrap_or(&[]),
            long_metrics: get(*b"hhea")
                .and_then(|hhea| read_u16(hhea, 34))
                .unwrap_or(0),
            outlines,
        })
    }

    fn open(font: &'a FontData) -> Option<Self> {
        let data = font.data.data();
        let directory = table_directory(data, font.index)?;
        Self::read(data, directory, strike_table(data, directory)?)
    }

    /// How many strikes are read, bounded by [`MAX_BITMAP_STRIKES`]: `sbix`'s
    /// `numStrikes` and `CBLC`'s `numSizes` both sit at byte 4.
    fn strike_count(&self) -> usize {
        read_u32(self.strikes, 4).map_or(0, |count| (count as usize).min(MAX_BITMAP_STRIKES))
    }

    /// Strike `index`'s `(choice, key)` ppem: `sbix` ppem twice, or a
    /// `BitmapSize`'s `ppemY` and `ppemX`.
    fn strike_ppem(&self, index: usize) -> Option<(u16, u16)> {
        match self.kind {
            StrikeTable::Sbix => {
                let ppem = read_u16(self.strikes, self.sbix_strike(index)?)?;
                Some((ppem, ppem))
            }
            StrikeTable::Cbdt | StrikeTable::Ebdt => {
                let size = 8 + index.checked_mul(48)?;
                let ppem_x = *self.strikes.get(size + 44)?;
                let ppem_y = *self.strikes.get(size + 45)?;
                Some((u16::from(ppem_y), u16::from(ppem_x)))
            }
        }
    }

    fn sbix_strike(&self, index: usize) -> Option<usize> {
        let offset = read_u32(self.strikes, 8 + index.checked_mul(4)?)?;
        usize::try_from(offset).ok()
    }

    /// `glyph`'s raw `sbix` record in the strike at `strike`, or `None` when
    /// it has none (an empty one included): `(originX, originY, type, data)`.
    fn sbix_record(&self, strike: usize, glyph: u32) -> Option<(i16, i16, [u8; 4], &'a [u8])> {
        if glyph >= u32::from(self.glyphs) {
            return None;
        }
        let at = strike.checked_add(4)?.checked_add((glyph as usize) * 4)?;
        let start = strike.checked_add(read_u32(self.strikes, at)? as usize)?;
        let end = strike.checked_add(read_u32(self.strikes, at + 4)? as usize)?;
        let record = self.strikes.get(start..end).filter(|r| r.len() >= 8)?;
        let tag: [u8; 4] = record[4..8].try_into().ok()?;
        let x = read_u16(record, 0)? as i16;
        let y = read_u16(record, 2)? as i16;
        Some((x, y, tag, &record[8..]))
    }

    /// `glyph`'s `png ` record in the `sbix` strike at `strike`, following
    /// `dupe` (the same strike's record for the named glyph) and `flip` (that
    /// record mirrored) at most [`MAX_REFERENCE_HOPS`] times. `Err(true)` when
    /// a record exists but does not resolve, `Err(false)` when there is none.
    fn sbix_png(&self, strike: usize, glyph: u32) -> Result<(i16, i16, &'a [u8], bool), bool> {
        let mut glyph = glyph;
        let mut mirrored = false;
        for hop in 0..=MAX_REFERENCE_HOPS {
            let Some((x, y, tag, data)) = self.sbix_record(strike, glyph) else {
                return Err(hop > 0);
            };
            match &tag {
                b"png " => return Ok((x, y, data, mirrored)),
                b"dupe" | b"flip" => {
                    mirrored ^= &tag == b"flip";
                    glyph = u32::from(read_u16(data, 0).ok_or(true)?);
                }
                _ => return Err(true),
            }
        }
        Err(true)
    }

    /// The glyph's `(advance, lsb)` from `hmtx`, in font units.
    fn horizontal(&self, glyph: u32) -> (u16, i16) {
        let long = u32::from(self.long_metrics);
        if long == 0 {
            return (0, 0);
        }
        let at = |glyph: u32| (glyph as usize) * 4;
        let advance = read_u16(self.hmtx, at(glyph.min(long - 1))).unwrap_or(0);
        let lsb = if glyph < long {
            read_u16(self.hmtx, at(glyph) + 2)
        } else {
            read_u16(self.hmtx, at(long) + 2 * (glyph - long) as usize)
        };
        (advance, lsb.unwrap_or(0) as i16)
    }

    /// The glyph's `glyf` `yMin` (zero without outlines or for an empty
    /// glyph), as `skrifa`'s `GlyphMetrics::bounds` reads it unscaled.
    fn y_min(&self, glyph: u32) -> i16 {
        let Some((loca, glyf, long)) = self.outlines else {
            return 0;
        };
        let offset = |glyph: u32| -> Option<usize> {
            if long {
                read_u32(loca, glyph as usize * 4).map(|o| o as usize)
            } else {
                read_u16(loca, glyph as usize * 2).map(|o| usize::from(o) * 2)
            }
        };
        match (offset(glyph), offset(glyph + 1)) {
            (Some(start), Some(end)) if end > start => {
                read_u16(glyf, start + 4).map_or(0, |y| y as i16)
            }
            _ => 0,
        }
    }

    /// `glyph`'s image in the strike `skrifa`'s `glyph_for_size` would choose
    /// at `size` (skrifa 0.44 `bitmap.rs`), with `skrifa`'s bearings for it.
    fn lookup(&self, glyph: u32, size: f32) -> Lookup<'a> {
        let mut recorded = false;
        let mut found = [None; MAX_BITMAP_STRIKES];
        let count = self.strike_count();
        for (index, slot) in found.iter_mut().enumerate().take(count) {
            let image = match self.kind {
                StrikeTable::Sbix => self.sbix_image(index, glyph),
                StrikeTable::Cbdt | StrikeTable::Ebdt => self.bdt_image(index, glyph),
            };
            match image {
                Ok(image) => {
                    recorded = true;
                    *slot = image;
                }
                Err(record) => recorded |= record,
            }
        }
        let ppems = (0..count).map(|index| self.strike_ppem(index).map_or(0, |p| p.0));
        let chosen = prefer_strike(size, ppems.zip(found.iter().map(Option::is_some)));
        match chosen.and_then(|index| found[index]) {
            Some(image) => Lookup::Image(image),
            None if recorded => Lookup::Undecodable,
            None => Lookup::Absent,
        }
    }

    /// Strike `index`'s `sbix` image of `glyph`, or whether a record was found.
    fn sbix_image(&self, index: usize, glyph: u32) -> Result<Option<StrikeGlyph<'a>>, bool> {
        let strike = self.sbix_strike(index).ok_or(false)?;
        let ppem = read_u16(self.strikes, strike).ok_or(false)?;
        let (x, y, png, mirrored) = self.sbix_png(strike, glyph)?;
        let (advance, lsb) = self.horizontal(glyph);
        let mut inner = (f32::from(x), f32::from(y));
        if mirrored {
            // Mirrored about the centre of the glyph's advance, in strike
            // pixels: `glifo` places the image at `-inner.0`, so its span
            // `[-x, -x + w]` becomes `[a + x - w, a + x]`.
            let width = png_dimensions(png).map_or(0.0, |(w, _)| w as f32);
            let advance = f32::from(advance) * f32::from(ppem) / f32::from(self.upem.max(1));
            inner.0 = width - advance - inner.0;
        }
        Ok(Some(StrikeGlyph {
            ppem: (ppem, ppem),
            png,
            inner,
            outer: (f32::from(lsb), f32::from(self.y_min(glyph))),
            mirrored,
        }))
    }

    /// Strike `index`'s `CBDT`/`EBDT` image of `glyph`: a PNG record (image
    /// formats 17-19 at 32 bits per pixel, as `skrifa` decodes them).
    fn bdt_image(&self, index: usize, glyph: u32) -> Result<Option<StrikeGlyph<'a>>, bool> {
        let size = index
            .checked_mul(48)
            .and_then(|at| at.checked_add(8))
            .ok_or(false)?;
        let (ppem_y, ppem_x) = self.strike_ppem(index).ok_or(false)?;
        let bit_depth = *self
            .strikes
            .get(size.checked_add(46).ok_or(false)?)
            .ok_or(false)?;
        let BdtImage {
            format,
            bytes: image,
            metrics: big,
        } = self.bdt_location(size, glyph).ok_or(false)?;
        let metrics = |at: usize| -> Option<(f32, f32)> {
            let x = *image.get(at.checked_add(2)?)? as i8;
            let y = *image.get(at.checked_add(3)?)? as i8;
            Some((f32::from(x), f32::from(y)))
        };
        // The PNG follows the record's metrics (5 or 8 bytes, none for 19)
        // and its `u32` length.
        let data = |at: usize| -> Option<&'a [u8]> {
            let length = usize::try_from(read_u32(image, at)?).ok()?;
            let start = at.checked_add(4)?;
            image.get(start..start.checked_add(length)?)
        };
        let (inner, png) = match format {
            17 => (metrics(0), data(5)),
            18 => (metrics(0), data(8)),
            19 => (big, data(0)),
            _ => (None, None),
        };
        let (Some(inner), Some(png), 32) = (inner, png, bit_depth) else {
            return Err(true);
        };
        Ok(Some(StrikeGlyph {
            ppem: (ppem_x, ppem_y),
            png,
            inner,
            outer: (0.0, 0.0),
            mirrored: false,
        }))
    }

    /// Where `glyph`'s image lives in the strike whose `BitmapSize` is at
    /// `size`.
    ///
    /// Every offset is an `Offset32` into the table, so the arithmetic is done
    /// in `u32` and checked: an index subtable whose sums or products leave
    /// that range is refused, on every target alike, rather than wrapped into
    /// another record where `usize` is 32 bits wide (the browser shell).
    fn bdt_location(&self, size: usize, glyph: u32) -> Option<BdtImage<'a>> {
        let glyph = u16::try_from(glyph).ok()?;
        let strikes = self.strikes;
        let at = |offset: u32| usize::try_from(offset).ok();
        let u16_at = |offset: u32| read_u16(strikes, at(offset)?);
        let u32_at = |offset: u32| read_u32(strikes, at(offset)?);
        let size = u32::try_from(size).ok()?;
        let array = u32_at(size)?;
        let count = u32_at(size.checked_add(8)?)?;
        for entry in 0..count.min(u32::from(u16::MAX)) {
            let record = array.checked_add(entry.checked_mul(8)?)?;
            let (first, last) = (u16_at(record)?, u16_at(record.checked_add(2)?)?);
            if glyph < first || glyph > last {
                continue;
            }
            let header = array.checked_add(u32_at(record.checked_add(4)?)?)?;
            let index_format = u16_at(header)?;
            let image_format = u16_at(header.checked_add(2)?)?;
            let base = u32_at(header.checked_add(4)?)?;
            let nth = u32::from(glyph - first);
            let field = |offset: u32| header.checked_add(offset);
            let big = |offset: u32| -> Option<(f32, f32)> {
                let offset = at(field(offset)?)?;
                let x = *strikes.get(offset.checked_add(2)?)? as i8;
                let y = *strikes.get(offset.checked_add(3)?)? as i8;
                Some((f32::from(x), f32::from(y)))
            };
            // `(start, end)` of the image, relative to `base`, from a run of
            // equal-size images: the `n`th of them.
            let sized = |n: u32, image_size: u32| -> Option<(u32, u32)> {
                let start = n.checked_mul(image_size)?;
                Some((start, start.checked_add(image_size)?))
            };
            let (start, end, metrics) = match index_format {
                1 => {
                    let offsets = field(8)?.checked_add(nth.checked_mul(4)?)?;
                    let start = u32_at(offsets)?;
                    let end = u32_at(offsets.checked_add(4)?)?;
                    (start, end, None)
                }
                3 => {
                    let offsets = field(8)?.checked_add(nth.checked_mul(2)?)?;
                    let start = u16_at(offsets)?;
                    let end = u16_at(offsets.checked_add(2)?)?;
                    (u32::from(start), u32::from(end), None)
                }
                2 => {
                    let image_size = u32_at(field(8)?)?;
                    let (start, end) = sized(nth, image_size)?;
                    (start, end, big(12))
                }
                4 => {
                    let glyphs = u32_at(field(8)?)?;
                    let pairs = field(12)?;
                    let pair_at = |pair: u32| pairs.checked_add(pair.checked_mul(4)?);
                    let found = (0..glyphs.min(u32::from(u16::MAX)))
                        .find(|&pair| pair_at(pair).and_then(u16_at) == Some(glyph))?;
                    let offset = pair_at(found)?.checked_add(2)?;
                    let start = u16_at(offset)?;
                    let end = u16_at(offset.checked_add(4)?)?;
                    (u32::from(start), u32::from(end), None)
                }
                5 => {
                    let image_size = u32_at(field(8)?)?;
                    let glyphs = u32_at(field(20)?)?;
                    let ids = field(24)?;
                    let id_at = |id: u32| ids.checked_add(id.checked_mul(2)?);
                    let found = (0..glyphs.min(u32::from(u16::MAX)))
                        .find(|&id| id_at(id).and_then(u16_at) == Some(glyph))?;
                    let (start, end) = sized(found, image_size)?;
                    (start, end, big(12))
                }
                _ => return None,
            };
            let start = at(base.checked_add(start)?)?;
            let end = at(base.checked_add(end)?)?;
            let image = self
                .images
                .get(start..end)
                .filter(|image| !image.is_empty())?;
            return Some(BdtImage {
                format: image_format,
                bytes: image,
                metrics,
            });
        }
        None
    }
}

/// One `CBDT`/`EBDT` glyph image as its index subtable locates it.
#[derive(Debug, Clone, Copy)]
struct BdtImage<'a> {
    /// The image format (17-19 carry PNG).
    format: u16,
    /// The image record.
    bytes: &'a [u8],
    /// The index subtable's big metrics' bearings, for formats 2 and 5.
    metrics: Option<(f32, f32)>,
}

/// A PNG's IHDR `(width, height)`, read without decoding anything.
fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    if png.get(..8)? != PNG_SIGNATURE || png.get(12..16)? != b"IHDR" {
        return None;
    }
    Some((read_u32(png, 16)?, read_u32(png, 20)?))
}

/// `png` decoded, mirrored when `mirrored`, or `None` when its header names an
/// image wider or taller than [`MAX_STRIKE_DIM`] (refused before the decoder
/// allocates `w * h * 4` bytes) or it does not decode.
fn decode_png(png: &[u8], mirrored: bool) -> Option<Pixmap> {
    let (width, height) = png_dimensions(png)?;
    if width == 0 || height == 0 || width > MAX_STRIKE_DIM || height > MAX_STRIKE_DIM {
        return None;
    }
    let mut pixmap = Pixmap::from_png(std::io::Cursor::new(png)).ok()?;
    if mirrored {
        let row = usize::from(pixmap.width());
        if row > 0 {
            for line in pixmap.data_mut().chunks_exact_mut(row) {
                line.reverse();
            }
        }
    }
    Some(pixmap)
}

/// `glifo`'s `DrawProps` for an unhinted run (glifo 0.3.0 `glyph.rs`,
/// `prepare_glyph_run`): the positioning transform, the effective transform
/// and the size the run is drawn and its strike chosen at. A bitmap is never
/// hinted, so only the unhinted absorption is restated.
#[derive(Debug, Clone, Copy)]
struct DrawProps {
    positioning: Affine,
    effective: Affine,
    font_size: f32,
}

impl DrawProps {
    fn of(font_size: f32, transform: Affine) -> Self {
        let [_, _, _, d, e, f] = transform.as_coeffs();
        let positioning = transform.with_translation(Vec2::ZERO);
        match absorbed_scale(transform) {
            Some(_) => Self {
                positioning,
                effective: Affine::new([1.0, 0.0, 0.0, 1.0, e, f]),
                font_size: font_size * d as f32,
            },
            None => Self {
                positioning,
                effective: transform,
                font_size,
            },
        }
    }

    /// `glifo`'s `positioned_transform`: the glyph's pen at its baseline.
    fn positioned(&self, glyph: &frust_scene::Glyph) -> Affine {
        let pen = self.positioning * Point::new(f64::from(glyph.x), f64::from(glyph.y));
        Affine::translate(pen.to_vec2()) * self.effective
    }
}

/// `glifo`'s `calculate_bitmap_transform` (glifo 0.3.0 `glyph.rs`) for an
/// image `height` pixels tall: outer bearings in font units (an `sbix` glyph
/// whose `bearing_y` is zero takes CoreText's 100-unit offset), the
/// `size / ppem` scale, inner bearings, and an `sbix` image's bottom-left
/// origin.
fn bitmap_transform(
    positioned: Affine,
    glyph: &StrikeGlyph<'_>,
    font_size: f32,
    upem: u16,
    height: u16,
    sbix: bool,
) -> Affine {
    let x_scale = font_size / f32::from(glyph.ppem.0);
    let y_scale = font_size / f32::from(glyph.ppem.1);
    let units = font_size / f32::from(upem);
    let bearing_y = if glyph.outer.1 == 0.0 && sbix {
        100.0
    } else {
        glyph.outer.1
    };
    let origin_shift = if sbix {
        Vec2::new(0.0, -f64::from(height))
    } else {
        Vec2::ZERO
    };
    positioned
        .pre_translate(Vec2::new(
            (-glyph.outer.0 * units).into(),
            (bearing_y * units).into(),
        ))
        .pre_scale_non_uniform(f64::from(x_scale), f64::from(y_scale))
        .pre_translate(Vec2::new((-glyph.inner.0).into(), (-glyph.inner.1).into()))
        .pre_translate(origin_shift)
}

/// `glifo`'s `supports_atlas_caching` for a bitmap (glifo 0.3.0
/// `renderer.rs`): no skew, both scales positive.
fn atlas_drawable(transform: Affine) -> bool {
    const NEARLY_ZERO: f64 = 1.0 / 4096.0;
    let [a, b, c, d, _, _] = transform.as_coeffs();
    b.abs() <= NEARLY_ZERO && c.abs() <= NEARLY_ZERO && a > 0.0 && d > 0.0
}

/// `glifo`'s `render_bitmap_glyph_from_atlas` (glifo 0.3.0 `renderer.rs`):
/// the slot's rectangle under `transform`, sampled at `quality_for_scale`,
/// untinted.
fn draw_from_atlas(sink: &mut EngineGlyphSink<'_>, slot: AtlasSlot, transform: Affine) {
    let area = Rect::new(0.0, 0.0, f64::from(slot.width), f64::from(slot.height));
    let paint_transform = sink.atlas_paint_transform(&slot);
    let image = Image {
        image: sink.atlas_image_source(&slot),
        sampler: ImageSampler {
            x_extend: Extend::Pad,
            y_extend: Extend::Pad,
            quality: glifo::renderer::quality_for_scale(&transform),
            alpha: 1.0,
        },
    };
    let state = sink.save_state();
    sink.set_tint(None);
    sink.set_transform(transform);
    sink.set_paint_image(image);
    sink.set_paint_transform(paint_transform);
    sink.fill_rect(&area);
    sink.set_tint(None);
    sink.restore_state(state);
}

/// What the route may do with the atlas for one run: look entries up, and
/// insert the ones it misses when the policy routed the run to the atlas.
#[derive(Debug)]
pub(crate) struct BitmapRouting<'a> {
    /// The policy and the shared allocator, or `None` with the atlas off.
    pub(crate) atlas: Option<(&'a mut AtlasPolicy, &'a mut ImageCache)>,
    /// Whether a missing glyph may be inserted (an atlas route); otherwise
    /// only resident glyphs are drawn and the rest counted skipped.
    pub(crate) insert: bool,
}

/// Draw `run` of a bitmap-first face under its device `transform`, painted
/// with `paint` (the run's brush: unused by a colour strike, carried for the
/// sink's bookkeeping). Every glyph is drawn from the atlas or counted
/// skipped; a glyph no strike holds a record for draws nothing, and is
/// counted skipped in an `sbix` face (see this module's doc).
pub(crate) fn lower_run(
    run: &GlyphRun,
    transform: Affine,
    paint: Paint,
    context_brush: &Brush,
    routing: BitmapRouting<'_>,
    targets: GlyphRunTargets<'_>,
) -> GlyphRunOutcome {
    let GlyphRunTargets {
        generator,
        clips,
        frame,
        depth,
        ..
    } = targets;
    let mut sink = EngineGlyphSink::new(
        generator,
        clips,
        frame,
        depth,
        paint,
        context_paint(context_brush),
        None,
    );
    let skipped = draw_glyphs(run, transform, routing, &mut sink);
    let mut outcome = sink.outcome();
    outcome.skipped = outcome.skipped.saturating_add(skipped);
    outcome
}

/// Every glyph of `run` through the route, answering how many were skipped.
fn draw_glyphs(
    run: &GlyphRun,
    transform: Affine,
    routing: BitmapRouting<'_>,
    sink: &mut EngineGlyphSink<'_>,
) -> u32 {
    let count = u32::try_from(run.glyphs.len()).unwrap_or(u32::MAX);
    let font = run.font.font();
    let props = DrawProps::of(run.font_size, transform);
    let (Some(face), Some((policy, images)), true) = (
        Face::open(font),
        routing.atlas,
        props.font_size.is_finite() && props.font_size > 0.0,
    ) else {
        return count;
    };
    let name = core_text_face(run, &face);
    let mut draw = RunDraw {
        face,
        props,
        policy,
        images,
        insert: routing.insert,
        font: (font.data.id(), font.index),
    };
    let mut skipped = 0_u32;
    for glyph in &run.glyphs {
        let positioned = props.positioned(glyph);
        let drawn = match name {
            Some(name) => draw.core_text(name, glyph.id, positioned, sink),
            None => draw.decoded(glyph.id, positioned, sink),
        };
        if !drawn {
            skipped = skipped.saturating_add(1);
        }
    }
    skipped
}

/// The PostScript name CoreText draws `run`'s glyphs by: on iOS, for an
/// `sbix` face whose handle names one. `None` everywhere else, where the
/// record is decoded here (macOS CoreText faults drawing `sbix` bitmaps).
fn core_text_face<'r>(run: &'r GlyphRun, face: &Face<'_>) -> Option<&'r str> {
    if cfg!(target_os = "ios") && face.kind == StrikeTable::Sbix {
        run.font.postscript_name()
    } else {
        None
    }
}

/// One run's glyph loop state: its face, placement, and atlas borrows.
struct RunDraw<'r, 'f> {
    face: Face<'f>,
    props: DrawProps,
    policy: &'r mut AtlasPolicy,
    images: &'r mut ImageCache,
    insert: bool,
    /// The blob id and collection index the keys carry.
    font: (u64, u32),
}

impl RunDraw<'_, '_> {
    /// `key`'s slot, inserting `produce`'s pixmap on a miss when the run may
    /// insert; `Ok(None)` for a glyph with nothing to draw, `Err` for a skip.
    ///
    /// A key whose production already failed is not produced again (see
    /// [`AtlasPolicy::bitmap_refused`]); a failure here is remembered. An
    /// insert the atlas has no room for is not a failure of the glyph's and
    /// is retried on a later frame.
    fn resolve(
        &mut self,
        key: glifo::GlyphCacheKey,
        produce: impl FnOnce() -> Option<(Pixmap, (i16, i16))>,
    ) -> Result<Option<AtlasSlot>, ()> {
        match self.policy.keyed_slot(&key) {
            KeyedSlot::Resident(slot) => Ok(Some(slot)),
            KeyedSlot::Inkless => Ok(None),
            KeyedSlot::Absent if self.insert && !self.policy.bitmap_refused(&key) => {
                let produced = produce();
                self.policy.note_bitmap_production(&key, produced.is_some());
                let (pixmap, bearing) = produced.ok_or(())?;
                self.policy
                    .insert_bitmap(self.images, key, Arc::new(pixmap), bearing)
                    .map(Some)
                    .ok_or(())
            }
            KeyedSlot::Absent => Err(()),
        }
    }

    /// One glyph whose record is decoded here; whether it was drawn (or had
    /// nothing to draw), `false` for a skip.
    fn decoded(&mut self, glyph: u32, positioned: Affine, sink: &mut EngineGlyphSink<'_>) -> bool {
        let size = self.props.font_size;
        let image = match self.face.lookup(glyph, size) {
            Lookup::Image(image) => image,
            // Nothing to draw in a face with no outlines; a missing glyph in
            // an `sbix` face, whose glyph would otherwise draw its outline.
            Lookup::Absent => return self.face.kind != StrikeTable::Sbix,
            Lookup::Undecodable => return false,
        };
        let scale = (
            f64::from(size / f32::from(image.ppem.0)),
            f64::from(size / f32::from(image.ppem.1)),
        );
        if !atlas_drawable(self.props.effective.pre_scale_non_uniform(scale.0, scale.1)) {
            return false;
        }
        let key = bitmap_key(self.font.0, self.font.1, glyph, image.ppem.0);
        let slot = self.resolve(key, || {
            decode_png(image.png, image.mirrored).map(|pixmap| (pixmap, (0, 0)))
        });
        match slot {
            Ok(Some(slot)) => {
                let sbix = self.face.kind == StrikeTable::Sbix;
                let upem = self.face.upem;
                let at = bitmap_transform(positioned, &image, size, upem, slot.height, sbix);
                draw_from_atlas(sink, slot, at);
                true
            }
            Ok(None) => true,
            Err(()) => false,
        }
    }

    /// One glyph of an `sbix` face drawn by CoreText at its chosen strike's
    /// ppem, its slot's bearings holding the raster's origin (see
    /// [`super::apple_raster`]'s placement convention). When CoreText refuses
    /// it, the same strike's record is decoded here instead and its slot
    /// marked [`DECODED_BEARINGS`], placed as [`Self::decoded`] places one.
    fn core_text(
        &mut self,
        name: &str,
        glyph: u32,
        positioned: Affine,
        sink: &mut EngineGlyphSink<'_>,
    ) -> bool {
        let face = self.face;
        let size = self.props.font_size;
        let strikes = (0..face.strike_count()).map(|strike| {
            let ppem = face.strike_ppem(strike).map_or(0, |p| p.0);
            let held = face
                .sbix_strike(strike)
                .and_then(|at| face.sbix_record(at, glyph))
                .is_some();
            (ppem, held)
        });
        // No strike holds a record: a missing glyph (see `Self::decoded`).
        let Some((strike, ppem)) = prefer_strike(size, strikes).and_then(|strike| {
            let ppem = face.strike_ppem(strike)?.0;
            (ppem > 0).then_some((strike, ppem))
        }) else {
            return false;
        };
        let scale = f64::from(size / f32::from(ppem));
        if !atlas_drawable(self.props.effective.pre_scale(scale)) {
            return false;
        }
        let key = bitmap_key(self.font.0, self.font.1, glyph, ppem);
        let record = || face.sbix_image(strike, glyph).ok().flatten();
        let produce = || {
            rasterize(name, glyph, face.glyphs, ppem)
                .filter(|(_, origin)| *origin != DECODED_BEARINGS)
                .or_else(|| {
                    let image = record()?;
                    let pixmap = decode_png(image.png, image.mirrored)?;
                    Some((pixmap, DECODED_BEARINGS))
                })
        };
        match self.resolve(key, produce) {
            Ok(Some(slot)) if (slot.bearing_x, slot.bearing_y) == DECODED_BEARINGS => {
                let Some(image) = record() else {
                    return false;
                };
                let at = bitmap_transform(positioned, &image, size, face.upem, slot.height, true);
                draw_from_atlas(sink, slot, at);
                true
            }
            Ok(Some(slot)) => {
                let origin = Vec2::new(f64::from(slot.bearing_x), -f64::from(slot.bearing_y));
                draw_from_atlas(
                    sink,
                    slot,
                    positioned.pre_scale(scale).pre_translate(origin),
                );
                true
            }
            Ok(None) => true,
            Err(()) => false,
        }
    }
}

/// CoreText's raster of `glyph` at `ppem`, `glyphs` being the face's `maxp`
/// glyph count (the identity check, see [`super::apple_raster`]), with its origin as slot bearings.
#[cfg(target_os = "ios")]
fn rasterize(name: &str, glyph: u32, glyphs: u16, ppem: u16) -> Option<(Pixmap, (i16, i16))> {
    let raster = super::apple_raster::rasterize_glyph(
        name,
        u16::try_from(glyph).ok()?,
        glyphs,
        ppem,
        MAX_STRIKE_DIM,
    )?;
    let origin = (
        i16::try_from(raster.origin_x).ok()?,
        i16::try_from(raster.origin_y).ok()?,
    );
    Some((raster.pixmap, origin))
}

/// Off iOS no glyph is sent to CoreText ([`core_text_face`] answers `None`).
#[cfg(not(target_os = "ios"))]
fn rasterize(_name: &str, _glyph: u32, _glyphs: u16, _ppem: u16) -> Option<(Pixmap, (i16, i16))> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut png = PNG_SIGNATURE.to_vec();
        png.extend_from_slice(&13_u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&width.to_be_bytes());
        png.extend_from_slice(&height.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        png
    }

    /// The header is read without decoding, and an oversized one is refused
    /// before the decoder sees it.
    #[test]
    fn an_oversized_png_header_is_refused_before_decoding() {
        assert_eq!(png_dimensions(&png_header(40, 30)), Some((40, 30)));
        assert!(decode_png(&png_header(MAX_STRIKE_DIM + 1, 8), false).is_none());
        assert!(decode_png(&png_header(8, 1 << 20), false).is_none());
        assert_eq!(png_dimensions(b"not a png at all, no header"), None);
    }

    /// `glifo`'s placement, worked by hand for a 109 ppem `CBDT` strike drawn
    /// at 32 px: scale 32/109, then the inner bearings.
    #[test]
    fn the_cbdt_placement_is_glifos() {
        let glyph = StrikeGlyph {
            ppem: (109, 109),
            png: &[],
            inner: (2.0, 101.0),
            outer: (0.0, 0.0),
            mirrored: false,
        };
        let at = bitmap_transform(
            Affine::translate((8.0, 64.0)),
            &glyph,
            32.0,
            2048,
            128,
            false,
        );
        let s = f64::from(32.0_f32 / 109.0);
        let corner = at * Point::ZERO;
        assert!((corner.x - (8.0 - 2.0 * s)).abs() < 1e-9);
        assert!((corner.y - (64.0 - 101.0 * s)).abs() < 1e-9);
    }

    /// An `sbix` glyph with no outline offset takes the 100-unit drop and
    /// sits on its bottom-left corner.
    #[test]
    fn the_sbix_placement_takes_the_hundred_unit_offset() {
        let glyph = StrikeGlyph {
            ppem: (160, 160),
            png: &[],
            inner: (0.0, 0.0),
            outer: (0.0, 0.0),
            mirrored: false,
        };
        let at = bitmap_transform(Affine::IDENTITY, &glyph, 160.0, 800, 160, true);
        let corner = at * Point::ZERO;
        assert!((corner.x).abs() < 1e-9);
        assert!((corner.y - (20.0 - 160.0)).abs() < 1e-9, "{corner:?}");
    }

    /// Only a skew-free transform with positive scales samples a strike.
    #[test]
    fn only_an_axis_aligned_positive_transform_is_drawable() {
        assert!(atlas_drawable(Affine::scale_non_uniform(0.5, 2.0)));
        assert!(!atlas_drawable(Affine::rotate(0.3)));
        assert!(!atlas_drawable(Affine::scale_non_uniform(-1.0, 1.0)));
    }

    /// A mirrored decode reverses every row.
    #[test]
    fn a_flipped_record_decodes_mirrored() {
        let font = include_bytes!("../../../../testing/fonts/NotoColorEmoji-CBDT-Subset.ttf");
        let data: &[u8] = font;
        let directory = table_directory(data, 0).unwrap();
        let face = Face::read(data, directory, StrikeTable::Cbdt).unwrap();
        let Lookup::Image(image) = face.lookup(1, 32.0) else {
            panic!("the grinning face is in the strike");
        };
        let plain = decode_png(image.png, false).unwrap();
        let mirrored = decode_png(image.png, true).unwrap();
        let width = usize::from(plain.width());
        let row = 40 * width;
        assert_eq!(plain.data()[row], mirrored.data()[row + width - 1]);
    }

    /// `glifo`'s own `calculate_bitmap_transform` (glifo 0.3.0 `glyph.rs`),
    /// fed `skrifa`'s reading of the glyph's strike — the source `glifo` takes
    /// every input from — for `glyph` of `font` at `size` under `transform`,
    /// beside the route's own transform from its own reading.
    fn both_transforms(
        font: &FontData,
        glyph: frust_scene::Glyph,
        size: f32,
        transform: Affine,
    ) -> (Affine, Affine) {
        use skrifa::MetadataProvider as _;
        use skrifa::bitmap::{BitmapData, BitmapFormat, Origin};

        let face = Face::open(font).expect("a bitmap-first face");
        let props = DrawProps::of(size, transform);
        let Lookup::Image(image) = face.lookup(glyph.id, props.font_size) else {
            panic!("the route finds the glyph's image");
        };
        let height = decode_png(image.png, false)
            .expect("the strike decodes")
            .height();
        let sbix = face.kind == StrikeTable::Sbix;
        let ours = bitmap_transform(
            props.positioned(&glyph),
            &image,
            props.font_size,
            face.upem,
            height,
            sbix,
        );

        let skrifa_font = skrifa::FontRef::from_index(font.data.data(), font.index).unwrap();
        let strikes = skrifa_font.bitmap_strikes();
        let theirs = strikes
            .glyph_for_size(
                skrifa::instance::Size::new(props.font_size),
                skrifa::GlyphId::new(glyph.id),
            )
            .expect("skrifa finds the glyph's image");
        let BitmapData::Png(png) = theirs.data else {
            panic!("the strike is PNG data");
        };
        assert_eq!(png, image.png, "the route reads the record skrifa reads");
        let x_scale = props.font_size / theirs.ppem_x;
        let y_scale = props.font_size / theirs.ppem_y;
        let units = props.font_size / f32::from(face.upem);
        let bearing_y = if theirs.bearing_y == 0.0 && strikes.format() == Some(BitmapFormat::Sbix) {
            100.0
        } else {
            theirs.bearing_y
        };
        let origin_shift = match theirs.placement_origin {
            Origin::TopLeft => Vec2::ZERO,
            Origin::BottomLeft => Vec2::new(0.0, -f64::from(height)),
        };
        let theirs = props
            .positioned(&glyph)
            .pre_translate(Vec2::new(
                (-theirs.bearing_x * units).into(),
                (bearing_y * units).into(),
            ))
            .pre_scale_non_uniform(f64::from(x_scale), f64::from(y_scale))
            .pre_translate(Vec2::new(
                (-theirs.inner_bearing_x).into(),
                (-theirs.inner_bearing_y).into(),
            ))
            .pre_translate(origin_shift);
        (theirs, ours)
    }

    fn assert_same(theirs: Affine, ours: Affine, case: &str) {
        let (theirs, ours) = (theirs.as_coeffs(), ours.as_coeffs());
        for (a, b) in theirs.iter().zip(ours) {
            assert!(
                (a - b).abs() <= 1e-9,
                "{case}: glifo {theirs:?}, route {ours:?}"
            );
        }
    }

    fn noto_cbdt() -> FontData {
        FontData::new(
            peniko::Blob::new(Arc::new(
                include_bytes!("../../../../testing/fonts/NotoColorEmoji-CBDT-Subset.ttf")
                    .as_slice(),
            )),
            0,
        )
    }

    /// The route places a `CBDT` strike where `glifo` does, absorbed or not,
    /// at fractional positions: against `glifo`'s formula over `skrifa`'s
    /// reading, and against the transforms `glifo` 0.3.0 itself drew the same
    /// glyph uncached with (recorded with its `png` feature on, which this
    /// crate no longer enables).
    #[test]
    fn the_cbdt_placement_matches_glifos_own() {
        let font = noto_cbdt();
        let glyph = frust_scene::Glyph {
            id: 1,
            x: 5.25,
            y: 1.5,
        };
        #[rustfmt::skip]
        let recorded: [(Affine, f32, [f64; 6]); 12] = [
            (Affine::translate((8.0, 64.0)), 24.0,
                [0.22018349170684814, 0.0, 0.0, 0.22018349170684814, 13.25, 43.26146733760834]),
            (Affine::translate((8.0, 64.0)), 32.0,
                [0.29357796907424927, 0.0, 0.0, 0.29357796907424927, 13.25, 35.848625123500824]),
            (Affine::translate((8.0, 64.0)), 109.0, [1.0, 0.0, 0.0, 1.0, 13.25, -35.5]),
            (Affine::translate((8.0, 64.0)), 160.0,
                [1.4678899049758911, 0.0, 0.0, 1.4678899049758911, 13.25, -82.756880402565]),
            (Affine::translate((3.3, 40.7)) * Affine::scale(2.0), 24.0,
                [0.4403669834136963, 0.0, 0.0, 0.4403669834136963, 13.8, -0.7770653247833224]),
            (Affine::translate((3.3, 40.7)) * Affine::scale(2.0), 32.0,
                [0.5871559381484985, 0.0, 0.0, 0.5871559381484985, 13.8, -15.60274975299835]),
            (Affine::translate((3.3, 40.7)) * Affine::scale(2.0), 109.0,
                [2.0, 0.0, 0.0, 2.0, 13.8, -158.3]),
            (Affine::translate((3.3, 40.7)) * Affine::scale(2.0), 160.0,
                [2.9357798099517822, 0.0, 0.0, 2.9357798099517822, 13.8, -252.81376080513002]),
            (Affine::scale_non_uniform(1.5, 0.75), 24.0,
                [0.3302752375602722, 0.0, 0.0, 0.1651376187801361, 7.875, -15.553899496793747]),
            (Affine::scale_non_uniform(1.5, 0.75), 32.0,
                [0.4403669536113739, 0.0, 0.0, 0.22018347680568695, 7.875, -21.113531157374382]),
            (Affine::scale_non_uniform(1.5, 0.75), 109.0, [1.5, 0.0, 0.0, 0.75, 7.875, -74.625]),
            (Affine::scale_non_uniform(1.5, 0.75), 160.0,
                [2.2018348574638367, 0.0, 0.0, 1.1009174287319183, 7.875, -110.06766030192375]),
        ];
        for (transform, size, drawn) in recorded {
            let case = format!("{size} px under {transform:?}");
            let (theirs, ours) = both_transforms(&font, glyph, size, transform);
            assert_same(theirs, ours, &case);
            assert_same(
                Affine::new(drawn),
                ours,
                &format!("{case}, as glifo drew it"),
            );
        }
    }

    /// The route places an Apple Color Emoji `sbix` strike where `glifo`'s
    /// formula over `skrifa`'s reading does, its 100-unit offset included.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_sbix_placement_matches_glifos_own() {
        let Ok(bytes) = std::fs::read("/System/Library/Fonts/Apple Color Emoji.ttc") else {
            eprintln!("Apple Color Emoji is not installed here; skipping");
            return;
        };
        let font = FontData::new(peniko::Blob::new(Arc::new(bytes)), 0);
        for size in [20.0, 32.0, 100.0, 160.0, 300.0] {
            let glyph = frust_scene::Glyph {
                id: 2096,
                x: 3.5,
                y: 0.25,
            };
            let transform = Affine::translate((10.0, 120.0));
            let (theirs, ours) = both_transforms(&font, glyph, size, transform);
            assert_same(theirs, ours, &format!("{size} px"));
        }
    }

    /// [`noto_cbdt`] with its strike's PNG header claiming a 4096 px square.
    fn oversized_cbdt() -> FontData {
        let mut bytes =
            include_bytes!("../../../../testing/fonts/NotoColorEmoji-CBDT-Subset.ttf").to_vec();
        let ihdr = bytes
            .windows(4)
            .position(|window| window == b"IHDR")
            .expect("the strike carries a PNG");
        bytes[ihdr + 4..ihdr + 8].copy_from_slice(&4096_u32.to_be_bytes());
        bytes[ihdr + 8..ihdr + 12].copy_from_slice(&4096_u32.to_be_bytes());
        FontData::new(peniko::Blob::new(Arc::new(bytes)), 0)
    }

    /// A glyph whose strike the route refuses is produced once, however many
    /// frames draw it, and counted skipped on every one of them.
    #[test]
    fn a_refused_strike_is_produced_once_across_frames() {
        let run = GlyphRun {
            font: frust_scene::FontHandle::new(oversized_cbdt()),
            font_size: 32.0,
            brush: Brush::Solid(peniko::color::palette::css::RED),
            transform: Affine::translate((8.0, 64.0)),
            glyphs: vec![frust_scene::Glyph {
                id: 1,
                x: 0.0,
                y: 0.0,
            }],
        };
        let mut scene = frust_scene::Scene::new();
        frust_scene::SceneBuilder::new(&mut scene).draw_glyph_run(run);
        let mut compiler = crate::SceneCompiler::new(128, 128);
        for frame_index in 0..12 {
            let frame = compiler
                .compile(&scene, Affine::IDENTITY, (128, 128))
                .expect("an in-range scene compiles");
            assert_eq!(frame.skipped_glyphs, 1, "frame {frame_index}");
            compiler.acknowledge_glyph_clears();
            compiler.acknowledge_glyph_replay();
        }
        assert_eq!(
            compiler.bitmap_productions(),
            1,
            "the refusal is remembered rather than produced every frame"
        );
        assert_eq!(compiler.glyph_atlas_entries(), 0);
    }

    /// A `CBLC` holding one strike whose one index subtable (`format`, image
    /// format 17) covers glyphs 0..=10 with `fields` after its header.
    fn cblc(format: u16, fields: &[u8]) -> Vec<u8> {
        let mut table = Vec::new();
        table.extend_from_slice(&0x0003_0000_u32.to_be_bytes());
        table.extend_from_slice(&1_u32.to_be_bytes());
        let array = 8 + 48;
        let mut size = [0_u8; 48];
        size[0..4].copy_from_slice(&(array as u32).to_be_bytes());
        size[8..12].copy_from_slice(&1_u32.to_be_bytes());
        size[44] = 109;
        size[45] = 109;
        size[46] = 32;
        table.extend_from_slice(&size);
        // The subtable array: glyphs 0..=10, the header right after it.
        table.extend_from_slice(&0_u16.to_be_bytes());
        table.extend_from_slice(&10_u16.to_be_bytes());
        table.extend_from_slice(&8_u32.to_be_bytes());
        table.extend_from_slice(&format.to_be_bytes());
        table.extend_from_slice(&17_u16.to_be_bytes());
        table.extend_from_slice(&0_u32.to_be_bytes());
        table.extend_from_slice(fields);
        table
    }

    fn bdt_face<'a>(strikes: &'a [u8], images: &'a [u8]) -> Face<'a> {
        Face {
            kind: StrikeTable::Cbdt,
            strikes,
            images,
            glyphs: 11,
            upem: 2048,
            hmtx: &[],
            long_metrics: 0,
            outlines: None,
        }
    }

    /// An index subtable whose image offsets leave the table's 32-bit offset
    /// space is refused rather than wrapped onto another record; the same
    /// subtable with in-range sizes locates its image.
    #[test]
    fn an_index_subtable_whose_offsets_overflow_is_refused() {
        let images = vec![7_u8; 64];
        // Format 2: one image size for every glyph, then big metrics.
        let format2 = |image_size: u32| {
            let mut fields = image_size.to_be_bytes().to_vec();
            fields.extend_from_slice(&[0; 8]);
            cblc(2, &fields)
        };
        let sane = format2(4);
        let found = bdt_face(&sane, &images)
            .bdt_location(8, 3)
            .expect("in range");
        assert_eq!(found.bytes.len(), 4);
        // Glyph 2's image starts at 2 * 2^31: past `u32`.
        let wrapping = format2(0x8000_0000);
        assert!(bdt_face(&wrapping, &images).bdt_location(8, 2).is_none());
        // Its end does too, for glyph 1.
        let ending = format2(0x8000_0001);
        assert!(bdt_face(&ending, &images).bdt_location(8, 1).is_none());

        // Format 5: an image size, big metrics, then the glyph id list.
        let format5 = |image_size: u32| {
            let mut fields = image_size.to_be_bytes().to_vec();
            fields.extend_from_slice(&[0; 8]);
            fields.extend_from_slice(&3_u32.to_be_bytes());
            for id in [4_u16, 5, 6] {
                fields.extend_from_slice(&id.to_be_bytes());
            }
            cblc(5, &fields)
        };
        let sane = format5(4);
        assert!(bdt_face(&sane, &images).bdt_location(8, 6).is_some());
        let wrapping = format5(0xFFFF_FFFF);
        assert!(bdt_face(&wrapping, &images).bdt_location(8, 6).is_none());

        // A subtable header past the 32-bit offset space.
        let mut far = cblc(1, &[0; 48]);
        far[8 + 48 + 4..8 + 48 + 8].copy_from_slice(&0xFFFF_FFF0_u32.to_be_bytes());
        assert!(bdt_face(&far, &images).bdt_location(8, 1).is_none());
        // And a strike record index that cannot be addressed.
        assert!(
            bdt_face(&far, &images)
                .bdt_image(usize::MAX / 2, 1)
                .is_err()
        );
    }
}
