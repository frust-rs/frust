//! Pure log-line classification: level, source, and a Rust panic/backtrace
//! fold-block state machine (workbook §B11 "Log styling"). Everything here is
//! plain data + pure functions run **once per line at push time**
//! ([`super::session_view::SessionView::push_line`]) — never at render — so
//! the hot render path only ever reads precomputed [`LineMeta`], matching the
//! existing `detect_level`/`strip_ansi` heuristic-probe style this module
//! supersedes (deliberately simple substring/token probes, not a parser; a
//! misfire is a colorizing hint, never a correctness hazard).
//!
//! The panic/backtrace state machine ([`PanicTracker`]) is reshaped from
//! fdemon's exception-parser pattern (`fdemon-app/src/session/session.rs`
//! `process_raw_line` → `exception_parser.feed_line`) onto Rust's own panic
//! shape: `thread '…' panicked at …:` starts a block, `stack backtrace:`
//! opens its foldable body, numbered ` N: ` frames and `at src/…`
//! continuations extend it, and the first non-matching line closes it — see
//! [`PanicTracker::feed`].

use std::time::{SystemTime, UNIX_EPOCH};

// ── Level ────────────────────────────────────────────────────────────────

/// A classified log level — always present (unlike the old `Option`-typed
/// `detect_level`; an ordinary line is simply [`LogLevel::Info`]), so the
/// level filter ([`LevelFilter`]) never special-cases "no level".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// An error / panic / failure / rustc `error[E…]:` line.
    Error,
    /// A warning / rustc `warning:` line.
    Warn,
    /// The default level — no badge, plain foreground.
    Info,
    /// A verbose/trace/debug line — dim, below `fg` contrast.
    Debug,
}

impl LogLevel {
    /// The one-letter badge glyph, or `None` for `Info` (the workbook's
    /// "blank — default level, no badge clutter" row).
    pub fn badge_char(self) -> Option<char> {
        match self {
            LogLevel::Error => Some('E'),
            LogLevel::Warn => Some('W'),
            LogLevel::Info => None,
            LogLevel::Debug => Some('D'),
        }
    }
}

/// Classify an already ANSI-stripped line's level. A substring/token probe,
/// not a parser: recognizes the `log` facade's uppercase level tokens, rustc
/// (`error[E0308]:`, `warning:`) and Gradle (`FAILURE:`, `BUILD FAILED`)
/// diagnostics, a Rust panic marker, and logcat's level column/prefix (see
/// [`classify_logcat_level`]).
///
/// Precedence, most to least specific (card m-b): panic marker > gradle/build
/// failure > `note:`/`help:` carve-out > `warning:`-prefixed diagnostic > the
/// word `error`/`errors` > generic `"warn"` substring fallback > `DEBUG`/
/// `TRACE` token > logcat level column/prefix > [`LogLevel::Info`] default.
/// The `warning:`-prefix check sits ahead of the error-word check so a
/// diagnostic tool's own severity prefix wins over any word its message
/// happens to quote (e.g. `` warning: unused variable: `error` `` stays
/// Warn, not Error). The generic `"warn"` fallback sits ahead of `DEBUG`/
/// `TRACE`/logcat — the order this module had before card W5 moved the
/// error-word check up front and left the fallback trailing behind it; no
/// test needs the opposite order, so the original relative position is kept.
pub fn classify_level(plain: &str) -> LogLevel {
    let l = plain.to_lowercase();

    // Check for panic first (highest priority).
    if l.contains("panicked at") {
        return LogLevel::Error;
    }

    // Check for gradle/build failures.
    if l.contains("failure:") || l.contains("build failed") {
        return LogLevel::Error;
    }

    // Check for note: or help: prefixes (after stripping source marker); these are not Error
    // even if they contain the word "error".
    let stripped = strip_source_prefix(plain);
    let trimmed = stripped.trim_start().to_lowercase();
    if trimmed.starts_with("note:") || trimmed.starts_with("help:") {
        return LogLevel::Info;
    }

    // Check for warning prefix (rustc/javac diagnostic shape); fire ONLY on diagnostic start,
    // and BEFORE the error-word check below — see this function's doc comment.
    if trimmed.starts_with("warning:") || trimmed.starts_with("warning[") {
        return LogLevel::Warn;
    }

    // Check for the word "error" (singular or plural) as a whole word, case-insensitive.
    // `l` is already lowercased above — pass it in rather than lowercasing again.
    if contains_error_word(&l) {
        return LogLevel::Error;
    }

    // Generic warning fallback (keep original behavior for log-facade WARN tokens, etc) — see
    // this function's doc comment for why it runs ahead of the DEBUG/TRACE/logcat checks below.
    if l.contains("warn") {
        return LogLevel::Warn;
    }

    // Check for debug/trace tokens (case-sensitive, whole-word only).
    if has_uppercase_word(plain, "DEBUG") || has_uppercase_word(plain, "TRACE") {
        // Case-*sensitive*, whole-word only — unlike the lowercase substring
        // checks above, a loose `l.contains("debug")` would misfire on the
        // extremely common `target/debug/…` cargo path (every `cargo run`'s
        // "Running `target/debug/app`" line). The `log` facade's own level
        // tokens are uppercase, so this still catches them.
        return LogLevel::Debug;
    }

    // Check for logcat level markers.
    if let Some(lvl) = classify_logcat_level(plain) {
        return lvl;
    }

    LogLevel::Info
}

