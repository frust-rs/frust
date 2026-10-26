//! IME composition state: the preedit range the editor itself owns on the
//! desktop path, the platform-owned composing range of the state-sync path,
//! and whether the IME has hidden the caret.
//!
//! The preedit lives in the buffer (the text includes it) and is tracked as
//! a byte range that every buffer edit keeps in step
//! ([`Composition::replaced`]); the platform range is only ever replaced
//! whole, by [`crate::EditOp::ApplyEditingState`], and is reported as-is.

use std::ops::Range;

/// The editor's composition state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Composition {
    /// The desktop path's preedit, a byte range into the buffer.
    pub(crate) preedit: Option<Range<usize>>,
    /// The state-sync path's composing region, as the platform reported it.
    pub(crate) platform: Option<Range<usize>>,
    /// `false` while the IME asked for the caret to be hidden.
    pub(crate) show_cursor: bool,
}

impl Default for Composition {
    fn default() -> Self {
        Self {
            preedit: None,
            platform: None,
            show_cursor: true,
        }
    }
}

impl Composition {
    /// The active composing region: the preedit, else the platform's.
    pub(crate) fn range(&self) -> Option<Range<usize>> {
        self.preedit.clone().or_else(|| self.platform.clone())
    }

    /// Starts or replaces the preedit: it now spans `start..start + len`, and
    /// the caret shows only when the IME placed one.
    pub(crate) fn set_preedit(&mut self, start: usize, len: usize, has_cursor: bool) {
        self.preedit = Some(start..start + len);
        self.show_cursor = has_cursor;
    }

    /// Ends the preedit (committed or discarded), showing the caret again;
    /// returns the range it spanned, or `None` when nothing was composing.
    pub(crate) fn end_preedit(&mut self) -> Option<Range<usize>> {
        let range = self.preedit.take()?;
        self.show_cursor = true;
        Some(range)
    }

    /// Keeps the preedit in step with a buffer edit that replaced `old` with
    /// `new_len` bytes: shifted when the edit is before it, cut when the edit
    /// eats into it, dropped when nothing of it is left.
    pub(crate) fn replaced(&mut self, old: Range<usize>, new_len: usize) {
        if new_len == old.len() {
            return;
        }
        let Some(preedit) = &mut self.preedit else {
            return;
        };
        if preedit.end <= old.start {
            return;
        }
        if preedit.start >= old.end {
            if new_len > old.len() {
                let diff = new_len - old.len();
                *preedit = preedit.start + diff..preedit.end + diff;
            } else {
                let diff = old.len() - new_len;
                *preedit = preedit.start - diff..preedit.end - diff;
            }
            return;
        }
        if new_len < old.len() {
            if preedit.start >= old.start + new_len {
                self.preedit = None;
                return;
            }
            preedit.end = preedit.end.min(old.start + new_len);
        }
    }

    /// The buffer was replaced whole: no preedit survives it.
    pub(crate) fn text_replaced(&mut self) {
        self.preedit = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composing(range: Range<usize>) -> Composition {
        Composition {
            preedit: Some(range),
            ..Composition::default()
        }
    }

    #[test]
    fn an_edit_before_the_preedit_shifts_it() {
        let mut c = composing(4..6);
        c.replaced(0..1, 3);
        assert_eq!(c.preedit, Some(6..8));
        c.replaced(0..3, 0);
        assert_eq!(c.preedit, Some(3..5));
    }

    #[test]
    fn an_edit_after_the_preedit_leaves_it() {
        let mut c = composing(1..3);
        c.replaced(3..3, 2);
        assert_eq!(c.preedit, Some(1..3));
    }

    #[test]
    fn a_deletion_into_the_preedit_cuts_or_drops_it() {
        let mut c = composing(2..6);
        c.replaced(4..8, 0);
        assert_eq!(c.preedit, Some(2..4));
        let mut c = composing(2..6);
        c.replaced(1..8, 0);
        assert_eq!(c.preedit, None);
    }

    #[test]
    fn ending_the_preedit_shows_the_caret_and_reports_its_range() {
        let mut c = Composition::default();
        assert_eq!(c.end_preedit(), None);
        c.set_preedit(3, 2, false);
        assert!(!c.show_cursor);
        assert_eq!(c.range(), Some(3..5));
        assert_eq!(c.end_preedit(), Some(3..5));
        assert!(c.show_cursor);
        c.platform = Some(0..1);
        assert_eq!(
            c.range(),
            Some(0..1),
            "the platform range when not composing"
        );
    }
}
