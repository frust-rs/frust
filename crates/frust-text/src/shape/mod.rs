//! The shaping driver: text + [`TextStyle`] spans in, a frust-owned [`Shaped`]
//! out, over parley_engine's `Analyzer`/`Shaper` with fonts chosen by the font
//! oracle (through the `selector` adapter).
//!
//! **Boundary (D14):** the only module in the crate naming parley_engine or
//! harfrust types. Layout, editing, glyph conversion and the shape cache
//! consume [`Shaped`]; a shaper swap replaces this module alone (guard test
//! below).
//!
//! [`ShapeContext::shape_spans`] itemizes the spans (`items`), analyzes the
//! text (base direction auto, per-span line-break language), shapes each item
//! and applies letter/word spacing per cluster where parley 0.11's
//! `LayoutData::finish` did. The engine takes variation coordinates per item,
//! not per face (its synthesis type has no public constructor), so a face
//! whose synthesis changes it (a variable face off its default weight) gets
//! them in a second pass: items split at the first pass's face changes, each
//! carrying its face's coordinates ahead of the style's `variations`, with the
//! font decisions replayed. Static faces and default instances take one pass.
//!
//! parley_engine 0.12.0 never marks a cluster safe to break before and has no
//! inline objects, so [`AtomFlags::safe_to_break_before`] and
//! [`AtomFlags::inline_object`] are always `false`: a line breaker breaks at
//! soft-wrap atoms without reshaping. An empty text shapes to an empty
//! [`Shaped`].

mod items;
mod selector;

use std::ops::Range;

use fontique::Language;
use parlance::{FontVariation, Tag};
use parley_engine::itemize::Item;
use parley_engine::shape::Whitespace as EngineWhitespace;
use parley_engine::{
    Analysis, AnalysisDataSources, AnalysisOptions, Analyzer, BaseDirection, LineBreakConfig,
    ShapeOptions, ShapedText, Shaper,
};
use peniko::Blob;

use crate::oracle::Oracle;
use crate::style::TextStyle;
use items::{Itemized, itemize};
use selector::{Adapter, SelectorState, StyleQuery};

/// Faux-bold stroke, as a fraction of the em, for a face the oracle decided to
/// embolden at `font_size` logical pixels.
///
/// Reference is Skia's fake bold: about 1/24 em at sizes up to 9 px, sliding
/// linearly to 1/32 em at 36 px and above, so small text gets proportionally
/// more weight to stay legible and large text does not look blobby. The
/// interpolation is kept (rather than one constant) so the strength matches
/// what Android and Flutter users see from synthetic bold; the renderer reads
/// the resulting em fraction off the handle and needs no size rule of its own.
pub(crate) fn faux_bold_em_fraction(font_size: f32) -> f32 {
    const SMALL: f32 = 1.0 / 24.0;
    const LARGE: f32 = 1.0 / 32.0;
    let t = ((font_size - 9.0) / (36.0 - 9.0)).clamp(0.0, 1.0);
    SMALL + (LARGE - SMALL) * if t.is_nan() { 0.0 } else { t }
}

/// A face a [`ShapedRun`] draws with, as the oracle chose it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ShapedFace {
    pub(crate) blob: Blob<u8>,
    pub(crate) index: u32,
    /// Faux bold: the face is lighter than requested and has no `wght` axis.
    pub(crate) embolden: bool,
    /// Faux oblique angle in degrees.
    pub(crate) skew: Option<f32>,
    pub(crate) postscript_name: Option<String>,
    /// A system-private face (`.SF NS`), passed through as the oracle flagged it.
    pub(crate) hidden: bool,
    /// `head.unitsPerEm`, `0` when the face has no readable `head`.
    pub(crate) units_per_em: u16,
}

/// A run's font metrics at its size and coordinates, in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RunMetrics {
    /// Baseline to the top of the alignment box.
    pub(crate) ascent: f32,
    /// Baseline to the bottom of the alignment box (positive).
    pub(crate) descent: f32,
    pub(crate) leading: f32,
    pub(crate) units_per_em: u16,
    pub(crate) underline_offset: f32,
    pub(crate) underline_size: f32,
    pub(crate) strikeout_offset: f32,
    pub(crate) strikeout_size: f32,
}

/// A maximal span shaped with one face, size, bidi level and script.
///
/// Every range indexes the matching [`Shaped`] list; runs are in logical order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ShapedRun {
    /// Index into [`Shaped::faces`].
    pub(crate) face: usize,
    /// Normalized variation coordinates (F2Dot14 bits) the run was shaped at;
    /// empty at the face's default instance.
    pub(crate) coords: Vec<i16>,
    pub(crate) font_size: f32,
    pub(crate) bidi_level: u8,
    /// ISO 15924 tag of the run's resolved script.
    pub(crate) script: [u8; 4],
    /// BCP 47 language the run was shaped with.
    pub(crate) language: Option<String>,
    /// The span (style) index of the run's first character.
    pub(crate) style_index: u16,
    pub(crate) metrics: RunMetrics,
    pub(crate) chars: Range<u32>,
    pub(crate) bytes: Range<usize>,
    pub(crate) clusters: Range<u32>,
    pub(crate) atoms: Range<u32>,
    pub(crate) graphemes: Range<u32>,
    pub(crate) glyphs: Range<u32>,
    /// Sum of the run's cluster advances, spacing included.
    pub(crate) advance: f32,
}

