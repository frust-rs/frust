//! What is drawn from an open face: the variation instances CoreText copies
//! of it ([`Face::instance`]), the bounded outline cache ([`OutlineCache`]),
//! and the one bounded least-recently-used map ([`Lru`]) both are kept in, as
//! are the open faces (`super::face`).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::hash::Hash;

use kurbo::{BezPath, PathEl};

use super::face::{Axis, tag_list, truncated};
use super::ffi::CtFace;
use super::{F2DOT14_ONE, FIXED_ONE, MAX_INSTANCES_PER_FACE, VariationKey};

/// One cached outline's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct OutlineKey {
    /// The open face's process-unique id.
    pub(super) face: u64,
    /// The glyph.
    pub(super) glyph_id: u32,
    /// The variation instance the outline was fetched at.
    pub(super) variation: VariationKey,
}

/// What the provider learned about one glyph.
#[derive(Debug)]
pub(super) enum Fetched {
    /// Its outline, in font units with y up.
    Path(BezPath),
    /// A glyph with no ink — a space. Drawn as nothing and not counted as
    /// skipped, exactly as an outline glyph with no ink is on `glifo`'s route.
    Empty,
    /// An inked glyph CoreText returned no outline for, or a glyph id outside
    /// the face.
    Missing,
}

/// A map of at most `capacity` entries that evicts its least recently used
/// entry to make room: the one bound the outline cache, each face's variation
/// instances and the open faces are kept under.
///
/// Each entry carries the tick it was last used at, and `order` maps ticks
/// back to keys, so the oldest entry is the first key of `order` — a lookup,
/// a touch and an eviction are each a hash lookup plus a `BTreeMap` step.
#[derive(Debug)]
pub(super) struct Lru<K, V> {
    /// The most entries kept.
    capacity: usize,
    /// Value and last-use tick per key.
    entries: HashMap<K, (V, u64)>,
    /// Last-use tick to key, oldest first.
    order: BTreeMap<u64, K>,
    /// The use counter ticks are drawn from.
    tick: u64,
}

impl<K: Clone + Eq + Hash, V> Lru<K, V> {
    /// An empty map keeping at most `capacity` entries (at least one).
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
        }
    }

    /// How many entries are kept.
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no entry is kept.
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many entries are kept at most.
    pub(super) fn capacity(&self) -> usize {
        self.capacity
    }

    /// Whether `key` has an entry, without marking it used.
    #[cfg(test)]
    pub(super) fn contains_key(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// Marks `key`'s entry the most recently used, answering whether there is
    /// one.
    pub(super) fn touch(&mut self, key: &K) -> bool {
        match self.entries.get_mut(key) {
            Some((_, used)) => {
                Self::retick(&mut self.order, &mut self.tick, used);
                true
            }
            None => false,
        }
    }

    /// Marks the entry whose key `matches` the most recently used, answering
    /// whether one does: a scan, for a key compared in place rather than built
    /// for a lookup. At most one key is expected to match.
    pub(super) fn touch_matching(&mut self, mut matches: impl FnMut(&K) -> bool) -> bool {
        match self.entries.iter_mut().find(|(key, _)| matches(key)) {
            Some((_, (_, used))) => {
                Self::retick(&mut self.order, &mut self.tick, used);
                true
            }
            None => false,
        }
    }

    /// The most recently used entry's value: the one just touched or
    /// inserted.
    pub(super) fn newest_mut(&mut self) -> Option<&mut V> {
        let (_, key) = self.order.last_key_value()?;
        self.entries.get_mut(key).map(|(value, _)| value)
    }

    /// Keeps `value` under `key` as the most recently used entry, evicting the
    /// least recently used entry first when the map is full, and answers it.
    pub(super) fn insert(&mut self, key: K, value: V) -> &mut V {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        if let Some((_, used)) = self.entries.remove(&key) {
            self.order.remove(&used);
        } else if self.entries.len() >= self.capacity
            && let Some((_, oldest)) = self.order.pop_first()
        {
            self.entries.remove(&oldest);
        }
        self.order.insert(tick, key.clone());
        &mut self.entries.entry(key).or_insert((value, tick)).0
    }

    /// Moves the entry last used at `used` to a fresh tick, the newest.
    fn retick(order: &mut BTreeMap<u64, K>, tick: &mut u64, used: &mut u64) {
        *tick = tick.wrapping_add(1);
        if let Some(key) = order.remove(used) {
            order.insert(*tick, key);
        }
        *used = *tick;
    }
}

