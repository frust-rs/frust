//! A bounded, width-independent shape cache for laid-out text.
//!
//! Adapts SkParagraph's separation of **shaping** (font matching + glyph
//! selection, width-independent) from **line-breaking** (width-dependent) to
//! parley 0.11. A cached [`parley::Layout`] retains its shaped runs; a width
//! change re-runs [`parley::Layout::break_all_lines`] (line-breaking only) on
//! that cached layout rather than re-shaping from scratch.
//!
//! The cache is a bounded LRU (Flutter's SkParagraph precedent: 128 entries)
//! with generation-based (`last_used` tick) eviction — no unbounded growth.
//! No `parley` type appears in this module's public API (scene-layer purity);
//! only [`ShapeCacheStats`] (plain counters) is exported.

use std::collections::HashMap;

use peniko::Brush;

use crate::style::{
    FamilyName, FontFamily, FontStyle, GenericSlot, LineHeight, TextAlign, TextStyle,
    to_parley_align,
};

/// Default cache capacity, mirroring Flutter's SkParagraph LRU paragraph cache
/// (128 entries, keyed by text + styling).
pub(crate) const DEFAULT_CAPACITY: usize = 128;

/// Instrumentation counters for the shape cache — the observable hook the
/// shape-cache tests assert against, and a perf signal otherwise.
///
/// Plain scalar data (no `parley`/`vello`/`wgpu` leak), safe to expose from
/// [`crate::TextContext::shape_cache_stats`].
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct ShapeCacheStats {
    /// Full shaping passes performed (a cache miss — parley `build`).
    pub shapes: u64,
    /// Line-break-only relayouts on an already-shaped cached entry (a width
    /// change with the shaping reused).
    pub line_breaks: u64,
    /// Full reuses: the shaped layout was already broken at the requested
    /// width, so neither shaping nor line-breaking ran.
    pub hits: u64,
    /// LRU evictions performed under capacity pressure.
    pub evictions: u64,
}

/// A hashable, equality-comparable projection of the shaping inputs.
///
/// Shaping is width-independent, so the key deliberately excludes `max_width`
/// — a width change re-breaks the cached shaped layout, it never re-shapes.
/// Float fields are keyed by their bit pattern (`f32::to_bits`) so equality
/// and hashing agree; the style values here (sizes, spacings, sRGB channels)
/// are never `NaN`, so bitwise keying is exact.
///
/// `align` is part of the key even though alignment is applied *after*
/// shaping (during line-breaking, not shaping) — parley bakes the aligned
/// line positions into the cached [`parley::Layout`] itself
/// ([`ShapeCache::get`] mutates the cached layout in place), so two texts
/// differing only in alignment must not collide on the same cache entry: a
/// collision would make the second text's request silently reuse and repaint
/// the first text's aligned line positions.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct ShapeKey {
    text: String,
    family: FamilyBits,
    weight: u32,
    slant: SlantBits,
    size: u32,
    color: [u32; 4],
    letter_spacing: u32,
    line_height: LineHeightBits,
    align: TextAlign,
}

