//! The per-session DevTools view-model: what the workbench knows about a
//! running app's in-process debug service (workbook §B12) — whether the
//! session's build could even host one, the discovery line it printed, the
//! connection the bridge holds on its behalf, which tab is showing, and the
//! frame-stats ring the Performance tab draws.
//!
//! Everything here is plain data + pure transitions — no sockets, no threads,
//! no `DevtoolsClient`. The moving part is [`crate::supervise::DevtoolsBridge`],
//! which owns the connection and reports back as [`ConnEvent`]s; this module
//! only mirrors what it says, exactly as [`super::session_view`] mirrors the
//! process supervisor.
//!
//! # Extension points (the per-tab payload slots)
//!
//! [`DevtoolsState`] deliberately holds only the state the chrome itself
//! needs plus [`DevtoolsState::frames`] (Performance's ring) and
//! [`DevtoolsState::performance`] (Performance's own scrub/focus state — see
//! [`PerformanceTab`]). The remaining three tabs each add their own payload
//! slot here — a System metrics/CPU sample, an Inspector widget-tree
//! snapshot + selection, a Network counter ring — fed by new [`ConnEvent`]
//! variants the bridge forwards. Adding a slot is additive: nothing outside
//! this module reads the ring or the connection state except through the
//! accessors below and [`DevtoolsState::phase`], whose five values are the
//! §B12 screen set and are matched exhaustively by the render and
//! key-routing layers.
//!
//! Performance can also draw from a *second* source when there is no live
//! connection: [`perf_window`] folds [`super::PerfPanel`]'s
//! already-ingested `frust-perf raw` log lines into the same [`PerfFrame`]
//! shape the live ring produces, selected by the pure [`select_perf_source`]
//! truth table — see that function's doc for exactly what a log-fallback
//! frame can and can't show.

use std::collections::VecDeque;

use frust_devtools_protocol::{Capability, Discovery, FrameStats};
use frust_drive::build_info::BuildMode;

use super::perf::PerfPanel;

/// How many frame-stats samples a session retains. Comfortably more than the
/// 120-frame window §B12's Performance chart scrubs, so a scrub back through
/// the ring has history behind the drawn window; bounded so a long-lived
/// session's ring can never grow without limit (the same drop-oldest ring
/// discipline the log buffer uses one layer up).
pub const FRAME_RING_CAP: usize = 240;

/// The four §B12 DevTools tabs, in their `1`–`4` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DevtoolsTab {
    /// Frame timing: the frame-stats ring + per-frame breakdown.
    #[default]
    Performance,
    /// Process metrics: RSS/uptime off the wire, CPU/thermal off a collector.
    System,
    /// The widget tree + the selected node's props.
    Inspector,
    /// Coarse process-level rx/tx byte counters.
    Network,
}

impl DevtoolsTab {
    /// Every tab in `1`–`4` order — the tab strip's render order and the
    /// sequence [`Self::cycle`] steps through.
    pub const ALL: [DevtoolsTab; 4] = [
        DevtoolsTab::Performance,
        DevtoolsTab::System,
        DevtoolsTab::Inspector,
        DevtoolsTab::Network,
    ];

    /// The tab's 0-based position in [`Self::ALL`] (its `1`–`4` key, minus one).
    pub fn index(self) -> usize {
        match self {
            DevtoolsTab::Performance => 0,
            DevtoolsTab::System => 1,
            DevtoolsTab::Inspector => 2,
            DevtoolsTab::Network => 3,
        }
    }

    /// The tab at 0-based position `index`, or `None` past the last tab.
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// The tab `delta` positions away, wrapping in both directions (`[`/`]`).
    pub fn cycle(self, delta: isize) -> Self {
        let len = Self::ALL.len() as isize;
        let next = (self.index() as isize + delta).rem_euclid(len);
        Self::ALL[next as usize]
    }

    /// The tab strip label (without its `[n]` key prefix).
    pub fn title(self) -> &'static str {
        match self {
            DevtoolsTab::Performance => "Performance",
            DevtoolsTab::System => "System",
            DevtoolsTab::Inspector => "Inspector",
            DevtoolsTab::Network => "Network",
        }
    }
}

/// What the session's own launch config says about reaching a devtools
/// service — captured at registration (`super::Message::RegisterSession`)
/// because it is a property of *how the session was launched*, not something
/// recoverable from its output later.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DevtoolsLaunch {
    /// Whether the build could host the service at all. A release build
    /// compiles the listener out entirely (`BuildMode::cargo_features`), so
    /// it never prints a discovery line and never will — §B12's
    /// app-without-devtools state is reached by this check, never by a
    /// timeout guess that would mislabel a slow debug build.
    pub capable: bool,
    /// The `adb` serial of the Android device the session runs on, when it is
    /// one. The service binds device-loopback, so reaching it from the host
    /// needs an `adb forward` — see
    /// [`crate::supervise::DevtoolsBridge`]. `None` for desktop and iOS
    /// (simulator/desktop connect straight over host loopback; iOS physical
    /// forwarding is not implemented — see `docs/LIMITATIONS.md`).
    pub android_serial: Option<String>,
}

