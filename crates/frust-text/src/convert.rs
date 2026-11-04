//! Line table -> `frust_scene` glyph-run conversion.
//!
//! This module owns the glyph-mapping contract `frust-engine`'s text lowering
//! (`text/`, built on `glifo`) consumes: [`lines_to_scene_runs`] walks a line
//! table over its [`Shaped`] paragraph — a [`crate::TextLayout`]'s and a
//! [`crate::TextEditor`]'s alike.
//!
//! # Coordinate convention
//!
//! Each emitted [`frust_scene::Glyph`] carries **run-local** coordinates that
//! are exactly what `glifo`'s own `Glyph` (`x`/`y`, each documented "offset in
//! run, relative to transform") expects:
//!
//! - `x` is the glyph's absolute horizontal position within the layout: the
//!   line's alignment offset plus the pen advance of every glyph (and every
//!   justified space's stretch) before it in visual order, plus the glyph's
//!   own offset from its pen position.
//! - `y` is the line's baseline plus the glyph's own (Y-down) offset, so
//!   glyphs on the same line share a `y` and successive lines get distinct,
//!   increasing `y` values.
//!
//! One [`GlyphRun`] is emitted per (line, shaped run) span, split further
//! where the span's characters change style (brush). The block-level
//! placement (`origin`, passed by the widget) is **not** baked into the glyph
//! coordinates; it lives on [`frust_scene::GlyphRun::transform`] as a
//! translation. The render backend applies that transform to the whole
//! run — the engine compiler composes it into the run's paint and its glyph
//! outlines alike (`frust-engine`'s `compile_glyph_run`). Keeping origin in the
//! transform (rather than added into every glyph) lets the scene builder compose
//! ancestor transforms cheaply (see `frust_scene::SceneBuilder::draw_glyph_run`).
//!
//! # `.notdef` scan
//!
//! The walk that fills a [`RunMemo`] counts the visible-character glyphs that
//! shaped to glyph 0 into the owning context's [`NotdefCounter`]
//! (`TextContext::notdef_glyph_count`) and warns once per (face, script); a
//! paint served from the memo scans nothing. The warning names the face
//! identity, the script and the affected cluster count, never the text.

use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use kurbo::{Affine, Point};
use peniko::{Brush, FontData};

use frust_scene::{FontHandle, Glyph, GlyphRun};

use crate::layout::{ELLIPSIS, LineTable, Paragraph, Source};
use crate::shape::{Shaped, ShapedFace, faux_bold_em_fraction};

