//! A bounded, width-independent shape cache for laid-out text.
//!
//! Adapts SkParagraph's separation of **shaping** (font matching + glyph
//! selection, width-independent) from **line-breaking** (width-dependent).
//! One LRU machinery ([`ShapeCache`]) serves two value types:
//!
//! - [`LaidOut`], what [`crate::TextContext::layout`] caches: the shaping
//!   driver's [`Shaped`] paragraph plus the line table it was last broken
//!   into. A width change re-runs the line layout (break, metrics, align) on
//!   the cached [`Shaped`] — never re-shaping — and keeps the new table.
//! - the driver's bare [`Shaped`] (`crate::context::shape`): width-free by
//!   construction, so a hit is the whole answer and analysis/shaping are
//!   skipped.
//!
//! Each cache is a bounded LRU (Flutter's SkParagraph precedent: 128 entries)
//! with generation-based (`last_used` tick) eviction — no unbounded growth.
//! No shaping-engine type appears in this module's public API (scene-layer
//! purity); only [`ShapeCacheStats`] (plain counters) is exported.

use std::collections::HashMap;
use std::sync::Arc;

use crate::context::shape::Shaped;
use crate::layout::{LineTable, Paragraph, TextLayout, lay_out};
use crate::style::{
    FamilyName, FontFamily, FontStyle, GenericSlot, LineHeight, TextAlign, TextStyle,
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
    /// Full shaping passes performed (a cache miss: the shaping driver's
    /// analysis + shaping).
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
/// shaping (during line layout, not shaping) — the cached [`LaidOut`] keeps
/// its aligned line table ([`LayoutCache::get`] replaces it in place on a
/// width change), so two texts differing only in alignment must not collide
/// on the same cache entry: a collision would make the second text's request
/// silently reuse and repaint the first text's aligned line positions.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct ShapeKey {
    text: String,
    family: FamilyBits,
    weight: u32,
    slant: SlantBits,
    width: u32,
    variations: Vec<([u8; 4], u32)>,
    locale: Option<String>,
    app_locale: Option<Arc<str>>,
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
            width: style.width.to_bits(),
            variations: style
                .variations
                .iter()
                .map(|&(tag, value)| (tag, value.to_bits()))
                .collect(),
            locale: style.locale.clone(),
            // The app default locale decides untagged Han fallback, so a style without a
            // locale carries it; a style with one is independent of it.
            app_locale: match style.locale {
                Some(_) => None,
                None => crate::default_locale(),
            },
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

/// The layout path's cached value: a shaped paragraph, its styles, and the
/// line table it was last broken into, at `broken_width`.
pub(crate) struct LaidOut {
    text: Arc<str>,
    shaped: Arc<Shaped>,
    styles: Arc<[TextStyle]>,
    lines: Arc<LineTable>,
    /// The `max_width` `lines` was broken at — a differing request re-breaks
    /// (line layout only).
    broken_width: Option<f32>,
}

impl LaidOut {
    /// Lays `shaped` (`text` in `style`) out at `max_width`.
    pub(crate) fn new(
        text: &str,
        shaped: Arc<Shaped>,
        style: &TextStyle,
        max_width: Option<f32>,
    ) -> Self {
        let styles: Arc<[TextStyle]> = Arc::from([style.clone()]);
        let lines = Arc::new(lay_out(
            &Paragraph::new(&shaped, &styles, None),
            max_width,
            None,
        ));
        Self {
            text: Arc::from(text),
            shaped,
            styles,
            lines,
            broken_width: max_width,
        }
    }

    /// A [`TextLayout`] sharing this value's shaping and line table.
    pub(crate) fn layout(&self) -> TextLayout {
        TextLayout::new(
            Arc::clone(&self.text),
            Arc::clone(&self.shaped),
            Arc::clone(&self.styles),
            self.broken_width,
            Arc::clone(&self.lines),
        )
    }
}

/// One cached value and its LRU stamp.
struct Entry<V> {
    value: V,
    /// Monotonic access tick for LRU eviction (higher = more recently used).
    last_used: u64,
}

/// A bounded LRU cache of shaping results, keyed width-independently.
pub(crate) struct ShapeCache<V> {
    entries: HashMap<ShapeKey, Entry<V>>,
    capacity: usize,
    tick: u64,
    stats: ShapeCacheStats,
}

/// The layout path's cache.
pub(crate) type LayoutCache = ShapeCache<LaidOut>;

/// The shaping driver's cache: one shared [`Shaped`] per (text, style).
pub(crate) type ShapedCache = ShapeCache<Arc<Shaped>>;

impl<V> ShapeCache<V> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity: capacity.max(1),
            tick: 0,
            stats: ShapeCacheStats::default(),
        }
    }

    /// The entry for `key`, its LRU tick refreshed; records nothing.
    fn touch(&mut self, key: &ShapeKey) -> Option<&mut V> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.entries.get_mut(key)?;
        entry.last_used = tick;
        Some(&mut entry.value)
    }

    /// Stores a freshly shaped value, evicting the least-recently-used entry
    /// first when at capacity. Records the shape.
    fn store(&mut self, key: ShapeKey, value: V) {
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
                value,
                last_used: self.tick,
            },
        );
    }

    /// The current instrumentation counters.
    pub(crate) fn stats(&self) -> ShapeCacheStats {
        self.stats
    }
}

