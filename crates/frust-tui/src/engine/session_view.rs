//! The engine-side, pure view-model of a supervised session: its identity and
//! metadata, a bounded ring buffer of retained log lines, and the log-view
//! interaction state (follow-tail vs. an absolute scroll anchor, and a
//! copy-while-scrolling line selection) the [`crate::ui`] log view renders.
//!
//! Everything here is plain data + pure functions — no threads, no tokio, no
//! terminal — so the scroll/selection/eviction invariants are unit-tested
//! without a TTY (see the tests below). The moving part (the tokio supervisor)
//! lives in [`crate::supervise`]; this module only mirrors what it reports.

use std::collections::VecDeque;
use std::path::PathBuf;

use crate::supervise::{SessionId, SessionState};

/// Ring-buffer cap for a session's retained log lines.
///
/// Aligned by value with `frust-drive`'s `LINE_BUFFER_CAP` (the
/// `spawn_streaming` producer-side cap added in TUI2-01): the drive already
/// bounds what it *delivers* to 10_000 lines, and the engine bounds what it
/// *retains* to the same figure so a long-lived session can never grow
/// unbounded memory. The drive const is private to that crate, so this is a
/// deliberate by-value alignment (one number, two layers), not a shared symbol.
pub const LOG_LINE_CAP: usize = 10_000;

/// A bounded, drop-oldest ring of a session's stdout lines, addressed by a
/// stable **absolute** index (the count of lines ever pushed) so a scroll
/// anchor or selection pinned to a line survives both incoming lines and the
/// eviction of older ones.
#[derive(Debug, Clone, Default)]
pub struct LogBuffer {
    lines: VecDeque<String>,
    /// Absolute index of `lines.front()` — advances by one each time the cap
    /// evicts the oldest line.
    base: u64,
}

impl LogBuffer {
    /// Append one line, evicting the oldest if the cap is exceeded.
    pub fn push(&mut self, line: String) {
        self.lines.push_back(line);
        if self.lines.len() > LOG_LINE_CAP {
            self.lines.pop_front();
            self.base += 1;
        }
    }

    /// Whether the buffer holds no lines.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The number of currently-retained lines.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// The absolute index of the oldest retained line.
    pub fn base_index(&self) -> u64 {
        self.base
    }

    /// The absolute index just past the newest line (== total lines ever
    /// pushed). Zero when empty.
    pub fn end_index(&self) -> u64 {
        self.base + self.lines.len() as u64
    }

    /// The line at absolute index `abs`, if it is still retained.
    pub fn get(&self, abs: u64) -> Option<&str> {
        abs.checked_sub(self.base)
            .and_then(|rel| self.lines.get(rel as usize))
            .map(String::as_str)
    }

    /// Iterate `(absolute index, line)` from oldest to newest retained.
    pub fn iter(&self) -> impl Iterator<Item = (u64, &str)> {
        let base = self.base;
        self.lines
            .iter()
            .enumerate()
            .map(move |(i, s)| (base + i as u64, s.as_str()))
    }
}

/// The vertical scroll position of a session's log view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    /// Pinned to the tail — new lines are always shown (the default). The
    /// dirty-frame skip keeps a background session's incoming lines from
    /// forcing a redraw; only the active, followed session repaints on a line.
    Follow,
    /// Frozen with an **absolute** log-line index at the *bottom* of the
    /// viewport. Absolute (not tail-relative) so incoming lines never move the
    /// content the user scrolled up to read — the fdemon 0.6.3
    /// "survives incoming lines" property.
    Anchored(u64),
}

/// An inclusive, whole-line selection over the log, addressed by **absolute**
/// line index so it, too, survives incoming lines (copy-while-scrolling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSelection {
    /// Where the selection was started.
    pub anchor: u64,
    /// The moving end (extended by the selection keys).
    pub cursor: u64,
}

impl LineSelection {
    /// The lower (older) bound of the selected range.
    pub fn lo(&self) -> u64 {
        self.anchor.min(self.cursor)
    }

    /// The upper (newer) bound of the selected range.
    pub fn hi(&self) -> u64 {
        self.anchor.max(self.cursor)
    }

    /// Whether absolute line `idx` falls inside the selection.
    pub fn contains(&self, idx: u64) -> bool {
        idx >= self.lo() && idx <= self.hi()
    }
}