impl DevtoolsLaunch {
    /// A session that can never host a devtools service — an ad-hoc
    /// build/clean/toolchain-fix session, which runs a tool rather than the
    /// app.
    pub fn unavailable() -> Self {
        Self::default()
    }

    /// The launch metadata for a real app session: capability is read off the
    /// build mode's own feature funnel (`BuildMode::cargo_features` selects
    /// `frust/devtools` for Debug/Profile and never for Release), so this
    /// layer never re-encodes the mode → feature mapping.
    pub fn from_launch(mode: BuildMode, android_serial: Option<String>) -> Self {
        Self {
            capable: mode.cargo_features().contains(&"frust/devtools"),
            android_serial,
        }
    }
}

/// The state of the bridge's connection for one session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnState {
    /// No connection attempt is in flight (nothing discovered yet, or
    /// DevTools has never been opened for this session).
    #[default]
    Idle,
    /// A connect + `handshake` is in flight on the bridge thread.
    Connecting,
    /// `handshake` succeeded; the bridge is streaming frame stats.
    Connected {
        /// The app the service reported at handshake.
        app_name: String,
        /// The capability set it declared.
        caps: Vec<Capability>,
    },
    /// The connection failed (or ended): the reason, verbatim, for the §B12
    /// failed screen.
    Failed {
        /// The failure text shown under the headline.
        error: String,
    },
}

/// The §B12 screen a [`DevtoolsState`] currently shows. Five values, matched
/// exhaustively by both the render layer and the key router, so a sixth
/// screen cannot be added without both sites handling it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevtoolsPhase {
    /// A release build: the listener is compiled out, so this never resolves.
    Unavailable,
    /// No discovery line seen in this session's log yet.
    Discovering,
    /// A discovery line was parsed; connect + handshake is in flight.
    Connecting,
    /// Handshake done — the tab strip and the active tab's body show.
    Connected,
    /// The connection failed; `r` retries.
    Failed,
}

/// What one [`crate::supervise::DevtoolsBridge`] thread reports back about
/// its connection. Carried on `super::Message::DevtoolsConn` and applied by
/// [`DevtoolsState::apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnEvent {
    /// `handshake` succeeded.
    Connected {
        /// The app name the service reported.
        app_name: String,
        /// The capability set it declared.
        caps: Vec<Capability>,
    },
    /// The connection could not be established, or died mid-stream.
    Failed(String),
    /// The bridge shut the connection down cleanly (an explicit disconnect).
    Closed,
    /// A coalesced batch of frame-stats samples, oldest first.
    Frames(Vec<FrameStats>),
}

/// One session's DevTools state: the §B12 mode's open/closed flag, what is
/// known about the service, the connection, the selected tab, and the
/// Performance ring.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DevtoolsState {
    /// Whether this session's tab is showing DevTools instead of its log
    /// (`d` toggles it; per session, so switching session tabs never forces
    /// you out of DevTools on the tab where you opened it).
    pub open: bool,
    /// What the session's launch config said about the service.
    pub launch: DevtoolsLaunch,
    /// The most recent discovery line parsed out of this session's log.
    /// Latest wins: an app that restarts within one session (a device
    /// relaunch) announces a fresh port/token, and the newest announcement is
    /// the live one.
    pub discovered: Option<Discovery>,
    /// The bridge's connection state.
    pub conn: ConnState,
    /// Which tab the §B12 tab strip has selected.
    pub active_tab: DevtoolsTab,
    /// The frame-stats ring, oldest first, capped at [`FRAME_RING_CAP`]
    /// (drop-oldest). The Performance tab draws its window from the tail.
    pub frames: VecDeque<FrameStats>,
    /// The Performance tab's own interaction state (scrub selection, pane
    /// focus) — see [`PerformanceTab`].
    pub performance: PerformanceTab,
}

impl DevtoolsState {
    /// A fresh state for a session launched per `launch`.
    pub fn new(launch: DevtoolsLaunch) -> Self {
        Self {
            launch,
            ..Self::default()
        }
    }

    /// The §B12 screen this state shows — the single derivation both the
    /// render dispatch and the key router read.
    pub fn phase(&self) -> DevtoolsPhase {
        if !self.launch.capable {
            return DevtoolsPhase::Unavailable;
        }
        match &self.conn {
            ConnState::Connected { .. } => DevtoolsPhase::Connected,
            ConnState::Failed { .. } => DevtoolsPhase::Failed,
            ConnState::Connecting => DevtoolsPhase::Connecting,
            ConnState::Idle => DevtoolsPhase::Discovering,
        }
    }

    /// Feed one ANSI-stripped log line: if it carries a discovery line,
    /// remember it. Returns `true` when the stored discovery actually changed
    /// (a first announcement, or a re-announcement with a different
    /// port/token) — the caller then opens (or replaces) the connection.
    pub fn ingest_line(&mut self, plain: &str) -> bool {
        let Some(found) = frust_devtools_protocol::parse_discovery_line(plain) else {
            return false;
        };
        if self.discovered.as_ref() == Some(&found) {
            return false;
        }
        self.discovered = Some(found);
        true
    }

