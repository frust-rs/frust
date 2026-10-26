//! Greedy line breaking over a [`Shaped`] paragraph's atoms.
//!
//! The rules are parley 0.11.1's `BreakLines::break_next`, applied per atom
//! (the smallest span bounded by both a grapheme and a cluster boundary, so a
//! line never splits a grapheme or a ligature):
//!
//! - an atom that fits (`x + advance <= max`) is appended;
//! - a soft wrap opportunity before an atom is recorded only once the line has
//!   advanced (`x != 0`), so overflowing content never yields an empty line;
//! - an overflowing space hangs: it is appended and the line ends there (its
//!   advance is part of the line but excluded from fitting and alignment as
//!   trailing whitespace, see `super::metrics`);
//! - otherwise the line reverts to the last soft wrap opportunity;
//! - with no soft wrap opportunity on the line, [`OverflowWrap`] decides: under
//!   `Normal` (parley's default, and the only mode a [`crate::TextStyle`] can ask
//!   for today) the word overflows the line; under `Anywhere` the line reverts
//!   to the last atom marked safe to break before (never set by the shaping
//!   driver today, so a dead branch kept for when the engine reports it), else
//!   breaks before the overflowing atom (an emergency break), and only an atom
//!   wider than an empty line overflows it;
//! - an atom ending in a newline ends the line (its advance is not counted).
//!
//! No atom is ever reshaped: a break between atoms reuses their glyphs as
//! shaped for the whole paragraph.

use std::ops::Range;

use crate::context::shape::Shaped;

/// What a line does with a word wider than itself (CSS `overflow-wrap`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OverflowWrap {
    /// The word overflows the line (parley 0.11.1's default, which apps lay
    /// out against: a clipped or ellipsized long token).
    #[default]
    Normal,
    /// The word breaks before its first atom that does not fit.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "no TextStyle knob selects it yet")
    )]
    Anywhere,
}

/// Why a line ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum BreakReason {
    /// The paragraph ended (or the line cap cut it here).
    #[default]
    None,
    /// At a soft wrap opportunity, or after a hanging space.
    Regular,
    /// After a newline.
    Explicit,
    /// Inside a word: no wrap opportunity fit.
    Emergency,
}

/// One line as the breaker cut it: a logical range of atoms.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BrokenLine {
    pub(crate) atoms: Range<usize>,
    pub(crate) reason: BreakReason,
    /// The pen advance of the line: every atom except a terminating newline,
    /// a hanging space included.
    pub(crate) advance: f32,
    /// Spaces a justified line may stretch: those that fitted, less a final
    /// space on a regular break (parley's rule).
    pub(crate) num_spaces: usize,
}

/// A remembered break opportunity: where the line would end, and its state.
#[derive(Clone, Copy)]
struct Mark {
    atom: usize,
    x: f32,
    spaces: usize,
}

/// Breaks `shaped` into lines no wider than `max_advance` (unbounded when
/// `None`), stopping after `max_lines` lines when given; `wrap` decides what a
/// word wider than the line does.
///
/// Always yields at least one line (an empty text is one empty line), and a
/// text ending in a newline gets a final empty line after it, as parley opens
/// one for the caret, unless the line cap stops first.
pub(crate) fn break_lines(
    shaped: &Shaped,
    max_advance: Option<f32>,
    max_lines: Option<usize>,
    wrap: OverflowWrap,
) -> Vec<BrokenLine> {
    let atoms = &shaped.atoms;
    let max = match max_advance {
        Some(width) if shaped.text_len != 0 => width,
        _ => f32::MAX,
    };
    let cap = max_lines.unwrap_or(usize::MAX);
    let mut lines: Vec<BrokenLine> = Vec::new();
    let mut start = 0;
    while lines.len() < cap {
        if start >= atoms.len() {
            let after_newline = lines
                .last()
                .is_some_and(|l| l.reason == BreakReason::Explicit);
            if lines.is_empty() || after_newline {
                lines.push(BrokenLine {
                    atoms: atoms.len()..atoms.len(),
                    reason: BreakReason::None,
                    advance: 0.0,
                    num_spaces: 0,
                });
            }
            break;
        }
        let line = next_line(shaped, start, max, wrap);
        start = line.atoms.end;
        lines.push(line);
    }
    lines
}

