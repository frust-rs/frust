//! parley layout -> `frust_scene` glyph-run conversion.
//!
//! This module owns the parley -> [`frust_scene::GlyphRun`] glyph-mapping
//! contract — the one `frust-engine`'s text lowering (`text/`, built on
//! `glifo`) consumes.
//!
//! # Coordinate convention
//!
//! Each emitted [`frust_scene::Glyph`] carries **run-local** coordinates that
//! are exactly what `glifo`'s own `Glyph` (`x`/`y`, each documented "offset in
//! run, relative to transform") expects:
//!
//! - `x` is the glyph's absolute horizontal position within its run, measured
//!   from the run origin — parley's per-glyph advance offset plus the run's
//!   `offset()` (the leading whitespace/indent before the first glyph). This is
//!   the value produced by [`parley`]'s `positioned_glyphs()`.
//! - `y` is the glyph's baseline-relative vertical position with the line
//!   baseline already applied (again via `positioned_glyphs()`), so glyphs on
//!   the same line share a `y` and successive lines get distinct, increasing
//!   `y` values.
//!
//! The block-level placement (`origin`, passed by the widget) is **not** baked
//! into the glyph coordinates; it lives on [`frust_scene::GlyphRun::transform`]
//! as a translation. The render backend applies that transform to the whole
//! run — the engine compiler composes it into the run's paint and its glyph
//! outlines alike (`frust-engine`'s `compile_glyph_run`). Keeping origin in the
//! transform (rather than added into every glyph) lets the scene builder compose
//! ancestor transforms cheaply (see `frust_scene::SceneBuilder::draw_glyph_run`).

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use kurbo::{Affine, Point};
use peniko::Brush;

use frust_scene::{FontHandle, Glyph, GlyphRun};

/// Walks every line's glyph runs, mapping each into a scene glyph run whose
/// transform translates it to `origin`.
///
/// Inline boxes (not used in v1) are skipped. An empty or run-less layout
/// yields an empty vector.
pub(crate) fn layout_to_scene_runs(layout: &parley::Layout<Brush>, origin: Point) -> Vec<GlyphRun> {
    let transform = Affine::translate((origin.x, origin.y));
    let mut runs = Vec::new();
    let mut notdef = NotdefScan::default();

    for line in layout.lines() {
        for item in line.items() {
            let parley::layout::PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                // InlineBox — not emitted in v1.
                continue;
            };

            let run = glyph_run.run();
            notdef.scan_run(run);
            let font = FontHandle::new(run.font().clone());
            let font_size = run.font_size();
            // Per-run brush resolved from the pushed `StyleProperty::Brush`.
            let brush = glyph_run.style().brush.clone();

            let glyphs: Vec<Glyph> = glyph_run
                .positioned_glyphs()
                .map(|g| Glyph {
                    id: g.id,
                    x: g.x,
                    y: g.y,
                })
                .collect();

            runs.push(GlyphRun {
                font,
                font_size,
                brush,
                transform,
                glyphs,
            });
        }
    }

    notdef.report();
    runs
}

static NOTDEF_TOTAL: AtomicU64 = AtomicU64::new(0);
static NOTDEF_WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// Process-wide count of visible-text glyphs that shaped to glyph 0
/// (`.notdef`) in layouts walked into scene runs. Each memoized layout walk
/// adds once; repaint of a retained layout adds nothing.
pub fn notdef_glyph_count() -> u64 {
    NOTDEF_TOTAL.load(Ordering::Relaxed)
}

/// Per-walk accumulator of `.notdef` glyphs on visible clusters.
#[derive(Default)]
struct NotdefScan {
    /// Font identity -> (glyph count, up to 8 distinct affected characters).
    hits: Vec<(String, u64, Vec<char>)>,
}

impl NotdefScan {
    fn scan_run(&mut self, run: &parley::layout::Run<'_, Brush>) {
        let mut count = 0u64;
        let mut chars: Vec<char> = Vec::new();
        for cluster in run.clusters() {
            let ch = cluster.source_char();
            if ch.is_whitespace() || ch.is_control() || is_format_char(ch) {
                continue;
            }
            for g in cluster.glyphs() {
                if g.id == 0 {
                    count += 1;
                    if chars.len() < 8 && !chars.contains(&ch) {
                        chars.push(ch);
                    }
                }
            }
        }
        if count == 0 {
            return;
        }
        let name = font_identity(run.font());
        match self.hits.iter_mut().find(|h| h.0 == name) {
            Some(h) => {
                h.1 += count;
                for c in chars {
                    if h.2.len() < 8 && !h.2.contains(&c) {
                        h.2.push(c);
                    }
                }
            }
            None => self.hits.push((name, count, chars)),
        }
    }