/// Walks `table`'s lines over `paragraph` (whose text is `text`), emitting
/// glyph runs at the layout origin (an identity transform) and adding the
/// walk's `.notdef` glyphs to `notdef`.
///
/// Called only to fill a [`RunMemo`], so each memoized walk scans once.
pub(crate) fn lines_to_scene_runs(
    paragraph: &Paragraph<'_>,
    text: &str,
    table: &LineTable,
    notdef: &NotdefCounter,
) -> Vec<GlyphRun> {
    let transform = Affine::IDENTITY;
    let mut ellipsis_text = [0u8; 4];
    let ellipsis_text: &str = ELLIPSIS.encode_utf8(&mut ellipsis_text);
    let mut runs = Vec::new();
    let mut scan = NotdefScan::default();

    for line in &table.lines {
        let baseline = line.metrics.baseline;
        let mut pen = line.metrics.offset;
        for item in &table.items[line.items.clone()] {
            let shaped: &Shaped = paragraph.source(item.source);
            let source_text = match item.source {
                Source::Text => text,
                Source::Ellipsis => ellipsis_text,
            };
            let run = &shaped.runs[item.run];
            let face = &shaped.faces[run.face];
            let data = FontData::new(face.blob.clone(), face.index);
            // The variation instance the shaper chose (a requested weight on
            // a variable face); all-zero coords name the default and are dropped.
            // The face identity the oracle resolved (PostScript name, hidden
            // flag), so the renderer opens the face the oracle chose rather
            // than re-deriving who it is from the blob.
            let font = font_handle(data, &run.coords, face, run.font_size);
            let mut current: Option<(u16, Vec<Glyph>)> = None;
            let flush = |current: &mut Option<(u16, Vec<Glyph>)>, runs: &mut Vec<GlyphRun>| {
                let Some((style, glyphs)) = current.take() else {
                    return;
                };
                if glyphs.is_empty() {
                    return;
                }
                let style = paragraph.style(item.source, style);
                runs.push(GlyphRun {
                    font: font.clone(),
                    font_size: run.font_size,
                    brush: Brush::Solid(style.color),
                    transform,
                    glyphs,
                });
            };
            let clusters = item.clusters.clone();
            let visual: Box<dyn Iterator<Item = usize>> = if item.bidi_level % 2 == 1 {
                Box::new(clusters.rev())
            } else {
                Box::new(clusters)
            };
            for ci in visual {
                let cluster = &shaped.clusters[ci];
                let first = shaped.chars.get(cluster.chars.start as usize);
                let style = first.map_or(0, |c| c.style_index);
                if current.as_ref().is_some_and(|(s, _)| *s != style) {
                    flush(&mut current, &mut runs);
                }
                let glyphs = &mut current.get_or_insert_with(|| (style, Vec::new())).1;
                let ids =
                    &shaped.glyphs[cluster.glyphs.start as usize..cluster.glyphs.end as usize];
                let mut missing = 0u64;
                for g in ids {
                    glyphs.push(Glyph {
                        id: g.id,
                        x: pen + g.x,
                        y: baseline + g.y,
                    });
                    pen += g.advance;
                    missing += u64::from(g.id == 0);
                }
                if ids.is_empty() {
                    pen += cluster.advance;
                }
                if item.source == Source::Text && line.stretched.binary_search(&(ci as u32)).is_ok()
                {
                    pen += line.justify;
                }
                if missing > 0 {
                    // Only a cluster holding glyph 0 decodes its first char,
                    // and only to classify it: nothing of the text is kept.
                    let visible = first
                        .and_then(|c| source_text.get(c.byte_start as usize..))
                        .and_then(|rest| rest.chars().next())
                        .is_some_and(is_visible);
                    if visible {
                        scan.record(item.source, run.face, run.script, missing);
                    }
                }
            }
            flush(&mut current, &mut runs);
        }
    }

    scan.report(paragraph, notdef);
    runs
}

/// The process-wide total behind the deprecated `crate::notdef_glyph_count`:
/// every [`NotdefCounter::add`] adds here too.
static NOTDEF_TOTAL: AtomicU64 = AtomicU64::new(0);
/// A warn-once key: the face and the run's ISO 15924 script tag.
type WarnKey = (FaceIdentity, [u8; 4]);
/// The (face, script) pairs already warned about, process-wide.
static NOTDEF_WARNED: OnceLock<Mutex<HashSet<WarnKey>>> = OnceLock::new();

/// The handle a run draws with: the font, the variation instance the shaper
/// chose (all-zero coords name the default and are dropped), the oracle's face
/// identity (PostScript name, hidden flag) so the renderer opens the face the
/// oracle chose, and the synthesis the oracle decided (faux-bold stroke as an
/// em fraction at `font_size`, faux-oblique degrees; zero when the face
/// satisfies the request itself).
pub(crate) fn font_handle(
    data: FontData,
    coords: &[i16],
    face: &ShapedFace,
    font_size: f32,
) -> FontHandle {
    let embolden = if face.embolden {
        faux_bold_em_fraction(font_size)
    } else {
        0.0
    };
    FontHandle::new(data)
        .with_normalized_coords(coords)
        .with_identity(face.postscript_name.as_deref().map(Arc::from), face.hidden)
        .with_synthesis(embolden, face.skew.unwrap_or(0.0))
}