/// A bounded least-recently-used map from glyph to outline ([`Lru`]),
/// counting the fetches it answered and the ones that asked CoreText.
#[derive(Debug)]
pub(super) struct OutlineCache {
    /// Outline per glyph, least recently used evicted first.
    entries: Lru<OutlineKey, Fetched>,
    /// Fetch calls answered from the cache, saturating.
    pub(super) hits: u64,
    /// Fetch calls that asked CoreText and got an answer for the glyph — an
    /// outline, no ink, or none — saturating. A fetch at a refused instance
    /// is neither a hit nor a miss.
    pub(super) misses: u64,
}

impl OutlineCache {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            entries: Lru::new(capacity),
            hits: 0,
            misses: 0,
        }
    }

    /// How many outlines are cached.
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// How many outlines the cache keeps at most.
    pub(super) fn capacity(&self) -> usize {
        self.entries.capacity()
    }

    /// `key`'s entry, marked most recently used, or `fetch`'s answer inserted
    /// in its place — evicting the least recently used entry when full. A
    /// `fetch` answering `None` (a refused instance) inserts and evicts
    /// nothing.
    ///
    /// Counts a hit when the entry was cached, a miss when `fetch` answered
    /// (CoreText was asked for the glyph), and neither when `fetch` answered
    /// `None`: a remembered refusal asks CoreText nothing.
    pub(super) fn get_or_fetch(
        &mut self,
        key: OutlineKey,
        fetch: impl FnOnce() -> Option<Fetched>,
    ) -> Option<&Fetched> {
        if self.entries.touch(&key) {
            self.hits = self.hits.saturating_add(1);
            return self.entries.newest_mut().map(|fetched| &*fetched);
        }
        let fetched = fetch()?;
        self.misses = self.misses.saturating_add(1);
        Some(&*self.entries.insert(key, fetched))
    }
}

/// One face open in CoreText, with the variation instances copied from it.
///
/// Built field by field where it is opened (`super::face`), not through a
/// constructor: on a target without CoreText a `Face` is uninhabited, and code
/// after a call returning one would be flagged unreachable.
pub(super) struct Face {
    /// A process-unique id, which the outline cache keys on. A face evicted
    /// and opened again is a new id; its old outlines age out of the cache.
    pub(super) id: u64,
    /// The CoreText face, opened at `units_per_em` points so its outlines are
    /// in font units — the face's default instance.
    pub(super) ct: CtFace,
    /// The face's `head.unitsPerEm`.
    pub(super) units_per_em: f64,
    /// The face's variation axes, in `fvar` order; empty for a face that does
    /// not vary.
    pub(super) axes: Vec<Axis>,
    /// The variation instances copied from `ct`, least recently used evicted
    /// first, at most [`MAX_INSTANCES_PER_FACE`].
    pub(super) instances: Lru<VariationKey, CtFace>,
    /// The instances refused (CoreText made no copy, or its copy contradicts
    /// a value asked for), oldest first, at most [`MAX_INSTANCES_PER_FACE`].
    pub(super) refused: VecDeque<VariationKey>,
    /// CoreText's own axis defaults for `ct`, read on the first instance
    /// copied: an axis a copy does not report is applied when it was asked
    /// at its default ([`read_back`]).
    pub(super) coretext_defaults: Option<Vec<(u32, f64)>>,
    /// Whether a refused instance of this face has been warned about: one
    /// warning per open face, however many instances are refused.
    pub(super) refusal_warned: bool,
    /// Whether an unverified instance of this face has been warned about: one
    /// warning per open face, however many instances draw unverified.
    pub(super) unverified_warned: bool,
}

