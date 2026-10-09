//! The engine-side, pure view-model of a supervised session: its identity and
//! metadata, a bounded ring buffer of retained log lines, and the log-view
//! interaction state (follow-tail vs. an absolute scroll anchor, and the
//! copy-while-scrolling line selection — plus the per-session
//! **line-selection mode** that drives it) the [`crate::ui`] log view renders.
//!
//! Everything here is plain data + pure functions — no threads, no tokio, no
//! terminal — so the scroll/selection/eviction invariants are unit-tested
//! without a TTY (see the tests below). The moving part (the tokio supervisor)
//! lives in [`crate::supervise`]; this module only mirrors what it reports.

use std::collections::VecDeque;
use std::path::PathBuf;

use frust_drive::devices::Platform;

use super::devtools::{DevtoolsLaunch, DevtoolsState};
use super::logstyle::{
    LevelFilter, LineMeta, LogLevel, PanicBlock, PanicTracker, classify_level, classify_source,
    now_hms,
};
use super::perf::PerfPanel;
use crate::supervise::{DeviceTarget, PhaseLabel, SessionId, SessionState};

/// Ring-buffer cap for a session's retained log lines.
///
/// Aligned by value with `frust-drive`'s `LINE_BUFFER_CAP` (the
/// `spawn_streaming` producer-side cap): the drive already
/// bounds what it *delivers* to 10_000 lines, and the engine bounds what it
/// *retains* to the same figure so a long-lived session can never grow
/// unbounded memory. The drive const is private to that crate, so this is a
/// deliberate by-value alignment (one number, two layers), not a shared symbol.
pub const LOG_LINE_CAP: usize = 10_000;

/// How far above the bottom-anchored row line-selection mode keeps its cursor
/// before scrolling the view for it.
///
/// The engine is terminal-free by contract — it never learns the log
/// viewport's height, and [`Scroll::Anchored`] names the *bottom* row alone —
/// so [`SessionView::ensure_visible`] enforces "the cursor row stays on
/// screen" against this conservative floor instead of a real height: any
/// viewport at least this tall always shows the cursor, and a shorter one
/// merely scrolls a little sooner than it strictly must (recoverable, where
/// losing the cursor off-screen is not).
pub const SELECT_CURSOR_WINDOW: u64 = 10;

/// How many visible entries one `PageUp`/`PageDown` moves the selection
/// cursor. Aligned by value with the runner's own log-scroll page step (one
/// number, two layers — the runner's is a key-table detail, this one is
/// model state the message `SelectPage` carries only a direction for).
pub const SELECT_PAGE_LINES: u64 = 10;

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
///
/// `[lo, hi]` names an absolute range, but what the selection *means* — the
/// count the status row shows ([`SessionView::selected_visible_count`]) and
/// the lines it copies ([`SessionView::selected_text`]) — is restricted to
/// the same **visible sequence** ([`SessionView::visible_indices`]) the
/// highlight itself is drawn against: a line the level filter or the
/// committed search filter hides, or one absorbed into a collapsed panic
/// block's `▶ n frames…` row, contributes nothing even though its absolute
/// index falls inside `[lo, hi]`; a collapsed block's single visible entry
/// counts (and copies) as the one row the fold draws, never as every line it
/// absorbs.
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

/// *Where* a session runs, as an identity rather than display text.
///
/// [`SessionView::target_label`] answers "what should this tab say"; this
/// answers "is another session already running the same app in the same
/// place", which is what the one-live-session-per-(project, target) guard
/// needs ([`super::AppState::live_session_for`]). They are separate on
/// purpose: two distinct devices can share a display name, so a label is not
/// an identity.
///
/// Built from the supervise layer's [`DeviceTarget`] by [`Self::of`]. An
/// ad-hoc session (build/clean/toolchain-fix) has no target at all and
/// carries `None` instead, so it never takes part in the guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTarget {
    /// The desktop `cargo run` preview — one place, so two desktop sessions
    /// of the same project always name the same target.
    Desktop,
    /// A concrete device. `id` is the identity (`adb` serial / simulator
    /// udid); `name` and `platform` are carried for the messages this target
    /// appears in, and are deliberately **not** part of
    /// [`Self::is_same_place_as`].
    Device {
        /// The discoverer's stable device id.
        id: String,
        /// The device's display name, as the run-config modal shows it.
        name: String,
        /// Which mobile platform the device belongs to.
        platform: Platform,
    },
}

impl SessionTarget {
    /// The identity of a launch target.
    pub fn of(target: &DeviceTarget) -> Self {
        match target {
            DeviceTarget::Desktop => Self::Desktop,
            DeviceTarget::Device(device) => Self::Device {
                id: device.id.clone(),
                name: device.name.clone(),
                platform: device.platform,
            },
        }
    }