/// Whether `haystack` contains `word` as a whole word (not adjacent to an
/// alphanumeric character on either side), matched case-sensitively.
fn has_uppercase_word(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(word) {
        let idx = start + pos;
        let before_ok = idx == 0 || !bytes[idx - 1].is_ascii_alphanumeric();
        let after = idx + word.len();
        let after_ok = after >= bytes.len() || !bytes[after].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = idx + word.len().max(1);
    }
    false
}

/// Strip a leading source marker like `[gradle] ` or `[logcat] ` from a line.
/// Returns the remainder of the line, or the full line if no marker is found.
fn strip_source_prefix(plain: &str) -> &str {
    let (_, strip_len) = classify_source(plain);
    if strip_len > 0 {
        &plain[strip_len..]
    } else {
        plain
    }
}

/// Whether an already-lowercased line contains "error" or "errors" as a whole
/// word (so `found 2 errors`/`3 errors generated` count, alongside the
/// singular). Delegates to [`contains_word_with_boundary`] for the shared
/// boundary rule — see its doc comment for why this module now uses one rule
/// instead of two: `error-code`, `thiserror`, `anyerror`, and `error_kind`
/// stay excluded either way.
fn contains_error_word(lower: &str) -> bool {
    contains_word_with_boundary(lower, "error") || contains_word_with_boundary(lower, "errors")
}

/// Whether `haystack` contains `word` as a whole word: the byte immediately
/// before and after a match, if any, must not be ASCII alphanumeric, `_`, or
/// `-`. This is the one boundary rule [`contains_error_word`] and (via
/// [`classify_level`]'s case-insensitive `error`/`errors` check) the former
/// separate `has_uppercase_word(plain, "ERROR")` case both used to apply
/// inconsistently — that call used a looser boundary (only excluding
/// alphanumerics, not `_`/`-`) and was otherwise redundant with the
/// case-insensitive check here, since lowercasing already folds any-case
/// `ERROR` into the same match. Dropping it removes the inconsistency
/// without changing observable behavior for any case this module classifies.
fn contains_word_with_boundary(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut start = 0;

    while let Some(pos) = haystack[start..].find(word) {
        let idx = start + pos;

        // Check character before: must be non-existent or not alphanumeric, '_', or '-'.
        let before_ok = idx == 0 || {
            let byte = bytes[idx - 1];
            !byte.is_ascii_alphanumeric() && byte != b'_' && byte != b'-'
        };

        // Check character after: must be non-existent or not alphanumeric, '_', or '-'.
        let after_idx = idx + word.len();
        let after_ok = after_idx >= bytes.len() || {
            let byte = bytes[after_idx];
            !byte.is_ascii_alphanumeric() && byte != b'_' && byte != b'-'
        };

        if before_ok && after_ok {
            return true;
        }

        start = idx + 1;
    }

    false
}

/// Recognize an Android logcat level from either the `brief` format's
/// leading `E/Tag(pid):` marker, or an isolated single-letter level token
/// near the start of a `threadtime`-formatted line (` … PID TID E Tag: …`).
/// `None` when neither shape is seen — most desktop/gradle output.
fn classify_logcat_level(plain: &str) -> Option<LogLevel> {
    if let Some((lvl, rest)) = plain.split_once('/')
        && lvl.len() == 1
        && rest.contains('(')
    {
        match lvl {
            "E" => return Some(LogLevel::Error),
            "W" => return Some(LogLevel::Warn),
            "D" => return Some(LogLevel::Debug),
            "I" | "V" => return Some(LogLevel::Info),
            _ => {}
        }
    }
    // threadtime's level column sits early in the line (after a date, time,
    // pid, tid) — restrict the scan so an unrelated lone letter deep in a
    // message can't misfire.
    for tok in plain.split_whitespace().take(6) {
        match tok {
            "E" => return Some(LogLevel::Error),
            "W" => return Some(LogLevel::Warn),
            "D" => return Some(LogLevel::Debug),
            _ => {}
        }
    }
    None
}

// ── Source ───────────────────────────────────────────────────────────────

/// A classified log source, rendered as a fixed-width muted tag
/// ([`SOURCE_TAG_WIDTH`]) so message text always starts in the same column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSource {
    /// Gradle build/install output — the drive pipeline already prefixes
    /// each line `"[gradle] "` (`frust-drive`'s `android_run`/`android_build`
    /// modules); [`classify_source`] strips it for display.
    Gradle,
    /// Android `adb logcat` output.
    Logcat,
    /// A `frust-perf` instrumentation line (see [`super::perf`]).
    Frust,
    /// Ordinary app/desktop process output — the default.
    App,
}

impl LogSource {
    /// The bracketed tag text (unpadded).
    pub fn tag(self) -> &'static str {
        match self {
            LogSource::Gradle => "[gradle]",
            LogSource::Logcat => "[logcat]",
            LogSource::Frust => "[frust]",
            LogSource::App => "[app]",
        }
    }
}

/// The column width every source tag right-pads to — the longest tag
/// (`[gradle]`/`[logcat]`) is 8 columns.
pub const SOURCE_TAG_WIDTH: usize = 8;

/// Classify an already ANSI-stripped line's source, returning the byte
/// length of a source-marker prefix (`"[gradle] "`) to strip from the *raw*
/// (still-ANSI) line for display — `0` when there is nothing to strip. The
/// stripped byte count is valid against the raw line too because the drive
/// pipeline's `"[gradle] "` marker is added outside of, and before, any ANSI
/// escape the wrapped tool's own output carries.
pub fn classify_source(plain: &str) -> (LogSource, usize) {
    const GRADLE_PREFIX: &str = "[gradle] ";
    if plain.starts_with(GRADLE_PREFIX) {
        return (LogSource::Gradle, GRADLE_PREFIX.len());
    }
    if plain.trim_start().starts_with("frust-perf ") {
        return (LogSource::Frust, 0);
    }
    if classify_logcat_level(plain).is_some() {
        return (LogSource::Logcat, 0);
    }
    (LogSource::App, 0)
}

