//! The font-selection adapter: the engine's per-cluster `FontSelector` answered
//! by the frust font oracle.
//!
//! Per cluster it asks [`Oracle::resolve`] with the cluster's style (family
//! chain, weight, slant, width, locale) and segment script, an emoji cluster
//! also offering the `Emoji` generic as parley 0.11 did. The oracle's answer
//! is checked with the engine's own coverage test (which also tries the
//! cluster's composed and decomposed forms); when it is missing or partial,
//! the chain's generic defaults and the faces this paragraph already uses
//! compete on coverage, and the best one wins — a face drawing `.notdef` is
//! still a face, because a selector that answers nothing aborts shaping.
//!
//! Which candidate the oracle tries first is the oracle's business; nothing
//! here depends on it. Answers are memoised per (style, script, emoji, cluster
//! text) for one shaping call, and every decision is recorded by cluster start
//! so a second pass can replay them (see `super::ShapeContext`).

use std::collections::HashMap;
use std::ops::Range;

use fontique::Synthesis as EngineSynthesis;
use parley_engine::itemize::Segment;
use parley_engine::shape::CharCluster;
use parley_engine::{
    AnalysisDataSources, FontData, FontInstance, FontInstanceRef, FontSelector, ShapeOptions,
};
use read_fonts::{FontRef, TableProvider};

use crate::oracle::{FaceRef, FallbackRequest, Oracle, ScriptTag};
use crate::style::{FamilyName, FontFamily, FontStyle, FontWeight, GenericSlot, TextStyle};

/// The selection-relevant projection of one span's style.
#[derive(Clone, Debug)]
pub(super) struct StyleQuery {
    family: FontFamily,
    weight: FontWeight,
    slant: FontStyle,
    width: f32,
    locale: Option<String>,
}

impl StyleQuery {
    pub(super) fn of(style: &TextStyle) -> Self {
        Self {
            family: style.family.clone(),
            weight: style.weight,
            slant: style.style,
            width: style.width,
            locale: style.locale.clone(),
        }
    }

    fn request(
        &self,
        chars: &str,
        script: [u8; 4],
        emoji: bool,
        oracle: &Oracle,
    ) -> FallbackRequest {
        let mut req = FallbackRequest::new(
            chars,
            ScriptTag(script),
            &self.family,
            oracle.capabilities(),
        );
        req.locale = self.locale.clone();
        req.weight = self.weight;
        req.slant = self.slant;
        req.width = self.width;
        let emoji_slot = FamilyName::Generic(GenericSlot::Emoji);
        if emoji && !req.families.contains(&emoji_slot) {
            req.families.push(emoji_slot);
        }
        req
    }
}

/// A face this paragraph shapes with: the oracle's answer plus the engine
/// instance handed to the shaper.
pub(super) struct Selected {
    pub(super) face: FaceRef,
    instance: FontInstance,
    /// The synthesis coordinates that change anything: settings of axes the
    /// face has, at values other than the axis default.
    pub(super) coords: Vec<([u8; 4], f32)>,
    pub(super) units_per_em: u16,
}

/// One segment the shaper reported: constant bidi level and script.
#[derive(Clone, Debug)]
pub(super) struct SegmentInfo {
    pub(super) chars: Range<usize>,
    pub(super) script: [u8; 4],
    pub(super) language: Option<String>,
}

/// A face's variation axes and their defaults, by tag.
type AxisDefaults = Vec<([u8; 4], f32)>;

/// Whether a shaping pass resolves through the oracle or replays decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Resolve,
    Replay,
}

/// What the adapter keeps between clusters, passes and shaping calls.
pub(super) struct SelectorState {
    faces: Vec<Selected>,
    /// `(cluster start byte, face index)`, in text order.
    decisions: Vec<(u32, usize)>,
    segments: Vec<SegmentInfo>,
    memo: HashMap<(u16, [u8; 4], bool), HashMap<String, usize>>,
    chars: String,
    mode: Mode,
    /// Per face file (blob id, index): its `fvar` axis defaults; font data
    /// never changes under a blob id, so this outlives shaping calls.
    axis_defaults: HashMap<(u64, u32), AxisDefaults>,
    resolves: u64,
}

impl Default for SelectorState {
    fn default() -> Self {
        Self {
            faces: Vec::new(),
            decisions: Vec::new(),
            segments: Vec::new(),
            memo: HashMap::new(),
            chars: String::new(),
            mode: Mode::Resolve,
            axis_defaults: HashMap::new(),
            resolves: 0,
        }
    }
}

impl SelectorState {
    /// Forgets the previous paragraph and resolves afresh.
    pub(super) fn begin_shape(&mut self) {
        self.faces.clear();
        self.decisions.clear();
        self.segments.clear();
        self.memo.clear();
        self.mode = Mode::Resolve;
    }

    /// Switches to replaying the recorded decisions for a further pass.
    pub(super) fn begin_replay(&mut self) {
        self.segments.clear();
        self.mode = Mode::Replay;
    }