/// What the read-back of variation instances has counted, since the provider
/// was built (saturating). Each counts events: an instance copied or refused
/// again after it left a face's remembered set counts again.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct VariationCounts {
    /// Instances refused: CoreText made no copy, or its copy reports an axis
    /// at another value than asked ([`ReadBack::Contradicted`]).
    pub(super) mismatches: u32,
    /// Instances drawn from a copy that contradicts nothing but does not
    /// report an axis asked off its default ([`ReadBack::Unverified`]).
    pub(super) unverified: u32,
}

impl Face {
    /// The CoreText face to read `variation`'s outlines from: the face as
    /// opened for the default instance, else a copy at the user values
    /// `coords` maps to, copied on first use — or `None` when that copy is
    /// refused.
    ///
    /// A copy is checked against the instance CoreText reads back from it
    /// ([`read_back`]). One that reports an axis at another value than asked
    /// is refused — drawing from it would cache another instance's outlines
    /// under this one's key — as is a copy CoreText does not make: counted in
    /// `counts.mismatches`, remembered while the face stays open, and warned
    /// about once per open face (the face's `name` escaped). One that
    /// contradicts nothing but leaves an axis asked off its default
    /// unreported is drawn as copied, unverified: counted in
    /// `counts.unverified` and warned about once per open face.
    pub(super) fn instance(
        &mut self,
        variation: VariationKey,
        coords: &[i16],
        name: &str,
        counts: &mut VariationCounts,
    ) -> Option<&CtFace> {
        if variation == VariationKey::DEFAULT {
            return Some(&self.ct);
        }
        if self.instances.touch(&variation) {
            return self.instances.newest_mut().map(|varied| &*varied);
        }
        if self.refused.contains(&variation) {
            return None;
        }
        let values = user_values(&self.axes, coords);
        let Some(varied) = self.ct.varied(&values, self.units_per_em) else {
            self.refuse(variation, name, &values, "CoreText made no copy", counts);
            return None;
        };
        let defaults = self
            .coretext_defaults
            .get_or_insert_with(|| self.ct.axis_defaults());
        match read_back(&self.axes, &values, defaults, &reported_variation(&varied)) {
            ReadBack::Applied => {}
            ReadBack::Unverified(unreported) => {
                self.draw_unverified(name, &values, &unreported, counts);
            }
            ReadBack::Contradicted(axis) => {
                self.refuse(variation, name, &values, &axis.to_string(), counts);
                return None;
            }
        }
        Some(&*self.instances.insert(variation, varied))
    }

    /// Remembers `variation` as refused (oldest refusal out at the cap),
    /// counting it in `counts.mismatches` and warning once per open face.
    fn refuse(
        &mut self,
        variation: VariationKey,
        name: &str,
        values: &[(u32, f64)],
        why: &str,
        counts: &mut VariationCounts,
    ) {
        if self.refused.len() >= MAX_INSTANCES_PER_FACE {
            self.refused.pop_front();
        }
        self.refused.push_back(variation);
        counts.mismatches = counts.mismatches.saturating_add(1);
        if core::mem::replace(&mut self.refusal_warned, true) {
            return;
        }
        #[cfg(test)]
        tests::REFUSAL_WARNINGS.set(tests::REFUSAL_WARNINGS.get() + 1);
        log::warn!(
            "frust-engine: hvgl face {:?} at [{}] refused: CoreText's copy is another instance \
             ({why}); its glyphs at that instance are counted as skipped (warned once per open \
             face)",
            truncated(name),
            instance_list(values),
        );
    }