/// A character's whitespace class (CSS Text 4 sets).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Whitespace {
    #[default]
    None,
    Space,
    NoBreakSpace,
    IdeographicSpace,
    /// Every other `Zs` space separator.
    OtherSpace,
    Tab,
    /// CR, LF, CRLF, LS or PS: a mandatory break follows.
    Newline,
    /// Vertical tab, form feed, next line: drawn as control glyphs.
    Control,
}

impl Whitespace {
    fn of(ws: EngineWhitespace) -> Self {
        match ws {
            EngineWhitespace::None => Self::None,
            EngineWhitespace::Space => Self::Space,
            EngineWhitespace::NoBreakSpace => Self::NoBreakSpace,
            EngineWhitespace::IdeographicSpace => Self::IdeographicSpace,
            EngineWhitespace::OtherSpaceSeparator => Self::OtherSpace,
            EngineWhitespace::Tab => Self::Tab,
            EngineWhitespace::Newline => Self::Newline,
            EngineWhitespace::ControlWhitespace => Self::Control,
        }
    }

    /// A standard or no-break space: where word spacing applies.
    pub(crate) fn is_space_or_nbsp(self) -> bool {
        matches!(self, Self::Space | Self::NoBreakSpace)
    }
}

/// Per-character analysis.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ShapedChar {
    pub(crate) byte_start: u32,
    pub(crate) style_index: u16,
    pub(crate) whitespace: Whitespace,
    /// Starts a grapheme (always true at a run's first character).
    pub(crate) grapheme_start: bool,
    pub(crate) word_boundary: bool,
    /// The character has the `Emoji` or `Extended_Pictographic` property
    /// (UTS #51): what makes the editor's backspace take a whole grapheme.
    pub(crate) emoji: bool,
    /// A soft wrap opportunity precedes this character.
    pub(crate) soft_wrap_opportunity: bool,
    pub(crate) bidi_level: u8,
}

/// A shaped cluster: characters shaped together into one or more glyphs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ShapedCluster {
    pub(crate) chars: Range<u32>,
    /// Index range into [`Shaped::glyphs`]; empty for a newline.
    pub(crate) glyphs: Range<u32>,
    /// Advance, letter and word spacing included.
    pub(crate) advance: f32,
    pub(crate) whitespace: Whitespace,
}

/// How a line breaker may treat an atom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AtomFlags {
    pub(crate) soft_wrap_opportunity_before: bool,
    /// Always `false` with parley_engine 0.12.0 (see the module docs).
    pub(crate) safe_to_break_before: bool,
    /// The atom ends in a newline.
    pub(crate) mandatory_break_after: bool,
    /// Always `false`: no inline objects yet.
    pub(crate) inline_object: bool,
}

/// The smallest span bounded by both a grapheme and a cluster boundary: a
/// line can break between atoms, never inside one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ShapedAtom {
    pub(crate) chars: Range<u32>,
    pub(crate) clusters: Range<u32>,
    pub(crate) advance: f32,
    pub(crate) flags: AtomFlags,
    /// The first character's whitespace class.
    pub(crate) whitespace: Whitespace,
}

/// A grapheme with its share of the clusters it overlaps (a cluster's advance
/// is split evenly over the graphemes it spans).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ShapedGrapheme {
    pub(crate) chars: Range<u32>,
    pub(crate) advance: f32,
    pub(crate) whitespace: Whitespace,
}

/// A positioned glyph, relative to its cluster's pen position; `y` is Y-down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShapedGlyph {
    pub(crate) id: u32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) advance: f32,
    /// Index into [`Shaped::clusters`].
    pub(crate) cluster: u32,
}

/// A paragraph shaped once, width-independently: everything a line breaker,
/// glyph emitter or caret walk needs, with no engine type in it.
///
/// Clusters, atoms and graphemes are in logical order; within an RTL run a
/// cluster's glyphs are in paint (left-to-right) order.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Shaped {
    pub(crate) text_len: usize,
    /// The paragraph's resolved base level (odd = RTL).
    pub(crate) paragraph_level: u8,
    pub(crate) faces: Vec<ShapedFace>,
    pub(crate) runs: Vec<ShapedRun>,
    pub(crate) chars: Vec<ShapedChar>,
    pub(crate) clusters: Vec<ShapedCluster>,
    pub(crate) atoms: Vec<ShapedAtom>,
    pub(crate) graphemes: Vec<ShapedGrapheme>,
    pub(crate) glyphs: Vec<ShapedGlyph>,
    /// `false` when no font at all was available for some cluster: shaping
    /// stopped there and the runs cover only a prefix of the text.
    pub(crate) complete: bool,
}

