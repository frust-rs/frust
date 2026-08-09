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

use super::logstyle::{
    LevelFilter, LineMeta, LogLevel, PanicBlock, PanicTracker, classify_level, classify_source,
    now_hms,
};
use super::perf::PerfPanel;
use crate::supervise::{PhaseLabel, SessionId, SessionState};

/// Ring-buffer cap for a session's retained log lines.
///
/// Aligned by value with `frust-drive`'s `LINE_BUFFER_CAP` (the
/// `spawn_streaming` producer-side cap): the drive already
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
    /// Per-line classification metadata (level/source/timestamp/panic-role),
    /// kept in lockstep with `lines` — same push/evict cycle, same absolute
    /// indexing — so a render pass never re-derives what was already
    /// computed once at [`SessionView::push_line`].
    meta: VecDeque<LineMeta>,
    /// Absolute index of `lines.front()` — advances by one each time the cap
    /// evicts the oldest line.
    base: u64,
}

impl LogBuffer {
    /// Append one line + its precomputed metadata, evicting the oldest pair
    /// if the cap is exceeded.
    pub fn push(&mut self, line: String, meta: LineMeta) {
        self.lines.push_back(line);
        self.meta.push_back(meta);
        if self.lines.len() > LOG_LINE_CAP {
            self.lines.pop_front();
            self.meta.pop_front();
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

    /// The precomputed metadata for the line at absolute index `abs`, if it
    /// is still retained.
    pub fn meta(&self, abs: u64) -> Option<&LineMeta> {
        abs.checked_sub(self.base)
            .and_then(|rel| self.meta.get(rel as usize))
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
    ///
    /// **Invariant:** an `Anchored` index always names a line that has
    /// actually been pushed — never a placeholder on an empty log. Every
    /// session starts `Follow` ([`SessionView::new`]); only a user scroll
    /// (which anchors an entry of the real visible sequence, see
    /// [`SessionView::visible_indices`]) or [`SessionView::toggle_follow`]
    /// (which reads the buffer's real tail) ever produces an `Anchored`
    /// value. The anchored line may later stop being *visible* — a filter
    /// change or a fold can hide it — which is why every reader resolves it
    /// through [`SessionView::bottom_pos`] rather than trusting it directly.
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
    /// Cumulative count of output lines the supervisor dropped because its
    /// bounded engine channel was full (drop-newest overflow — see
    /// [`crate::supervise`]). `0` for a healthy session; a non-zero value means
    /// the log is missing lines the consumer couldn't keep up with, which the
    /// UI can surface. Mirrors `frust_drive::process::LineReceiver::dropped_lines`.
    pub dropped: u64,
    /// The parsed `frust-perf` sparkline/stats panel for this session —
    /// fed one line at a time from [`Self::push_line`].
    pub perf: PerfPanel,
    /// The active minimum-level cutoff for this session's log view
    /// (workbook §B11's filter chip) — `All` by default (zero-noise).
    pub level_filter: LevelFilter,
    /// The Rust panic/backtrace fold-block state machine + block list for
    /// this session, fed one line at a time from [`Self::push_line`].
    panic_tracker: PanicTracker,
    /// The latest parsed build/install/launch phase label
    /// (`crate::supervise::progress::phase_from_output_line`, workbook
    /// §B10's transient status line) — `None` before any marker line has
    /// been seen, or once the session has left its transient window.
    /// [`crate::engine::update::on_session_event`] is the sole clearer: the
    /// moment a `SessionState` event reports a non-`Building`/`Installing`
    /// state, it resets this to `None` regardless of send ordering, so a
    /// mis-parse (or a `Phase` event racing a `State` event through the
    /// bounded channel) can never linger past a real state advance — see
    /// `crate::supervise::progress`'s module docs.
    pub current_phase: Option<PhaseLabel>,
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
            dropped: 0,
            perf: PerfPanel::default(),
            level_filter: LevelFilter::default(),
            panic_tracker: PanicTracker::default(),
            current_phase: None,
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
    /// where it was — the "survives incoming lines" invariant. The line's
    /// level/source/panic-role are classified once here (never at render —
    /// see [`crate::engine::logstyle`]) against the real wall clock
    /// ([`now_hms`]); see [`Self::push_line_at`] for a deterministic variant.
    pub fn push_line(&mut self, line: String) {
        self.push_line_at(line, now_hms());
    }

    /// [`Self::push_line`] with an explicit `timestamp` instead of the real
    /// wall clock — the primitive `push_line` delegates to, and the seam
    /// tests (and snapshot fixtures) use for reproducible output.
    pub fn push_line_at(&mut self, line: String, timestamp: impl Into<String>) {
        let plain = strip_ansi(&line);
        self.perf.ingest(&plain);
        let abs = self.log.end_index();
        let role = self.panic_tracker.feed(abs, &plain);
        let (source, source_prefix_strip) = classify_source(&plain);
        // A panic/backtrace-block line is always error-severity regardless of
        // its own text — the level filter treats the whole block as one unit
        // (see `logstyle::LineRole::is_panic_related`).
        let level = if role.is_panic_related() {
            LogLevel::Error
        } else {
            classify_level(&plain)
        };
        let meta = LineMeta {
            level,
            source,
            timestamp: timestamp.into(),
            role,
            source_prefix_strip,
        };
        self.log.push(line, meta);
        let base = self.log.base_index();
        self.panic_tracker.evict_before(base);
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

    /// The precomputed metadata for the line at absolute index `abs`, if
    /// still retained.
    pub fn line_meta(&self, abs: u64) -> Option<&LineMeta> {
        self.log.meta(abs)
    }

    /// The panic block (if any) whose foldable body contains `abs`.
    pub fn panic_block_covering(&self, abs: u64) -> Option<&PanicBlock> {
        self.panic_tracker.block_covering(abs)
    }

    /// Whether the panic block starting at absolute index `block_start` is
    /// currently collapsed.
    pub fn is_fold_collapsed(&self, block_start: u64) -> bool {
        self.panic_tracker.is_collapsed(block_start)
    }

    /// Toggle the fold state of the panic block starting at `block_start`
    /// (mouse click on its `▶ n frames…` affordance row). A no-op — but
    /// still returns `false` — on a stale/unknown id.
    pub fn toggle_fold(&mut self, block_start: u64) -> bool {
        self.panic_tracker.toggle(block_start)
    }

    /// Toggle the fold state of whichever panic block is nearest the current
    /// scroll position (`z` — no mouse target under the cursor to aim at).
    /// The "cursor" is the selection cursor when a selection is active, else
    /// the bottom-of-viewport line (the tail when following). Returns the
    /// toggled block's id, if any block exists yet.
    pub fn toggle_nearest_fold(&mut self) -> Option<u64> {
        let near = self
            .selection
            .map(|s| s.cursor)
            .unwrap_or_else(|| match self.scroll {
                Scroll::Follow => self.log.end_index().saturating_sub(1),
                Scroll::Anchored(b) => b,
            });
        self.panic_tracker.toggle_nearest(near)
    }

    /// Step the level filter `delta` positions (`l`/`L`).
    pub fn cycle_level_filter(&mut self, delta: isize) {
        self.level_filter = self.level_filter.cycle(delta);
    }

    /// Jump the level filter directly to `filter` (a filter-chip segment
    /// click).
    pub fn set_level_filter(&mut self, filter: LevelFilter) {
        self.level_filter = filter;
    }

    /// The effective level a `filter` cutoff should test for the line at
    /// `abs` — `None` for an evicted/unknown line (never filtered out by a
    /// missing classification; the caller's `vis` list already only holds
    /// retained indices).
    pub fn effective_level(&self, abs: u64) -> Option<LogLevel> {
        self.log.meta(abs).map(|m| m.level)
    }

    /// Whether any panic/backtrace block has been detected yet — the
    /// zero-noise gate for the log status row's `z` fold keyhint (mirrors
    /// `PerfPanel::has_data`'s pattern for the `t` perf keyhint).
    pub fn has_panic_blocks(&self) -> bool {
        !self.panic_tracker.blocks().is_empty()
    }

    /// Whether at least one panic/backtrace block is currently collapsed —
    /// the stricter gate the log status row's `z` fold keyhint prioritizes
    /// on when the bottom row is tight for space (a block that's been fully
    /// expanded has no `▶ n frames…` row to re-collapse, so its keyhint is
    /// less urgent than one still folded).
    pub fn has_collapsed_panic_blocks(&self) -> bool {
        self.panic_tracker.blocks().iter().any(|b| b.collapsed)
    }

    /// Whether the view is currently following the tail.
    pub fn is_following(&self) -> bool {
        matches!(self.scroll, Scroll::Follow)
    }

    /// The absolute indices of the log lines the view actually shows, oldest
    /// first — the **visible sequence** every scroll step and every rendered
    /// row is measured in:
    ///
    /// - retained lines only (an evicted index never appears);
    /// - passing this session's [`Self::level_filter`];
    /// - passing the caller's committed free-text `filter` (`None` = no
    ///   text filter);
    /// - each *collapsed* panic block contributing exactly **one** entry —
    ///   its oldest still-visible body line — because the log view draws it
    ///   as one `▶ n frames…` row. An expanded block contributes one entry
    ///   per body line, as it draws one row per line.
    ///
    /// A panic/backtrace-block line is always classified
    /// [`LogLevel::Error`] regardless of its own text
    /// ([`Self::push_line_at`]), so a block's body passes or fails the level
    /// cutoff as one unit, never partially.
    ///
    /// `filter` is a parameter rather than session state because the
    /// free-text search filter is *global* engine state
    /// (`crate::engine::AppState::search`), shared by every session's log
    /// view, while the level filter and the folds are per-session — see
    /// [`crate::engine::update`]'s log-scroll arms, which supply it.
    ///
    /// Cost: O(retained lines · log blocks) — each line also pays a
    /// [`PanicTracker::block_covering`] lookup (binary search over the
    /// panic-block list, itself capped at [`super::logstyle::MAX_TRACKED_BLOCKS`]),
    /// not the O(blocks) linear scan an earlier revision of this walk paid
    /// per line (the multiplier a crash-looping app — many concurrent
    /// backtrace-less panic headers within one ring window — could otherwise
    /// drive unbounded). Plus an ANSI-strip + lowercase per line while a text
    /// filter is active. Paid only on a render or a scroll event —
    /// [`Self::push_line`] never builds it, so ingesting output stays O(1).
    pub fn visible_indices(&self, filter: Option<&str>) -> Vec<u64> {
        let mut out = Vec::new();
        // Absolute index through which a collapsed block already represented
        // by an emitted entry still runs; its remaining body lines are
        // absorbed into that one entry.
        let mut absorb_through: Option<u64> = None;
        for (abs, line) in self.log.iter() {
            if let Some(end) = absorb_through {
                if abs <= end {
                    continue;
                }
                absorb_through = None;
            }
            let level_ok = self
                .effective_level(abs)
                .is_none_or(|lvl| self.level_filter.allows(lvl));
            if !level_ok {
                continue;
            }
            if filter.is_some_and(|q| !line_matches(line, q)) {
                continue;
            }
            if let Some(block) = self.panic_block_covering(abs)
                && self.is_fold_collapsed(block.start)
            {
                absorb_through = Some(block.end);
            }
            out.push(abs);
        }
        out
    }

    /// The position within `vis` (a [`Self::visible_indices`] sequence) of
    /// the entry drawn at the *bottom* of the viewport: the last entry while
    /// following, else the newest entry at or before the absolute anchor.
    ///
    /// Anchors that name a line the current filters hide (or a line inside a
    /// collapsed block's body) resolve to the entry that visually stands in
    /// for them, which is what makes a stale anchor render — and scroll —
    /// sensibly after a filter or fold change. `0` on an empty sequence;
    /// callers treat "nothing visible" as its own case.
    pub fn bottom_pos(&self, vis: &[u64]) -> usize {
        match self.scroll {
            Scroll::Follow => vis.len().saturating_sub(1),
            Scroll::Anchored(b) => match vis.binary_search(&b) {
                Ok(p) => p,
                Err(0) => 0,
                Err(p) => p - 1,
            },
        }
    }

    /// Scroll up (toward older entries) by `n` **visible** steps, freezing
    /// the view at an absolute anchor. One step is one drawn row: a line the
    /// filters hide is never landed on, and a collapsed panic block is
    /// crossed in a single step. Clamped at the oldest visible entry; a
    /// no-op while nothing is visible.
    ///
    /// `filter` is the committed free-text search filter — see
    /// [`Self::visible_indices`].
    pub fn scroll_up(&mut self, n: u64, filter: Option<&str>) {
        let vis = self.visible_indices(filter);
        if vis.is_empty() {
            return;
        }
        let pos = self.bottom_pos(&vis);
        let new = pos.saturating_sub(clamp_steps(n));
        self.scroll = Scroll::Anchored(vis[new]);
    }

    /// Scroll down (toward newer entries) by `n` **visible** steps — the
    /// mirror of [`Self::scroll_up`]. Reaching the last visible entry
    /// re-engages follow-tail; a no-op while already following (there is
    /// nothing below the tail) or while nothing is visible.
    pub fn scroll_down(&mut self, n: u64, filter: Option<&str>) {
        if self.is_following() {
            return;
        }
        let vis = self.visible_indices(filter);
        if vis.is_empty() {
            return;
        }
        let last = vis.len() - 1;
        let pos = self.bottom_pos(&vis).saturating_add(clamp_steps(n));
        self.scroll = if pos >= last {
            Scroll::Follow
        } else {
            Scroll::Anchored(vis[pos])
        };
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

    /// Set the scroll position from a scrollbar-thumb `frac` in `0.0..=1.0`
    /// (top → oldest visible entry, bottom → the tail) — the drag mapping for
    /// [`crate::engine::DragKind::LogScrollbar`]. Maps the fraction across the
    /// **visible sequence** ([`Self::visible_indices`]) — the same sequence
    /// the thumb's own position is measured against and `scroll_up`/
    /// `scroll_down` step through — so a drag can never land the anchor on a
    /// filtered-out line; at (or past) the very bottom it re-engages
    /// follow-tail. A no-op when nothing is visible. Returns whether the
    /// scroll position actually changed.
    pub fn scroll_to_fraction(&mut self, frac: f32, filter: Option<&str>) -> bool {
        let vis = self.visible_indices(filter);
        if vis.is_empty() {
            return false;
        }
        let last = vis.len() - 1;
        let f = frac.clamp(0.0, 1.0);
        let target = (f * last as f32).round() as usize;
        let next = if target >= last {
            Scroll::Follow
        } else {
            Scroll::Anchored(vis[target])
        };
        if next != self.scroll {
            self.scroll = next;
            true
        } else {
            false
        }
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

/// A scroll distance in lines as a position step within the visible
/// sequence, saturating instead of truncating on a 32-bit `usize` (the
/// sequence is bounded by [`LOG_LINE_CAP`], so a saturated step just means
/// "as far as it goes" — which is exactly what the callers clamp to anyway).
fn clamp_steps(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// Whether `line` matches a case-insensitive substring `query` (ANSI-stripped).
pub fn line_matches(line: &str, query: &str) -> bool {
    strip_ansi(line)
        .to_lowercase()
        .contains(&query.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::super::logstyle::{LineRole, LogSource, MAX_TRACKED_BLOCKS};
    use super::*;

    fn sess() -> SessionView {
        SessionView::new(SessionId(0), PathBuf::from("/tmp/app"), "desktop")
    }

    fn meta() -> LineMeta {
        LineMeta {
            level: LogLevel::Info,
            source: LogSource::App,
            timestamp: "00:00:00".to_string(),
            role: LineRole::Normal,
            source_prefix_strip: 0,
        }
    }

    #[test]
    fn ring_evicts_oldest_and_advances_base() {
        let mut buf = LogBuffer::default();
        for i in 0..(LOG_LINE_CAP as u64 + 3) {
            buf.push(format!("line {i}"), meta());
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
        s.scroll_up(5, None); // freeze at an absolute anchor
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
    fn scroll_to_fraction_maps_track_position_to_anchor() {
        let mut s = sess();
        for i in 0..100 {
            s.push_line(format!("line {i}")); // base 0, end 100, last 99
        }
        // Top of the track → oldest line anchored.
        assert!(s.scroll_to_fraction(0.0, None));
        assert_eq!(s.scroll, Scroll::Anchored(0));
        // Middle → ~line 50.
        s.scroll_to_fraction(0.5, None);
        assert_eq!(s.scroll, Scroll::Anchored(50));
        // Bottom → follow re-engaged (never an out-of-range anchor).
        s.scroll_to_fraction(1.0, None);
        assert!(s.is_following());
        // Out-of-range fractions clamp, never panic.
        s.scroll_to_fraction(-3.0, None);
        assert_eq!(s.scroll, Scroll::Anchored(0));
        s.scroll_to_fraction(9.0, None);
        assert!(s.is_following());
    }

    #[test]
    fn scroll_to_fraction_is_a_noop_on_empty_log() {
        let mut s = sess();
        assert!(!s.scroll_to_fraction(0.5, None));
        assert!(s.is_following());
    }

    #[test]
    fn scroll_down_to_bottom_re_engages_follow() {
        let mut s = sess();
        for i in 0..10 {
            s.push_line(format!("line {i}"));
        }
        s.scroll_up(4, None);
        assert!(!s.is_following());
        s.scroll_down(100, None);
        assert!(s.is_following());
    }

    // ── Scrolling steps the *visible* sequence, not raw indices ────────────

    /// Info/Error alternating: abs 0,2,4,… are Info and 1,3,5,… are Error.
    fn interleaved() -> SessionView {
        let mut s = sess();
        for i in 0..6 {
            s.push_line(format!("plain {i}"));
            s.push_line(format!("error: boom {i}"));
        }
        s
    }

    #[test]
    fn scroll_steps_visible_lines_only_under_a_level_filter() {
        let mut s = interleaved();
        // Negative control: unfiltered, one step is one raw line.
        s.scroll_up(1, None);
        assert_eq!(s.scroll, Scroll::Anchored(10));

        s.scroll_to_bottom();
        s.set_level_filter(LevelFilter::ErrorOnly);
        assert_eq!(s.visible_indices(None), vec![1, 3, 5, 7, 9, 11]);

        // One step from the tail lands on the previous *Error* line (11 → 9),
        // not on the hidden Info line 10.
        s.scroll_up(1, None);
        assert_eq!(s.scroll, Scroll::Anchored(9));
        s.scroll_up(2, None);
        assert_eq!(s.scroll, Scroll::Anchored(5));
        // Symmetric downward.
        s.scroll_down(1, None);
        assert_eq!(s.scroll, Scroll::Anchored(7));
        s.scroll_down(2, None);
        assert!(s.is_following(), "reaching the last visible line follows");
        // Clamped at the oldest visible line, never above it.
        s.scroll_up(500, None);
        assert_eq!(s.scroll, Scroll::Anchored(1));
    }

    #[test]
    fn scroll_steps_visible_lines_only_under_a_text_filter() {
        let mut s = sess();
        for l in ["hello 0", "noise", "hello 1", "noise", "hello 2"] {
            s.push_line(l.to_string());
        }
        let q = Some("hello");
        assert_eq!(s.visible_indices(q), vec![0, 2, 4]);
        s.scroll_up(1, q);
        assert_eq!(s.scroll, Scroll::Anchored(2));
        s.scroll_up(1, q);
        assert_eq!(s.scroll, Scroll::Anchored(0));
        s.scroll_down(1, q);
        assert_eq!(s.scroll, Scroll::Anchored(2));
        s.scroll_down(1, q);
        assert!(s.is_following());
    }

    #[test]
    fn a_stale_anchor_on_a_now_hidden_line_steps_from_what_is_shown() {
        let mut s = interleaved();
        // Anchored on an Info line, then the filter hides it: the view draws
        // the newest visible line at or before it (Error 9), so that — not
        // the stale anchor — is what a step moves from.
        s.scroll = Scroll::Anchored(10);
        s.set_level_filter(LevelFilter::ErrorOnly);
        s.scroll_up(1, None);
        assert_eq!(s.scroll, Scroll::Anchored(7));
    }

    #[test]
    fn scrolling_is_a_noop_while_the_filter_hides_everything() {
        let mut s = sess();
        for i in 0..5 {
            s.push_line(format!("plain {i}"));
        }
        s.set_level_filter(LevelFilter::ErrorOnly);
        assert!(s.visible_indices(None).is_empty());
        s.scroll_up(1, None);
        assert!(s.is_following(), "nothing visible to freeze on");
        s.scroll = Scroll::Anchored(2);
        s.scroll_down(1, None);
        assert_eq!(s.scroll, Scroll::Anchored(2));
        assert!(!s.scroll_to_fraction(0.5, None));
    }

    #[test]
    fn a_collapsed_backtrace_is_one_scroll_step_and_expands_to_one_per_line() {
        let mut s = sess();
        for l in panic_lines() {
            s.push_line(l.to_string());
        }
        // Collapsed (the default): the whole foldable body (2..=4) is one
        // entry, exactly as the log view draws it — one `▶ n frames…` row.
        assert_eq!(s.visible_indices(None), vec![0, 1, 2, 5]);
        s.scroll_up(1, None); // tail (5) → the fold entry
        assert_eq!(s.scroll, Scroll::Anchored(2));
        s.scroll_up(1, None); // …and one more step is past it entirely
        assert_eq!(s.scroll, Scroll::Anchored(1));

        // Expanded: every frame line is its own step again.
        s.toggle_fold(0);
        s.scroll_to_bottom();
        assert_eq!(s.visible_indices(None), vec![0, 1, 2, 3, 4, 5]);
        s.scroll_up(1, None);
        assert_eq!(s.scroll, Scroll::Anchored(4));
        s.scroll_up(1, None);
        assert_eq!(s.scroll, Scroll::Anchored(3));
    }

    #[test]
    fn scroll_to_fraction_lands_inside_the_visible_sequence() {
        let mut s = interleaved();
        s.set_level_filter(LevelFilter::ErrorOnly);
        // Six visible entries [1,3,5,7,9,11]: top → the oldest of them.
        assert!(s.scroll_to_fraction(0.0, None));
        assert_eq!(s.scroll, Scroll::Anchored(1));
        // Mid-track lands on a visible (Error) line, never a hidden one.
        s.scroll_to_fraction(0.5, None);
        assert_eq!(s.scroll, Scroll::Anchored(7));
        // Every reachable anchor is a visible line.
        for step in 0..=10 {
            s.scroll_to_fraction(step as f32 / 10.0, None);
            if let Scroll::Anchored(a) = s.scroll {
                assert_eq!(
                    s.effective_level(a),
                    Some(LogLevel::Error),
                    "fraction {step}/10 anchored a filtered-out line"
                );
            }
        }
        // The bottom still re-engages follow rather than anchoring the tail.
        s.scroll_to_fraction(1.0, None);
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

    // ── Log-styling: classification wiring, folds, level filter ────────────

    #[test]
    fn push_line_classifies_level_and_source_once() {
        let mut s = sess();
        s.push_line("warning: unused import".to_string());
        s.push_line("[gradle] BUILD SUCCESSFUL".to_string());
        assert_eq!(s.line_meta(0).unwrap().level, LogLevel::Warn);
        assert_eq!(s.line_meta(1).unwrap().source, LogSource::Gradle);
    }

    fn panic_lines() -> Vec<&'static str> {
        vec![
            "thread 'main' panicked at src/main.rs:42:9:",
            "called `Option::unwrap()` on a `None` value",
            "stack backtrace:",
            "   0: my_app::state::reduce",
            "             at src/state.rs:88:13",
            "app: recovering",
        ]
    }

    #[test]
    fn a_panic_backtrace_collapses_by_default_and_toggles_via_key_and_click() {
        let mut s = sess();
        for l in panic_lines() {
            s.push_line(l.to_string());
        }
        // The block starts at abs 0 (the panic header); its foldable body
        // covers the backtrace header through the last frame line (2..=4).
        assert!(s.is_fold_collapsed(0));
        for abs in 2..=4 {
            assert_eq!(s.panic_block_covering(abs).map(|b| b.start), Some(0));
        }
        assert!(s.panic_block_covering(0).is_none()); // the header itself isn't foldable
        assert!(s.panic_block_covering(1).is_none()); // nor the message line

        // Keyboard toggle (`z`) — nearest block to the current tail.
        let toggled = s.toggle_nearest_fold();
        assert_eq!(toggled, Some(0));
        assert!(!s.is_fold_collapsed(0));

        // Mouse click toggle — same block id, re-collapses.
        assert!(s.toggle_fold(0));
        assert!(s.is_fold_collapsed(0));
    }

    #[test]
    fn has_collapsed_panic_blocks_tracks_fold_state_not_just_presence() {
        let mut s = sess();
        assert!(!s.has_panic_blocks());
        assert!(!s.has_collapsed_panic_blocks());

        for l in panic_lines() {
            s.push_line(l.to_string());
        }
        // Starts collapsed by default — both gates are true.
        assert!(s.has_panic_blocks());
        assert!(s.has_collapsed_panic_blocks());

        // Expanding the only block drops the stricter (collapsed) gate but
        // not the looser (any block at all) one — the distinction the log
        // status row's `z fold` keyhint priority relies on.
        s.toggle_fold(0);
        assert!(s.has_panic_blocks());
        assert!(!s.has_collapsed_panic_blocks());
    }

    #[test]
    fn fold_state_survives_new_lines_streaming_in() {
        let mut s = sess();
        for l in panic_lines() {
            s.push_line(l.to_string());
        }
        s.toggle_fold(0); // expand
        assert!(!s.is_fold_collapsed(0));
        for i in 0..50 {
            s.push_line(format!("more output {i}"));
        }
        assert!(!s.is_fold_collapsed(0), "expand survives streaming lines");
    }

    #[test]
    fn fold_group_is_dropped_once_fully_evicted_from_the_ring() {
        let mut s = sess();
        for l in panic_lines() {
            s.push_line(l.to_string());
        }
        assert!(s.panic_block_covering(2).is_some());
        // Push enough lines to fully evict the whole block (end abs = 4) past
        // the ring cap.
        for i in 0..LOG_LINE_CAP as u64 + 10 {
            s.push_line(format!("filler {i}"));
        }
        assert!(s.panic_block_covering(2).is_none());
        assert!(!s.is_fold_collapsed(0)); // unknown id — false, never panics
    }

    // ── Bounded panic-block cost under a crash loop ──────────────────────────

    #[test]
    fn visible_indices_stays_correct_over_hundreds_of_backtrace_less_panic_headers() {
        // The debugging scenario this fix targets: a crash-looping app
        // re-triggering panic after panic, none ever growing a backtrace
        // (RUST_BACKTRACE unset — the default), so none is ever "absorbed"
        // and each pushed line stays its own visible-sequence entry. Well
        // within one ring window (LOG_LINE_CAP), so `evict_before`'s
        // ring-position eviction never runs — only the block-count cap
        // (`MAX_TRACKED_BLOCKS`) bounds `PanicTracker::blocks`, and the walk
        // below must still complete correctly with hundreds of tracked (or
        // cap-evicted) blocks behind it.
        let mut s = sess();
        let n = (MAX_TRACKED_BLOCKS * 4) as u64;
        for i in 0..n {
            s.push_line(format!("thread 'main' panicked at src/main.rs:{i}:1:"));
        }
        let vis = s.visible_indices(None);
        assert_eq!(vis, (0..n).collect::<Vec<_>>());
        // Scrolling still walks the full (uncollapsed) sequence correctly.
        s.scroll_up(5, None);
        assert_eq!(s.scroll, Scroll::Anchored(n - 6));
        s.scroll_to_top();
        assert_eq!(s.scroll, Scroll::Anchored(0));
        s.scroll_to_bottom();
        assert!(s.is_following());
    }

    #[test]
    fn visible_indices_stays_correct_across_many_folded_blocks_beyond_the_cap() {
        // Same crash-loop shape, but each panic *does* grow a full backtrace
        // — every block is foldable and, collapsed by default, absorbs its
        // body into one visible entry. More blocks than `MAX_TRACKED_BLOCKS`:
        // the oldest blocks' fold state (and absorb behavior) drops once
        // cap-evicted — same UX as ring eviction — so their backtrace lines
        // simply become their own visible entries again, while the newest
        // `MAX_TRACKED_BLOCKS` blocks stay tracked and folded.
        let mut s = sess();
        let block_count = MAX_TRACKED_BLOCKS * 3;
        for _ in 0..block_count {
            for l in panic_lines() {
                s.push_line(l.to_string());
            }
        }
        let vis = s.visible_indices(None);
        // Cap-evicted blocks (the oldest `block_count - MAX_TRACKED_BLOCKS`)
        // contribute all 6 pushed lines each (no fold — never absorbed);
        // still-tracked blocks (the newest `MAX_TRACKED_BLOCKS`) contribute
        // 4 each (header, message, one fold-row entry, trailing line) — see
        // `a_collapsed_backtrace_is_one_scroll_step_and_expands_to_one_per_line`.
        let evicted = block_count - MAX_TRACKED_BLOCKS;
        let expected = evicted * panic_lines().len() + MAX_TRACKED_BLOCKS * 4;
        assert_eq!(vis.len(), expected);

        // Structural spot-check: the earliest block's backtrace line is no
        // longer covered by any tracked block (cap-evicted), while the most
        // recent block's still is and is still collapsed.
        assert!(s.panic_block_covering(2).is_none());
        let last_header = s.log.end_index() - panic_lines().len() as u64;
        assert!(s.panic_block_covering(last_header + 2).is_some());
        assert!(s.is_fold_collapsed(last_header));
    }

    #[test]
    fn level_filter_hides_and_shows_by_cutoff() {
        let mut s = sess();
        s.push_line("plain info line".to_string());
        s.push_line("warning: careful".to_string());
        s.push_line("error: boom".to_string());
        assert_eq!(s.effective_level(0), Some(LogLevel::Info));
        assert_eq!(s.effective_level(1), Some(LogLevel::Warn));
        assert_eq!(s.effective_level(2), Some(LogLevel::Error));

        s.set_level_filter(LevelFilter::WarnPlus);
        assert!(!s.level_filter.allows(LogLevel::Info));
        assert!(s.level_filter.allows(LogLevel::Warn));
        assert!(s.level_filter.allows(LogLevel::Error));

        s.cycle_level_filter(1);
        assert_eq!(s.level_filter, LevelFilter::ErrorOnly);
        s.cycle_level_filter(-2);
        assert_eq!(s.level_filter, LevelFilter::InfoPlus);
    }

    #[test]
    fn follow_tail_keeps_tracking_the_tail_while_a_level_filter_is_active() {
        let mut s = sess();
        s.set_level_filter(LevelFilter::ErrorOnly);
        for i in 0..5 {
            s.push_line(format!("plain {i}"));
        }
        s.push_line("error: first".to_string());
        assert!(s.is_following());
        for i in 0..5 {
            s.push_line(format!("plain again {i}"));
        }
        s.push_line("error: second".to_string());
        // Still following: a filter only changes what's *shown* (the visible
        // sequence — see `visible_indices`), never the follow/anchored scroll
        // mode itself.
        assert!(s.is_following());
        assert_eq!(s.effective_level(5), Some(LogLevel::Error));
        assert_eq!(s.effective_level(11), Some(LogLevel::Error));
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
