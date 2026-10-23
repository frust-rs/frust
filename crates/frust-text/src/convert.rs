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
pub fn layout_to_scene_runs(layout: &parley::Layout<Brush>, origin: Point) -> Vec<GlyphRun> {
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

fn name_table_string(data: &[u8], index: u32) -> Option<String> {
    let u16_at = |o: usize| -> Option<usize> {
        Some(u16::from_be_bytes(data.get(o..o + 2)?.try_into().ok()?) as usize)
    };
    let u32_at = |o: usize| -> Option<usize> {
        Some(u32::from_be_bytes(data.get(o..o + 4)?.try_into().ok()?) as usize)
    };
    let mut base = 0;
    if data.get(0..4)? == b"ttcf" {
        base = u32_at(12 + 4 * index as usize)?;
    }
    let num_tables = u16_at(base + 4)?;
    let name_off = (0..num_tables).find_map(|i| {
        let rec = base + 12 + 16 * i;
        (data.get(rec..rec + 4)? == b"name").then(|| u32_at(rec + 8))?
    })?;
    let count = u16_at(name_off + 2)?;
    let strings = name_off + u16_at(name_off + 4)?;
    for want in [4usize, 1] {
        for i in 0..count {
            let r = name_off + 6 + 12 * i;
            let (platform, id) = (u16_at(r)?, u16_at(r + 6)?);
            if id != want {
                continue;
            }
            let (len, off) = (u16_at(r + 8)?, u16_at(r + 10)?);
            let raw = data.get(strings + off..strings + off + len)?;
            let s = if platform == 0 || platform == 3 {
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes(*c))
                    .collect();
                String::from_utf16_lossy(&units)
            } else {
                String::from_utf8_lossy(raw).into_owned()
            };
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}