    pub(super) fn faces(&self) -> &[Selected] {
        &self.faces
    }

    /// The face chosen for the cluster starting at byte `start`.
    pub(super) fn face_at(&self, start: usize) -> Option<usize> {
        let start = u32::try_from(start).ok()?;
        self.decisions
            .binary_search_by_key(&start, |&(s, _)| s)
            .ok()
            .map(|i| self.decisions[i].1)
    }

    /// The face recorded for the last cluster starting at or before `start`.
    fn replayed(&self, start: u32) -> Option<usize> {
        let i = self.decisions.partition_point(|&(s, _)| s <= start);
        self.decisions.get(i.checked_sub(1)?).map(|&(_, face)| face)
    }

    /// The segment holding character `char_index`.
    pub(super) fn segment_at(&self, char_index: usize) -> Option<&SegmentInfo> {
        let i = self.segments.partition_point(|s| s.chars.end <= char_index);
        self.segments.get(i)
    }

    /// Whether any chosen face needs coordinates the first pass could not give it.
    pub(super) fn any_coords(&self) -> bool {
        self.faces.iter().any(|f| !f.coords.is_empty())
    }

    /// Oracle resolutions performed (memo misses), across every call.
    #[cfg(test)]
    pub(super) fn resolves(&self) -> u64 {
        self.resolves
    }

    /// The index of `face` among this paragraph's faces, adding it if new.
    fn intern(&mut self, face: FaceRef) -> usize {
        if let Some(i) = self.faces.iter().position(|s| {
            s.face.blob.id() == face.blob.id()
                && s.face.index == face.index
                && s.face.synthesis == face.synthesis
        }) {
            return i;
        }
        let defaults = self
            .axis_defaults
            .entry((face.blob.id(), face.index))
            .or_insert_with(|| axis_defaults(&face))
            .clone();
        let coords = face
            .synthesis
            .variations
            .iter()
            .filter(|v| {
                defaults
                    .iter()
                    .any(|&(tag, default)| tag == v.tag && default != v.value)
            })
            .map(|v| (v.tag, v.value))
            .collect();
        let instance = FontInstance {
            font: FontData::new(face.blob.clone(), face.index),
            synthesis: EngineSynthesis::default(),
        };
        let units_per_em = units_per_em(&face);
        self.faces.push(Selected {
            face,
            instance,
            coords,
            units_per_em,
        });
        self.faces.len() - 1
    }
}

/// The adapter for one shaping pass.
pub(super) struct Adapter<'a> {
    pub(super) oracle: &'a mut Oracle,
    pub(super) state: &'a mut SelectorState,
    pub(super) styles: &'a [StyleQuery],
    pub(super) sources: &'a AnalysisDataSources,
}

impl Adapter<'_> {
    fn resolve(&mut self, segment: &Segment, cluster: &mut CharCluster) -> Option<usize> {
        let style_index = cluster.style_index();
        let style = self.styles.get(usize::from(style_index))?;
        let script = segment.script.to_bytes();
        let emoji = cluster.is_emoji();
        let mut chars = std::mem::take(&mut self.state.chars);
        chars.clear();
        chars.extend(cluster.chars().iter().map(|c| c.ch));
        let key = (style_index, script, emoji);
        let memoised = self
            .state
            .memo
            .get(&key)
            .and_then(|m| m.get(chars.as_str()))
            .copied();
        let chosen = match memoised {
            Some(i) => Some(i),
            None => {
                self.state.resolves += 1;
                let req = style.request(&chars, script, emoji, self.oracle);
                let chosen = self.choose(&req, cluster);
                if let Some(i) = chosen {
                    self.state
                        .memo
                        .entry(key)
                        .or_default()
                        .insert(chars.clone(), i);
                }
                chosen
            }
        };
        self.state.chars = chars;
        chosen
    }

    /// The oracle's answer when it fully covers the cluster; otherwise the
    /// best-covering candidate, the first candidate, or any face in use.
    fn choose(&mut self, req: &FallbackRequest, cluster: &mut CharCluster) -> Option<usize> {
        let mut candidates = Vec::new();
        if let Some(face) = self.oracle.resolve(req) {
            if covers(&face, cluster, self.sources).is_complete() {
                return Some(self.state.intern(face));
            }
            candidates.push(face);
        }
        for generic in req.generics() {
            if let Some(face) = self.oracle.default_face(generic) {
                candidates.push(face);
            }
        }
        candidates.extend(self.state.faces.iter().map(|s| s.face.clone()));
        let mut best: Option<(usize, parley_engine::shape::Coverage)> = None;
        for (i, face) in candidates.iter().enumerate() {
            let coverage = covers(face, cluster, self.sources);
            if coverage.is_complete() {
                best = Some((i, coverage));
                break;
            }
            if coverage.covered > 0 && best.is_none_or(|(_, b)| coverage > b) {
                best = Some((i, coverage));
            }
        }
        let pick = best.map_or(0, |(i, _)| i);
        let face = candidates.into_iter().nth(pick)?;
        Some(self.state.intern(face))
    }
}

