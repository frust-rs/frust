//! Per-line visual order (UAX #9 rule L2).
//!
//! A line's items (maximal same-run atom spans) arrive in logical order with
//! the bidi level of their run; L2 reverses every maximal sequence at or above
//! each level, from the highest level down to the lowest odd one. Within an
//! odd-level (right-to-left) item the clusters are then walked last to first
//! by the glyph emitter; a cluster's own glyphs are already in paint order.
//!
//! As in parley 0.11.1, the run levels are used as resolved for the paragraph:
//! rule L1 (trailing whitespace back to the paragraph level) is not applied.

/// Reorders `items` (logical order in, visual order out) by the bidi level
/// `level` reports for each.
pub(crate) fn reorder<T>(items: &mut [T], level: impl Fn(&T) -> u8) {
    if items.len() < 2 {
        return;
    }
    let mut max = 0u8;
    let mut min_odd = u8::MAX;
    for item in items.iter() {
        let l = level(item);
        max = max.max(l);
        if l % 2 == 1 {
            min_odd = min_odd.min(l);
        }
    }
    if min_odd == u8::MAX {
        return;
    }
    for current in (min_odd..=max).rev() {
        let mut i = 0;
        while i < items.len() {
            if level(&items[i]) < current {
                i += 1;
                continue;
            }
            let mut end = i + 1;
            while end < items.len() && level(&items[end]) >= current {
                end += 1;
            }
            items[i..end].reverse();
            i = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visual(levels: &[u8]) -> Vec<usize> {
        let mut items: Vec<(usize, u8)> = levels.iter().copied().enumerate().collect();
        reorder(&mut items, |&(_, l)| l);
        items.into_iter().map(|(i, _)| i).collect()
    }

    #[test]
    fn an_embedded_rtl_span_stays_in_place_between_ltr_spans() {
        // 'abc אבג def': LTR, RTL, LTR items keep their order; only the
        // Hebrew item's own clusters run right to left.
        assert_eq!(visual(&[0, 1, 0]), vec![0, 1, 2]);
    }

    #[test]
    fn consecutive_rtl_items_swap() {
        assert_eq!(visual(&[1, 1]), vec![1, 0]);
        assert_eq!(visual(&[0, 1, 1, 0]), vec![0, 2, 1, 3]);
    }

    #[test]
    fn an_ltr_number_inside_rtl_text_keeps_its_own_direction() {
        // RTL paragraph (level 1) with an LTR number (level 2) inside: the
        // whole line reverses, then the number reverses back.
        assert_eq!(visual(&[1, 2, 2, 1]), vec![3, 1, 2, 0]);
    }

    #[test]
    fn all_ltr_is_untouched() {
        assert_eq!(visual(&[0, 0, 0]), vec![0, 1, 2]);
        assert_eq!(
            visual(&[2, 2]),
            vec![0, 1],
            "no odd level, nothing reverses"
        );
    }
}