/// The line starting at atom `start`.
fn next_line(shaped: &Shaped, start: usize, max: f32, wrap: OverflowWrap) -> BrokenLine {
    let atoms = &shaped.atoms;
    let mut x = 0.0f32;
    let mut spaces = 0usize;
    let mut soft: Option<Mark> = None;
    let mut safe: Option<Mark> = None;
    let mut i = start;
    let mut end = (atoms.len(), BreakReason::None);
    while i < atoms.len() {
        let atom = &atoms[i];
        let here = Mark { atom: i, x, spaces };
        if atom.flags.soft_wrap_opportunity_before {
            if x != 0.0 {
                soft = Some(here);
            }
        } else if x != 0.0 && atom.flags.safe_to_break_before {
            safe = Some(here);
        }
        if atom.flags.mandatory_break_after {
            end = (i + 1, BreakReason::Explicit);
            break;
        }
        let next_x = x + atom.advance;
        let is_space = atom.whitespace.is_space_or_nbsp();
        if next_x <= max {
            x = next_x;
            spaces += usize::from(is_space);
            i += 1;
            continue;
        }
        if is_space {
            // Hang the overflowing space and end the line after it.
            x = next_x;
            end = (i + 1, BreakReason::Regular);
            break;
        }
        let emergency = || match wrap {
            OverflowWrap::Normal => None,
            OverflowWrap::Anywhere => safe
                .or((x != 0.0).then_some(here))
                .map(|m| (m, BreakReason::Emergency)),
        };
        let fallback = soft.map(|m| (m, BreakReason::Regular)).or_else(emergency);
        if let Some((mark, reason)) = fallback {
            x = mark.x;
            spaces = mark.spaces;
            end = (mark.atom, reason);
            break;
        }
        // An atom wider than the whole line: it overflows rather than vanish.
        x = next_x;
        i += 1;
    }
    let (end, reason) = end;
    let end = end.max(start + 1).min(atoms.len());
    if reason == BreakReason::Regular && ends_in_space(shaped, end) {
        spaces = spaces.saturating_sub(1);
    }
    BrokenLine {
        atoms: start..end,
        reason,
        advance: x,
        num_spaces: spaces,
    }
}