    /// Whether a connect attempt is worth making right now: the build can
    /// host a service, a discovery line has landed, and nothing is already
    /// connected or in flight.
    pub fn wants_connect(&self) -> bool {
        self.launch.capable
            && self.discovered.is_some()
            && matches!(self.conn, ConnState::Idle | ConnState::Failed { .. })
    }

    /// Mark a connect attempt as started (the engine emits the matching
    /// `super::Effect::DevtoolsConnect` alongside).
    pub fn begin_connect(&mut self) {
        self.conn = ConnState::Connecting;
    }

    /// Apply one bridge report. Returns whether the visible state changed.
    pub fn apply(&mut self, event: ConnEvent) -> bool {
        match event {
            ConnEvent::Connected { app_name, caps } => {
                let next = ConnState::Connected { app_name, caps };
                let changed = self.conn != next;
                self.conn = next;
                changed
            }
            ConnEvent::Failed(error) => {
                let next = ConnState::Failed { error };
                let changed = self.conn != next;
                self.conn = next;
                changed
            }
            ConnEvent::Closed => {
                let changed = self.conn != ConnState::Idle;
                self.conn = ConnState::Idle;
                changed
            }
            ConnEvent::Frames(batch) => {
                let any = !batch.is_empty();
                for stats in batch {
                    self.push_frame(stats);
                }
                any
            }
        }
    }

    /// Push one frame-stats sample into the ring, evicting the oldest past
    /// [`FRAME_RING_CAP`].
    pub fn push_frame(&mut self, stats: FrameStats) {
        self.frames.push_back(stats);
        while self.frames.len() > FRAME_RING_CAP {
            self.frames.pop_front();
        }
    }

    /// Select a tab by its 0-based position (`1`–`4`, or a tab-pill click).
    /// Returns whether the selection changed; an out-of-range index is a
    /// no-op.
    pub fn select_tab(&mut self, index: usize) -> bool {
        match DevtoolsTab::from_index(index) {
            Some(tab) if tab != self.active_tab => {
                self.active_tab = tab;
                true
            }
            _ => false,
        }
    }

    /// Step the selection `delta` tabs (`[`/`]`), wrapping.
    pub fn cycle_tab(&mut self, delta: isize) -> bool {
        let next = self.active_tab.cycle(delta);
        let changed = next != self.active_tab;
        self.active_tab = next;
        changed
    }

    /// Mark the connection as gone because the session itself ended — the
    /// service went away with the process, so this is a terminal condition
    /// for the session, not something `r` can retry (see
    /// `super::update`'s retry gate).
    pub fn on_session_end(&mut self) -> bool {
        if matches!(
            self.conn,
            ConnState::Connecting | ConnState::Connected { .. }
        ) {
            self.conn = ConnState::Failed {
                error: "the session ended — its devtools service went with it".to_string(),
            };
            true
        } else {
            false
        }
    }
}

// ── Performance tab (workbook §B12) ─────────────────────────────────────────

/// How many of the ring's newest frames the Performance chart scrubs —
/// §B12's "120-frame ring". Strictly smaller than [`FRAME_RING_CAP`], which
/// retains extra history behind the drawn window.
pub const PERF_WINDOW: usize = 120;

/// Which pane of the Performance tab has keyboard focus (`Tab` cycles
/// between them). Two values today; [`Self::toggle`] is a plain flip rather
/// than [`DevtoolsTab::cycle`]'s wrapping-`n`-way shape since a third pane
/// isn't on the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PerfFocus {
    /// The frame-time chart — `←`/`→` scrub the selection here.
    #[default]
    Chart,
    /// The selected frame's per-phase breakdown bar.
    Breakdown,
}

impl PerfFocus {
    /// The other pane.
    pub fn toggle(self) -> Self {
        match self {
            PerfFocus::Chart => PerfFocus::Breakdown,
            PerfFocus::Breakdown => PerfFocus::Chart,
        }
    }
}

/// The Performance tab's own interaction state: which pane has focus, and
/// which ring frame (if any) is pinned for inspection. `selected_frame` is a
/// 0-based index into whatever window [`perf_window`] currently returns
/// (oldest first) — `None` means "follow the live tail", the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerformanceTab {
    /// Which pane `Tab` moves between.
    pub focus: PerfFocus,
    /// The scrubbed-to frame, or `None` to track the newest.
    pub selected_frame: Option<usize>,
}

impl PerformanceTab {
    /// `Tab`: flip the focused pane. Always a change (two values, always
    /// flips).
    pub fn cycle_focus(&mut self) -> bool {
        self.focus = self.focus.toggle();
        true
    }