// ── Per-line metadata + wall-clock timestamp ────────────────────────────────

/// A Rust panic/backtrace-block line's role — set once at classification
/// time so the render path never re-derives it (see [`PanicTracker`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineRole {
    /// An ordinary line, outside any panic block.
    Normal,
    /// `thread '…' panicked at …:` — always visible, never folded.
    PanicHeader,
    /// A message line between the panic header and the backtrace header
    /// (e.g. `called \`Option::unwrap()\` …`) — always visible.
    PanicMessage,
    /// `stack backtrace:` — the first foldable line of the block's body.
    BacktraceHeader,
    /// A numbered frame line (`   N: symbol`).
    Frame,
    /// A frame's `at src/…:L:C` location continuation.
    FrameLocation,
}

impl LineRole {
    /// Whether this line belongs to a panic block at all (anything but
    /// `Normal`) — such lines are always tagged [`LogLevel::Error`]
    /// regardless of their own text, so an entire block passes or fails the
    /// level filter as one unit (see [`super::session_view::SessionView`]).
    pub fn is_panic_related(self) -> bool {
        !matches!(self, LineRole::Normal)
    }

    /// Whether this line is part of a block's *foldable* body — hidden
    /// behind the `▶ n frames…` affordance while its block is collapsed.
    pub fn is_foldable(self) -> bool {
        matches!(
            self,
            LineRole::BacktraceHeader | LineRole::Frame | LineRole::FrameLocation
        )
    }

    /// Whether this line renders its own badge/timestamp/source prefix
    /// chrome. Every non-first line of a panic block renders a blank prefix
    /// instead (the whole block reads as one grouped entry under one
    /// timestamp — workbook §B11's backtrace mockup).
    pub fn shows_prefix_chrome(self) -> bool {
        matches!(self, LineRole::Normal | LineRole::PanicHeader)
    }
}

/// The precomputed, per-line render metadata — classified once at
/// [`super::session_view::SessionView::push_line`], read (never recomputed)
/// by [`crate::ui::views::sessions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMeta {
    /// The classified level (drives the badge + no-ANSI message tint).
    pub level: LogLevel,
    /// The classified source (drives the fixed-width tag column).
    pub source: LogSource,
    /// The wall-clock `HH:MM:SS` (UTC) this line was ingested at.
    pub timestamp: String,
    /// This line's role within a panic/backtrace block, if any.
    pub role: LineRole,
    /// The byte length of a source-marker prefix (e.g. `"[gradle] "`) the
    /// render path strips from the raw line before ANSI-parsing it for
    /// display — see [`classify_source`]'s return value.
    pub source_prefix_strip: usize,
}

/// Wall-clock `HH:MM:SS` (UTC) "now" — the log view's per-line timestamp
/// column. Hand-rolled from `SystemTime` (no `chrono`/`time` dependency,
/// consistent with this crate's OSC-52 base64 precedent in `runner.rs`):
/// only a plain clock read is needed, never a calendar date or timezone
/// conversion.
pub fn now_hms() -> String {
    hms_at(SystemTime::now())
}

/// [`now_hms`] for an arbitrary instant — the same `HH:MM:SS` (UTC) form, so
/// every wall-clock time the workbench shows (log lines, the MCP panel's
/// client connect times) reads the same way. A pre-epoch `at` (only reachable
/// from a clock stepped backwards) formats as `00:00:00` rather than failing.
pub fn hms_at(at: SystemTime) -> String {
    let secs = at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

// ── Panic/backtrace fold-block state machine ────────────────────────────────

/// One detected panic + (possibly still-streaming) backtrace block, addressed
/// by **absolute** line indices exactly like [`super::session_view::LineSelection`]
/// so it survives ring eviction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanicBlock {
    /// The absolute index of the `thread '…' panicked at …:` line — the
    /// block's stable identity (used as the fold-toggle key).
    pub start: u64,
    /// The absolute index of the first foldable body line (the `stack
    /// backtrace:` header), once seen.
    pub backtrace_start: Option<u64>,
    /// The absolute index of the last line currently known to belong to the
    /// block (grows while `open`).
    pub end: u64,
    /// The number of numbered frame lines seen so far.
    pub frame_count: usize,
    /// Whether the block can still be extended by the next pushed line
    /// (cleared once a non-matching line closes it).
    pub open: bool,
    /// Collapsed by default ([`PanicTracker::feed`] seeds `true` the moment
    /// a backtrace header is seen); toggled by [`PanicTracker::toggle`].
    pub collapsed: bool,
}

/// The state machine's current parse position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum TrackState {
    #[default]
    Idle,
    /// Seen the panic header; `u64` is the tracked block's stable `start`
    /// (looked up by value in `PanicTracker::blocks`, never a positional
    /// index — `evict_before` compacts the Vec, so a cached index would go
    /// stale/out-of-bounds the moment an earlier block is evicted while this
    /// one is still open; `start` survives that compaction unchanged).
    SawHeader(u64),
    /// Seen the backtrace header; collecting frames/locations. Same
    /// by-`start` addressing as `SawHeader`.
    InBacktrace(u64),
}