/// One context's count of visible-text glyphs that shaped to glyph 0
/// (`.notdef`), shared by every [`crate::TextLayout`] and
/// [`crate::TextEditor`] view the context builds.
///
/// Only the walk filling a [`RunMemo`] adds to it, so a memoized walk adds
/// once and a paint served from the memo adds nothing.
#[derive(Clone, Default)]
pub(crate) struct NotdefCounter(Arc<AtomicU64>);

impl NotdefCounter {
    /// The glyphs counted so far.
    pub(crate) fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    /// Adds `glyphs` here and to the process-wide total.
    fn add(&self, glyphs: u64) {
        self.0.fetch_add(glyphs, Ordering::Relaxed);
        NOTDEF_TOTAL.fetch_add(glyphs, Ordering::Relaxed);
    }
}

/// The process-wide sum of every [`NotdefCounter`], kept for the deprecated
/// `crate::notdef_glyph_count`.
pub(crate) fn process_notdef_total() -> u64 {
    NOTDEF_TOTAL.load(Ordering::Relaxed)
}

/// The memo of one line table's origin-independent glyph runs, and the
/// counter its walk adds to.
///
/// Clones share the memo: a [`crate::TextLayout`] served from a shape-cache
/// hit shares its entry's, so a layout rebuilt every paint from the cache (a
/// text field's placeholder) walks and scans once per line table, like a
/// retained one. A new line table takes [`Self::fresh`].
#[derive(Clone, Default)]
pub(crate) struct RunMemo {
    runs: Arc<OnceLock<Vec<GlyphRun>>>,
    notdef: NotdefCounter,
}

impl RunMemo {
    /// An empty memo whose walk adds to `notdef`.
    pub(crate) fn new(notdef: &NotdefCounter) -> Self {
        Self {
            runs: Arc::default(),
            notdef: notdef.clone(),
        }
    }

    /// An empty memo over the same counter, for a new line table.
    pub(crate) fn fresh(&self) -> Self {
        Self::new(&self.notdef)
    }

    /// The memoized runs translated to `origin`, filled by `walk` (handed the
    /// counter) on the first call.
    ///
    /// The only origin-dependent field of a [`GlyphRun`] is its `transform`,
    /// so a paint at any origin clones the base runs and re-translates them.
    pub(crate) fn placed(
        &self,
        origin: Point,
        walk: impl FnOnce(&NotdefCounter) -> Vec<GlyphRun>,
    ) -> Vec<GlyphRun> {
        let base = self.runs.get_or_init(|| walk(&self.notdef));
        let transform = Affine::translate((origin.x, origin.y));
        base.iter()
            .map(|run| GlyphRun {
                transform,
                ..run.clone()
            })
            .collect()
    }

    /// Whether the memo is filled.
    #[cfg(test)]
    pub(crate) fn is_filled(&self) -> bool {
        self.runs.get().is_some()
    }
}

/// What a `.notdef` warning names a face by: the PostScript name the oracle
/// resolved, else its blob (id, collection index, byte length). Never read
/// from the text being drawn.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum FaceIdentity {
    PostScript(String),
    Blob { id: u64, index: u32, len: usize },
}

/// The longest PostScript name a warning shows (the `name` table's own limit
/// is 63 characters; the bytes come from a font and are not trusted).
const MAX_SHOWN_NAME: usize = 63;

impl FaceIdentity {
    fn of(face: &ShapedFace) -> Self {
        match &face.postscript_name {
            Some(name) => Self::PostScript(name.chars().take(MAX_SHOWN_NAME).collect()),
            None => Self::Blob {
                id: face.blob.id(),
                index: face.index,
                len: face.blob.len(),
            },
        }
    }
}

impl fmt::Display for FaceIdentity {
    /// The name `Debug`-escaped (quotes, control characters), or the blob.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PostScript(name) => write!(f, "{name:?}"),
            Self::Blob { id, index, len } => {
                write!(f, "blob #{id} face {index} ({len} bytes)")
            }
        }
    }
}