    /// Counts an instance drawn from a copy whose read-back leaves the axes
    /// `unreported` (`(axis tag, user value asked)`) unconfirmed, warning once
    /// per open face.
    fn draw_unverified(
        &mut self,
        name: &str,
        values: &[(u32, f64)],
        unreported: &[(u32, f64)],
        counts: &mut VariationCounts,
    ) {
        counts.unverified = counts.unverified.saturating_add(1);
        if core::mem::replace(&mut self.unverified_warned, true) {
            return;
        }
        #[cfg(test)]
        tests::UNVERIFIED_WARNINGS.set(tests::UNVERIFIED_WARNINGS.get() + 1);
        let tags: Vec<u32> = unreported.iter().map(|(tag, _)| *tag).collect();
        log::warn!(
            "frust-engine: hvgl face {:?} at [{}] drawn unverified: CoreText's copy does not \
             report {}; its glyphs at that instance are drawn from the copy as made (warned \
             once per open face)",
            truncated(name),
            instance_list(values),
            tag_list(&tags),
        );
    }
}

/// The axes CoreText reports for `varied` (`CTFontCopyVariation`), as `(axis
/// tag, user value)`. CoreText may leave out an axis it holds at its default.
fn reported_variation(varied: &CtFace) -> Vec<(u32, f64)> {
    let reported = varied.variation_values();
    #[cfg(test)]
    let reported = tests::read_back_hook(reported);
    reported
}

/// How CoreText's copy carries one axis asked for, read back from it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum AxisReadBack {
    /// Reported within the axis's precision of the value asked for, or not
    /// reported and asked at its default.
    Applied,
    /// Reported at the value held, outside the axis's precision of the one
    /// asked for — CoreText clamping an out-of-range request included.
    Contradicted(f64),
    /// Not reported, and asked off its default: the copy may carry it or not.
    Unreported,
}

/// How the copy carries `asked` on `axis`: `reported` is the value CoreText
/// reports for the axis, if any, and `default` the axis's default — both
/// compared with `asked` within the axis's precision ([`axis_precision`]).
fn classify_axis(axis: &Axis, asked: f64, reported: Option<f64>, default: f64) -> AxisReadBack {
    let within = |value: f64| (value - asked).abs() <= axis_precision(axis, asked) + f64::EPSILON;
    match reported {
        Some(value) if within(value) => AxisReadBack::Applied,
        Some(value) => AxisReadBack::Contradicted(value),
        None if within(default) => AxisReadBack::Applied,
        None => AxisReadBack::Unreported,
    }
}

/// One axis CoreText's copy reports at another value than asked.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ContradictedAxis {
    /// The axis tag.
    tag: u32,
    /// The user value asked for.
    asked: f64,
    /// The user value CoreText reports.
    reported: f64,
}

impl core::fmt::Display for ContradictedAxis {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} read back as {}, asked {}",
            tag_list(&[self.tag]),
            self.reported,
            self.asked
        )
    }
}

/// What the read-back of a copy says of the whole instance asked for.
#[derive(Debug, Clone, PartialEq)]
enum ReadBack {
    /// Every axis applied ([`AxisReadBack::Applied`]).
    Applied,
    /// No axis contradicted, but these (`(axis tag, user value asked)`) are
    /// unreported ([`AxisReadBack::Unreported`]).
    Unverified(Vec<(u32, f64)>),
    /// The first axis CoreText reports at another value than asked.
    Contradicted(ContradictedAxis),
}