/// The maximum number of panic/backtrace blocks [`PanicTracker`] keeps at
/// once. Generous headroom for real use (a session showing signs of dozens
/// of concurrent unresolved panics is already deep in "something is very
/// wrong" territory) while bounding the pathological case this cap exists
/// for: a crash-looping app re-triggering the same panic on every restart
/// can open hundreds of blocks well within one still-live [`LOG_LINE_CAP`]
/// ring window (`evict_before`'s ring-position eviction alone would never
/// trim any of them, since none has scrolled out of the ring). Beyond the
/// cap, [`PanicTracker::push_block`] drops the *oldest* tracked block —
/// exactly the same fold-state-disappears UX [`PanicTracker::evict_before`]
/// already applies to a block that scrolls out of the ring: its lines stop
/// being grouped behind a `▶ n frames…` row and simply render as normal
/// lines again, never a panic or a stale fold toggle.
///
/// [`LOG_LINE_CAP`]: super::session_view::LOG_LINE_CAP
pub const MAX_TRACKED_BLOCKS: usize = 64;

/// Feeds pushed lines through the panic/backtrace block state machine,
/// one per [`super::session_view::SessionView::push_line`] call, and owns
/// the resulting block list + fold (collapsed) state.
///
/// `blocks` is sorted by `start` (and therefore by `backtrace_start` and
/// `end` too) by construction: a new block is only ever opened after the
/// previous one has closed (see [`TrackState`]), and `abs` strictly
/// increases with every [`Self::feed`] call, so ranges never overlap and
/// never appear out of order. [`Self::block_covering`] relies on this to
/// binary-search rather than linear-scan.
#[derive(Debug, Clone, Default)]
pub struct PanicTracker {
    state: TrackState,
    blocks: Vec<PanicBlock>,
}

impl PanicTracker {
    /// Classify one more line (already ANSI-stripped) at absolute index
    /// `abs`, advancing the state machine and returning its role.
    pub fn feed(&mut self, abs: u64, plain: &str) -> LineRole {
        match self.state {
            TrackState::Idle => {
                if is_panic_header(plain) {
                    self.push_block(PanicBlock {
                        start: abs,
                        backtrace_start: None,
                        end: abs,
                        frame_count: 0,
                        open: true,
                        collapsed: true,
                    });
                    self.state = TrackState::SawHeader(abs);
                    LineRole::PanicHeader
                } else {
                    LineRole::Normal
                }
            }
            TrackState::SawHeader(start) => {
                // Defensive: `evict_before` (called on every line, see its
                // doc comment) may have dropped the tracked block out from
                // under a stale state — a future change to its predicate
                // could make this reachable even though today's `end >=
                // base` retain can't fully evict a still-open block. Reset
                // to `Idle` and re-classify the line on its own merits
                // rather than risk a stale/wrong lookup.
                let Some(b) = self.blocks.iter_mut().find(|b| b.start == start) else {
                    self.state = TrackState::Idle;
                    return self.feed(abs, plain);
                };
                if is_backtrace_header(plain) {
                    b.backtrace_start = Some(abs);
                    b.end = abs;
                    self.state = TrackState::InBacktrace(start);
                    LineRole::BacktraceHeader
                } else if is_panic_header(plain) {
                    // A fresh panic header while the previous one never grew
                    // a backtrace — close it (no foldable body) and start
                    // tracking the new one.
                    b.open = false;
                    self.push_block(PanicBlock {
                        start: abs,
                        backtrace_start: None,
                        end: abs,
                        frame_count: 0,
                        open: true,
                        collapsed: true,
                    });
                    self.state = TrackState::SawHeader(abs);
                    LineRole::PanicHeader
                } else {
                    b.end = abs;
                    LineRole::PanicMessage
                }
            }
            TrackState::InBacktrace(start) => {
                let Some(b) = self.blocks.iter_mut().find(|b| b.start == start) else {
                    self.state = TrackState::Idle;
                    return self.feed(abs, plain);
                };
                if is_frame_line(plain) {
                    b.end = abs;
                    b.frame_count += 1;
                    LineRole::Frame
                } else if is_frame_location(plain) {
                    b.end = abs;
                    LineRole::FrameLocation
                } else {
                    // The first non-matching line closes the block (task
                    // spec: "end: first non-matching line") — re-feed it
                    // fresh from `Idle` so it's classified on its own merits
                    // (it may itself open a new panic block).
                    b.open = false;
                    self.state = TrackState::Idle;
                    self.feed(abs, plain)
                }
            }
        }
    }

    /// The blocks detected so far, oldest first, capped at
    /// [`MAX_TRACKED_BLOCKS`].
    pub fn blocks(&self) -> &[PanicBlock] {
        &self.blocks
    }

    /// Append a newly opened block, then enforce [`MAX_TRACKED_BLOCKS`] by
    /// dropping the oldest tracked block beyond the cap. This is the sole
    /// place a block is added, so the cap holds after every [`Self::feed`]
    /// call. Distinct from, and in addition to, [`Self::evict_before`]'s
    /// eviction by ring position — see [`MAX_TRACKED_BLOCKS`]'s doc comment
    /// for why both are needed.
    fn push_block(&mut self, block: PanicBlock) {
        self.blocks.push(block);
        if self.blocks.len() > MAX_TRACKED_BLOCKS {
            self.blocks.remove(0);
        }
    }

    /// The block whose foldable body (`backtrace_start..=end`) contains
    /// `abs`, if any.
    ///
    /// `O(log blocks)`: `blocks` is sorted (and non-overlapping) by
    /// construction — see [`PanicTracker`]'s doc comment — so at most one
    /// block, the last one whose `start <= abs`, could possibly cover `abs`;
    /// a full linear scan is unnecessary.
    pub fn block_covering(&self, abs: u64) -> Option<&PanicBlock> {
        let idx = self.blocks.partition_point(|b| b.start <= abs);
        idx.checked_sub(1)
            .and_then(|i| self.blocks.get(i))
            .filter(|b| {
                b.backtrace_start
                    .is_some_and(|bs| abs >= bs && abs <= b.end)
            })
    }

