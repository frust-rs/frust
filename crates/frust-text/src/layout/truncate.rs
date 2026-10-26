//! Native ellipsis truncation of a capped paragraph's last visible line.
//!
//! The line is first refilled from the rest of its paragraph: the cut may
//! reach past the line's own soft-wrap point over the atoms that follow it,
//! up to (not including) the next mandatory break or the end of the text, as
//! Skia and Flutter refill an ellipsized last line. The cut is then the
//! longest prefix of that extent whose advance plus the ellipsis fits the
//! layout width, with no whitespace at the cut (a hanging space, a newline,
//! a space the refill crossed). A cut sits on a grapheme boundary that is a
//! cluster boundary too, so it never splits a grapheme or a ligature, but it
//! may fall inside an atom (a long URL is cut character by character, not
//! word by word). Nothing is reshaped: the kept clusters keep the glyphs the
//! paragraph was shaped with.
//!
//! What the cut is today: the shaping engine's atoms are already grapheme and
//! cluster bounded, so the cut is per grapheme = per atom, and the line is
//! refilled from the remaining text one atom at a time. The in-atom branch
//! of [`ellipsize`] guards a future engine producing multi-grapheme atoms; the
//! hand-built grouped-atom tests drive it, and a driver-shaped test asserts
//! the longest-prefix property on real shaping.
//!
//! Accepted limitation: because nothing is reshaped, the last kept glyph keeps
//! its contextual (joining) form and its kerning against the dropped neighbour,
//! which a cursive script (Arabic) shows as a medial form before the ellipsis.
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
/// The kept text is refilled from the atoms after the line up to the next
/// mandatory break (see [`refill_end`]), so `line.atoms` may come out longer
/// than the breaker cut it; every atom after it is dropped by the caller. The
/// cut is the longest prefix of whole graphemes that fits and does not end in
/// whitespace, at a cluster boundary too: a word wider than the line is cut
/// inside the word.
pub(crate) fn ellipsize(
    shaped: &Shaped,
    line: &mut BrokenLine,
    max_width: Option<f32>,
    ellipsis_advance: f32,
) -> u32 {
    let start = line.atoms.start;
    let end = refill_end(shaped, start, line.atoms.end);
    let first_cluster = shaped.atoms.get(start).map_or(0, |a| a.clusters.start);
    // Every position the cut may take, in logical order: (end cluster, kept
    // advance, end atom). Whole atoms sum exactly as the breaker sums them;
    // inside an atom the kept advance is its clusters' advances.
    let mut cuts: Vec<(u32, f32, usize)> = Vec::new();
    let mut atom_advance = 0.0f32;
    for (i, atom) in shaped.atoms[start..end].iter().enumerate() {
        let mut partial = atom_advance;
        for c in atom.clusters.clone() {
            let cluster = &shaped.clusters[c as usize];
            partial += cluster.advance;
            if cluster.whitespace != Whitespace::None {
                continue;
            }
            if c + 1 == atom.clusters.end {
                cuts.push((c + 1, atom_advance + atom.advance, start + i + 1));
                continue;
            }
            let on_grapheme = shaped
                .chars
                .get(cluster.chars.end as usize)
                .is_none_or(|ch| ch.grapheme_start);
            if on_grapheme {
                cuts.push((c + 1, partial, start + i + 1));
            }
        }
        atom_advance += atom.advance;
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

/// The exclusive end atom the cut of a last visible line `start..end` may
/// reach: the line's own end when it ends in a mandatory break, else the rest
/// of its paragraph up to (not including) the next atom ending in one, or the
/// end of the text.
fn refill_end(shaped: &Shaped, start: usize, end: usize) -> usize {
    let atoms = &shaped.atoms;
    let ends_paragraph = end
        .checked_sub(1)
        .filter(|&last| last >= start)
        .and_then(|last| atoms.get(last))
        .is_some_and(|a| a.flags.mandatory_break_after);
    if ends_paragraph {
        return end;
    }
    let mut refill = end.min(atoms.len());
    while atoms
        .get(refill)
        .is_some_and(|a| !a.flags.mandatory_break_after)
    {
        refill += 1;
    }
    refill
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::shape::{AtomFlags, ShapedAtom, ShapedChar, ShapedCluster};

    /// One char, cluster and grapheme per letter (`W` is 20 wide, the rest
    /// 10); `|` ends an atom, otherwise every letter is its own atom. An atom
    /// ending in `\n` has a mandatory break after it.
    fn build(spec: &str, grouped: bool) -> Shaped {
        let mut shaped = Shaped::default();
        let push_atom = |shaped: &mut Shaped, from: u32, to: u32| {
            let advance = shaped.clusters[from as usize..to as usize]
                .iter()
                .map(|c| c.advance)
                .sum();
            let whitespace = shaped.clusters[from as usize].whitespace;
            let ends_in_newline =
                shaped.clusters[to as usize - 1].whitespace == Whitespace::Newline;
            shaped.atoms.push(ShapedAtom {
                chars: from..to,
                clusters: from..to,
                advance,
                flags: AtomFlags {
                    mandatory_break_after: ends_in_newline,
                    ..AtomFlags::default()
                },
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

    #[test]
    fn a_line_that_broke_early_is_refilled_from_the_next_atoms() {
        // The breaker ended the line after `ab ` (the next word overflowed),
        // but the cut reaches into `cdefg`: `ab cde` (60) + 8 fits 70.
        let s = build("ab |cdefg", true);
        let mut l = line(0..1);
        let end = ellipsize(&s, &mut l, Some(70.0), 8.0);
        assert_eq!(l.atoms, 0..2, "the line now ends inside the next atom");
        assert_eq!(end, 6, "`ab cde` is six clusters");
        assert_eq!(l.advance, 60.0);
        assert_eq!(l.reason, BreakReason::None);
    }

    #[test]
    fn the_refill_never_ends_the_cut_on_a_space_it_crossed() {
        // `ab ` (30) + 8 fits 38 but ends in the space; `ab c` (40) does not.
        let s = build("ab |cd", true);
        let mut l = line(0..1);
        let end = ellipsize(&s, &mut l, Some(38.0), 8.0);
        assert_eq!(l.atoms, 0..1);
        assert_eq!(end, 2, "the cut keeps `ab`, its space dropped");
        assert_eq!(l.advance, 20.0);
    }

    #[test]
    fn a_mandatory_break_stops_the_refill() {
        let s = build("ab |cd|\n|efgh", true);
        let mut l = line(0..1);
        let end = ellipsize(&s, &mut l, Some(200.0), 8.0);
        assert_eq!(l.atoms, 0..2, "the refill stops before the newline");
        assert_eq!(end, 5, "`ab cd`, nothing of `efgh`");
        assert_eq!(l.advance, 50.0);

        let mut l = line(0..3);
        let end = ellipsize(&s, &mut l, Some(200.0), 8.0);
        assert_eq!(
            (l.atoms.clone(), end),
            (0..2, 5),
            "a line ending in its own newline is not refilled past it"
        );
    }

    /// A real driver-shaped token (the bundled NotoSans subset through
    /// `ShapeContext`): at every width the cut is the longest whole-cluster
    /// prefix whose advance plus the ellipsis fits, found by brute force.
    #[test]
    fn the_cut_is_the_longest_fitting_prefix_on_driver_shaped_text() {
        use crate::context::shape::ShapeContext;
        use crate::oracle::synthetic::SyntheticBackend;
        use crate::oracle::{Oracle, RenderCapabilities};
        use crate::style::{GenericSlot, TextStyle};
        use peniko::{Blob, Color};

        let blob =
            Blob::from(include_bytes!("../../../../testing/fonts/NotoSans-Subset.ttf").to_vec());
        let mut backend = SyntheticBackend::new("truncate-tests");
        for slot in [GenericSlot::SystemUi, GenericSlot::SansSerif] {
            backend.default_for(slot, &blob);
        }
        let all: String = (' '..='\u{FFFF}').collect();
        backend.cover(&blob, &all);
        let mut oracle = Oracle::new(Box::new(backend), RenderCapabilities::default(), None);
        let text = "https://example.com/a/very/long/path/to/some/resource?query=value";
        let shaped =
            ShapeContext::default().shape(&mut oracle, text, &TextStyle::new(16.0, Color::BLACK));
        let n = shaped.clusters.len();
        assert!(n > 20 && !shaped.atoms.is_empty());
        let total: f32 = shaped.clusters.iter().map(|c| c.advance).sum();
        let ellipsis = 9.0f32;
        let mut width = 1.0f32;
        while width < total + ellipsis {
            let mut l = BrokenLine {
                atoms: 0..shaped.atoms.len().min(1),
                reason: BreakReason::Regular,
                advance: 0.0,
                num_spaces: 0,
            };
            let end = ellipsize(&shaped, &mut l, Some(width), ellipsis) as usize;
            let mut kept = 0usize;
            let mut sum = 0.0f32;
            for (i, c) in shaped.clusters.iter().enumerate() {
                sum += c.advance;
                if sum + ellipsis <= width {
                    kept = i + 1;
                } else {
                    break;
                }
            }
            assert_eq!(end, kept, "width {width}: longest fitting prefix");
            assert!(l.atoms.end <= shaped.atoms.len());
            width += 3.7;
        }
    }
}