/// How the copy whose read-back is `reported` carries `asked` (`(axis tag,
/// user value)`, every axis of `axes`). An axis's default is CoreText's own
/// (`defaults`, read from the face the copy was made from) when it lists the
/// axis, else the bytes' `fvar` default. Only what CoreText reports is
/// compared with the value asked: an axis it leaves out is never filled in.
fn read_back(
    axes: &[Axis],
    asked: &[(u32, f64)],
    defaults: &[(u32, f64)],
    reported: &[(u32, f64)],
) -> ReadBack {
    let value_of = |list: &[(u32, f64)], tag: u32| {
        list.iter()
            .find(|(listed, _)| *listed == tag)
            .map(|(_, value)| *value)
    };
    let mut unreported = Vec::new();
    for ((tag, asked), axis) in asked.iter().zip(axes) {
        let default = value_of(defaults, *tag).unwrap_or(axis.default);
        match classify_axis(axis, *asked, value_of(reported, *tag), default) {
            AxisReadBack::Applied => {}
            AxisReadBack::Contradicted(reported) => {
                return ReadBack::Contradicted(ContradictedAxis {
                    tag: *tag,
                    asked: *asked,
                    reported,
                });
            }
            AxisReadBack::Unreported => unreported.push((*tag, *asked)),
        }
    }
    if unreported.is_empty() {
        ReadBack::Applied
    } else {
        ReadBack::Unverified(unreported)
    }
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
pub(super) fn user_value(axis: &Axis, normalized: f64) -> f64 {
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

/// What CoreText supplies for `glyph_id` of `face`.
pub(super) fn fetch(face: &CtFace, glyph_id: u32) -> Fetched {
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

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use std::sync::Arc;

    #[cfg(target_os = "macos")]
    use frust_scene::FontHandle;

    #[cfg(target_os = "macos")]
    use super::super::face::tests::system_pingfang_sc;
    #[cfg(target_os = "macos")]
    use super::super::face::variation_axes;
    #[cfg(target_os = "macos")]
    use super::super::{GlyphOutline, HvglOutlines};
    use super::*;
    #[cfg(target_os = "macos")]
    use crate::text::table_directory;

    thread_local! {
        /// Added to every user value CoreText reports on this thread, so a
        /// case can force a contradicted read-back.
        static READ_BACK_SKEW: core::cell::Cell<f64> = const { core::cell::Cell::new(0.0) };
        /// Whether every axis CoreText reports on this thread is dropped, so a
        /// case can force an unreported read-back.
        static READ_BACK_DROPPED: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
        /// How many refusal warnings were logged on this thread.
        pub(super) static REFUSAL_WARNINGS: core::cell::Cell<u32> =
            const { core::cell::Cell::new(0) };
        /// How many unverified-instance warnings were logged on this thread.
        pub(super) static UNVERIFIED_WARNINGS: core::cell::Cell<u32> =
            const { core::cell::Cell::new(0) };
    }

    /// The read-back hook: `reported` with this thread's skew added, or
    /// nothing when this thread drops it.
    pub(super) fn read_back_hook(reported: Vec<(u32, f64)>) -> Vec<(u32, f64)> {
        if READ_BACK_DROPPED.get() {
            return Vec::new();
        }
        let skew = READ_BACK_SKEW.get();
        reported
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
        assert_eq!(cache.entries.order.len(), 3);
        assert!(cache.entries.contains_key(&key(0)));
        assert!(
            !cache.entries.contains_key(&key(1)),
            "the oldest entry is evicted"
        );
        assert!(cache.entries.contains_key(&key(2)));
        assert!(cache.entries.contains_key(&key(3)));
        assert!(matches!(OutlineCache::new(0).capacity(), 1));
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

    #[test]
    fn an_instance_is_applied_only_within_each_axis_precision() {
        let axes = [
            axis(b"wght", 100.0, 500.0, 600.0),
            axis(b"WDTH", 1.0, 500.0, 1000.0),
        ];
        let [wght, wdth] = [axes[0].tag, axes[1].tag];
        let asked = [(wght, 300.0), (wdth, 500.0)];
        let defaults = [(wght, 500.0), (wdth, 500.0)];
        assert_eq!(
            read_back(&axes, &asked, &defaults, &asked),
            ReadBack::Applied
        );
        // One F2Dot14 step below the default is 400 / 16384 user units.
        let step = 400.0 / F2DOT14_ONE;
        assert_eq!(axis_precision(&axes[0], 300.0), step);
        assert_eq!(axis_precision(&axes[0], 550.0), 100.0 / F2DOT14_ONE);
        let close = [(wght, 300.0 + step), (wdth, 500.0)];
        assert_eq!(
            read_back(&axes, &asked, &defaults, &close),
            ReadBack::Applied,
            "within a step"
        );
        let off = [(wdth, 500.0), (wght, 300.0 + 2.0 * step)];
        assert_eq!(
            read_back(&axes, &asked, &defaults, &off),
            ReadBack::Contradicted(ContradictedAxis {
                tag: wght,
                asked: 300.0,
                reported: 300.0 + 2.0 * step
            }),
            "matched by tag, not by position"
        );
        let ReadBack::Contradicted(contradicted) =
            read_back(&axes, &asked, &defaults, &[(wght, 500.0), (wdth, 500.0)])
        else {
            panic!("the default weight contradicts 300");
        };
        assert_eq!(contradicted.to_string(), "wght read back as 500, asked 300");
        // A degenerate axis still allows one `Fixed` unit.
        assert_eq!(
            axis_precision(&axis(b"flat", 5.0, 5.0, 5.0), 5.0),
            1.0 / FIXED_ONE
        );
        assert_eq!(instance_list(&asked), "wght=300 WDTH=500");
    }

    #[test]
    fn each_axis_reads_back_applied_contradicted_or_unreported() {
        let wght = axis(b"wght", 100.0, 500.0, 600.0);
        let step = 400.0 / F2DOT14_ONE;
        let classify = |reported, default| classify_axis(&wght, 300.0, reported, default);
        assert_eq!(classify(Some(300.0), 500.0), AxisReadBack::Applied);
        assert_eq!(classify(Some(300.0 + step), 500.0), AxisReadBack::Applied);
        assert_eq!(
            classify(Some(300.0 + 2.0 * step), 500.0),
            AxisReadBack::Contradicted(300.0 + 2.0 * step)
        );
        assert_eq!(classify(None, 500.0), AxisReadBack::Unreported);
        assert_eq!(
            classify(None, 300.0),
            AxisReadBack::Applied,
            "not reported, asked at the default"
        );
        // CoreText clamping a request past the axis is a contradiction.
        assert_eq!(
            classify_axis(&wght, 700.0, Some(600.0), 500.0),
            AxisReadBack::Contradicted(600.0)
        );
    }

    #[test]
    fn an_axis_coretext_leaves_out_is_compared_with_nothing_but_its_default() {
        let axes = [
            axis(b"wght", 100.0, 500.0, 600.0),
            axis(b"WDTH", 1.0, 500.0, 1000.0),
            axis(b"HGHT", 1.0, 500.0, 1000.0),
        ];
        let [wght, wdth, hght] = [axes[0].tag, axes[1].tag, axes[2].tag];
        let defaults = [(wght, 500.0), (wdth, 500.0), (hght, 500.0)];
        let asked = [(wght, 300.0), (wdth, 500.0), (hght, 500.0)];
        // CoreText reports only the axis off its default.
        assert_eq!(
            read_back(&axes, &asked, &defaults, &[(wght, 300.0)]),
            ReadBack::Applied,
            "an omitted default-valued axis is applied"
        );
        assert_eq!(
            read_back(&axes, &asked, &defaults, &[]),
            ReadBack::Unverified(vec![(wght, 300.0)]),
            "an omitted axis asked off its default is unreported, not a mismatch"
        );
        // With no CoreText defaults, the bytes' `fvar` default stands in.
        assert_eq!(
            read_back(&axes, &asked, &[], &[(wght, 300.0)]),
            ReadBack::Applied
        );
        assert_eq!(
            read_back(&axes, &asked, &[], &[]),
            ReadBack::Unverified(vec![(wght, 300.0)])
        );
        // CoreText's own default wins over the bytes' where it lists the axis.
        assert_eq!(
            read_back(&axes, &asked, &[(wdth, 400.0)], &[(wght, 300.0)]),
            ReadBack::Unverified(vec![(wdth, 500.0)])
        );
        // One contradicted axis refuses, whatever else is unreported.
        let wide = [(wght, 300.0), (wdth, 800.0), (hght, 500.0)];
        assert_eq!(
            read_back(&axes, &wide, &defaults, &[(wdth, 800.0)]),
            ReadBack::Unverified(vec![(wght, 300.0)])
        );
        assert_eq!(
            read_back(&axes, &wide, &defaults, &[(wdth, 700.0)]),
            ReadBack::Contradicted(ContradictedAxis {
                tag: wdth,
                asked: 800.0,
                reported: 700.0
            })
        );
    }

    #[test]
    fn a_refused_fetch_caches_nothing_and_evicts_nothing() {
        let mut cache = OutlineCache::new(1);
        cache.get_or_fetch(key(0), || Some(Fetched::Empty));
        assert!(cache.get_or_fetch(key(1), || None).is_none());
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.entries.contains_key(&key(0)), "nothing evicted");
        assert!(!cache.entries.contains_key(&key(1)), "nothing cached");
        assert_eq!(cache.entries.order.len(), 1);
        assert_eq!(
            (cache.hits, cache.misses),
            (0, 1),
            "a refused fetch is neither a hit nor a miss"
        );
        let mut asked = false;
        assert!(
            cache
                .get_or_fetch(key(1), || {
                    asked = true;
                    None
                })
                .is_none()
        );
        assert!(asked, "an uncached key is fetched again");
        assert_eq!((cache.hits, cache.misses), (0, 1));
        cache.get_or_fetch(key(0), || Some(Fetched::Missing));
        assert_eq!((cache.hits, cache.misses), (1, 1));
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
            let (instance, resolved) = CtFace::open(named, 1000.0).expect("installed");
            assert_eq!(resolved, named);
            // CoreText lists the axes off their defaults; the rest are at
            // CoreText's own defaults.
            let reported = instance.variation_values();
            let defaults = instance.axis_defaults();
            let user = |tag: u32| {
                reported
                    .iter()
                    .chain(&defaults)
                    .find(|(reported, _)| *reported == tag)
                    .map(|(_, value)| *value)
                    .expect("CoreText knows every axis")
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

    /// PingFang SC's `wght` axis index, and a handle on its `PingFangSC-Medium`
    /// face at `wght`'s normalized coordinate `normalized`.
    #[cfg(target_os = "macos")]
    fn pingfang_at(font: &peniko::FontData, normalized: i16) -> FontHandle {
        let data = font.data.data();
        let wght = variation_axes(data, table_directory(data, font.index).unwrap())
            .iter()
            .position(|axis| axis.tag == u32::from_be_bytes(*b"wght"))
            .expect("PingFang SC varies its weight");
        let mut coords = vec![0_i16; wght + 1];
        coords[wght] = normalized;
        FontHandle::new(font.clone())
            .with_normalized_coords(&coords)
            .with_identity(Some(Arc::from("PingFangSC-Medium")), false)
    }

    /// A read-back that contradicts the values asked for refuses the
    /// instance: counted once, warned once, its glyphs unavailable, nothing
    /// cached under its key, and the default instance still drawn.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_forced_read_back_mismatch_refuses_the_instance_without_caching() {
        let Some(font) = system_pingfang_sc() else {
            println!("skipped: PingFangUI.ttc is not installed");
            return;
        };
        let name = Some(Arc::from("PingFangSC-Medium"));
        let heavy = pingfang_at(&font, 3276);
        let mut outlines = HvglOutlines::default();
        let warned = REFUSAL_WARNINGS.get();
        READ_BACK_SKEW.set(1.0);
        for _ in 0..3 {
            assert!(matches!(
                outlines.outline(&heavy, 289),
                GlyphOutline::Unavailable
            ));
        }
        READ_BACK_SKEW.set(0.0);
        assert_eq!(outlines.variation_mismatches(), 1, "counted once");
        assert_eq!(REFUSAL_WARNINGS.get() - warned, 1, "warned once");
        assert_eq!(outlines.variation_unverified(), 0);
        assert_eq!(
            (outlines.outline_hits(), outlines.outline_misses()),
            (0, 0),
            "a refused fetch is neither a hit nor a miss"
        );
        assert_eq!(outlines.cached_outlines(), 0, "nothing cached");
        let face = outlines.faces.open.newest_mut().expect("the face is open");
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

    /// A copy whose read-back reports none of its axes contradicts nothing:
    /// the instance is drawn from it as before the read-back existed, counted
    /// once as unverified and warned about once, not refused.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_read_back_reporting_no_axis_draws_the_instance_unverified() {
        let Some(font) = system_pingfang_sc() else {
            println!("skipped: PingFangUI.ttc is not installed");
            return;
        };
        let heavy = pingfang_at(&font, 3276);
        let semibold = pingfang_at(&font, 2000);
        let mut outlines = HvglOutlines::default();
        let warned = UNVERIFIED_WARNINGS.get();
        READ_BACK_DROPPED.set(true);
        let drawn = (0..3)
            .map(|_| matches!(outlines.outline(&heavy, 289), GlyphOutline::Path { .. }))
            .collect::<Vec<_>>();
        let other = matches!(outlines.outline(&semibold, 289), GlyphOutline::Path { .. });
        READ_BACK_DROPPED.set(false);
        assert_eq!(drawn, [true; 3], "drawn from the copy");
        assert!(other, "a second unverified instance draws too");
        assert_eq!(
            outlines.variation_unverified(),
            2,
            "counted once per instance copied"
        );
        assert_eq!(
            UNVERIFIED_WARNINGS.get() - warned,
            1,
            "warned once per open face"
        );
        assert_eq!(outlines.variation_mismatches(), 0, "nothing refused");
        assert_eq!(outlines.cached_outlines(), 2, "one outline per instance");
        assert_eq!((outlines.outline_hits(), outlines.outline_misses()), (2, 2));
        let face = outlines.faces.open.newest_mut().expect("the face is open");
        assert_eq!(face.instances.len(), 2, "both copies kept");
        assert!(face.refused.is_empty());
    }

    /// More refused instances of one face than the face remembers warn once:
    /// a refusal the remembered set evicted is counted again when it is asked
    /// for again, but not warned about again.
    #[cfg(target_os = "macos")]
    #[test]
    fn many_refused_instances_of_one_face_warn_once() {
        let Some(font) = system_pingfang_sc() else {
            println!("skipped: PingFangUI.ttc is not installed");
            return;
        };
        let refused = MAX_INSTANCES_PER_FACE as u32 + 2;
        let instances: Vec<FontHandle> = (1..=refused)
            .map(|step| pingfang_at(&font, (step * 1000) as i16))
            .collect();
        let mut outlines = HvglOutlines::default();
        let warned = REFUSAL_WARNINGS.get();
        READ_BACK_SKEW.set(1.0);
        let unavailable = instances
            .iter()
            .chain(instances.first())
            .filter(|handle| matches!(outlines.outline(handle, 289), GlyphOutline::Unavailable))
            .count();
        READ_BACK_SKEW.set(0.0);
        assert_eq!(unavailable, instances.len() + 1);
        assert_eq!(
            outlines.variation_mismatches(),
            refused + 1,
            "the evicted first refusal counts again"
        );
        assert_eq!(
            REFUSAL_WARNINGS.get() - warned,
            1,
            "warned once per open face"
        );
        let face = outlines.faces.open.newest_mut().expect("the face is open");
        assert_eq!(face.refused.len(), MAX_INSTANCES_PER_FACE);
        assert!(face.refusal_warned);
    }
}