    /// Whether the block starting at absolute index `block_start` is
    /// currently collapsed (`false`/not-found for an unknown id — never
    /// panics on a stale/evicted id).
    pub fn is_collapsed(&self, block_start: u64) -> bool {
        self.blocks
            .iter()
            .find(|b| b.start == block_start)
            .is_some_and(|b| b.collapsed)
    }

    /// Toggle the fold state of the block starting at `block_start`.
    /// Returns whether a matching block was found.
    pub fn toggle(&mut self, block_start: u64) -> bool {
        if let Some(b) = self.blocks.iter_mut().find(|b| b.start == block_start) {
            b.collapsed = !b.collapsed;
            true
        } else {
            false
        }
    }

    /// Toggle the fold state of whichever block (with a known backtrace
    /// start) is nearest `near` (by absolute-index distance from its panic
    /// header), returning the toggled block's `start`. `None` when no block
    /// has a foldable body yet.
    pub fn toggle_nearest(&mut self, near: u64) -> Option<u64> {
        let target = self
            .blocks
            .iter()
            .filter(|b| b.backtrace_start.is_some())
            .min_by_key(|b| b.start.abs_diff(near))?
            .start;
        self.toggle(target);
        Some(target)
    }

    /// Drop blocks that fell entirely below `base` (fully evicted from the
    /// ring) — called from [`super::session_view::SessionView::push_line`]
    /// right after the ring evicts, mirroring `LineSelection`'s eviction
    /// clamp.
    pub fn evict_before(&mut self, base: u64) {
        self.blocks.retain(|b| b.end >= base);
    }
}

fn is_panic_header(plain: &str) -> bool {
    plain.trim_start().starts_with("thread '") && plain.contains("panicked at")
}

fn is_backtrace_header(plain: &str) -> bool {
    plain.trim().eq_ignore_ascii_case("stack backtrace:")
}

fn is_frame_line(plain: &str) -> bool {
    let t = plain.trim_start();
    let digits: usize = t.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && t[digits..].starts_with(':')
}

fn is_frame_location(plain: &str) -> bool {
    plain.trim_start().starts_with("at ")
}

// ── Level filter (per-session log-view state) ───────────────────────────────

/// The active minimum-level cutoff for a session's log view — a segmented
/// pill cycled by key or jumped to directly by click (workbook §B11's
/// "Level-filter chip").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LevelFilter {
    /// No filtering — every level shows.
    #[default]
    All,
    /// Hides [`LogLevel::Debug`] only.
    InfoPlus,
    /// [`LogLevel::Warn`] and [`LogLevel::Error`] only.
    WarnPlus,
    /// [`LogLevel::Error`] only.
    ErrorOnly,
}

/// The four segments in their fixed cycle order (also the click-to-jump
/// order the filter chip renders).
pub const LEVEL_FILTER_SEGMENTS: [LevelFilter; 4] = [
    LevelFilter::All,
    LevelFilter::InfoPlus,
    LevelFilter::WarnPlus,
    LevelFilter::ErrorOnly,
];

impl LevelFilter {
    /// Whether a line at `level` passes this cutoff.
    pub fn allows(self, level: LogLevel) -> bool {
        match self {
            LevelFilter::All => true,
            LevelFilter::InfoPlus => !matches!(level, LogLevel::Debug),
            LevelFilter::WarnPlus => matches!(level, LogLevel::Warn | LogLevel::Error),
            LevelFilter::ErrorOnly => matches!(level, LogLevel::Error),
        }
    }

    /// Step `delta` positions through [`LEVEL_FILTER_SEGMENTS`], wrapping.
    pub fn cycle(self, delta: isize) -> Self {
        let cur = LEVEL_FILTER_SEGMENTS
            .iter()
            .position(|f| *f == self)
            .unwrap_or(0) as isize;
        let n = LEVEL_FILTER_SEGMENTS.len() as isize;
        LEVEL_FILTER_SEGMENTS[(cur + delta).rem_euclid(n) as usize]
    }

