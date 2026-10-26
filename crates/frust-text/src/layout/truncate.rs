//! Native ellipsis truncation of a capped paragraph's last visible line.
//!
//! The line's trailing whitespace (a hanging space, a newline) is dropped
//! first, then graphemes from its logical end until the kept text plus the
//! ellipsis fit the layout width. A cut sits on a grapheme boundary that is a
//! cluster boundary too, so it never splits a grapheme or a ligature, but it
//! may fall inside an atom (a long URL is cut character by character, not
//! word by word). Nothing is reshaped: the kept clusters keep the glyphs the
//! paragraph was shaped with.
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

/// Cuts `line` so its kept text plus `ellipsis_advance` fits `max_width`,
/// making it the paragraph's last line, and returns the exclusive end cluster
/// of the kept text (the line's atoms then end at the atom holding it, which
/// may be only partly kept).
///
/// The cut is the longest prefix of whole graphemes that fits, at a cluster
/// boundary too: a word wider than the line is cut inside the word.
pub(crate) fn ellipsize(
    shaped: &Shaped,
    line: &mut BrokenLine,
    max_width: Option<f32>,
    ellipsis_advance: f32,
) -> u32 {
    let start = line.atoms.start;
    let mut end = line.atoms.end;
    while end > start && shaped.atoms[end - 1].whitespace != Whitespace::None {
        end -= 1;
    }
    let first_cluster = shaped.atoms.get(start).map_or(0, |a| a.clusters.start);
    // Every position the cut may take, in logical order: (end cluster, kept
    // advance, end atom). Whole atoms sum exactly as the breaker sums them;
    // inside an atom the kept advance is its clusters' advances.
    let mut cuts: Vec<(u32, f32, usize)> = Vec::new();
    let mut atom_advance = 0.0f32;
    for (i, atom) in shaped.atoms[start..end].iter().enumerate() {
        let mut partial = atom_advance;
        for c in atom.clusters.clone() {
            partial += shaped.clusters[c as usize].advance;
            let char_end = shaped.clusters[c as usize].chars.end as usize;
            let on_grapheme = shaped
                .chars
                .get(char_end)
                .is_none_or(|ch| ch.grapheme_start);
            if on_grapheme && c + 1 < atom.clusters.end {
                cuts.push((c + 1, partial, start + i + 1));
            }
        }
        atom_advance += atom.advance;
        cuts.push((atom.clusters.end, atom_advance, start + i + 1));
    }
    let fits = |advance: f32| max_width.is_none_or(|width| advance + ellipsis_advance <= width);
    let (end_cluster, advance, atoms_end) = match max_width {
        None => cuts.last().copied(),
        Some(_) => cuts.iter().rev().find(|c| fits(c.1)).copied(),
    }
    .unwrap_or((first_cluster, 0.0, start));
    line.atoms = start..atoms_end;
    line.advance = advance;
    line.reason = BreakReason::None;
    line.num_spaces = 0;
    end_cluster
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::shape::{AtomFlags, ShapedAtom, ShapedChar, ShapedCluster};

    /// One char, cluster and grapheme per letter (`W` is 20 wide, the rest
    /// 10); `|` ends an atom, otherwise every letter is its own atom.
    fn build(spec: &str, grouped: bool) -> Shaped {
        let mut shaped = Shaped::default();
        let push_atom = |shaped: &mut Shaped, from: u32, to: u32| {
            let advance = shaped.clusters[from as usize..to as usize]
                .iter()
                .map(|c| c.advance)
                .sum();
            let whitespace = shaped.clusters[from as usize].whitespace;
            shaped.atoms.push(ShapedAtom {
                chars: from..to,
                clusters: from..to,
                advance,
                flags: AtomFlags::default(),
                whitespace,
            });
        };
        for ch in spec.chars().filter(|&c| c != '|') {
            let i = shaped.chars.len() as u32;
            let whitespace = match ch {
                ' ' => Whitespace::Space,
                '\n' => Whitespace::Newline,
                _ => Whitespace::None,
            };
            shaped.chars.push(ShapedChar {
                byte_start: i,
                grapheme_start: true,
                whitespace,
                ..ShapedChar::default()
            });
            shaped.clusters.push(ShapedCluster {
                chars: i..i + 1,
                glyphs: 0..0,
                advance: if ch == 'W' { 20.0 } else { 10.0 },
                whitespace,
            });
        }
        if grouped {
            let mut at = 0u32;
            for word in spec.split('|') {
                let n = word.chars().count() as u32;
                push_atom(&mut shaped, at, at + n);
                at += n;
            }
        } else {
            for i in 0..shaped.chars.len() as u32 {
                push_atom(&mut shaped, i, i + 1);
            }
        }
        shaped
    }

    fn shaped(spec: &str) -> Shaped {
        build(spec, false)
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
        let end = ellipsize(&s, &mut l, Some(60.0), 8.0);
        assert_eq!(l.atoms, 0..4, "abcW (50) + ellipsis (8) fits 60");
        assert_eq!(end, 4);
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
        let end = ellipsize(&s, &mut l, Some(30.0), 8.0);
        assert_eq!(
            l.atoms,
            3..5,
            "`cd` (20) + ellipsis fits 30; `cd ` (30) would not"
        );
        assert_eq!(end, 5);
    }

    #[test]
    fn an_atom_is_kept_partially_at_grapheme_granularity() {
        // Atoms `/very` (50) and `/long` (50): the cut at 40 keeps `/ve`.
        let s = build("/very|/long", true);
        let mut l = line(0..2);
        let end = ellipsize(&s, &mut l, Some(40.0), 8.0);
        assert_eq!(l.atoms, 0..1, "the cut falls inside the first atom");
        assert_eq!(end, 3, "`/ve` is three clusters");
        assert_eq!(l.advance, 30.0);
    }

    #[test]
    fn a_cut_never_lands_inside_a_grapheme() {
        // `a` + combining mark share a grapheme, so the cut skips between them.
        let mut s = build("xaby", true);
        s.chars[2].grapheme_start = false;
        let mut l = line(0..1);
        let end = ellipsize(&s, &mut l, Some(28.0), 8.0);
        assert_eq!(
            end, 1,
            "`xa` (20) + 8 fits, but `a` and `b` share a grapheme"
        );
        assert_eq!(l.advance, 10.0);
    }
}