    /// `←`/`→`: move the selection one frame at a time across a
    /// `window_len`-frame window, clamped to its bounds. With nothing
    /// selected yet, the first press starts scrubbing from the tail (the
    /// newest frame) rather than jumping straight to an edge — the same
    /// "step off live" shape a video player's scrub bar takes. An empty
    /// window has nothing to select.
    pub fn scrub(&mut self, delta: isize, window_len: usize) -> bool {
        if window_len == 0 {
            let changed = self.selected_frame.is_some();
            self.selected_frame = None;
            return changed;
        }
        let current = self.selected_frame.unwrap_or(window_len - 1);
        let next = (current as isize + delta).clamp(0, window_len as isize - 1) as usize;
        let changed = self.selected_frame != Some(next);
        self.selected_frame = Some(next);
        changed
    }

    /// Select a specific window index directly (a chart-column click),
    /// clamped into range. A no-op on an empty window.
    pub fn select(&mut self, index: usize, window_len: usize) -> bool {
        if window_len == 0 {
            return false;
        }
        let clamped = index.min(window_len - 1);
        let changed = self.selected_frame != Some(clamped);
        self.selected_frame = Some(clamped);
        changed
    }

    /// `Esc`'s first stage inside the Performance tab: drop back to the live
    /// tail without leaving DevTools. `crate::runner`'s key router only
    /// reaches for this while a frame is actually selected — a second `Esc`
    /// with nothing selected falls through to `Message::DevtoolsClose`.
    pub fn clear_selection(&mut self) -> bool {
        let changed = self.selected_frame.is_some();
        self.selected_frame = None;
        changed
    }

    /// Whether a frame is currently pinned — the two-stage `Esc` gate.
    pub fn has_selection(&self) -> bool {
        self.selected_frame.is_some()
    }
}

/// One frame-phase breakdown, in the wire's own field order (`FrameStats`'
/// `rebuild_us`/`layout_us`/`paint_us`/`encode_us`/`acquire_us`/
/// `submit_us`) — the exact left-to-right order the breakdown bar draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfPhases {
    pub rebuild_us: u64,
    pub layout_us: u64,
    pub paint_us: u64,
    pub encode_us: u64,
    pub acquire_us: u64,
    pub submit_us: u64,
}

impl PerfPhases {
    /// The sum of every phase — not necessarily equal to the frame's
    /// `total_us` (the wire's `total_us` covers the whole frame, including
    /// any gap between phases), but the right denominator for *this bar's*
    /// own percentages so its segments always sum to (about) 100%.
    pub fn phase_total_us(&self) -> u64 {
        self.rebuild_us
            + self.layout_us
            + self.paint_us
            + self.encode_us
            + self.acquire_us
            + self.submit_us
    }

    /// `(label, microseconds, percent of [`Self::phase_total_us`])` for each
    /// phase, in wire order — what the breakdown bar's segments and legend
    /// draw directly.
    pub fn segments(&self) -> [(&'static str, u64, f64); 6] {
        let total = (self.phase_total_us().max(1)) as f64;
        let pct = |us: u64| us as f64 / total * 100.0;
        [
            ("rebuild", self.rebuild_us, pct(self.rebuild_us)),
            ("layout", self.layout_us, pct(self.layout_us)),
            ("paint", self.paint_us, pct(self.paint_us)),
            ("encode", self.encode_us, pct(self.encode_us)),
            ("acquire", self.acquire_us, pct(self.acquire_us)),
            ("submit", self.submit_us, pct(self.submit_us)),
        ]
    }
}

/// One Performance-chart point, independent of whether it came off the live
/// devtools ring or the log-fallback parser — the shared shape the render
/// layer draws regardless of [`PerfSource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerfFrame {
    /// A display counter: the live ring's real `FrameStats::n`, or (in
    /// log-fallback) a synthesized 1-based position in the retained sample
    /// window — see [`PerfFrame::phases`]'s doc for why the fallback source
    /// can't recover the real one.
    pub n: u64,
    /// Whole-frame duration, microseconds.
    pub total_us: u64,
    /// Whether the frame-gate skipped this frame — drawn as a dim column
    /// rather than a height. Always `false` in log-fallback (see the doc
    /// below): [`PerfPanel`]'s ring doesn't retain it.
    pub skipped: bool,
    /// The per-phase split, when the source has one. `None` in log-fallback
    /// mode — **a §B12 detail not implemented as drawn**: [`PerfPanel`]'s
    /// ring keeps only each raw line's `total_us` (all it needs for its own
    /// sparkline), discarding `rebuild_us`/`layout_us`/…/`skipped` at
    /// ingest, so a fallback frame has no phase split or skip flag to show.
    /// The breakdown pane renders an explicit "not available in log
    /// fallback" note for this case rather than fabricating a split.
    pub phases: Option<PerfPhases>,
}

impl PerfFrame {
    fn from_stats(stats: &FrameStats) -> Self {
        Self {
            n: stats.n,
            total_us: stats.total_us,
            skipped: stats.skipped,
            phases: Some(PerfPhases {
                rebuild_us: stats.rebuild_us,
                layout_us: stats.layout_us,
                paint_us: stats.paint_us,
                encode_us: stats.encode_us,
                acquire_us: stats.acquire_us,
                submit_us: stats.submit_us,
            }),
        }
    }
}