impl ShapeKey {
    /// Projects the shaping inputs (text + style, width-independent) into a
    /// hashable key.
    pub(crate) fn new(text: &str, style: &TextStyle) -> Self {
        Self {
            text: text.to_string(),
            family: FamilyBits::from(&style.family),
            weight: style.weight.value().to_bits(),
            slant: SlantBits::from(style.style),
            size: style.size.to_bits(),
            color: style.color.components.map(f32::to_bits),
            letter_spacing: style.letter_spacing.to_bits(),
            line_height: LineHeightBits::from(style.line_height),
            align: style.align,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum FamilyBits {
    SystemUi,
    Named(Vec<String>),
    NamedWithGeneric(Vec<(Option<String>, Option<u8>)>),
}

impl From<&FontFamily> for FamilyBits {
    fn from(family: &FontFamily) -> Self {
        match family {
            FontFamily::SystemUi => FamilyBits::SystemUi,
            FontFamily::Named(names) => FamilyBits::Named(names.clone()),
            FontFamily::NamedWithGeneric(families) => {
                let bits = families
                    .iter()
                    .map(|f| match f {
                        FamilyName::Named(name) => (Some(name.clone()), None),
                        FamilyName::Generic(slot) => {
                            // Use discriminant as a unique identifier for each generic slot
                            let discriminant = match slot {
                                GenericSlot::Monospace => 0u8,
                                GenericSlot::SansSerif => 1u8,
                                GenericSlot::Serif => 2u8,
                                GenericSlot::SystemUi => 3u8,
                                GenericSlot::Emoji => 4u8,
                            };
                            (None, Some(discriminant))
                        }
                    })
                    .collect();
                FamilyBits::NamedWithGeneric(bits)
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum SlantBits {
    Normal,
    Italic,
    /// The optional oblique angle's bit pattern (`None` = engine default).
    Oblique(Option<u32>),
}

impl From<FontStyle> for SlantBits {
    fn from(style: FontStyle) -> Self {
        match style {
            FontStyle::Normal => SlantBits::Normal,
            FontStyle::Italic => SlantBits::Italic,
            FontStyle::Oblique(angle) => SlantBits::Oblique(angle.map(f32::to_bits)),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum LineHeightBits {
    MetricsRelative(u32),
    FontSizeRelative(u32),
    Absolute(u32),
}

impl From<LineHeight> for LineHeightBits {
    fn from(line_height: LineHeight) -> Self {
        match line_height {
            LineHeight::MetricsRelative(v) => LineHeightBits::MetricsRelative(v.to_bits()),
            LineHeight::FontSizeRelative(v) => LineHeightBits::FontSizeRelative(v.to_bits()),
            LineHeight::Absolute(v) => LineHeightBits::Absolute(v.to_bits()),
        }
    }
}

/// One cached shaped layout plus the width it was last line-broken at.
struct Entry {
    /// The shaped, line-broken, aligned parley layout.
    layout: parley::Layout<Brush>,
    /// The `max_width` `layout` is currently broken at — a differing request
    /// re-breaks (line-breaking only).
    broken_width: Option<f32>,
    /// Monotonic access tick for LRU eviction (higher = more recently used).
    last_used: u64,
}

/// A bounded LRU cache of shaped layouts, keyed width-independently.
pub(crate) struct ShapeCache {
    entries: HashMap<ShapeKey, Entry>,
    capacity: usize,
    tick: u64,
    stats: ShapeCacheStats,
}

impl ShapeCache {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity: capacity.max(1),
            tick: 0,
            stats: ShapeCacheStats::default(),
        }
    }

    /// Looks up a cached shaped layout for `key`, re-breaking it to `max_width`
    /// (line-breaking only, no re-shaping) when the cached break width differs.
    ///
    /// Returns a clone of the laid-out layout on a hit, or `None` on a miss (the
    /// caller must shape, then [`insert`](Self::insert)). Records the hit /
    /// line-break in [`ShapeCacheStats`] and refreshes the entry's LRU tick.
    pub(crate) fn get(
        &mut self,
        key: &ShapeKey,
        max_width: Option<f32>,
    ) -> Option<parley::Layout<Brush>> {
        self.tick += 1;
        let tick = self.tick;
        // Disjoint field borrows: `entry` borrows `self.entries`, the stat
        // bumps touch `self.stats` — the borrow checker allows this within one
        // fn body (no method call re-borrows all of `self`).
        let entry = self.entries.get_mut(key)?;
        entry.last_used = tick;
        if !same_width(entry.broken_width, max_width) {
            entry.layout.break_all_lines(max_width);
            // `key.align` (not a hardcoded `Start`): this is the width-change
            // re-break path context.rs's initial shape+break call doesn't
            // reach, so it must independently re-apply the same alignment or
            // a resized layout silently reverts to `Start` — see `ShapeKey`'s
            // docs.
            entry.layout.align(
                to_parley_align(key.align),
                parley::layout::AlignmentOptions::default(),
            );
            entry.broken_width = max_width;
            self.stats.line_breaks += 1;
        } else {
            self.stats.hits += 1;
        }
        Some(entry.layout.clone())
    }

    /// Inserts a freshly shaped (and broken/aligned) layout, evicting the
    /// least-recently-used entry first when at capacity. Records the shape.
    pub(crate) fn insert(
        &mut self,
        key: ShapeKey,
        layout: parley::Layout<Brush>,
        broken_width: Option<f32>,
    ) {
        self.stats.shapes += 1;
        self.tick += 1;
        if self.entries.len() >= self.capacity
            && !self.entries.contains_key(&key)
            && let Some(evict) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone())
        {
            self.entries.remove(&evict);
            self.stats.evictions += 1;
        }
        self.entries.insert(
            key,
            Entry {
                layout,
                broken_width,
                last_used: self.tick,
            },
        );
    }

    /// The current instrumentation counters.
    pub(crate) fn stats(&self) -> ShapeCacheStats {
        self.stats
    }
}

/// Whether two break widths are equal, comparing `Some` values by bit pattern
/// so `-0.0`/`+0.0` and any incidental representation differences never trigger
/// a needless re-break.
fn same_width(a: Option<f32>, b: Option<f32>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.to_bits() == b.to_bits(),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peniko::Color;

    fn key(text: &str, size: f32) -> ShapeKey {
        ShapeKey::new(text, &TextStyle::new(size, Color::BLACK))
    }

    #[test]
    fn key_ignores_width_but_distinguishes_text_and_style() {
        // Width is not part of the key (shaping is width-independent).
        assert_eq!(key("hello", 16.0), key("hello", 16.0));
        // Text and style still distinguish keys.
        assert_ne!(key("hello", 16.0), key("world", 16.0));
        assert_ne!(key("hello", 16.0), key("hello", 24.0));
    }

    #[test]
    fn key_distinguishes_alignment() {
        // Two texts identical but for alignment must not collide in the
        // cache — see `ShapeKey`'s docs on why `align` is part of the key.
        let start = ShapeKey::new(
            "hello",
            &TextStyle {
                align: TextAlign::Start,
                ..TextStyle::new(16.0, Color::BLACK)
            },
        );
        let center = ShapeKey::new(
            "hello",
            &TextStyle {
                align: TextAlign::Center,
                ..TextStyle::new(16.0, Color::BLACK)
            },
        );
        assert_ne!(start, center);
    }

    #[test]
    fn same_width_compares_optionals_bitwise() {
        assert!(same_width(None, None));
        assert!(same_width(Some(80.0), Some(80.0)));
        assert!(!same_width(Some(80.0), None));
        assert!(!same_width(Some(80.0), Some(81.0)));
    }
}