    /// The chip's segment label.
    pub fn label(self) -> &'static str {
        match self {
            LevelFilter::All => "all",
            LevelFilter::InfoPlus => "info+",
            LevelFilter::WarnPlus => "warn+",
            LevelFilter::ErrorOnly => "error only",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Level classification fixtures ───────────────────────────────────────

    #[test]
    fn classifies_cargo_rustc_diagnostics() {
        assert_eq!(
            classify_level("error[E0308]: mismatched types"),
            LogLevel::Error
        );
        assert_eq!(
            classify_level("warning: unused variable: `x`"),
            LogLevel::Warn
        );
        assert_eq!(classify_level("   Compiling app v0.1.0"), LogLevel::Info);
    }

    #[test]
    fn classifies_gradle_failure() {
        assert_eq!(
            classify_level("FAILURE: Build failed with an exception."),
            LogLevel::Error
        );
        assert_eq!(
            classify_level("Note: MainActivity.kt uses or overrides a deprecated API."),
            LogLevel::Info
        );
    }

    #[test]
    fn classifies_logcat_brief_and_threadtime() {
        assert_eq!(
            classify_level("E/ActivityThread( 1234): Failed to find provider info"),
            LogLevel::Error
        );
        assert_eq!(
            classify_level(
                "08-09 12:04:24.001  1234  1234 W ActivityThread: Failed to find provider info"
            ),
            LogLevel::Warn
        );
        assert_eq!(
            classify_level("08-09 12:04:26.001  1234  1234 D SomeTag: poll tick"),
            LogLevel::Debug
        );
    }

    #[test]
    fn a_debug_build_path_is_not_misclassified_as_the_debug_level() {
        // Regression: a loose lowercase `contains("debug")` would tag every
        // `cargo run`'s "Running `target/debug/app`" line as Debug-level.
        assert_eq!(
            classify_level("     Running `target/debug/huddle`"),
            LogLevel::Info
        );
        // The `log` facade's own uppercase token still classifies.
        assert_eq!(
            classify_level("2026-08-09T12:00:00Z DEBUG my_app: poll tick"),
            LogLevel::Debug
        );
    }

    #[test]
    fn classifies_rust_panic() {
        assert_eq!(
            classify_level("thread 'main' panicked at src/main.rs:42:9:"),
            LogLevel::Error
        );
    }

    #[test]
    fn does_not_misclassify_error_crate_names() {
        // Regression: whole-word match must not tag crate names as Error.
        assert_eq!(
            classify_level("   Compiling error-code v3.4.0"),
            LogLevel::Info
        );
        assert_eq!(
            classify_level("   Compiling thiserror-impl v2.0.0"),
            LogLevel::Info
        );
    }

    #[test]
    fn does_not_misclassify_error_in_other_contexts() {
        // A note or warning line with "error" as part of a message keeps its own level.
        assert_eq!(
            classify_level("note: this may become a hard error"),
            LogLevel::Info
        );
        assert_eq!(
            classify_level("warning: [deprecation] foo() in Bar has been deprecated; error-prone"),
            LogLevel::Warn
        );
    }

    #[test]
    fn classifies_gradle_error_with_source_prefix() {
        // Gradle errors with source prefix should still be Error.
        assert_eq!(
            classify_level("[gradle] error: cannot find symbol"),
            LogLevel::Error
        );
    }

    #[test]
    fn classifies_rust_error_enum_variant() {
        // Rust Error enum variant should be classified as Error.
        assert_eq!(
            classify_level("Error: Os { code: 2, kind: NotFound, message: \"x\" }"),
            LogLevel::Error
        );
    }

    #[test]
    fn still_classifies_actual_error_tokens() {
        // Ensure the fix doesn't break the original error detection.
        assert_eq!(classify_level("error: could not compile"), LogLevel::Error);
        assert_eq!(
            classify_level("error[E0425]: cannot find value `x` in this scope"),
            LogLevel::Error
        );
        assert_eq!(
            classify_level("   error: mismatched types"),
            LogLevel::Error
        );
    }

    #[test]
    fn rustc_error_summary_with_warnings_is_error() {
        // The rustc summary line contains both "error" and "warnings".
        // It should classify as Error, not Warn.
        assert_eq!(
            classify_level(
                "error: could not compile `app` (bin \"app\") due to 2 previous errors; 3 warnings emitted"
            ),
            LogLevel::Error
        );
    }

    #[test]
    fn gradle_warning_with_error_prone_is_warn() {
        // Gradle warnings that mention "error-prone" as a tool/concept should stay Warn,
        // because the line starts with "warning:" diagnostic shape.
        assert_eq!(
            classify_level(
                "[gradle] warning: [deprecation] foo() in Bar has been deprecated; error-prone"
            ),
            LogLevel::Warn
        );
    }

    #[test]
    fn warning_prefixed_lines_win_over_the_word_error() {
        // card m-b, finding 1: a `warning:`-prefixed diagnostic line stays
        // Warn even when its own message quotes the word "error" — the
        // diagnostic tool's own severity prefix wins over any word its
        // message happens to quote.
        assert_eq!(
            classify_level("warning: unused variable: `error`"),
            LogLevel::Warn
        );
        assert_eq!(
            classify_level("[gradle] warning: unused variable: `error`"),
            LogLevel::Warn
        );
    }

    #[test]
    fn plural_errors_counts_as_the_error_word() {
        // card m-b, finding 2: plural "errors" must classify Error just like
        // the singular.
        assert_eq!(classify_level("found 2 errors"), LogLevel::Error);
        assert_eq!(classify_level("3 errors generated"), LogLevel::Error);
        assert_eq!(
            classify_level(
                "error: could not compile `app` due to 2 previous errors; 3 warnings emitted"
            ),
            LogLevel::Error
        );
    }

    #[test]
    fn uppercase_warn_token_is_warn() {
        // WARN uppercase log-facade token (not the diagnostic "warning:" shape).
        assert_eq!(classify_level("WARN something"), LogLevel::Warn);
    }

    #[test]
    fn classify_source_strips_the_gradle_marker() {
        let (source, strip) = classify_source("[gradle] > Task :app:assembleDebug");
        assert_eq!(source, LogSource::Gradle);
        assert_eq!(
            &"[gradle] > Task :app:assembleDebug"[strip..],
            "> Task :app:assembleDebug"
        );
    }

    #[test]
    fn classify_source_recognizes_frust_perf_and_logcat_and_defaults_to_app() {
        assert_eq!(
            classify_source("frust-perf frame n=64 total_p50_ms=8").0,
            LogSource::Frust
        );
        assert_eq!(
            classify_source("E/ActivityThread( 1234): boom").0,
            LogSource::Logcat
        );
        assert_eq!(classify_source("app: booting up").0, LogSource::App);
    }

    // ── Panic/backtrace tracker ──────────────────────────────────────────────

    fn feed_all(tracker: &mut PanicTracker, lines: &[&str]) -> Vec<LineRole> {
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| tracker.feed(i as u64, l))
            .collect()
    }