impl LayoutCache {
    /// Looks up a cached layout for `key`, re-running the line layout on its
    /// cached [`Shaped`] at `max_width` (no re-shaping) when the cached break
    /// width differs.
    ///
    /// Returns the layout on a hit, or `None` on a miss (the caller must
    /// shape, then [`insert`](Self::insert)). Records the hit / line-break in
    /// [`ShapeCacheStats`] and refreshes the entry's LRU tick.
    pub(crate) fn get(&mut self, key: &ShapeKey, max_width: Option<f32>) -> Option<TextLayout> {
        let entry = self.touch(key)?;
        let rebroken = !same_width(entry.broken_width, max_width);
        if rebroken {
            // The cached styles carry `key.align`, so the re-broken table is
            // aligned exactly as the initial layout was — see `ShapeKey`.
            let paragraph = Paragraph::new(&entry.shaped, &entry.styles, None);
            entry.lines = Arc::new(lay_out(&paragraph, max_width, None));
            entry.broken_width = max_width;
        }
        let layout = entry.layout();
        if rebroken {
            self.stats.line_breaks += 1;
        } else {
            self.stats.hits += 1;
        }
        Some(layout)
    }

    /// Inserts a freshly shaped (and laid out) value, evicting the
    /// least-recently-used entry first when at capacity. Records the shape.
    pub(crate) fn insert(&mut self, key: ShapeKey, value: LaidOut) {
        self.store(key, value);
    }
}

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the line layout over `Shaped` is the driver's first production caller"
    )
)]
impl ShapedCache {
    /// The cached [`Shaped`] for `key`, recording a hit; `None` on a miss
    /// (the caller shapes, then [`insert_shaped`](Self::insert_shaped)).
    pub(crate) fn get_shaped(&mut self, key: &ShapeKey) -> Option<Arc<Shaped>> {
        let shaped = Arc::clone(self.touch(key)?);
        self.stats.hits += 1;
        Some(shaped)
    }

    /// Inserts a freshly shaped [`Shaped`], recording the shape.
    pub(crate) fn insert_shaped(&mut self, key: ShapeKey, shaped: Arc<Shaped>) {
        self.store(key, shaped);
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
        let _guard = crate::locale::test_support::DefaultLocale::set(None);
        // Width is not part of the key (shaping is width-independent).
        assert_eq!(key("hello", 16.0), key("hello", 16.0));
        // Text and style still distinguish keys.
        assert_ne!(key("hello", 16.0), key("world", 16.0));
        assert_ne!(key("hello", 16.0), key("hello", 24.0));
    }

    #[test]
    fn key_distinguishes_alignment() {
        let _guard = crate::locale::test_support::DefaultLocale::set(None);
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
    fn key_distinguishes_width_variations_and_locale() {
        let _guard = crate::locale::test_support::DefaultLocale::set(None);
        let base = TextStyle::new(16.0, Color::BLACK);
        let plain = ShapeKey::new("hello", &base);
        for varied in [
            base.clone().with_width(0.875),
            base.clone().with_variation(*b"wght", 600.0),
            base.clone().with_locale(Some("ja")),
        ] {
            assert_ne!(plain, ShapeKey::new("hello", &varied), "{varied:?}");
        }
    }

    #[test]
    fn key_follows_the_app_default_locale_only_without_a_style_locale() {
        use crate::locale::test_support::DefaultLocale;
        let base = TextStyle::new(16.0, Color::BLACK);
        let tagged = base.clone().with_locale(Some("ko"));
        let _guard = DefaultLocale::set(None);
        let (plain_none, tagged_none) =
            (ShapeKey::new("hello", &base), ShapeKey::new("x", &tagged));
        crate::set_default_locale(Some("ja"));
        let plain_ja = ShapeKey::new("hello", &base);
        assert_ne!(plain_none, plain_ja);
        assert_eq!(
            plain_ja,
            ShapeKey::new("hello", &base),
            "a uniform default is stable"
        );
        assert_eq!(
            tagged_none,
            ShapeKey::new("x", &tagged),
            "an explicit locale ignores it"
        );
        crate::set_default_locale(Some("zh-Hant"));
        assert_ne!(plain_ja, ShapeKey::new("hello", &base));
        crate::set_default_locale(Some("ja"));
        assert_eq!(
            plain_ja,
            ShapeKey::new("hello", &base),
            "switching back reuses entries"
        );
    }

    #[test]
    fn the_shaped_cache_counts_hits_shapes_and_evictions() {
        let _guard = crate::locale::test_support::DefaultLocale::set(None);
        let mut cache = ShapedCache::new(2);
        let shaped = Arc::new(Shaped::default());
        assert!(cache.get_shaped(&key("a", 16.0)).is_none());
        cache.insert_shaped(key("a", 16.0), Arc::clone(&shaped));
        cache.insert_shaped(key("b", 16.0), Arc::clone(&shaped));
        assert!(cache.get_shaped(&key("a", 16.0)).is_some());
        cache.insert_shaped(key("c", 16.0), shaped);
        assert!(cache.get_shaped(&key("b", 16.0)).is_none(), "LRU evicted");
        let stats = cache.stats();
        assert_eq!((stats.shapes, stats.hits, stats.evictions), (3, 1, 1));
        assert_eq!(stats.line_breaks, 0, "a shaped value is width-free");
    }

    #[test]
    fn same_width_compares_optionals_bitwise() {
        let _guard = crate::locale::test_support::DefaultLocale::set(None);
        assert!(same_width(None, None));
        assert!(same_width(Some(80.0), Some(80.0)));
        assert!(!same_width(Some(80.0), None));
        assert!(!same_width(Some(80.0), Some(81.0)));
    }
}