/// Which data source is behind the Performance tab's chart right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerfSource {
    /// The live devtools frame-stats ring ([`DevtoolsState::frames`]) — full
    /// per-phase fidelity.
    Service,
    /// No live connection right now; drawing from `frust-perf raw` lines
    /// already parsed out of the session's log by [`PerfPanel`] — totals
    /// only (see [`PerfFrame::phases`]'s doc).
    LogFallback,
    /// Neither source has produced a sample yet.
    NoData,
}

/// Pure truth table selecting [`PerfSource`] from three cheap-to-read facts:
/// whether the bridge is presently `Connected`, whether the live ring is
/// empty, and whether the log-fallback parser has ingested any raw samples.
///
/// - **`connected` always wins**, even with an as-yet-empty ring (a fresh
///   connection that hasn't streamed its first frame yet) — it is the more
///   truthful badge, and the chart will fill in on the next batch.
/// - Otherwise, a non-empty [`PerfPanel`] wins the fallback slot — the badge
///   flips to `log fallback` per §B12's Performance-tab note.
/// - Otherwise, a *previously* connected ring's leftover samples still draw
///   (labeled `Service`, since the data itself came off the wire rather than
///   a log line) instead of being hidden the moment the connection drops.
/// - Only with nothing at all does this return `NoData`.
pub fn select_perf_source(connected: bool, ring_empty: bool, perf_panel_empty: bool) -> PerfSource {
    if connected {
        PerfSource::Service
    } else if !perf_panel_empty {
        PerfSource::LogFallback
    } else if !ring_empty {
        PerfSource::Service
    } else {
        PerfSource::NoData
    }
}

/// Build the Performance tab's drawable window: which source is live right
/// now, and up to [`PERF_WINDOW`] frames of it (oldest first), mapped into
/// the shared [`PerfFrame`] shape — "the protocol doc requires it, so the
/// chart never needs two renderers" (§B12).
pub fn perf_window(
    conn: &ConnState,
    frames: &VecDeque<FrameStats>,
    perf_panel: &PerfPanel,
) -> (PerfSource, Vec<PerfFrame>) {
    let connected = matches!(conn, ConnState::Connected { .. });
    let source = select_perf_source(
        connected,
        frames.is_empty(),
        perf_panel.samples().len() == 0,
    );
    let window = match source {
        PerfSource::Service => {
            let start = frames.len().saturating_sub(PERF_WINDOW);
            frames
                .iter()
                .skip(start)
                .map(PerfFrame::from_stats)
                .collect()
        }
        PerfSource::LogFallback => {
            let samples: Vec<u64> = perf_panel.samples().collect();
            let start = samples.len().saturating_sub(PERF_WINDOW);
            samples[start..]
                .iter()
                .enumerate()
                .map(|(i, &total_us)| PerfFrame {
                    n: (start + i + 1) as u64,
                    total_us,
                    skipped: false,
                    phases: None,
                })
                .collect()
        }
        PerfSource::NoData => Vec::new(),
    };
    (source, window)
}

/// FPS/percentile/jank summary over one [`perf_window`] result — the
/// Performance tab's chip row. `None` when the window has no non-skipped
/// frame to measure (an empty window, or a window of skips only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerfStats {
    /// Frames per second, derived from the window's mean frame time —
    /// `1_000_000 / mean(total_us)`, equivalent to `frame_count /
    /// total_elapsed_seconds` over the same window (no wall-clock
    /// timestamps are on the wire, only per-frame durations).
    pub fps: f64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    /// Frames whose `total_us` exceeds [`JANK_MEDIAN_MULTIPLIER`]× the
    /// window's own median (`p50_us`) — skipped frames are never counted
    /// (they're already flagged distinctly as a dim column, a different
    /// failure mode than a slow-but-rendered frame).
    pub jank_count: usize,
    /// `jank_count` as a percentage of the window's non-skipped frame count.
    pub jank_pct: f64,
    /// The absolute threshold `jank_count` was measured against
    /// (`p50_us` × [`JANK_MEDIAN_MULTIPLIER`]) — exposed so the chart can
    /// color a bar consistently with the chip's own count rather than a
    /// second, independently-computed threshold.
    pub jank_threshold_us: u64,
}

/// Jank threshold, as a multiple of the window's median frame time —
/// §B12 doesn't pin an exact definition (only the chip's rendered example),
/// so this is an author-chosen threshold, not a spec.
const JANK_MEDIAN_MULTIPLIER: f64 = 1.5;

/// Nearest-rank percentile over an ascending-sorted slice (`p` in `0.0..=100.0`).
fn percentile_us(sorted_asc: &[u64], p: f64) -> u64 {
    if sorted_asc.is_empty() {
        return 0;
    }
    let idx = ((p / 100.0) * (sorted_asc.len() as f64 - 1.0)).round() as usize;
    sorted_asc[idx.min(sorted_asc.len() - 1)]
}