    /// Whether `self` and `other` name the same place to run.
    ///
    /// Devices compare by **id only**: a rediscovery reporting a renamed (or
    /// re-cased) device is still the same phone, and treating it as a new
    /// target would let the same app be launched onto it twice — exactly what
    /// the guard exists to prevent.
    pub fn is_same_place_as(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Desktop, Self::Desktop) => true,
            (Self::Device { id, .. }, Self::Device { id: other_id, .. }) => id == other_id,
            (Self::Desktop, Self::Device { .. }) | (Self::Device { .. }, Self::Desktop) => false,
        }
    }

    /// Whether "Watch: hot patch on save" can run here: the desktop preview
    /// or an Android device (a debug build of either runs hot; any other
    /// build restarts on save). An iOS device has neither story. The one
    /// gate the watch toggle, the palette row and the runner's watched
    /// launch all read.
    pub fn supports_watch(&self) -> bool {
        match self {
            Self::Desktop => true,
            Self::Device { platform, .. } => *platform == Platform::Android,
        }
    }

    /// How to name this target to the user (`desktop`, or the device's own
    /// name) — the subject of the "already running here" toast and of the
    /// MCP refusal.
    pub fn label(&self) -> String {
        match self {
            Self::Desktop => "desktop".to_string(),
            Self::Device { name, .. } => name.clone(),
        }
    }
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
    /// *Where* this session runs, as an identity — `None` for an ad-hoc
    /// build/clean/toolchain-fix session, which runs nothing on a target and
    /// so never blocks (or is blocked by) a launch. Distinct from
    /// [`Self::target_label`], which is display text: see [`SessionTarget`].
    pub target: Option<SessionTarget>,
    /// The last lifecycle state the supervisor reported (drives the status
    /// glyph).
    pub state: SessionState,
    /// The retained log lines.
    pub log: LogBuffer,
    /// Follow-tail vs. a frozen absolute anchor.
    pub scroll: Scroll,
    /// The copy-while-scrolling selection, if any.
    pub selection: Option<LineSelection>,
    /// Whether this session's log view is in **line-selection mode** (`v`):
    /// the log-view keys move a selection cursor instead of scrolling,
    /// follow-tail is paused, and a row click sets the range. Per session, so
    /// switching tabs neither carries the mode along nor cancels it on the
    /// tab it belongs to.
    pub select_mode: bool,
    /// Whether follow-tail was engaged when the mode was entered — what
    /// [`Self::exit_select_mode`] restores (only with the cursor still at the
    /// tail; see its doc). `None` outside the mode.
    pub follow_before_select: Option<bool>,
    /// Whether a log row has been clicked since the mode was entered: the
    /// first click re-anchors the selection, later ones only move its range
    /// end (see [`Self::clicked_since_enter`]).
    clicked_since_enter: bool,
    /// Cumulative count of output lines the supervisor dropped because its
    /// bounded engine channel was full (drop-newest overflow — see
    /// [`crate::supervise`]). `0` for a healthy session; a non-zero value means
    /// the log is missing lines the consumer couldn't keep up with, which the
    /// UI can surface. Mirrors `frust_drive::process::LineReceiver::dropped_lines`.
    pub dropped: u64,
    /// Set by `Message::CloseTab`/`CloseActiveTab` when the tab was closed
    /// while the session was still live: the session is stopped as normal,
    /// and the moment its terminal event lands the tab removes itself (see
    /// `super::update::on_session_event`) instead of staying around like an
    /// ordinary stopped session. `false` for every ordinary session.
    pub close_on_exit: bool,
    /// "Watch: restart on save" (`Message::ToggleWatch`): while `true` the
    /// runner keeps a source watcher over this session's project and a
    /// settled change burst restarts it (`Message::WatchTriggered`). Desktop
    /// sessions only. Survives a terminal state on purpose — a build that
    /// failed to compile is exactly the session a save should relaunch — and
    /// carries over to the relaunch: the runner re-enables it on the new
    /// session with `Message::EnableWatch` once that registers. `false` for
    /// every freshly registered session.
    pub watch: bool,
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
    /// The session's DevTools mode state (workbook §B12): the discovery line
    /// parsed out of its log, the bridge's connection, the selected tab, and
    /// the frame-stats ring — see [`super::devtools`]. Per session, so
    /// switching tabs never forces a tab out of DevTools.
    pub devtools: DevtoolsState,
}

impl SessionView {
    /// A fresh session view (empty log, following the tail, no selection) for
    /// an **ad-hoc** session: one that can neither host a devtools service
    /// nor run an app on a target, so it carries no [`SessionTarget`] and
    /// takes no part in the one-live-session guard (a build and a run of the
    /// same project are not the same thing). See [`Self::with_devtools`] for
    /// a real app session.
    pub fn new(id: SessionId, project_root: PathBuf, target_label: impl Into<String>) -> Self {
        Self::with_devtools(
            id,
            project_root,
            target_label,
            DevtoolsLaunch::unavailable(),
            None,
        )
    }

