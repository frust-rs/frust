//! Shaping items: the spans of text over which every shaping-relevant style
//! property is constant.
//!
//! The engine re-segments each item by bidi level and script itself; an item
//! only has to end where a property the shaper takes per item changes — font
//! size, locale, variation settings, letter and word spacing — exactly the
//! points parley 0.11 broke its items at. Family, weight, slant and width are
//! not item properties: font selection reads them per cluster through the
//! per-character style indices built here.

use std::ops::Range;

use crate::style::TextStyle;

/// One item: a run of characters ending at `char_end` (exclusive, counted in
/// characters from the start of the text) whose shaping properties are those
/// of span `style_index`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ItemSpec {
    pub(super) char_end: u32,
    pub(super) style_index: u16,
    pub(super) font_size: f32,
    pub(super) locale: Option<String>,
    pub(super) variations: Vec<([u8; 4], f32)>,
    pub(super) letter_spacing: f32,
    /// Always `0.0`: [`TextStyle`] has no word-spacing knob yet.
    pub(super) word_spacing: f32,
}

impl ItemSpec {
    fn of(style: &TextStyle, style_index: u16, char_end: u32) -> Self {
        Self {
            char_end,
            style_index,
            font_size: style.size,
            locale: style.locale.clone(),
            variations: style.variations.clone(),
            letter_spacing: style.letter_spacing,
            word_spacing: 0.0,
        }
    }

    /// Whether `other` shapes identically to `self` (so no item boundary is needed).
    fn shapes_like(&self, other: &Self) -> bool {
        self.font_size == other.font_size
            && self.locale == other.locale
            && self.variations == other.variations
            && self.letter_spacing == other.letter_spacing
            && self.word_spacing == other.word_spacing
    }
}

/// The items tiling a text plus each character's span (style) index.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Itemized {
    pub(super) items: Vec<ItemSpec>,
    pub(super) char_style_indices: Vec<u16>,
}

impl Itemized {
    /// The index of the item holding character `char_index`.
    pub(super) fn item_at(&self, char_index: u32) -> usize {
        self.items
            .partition_point(|item| item.char_end <= char_index)
            .min(self.items.len().saturating_sub(1))
    }
}

/// Itemizes `text` styled by `spans`: byte ranges in text order that tile it
/// exactly, each with its style (the span's position is its style index).
/// Empty spans are skipped; an empty text has no items.
///
/// # Panics
///
/// When the spans do not tile the text on character boundaries, or there are
/// more than `u16::MAX` of them.
pub(super) fn itemize(text: &str, spans: &[(Range<usize>, &TextStyle)]) -> Itemized {
    assert!(spans.len() <= usize::from(u16::MAX), "too many style spans");
    let mut out = Itemized::default();
    let mut byte_end = 0;
    let mut char_end = 0u32;
    for (index, (range, style)) in spans.iter().enumerate() {
        assert_eq!(range.start, byte_end, "style spans must tile the text");
        assert!(
            range.end >= range.start && text.is_char_boundary(range.end),
            "a style span must end on a character boundary"
        );
        byte_end = range.end;
        if range.is_empty() {
            continue;
        }
        let style_index = u16::try_from(index).expect("checked above");
        let chars = text[range.clone()].chars().count();
        out.char_style_indices
            .extend(std::iter::repeat_n(style_index, chars));
        char_end += u32::try_from(chars).expect("text under 4G characters");
        let item = ItemSpec::of(style, style_index, char_end);
        match out.items.last_mut() {
            Some(last) if last.shapes_like(&item) => last.char_end = char_end,
            _ => out.items.push(item),
        }
    }
    assert_eq!(byte_end, text.len(), "style spans must cover the text");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use peniko::Color;

    fn style(size: f32) -> TextStyle {
        TextStyle::new(size, Color::BLACK)
    }

    #[test]
    fn a_whole_text_style_is_one_item() {
        let text = "Hello 中文";
        let s = style(16.0);
        let itemized = itemize(text, &[(0..text.len(), &s)]);
        assert_eq!(itemized.items.len(), 1);
        assert_eq!(itemized.items[0].char_end, 8);
        assert_eq!(itemized.char_style_indices, vec![0; 8]);
    }

    #[test]
    fn a_size_change_mid_text_splits_and_the_items_tile_the_text() {
        let text = "Hello world";
        let (small, large) = (style(12.0), style(24.0));
        let itemized = itemize(text, &[(0..6, &small), (6..11, &large)]);
        let ends: Vec<u32> = itemized.items.iter().map(|i| i.char_end).collect();
        assert_eq!(
            ends,
            vec![6, 11],
            "items end where the size changes and at the end"
        );
        assert_eq!(itemized.items[1].font_size, 24.0);
        assert_eq!(itemized.items[1].style_index, 1);
        assert_eq!(&itemized.char_style_indices[5..7], &[0, 1]);
        assert_eq!(itemized.item_at(5), 0);
        assert_eq!(itemized.item_at(6), 1);
        assert_eq!(itemized.item_at(10), 1);
    }

    #[test]
    fn a_weight_change_keeps_one_item_but_distinct_style_indices() {
        let text = "ab";
        let regular = style(16.0);
        let bold = TextStyle {
            weight: crate::FontWeight::BOLD,
            ..style(16.0)
        };
        let itemized = itemize(text, &[(0..1, &regular), (1..2, &bold)]);
        assert_eq!(itemized.items.len(), 1, "weight is chosen per cluster");
        assert_eq!(itemized.char_style_indices, vec![0, 1]);
    }

    #[test]
    fn locale_variations_and_spacing_each_split() {
        let base = style(16.0);
        for other in [
            base.clone().with_locale(Some("ja")),
            base.clone().with_variation(*b"wght", 500.0),
            TextStyle {
                letter_spacing: 1.0,
                ..base.clone()
            },
        ] {
            let itemized = itemize("ab", &[(0..1, &base), (1..2, &other)]);
            assert_eq!(itemized.items.len(), 2, "{other:?}");
        }
    }

    #[test]
    fn empty_spans_and_text_are_skipped() {
        let s = style(16.0);
        assert_eq!(itemize("", &[(0..0, &s)]), Itemized::default());
        let itemized = itemize("ab", &[(0..0, &s), (0..2, &s)]);
        assert_eq!(itemized.items.len(), 1);
        assert_eq!(itemized.char_style_indices, vec![1, 1]);
    }

    #[test]
    #[should_panic(expected = "tile")]
    fn a_gap_between_spans_is_refused() {
        let s = style(16.0);
        let _ = itemize("abc", &[(0..1, &s), (2..3, &s)]);
    }
}