/// A detected log level, used only to colorize a line that carries no explicit
/// ANSI color of its own (see [`crate::ui`]'s log view).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// An error / panic / failure line.
    Error,
    /// A warning line.
    Warn,
}

/// Heuristically classify a (already ANSI-stripped) line's level, or `None` for
/// an ordinary line. Deliberately simple — a substring probe, not a parser — so
/// it never misfires into a wedge; the colorize is a hint, not semantics.
pub fn detect_level(plain: &str) -> Option<LogLevel> {
    let l = plain.to_lowercase();
    if l.contains("error") || l.contains("panic") || l.contains("[e]") {
        Some(LogLevel::Error)
    } else if l.contains("warning") || l.contains("warn") || l.contains("[w]") {
        Some(LogLevel::Warn)
    } else {
        None
    }
}

/// Strip ANSI CSI/SGR escape sequences from `s`, returning the plain text — used
/// for level detection, search matching, and clipboard copy (the raw retained
/// line keeps its ANSI for colored rendering).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC — consume a CSI (`ESC [ … final`) or a two-char escape.
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    // Skip until a final byte in the 0x40..=0x7e range.
                    for f in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&f) {
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The engine's mirror of one supervised session: identity + metadata, the
/// retained log ring, and the per-session log-view state (scroll + selection).
#[derive(Debug, Clone)]
pub struct SessionView {
    /// The supervisor-assigned id this view mirrors.
    pub id: SessionId,
    /// The project the session runs (drives the tab grouping).
    pub project_root: PathBuf,
    /// A short target label for the tab (`desktop`, or a device name).
    pub target_label: String,
    /// The last lifecycle state the supervisor reported (drives the status
    /// glyph).
    pub state: SessionState,
    /// The retained log lines.
    pub log: LogBuffer,
    /// Follow-tail vs. a frozen absolute anchor.
    pub scroll: Scroll,
    /// The copy-while-scrolling selection, if any.
    pub selection: Option<LineSelection>,
}

impl SessionView {
    /// A fresh session view (empty log, following the tail, no selection).
    pub fn new(id: SessionId, project_root: PathBuf, target_label: impl Into<String>) -> Self {
        Self {
            id,
            project_root,
            target_label: target_label.into(),
            state: SessionState::Configuring,
            log: LogBuffer::default(),
            scroll: Scroll::Follow,
            selection: None,
        }
    }

    /// The project's display name (its directory's file name).
    pub fn project_name(&self) -> String {
        self.project_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.project_root.to_string_lossy().into_owned())
    }

    /// Push one incoming log line, then clamp the scroll anchor and selection
    /// up to the (possibly advanced) oldest retained line so neither ever
    /// points below the buffer. A non-evicted anchor/selection is left exactly
    /// where it was — the "survives incoming lines" invariant.
    pub fn push_line(&mut self, line: String) {
        self.log.push(line);
        let base = self.log.base_index();
        if let Scroll::Anchored(b) = self.scroll
            && b < base
        {
            self.scroll = Scroll::Anchored(base);
        }
        if let Some(sel) = self.selection {
            if sel.hi() < base {
                self.selection = None;
            } else if sel.lo() < base {
                self.selection = Some(LineSelection {
                    anchor: sel.anchor.max(base),
                    cursor: sel.cursor.max(base),
                });
            }
        }
    }

    /// Whether the view is currently following the tail.
    pub fn is_following(&self) -> bool {
        matches!(self.scroll, Scroll::Follow)
    }

    /// Scroll up (toward older lines) by `n` lines, freezing the view at an
    /// absolute anchor. Clamped so it never points above the oldest line.
    pub fn scroll_up(&mut self, n: u64) {
        let end = self.log.end_index();
        if end == 0 {
            return;
        }
        let bottom = match self.scroll {
            Scroll::Follow => end - 1,
            Scroll::Anchored(b) => b,
        };
        let new = bottom.saturating_sub(n).max(self.log.base_index());
        self.scroll = Scroll::Anchored(new);
    }

    /// Scroll down (toward newer lines) by `n` lines. Reaching the last line
    /// re-engages follow-tail.
    pub fn scroll_down(&mut self, n: u64) {
        let end = self.log.end_index();
        if end == 0 {
            return;
        }
        let last = end - 1;
        if let Scroll::Anchored(b) = self.scroll {
            let new = b.saturating_add(n);
            self.scroll = if new >= last {
                Scroll::Follow
            } else {
                Scroll::Anchored(new)
            };
        }
    }

    /// Jump to the oldest retained line (top).
    pub fn scroll_to_top(&mut self) {
        if !self.log.is_empty() {
            self.scroll = Scroll::Anchored(self.log.base_index());
        }
    }

    /// Jump back to the tail, re-engaging follow.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll = Scroll::Follow;
    }

    /// Toggle follow-tail: engage it, or freeze at the current tail line.
    pub fn toggle_follow(&mut self) {
        self.scroll = match self.scroll {
            Scroll::Follow => {
                let end = self.log.end_index();
                Scroll::Anchored(end.saturating_sub(1))
            }
            Scroll::Anchored(_) => Scroll::Follow,
        };
    }

    /// Begin a selection anchored at the newest line.
    pub fn begin_selection(&mut self) {
        let end = self.log.end_index();
        if end == 0 {
            return;
        }
        let at = end - 1;
        self.selection = Some(LineSelection {
            anchor: at,
            cursor: at,
        });
    }

    /// Extend the selection cursor toward older lines by `n`.
    pub fn extend_selection_up(&mut self, n: u64) {
        if let Some(sel) = self.selection.as_mut() {
            sel.cursor = sel.cursor.saturating_sub(n).max(self.log.base_index());
        }
    }

    /// Extend the selection cursor toward newer lines by `n`.
    pub fn extend_selection_down(&mut self, n: u64) {
        if let Some(sel) = self.selection.as_mut() {
            let last = self.log.end_index().saturating_sub(1);
            sel.cursor = sel.cursor.saturating_add(n).min(last);
        }
    }

    /// Clear any selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// The artifact paths a completed build session reported, in the order
    /// they were built. A build session (`crate::runner`'s `Effect::LaunchBuild`
    /// enactment) reports each with a `"Built: {path}"` line — the same
    /// prefix `frust-cli`'s own `commands/build.rs::print_artifacts` prints —
    /// so this is a pure, testable parse over the retained log rather than a
    /// dedicated (drive-touching) event kind.
    pub fn built_artifact_paths(&self) -> Vec<&str> {
        const PREFIX: &str = "Built: ";
        self.log
            .iter()
            .filter_map(|(_, line)| line.strip_prefix(PREFIX))
            .collect()
    }

    /// The selected lines joined by newlines, ANSI-stripped, ready for the
    /// clipboard — or `None` if nothing is selected / retained.
    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection?;
        let mut out = String::new();
        let mut any = false;
        for abs in sel.lo()..=sel.hi() {
            if let Some(line) = self.log.get(abs) {
                if any {
                    out.push('\n');
                }
                out.push_str(&strip_ansi(line));
                any = true;
            }
        }
        any.then_some(out)
    }
}

