//! Native ellipsis truncation of a capped paragraph's last visible line.
//!
//! The line's trailing whitespace (a hanging space, a newline) is dropped
//! first, then whole atoms from its logical end until the kept atoms plus the
//! ellipsis fit the layout width. Atoms are bounded by grapheme and cluster
//! boundaries, so the cut never splits a grapheme or a ligature, and nothing
//! is reshaped: the kept atoms keep the glyphs the paragraph was shaped with.
//! When not even the bare ellipsis fits, it is drawn alone (a truncation is
//! always visible); with no width bound the whole line is kept.
//!
//! The ellipsis is a separately shaped `'…'` in the line's style, so its face
//! is the one the font oracle picks for U+2026 in that style: the run's own
//! face whenever it covers the character, as the text's fallback would.

use super::r#break::{BreakReason, BrokenLine};
use crate::context::shape::{Shaped, Whitespace};

/// The character [`crate::TextOverflow::Ellipsis`] draws: U+2026 HORIZONTAL
/// ELLIPSIS, one glyph rather than three periods.
pub(crate) const ELLIPSIS: char = '\u{2026}';

/// Cuts `line` so its atoms plus `ellipsis_advance` fit `max_width`, making it
/// the paragraph's last line.
pub(crate) fn ellipsize(
    shaped: &Shaped,
    line: &mut BrokenLine,
    max_width: Option<f32>,
    ellipsis_advance: f32,
) {
    let start = line.atoms.start;
    let mut end = line.atoms.end;
    while end > start && shaped.atoms[end - 1].whitespace != Whitespace::None {
        end -= 1;
    }
    // `prefix[k]`: the advance of the first `k` kept atoms, summed in order
    // exactly as the breaker sums them.
    let mut prefix = Vec::with_capacity(end - start + 1);
    let mut advance = 0.0f32;
    prefix.push(advance);
    for atom in &shaped.atoms[start..end] {
        advance += atom.advance;
        prefix.push(advance);
    }
    let mut kept = end - start;
    if let Some(width) = max_width {
        while kept > 0 && prefix[kept] + ellipsis_advance > width {
            kept -= 1;
        }
    }
    line.atoms = start..start + kept;
    line.advance = prefix[kept];
    line.reason = BreakReason::None;
    line.num_spaces = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::shape::{AtomFlags, ShapedAtom};

    fn shaped(spec: &str) -> Shaped {
        let atoms = spec
            .chars()
            .enumerate()
            .map(|(i, ch)| {
                let i = i as u32;
                let whitespace = match ch {
                    ' ' => Whitespace::Space,
                    '\n' => Whitespace::Newline,
                    _ => Whitespace::None,
                };
                ShapedAtom {
                    chars: i..i + 1,
                    clusters: i..i + 1,
                    advance: if ch == 'W' { 20.0 } else { 10.0 },
                    flags: AtomFlags::default(),
                    whitespace,
                }
            })
            .collect();
        Shaped {
            atoms,
            ..Shaped::default()
        }
    }

    fn line(atoms: std::ops::Range<usize>) -> BrokenLine {
        BrokenLine {
            atoms,
            reason: BreakReason::Regular,
            advance: 0.0,
            num_spaces: 3,
        }
    }

    #[test]
    fn drops_trailing_whitespace_then_atoms_until_the_ellipsis_fits() {
        let s = shaped("abcWd \n");
        let mut l = line(0..7);
        ellipsize(&s, &mut l, Some(60.0), 8.0);
        assert_eq!(l.atoms, 0..4, "abcW (50) + ellipsis (8) fits 60");
        assert_eq!(l.advance, 50.0);
        assert_eq!(l.reason, BreakReason::None);
        assert_eq!(l.num_spaces, 0);
    }

    #[test]
    fn an_unbounded_line_keeps_everything_but_its_trailing_whitespace() {
        let s = shaped("ab  ");
        let mut l = line(0..4);
        ellipsize(&s, &mut l, None, 8.0);
        assert_eq!(l.atoms, 0..2);
    }

    #[test]
    fn nothing_fits_leaves_the_bare_ellipsis() {
        let s = shaped("abc");
        let mut l = line(0..3);
        ellipsize(&s, &mut l, Some(5.0), 8.0);
        assert_eq!(l.atoms, 0..0);
        assert_eq!(l.advance, 0.0);
    }

    #[test]
    fn a_later_line_keeps_its_start() {
        let s = shaped("ab cd ef");
        let mut l = line(3..8);
        ellipsize(&s, &mut l, Some(30.0), 8.0);
        assert_eq!(
            l.atoms,
            3..5,
            "`cd` (20) + ellipsis fits 30; `cd ` (30) would not"
        );
    }
}