/// Compute [`PerfStats`] over `window` (as returned by [`perf_window`]).
pub fn perf_stats(window: &[PerfFrame]) -> Option<PerfStats> {
    let mut totals: Vec<u64> = window
        .iter()
        .filter(|f| !f.skipped)
        .map(|f| f.total_us)
        .collect();
    if totals.is_empty() {
        return None;
    }
    totals.sort_unstable();
    let p50_us = percentile_us(&totals, 50.0);
    let p95_us = percentile_us(&totals, 95.0);
    let p99_us = percentile_us(&totals, 99.0);
    let jank_threshold = (p50_us as f64 * JANK_MEDIAN_MULTIPLIER) as u64;
    let jank_count = totals.iter().filter(|&&us| us > jank_threshold).count();
    let jank_pct = jank_count as f64 / totals.len() as f64 * 100.0;
    let mean_us: f64 = totals.iter().sum::<u64>() as f64 / totals.len() as f64;
    let fps = if mean_us > 0.0 {
        1_000_000.0 / mean_us
    } else {
        0.0
    };
    Some(PerfStats {
        fps,
        p50_us,
        p95_us,
        p99_us,
        jank_count,
        jank_pct,
        jank_threshold_us: jank_threshold,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(n: u64) -> FrameStats {
        FrameStats {
            n,
            total_us: 16_000,
            rebuild_us: 8_000,
            layout_us: 3_000,
            paint_us: 2_000,
            encode_us: 1_400,
            acquire_us: 600,
            submit_us: 1_000,
            skipped: false,
        }
    }

    fn debug_state() -> DevtoolsState {
        DevtoolsState::new(DevtoolsLaunch::from_launch(BuildMode::Debug, None))
    }

    #[test]
    fn build_mode_drives_capability_off_the_feature_funnel() {
        for mode in [BuildMode::Debug, BuildMode::Profile] {
            assert!(DevtoolsLaunch::from_launch(mode, None).capable, "{mode:?}");
        }
        assert!(!DevtoolsLaunch::from_launch(BuildMode::Release, None).capable);
        assert!(!DevtoolsLaunch::unavailable().capable);
    }

    #[test]
    fn a_release_session_is_unavailable_regardless_of_what_it_logs() {
        let mut state = DevtoolsState::new(DevtoolsLaunch::from_launch(BuildMode::Release, None));
        // Even a discovery line (impossible in practice) can't make a release
        // build reachable — the phase is decided by the build, not a timeout.
        state.ingest_line("frust-devtools listening on 53214 token abc");
        assert_eq!(state.phase(), DevtoolsPhase::Unavailable);
        assert!(!state.wants_connect());
    }

    #[test]
    fn discovery_line_stores_port_and_token_and_latest_wins() {
        let mut state = debug_state();
        assert_eq!(state.phase(), DevtoolsPhase::Discovering);
        assert!(!state.ingest_line("app: booting up"));

        assert!(state.ingest_line(
            "08-09 12:00:01.234 1234 1234 I frust: frust-devtools listening on 53214 token cafe"
        ));
        let first = state.discovered.clone().unwrap();
        assert_eq!(first.port, 53214);
        assert_eq!(first.token.as_deref(), Some("cafe"));
        assert!(state.wants_connect());

        // The same line again is not a change (fires at most once per
        // announcement).
        assert!(!state.ingest_line("frust-devtools listening on 53214 token cafe"));
        // A restart re-announces a fresh port/token — latest wins.
        assert!(state.ingest_line("frust-devtools listening on 60000 token beef"));
        let second = state.discovered.clone().unwrap();
        assert_eq!(second.port, 60000);
        assert_eq!(second.token.as_deref(), Some("beef"));
    }

    #[test]
    fn conn_events_drive_the_phase() {
        let mut state = debug_state();
        state.ingest_line("frust-devtools listening on 1 token t");
        state.begin_connect();
        assert_eq!(state.phase(), DevtoolsPhase::Connecting);
        assert!(!state.wants_connect(), "a connect in flight is not retried");

        assert!(state.apply(ConnEvent::Connected {
            app_name: "huddle".to_string(),
            caps: vec![Capability::FrameStats],
        }));
        assert_eq!(state.phase(), DevtoolsPhase::Connected);
        // An identical report is not a visible change.
        assert!(!state.apply(ConnEvent::Connected {
            app_name: "huddle".to_string(),
            caps: vec![Capability::FrameStats],
        }));

        assert!(state.apply(ConnEvent::Failed("connection refused".to_string())));
        assert_eq!(state.phase(), DevtoolsPhase::Failed);
        assert!(state.wants_connect(), "a failed connection is retryable");

        assert!(state.apply(ConnEvent::Closed));
        assert_eq!(state.phase(), DevtoolsPhase::Discovering);
    }

    #[test]
    fn session_end_closes_a_live_connection_but_leaves_an_idle_one_alone() {
        let mut state = debug_state();
        state.begin_connect();
        assert!(state.on_session_end());
        assert_eq!(state.phase(), DevtoolsPhase::Failed);

        let mut idle = debug_state();
        assert!(!idle.on_session_end());
        assert_eq!(idle.phase(), DevtoolsPhase::Discovering);
    }

    #[test]
    fn frame_ring_keeps_the_newest_samples_and_caps() {
        let mut state = debug_state();
        for n in 0..(FRAME_RING_CAP as u64 + 25) {
            state.push_frame(frame(n));
        }
        assert_eq!(state.frames.len(), FRAME_RING_CAP);
        assert_eq!(state.frames.front().unwrap().n, 25);
        assert_eq!(state.frames.back().unwrap().n, FRAME_RING_CAP as u64 + 24);
    }

    #[test]
    fn a_frames_batch_lands_in_order_and_an_empty_batch_is_not_a_redraw() {
        let mut state = debug_state();
        assert!(state.apply(ConnEvent::Frames(vec![frame(1), frame(2)])));
        assert_eq!(
            state.frames.iter().map(|f| f.n).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(!state.apply(ConnEvent::Frames(Vec::new())));
    }

    #[test]
    fn tabs_select_by_index_and_cycle_wrapping() {
        let mut state = debug_state();
        assert_eq!(state.active_tab, DevtoolsTab::Performance);
        assert!(state.select_tab(2));
        assert_eq!(state.active_tab, DevtoolsTab::Inspector);
        assert!(!state.select_tab(2), "re-selecting is not a change");
        assert!(!state.select_tab(9), "out of range is a no-op");
        assert_eq!(state.active_tab, DevtoolsTab::Inspector);

        assert!(state.cycle_tab(1));
        assert_eq!(state.active_tab, DevtoolsTab::Network);
        assert!(state.cycle_tab(1));
        assert_eq!(state.active_tab, DevtoolsTab::Performance, "wraps forward");
        assert!(state.cycle_tab(-1));
        assert_eq!(state.active_tab, DevtoolsTab::Network, "wraps backward");
    }

    // ── Performance tab ──────────────────────────────────────────────────

    #[test]
    fn focus_cycle_flips_between_the_two_panes() {
        let mut perf = PerformanceTab::default();
        assert_eq!(perf.focus, PerfFocus::Chart);
        assert!(perf.cycle_focus());
        assert_eq!(perf.focus, PerfFocus::Breakdown);
        assert!(perf.cycle_focus());
        assert_eq!(perf.focus, PerfFocus::Chart);
    }

    #[test]
    fn scrub_starts_from_the_tail_and_clamps_at_both_ends() {
        let mut perf = PerformanceTab::default();
        assert_eq!(perf.selected_frame, None);

        // First press starts at the tail (index `window_len - 1`), not at 0.
        assert!(perf.scrub(0, 10));
        assert_eq!(perf.selected_frame, Some(9));

        assert!(perf.scrub(-1, 10));
        assert_eq!(perf.selected_frame, Some(8));
        assert!(perf.scrub(1, 10));
        assert_eq!(perf.selected_frame, Some(9));

        // Clamped at the newest end — repeated Right never overflows.
        assert!(!perf.scrub(1, 10), "already at the newest frame");
        assert_eq!(perf.selected_frame, Some(9));

        // Clamped at the oldest end.
        for _ in 0..20 {
            perf.scrub(-1, 10);
        }
        assert_eq!(perf.selected_frame, Some(0));
        assert!(!perf.scrub(-1, 10), "already at the oldest frame");
    }

    #[test]
    fn scrub_on_an_empty_window_clears_any_selection() {
        let mut perf = PerformanceTab {
            selected_frame: Some(3),
            ..Default::default()
        };
        assert!(perf.scrub(1, 0));
        assert_eq!(perf.selected_frame, None);
    }

    #[test]
    fn select_clamps_into_range_and_reports_change() {
        let mut perf = PerformanceTab::default();
        assert!(perf.select(4, 10));
        assert_eq!(perf.selected_frame, Some(4));
        assert!(!perf.select(4, 10), "re-selecting the same frame");
        assert!(perf.select(50, 10), "out-of-range clamps to the last index");
        assert_eq!(perf.selected_frame, Some(9));
        assert!(!perf.select(0, 0), "an empty window has nothing to select");
    }

    #[test]
    fn clear_selection_is_the_esc_first_stage() {
        let mut perf = PerformanceTab {
            selected_frame: Some(2),
            ..Default::default()
        };
        assert!(perf.has_selection());
        assert!(perf.clear_selection());
        assert!(!perf.has_selection());
        assert!(
            !perf.clear_selection(),
            "clearing an already-clear selection is not a change"
        );
    }

    fn phased_frame(n: u64, total_us: u64, skipped: bool) -> PerfFrame {
        PerfFrame {
            n,
            total_us,
            skipped,
            phases: Some(PerfPhases {
                rebuild_us: total_us / 2,
                layout_us: total_us / 4,
                paint_us: total_us / 4,
                encode_us: 0,
                acquire_us: 0,
                submit_us: 0,
            }),
        }
    }

    #[test]
    fn perf_phases_segments_sum_to_the_phase_total_in_wire_order() {
        let phases = PerfPhases {
            rebuild_us: 8_200,
            layout_us: 3_000,
            paint_us: 2_100,
            encode_us: 1_400,
            acquire_us: 600,
            submit_us: 1_000,
        };
        let segments = phases.segments();
        assert_eq!(
            segments.map(|(label, _, _)| label),
            ["rebuild", "layout", "paint", "encode", "acquire", "submit"]
        );
        let pct_sum: f64 = segments.iter().map(|(_, _, pct)| pct).sum();
        assert!(
            (pct_sum - 100.0).abs() < 0.01,
            "segment percentages sum to ~100%: {pct_sum}"
        );
    }

    #[test]
    fn perf_stats_on_a_known_fixture_computes_percentiles_fps_and_jank() {
        // 8 non-skipped frames at 16ms plus one 40ms spike (> 1.5x median),
        // and one skipped frame that must not pollute any of the math.
        let mut window: Vec<PerfFrame> = (0..8).map(|n| phased_frame(n, 16_000, false)).collect();
        window.push(phased_frame(8, 40_000, false));
        window.push(phased_frame(9, 999_000, true)); // skipped: excluded entirely

        let stats = perf_stats(&window).expect("non-empty window");
        assert_eq!(stats.p50_us, 16_000);
        assert_eq!(stats.p99_us, 40_000);
        assert_eq!(
            stats.jank_count, 1,
            "only the 40ms frame exceeds 1.5x the 16ms median"
        );
        assert!((stats.jank_pct - (100.0 / 9.0)).abs() < 0.01);
        // fps = 1e6 / mean(total_us); mean = (8*16_000 + 40_000) / 9.
        let expected_mean = (8.0 * 16_000.0 + 40_000.0) / 9.0;
        assert!((stats.fps - 1_000_000.0 / expected_mean).abs() < 0.01);
    }

    #[test]
    fn perf_stats_on_all_skipped_frames_is_none() {
        let window = vec![phased_frame(0, 16_000, true), phased_frame(1, 16_000, true)];
        assert_eq!(perf_stats(&window), None);
    }

    #[test]
    fn perf_stats_on_an_empty_window_is_none() {
        assert_eq!(perf_stats(&[]), None);
    }

    #[test]
    fn select_perf_source_truth_table() {
        use PerfSource::*;
        // (connected, ring_empty, perf_panel_empty) -> expected
        let cases = [
            (true, true, true, Service),
            (true, true, false, Service),
            (true, false, true, Service),
            (true, false, false, Service),
            (false, true, false, LogFallback),
            (false, false, false, LogFallback),
            (false, false, true, Service),
            (false, true, true, NoData),
        ];
        for (connected, ring_empty, perf_empty, expected) in cases {
            assert_eq!(
                select_perf_source(connected, ring_empty, perf_empty),
                expected,
                "connected={connected} ring_empty={ring_empty} perf_empty={perf_empty}"
            );
        }
    }

    #[test]
    fn perf_window_service_source_is_capped_at_the_120_frame_window() {
        let mut frames = VecDeque::new();
        for n in 0..(PERF_WINDOW as u64 + 30) {
            frames.push_back(frame(n));
        }
        let conn = ConnState::Connected {
            app_name: "huddle".to_string(),
            caps: Vec::new(),
        };
        let (source, window) = perf_window(&conn, &frames, &PerfPanel::default());
        assert_eq!(source, PerfSource::Service);
        assert_eq!(window.len(), PERF_WINDOW);
        assert_eq!(window.first().unwrap().n, 30);
        assert_eq!(window.last().unwrap().n, PERF_WINDOW as u64 + 29);
        assert!(window[0].phases.is_some());
    }

    #[test]
    fn perf_window_falls_back_to_the_log_panel_with_no_connection() {
        let mut panel = PerfPanel::default();
        for i in 0..5u64 {
            panel.ingest(&format!(
                "frust-perf raw n={i} total_us={} rebuild_us=0 layout_us=0 paint_us=0 \
                 encode_us=0 present_us=0 skipped=0",
                1_000 + i
            ));
        }
        let (source, window) = perf_window(
            &ConnState::Failed {
                error: "refused".to_string(),
            },
            &VecDeque::new(),
            &panel,
        );
        assert_eq!(source, PerfSource::LogFallback);
        assert_eq!(window.len(), 5);
        assert_eq!(window[0].total_us, 1_000);
        assert_eq!(window[4].total_us, 1_004);
        assert!(
            window.iter().all(|f| f.phases.is_none() && !f.skipped),
            "log-fallback frames carry totals only"
        );
    }

    #[test]
    fn perf_window_with_neither_source_is_empty() {
        let (source, window) =
            perf_window(&ConnState::Idle, &VecDeque::new(), &PerfPanel::default());
        assert_eq!(source, PerfSource::NoData);
        assert!(window.is_empty());
    }
}