/// Whether `line` matches a case-insensitive substring `query` (ANSI-stripped).
pub fn line_matches(line: &str, query: &str) -> bool {
    strip_ansi(line)
        .to_lowercase()
        .contains(&query.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess() -> SessionView {
        SessionView::new(SessionId(0), PathBuf::from("/tmp/app"), "desktop")
    }

    #[test]
    fn ring_evicts_oldest_and_advances_base() {
        let mut buf = LogBuffer::default();
        for i in 0..(LOG_LINE_CAP as u64 + 3) {
            buf.push(format!("line {i}"));
        }
        assert_eq!(buf.len(), LOG_LINE_CAP);
        assert_eq!(buf.base_index(), 3);
        assert_eq!(buf.end_index(), LOG_LINE_CAP as u64 + 3);
        // The three oldest are gone; absolute indexing still resolves.
        assert_eq!(buf.get(2), None);
        assert_eq!(buf.get(3), Some("line 3"));
        assert_eq!(buf.get(LOG_LINE_CAP as u64 + 2), Some("line 10002"));
    }

    #[test]
    fn anchored_scroll_survives_incoming_lines() {
        let mut s = sess();
        for i in 0..20 {
            s.push_line(format!("line {i}"));
        }
        s.scroll_up(5); // freeze at an absolute anchor
        let Scroll::Anchored(anchor) = s.scroll else {
            panic!("expected anchored");
        };
        // New lines arriving must not move the frozen anchor.
        for i in 20..40 {
            s.push_line(format!("line {i}"));
        }
        assert_eq!(s.scroll, Scroll::Anchored(anchor));
    }

    #[test]
    fn evicted_anchor_clamps_up_to_base() {
        let mut s = sess();
        for i in 0..LOG_LINE_CAP as u64 {
            s.push_line(format!("line {i}"));
        }
        s.scroll = Scroll::Anchored(0); // anchor the very first line
        s.push_line("one more".into()); // evicts line 0
        assert_eq!(s.scroll, Scroll::Anchored(1));
    }

    #[test]
    fn scroll_down_to_bottom_re_engages_follow() {
        let mut s = sess();
        for i in 0..10 {
            s.push_line(format!("line {i}"));
        }
        s.scroll_up(4);
        assert!(!s.is_following());
        s.scroll_down(100);
        assert!(s.is_following());
    }

    #[test]
    fn selection_survives_incoming_lines_then_copies() {
        let mut s = sess();
        for i in 0..10 {
            s.push_line(format!("line {i}"));
        }
        s.selection = Some(LineSelection {
            anchor: 2,
            cursor: 4,
        });
        for i in 10..30 {
            s.push_line(format!("line {i}"));
        }
        // Unchanged (absolute) since none of 2..=4 were evicted.
        assert_eq!(
            s.selection,
            Some(LineSelection {
                anchor: 2,
                cursor: 4
            })
        );
        assert_eq!(s.selected_text().as_deref(), Some("line 2\nline 3\nline 4"));
    }

    #[test]
    fn evicted_selection_clamps_or_drops() {
        let mut s = sess();
        for i in 0..LOG_LINE_CAP as u64 {
            s.push_line(format!("line {i}"));
        }
        s.selection = Some(LineSelection {
            anchor: 0,
            cursor: 1,
        });
        s.push_line("x".into()); // evicts line 0, base becomes 1
        // Whole selection [0,1] still overlaps retained (>=1): clamps up.
        assert_eq!(
            s.selection,
            Some(LineSelection {
                anchor: 1,
                cursor: 1
            })
        );
        // A selection wholly below base drops entirely.
        let base = s.log.base_index();
        s.selection = Some(LineSelection {
            anchor: base,
            cursor: base,
        });
        s.push_line("y".into());
        assert_eq!(s.selection, None);
    }

    #[test]
    fn strip_ansi_removes_sgr_sequences() {
        assert_eq!(strip_ansi("\u{1b}[32mgreen\u{1b}[0m"), "green");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(
            strip_ansi("\u{1b}[1;31mred bold\u{1b}[0m tail"),
            "red bold tail"
        );
    }

    #[test]
    fn detect_level_classifies_common_lines() {
        assert_eq!(
            detect_level("thread panicked at ..."),
            Some(LogLevel::Error)
        );
        assert_eq!(detect_level("error: cannot find"), Some(LogLevel::Error));
        assert_eq!(detect_level("warning: unused import"), Some(LogLevel::Warn));
        assert_eq!(detect_level("Compiling app v0.1.0"), None);
    }

    #[test]
    fn line_matches_is_case_insensitive_and_ansi_blind() {
        assert!(line_matches("\u{1b}[31mERROR here\u{1b}[0m", "error"));
        assert!(!line_matches("all good", "error"));
    }

    #[test]
    fn built_artifact_paths_parses_the_built_prefix() {
        let mut s = sess();
        s.push_line("Building `it.f0x.huddle`…".to_string());
        s.push_line("Built: /tmp/huddle/android/app/build/outputs/apk/release/app.apk".to_string());
        s.push_line(
            "Built: /tmp/huddle/android/app/build/outputs/apk/release/app2.apk".to_string(),
        );
        assert_eq!(
            s.built_artifact_paths(),
            vec![
                "/tmp/huddle/android/app/build/outputs/apk/release/app.apk",
                "/tmp/huddle/android/app/build/outputs/apk/release/app2.apk",
            ]
        );
    }

    #[test]
    fn built_artifact_paths_empty_with_no_built_lines() {
        let mut s = sess();
        s.push_line("hello".to_string());
        assert!(s.built_artifact_paths().is_empty());
    }
}