    fn report(self) {
        for (name, count, chars) in self.hits {
            NOTDEF_TOTAL.fetch_add(count, Ordering::Relaxed);
            let first = NOTDEF_WARNED
                .get_or_init(Default::default)
                .lock()
                .map(|mut set| set.insert(name.clone()))
                .unwrap_or(false);
            if first {
                let shown: String = chars.iter().collect();
                log::warn!(
                    "font '{name}' has no glyph for {count} visible character(s) in this layout \
                     (drawn as .notdef); affected: '{shown}'. Register a font covering this script."
                );
            }
        }
    }
}

/// Zero-width and bidi/format characters that legitimately map to no glyph.
fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}'
        | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{E0100}'..='\u{E01EF}')
}

/// Best-effort human name for a font: its `name` table full name (ID 4) or
/// family (ID 1), else a size-based placeholder.
fn font_identity(font: &parley::FontData) -> String {
    let data = font.data.as_ref();
    name_table_string(data, font.index)
        .unwrap_or_else(|| format!("unnamed font ({} bytes)", data.len()))
}

/// Reads the full name (ID 4), else family (ID 1), from a font's `name` table.
///
/// Hand-rolled over untrusted bytes, so every offset and length is combined
/// with checked arithmetic and every read goes through `slice::get`: any
/// overflow, out-of-range or malformed field yields `None`, never a panic.
/// `ttcf` collections select the face by `index` (out of range -> `None`).
///
/// Platform 0 and 3 records are UTF-16BE. Every other platform (notably
/// platform 1, Macintosh Roman) is decoded as its ASCII subset; bytes at or
/// above 0x80 become U+FFFD, which is enough for a log identity.
fn name_table_string(data: &[u8], index: u32) -> Option<String> {
    let u16_at = |o: usize| -> Option<usize> {
        let bytes = data.get(o..o.checked_add(2)?)?;
        Some(u16::from_be_bytes(bytes.try_into().ok()?) as usize)
    };
    let u32_at = |o: usize| -> Option<usize> {
        let bytes = data.get(o..o.checked_add(4)?)?;
        usize::try_from(u32::from_be_bytes(bytes.try_into().ok()?)).ok()
    };
    let mut base = 0usize;
    if data.get(0..4)? == b"ttcf" {
        if index as usize >= u32_at(8)? {
            return None;
        }
        let slot = 12usize.checked_add(4usize.checked_mul(index as usize)?)?;
        base = u32_at(slot)?;
    }
    let num_tables = u16_at(base.checked_add(4)?)?;
    let name_off = (0..num_tables).find_map(|i| {
        let rec = base.checked_add(12)?.checked_add(16usize.checked_mul(i)?)?;
        (data.get(rec..rec.checked_add(4)?)? == b"name").then(|| u32_at(rec.checked_add(8)?))?
    })?;
    let count = u16_at(name_off.checked_add(2)?)?;
    let strings = name_off.checked_add(u16_at(name_off.checked_add(4)?)?)?;
    for want in [4usize, 1] {
        for i in 0..count {
            let r = name_off
                .checked_add(6)?
                .checked_add(12usize.checked_mul(i)?)?;
            let (platform, id) = (u16_at(r)?, u16_at(r.checked_add(6)?)?);
            if id != want {
                continue;
            }
            let (len, off) = (u16_at(r.checked_add(8)?)?, u16_at(r.checked_add(10)?)?);
            let start = strings.checked_add(off)?;
            let raw = data.get(start..start.checked_add(len)?)?;
            let s = if platform == 0 || platform == 3 {
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes(*c))
                    .collect();
                String::from_utf16_lossy(&units)
            } else {
                raw.iter()
                    .map(|&b| if b < 0x80 { b as char } else { '\u{FFFD}' })
                    .collect()
            };
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `name` table from `(platform, name_id, raw bytes)` records.
    fn name_table(records: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
        let storage_off = 6 + 12 * records.len();
        let mut out = Vec::new();
        for v in [0u16, records.len() as u16, storage_off as u16] {
            out.extend(v.to_be_bytes());
        }
        let mut strings: Vec<u8> = Vec::new();
        for (platform, id, raw) in records {
            for v in [*platform, 0, 0, *id, raw.len() as u16, strings.len() as u16] {
                out.extend(v.to_be_bytes());
            }
            strings.extend(raw);
        }
        out.extend(strings);
        out
    }

    /// An sfnt starting at file offset `base`, with absolute table offsets.
    fn sfnt_at(base: usize, tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let dir_len = 12 + 16 * tables.len();
        let mut out = vec![0, 1, 0, 0];
        out.extend((tables.len() as u16).to_be_bytes());
        out.extend([0u8; 6]);
        let mut body: Vec<u8> = Vec::new();
        for (tag, table) in tables {
            out.extend(*tag);
            out.extend([0u8; 4]);
            out.extend(((base + dir_len + body.len()) as u32).to_be_bytes());
            out.extend((table.len() as u32).to_be_bytes());
            body.extend(table);
        }
        out.extend(body);
        out
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_be_bytes).collect()
    }

    fn font(records: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
        sfnt_at(0, &[(b"name", name_table(records))])
    }

    fn ttc(a: &[(u16, u16, Vec<u8>)], b: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
        let first = 20;
        let face_a = sfnt_at(first, &[(b"name", name_table(a))]);
        let second = first + face_a.len();
        let face_b = sfnt_at(second, &[(b"name", name_table(b))]);
        let mut out = b"ttcf".to_vec();
        out.extend([0, 1, 0, 0]);
        out.extend(2u32.to_be_bytes());
        out.extend((first as u32).to_be_bytes());
        out.extend((second as u32).to_be_bytes());
        out.extend(face_a);
        out.extend(face_b);
        out
    }

    #[test]
    fn plain_sfnt_prefers_full_name() {
        let data = font(&[(3, 1, utf16("Family")), (3, 4, utf16("Family Bold"))]);
        assert_eq!(name_table_string(&data, 0).as_deref(), Some("Family Bold"));
    }

    #[test]
    fn falls_back_to_family_without_full_name() {
        let data = font(&[(3, 2, utf16("Sub")), (0, 1, utf16("Only Family"))]);
        assert_eq!(name_table_string(&data, 0).as_deref(), Some("Only Family"));
    }

    #[test]
    fn mac_roman_record_decodes_ascii() {
        let data = font(&[(1, 4, b"Mac Name".to_vec())]);
        assert_eq!(name_table_string(&data, 0).as_deref(), Some("Mac Name"));
        let high = font(&[(1, 4, vec![b'A', 0x8A, b'B'])]);
        assert_eq!(name_table_string(&high, 0).as_deref(), Some("A\u{FFFD}B"));
    }

    #[test]
    fn ttc_selects_face_by_index() {
        let data = ttc(&[(3, 4, utf16("First"))], &[(3, 4, utf16("Second"))]);
        assert_eq!(name_table_string(&data, 0).as_deref(), Some("First"));
        assert_eq!(name_table_string(&data, 1).as_deref(), Some("Second"));
        assert_eq!(name_table_string(&data, 2), None);
        assert_eq!(name_table_string(&data, u32::MAX), None);
    }

    #[test]
    fn missing_name_table_is_none() {
        let data = sfnt_at(0, &[(b"head", vec![0; 8])]);
        assert_eq!(name_table_string(&data, 0), None);
    }

    #[test]
    fn truncation_at_every_length_never_panics() {
        let plain = font(&[(3, 1, utf16("Family")), (3, 4, utf16("Family Bold"))]);
        let coll = ttc(&[(3, 4, utf16("First"))], &[(1, 1, b"Second".to_vec())]);
        for data in [&plain, &coll] {
            for len in 0..=data.len() {
                for index in 0..3 {
                    let _ = name_table_string(&data[..len], index);
                }
            }
        }
        assert!(name_table_string(&plain, 0).is_some());
    }

    #[test]
    fn huge_offsets_are_none_not_panics() {
        // `name` directory entry pointing at u32::MAX.
        let mut data = font(&[(3, 4, utf16("X"))]);
        data[12 + 8..12 + 12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(name_table_string(&data, 0), None);

        // Storage offset and record length/offset near their maxima.
        let mut data = font(&[(3, 4, utf16("X"))]);
        let name_off = 12 + 16;
        data[name_off + 4..name_off + 6].copy_from_slice(&u16::MAX.to_be_bytes());
        assert_eq!(name_table_string(&data, 0), None);
        let mut data = font(&[(3, 4, utf16("X"))]);
        let rec = name_off + 6;
        data[rec + 8..rec + 12].copy_from_slice(&[0xFF; 4]);
        assert_eq!(name_table_string(&data, 0), None);

        // `ttcf` face offset at u32::MAX.
        let mut data = ttc(&[(3, 4, utf16("A"))], &[(3, 4, utf16("B"))]);
        data[12..16].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(name_table_string(&data, 0), None);
    }

    #[test]
    fn font_identity_falls_back_to_size_placeholder() {
        let junk = parley::FontData::new(parley::fontique::Blob::from(vec![1u8, 2, 3, 4, 5]), 0);
        assert_eq!(font_identity(&junk), "unnamed font (5 bytes)");
        let named = parley::FontData::new(
            parley::fontique::Blob::from(font(&[(3, 4, utf16("Real"))])),
            0,
        );
        assert_eq!(font_identity(&named), "Real");
    }
}