    const PANIC_LINES: &[&str] = &[
        "thread 'main' panicked at src/main.rs:42:9:",
        "called `Option::unwrap()` on a `None` value",
        "stack backtrace:",
        "   0: rust_begin_unwind",
        "   1: my_app::state::reduce",
        "             at src/state.rs:88:13",
        "   2: my_app::widget::on_tap",
        "             at src/widget.rs:206:21",
        "note: run with `RUST_BACKTRACE=full` for a verbose backtrace.",
        "app: recovering",
    ];

    #[test]
    fn tracker_classifies_a_full_panic_block() {
        let mut tracker = PanicTracker::default();
        let roles = feed_all(&mut tracker, PANIC_LINES);
        assert_eq!(
            roles,
            vec![
                LineRole::PanicHeader,
                LineRole::PanicMessage,
                LineRole::BacktraceHeader,
                LineRole::Frame,
                LineRole::Frame,
                LineRole::FrameLocation,
                LineRole::Frame,
                LineRole::FrameLocation,
                LineRole::Normal, // the "note: ..." line closes the block
                LineRole::Normal,
            ]
        );
        let blocks = tracker.blocks();
        assert_eq!(blocks.len(), 1);
        let b = blocks[0];
        assert_eq!(b.start, 0);
        assert_eq!(b.backtrace_start, Some(2));
        assert_eq!(b.end, 7); // the last frame-location line
        assert_eq!(b.frame_count, 3);
        assert!(!b.open); // closed by the "note:" line
        assert!(b.collapsed); // collapsed by default
    }

    #[test]
    fn tracker_grows_a_block_incrementally_across_pushes() {
        let mut tracker = PanicTracker::default();
        // Feed the block one line at a time, as it would stream in live.
        for (i, line) in PANIC_LINES[..6].iter().enumerate() {
            tracker.feed(i as u64, line);
        }
        let mid = tracker.blocks()[0];
        assert!(mid.open);
        assert_eq!(mid.frame_count, 2);
        // More frames stream in later.
        for (i, line) in PANIC_LINES[6..].iter().enumerate() {
            tracker.feed(6 + i as u64, line);
        }
        let done = tracker.blocks()[0];
        assert!(!done.open);
        assert_eq!(done.frame_count, 3);
    }

    #[test]
    fn tracker_block_covering_matches_only_the_foldable_body() {
        let mut tracker = PanicTracker::default();
        feed_all(&mut tracker, PANIC_LINES);
        // The panic header/message (0, 1) are never foldable.
        assert!(tracker.block_covering(0).is_none());
        assert!(tracker.block_covering(1).is_none());
        // The backtrace header through the last frame location (2..=7) are.
        for abs in 2..=7 {
            assert_eq!(tracker.block_covering(abs).map(|b| b.start), Some(0));
        }
        assert!(tracker.block_covering(8).is_none());
    }

    #[test]
    fn tracker_toggle_and_toggle_nearest() {
        let mut tracker = PanicTracker::default();
        feed_all(&mut tracker, PANIC_LINES);
        assert!(tracker.is_collapsed(0));
        assert!(tracker.toggle(0));
        assert!(!tracker.is_collapsed(0));
        assert!(!tracker.toggle(999)); // unknown id — no panic, no-op
        assert_eq!(tracker.toggle_nearest(3), Some(0));
        assert!(tracker.is_collapsed(0)); // toggled back by the nearest-toggle
    }

    #[test]
    fn tracker_evict_before_drops_fully_evicted_blocks_only() {
        let mut tracker = PanicTracker::default();
        feed_all(&mut tracker, PANIC_LINES); // block spans abs 0..=7
        tracker.evict_before(3); // partial eviction — block still referenced
        assert_eq!(tracker.blocks().len(), 1);
        tracker.evict_before(8); // now fully below base
        assert!(tracker.blocks().is_empty());
    }

    #[test]
    fn tracked_blocks_are_capped_dropping_the_oldest_first() {
        // A crash-looping app re-triggering the same (backtrace-less) panic
        // well within one still-live ring window: `evict_before` alone would
        // never trim any of these (none has scrolled out of the ring), so
        // the count cap is the only thing bounding memory/lookup cost.
        let mut tracker = PanicTracker::default();
        let total = MAX_TRACKED_BLOCKS + 20;
        for i in 0..total as u64 {
            tracker.feed(i, &format!("thread 'main' panicked at src/main.rs:{i}:1:"));
        }
        assert_eq!(tracker.blocks().len(), MAX_TRACKED_BLOCKS);
        // Oldest 20 blocks (starts 0..20) evicted; the newest MAX_TRACKED_BLOCKS
        // survive, oldest-first.
        assert_eq!(tracker.blocks().first().unwrap().start, 20);
        assert_eq!(tracker.blocks().last().unwrap().start, total as u64 - 1);
    }