impl FontSelector for Adapter<'_> {
    fn begin_segment(&mut self, segment: &Segment, options: &ShapeOptions<'_>) {
        self.state.segments.push(SegmentInfo {
            chars: segment.range.char_range.clone(),
            script: segment.script.to_bytes(),
            language: options.language.as_ref().map(|l| l.as_str().to_owned()),
        });
    }

    fn select_font(
        &mut self,
        segment: &Segment,
        _options: &ShapeOptions<'_>,
        cluster: &mut CharCluster,
    ) -> Option<FontInstanceRef<'_>> {
        let start = cluster.range().start;
        let index = match self.state.mode {
            Mode::Replay => self.state.replayed(start)?,
            Mode::Resolve => {
                let index = self.resolve(segment, cluster)?;
                self.state.decisions.push((start, index));
                index
            }
        };
        Some(self.state.faces[index].instance.as_ref())
    }
}

/// `face`'s coverage of `cluster` through its `cmap` (glyph 0 is no glyph).
fn covers(
    face: &FaceRef,
    cluster: &mut CharCluster,
    sources: &AnalysisDataSources,
) -> parley_engine::shape::Coverage {
    let cmap = FontRef::from_index(face.blob.data(), face.index)
        .ok()
        .and_then(|font| font.cmap().ok());
    cluster.calculate_coverage(
        |c| {
            cmap.as_ref()
                .and_then(|cmap| cmap.map_codepoint(c))
                .is_some_and(|glyph| glyph.to_u32() != 0)
        },
        sources,
    )
}

/// `face`'s variation axes and their defaults (empty for a static face).
fn axis_defaults(face: &FaceRef) -> AxisDefaults {
    FontRef::from_index(face.blob.data(), face.index)
        .ok()
        .and_then(|font| font.fvar().ok())
        .and_then(|fvar| fvar.axes().ok())
        .map(|axes| {
            axes.iter()
                .map(|axis| (axis.axis_tag().to_be_bytes(), axis.default_value().to_f32()))
                .collect()
        })
        .unwrap_or_default()
}

/// `face`'s `head.unitsPerEm`, `0` when unreadable.
fn units_per_em(face: &FaceRef) -> u16 {
    FontRef::from_index(face.blob.data(), face.index)
        .ok()
        .and_then(|font| font.head().ok())
        .map_or(0, |head| head.units_per_em())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::Synthesis;
    use crate::oracle::synthetic::FaceSpec;
    use peniko::Blob;

    fn face(blob: &Blob<u8>, synthesis: Synthesis) -> FaceRef {
        FaceRef {
            blob: blob.clone(),
            index: 0,
            synthesis,
            postscript_name: None,
            hidden: false,
        }
    }

    #[test]
    fn interning_dedupes_and_keeps_only_coordinates_that_change_the_face() {
        let variable = Blob::from(FaceSpec::new("Var").chars("a").axes(&[*b"wght"]).build());
        let mut state = SelectorState::default();
        let at = |value| Synthesis {
            variations: vec![
                crate::oracle::VariationSetting {
                    tag: *b"wght",
                    value,
                },
                crate::oracle::VariationSetting {
                    tag: *b"wdth",
                    value: 75.0,
                },
            ],
            ..Synthesis::default()
        };
        let default_weight = state.intern(face(&variable, at(400.0)));
        assert!(
            state.faces()[default_weight].coords.is_empty(),
            "the axis default and an absent axis change nothing"
        );
        let light = state.intern(face(&variable, at(300.0)));
        assert_ne!(light, default_weight);
        assert_eq!(state.faces()[light].coords, vec![(*b"wght", 300.0)]);
        assert_eq!(state.intern(face(&variable, at(300.0))), light, "deduped");
        assert!(state.any_coords());
    }

    #[test]
    fn decisions_and_segments_are_looked_up_by_position() {
        let state = SelectorState {
            decisions: vec![(0, 0), (5, 1), (9, 0)],
            segments: vec![
                SegmentInfo {
                    chars: 0..6,
                    script: *b"Latn",
                    language: None,
                },
                SegmentInfo {
                    chars: 6..8,
                    script: *b"Hani",
                    language: Some("zh".into()),
                },
            ],
            ..SelectorState::default()
        };
        assert_eq!(state.face_at(5), Some(1));
        assert_eq!(state.face_at(6), None, "not a cluster start");
        assert_eq!(
            state.replayed(6),
            Some(1),
            "replay falls back to the cluster before"
        );
        assert_eq!(state.replayed(9), Some(0));
        assert_eq!(state.segment_at(5).map(|s| s.script), Some(*b"Latn"));
        assert_eq!(state.segment_at(6).map(|s| s.script), Some(*b"Hani"));
        assert!(state.segment_at(8).is_none());
    }
}