/// One item of a shaping pass: where it ends, which [`Itemized`] item it
/// belongs to, and the variation settings it is shaped with.
struct PassItem {
    char_end: u32,
    item: usize,
    variations: Vec<([u8; 4], f32)>,
}

/// The reusable shaping state a [`crate::TextContext`] owns: the engine's
/// analyzer and shaper (with their plan/instance LRUs), scratch buffers, and
/// the oracle adapter's state.
pub(crate) struct ShapeContext {
    analyzer: Analyzer,
    analysis: Analysis,
    shaper: Shaper,
    shaped: ShapedText,
    sources: AnalysisDataSources,
    selector: SelectorState,
    /// Analysis runs performed, for the shape cache's skip-on-hit tests.
    analyses: u64,
    /// Engine shaping passes performed.
    passes: u64,
}

impl Default for ShapeContext {
    fn default() -> Self {
        Self {
            analyzer: Analyzer::new(),
            analysis: Analysis::new(),
            shaper: Shaper::default(),
            shaped: ShapedText::new(),
            sources: AnalysisDataSources::new(),
            selector: SelectorState::default(),
            analyses: 0,
            passes: 0,
        }
    }
}

impl std::fmt::Debug for ShapeContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShapeContext")
            .field("analyses", &self.analyses)
            .field("passes", &self.passes)
            .finish_non_exhaustive()
    }
}

impl ShapeContext {
    /// Shapes `text` styled uniformly by `style`.
    pub(crate) fn shape(&mut self, oracle: &mut Oracle, text: &str, style: &TextStyle) -> Shaped {
        self.shape_spans(oracle, text, &[(0..text.len(), style)])
    }

    /// Shapes `text` styled by `spans` (byte ranges tiling it, see
    /// `items::itemize`); a span's position is the style index runs and
    /// characters report.
    pub(crate) fn shape_spans(
        &mut self,
        oracle: &mut Oracle,
        text: &str,
        spans: &[(Range<usize>, &TextStyle)],
    ) -> Shaped {
        let itemized = itemize(text, spans);
        if itemized.items.is_empty() {
            return Shaped {
                text_len: text.len(),
                complete: true,
                ..Shaped::default()
            };
        }
        let line_break: Vec<(Range<usize>, LineBreakConfig)> = spans
            .iter()
            .filter_map(|(range, style)| {
                let language = Language::parse(style.locale.as_deref()?).ok()?;
                let config = LineBreakConfig {
                    language: Some(language),
                    ..LineBreakConfig::default()
                };
                Some((range.clone(), config))
            })
            .collect();
        let options = AnalysisOptions {
            base_direction: BaseDirection::Auto,
            line_break: &line_break,
            ..AnalysisOptions::default()
        };
        self.analyzer.analyze(text, &options, &mut self.analysis);
        self.analyses += 1;

        let queries: Vec<StyleQuery> = spans.iter().map(|(_, s)| StyleQuery::of(s)).collect();
        let pass: Vec<PassItem> = itemized
            .items
            .iter()
            .enumerate()
            .map(|(item, spec)| PassItem {
                char_end: spec.char_end,
                item,
                variations: spec.variations.clone(),
            })
            .collect();
        self.selector.begin_shape();
        self.run_pass(oracle, text, &itemized, &queries, &pass);
        let char_count = self.analysis.char_info().len();
        if self.shaped.characters().len() == char_count && self.selector.any_coords() {
            let pass = self.coordinate_items(&itemized);
            self.selector.begin_replay();
            self.run_pass(oracle, text, &itemized, &queries, &pass);
        }
        let complete = self.shaped.characters().len() == char_count;
        self.convert(text, &itemized, complete)
    }

    /// Analysis runs performed so far (cache tests only).
    #[cfg(test)]
    pub(crate) fn analyses(&self) -> u64 {
        self.analyses
    }

    /// Engine shaping passes performed so far (cache tests only).
    #[cfg(test)]
    pub(crate) fn passes(&self) -> u64 {
        self.passes
    }

    fn run_pass(
        &mut self,
        oracle: &mut Oracle,
        text: &str,
        itemized: &Itemized,
        queries: &[StyleQuery],
        pass: &[PassItem],
    ) {
        let variations: Vec<Vec<FontVariation>> = pass
            .iter()
            .map(|p| {
                p.variations
                    .iter()
                    .map(|&(tag, value)| FontVariation::new(Tag::new(&tag), value))
                    .collect()
            })
            .collect();
        let items = pass.iter().zip(&variations).map(|(p, variations)| {
            let spec = &itemized.items[p.item];
            Item {
                char_end: p.char_end,
                options: ShapeOptions {
                    font_size: spec.font_size,
                    language: spec.locale.as_deref().and_then(|l| Language::parse(l).ok()),
                    features: &[],
                    variations,
                },
            }
        });
        let adapter = Adapter {
            oracle,
            state: &mut self.selector,
            styles: queries,
            sources: &self.sources,
        };
        self.shaper.shape_text(
            text,
            &self.analysis,
            &itemized.char_style_indices,
            items,
            adapter,
            &mut self.shaped,
        );
        self.passes += 1;
    }