    /// [`Self::new`] with the session's own devtools launch metadata (build
    /// mode + Android serial), which decides §B12's app-without-devtools
    /// state and how the bridge reaches the service, plus the
    /// [`SessionTarget`] it runs on (`None` for an ad-hoc session).
    pub fn with_devtools(
        id: SessionId,
        project_root: PathBuf,
        target_label: impl Into<String>,
        devtools: DevtoolsLaunch,
        target: Option<SessionTarget>,
    ) -> Self {
        Self {
            id,
            project_root,
            target_label: target_label.into(),
            target,
            state: SessionState::Configuring,
            log: LogBuffer::default(),
            scroll: Scroll::Follow,
            selection: None,
            select_mode: false,
            follow_before_select: None,
            clicked_since_enter: false,
            dropped: 0,
            close_on_exit: false,
            watch: false,
            perf: PerfPanel::default(),
            level_filter: LevelFilter::default(),
            panic_tracker: PanicTracker::default(),
            current_phase: None,
            devtools: DevtoolsState::new(devtools),
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
    ///
    /// Returns whether the line carried a *new* devtools discovery
    /// announcement (see [`Self::push_line_at`]).
    pub fn push_line(&mut self, line: String) -> bool {
        self.push_line_at(line, now_hms())
    }

    /// [`Self::push_line`] with an explicit `timestamp` instead of the real
    /// wall clock — the primitive `push_line` delegates to, and the seam
    /// tests (and snapshot fixtures) use for reproducible output.
    ///
    /// Returns whether the line announced a devtools service this session
    /// hadn't already recorded (`frust-devtools listening on …` — workbook
    /// §B12). The caller (`crate::engine::update`) turns a `true` into the
    /// connect effect; the parse itself runs here, once, on the same
    /// ANSI-stripped line the perf/level/panic classifiers already see.
    pub fn push_line_at(&mut self, line: String, timestamp: impl Into<String>) -> bool {
        let plain = strip_ansi(&line);
        self.perf.ingest(&plain);
        let discovered = self.devtools.ingest_line(&plain);
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
        // Store a redacted copy so the retained ring — surfaced verbatim by
        // both the log view and `TuiSessionBackend::log_tail`'s MCP
        // `app_logs` tool — never carries the devtools handshake token past
        // this point. The parser above already saw the real token off
        // `plain`, so the connect path is unaffected; only what gets
        // *retained* changes. `Cow::Borrowed` (the overwhelming common case
        // — no discovery line) means no allocation here.
        let stored = match frust_devtools_protocol::redact_discovery_token(&line) {
            std::borrow::Cow::Borrowed(_) => line,
            std::borrow::Cow::Owned(redacted) => redacted,
        };
        self.log.push(stored, meta);
        let base = self.log.base_index();
        self.panic_tracker.evict_before(base);
        if let Scroll::Anchored(b) = self.scroll
            && b < base
        {
            self.scroll = Scroll::Anchored(base);
        }
        if let Some(sel) = self.selection {
            if sel.hi() < base {
                // The whole selection is gone — a selection with nothing
                // left to highlight is not a state line-selection mode can
                // be in, so the mode ends with it rather than lingering with
                // `select_mode && selection.is_none()`. The dropped range can
                // never still be the visible tail (it is strictly older than
                // every retained line), so `exit_select_mode`'s "cursor still
                // at the tail" follow-restore rule correctly never fires here
                // regardless of the filter passed — `None` is as good as any.
                if self.select_mode {
                    self.exit_select_mode(None);
                } else {
                    self.selection = None;
                }
            } else if sel.lo() < base {
                self.selection = Some(LineSelection {
                    anchor: sel.anchor.max(base),
                    cursor: sel.cursor.max(base),
                });
            }
        }
        discovered
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
    /// re-engages follow-tail (except in line-selection mode, which keeps
    /// follow paused — see [`Self::enter_select_mode`]); a no-op while
    /// already following (there is nothing below the tail) or while nothing
    /// is visible.
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
        self.scroll = if pos >= last && !self.select_mode {
            // Line-selection mode holds follow-tail paused for its whole
            // life, so a wheel back down to the tail stops *at* the tail line
            // rather than re-engaging follow under the cursor.
            Scroll::Follow
        } else {
            Scroll::Anchored(vis[pos.min(last)])
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

    /// Clear any selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    // ── Line-selection mode ─────────────────────────────────────────────────

    /// Whether a log row has already been clicked since the mode was entered
    /// — the first click of a mode session re-anchors ([`Self::anchor_at`]),
    /// every later one only moves the range end ([`Self::cursor_to`]).
    pub fn clicked_since_enter(&self) -> bool {
        self.clicked_since_enter
    }

    /// Enter line-selection mode: anchor a one-line selection on the newest
    /// **visible** entry (the bottom row of the current viewport — the newest
    /// line while following, the bottom-anchored one while scrolled up),
    /// freeze follow-tail so incoming lines can never drag the viewport out
    /// from under the cursor, and remember whether follow was on so
    /// [`Self::exit_select_mode`] can put it back.
    ///
    /// A no-op (returning `false`) on an empty log, on a log whose filters
    /// currently hide every line — there is no row to anchor on — and while
    /// the mode is already engaged. `filter` is the committed free-text
    /// search filter, as everywhere else the visible sequence is measured
    /// (see [`Self::visible_indices`]).
    pub fn enter_select_mode(&mut self, filter: Option<&str>) -> bool {
        if self.select_mode {
            return false;
        }
        let vis = self.visible_indices(filter);
        if vis.is_empty() {
            return false;
        }
        let at = vis[self.bottom_pos(&vis)];
        self.follow_before_select = Some(self.is_following());
        self.scroll = Scroll::Anchored(at);
        self.selection = Some(LineSelection {
            anchor: at,
            cursor: at,
        });
        self.clicked_since_enter = false;
        self.select_mode = true;
        true
    }

    /// Leave line-selection mode, dropping the selection.
    ///
    /// **Follow-tail is restored only when it was on at entry *and* the
    /// cursor is still sitting on the newest visible entry.** Having walked
    /// the cursor away from the tail is a deliberate "I am reading here"
    /// statement, so leaving the mode keeps the anchored scroll exactly where
    /// the user left it rather than snapping the view back to the tail; `f`
    /// (or scrolling to the bottom) re-engages follow the usual way.
    ///
    /// Returns whether anything changed.
    pub fn exit_select_mode(&mut self, filter: Option<&str>) -> bool {
        if !self.select_mode {
            return false;
        }
        let vis = self.visible_indices(filter);
        let at_tail = match (self.selection, vis.last()) {
            (Some(sel), Some(last)) => sel.cursor == *last,
            _ => false,
        };
        self.select_mode = false;
        self.clicked_since_enter = false;
        self.selection = None;
        if self.follow_before_select.take() == Some(true) && at_tail {
            self.scroll = Scroll::Follow;
        }
        true
    }

    /// Step the selection cursor `delta` **visible** entries (negative =
    /// toward older lines), clamped to the ends of the visible sequence, and
    /// scroll only as far as [`Self::ensure_visible`] needs to keep the
    /// cursor row on screen. One step is one drawn row, exactly like the
    /// scroll keys: a filtered-out line is never landed on and a collapsed
    /// panic block is crossed in a single step.
    ///
    /// Returns whether the cursor (or the scroll) actually moved.
    pub fn move_cursor(&mut self, delta: i64, filter: Option<&str>) -> bool {
        if !self.select_mode {
            return false;
        }
        let vis = self.visible_indices(filter);
        let (Some(sel), false) = (self.selection, vis.is_empty()) else {
            return false;
        };
        let pos = pos_of(&vis, sel.cursor) as i64;
        let last = (vis.len() - 1) as i64;
        let next = pos.saturating_add(delta).clamp(0, last) as usize;
        self.set_cursor(vis[next], &vis)
    }

    /// [`Self::move_cursor`] by one page (`dir` is the sign: `-1` older, `1`
    /// newer) — [`SELECT_PAGE_LINES`] visible entries.
    pub fn move_cursor_page(&mut self, dir: i8, filter: Option<&str>) -> bool {
        let delta = i64::from(dir.signum()) * SELECT_PAGE_LINES as i64;
        self.move_cursor(delta, filter)
    }

    /// Jump the selection cursor to the oldest visible entry.
    pub fn cursor_home(&mut self, filter: Option<&str>) -> bool {
        self.jump_cursor(filter, |vis| vis.first().copied())
    }

    /// Jump the selection cursor to the newest visible entry.
    pub fn cursor_end(&mut self, filter: Option<&str>) -> bool {
        self.jump_cursor(filter, |vis| vis.last().copied())
    }

    /// Shared body of [`Self::cursor_home`]/[`Self::cursor_end`]: pick an end
    /// of the visible sequence and put the cursor there.
    fn jump_cursor(
        &mut self,
        filter: Option<&str>,
        pick: impl FnOnce(&[u64]) -> Option<u64>,
    ) -> bool {
        if !self.select_mode || self.selection.is_none() {
            return false;
        }
        let vis = self.visible_indices(filter);
        match pick(&vis) {
            Some(abs) => self.set_cursor(abs, &vis),
            None => false,
        }
    }

    /// Start a fresh one-line selection at absolute line `abs` (the first log
    /// row click after entering the mode). A no-op outside the mode or on a
    /// line the ring no longer retains.
    ///
    /// Unlike the cursor keys this never scrolls: `abs` came from a row the
    /// user just clicked, so it is on screen by construction, and moving the
    /// view under a click would be the one thing a click must not do.
    pub fn anchor_at(&mut self, abs: u64) -> bool {
        if !self.select_mode || self.log.get(abs).is_none() {
            return false;
        }
        self.selection = Some(LineSelection {
            anchor: abs,
            cursor: abs,
        });
        self.clicked_since_enter = true;
        true
    }

    /// Move the selection's moving end to absolute line `abs` (a later log
    /// row click), leaving the anchor where it is — the whole range between
    /// the two becomes selected. Scrolls no more than [`Self::anchor_at`]
    /// does, and for the same reason.
    pub fn cursor_to(&mut self, abs: u64) -> bool {
        if !self.select_mode || self.log.get(abs).is_none() {
            return false;
        }
        let moved = self.selection.is_some_and(|sel| sel.cursor != abs);
        if let Some(sel) = self.selection.as_mut() {
            sel.cursor = abs;
        }
        moved
    }

    /// The plain text of the line at absolute index `abs` — ANSI-stripped
    /// exactly like [`Self::selected_text`], and read from the same
    /// **redacted** ring the log view renders, so a copy can never resurrect
    /// a redacted devtools handshake token. `None` once the line has been
    /// evicted.
    pub fn line_text(&self, abs: u64) -> Option<String> {
        self.log.get(abs).map(strip_ansi)
    }

    /// Put the cursor on `abs` and scroll only as far as needed to keep it on
    /// screen; returns whether anything moved.
    fn set_cursor(&mut self, abs: u64, vis: &[u64]) -> bool {
        let before = (self.selection.map(|s| s.cursor), self.scroll);
        if let Some(sel) = self.selection.as_mut() {
            sel.cursor = abs;
        }
        self.ensure_visible(abs, vis);
        before != (self.selection.map(|s| s.cursor), self.scroll)
    }

    /// Move the scroll anchor the minimum distance that keeps absolute line
    /// `abs` on screen, measured in the visible sequence `vis`.
    ///
    /// The engine is terminal-free and the scroll anchor names the *bottom*
    /// row alone, so "on screen" is enforced against [`SELECT_CURSOR_WINDOW`]
    /// rather than a real viewport height: `abs` is never left below the
    /// bottom row, and never more than `SELECT_CURSOR_WINDOW - 1` rows above
    /// it. A no-op while `abs` already sits inside that window — walking the
    /// cursor a few rows up does not scroll the view.
    pub fn ensure_visible(&mut self, abs: u64, vis: &[u64]) {
        if vis.is_empty() {
            return;
        }
        let target = pos_of(vis, abs);
        let bottom = self.bottom_pos(vis);
        let next = if target > bottom {
            target
        } else if bottom - target >= SELECT_CURSOR_WINDOW as usize {
            target + SELECT_CURSOR_WINDOW as usize - 1
        } else {
            return;
        };
        self.scroll = Scroll::Anchored(vis[next.min(vis.len() - 1)]);
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
    /// clipboard — or `None` if nothing is selected / retained. Restricted to
    /// the **visible sequence** between the selection's bounds (see
    /// [`LineSelection`]'s doc for the rule) — `filter` is the committed
    /// free-text search filter, as everywhere else the visible sequence is
    /// measured (see [`Self::visible_indices`]).
    pub fn selected_text(&self, filter: Option<&str>) -> Option<String> {
        let sel = self.selection?;
        let mut out = String::new();
        let mut any = false;
        for abs in self.visible_indices(filter) {
            if !sel.contains(abs) {
                continue;
            }
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

    /// The number of entries in the **visible sequence** the current
    /// selection spans — the same count [`Self::selected_text`] copies and
    /// the status row's `SELECT · {n} lines` hint shows (see
    /// [`LineSelection`]'s doc for the rule). `0` with no selection.
    pub fn selected_visible_count(&self, filter: Option<&str>) -> u64 {
        let Some(sel) = self.selection else {
            return 0;
        };
        self.visible_indices(filter)
            .into_iter()
            .filter(|abs| sel.contains(*abs))
            .count() as u64
    }
}

/// A scroll distance in lines as a position step within the visible
/// sequence, saturating instead of truncating on a 32-bit `usize` (the
/// sequence is bounded by [`LOG_LINE_CAP`], so a saturated step just means
/// "as far as it goes" — which is exactly what the callers clamp to anyway).
fn clamp_steps(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// The position within `vis` of absolute line `abs`, resolving a line the
/// sequence does not hold (filtered out, or absorbed into a collapsed panic
/// block) to the entry that visually stands in for it — the same resolution
/// [`SessionView::bottom_pos`] gives a stale scroll anchor, so a cursor and
/// an anchor can never disagree about where a line "is".
fn pos_of(vis: &[u64], abs: u64) -> usize {
    match vis.binary_search(&abs) {
        Ok(p) => p,
        Err(0) => 0,
        Err(p) => p - 1,
    }
}

/// Truncate `s` to at most `max` Unicode scalar values, marking a cut with a
/// trailing ellipsis — the one-line preview a "copied this line" notice
/// shows. Counted in `char`s (not bytes) so a multi-byte line can never be
/// split mid-scalar; a string already within `max` is returned unchanged,
/// with no ellipsis to suggest a cut that never happened.
pub fn truncate_with_ellipsis(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().nth(max).is_some() {
        out.push('\u{2026}');
    }
    out
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

    // ── Line-selection mode ─────────────────────────────────────────────

    /// A session holding `n` lines, following its tail.
    fn seeded(n: u64) -> SessionView {
        let mut s = sess();
        for i in 0..n {
            s.push_line_at(format!("line {i}"), "00:00:00");
        }
        s
    }

    fn cursor(s: &SessionView) -> u64 {
        s.selection.expect("a selection").cursor
    }

    fn bounds(s: &SessionView) -> (u64, u64) {
        let sel = s.selection.expect("a selection");
        (sel.lo(), sel.hi())
    }

    #[test]
    fn entering_anchors_the_newest_visible_line_and_freezes_follow() {
        let mut s = seeded(30);
        assert!(s.enter_select_mode(None));
        assert!(s.select_mode);
        assert_eq!(bounds(&s), (29, 29));
        assert_eq!(s.follow_before_select, Some(true));
        assert_eq!(s.scroll, Scroll::Anchored(29), "follow is frozen in place");
        assert!(!s.enter_select_mode(None), "entering twice is a no-op");
    }

    #[test]
    fn entering_while_scrolled_up_anchors_the_bottom_row_on_screen() {
        let mut s = seeded(30);
        s.scroll_up(10, None);
        let Scroll::Anchored(bottom) = s.scroll else {
            panic!("scrolled up → anchored");
        };
        assert_eq!(bottom, 19);

        assert!(s.enter_select_mode(None));

        assert_eq!(bounds(&s), (19, 19), "not the tail — the bottom row");
        assert_eq!(s.follow_before_select, Some(false));
    }

    #[test]
    fn entering_an_empty_or_wholly_filtered_log_is_a_no_op() {
        let mut empty = sess();
        assert!(!empty.enter_select_mode(None));
        assert!(!empty.select_mode);
        assert_eq!(empty.selection, None);

        let mut s = seeded(5);
        assert!(!s.enter_select_mode(Some("nothing matches this")));
        assert!(!s.select_mode);
    }

    #[test]
    fn the_cursor_clamps_at_both_ends_of_the_visible_sequence() {
        let mut s = seeded(10);
        s.enter_select_mode(None);
        assert!(s.move_cursor(-3, None));
        assert_eq!(bounds(&s), (6, 9));
        assert!(s.move_cursor(-100, None));
        assert_eq!(cursor(&s), 0, "clamped at the oldest retained line");
        assert!(!s.move_cursor(-1, None), "already clamped — nothing moved");
        assert!(s.move_cursor(100, None));
        assert_eq!(cursor(&s), 9, "clamped at the newest line");
        assert!(!s.move_cursor(1, None));
    }

    #[test]
    fn a_page_move_steps_a_page_of_visible_entries() {
        let mut s = seeded(40);
        s.enter_select_mode(None);
        assert!(s.move_cursor_page(-1, None));
        assert_eq!(cursor(&s), 39 - SELECT_PAGE_LINES);
        assert!(s.move_cursor_page(1, None));
        assert_eq!(cursor(&s), 39);
    }

    #[test]
    fn home_and_end_jump_to_the_ends_of_the_visible_sequence() {
        let mut s = seeded(12);
        s.enter_select_mode(None);
        assert!(s.cursor_home(None));
        assert_eq!(bounds(&s), (0, 11));
        assert!(s.cursor_end(None));
        assert_eq!(bounds(&s), (11, 11));
    }

    #[test]
    fn the_cursor_steps_through_visible_entries_not_raw_indices() {
        let mut s = sess();
        s.push_line_at("plain".into(), "00:00:00"); // Info
        s.push_line_at("warning: careful".into(), "00:00:01"); // Warn
        s.push_line_at("error: boom".into(), "00:00:02"); // Error
        s.set_level_filter(LevelFilter::WarnPlus);
        assert_eq!(s.visible_indices(None), vec![1, 2]);

        s.enter_select_mode(None);
        assert_eq!(cursor(&s), 2);
        s.move_cursor(-1, None);
        assert_eq!(cursor(&s), 1, "the hidden Info line is never landed on");
        s.move_cursor(-1, None);
        assert_eq!(cursor(&s), 1, "and it is not below the oldest visible one");
    }

    #[test]
    fn the_view_scrolls_only_as_far_as_the_cursor_needs() {
        let mut s = seeded(60);
        s.enter_select_mode(None);
        let anchored = s.scroll;

        // Inside the cursor window the view holds still…
        for _ in 0..(SELECT_CURSOR_WINDOW - 1) {
            s.move_cursor(-1, None);
        }
        assert_eq!(s.scroll, anchored, "no scroll while the cursor is in view");

        // …and beyond it the anchor follows one row at a time, keeping the
        // cursor exactly `SELECT_CURSOR_WINDOW - 1` rows above the bottom.
        s.move_cursor(-1, None);
        assert_eq!(
            s.scroll,
            Scroll::Anchored(cursor(&s) + SELECT_CURSOR_WINDOW - 1)
        );

        // Jumping back down puts the cursor on the bottom row rather than
        // leaving it below the viewport.
        s.cursor_end(None);
        assert_eq!(s.scroll, Scroll::Anchored(59));
    }

    #[test]
    fn a_click_anchors_once_then_moves_the_range_end() {
        let mut s = seeded(20);
        s.enter_select_mode(None);
        assert!(!s.clicked_since_enter());

        let anchored = s.scroll;

        assert!(s.anchor_at(4));
        assert!(s.clicked_since_enter());
        assert_eq!(bounds(&s), (4, 4));

        assert!(s.cursor_to(11));
        assert_eq!(bounds(&s), (4, 11), "the range runs between the two");
        assert!(s.cursor_to(2));
        assert_eq!(bounds(&s), (2, 4), "the anchor never moved");
        assert_eq!(s.scroll, anchored, "a click never scrolls the view");

        assert!(!s.cursor_to(999), "an unretained line is refused");
    }

    #[test]
    fn a_click_outside_the_mode_changes_nothing() {
        let mut s = seeded(5);
        assert!(!s.anchor_at(2));
        assert!(!s.cursor_to(2));
        assert_eq!(s.selection, None);
    }

    #[test]
    fn exiting_restores_follow_only_from_the_tail() {
        // Cursor still at the tail: the follow that was paused comes back.
        let mut s = seeded(10);
        s.enter_select_mode(None);
        assert!(s.exit_select_mode(None));
        assert!(!s.select_mode);
        assert_eq!(s.selection, None);
        assert_eq!(s.follow_before_select, None);
        assert!(s.is_following());

        // Cursor walked away from the tail: the anchored scroll stays put.
        let mut s = seeded(10);
        s.enter_select_mode(None);
        s.move_cursor(-4, None);
        let anchored = s.scroll;
        s.exit_select_mode(None);
        assert!(!s.is_following());
        assert_eq!(s.scroll, anchored);

        // Follow was already off at entry: it stays off at the tail too.
        let mut s = seeded(10);
        s.scroll_up(3, None);
        s.enter_select_mode(None);
        s.cursor_end(None);
        s.exit_select_mode(None);
        assert!(!s.is_following());

        assert!(!s.exit_select_mode(None), "leaving twice is a no-op");
    }

    #[test]
    fn a_selection_clamps_and_then_drops_as_its_lines_are_evicted() {
        let mut s = sess();
        for i in 0..LOG_LINE_CAP as u64 {
            s.push_line_at(format!("line {i}"), "00:00:00");
        }
        s.enter_select_mode(None);
        s.move_cursor(-2, None); // the three newest lines
        let (lo, hi) = bounds(&s);

        // Enough incoming output to evict the older half of the selection.
        for i in 0..(lo + 2) {
            s.push_line_at(format!("more {i}"), "00:00:00");
        }
        let base = s.log.base_index();
        assert!(base > lo && base <= hi);
        assert_eq!(bounds(&s), (base, hi), "clamped up to the oldest retained");
        assert!(s.select_mode, "the mode outlives the eviction");

        // And once every selected line is gone, so is the selection — and,
        // since a mode with nothing selected is not a state this reaches, the
        // mode itself.
        for i in 0..(hi - base + 1) {
            s.push_line_at(format!("even more {i}"), "00:00:00");
        }
        assert_eq!(s.selection, None);
        assert!(
            !s.select_mode,
            "the mode ends once its selection is fully evicted"
        );
    }

    #[test]
    fn line_text_reads_the_redacted_ring_ansi_stripped() {
        let mut s = sess();
        s.push_line_at("\u{1b}[31mred alert\u{1b}[0m".into(), "00:00:00");
        assert_eq!(s.line_text(0).as_deref(), Some("red alert"));
        assert_eq!(s.line_text(9), None, "an unretained line has no text");
    }

    #[test]
    fn truncate_with_ellipsis_counts_scalars_and_only_marks_a_real_cut() {
        assert_eq!(truncate_with_ellipsis("short", 60), "short");
        assert_eq!(truncate_with_ellipsis("abcdef", 6), "abcdef");
        assert_eq!(truncate_with_ellipsis("abcdefg", 6), "abcdef\u{2026}");
        // Multi-byte input is cut on scalar boundaries, never mid-character.
        assert_eq!(truncate_with_ellipsis("héllo wörld", 5), "héllo\u{2026}");
        assert_eq!(truncate_with_ellipsis("", 0), "");
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
        assert_eq!(
            s.selected_text(None).as_deref(),
            Some("line 2\nline 3\nline 4")
        );
    }

    /// The WarnPlus counterexample: a Warn/Info/Warn triple with the level
    /// filter hiding the middle line — the selection's absolute range still
    /// spans all three, but the copy and the count see only the two visible
    /// rows the highlight itself draws over.
    #[test]
    fn selected_text_and_count_are_restricted_to_the_visible_sequence() {
        let mut s = sess();
        s.push_line_at("warn a".into(), "00:00:00"); // Warn — abs 0
        s.push_line_at("info b".into(), "00:00:01"); // Info — abs 1, hidden
        s.push_line_at("warn c".into(), "00:00:02"); // Warn — abs 2
        s.set_level_filter(LevelFilter::WarnPlus);
        s.selection = Some(LineSelection {
            anchor: 0,
            cursor: 2,
        });
        assert_eq!(
            s.selected_visible_count(None),
            2,
            "the hidden Info line does not count"
        );
        assert_eq!(s.selected_text(None).as_deref(), Some("warn a\nwarn c"));
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

    // ── DevTools discovery capture (workbook §B12) ──────────────────────────

    #[test]
    fn push_line_captures_a_devtools_discovery_line_once_and_takes_the_latest() {
        let mut s = SessionView::with_devtools(
            SessionId(0),
            PathBuf::from("/tmp/app"),
            "desktop",
            crate::engine::DevtoolsLaunch::from_launch(
                frust_drive::build_info::BuildMode::Debug,
                None,
            ),
            Some(SessionTarget::Desktop),
        );
        assert!(!s.push_line("app: booting up".to_string()));
        assert!(s.push_line(
            "08-09 12:00:01.234 1234 1234 I frust: frust-devtools listening on 53214 token cafe"
                .to_string()
        ));
        let found = s.devtools.discovered.clone().unwrap();
        assert_eq!(found.port, 53214);
        assert_eq!(found.token.as_deref(), Some("cafe"));
        // A repeat of the same announcement is not a fresh discovery…
        assert!(!s.push_line("frust-devtools listening on 53214 token cafe".to_string()));
        // …but a restart's re-announcement is, and the latest wins.
        assert!(s.push_line("frust-devtools listening on 60001 token beef".to_string()));
        assert_eq!(s.devtools.discovered.as_ref().unwrap().port, 60001);
    }

    #[test]
    fn a_session_that_cannot_host_devtools_records_the_line_but_never_connects() {
        let mut s = sess();
        // The parse is unconditional (the state records what was actually
        // logged); the *connect* decision is what capability gates.
        assert!(s.push_line("frust-devtools listening on 53214 token cafe".to_string()));
        assert!(s.devtools.discovered.is_some());
        assert!(!s.devtools.launch.capable);
        assert!(
            !s.devtools.wants_connect(),
            "an ad-hoc (build/clean) session is never devtools-capable"
        );
    }

    #[test]
    fn built_artifact_paths_empty_with_no_built_lines() {
        let mut s = sess();
        s.push_line("hello".to_string());
        assert!(s.built_artifact_paths().is_empty());
    }

    // ── Devtools token redaction at the log-ring edge ──────────────────────

    /// The stored ring copy of a discovery line never carries the token,
    /// while the connect path (fed the pre-redaction `plain` text) still
    /// fires — `app_logs`/the log view and the devtools handshake are
    /// independent of each other.
    #[test]
    fn discovery_line_is_redacted_in_the_ring_but_still_parsed_for_connect() {
        let mut s = SessionView::with_devtools(
            SessionId(0),
            PathBuf::from("/tmp/app"),
            "desktop",
            crate::engine::DevtoolsLaunch::from_launch(
                frust_drive::build_info::BuildMode::Debug,
                None,
            ),
            Some(SessionTarget::Desktop),
        );
        let discovered =
            s.push_line("frust-devtools listening on 53214 token cafe1234".to_string());
        // Connect path unaffected: the parser saw the real token.
        assert!(discovered);
        let found = s.devtools.discovered.clone().unwrap();
        assert_eq!(found.port, 53214);
        assert_eq!(found.token.as_deref(), Some("cafe1234"));

        // But the retained ring copy — what `app_logs`/the log view surface
        // — is redacted.
        let stored = s.log.get(0).unwrap();
        assert!(stored.contains("<redacted>"), "stored line: {stored}");
        assert!(!stored.contains("cafe1234"), "stored line: {stored}");
    }

    /// The same redaction holds when the discovery line still carries ANSI
    /// escapes around/inside it (the raw, unstripped form actually pushed
    /// onto the ring) — `redact_discovery_token`'s substring search must
    /// still land on the right span.
    #[test]
    fn ansi_wrapped_discovery_line_is_also_redacted() {
        let mut s = SessionView::with_devtools(
            SessionId(0),
            PathBuf::from("/tmp/app"),
            "desktop",
            crate::engine::DevtoolsLaunch::from_launch(
                frust_drive::build_info::BuildMode::Debug,
                None,
            ),
            Some(SessionTarget::Desktop),
        );
        let raw = "\u{1b}[32mfrust-devtools listening on 53214 token cafe1234\u{1b}[0m";
        let discovered = s.push_line(raw.to_string());
        assert!(discovered, "ANSI must not defeat the plain-text parse");
        let found = s.devtools.discovered.clone().unwrap();
        assert_eq!(found.port, 53214);
        assert_eq!(found.token.as_deref(), Some("cafe1234"));

        let stored = s.log.get(0).unwrap();
        assert!(stored.contains("<redacted>"), "stored line: {stored}");
        assert!(!stored.contains("cafe1234"), "stored line: {stored}");
        // The leading ANSI (before the discovery prefix) is untouched — the
        // substring search only ever consumes from `DISCOVERY_PREFIX`
        // onward.
        assert!(stored.starts_with("\u{1b}[32m"));
    }

    /// A discovery line whose trailing ANSI reset is separated from the
    /// token by whitespace (the realistic shape — a logger's reset code
    /// follows the newline/line-end, not glued onto the token digits) keeps
    /// that reset intact in the stored, redacted copy.
    #[test]
    fn ansi_reset_separated_by_whitespace_survives_redaction() {
        let mut s = SessionView::with_devtools(
            SessionId(0),
            PathBuf::from("/tmp/app"),
            "desktop",
            crate::engine::DevtoolsLaunch::from_launch(
                frust_drive::build_info::BuildMode::Debug,
                None,
            ),
            Some(SessionTarget::Desktop),
        );
        let raw = "\u{1b}[32mfrust-devtools listening on 53214 token cafe1234 \u{1b}[0m(ready)";
        let discovered = s.push_line(raw.to_string());
        assert!(discovered);
        let stored = s.log.get(0).unwrap();
        assert!(stored.contains("<redacted>"), "stored line: {stored}");
        assert!(!stored.contains("cafe1234"), "stored line: {stored}");
        assert!(
            stored.ends_with("\u{1b}[0m(ready)"),
            "stored line: {stored}"
        );
    }

    /// A normal (non-discovery) line is stored byte-identical — no
    /// allocation regression for the common case.
    #[test]
    fn non_discovery_line_is_stored_unchanged() {
        let mut s = sess();
        s.push_line("just some ordinary app output".to_string());
        assert_eq!(s.log.get(0), Some("just some ordinary app output"));
    }
}