/// Whether the cluster before atom `end` is a space or no-break space.
fn ends_in_space(shaped: &Shaped, end: usize) -> bool {
    let Some(atom) = end.checked_sub(1).and_then(|i| shaped.atoms.get(i)) else {
        return false;
    };
    atom.clusters
        .end
        .checked_sub(1)
        .and_then(|c| shaped.clusters.get(c as usize))
        .is_some_and(|c| c.whitespace.is_space_or_nbsp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::shape::{AtomFlags, ShapedAtom, ShapedCluster, Whitespace};

    /// A synthetic paragraph: `words` separated by single spaces, every
    /// letter `letter` wide and every space `space` wide, one cluster per atom.
    fn paragraph(spec: &str, letter: f32, space: f32) -> Shaped {
        let mut shaped = Shaped {
            text_len: spec.len(),
            complete: true,
            ..Shaped::default()
        };
        let mut after_space = false;
        for (i, ch) in spec.chars().enumerate() {
            let i = i as u32;
            let whitespace = match ch {
                ' ' => Whitespace::Space,
                '\n' => Whitespace::Newline,
                _ => Whitespace::None,
            };
            let advance = match whitespace {
                Whitespace::Space => space,
                Whitespace::Newline => 0.0,
                _ => letter,
            };
            shaped.clusters.push(ShapedCluster {
                chars: i..i + 1,
                glyphs: 0..0,
                advance,
                whitespace,
            });
            shaped.atoms.push(ShapedAtom {
                chars: i..i + 1,
                clusters: i..i + 1,
                advance,
                flags: AtomFlags {
                    soft_wrap_opportunity_before: after_space,
                    mandatory_break_after: whitespace == Whitespace::Newline,
                    ..AtomFlags::default()
                },
                whitespace,
            });
            after_space = matches!(whitespace, Whitespace::Space | Whitespace::Newline);
        }
        shaped
    }

    fn ranges(lines: &[BrokenLine]) -> Vec<Range<usize>> {
        lines.iter().map(|l| l.atoms.clone()).collect()
    }

    #[test]
    fn breaks_at_the_last_soft_wrap_opportunity() {
        // "aaa bbb ccc": letters 10, spaces 5 → "aaa " is 35, "aaa bbb" 65.
        let shaped = paragraph("aaa bbb ccc", 10.0, 5.0);
        let lines = break_lines(&shaped, Some(70.0), None, OverflowWrap::Normal);
        assert_eq!(ranges(&lines), vec![0..8, 8..11]);
        assert_eq!(lines[0].reason, BreakReason::Regular);
        assert_eq!(lines[1].reason, BreakReason::None);
        assert_eq!(
            lines[0].advance, 70.0,
            "the space before `ccc` fits exactly"
        );
        assert_eq!(
            lines[0].num_spaces, 1,
            "the line-final space is not stretched"
        );
    }

    #[test]
    fn an_overflowing_space_hangs_and_ends_the_line() {
        let shaped = paragraph("aaa bbb ccc", 10.0, 5.0);
        let lines = break_lines(&shaped, Some(66.0), None, OverflowWrap::Normal);
        assert_eq!(ranges(&lines), vec![0..8, 8..11]);
        assert_eq!(lines[0].advance, 70.0, "the hung space is in the advance");
        assert_eq!(
            lines[0].num_spaces, 0,
            "a hung space is never counted, and a regular line's final space is dropped"
        );
    }

    #[test]
    fn a_word_wider_than_the_line_breaks_in_an_emergency() {
        let shaped = paragraph("aaaaaaaa bb", 10.0, 5.0);
        let lines = break_lines(&shaped, Some(35.0), None, OverflowWrap::Anywhere);
        assert_eq!(ranges(&lines), vec![0..3, 3..6, 6..9, 9..11]);
        assert_eq!(lines[0].reason, BreakReason::Emergency);
        assert_eq!(
            lines[2].reason,
            BreakReason::Regular,
            "`aa ` ends at the soft wrap"
        );
        let wide = break_lines(&shaped, Some(5.0), None, OverflowWrap::Anywhere);
        assert!(
            wide.iter()
                .all(|l| l.atoms.len() == 1 || l.reason == BreakReason::Regular),
            "an atom wider than the line overflows alone: {:?}",
            ranges(&wide)
        );
    }

    #[test]
    fn a_word_wider_than_the_line_overflows_by_default() {
        let shaped = paragraph("aaaaaaaa bb cc", 10.0, 5.0);
        let lines = break_lines(&shaped, Some(35.0), None, OverflowWrap::Normal);
        assert_eq!(ranges(&lines), vec![0..9, 9..12, 12..14]);
        assert_eq!(
            lines[0].advance, 85.0,
            "the word overflows, its space hangs"
        );
        let mut safe = paragraph("aaaaaa", 10.0, 5.0);
        safe.atoms[2].flags.safe_to_break_before = true;
        let lines = break_lines(&safe, Some(45.0), None, OverflowWrap::Normal);
        assert_eq!(
            ranges(&lines),
            vec![0..6],
            "safe-to-break is not a wrap opportunity"
        );
    }

    #[test]
    fn newlines_break_and_a_final_one_opens_an_empty_line() {
        let shaped = paragraph("ab\ncd\n", 10.0, 5.0);
        let lines = break_lines(&shaped, None, None, OverflowWrap::Normal);
        assert_eq!(ranges(&lines), vec![0..3, 3..6, 6..6]);
        assert_eq!(lines[0].reason, BreakReason::Explicit);
        assert_eq!(
            lines[0].advance, 20.0,
            "the newline's advance is not counted"
        );
        assert_eq!(lines[2].reason, BreakReason::None);
        let capped = break_lines(&shaped, None, Some(2), OverflowWrap::Normal);
        assert_eq!(ranges(&capped), vec![0..3, 3..6], "the cap stops first");
    }

    #[test]
    fn unbounded_text_is_one_line_and_empty_text_one_empty_line() {
        let shaped = paragraph("aaa bbb ccc", 10.0, 5.0);
        assert_eq!(
            ranges(&break_lines(&shaped, None, None, OverflowWrap::Normal)),
            vec![0..11]
        );
        assert_eq!(
            ranges(&break_lines(
                &Shaped::default(),
                Some(10.0),
                None,
                OverflowWrap::Normal
            )),
            vec![0..0]
        );
    }

    #[test]
    fn the_line_cap_stops_breaking() {
        let shaped = paragraph("aa bb cc dd", 10.0, 5.0);
        let lines = break_lines(&shaped, Some(30.0), Some(2), OverflowWrap::Normal);
        assert_eq!(ranges(&lines), vec![0..3, 3..6]);
    }

    #[test]
    fn a_safe_to_break_atom_is_the_fallback_before_an_emergency_break() {
        let mut shaped = paragraph("aaaaaa", 10.0, 5.0);
        shaped.atoms[2].flags.safe_to_break_before = true;
        let lines = break_lines(&shaped, Some(45.0), None, OverflowWrap::Anywhere);
        assert_eq!(ranges(&lines), vec![0..2, 2..6]);
        assert_eq!(lines[0].reason, BreakReason::Emergency);
    }
}