    /// The second pass's items: the first pass's runs grouped by item and by
    /// their face's coordinates, each carrying those ahead of the item's own
    /// variation settings.
    fn coordinate_items(&self, itemized: &Itemized) -> Vec<PassItem> {
        let mut out: Vec<PassItem> = Vec::new();
        let faces = self.selector.faces();
        for run in self.shaped.runs() {
            let start = run.range.char_range.start;
            let item = itemized.item_at(u32::try_from(start).unwrap_or(u32::MAX));
            let coords = self
                .selector
                .face_at(run.range.byte_range.start)
                .map_or(&[][..], |f| faces[f].coords.as_slice());
            let mut variations = coords.to_vec();
            variations.extend_from_slice(&itemized.items[item].variations);
            let char_end = u32::try_from(run.range.char_range.end).unwrap_or(u32::MAX);
            match out.last_mut() {
                Some(last) if last.item == item && last.variations == variations => {
                    last.char_end = char_end;
                }
                _ => out.push(PassItem {
                    char_end,
                    item,
                    variations,
                }),
            }
        }
        out
    }

    fn convert(&self, text: &str, itemized: &Itemized, complete: bool) -> Shaped {
        let shaped = &self.shaped;
        let paragraph_level = self.analysis.paragraph_level().to_u8();
        let levels = self.analysis.bidi_levels();
        let infos = self.analysis.char_info();
        let mut out = Shaped {
            text_len: text.len(),
            paragraph_level,
            complete,
            ..Shaped::default()
        };
        out.faces = self
            .selector
            .faces()
            .iter()
            .map(|s| ShapedFace {
                blob: s.face.blob.clone(),
                index: s.face.index,
                embolden: s.face.synthesis.embolden,
                skew: s.face.synthesis.skew,
                postscript_name: s.face.postscript_name.clone(),
                hidden: s.face.hidden,
                units_per_em: s.units_per_em,
            })
            .collect();
        out.chars = shaped
            .characters()
            .iter()
            .enumerate()
            .map(|(i, c)| ShapedChar {
                byte_start: c.text_byte_start,
                style_index: c.style_index,
                whitespace: Whitespace::of(c.whitespace),
                grapheme_start: c.grapheme_start,
                word_boundary: c.flags.is_word_boundary(),
                emoji: infos
                    .get(i)
                    .is_some_and(|info| info.is_emoji_or_pictograph()),
                soft_wrap_opportunity: c.flags.is_soft_wrap_opportunity(),
                bidi_level: levels.get(i).map_or(paragraph_level, |l| l.to_u8()),
            })
            .collect();
        for (run_index, run) in shaped.runs().iter().enumerate() {
            let slice = shaped.run_slice(u32::try_from(run_index).unwrap_or(u32::MAX));
            let face = self
                .selector
                .face_at(run.range.byte_range.start)
                .unwrap_or(0);
            let style_index = out
                .chars
                .get(run.characters_range.start as usize)
                .map_or(0, |c| c.style_index);
            // Runs never cross items, and an item's spacing is constant.
            let item = &itemized.items[itemized.item_at(run.characters_range.start)];
            let (letter, word) = (item.letter_spacing, item.word_spacing);

            let clusters_start = out.clusters.len();
            let glyphs_start = out.glyphs.len();
            for ci in run.shaped_clusters_range.clone() {
                let cluster = shaped.shaped_clusters()[ci as usize];
                let index = u32::try_from(out.clusters.len()).unwrap_or(u32::MAX);
                let first = out.glyphs.len();
                out.glyphs
                    .extend(slice.shaped_cluster_glyphs(ci).map(|g| ShapedGlyph {
                        id: g.id,
                        x: g.x,
                        y: g.y,
                        advance: g.advance,
                        cluster: index,
                    }));
                let whitespace = Whitespace::of(cluster.whitespace());
                let mut advance = cluster.advance;
                let mut spacing = letter;
                if word != 0.0 && whitespace.is_space_or_nbsp() {
                    spacing += word;
                }
                if spacing != 0.0 {
                    advance += spacing;
                    if let Some(last) = out.glyphs[first..].last_mut() {
                        last.advance += spacing;
                    }
                }
                out.clusters.push(ShapedCluster {
                    chars: cluster.chars_range(),
                    glyphs: to_u32(first)..to_u32(out.glyphs.len()),
                    advance,
                    whitespace,
                });
            }

            let atoms_start = out.atoms.len();
            for atom in slice.atoms_start() {
                let clusters = atom.shaped_clusters_range();
                let chars = atom.char_range();
                let advance = out.clusters[clusters.start as usize..clusters.end as usize]
                    .iter()
                    .map(|c| c.advance)
                    .sum();
                let first = out.chars[chars.start as usize].whitespace;
                let last = out.chars[chars.end as usize - 1].whitespace;
                out.atoms.push(ShapedAtom {
                    chars,
                    clusters,
                    advance,
                    flags: AtomFlags {
                        soft_wrap_opportunity_before: atom.is_soft_wrap_opportunity_before(),
                        safe_to_break_before: atom.is_safe_to_break_before(),
                        mandatory_break_after: last == Whitespace::Newline,
                        inline_object: false,
                    },
                    whitespace: first,
                });
            }

            let graphemes_start = out.graphemes.len();
            push_graphemes(
                &mut out.graphemes,
                &out.chars,
                &out.clusters[clusters_start..],
                run.characters_range.clone(),
            );

            let segment = self.selector.segment_at(run.range.char_range.start);
            let selected = self.selector.faces().get(face);
            let m = run.font_metrics;
            let mut coords: Vec<i16> = shaped.normalized_coords()
                [run.normalized_coords_range.clone()]
            .iter()
            .map(|c| c.to_bits())
            .collect();
            if coords.iter().all(|&c| c == 0) {
                coords.clear();
            }
            out.runs.push(ShapedRun {
                face,
                coords,
                font_size: run.font_size,
                bidi_level: run.bidi_level.to_u8(),
                script: segment.map_or(*b"Zyyy", |s| s.script),
                language: segment.and_then(|s| s.language.clone()),
                style_index,
                metrics: RunMetrics {
                    ascent: m.ascent,
                    descent: m.descent,
                    leading: m.leading,
                    units_per_em: selected.map_or(0, |s| s.units_per_em),
                    underline_offset: m.underline_offset,
                    underline_size: m.underline_size,
                    strikeout_offset: m.strikethrough_offset,
                    strikeout_size: m.strikethrough_size,
                },
                chars: run.characters_range.clone(),
                bytes: run.range.byte_range.clone(),
                clusters: to_u32(clusters_start)..to_u32(out.clusters.len()),
                atoms: to_u32(atoms_start)..to_u32(out.atoms.len()),
                graphemes: to_u32(graphemes_start)..to_u32(out.graphemes.len()),
                glyphs: to_u32(glyphs_start)..to_u32(out.glyphs.len()),
                advance: out.clusters[clusters_start..]
                    .iter()
                    .map(|c| c.advance)
                    .sum(),
            });
        }
        out
    }
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Appends the graphemes of the run spanning `chars`, each taking an even
/// share of every cluster it overlaps.
fn push_graphemes(
    out: &mut Vec<ShapedGrapheme>,
    all_chars: &[ShapedChar],
    clusters: &[ShapedCluster],
    chars: Range<u32>,
) {
    let first = out.len();
    let mut start = chars.start;
    for i in chars.start + 1..=chars.end {
        if i == chars.end || all_chars[i as usize].grapheme_start {
            out.push(ShapedGrapheme {
                chars: start..i,
                advance: 0.0,
                whitespace: all_chars[start as usize].whitespace,
            });
            start = i;
        }
    }
    let graphemes = &mut out[first..];
    let mut g = 0;
    for cluster in clusters {
        while g < graphemes.len() && graphemes[g].chars.end <= cluster.chars.start {
            g += 1;
        }
        let overlapping = graphemes[g..]
            .iter()
            .take_while(|gr| gr.chars.start < cluster.chars.end)
            .count()
            .max(1);
        let share = cluster.advance / overlapping as f32;
        for gr in graphemes[g..].iter_mut().take(overlapping) {
            gr.advance += share;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::synthetic::{FaceSpec, SyntheticBackend};
    use crate::oracle::{FaceSource, RenderCapabilities};
    use crate::style::{FontFamily, FontWeight, GenericSlot};
    use peniko::Color;
    use read_fonts::{FontRef, TableProvider};

    const NOTO_SANS: &[u8] = include_bytes!("../../../../testing/fonts/NotoSans-Subset.ttf");
    const GEIST_VARIABLE: &[u8] =
        include_bytes!("../../../../plugins/beui/fonts/geist/Geist-Variable.ttf");

    fn oracle_over(backend: SyntheticBackend) -> Oracle {
        Oracle::new(Box::new(backend), RenderCapabilities::default(), None)
    }

    /// An oracle whose only system font is `bytes`, the default for every slot.
    fn oracle_with(bytes: &[u8]) -> (Oracle, Blob<u8>) {
        let blob = Blob::from(bytes.to_vec());
        let mut backend = SyntheticBackend::new("shape-tests");
        for slot in [GenericSlot::SystemUi, GenericSlot::SansSerif] {
            backend.default_for(slot, &blob);
        }
        let all: String = (' '..='\u{FFFF}').collect();
        backend.cover(&blob, &all);
        (oracle_over(backend), blob)
    }

    fn style(size: f32) -> TextStyle {
        TextStyle::new(size, Color::BLACK)
    }

    fn cmap_glyphs(bytes: &[u8], text: &str) -> Vec<u32> {
        let font = FontRef::new(bytes).expect("font parses");
        let cmap = font.cmap().expect("cmap");
        text.chars()
            .map(|c| cmap.map_codepoint(c).map_or(0, |g| g.to_u32()))
            .collect()
    }

    fn assert_tiles(shaped: &Shaped, text: &str) {
        assert!(shaped.complete);
        let mut char_end = 0;
        let mut byte_end = 0;
        for run in &shaped.runs {
            assert_eq!(run.chars.start, char_end, "runs tile the characters");
            assert_eq!(run.bytes.start, byte_end, "runs tile the bytes");
            char_end = run.chars.end;
            byte_end = run.bytes.end;
        }
        assert_eq!(char_end as usize, text.chars().count());
        assert_eq!(byte_end, text.len());
    }

    #[test]
    fn a_latin_paragraph_shapes_to_the_expected_glyph_ids() {
        let (mut oracle, blob) = oracle_with(NOTO_SANS);
        let mut cx = ShapeContext::default();
        let text = "Hello, Hello Hello";
        let shaped = cx.shape(&mut oracle, text, &style(20.0));
        assert_tiles(&shaped, text);
        assert_eq!(shaped.runs.len(), 1, "one face, one script, one run");
        assert_eq!(shaped.faces[shaped.runs[0].face].blob.id(), blob.id());
        let ids: Vec<u32> = shaped.glyphs.iter().map(|g| g.id).collect();
        let expected = cmap_glyphs(NOTO_SANS, text);
        assert!(
            expected.iter().all(|&g| g != 0),
            "the subset maps every char"
        );
        assert_eq!(ids, expected);
        assert!(shaped.glyphs.iter().all(|g| g.advance > 0.0));
        let run = &shaped.runs[0];
        assert_eq!(run.metrics.units_per_em, 1000);
        assert!(run.metrics.ascent > 0.0 && run.metrics.descent > 0.0);
        assert_eq!(run.script, *b"Latn");
        assert!(run.coords.is_empty(), "a static face has no coordinates");
        assert_eq!(cx.passes(), 1);
        let words = shaped
            .atoms
            .iter()
            .filter(|a| a.flags.soft_wrap_opportunity_before)
            .count();
        assert_eq!(words, 2, "a soft wrap opportunity before each later word");
        assert!(shaped.atoms.iter().all(|a| !a.flags.safe_to_break_before));
    }

    #[test]
    fn mixed_latin_and_han_yields_two_runs_with_the_right_faces() {
        let latin = Blob::from(NOTO_SANS.to_vec());
        let han = Blob::from(FaceSpec::new("Synthetic Han").chars("中文").build());
        let mut backend = SyntheticBackend::new("hello-chinese");
        backend.cover(&latin, "Helo ");
        backend.cover(&han, "中文");
        let mut oracle = oracle_over(backend);
        let mut cx = ShapeContext::default();
        let text = "Hello 中文";
        let shaped = cx.shape(&mut oracle, text, &style(16.0));
        assert_tiles(&shaped, text);
        assert_eq!(shaped.runs.len(), 2, "{:?}", shaped.runs);
        let face_of = |run: &ShapedRun| shaped.faces[run.face].blob.id();
        assert_eq!(face_of(&shaped.runs[0]), latin.id());
        assert_eq!(face_of(&shaped.runs[1]), han.id());
        assert_eq!(
            shaped.runs[0].chars,
            0..6,
            "the space stays with the Latin run"
        );
        assert_eq!(shaped.runs[1].script, *b"Hani");
        let han_glyphs = &shaped.glyphs[shaped.runs[1].glyphs.start as usize..];
        let ids: Vec<u32> = han_glyphs.iter().map(|g| g.id).collect();
        assert_eq!(
            ids,
            vec![1, 2],
            "the synthetic face maps 中文 to glyphs 1 and 2"
        );
    }

    #[test]
    fn an_unresolvable_cluster_still_gets_a_face() {
        let (mut oracle, blob) = oracle_with(NOTO_SANS);
        let mut cx = ShapeContext::default();
        let text = "He中lo";
        let shaped = cx.shape(&mut oracle, text, &style(16.0));
        assert_tiles(&shaped, text);
        assert!(shaped.faces.iter().all(|f| f.blob.id() == blob.id()));
        assert!(shaped.glyphs.iter().any(|g| g.id == 0), "中 is .notdef");
    }

    fn geist_oracle() -> Oracle {
        let mut oracle = oracle_over(SyntheticBackend::new("geist"));
        oracle
            .registry_mut()
            .insert_blob(&Blob::from(GEIST_VARIABLE.to_vec()));
        oracle
    }

    #[test]
    fn weight_synthesis_reaches_the_coordinates() {
        let mut oracle = geist_oracle();
        let mut cx = ShapeContext::default();
        let mut coords = |weight| {
            let s = TextStyle {
                family: FontFamily::named("Geist"),
                weight,
                ..style(16.0)
            };
            let shaped = cx.shape(&mut oracle, "Hamburg", &s);
            assert_eq!(shaped.runs.len(), 1, "one face, one run");
            shaped.runs[0].coords.clone()
        };
        let light = coords(FontWeight::LIGHT);
        let semibold = coords(FontWeight::SEMI_BOLD);
        assert!(!light.is_empty() && !semibold.is_empty());
        assert!(light[0] < 0 && semibold[0] > 0, "{light:?} {semibold:?}");
        assert!(
            coords(FontWeight::REGULAR).is_empty(),
            "the default instance"
        );
        assert_eq!(cx.passes(), 2 + 2 + 1, "a second pass only off the default");
    }

    #[test]
    fn style_variations_change_the_coordinates() {
        let mut oracle = geist_oracle();
        let mut cx = ShapeContext::default();
        let base = TextStyle {
            family: FontFamily::named("Geist"),
            ..style(16.0)
        };
        let plain = cx.shape(&mut oracle, "Hamburg", &base);
        let heavy = cx.shape(
            &mut oracle,
            "Hamburg",
            &base.clone().with_variation(*b"wght", 800.0),
        );
        assert!(plain.runs[0].coords.is_empty());
        assert!(heavy.runs[0].coords[0] > 0, "{:?}", heavy.runs[0].coords);
        let advance = |s: &Shaped| s.runs[0].advance;
        assert!(advance(&heavy) > advance(&plain), "heavier Geist is wider");
    }

    #[test]
    fn letter_spacing_changes_atom_cluster_and_glyph_advances() {
        let (mut oracle, _) = oracle_with(NOTO_SANS);
        let mut cx = ShapeContext::default();
        let text = "Hello hello";
        let plain = cx.shape(&mut oracle, text, &style(20.0));
        let spaced = TextStyle {
            letter_spacing: 3.0,
            ..style(20.0)
        };
        let wide = cx.shape(&mut oracle, text, &spaced);
        assert_eq!(plain.atoms.len(), wide.atoms.len());
        for (a, b) in plain.atoms.iter().zip(&wide.atoms) {
            let clusters = (a.clusters.end - a.clusters.start) as f32;
            assert!((b.advance - a.advance - 3.0 * clusters).abs() < 1e-3);
        }
        for (a, b) in plain.glyphs.iter().zip(&wide.glyphs) {
            assert!(
                (b.advance - a.advance - 3.0).abs() < 1e-3,
                "one glyph per cluster"
            );
        }
        let total = |s: &Shaped| s.graphemes.iter().map(|g| g.advance).sum::<f32>();
        assert!((total(&wide) - total(&plain) - 3.0 * 11.0).abs() < 1e-3);
        assert!((wide.runs[0].advance - total(&wide)).abs() < 1e-3);
    }

    #[test]
    fn repeated_clusters_ask_the_oracle_once_per_shaping_call() {
        let (mut oracle, _) = oracle_with(NOTO_SANS);
        let mut cx = ShapeContext::default();
        let _ = cx.shape(&mut oracle, "lll lll", &style(16.0));
        assert_eq!(cx.selector.resolves(), 2, "`l` and the space, once each");
    }

    #[test]
    fn a_style_change_mid_text_tiles_and_keeps_both_sizes() {
        let (mut oracle, _) = oracle_with(NOTO_SANS);
        let mut cx = ShapeContext::default();
        let text = "Hello hello";
        let (small, large) = (style(12.0), style(24.0));
        let shaped = cx.shape_spans(&mut oracle, text, &[(0..6, &small), (6..11, &large)]);
        assert_tiles(&shaped, text);
        let sizes: Vec<f32> = shaped.runs.iter().map(|r| r.font_size).collect();
        assert_eq!(sizes, vec![12.0, 24.0]);
        assert_eq!(shaped.runs[1].style_index, 1);
        assert_eq!(shaped.runs[1].chars, 6..11);
    }

    #[test]
    fn newlines_mark_mandatory_breaks_and_rtl_sets_levels() {
        let (mut oracle, _) = oracle_with(NOTO_SANS);
        let mut cx = ShapeContext::default();
        let shaped = cx.shape(&mut oracle, "Hello\nhello", &style(16.0));
        let breaks: Vec<bool> = shaped
            .atoms
            .iter()
            .map(|a| a.flags.mandatory_break_after)
            .collect();
        assert_eq!(breaks.iter().filter(|&&b| b).count(), 1);
        assert_eq!(shaped.paragraph_level, 0);
        let rtl = cx.shape(
            &mut oracle,
            "\u{5e9}\u{5dc}\u{5d5}\u{5dd} Hello",
            &style(16.0),
        );
        assert_eq!(rtl.paragraph_level, 1, "first strong character is Hebrew");
        assert!(rtl.chars[0].bidi_level % 2 == 1);
        assert!(rtl.chars.last().is_some_and(|c| c.bidi_level % 2 == 0));
    }

    #[test]
    fn an_empty_text_shapes_to_nothing() {
        let (mut oracle, _) = oracle_with(NOTO_SANS);
        let mut cx = ShapeContext::default();
        let shaped = cx.shape(&mut oracle, "", &style(16.0));
        assert!(shaped.complete && shaped.runs.is_empty());
        assert_eq!(cx.analyses(), 0);
    }

    fn handle_for(oracle: &mut Oracle, style: &TextStyle) -> frust_scene::FontHandle {
        let mut cx = ShapeContext::default();
        let shaped = cx.shape(oracle, "Hamburg", style);
        let run = &shaped.runs[0];
        crate::convert::font_handle(
            peniko::FontData::new(shaped.faces[run.face].blob.clone(), 0),
            &run.coords,
            &shaped.faces[run.face],
            run.font_size,
        )
    }

    #[test]
    fn faux_bold_amount_follows_skia_interpolation() {
        assert_eq!(faux_bold_em_fraction(9.0), 1.0 / 24.0);
        assert_eq!(faux_bold_em_fraction(2.0), 1.0 / 24.0);
        assert_eq!(faux_bold_em_fraction(36.0), 1.0 / 32.0);
        assert_eq!(faux_bold_em_fraction(90.0), 1.0 / 32.0);
        let mid = faux_bold_em_fraction(20.0);
        assert!(mid < 1.0 / 24.0 && mid > 1.0 / 32.0);
    }

    #[test]
    fn handle_carries_embolden_for_a_static_face_asked_for_bold() {
        let (mut oracle, _) = oracle_with(NOTO_SANS);
        let bold = TextStyle {
            weight: FontWeight::BOLD,
            ..style(16.0)
        };
        let handle = handle_for(&mut oracle, &bold);
        assert!(handle.embolden() > 0.0);
        assert!(handle.normalized_coords().is_empty());
        let plain = handle_for(&mut oracle, &style(16.0));
        assert_eq!((plain.embolden(), plain.skew_degrees()), (0.0, 0.0));
        assert!(plain.normalized_coords().is_empty());
    }

    #[test]
    fn handle_on_a_variable_face_has_coords_and_no_embolden() {
        let mut oracle = geist_oracle();
        let bold = TextStyle {
            family: FontFamily::named("Geist"),
            weight: FontWeight::BOLD,
            ..style(16.0)
        };
        let handle = handle_for(&mut oracle, &bold);
        assert!(!handle.normalized_coords().is_empty());
        assert_eq!(handle.embolden(), 0.0);
    }

    #[test]
    fn handle_skews_an_italic_ask_on_an_upright_face() {
        let (mut oracle, _) = oracle_with(NOTO_SANS);
        let italic = TextStyle {
            style: crate::style::FontStyle::Italic,
            ..style(16.0)
        };
        let handle = handle_for(&mut oracle, &italic);
        assert_eq!(handle.skew_degrees(), 14.0);
        assert_eq!(handle.embolden(), 0.0);
    }

    #[test]
    fn hidden_and_synthesis_flags_pass_through() {
        let face = Blob::from(FaceSpec::new("Hidden UI").chars("a").build());
        let mut source = FaceSource::blob(face.clone(), 0);
        source.hidden = true;
        source.postscript_name = Some(".HiddenUI".into());
        let mut backend = SyntheticBackend::new("hidden");
        backend.cover_with(source, "a");
        let mut oracle = oracle_over(backend);
        let mut cx = ShapeContext::default();
        let bold = TextStyle {
            weight: FontWeight::BOLD,
            ..style(16.0)
        };
        let shaped = cx.shape(&mut oracle, "a", &bold);
        let face = &shaped.faces[shaped.runs[0].face];
        assert!(face.hidden);
        assert_eq!(face.postscript_name.as_deref(), Some(".HiddenUI"));
        assert!(face.embolden, "a static regular face asked for bold");
    }

    /// Every `.rs` file under `dir`, recursively.
    fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("source dir is readable") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn no_shaping_engine_type_is_named_outside_shape() {
        // Built with `concat!` so nothing matches its own spelling by accident.
        let forbidden = [concat!("parley", "_engine"), concat!("harf", "rust")];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let shape = src.join("shape");
        let mut files = Vec::new();
        sources(&src, &mut files);
        assert!(
            files.iter().any(|p| p.starts_with(src.join("oracle"))),
            "the scan includes oracle/"
        );
        for path in files.iter().filter(|p| !p.starts_with(&shape)) {
            let text = std::fs::read_to_string(path).expect("source is UTF-8");
            for word in forbidden {
                assert!(
                    !text.contains(word),
                    "{} names `{word}`: only src/shape/ may (D14)",
                    path.display()
                );
            }
        }
    }
}