    #[test]
    fn cap_eviction_drops_fold_state_exactly_like_ring_eviction() {
        // Block 0 gets a real foldable backtrace body; then enough further
        // panic headers stream in to push it past MAX_TRACKED_BLOCKS, purely
        // by *count* — the ring (LOG_LINE_CAP) never comes into play here.
        let mut tracker = PanicTracker::default();
        for (i, line) in PANIC_LINES.iter().enumerate() {
            tracker.feed(i as u64, line);
        }
        assert!(tracker.block_covering(3).is_some());
        assert!(tracker.is_collapsed(0));

        for abs in (PANIC_LINES.len() as u64..).take(MAX_TRACKED_BLOCKS) {
            tracker.feed(
                abs,
                &format!("thread 'main' panicked at src/main.rs:{abs}:1:"),
            );
        }
        // Block 0 is now cap-evicted: covering lookups return None and the
        // fold toggle is a stale-id no-op — the same shape
        // `fold_group_is_dropped_once_fully_evicted_from_the_ring` asserts
        // for ring eviction, never a panic.
        assert!(tracker.block_covering(3).is_none());
        assert!(!tracker.is_collapsed(0));
        assert!(!tracker.toggle(0));
    }

    #[test]
    fn evicting_an_earlier_block_mid_track_of_a_later_one_does_not_panic() {
        // Regression for the M1 crash: block A completes and closes, block B
        // opens and is still `InBacktrace` when `evict_before` compacts A out
        // of `blocks` — the state machine must resolve B by its stable
        // `start`, not by a now-stale Vec position.
        let mut tracker = PanicTracker::default();
        // Block A: full panic + backtrace, lines 0..=7 (mirrors PANIC_LINES).
        for (i, line) in PANIC_LINES.iter().enumerate() {
            tracker.feed(i as u64, line);
        }
        assert_eq!(tracker.blocks().len(), 1);
        assert!(!tracker.blocks()[0].open);

        // Some ordinary lines stream by without yet evicting A — mirroring
        // `push_line_at`'s per-line `feed → push → evict_before`, but with a
        // ring base that hasn't outgrown A's `end` (7) yet, so A is still
        // present in `blocks` (at Vec position 0) when B opens below.
        for i in 10..15u64 {
            tracker.feed(i, "app: still running");
        }

        // Block B opens at abs 20 — with A still present, B lands at Vec
        // position 1, which is exactly the stale index the pre-fix code
        // would have cached.
        assert_eq!(
            tracker.feed(20, "thread 'b' panicked at src/b.rs:5:5:"),
            LineRole::PanicHeader
        );
        assert_eq!(
            tracker.feed(21, "called `Option::unwrap()` on a `None` value"),
            LineRole::PanicMessage
        );
        assert_eq!(
            tracker.feed(22, "stack backtrace:"),
            LineRole::BacktraceHeader
        );

        // Evict now — A (end=7) is fully below base and gets dropped, while
        // B (start=20) is still open and mid-`InBacktrace`. Pre-fix, this
        // compacted B from Vec position 1 to 0 while `InBacktrace(1)` still
        // pointed at the old position — the next feed indexed out of bounds.
        tracker.evict_before(15);
        assert_eq!(tracker.blocks().len(), 1);
        assert_eq!(tracker.blocks()[0].start, 20);

        // Continue feeding B's backtrace body — no panic, fields correct.
        assert_eq!(tracker.feed(23, "   0: rust_begin_unwind"), LineRole::Frame);
        assert_eq!(
            tracker.feed(24, "   1: my_app::state::reduce"),
            LineRole::Frame
        );
        assert_eq!(
            tracker.feed(25, "             at src/state.rs:88:13"),
            LineRole::FrameLocation
        );
        assert_eq!(tracker.feed(26, "app: recovered"), LineRole::Normal);

        let b = tracker.blocks()[0];
        assert_eq!(b.start, 20);
        assert_eq!(b.backtrace_start, Some(22));
        assert_eq!(b.end, 25);
        assert_eq!(b.frame_count, 2);
        assert!(!b.open);
    }

    #[test]
    fn a_line_that_only_looks_like_a_frame_number_is_not_matched_outside_a_backtrace() {
        let mut tracker = PanicTracker::default();
        assert_eq!(tracker.feed(0, "   0: not a real frame"), LineRole::Normal);
    }

    #[test]
    fn nested_panic_header_closes_the_open_block_without_a_backtrace() {
        let mut tracker = PanicTracker::default();
        assert_eq!(
            tracker.feed(0, "thread 'a' panicked at a.rs:1:1:"),
            LineRole::PanicHeader
        );
        assert_eq!(
            tracker.feed(1, "thread 'b' panicked at b.rs:2:2:"),
            LineRole::PanicHeader
        );
        assert_eq!(tracker.blocks().len(), 2);
        assert!(!tracker.blocks()[0].open);
        assert!(tracker.blocks()[1].open);
    }

    // ── Level filter ─────────────────────────────────────────────────────────

    #[test]
    fn level_filter_allows_matrix() {
        use LevelFilter::*;
        use LogLevel::*;
        assert!(All.allows(Debug) && All.allows(Error));
        assert!(InfoPlus.allows(Info) && !InfoPlus.allows(Debug));
        assert!(WarnPlus.allows(Warn) && WarnPlus.allows(Error) && !WarnPlus.allows(Info));
        assert!(ErrorOnly.allows(Error) && !ErrorOnly.allows(Warn));
    }

    #[test]
    fn level_filter_cycles_forward_and_back_and_wraps() {
        assert_eq!(LevelFilter::All.cycle(1), LevelFilter::InfoPlus);
        assert_eq!(LevelFilter::ErrorOnly.cycle(1), LevelFilter::All);
        assert_eq!(LevelFilter::All.cycle(-1), LevelFilter::ErrorOnly);
    }

    #[test]
    fn now_hms_is_well_formed() {
        let ts = now_hms();
        assert_eq!(ts.len(), 8);
        assert_eq!(ts.as_bytes()[2], b':');
        assert_eq!(ts.as_bytes()[5], b':');
    }
}