/// One walk's `.notdef` hits on visible clusters, grouped by where the face
/// lives and the run's script; the face identity is read only at report time.
#[derive(Default)]
struct NotdefScan {
    hits: Vec<Hit>,
}

struct Hit {
    source: Source,
    /// Index into the source's [`Shaped::faces`].
    face: usize,
    /// ISO 15924 tag of the run's script.
    script: [u8; 4],
    clusters: u64,
    glyphs: u64,
}

impl NotdefScan {
    /// Records one visible cluster of a run (`source`'s face `face`, script
    /// `script`) holding `glyphs` `.notdef` glyphs.
    fn record(&mut self, source: Source, face: usize, script: [u8; 4], glyphs: u64) {
        let hit = self
            .hits
            .iter_mut()
            .find(|h| h.source == source && h.face == face && h.script == script);
        match hit {
            Some(h) => {
                h.clusters += 1;
                h.glyphs += glyphs;
            }
            None => self.hits.push(Hit {
                source,
                face,
                script,
                clusters: 1,
                glyphs,
            }),
        }
    }

    /// Adds the walk's glyphs to `notdef` and warns once per (face, script)
    /// per process.
    fn report(self, paragraph: &Paragraph<'_>, notdef: &NotdefCounter) {
        if self.hits.is_empty() {
            return;
        }
        let mut grouped: Vec<(FaceIdentity, [u8; 4], u64)> = Vec::new();
        let mut glyphs = 0;
        for hit in self.hits {
            glyphs += hit.glyphs;
            let face = FaceIdentity::of(&paragraph.source(hit.source).faces[hit.face]);
            match grouped
                .iter_mut()
                .find(|(f, s, _)| *f == face && *s == hit.script)
            {
                Some(group) => group.2 += hit.clusters,
                None => grouped.push((face, hit.script, hit.clusters)),
            }
        }
        notdef.add(glyphs);
        for (face, script, clusters) in grouped {
            let first = NOTDEF_WARNED
                .get_or_init(Default::default)
                .lock()
                .map(|mut set| set.insert((face.clone(), script)))
                .unwrap_or(false);
            if first {
                log::warn!("{}", notdef_warning(&face, script, clusters));
            }
        }
    }
}

/// The once-per-(face, script) warning: the cluster count, the face identity
/// and the script tag (escaped), and nothing of the text.
fn notdef_warning(face: &FaceIdentity, script: [u8; 4], clusters: u64) -> String {
    format!(
        "frust-text: font {face} drew {clusters} visible cluster(s) of script {} as .notdef \
         in one layout; register a font covering this script (warned once per font and script)",
        script.escape_ascii()
    )
}

/// Whether a cluster starting with `ch` should have drawn something: not
/// whitespace, a control, or a zero-width / bidi format character.
fn is_visible(ch: char) -> bool {
    !(ch.is_whitespace() || ch.is_control() || is_format_char(ch))
}

/// Zero-width and bidi/format characters that legitimately map to no glyph.
fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}'
        | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{E0100}'..='\u{E01EF}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use peniko::Blob;

    fn face(blob: &Blob<u8>, postscript_name: Option<&str>) -> ShapedFace {
        ShapedFace {
            blob: blob.clone(),
            index: 0,
            embolden: false,
            skew: None,
            postscript_name: postscript_name.map(str::to_owned),
            hidden: false,
            units_per_em: 1000,
        }
    }

    #[test]
    fn a_named_face_is_identified_by_its_postscript_name() {
        let blob = Blob::from(vec![0u8; 7]);
        let named = FaceIdentity::of(&face(&blob, Some("NotoSans-Regular")));
        assert_eq!(named, FaceIdentity::PostScript("NotoSans-Regular".into()));
        assert_eq!(named.to_string(), "\"NotoSans-Regular\"");
    }

    #[test]
    fn nameless_faces_of_equal_size_are_told_apart_by_blob() {
        // Two nameless faces of one byte length used to share the
        // "unnamed font (N bytes)" key, so the second never warned.
        let (a, b) = (Blob::from(vec![1u8; 9]), Blob::from(vec![2u8; 9]));
        let (fa, fb) = (
            FaceIdentity::of(&face(&a, None)),
            FaceIdentity::of(&face(&b, None)),
        );
        assert_ne!(fa, fb);
        assert_eq!(
            fa.to_string(),
            format!("blob #{} face 0 (9 bytes)", a.id()),
            "blob id, collection index and byte length"
        );
    }

    #[test]
    fn a_hostile_postscript_name_is_escaped_and_capped() {
        let blob = Blob::from(vec![0u8; 4]);
        let long = format!("Evil\n\u{1b}[31m\"{}", "x".repeat(200));
        let shown = FaceIdentity::of(&face(&blob, Some(&long))).to_string();
        assert!(
            !shown.contains('\n') && !shown.contains('\u{1b}'),
            "{shown}"
        );
        assert!(
            shown.contains("\\n") && shown.contains("\\u{1b}"),
            "{shown}"
        );
        assert!(shown.chars().count() <= MAX_SHOWN_NAME + 16, "{shown}");
    }

    #[test]
    fn the_warning_names_count_face_and_script_only() {
        let face = FaceIdentity::PostScript("NotoSans-Regular".into());
        let line = notdef_warning(&face, *b"Hani", 2);
        assert_eq!(
            line,
            "frust-text: font \"NotoSans-Regular\" drew 2 visible cluster(s) of script Hani as \
             .notdef in one layout; register a font covering this script (warned once per font \
             and script)"
        );
        // A script tag is font/engine data too: non-printable bytes are escaped.
        let odd = notdef_warning(&face, [b'H', 0, 0x1b, 0xff], 1);
        assert!(odd.contains("H\\x00\\x1b\\xff"), "{odd}");
    }

    #[test]
    fn whitespace_controls_and_format_chars_are_not_visible() {
        for ch in [
            ' ', '\u{3000}', '\n', '\t', '\u{200B}', '\u{200D}', '\u{FE0F}',
        ] {
            assert!(!is_visible(ch), "{ch:?}");
        }
        for ch in ['a', '中', '\u{0627}'] {
            assert!(is_visible(ch), "{ch:?}");
        }
    }

    #[test]
    fn a_memo_walks_once_and_its_clones_share_it() {
        let counter = NotdefCounter::default();
        let memo = RunMemo::new(&counter);
        let shared = memo.clone();
        let walks = std::cell::Cell::new(0);
        let walk = |notdef: &NotdefCounter| {
            walks.set(walks.get() + 1);
            notdef.add(3);
            Vec::new()
        };
        let _ = memo.placed(Point::ORIGIN, walk);
        let _ = shared.placed(Point::new(5.0, 5.0), walk);
        assert_eq!(walks.get(), 1, "a clone reads the filled memo");
        assert_eq!(counter.get(), 3);
        assert!(shared.is_filled());

        let fresh = memo.fresh();
        assert!(!fresh.is_filled(), "a new line table starts empty");
        let _ = fresh.placed(Point::ORIGIN, walk);
        assert_eq!(walks.get(), 2);
        assert_eq!(counter.get(), 6, "the fresh memo adds to the same counter");
    }

    #[test]
    fn counters_are_per_context_and_sum_process_wide() {
        let (a, b) = (NotdefCounter::default(), NotdefCounter::default());
        let before = process_notdef_total();
        a.add(2);
        b.add(5);
        assert_eq!((a.get(), b.get()), (2, 5));
        // Other tests add concurrently: the total only grows by at least ours.
        assert!(process_notdef_total() - before >= 7);
    }
}
