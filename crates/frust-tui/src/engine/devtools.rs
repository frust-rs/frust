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
//! needs plus [`DevtoolsState::frames`] (Performance's ring),
//! [`DevtoolsState::performance`] (Performance's own scrub/focus state — see
//! [`PerformanceTab`]), [`DevtoolsState::inspector`] (the widget-tree
//! snapshot + selection — see [`InspectorTab`]) and
//! [`DevtoolsState::metrics`] (the System/Network tabs' CPU/RSS/thermal/net
//! rings, fed by `crate::supervise::MetricsBridge` — see [`MetricsState`]).
//! Adding a slot is additive: nothing outside this module reads a tab's own
//! state except through the accessors below and [`DevtoolsState::phase`],
//! whose five values are the §B12 screen set and are matched exhaustively by
//! the render and key-routing layers.
//!
//! The Inspector's payload arrives on its own report channel
//! ([`InspectorEvent`], carried by `super::Message::DevtoolsInspector`)
//! rather than as a [`ConnEvent`] variant: those are the *connection's* own
//! state plus its one subscription, while a widget tree/props pull is an
//! on-demand request/response the tab drives.
//!
//! Performance can also draw from a *second* source when there is no live
//! connection: [`perf_window`] folds [`super::PerfPanel`]'s
//! already-ingested `frust-perf raw` log lines into the same [`PerfFrame`]
//! shape the live ring produces, selected by the pure [`select_perf_source`]
//! truth table — see that function's doc for exactly what a log-fallback
//! frame can and can't show.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use frust_devtools_protocol::{
    Capability, Discovery, FrameStats, RectPx, WidgetNode, WidgetProps, WidgetTreeDump,
};
use frust_drive::build_info::BuildMode;
use frust_drive::metrics::{MetricsSample, NetSample};

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
/// per-tab payload slots.
///
/// Not `Eq` (unlike the connection types above): the Inspector's payload
/// carries the wire's own `RectPx` bounds, which are `f64` — an equality
/// marker no float can honor.
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// The reason from the most recent "service did not start" line, if the
    /// app logged one (a bind refused by the OS — e.g. a per-app network
    /// permission being off — a runtime that would not build, …). Turns the
    /// otherwise-eternal "waiting for a discovery line…" screen into the
    /// concrete cause. Cleared by a later successful discovery line, since a
    /// relaunch that binds supersedes an earlier failure.
    pub start_error: Option<String>,
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
    /// The Inspector tab's own state (the widget-tree snapshot, its
    /// expansion/selection, and the selected node's props) — see
    /// [`InspectorTab`].
    pub inspector: InspectorTab,
    /// The System/Network tabs' shared metrics-sampler state — see
    /// [`MetricsState`].
    pub metrics: MetricsState,
}

impl DevtoolsState {
    /// A fresh state for a session launched per `launch`. `metrics` seeds its
    /// Android identity from the same `android_serial` the devtools bridge
    /// uses (`launch.android_serial`) — see [`MetricsState::new`].
    pub fn new(launch: DevtoolsLaunch) -> Self {
        let metrics = MetricsState::new(launch.android_serial.clone());
        Self {
            launch,
            metrics,
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
            // Not a discovery line — but it may be the app reporting that its
            // service could not start. Record the reason so the Discovering
            // screen can explain the silence instead of waiting forever. This
            // never triggers a connect (there is no port), so it returns false.
            if let Some(reason) = frust_devtools_protocol::parse_failure_line(plain) {
                self.start_error = Some(reason.to_string());
            }
            return false;
        };
        // A live bind supersedes any earlier failure this session logged.
        self.start_error = None;
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
/// which frame (if any) is pinned for inspection.
///
/// # Why the pin is a frame identity, not a window index
///
/// The drawn window ([`perf_window`]) is a *sliding* view of the newest
/// [`PERF_WINDOW`] samples: every arriving frame shifts every position in it
/// by one. A pin stored as a position would therefore name a different frame
/// after each batch — the header and breakdown would walk off the spike the
/// user pinned with no key pressed, which is exactly what holding a jank
/// frame still is for. So the pin is the frame's own `FrameStats::n`
/// ([`PerfFrame::n`]), resolved back to a position at read time
/// ([`Self::resolve`]); it stays glued to its frame as the window slides, and
/// falls back to the live tail only when that frame ages out of the window
/// entirely ([`Self::retain_in_window`]).
///
/// **Log-fallback caveat**: in [`PerfSource::LogFallback`] the `n` a frame
/// carries is *synthesized* from its position in [`PerfPanel`]'s own ring
/// (the raw-line parser keeps only totals — see [`PerfFrame::n`]'s doc), so
/// once that ring is full the identity a fallback frame reports is positional
/// after all. Pinning is only as stable as the source's own numbering; the
/// live service source (the one §B12's scrub is about) carries the real
/// frame counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerformanceTab {
    /// Which pane `Tab` moves between.
    pub focus: PerfFocus,
    /// The pinned frame's own [`PerfFrame::n`], or `None` to track the
    /// newest (the default "follow the live tail").
    pub selected_n: Option<u64>,
}

impl PerformanceTab {
    /// `Tab`: flip the focused pane. Always a change (two values, always
    /// flips).
    pub fn cycle_focus(&mut self) -> bool {
        self.focus = self.focus.toggle();
        true
    }

    /// The pinned frame's 0-based position in `window`, or `None` when
    /// nothing is pinned *or* the pinned frame is no longer in the window.
    /// The render layer treats `None` as "live tail" — the same thing an
    /// un-pinned tab shows.
    pub fn resolve(&self, window: &[PerfFrame]) -> Option<usize> {
        let n = self.selected_n?;
        window.iter().position(|frame| frame.n == n)
    }

    /// `←`/`→`: move the pin one frame at a time along `window` (oldest
    /// first), clamped to its bounds — the *adjacent retained frame*, by
    /// identity, not a position that would later drift. With nothing pinned
    /// yet, the first press starts scrubbing from the tail (the newest frame)
    /// rather than jumping straight to an edge — the same "step off live"
    /// shape a video player's scrub bar takes. An empty window has nothing to
    /// pin.
    pub fn scrub(&mut self, delta: isize, window: &[PerfFrame]) -> bool {
        if window.is_empty() {
            return self.clear_selection();
        }
        let current = self.resolve(window).unwrap_or(window.len() - 1);
        let next = (current as isize + delta).clamp(0, window.len() as isize - 1) as usize;
        let n = window[next].n;
        let changed = self.selected_n != Some(n);
        self.selected_n = Some(n);
        changed
    }

    /// Pin the frame a chart column named (a column click), by the `n` that
    /// column carried *at render time*. A frame that has since left the
    /// window is not pinned at all: dropping the stale click is what makes
    /// the click race-free, where a positional payload would silently pin
    /// whichever frame had slid into that column since.
    pub fn select_frame(&mut self, n: u64, window: &[PerfFrame]) -> bool {
        if !window.iter().any(|frame| frame.n == n) {
            return false;
        }
        let changed = self.selected_n != Some(n);
        self.selected_n = Some(n);
        changed
    }

    /// The window slid: if the pinned frame has fallen out of it, drop back
    /// to the live tail. Returns whether the pin was actually cleared.
    ///
    /// Clearing (rather than clamping onto the oldest retained column) is the
    /// deliberate choice: the pinned frame's data is genuinely gone from the
    /// window, and clamping would re-introduce the very walking-identity
    /// defect this type exists to avoid — the "oldest column" names a new
    /// frame on every batch. Falling back to live is one discrete transition
    /// the axis row already labels (`live (frame #N)`).
    pub fn retain_in_window(&mut self, window: &[PerfFrame]) -> bool {
        if self.selected_n.is_none() || self.resolve(window).is_some() {
            return false;
        }
        self.selected_n = None;
        true
    }

    /// `Esc`'s first stage inside the Performance tab: drop back to the live
    /// tail without leaving DevTools. `crate::runner`'s key router only
    /// reaches for this while a frame is actually selected — a second `Esc`
    /// with nothing selected falls through to `Message::DevtoolsClose`.
    pub fn clear_selection(&mut self) -> bool {
        let changed = self.selected_n.is_some();
        self.selected_n = None;
        changed
    }

    /// Whether a frame is currently pinned — the two-stage `Esc` gate.
    pub fn has_selection(&self) -> bool {
        self.selected_n.is_some()
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

// ── System/Network tabs (workbook §B12) ─────────────────────────────────────

/// How many recent samples the System/Network rings retain — comfortably
/// more than a typical pane's sparkline width, bounded the same drop-oldest
/// way [`DevtoolsState::frames`] is.
pub const METRICS_RING_CAP: usize = 120;

/// One CPU-percent point (the System tab's sparkline).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuPoint {
    pub percent: f32,
    pub at_ms: u64,
}

/// One RSS (desktop `VmRSS`) / PSS (Android `TOTAL PSS`) point, bytes — see
/// `frust_drive::metrics`'s module doc for the PSS-not-RSS-on-Android note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RssPoint {
    pub rss_bytes: u64,
    pub at_ms: u64,
}

/// One thermal zone's latest reading, unconverted (see
/// `frust_drive::metrics::ThermalSample`'s doc: Android vendors aren't
/// consistent about whether this is millidegrees or plain degrees).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThermalPoint {
    pub millideg_c: i64,
    pub at_ms: u64,
}

/// One derived network-rate point (bytes/second per direction), computed by
/// diffing two consecutive cumulative [`NetSample`]s — see
/// [`MetricsState::apply`]'s doc for the first-sample and counter-reset
/// handling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NetRatePoint {
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub at_ms: u64,
}

/// Cumulative rx/tx bytes since sampling started, or since the last counter
/// reset (see [`MetricsState::apply`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NetTotals {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// One session's resolved OS-process identity for the System/Network
/// metrics sampler — orthogonal to [`DevtoolsLaunch`], which is about
/// reaching the in-process devtools *service*. Metrics sampling reads
/// `/proc`/`/sys` (desktop) or `adb` (Android) directly
/// (`frust_drive::metrics`), so it needs no devtools build feature and works
/// against a release build too.
///
/// **Desktop honesty note**: `crate::supervise::session`'s process plumbing
/// (`frust_drive::process::StreamHandle`) never exposes a spawned child's
/// pid, and extending it is out of scope here (`frust-drive` stays
/// untouched) — so a non-Android session can never resolve past
/// [`MetricsIdentity::NotAndroid`]. This also folds in iOS: no pid plumbing
/// exists for it either, so this type does not distinguish "desktop" from
/// "iOS" — both render the same `desktop` source label and the same
/// unavailable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetricsIdentity {
    /// Not an Android session — sampling can never start (see the doc
    /// above).
    NotAndroid,
    /// An Android session. `pkg`/`pid` fill in as the session's own log
    /// reports them (`Launching {pkg}…` / `Streaming logs (pid {pid})` — the
    /// same lines `crate::supervise::session::infer_state` reads for its own
    /// purpose), parsed by [`MetricsState::ingest_line`].
    Android {
        serial: String,
        pkg: Option<String>,
        pid: Option<String>,
    },
}

impl MetricsIdentity {
    fn new(android_serial: Option<String>) -> Self {
        match android_serial {
            Some(serial) => MetricsIdentity::Android {
                serial,
                pkg: None,
                pid: None,
            },
            None => MetricsIdentity::NotAndroid,
        }
    }

    /// `(serial, pid, pkg)` once every piece has arrived; `None` otherwise
    /// (including [`Self::NotAndroid`], which never resolves).
    fn ready(&self) -> Option<(&str, &str, &str)> {
        match self {
            MetricsIdentity::Android {
                serial,
                pkg: Some(pkg),
                pid: Some(pid),
            } => Some((serial, pid, pkg)),
            _ => None,
        }
    }
}

/// Parses the drive's `Launching {package}…` phase line (`android_run::run`
/// / `spawn_session_with_env`'s `on_line` calls) into the package name.
fn parse_android_package_line(line: &str) -> Option<String> {
    let pkg = line.strip_prefix("Launching ")?.strip_suffix('…')?;
    (!pkg.is_empty()).then(|| pkg.to_string())
}

/// Parses the drive's `Streaming logs (pid {pid})` marker — the same line
/// `crate::supervise::session::infer_state` reads to advance a session to
/// `Running` — into the pid.
fn parse_android_pid_line(line: &str) -> Option<String> {
    let pid = line
        .strip_prefix("Streaming logs (pid ")?
        .strip_suffix(')')?;
    (!pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())).then(|| pid.to_string())
}

/// Whether a session's metrics sampler is running, has never been started,
/// or can never be — the System/Network tabs' shared status badge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamplingState {
    /// Nothing has been started yet (an Android identity that hasn't
    /// resolved, or DevTools hasn't been opened for this session yet).
    Off,
    /// A [`crate::supervise::MetricsBridge`] thread is sampling this
    /// session.
    On,
    /// Sampling can never run (a non-Android session — see
    /// [`MetricsIdentity::NotAndroid`]'s doc) or no longer can (the session
    /// ended). `reason` is shown verbatim.
    Unavailable { reason: String },
}

/// One session's System/Network tab state (workbook §B12): its resolved
/// platform identity, whether a sampler is running, and the rings/totals fed
/// by `crate::supervise::MetricsBridge`'s coalesced
/// `super::Message::DevtoolsMetrics` batches.
///
/// Not `Eq`: the rate/CPU points carry `f32`/`f64`.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricsState {
    pub identity: MetricsIdentity,
    pub sampling: SamplingState,
    /// CPU-percent ring, oldest first, capped at [`METRICS_RING_CAP`].
    pub cpu: VecDeque<CpuPoint>,
    /// RSS/PSS ring, oldest first, capped at [`METRICS_RING_CAP`].
    pub rss: VecDeque<RssPoint>,
    /// The *latest* reading per thermal zone (not a ring — §B12 shows one
    /// current row per zone, not a history).
    pub thermal: BTreeMap<String, ThermalPoint>,
    /// Derived rx/tx rate ring, oldest first, capped at [`METRICS_RING_CAP`].
    pub net_rates: VecDeque<NetRatePoint>,
    /// Cumulative rx/tx since sampling started (or since the last counter
    /// reset — see [`Self::apply`]).
    pub net_totals: NetTotals,
    /// The most recent cumulative net sample, kept only to diff the next one
    /// against — never rendered directly.
    last_net: Option<NetSample>,
    /// The sample [`Self::net_totals`] is measured from — reset to the
    /// current sample whenever the counters (or the clock) go backward, so a
    /// device reboot or an `adb forward` restart never reads as a negative
    /// rate or an underflowed total.
    net_baseline: Option<NetSample>,
    /// The newest `at_ms` seen across every sample kind — the System tab's
    /// uptime readout.
    pub latest_at_ms: Option<u64>,
}

impl Default for MetricsState {
    fn default() -> Self {
        Self::new(None)
    }
}

impl MetricsState {
    /// A fresh state for a session whose devtools launch metadata reported
    /// `android_serial` (`None` for desktop/iOS — see [`MetricsIdentity`]'s
    /// doc). A non-Android session starts already [`SamplingState::Unavailable`]:
    /// there is nothing to wait for that would ever make it samplable.
    pub fn new(android_serial: Option<String>) -> Self {
        let identity = MetricsIdentity::new(android_serial);
        let sampling = match &identity {
            MetricsIdentity::NotAndroid => SamplingState::Unavailable {
                reason: "sampling unavailable — pid not exposed".to_string(),
            },
            MetricsIdentity::Android { .. } => SamplingState::Off,
        };
        Self {
            identity,
            sampling,
            cpu: VecDeque::new(),
            rss: VecDeque::new(),
            thermal: BTreeMap::new(),
            net_rates: VecDeque::new(),
            net_totals: NetTotals::default(),
            last_net: None,
            net_baseline: None,
            latest_at_ms: None,
        }
    }

    /// Whether any network sample has landed yet — distinguishes "no data"
    /// from "totals are genuinely zero" for the Network tab's totals row.
    pub fn has_net_data(&self) -> bool {
        self.last_net.is_some()
    }

    /// Feed one session log line. Returns whether the identity just became
    /// launch-ready (both `pkg` and `pid` now known) — the caller starts
    /// sampling (if DevTools is open) on that transition, the same
    /// discovery-line-triggers-a-connect shape [`DevtoolsState::ingest_line`]
    /// uses for the frame-stats connection.
    pub fn ingest_line(&mut self, line: &str) -> bool {
        let MetricsIdentity::Android { pkg, pid, .. } = &mut self.identity else {
            return false;
        };
        let was_ready = pkg.is_some() && pid.is_some();
        if pkg.is_none() {
            *pkg = parse_android_package_line(line);
        }
        if pid.is_none() {
            *pid = parse_android_pid_line(line);
        }
        pkg.is_some() && pid.is_some() && !was_ready
    }

    /// Whether a sampler is worth starting right now: the identity has
    /// resolved and nothing is already running.
    pub fn wants_start(&self) -> bool {
        matches!(self.sampling, SamplingState::Off) && self.identity.ready().is_some()
    }

    /// `(serial, pid, pkg)`, owned, for `super::Effect::MetricsStart` — `None`
    /// unless [`Self::wants_start`] (or an equivalent ready check) already
    /// passed.
    pub fn target(&self) -> Option<(String, String, String)> {
        self.identity
            .ready()
            .map(|(serial, pid, pkg)| (serial.to_string(), pid.to_string(), pkg.to_string()))
    }

    /// Mark a sampler as started — the caller pairs this with the matching
    /// effect, the same optimistic-mark-before-enact shape
    /// [`DevtoolsState::begin_connect`] uses.
    pub fn begin_sampling(&mut self) {
        self.sampling = SamplingState::On;
    }

    /// The session ended: stop a running sampler. Returns whether a
    /// `super::Effect::MetricsStop` is warranted (a sampler was actually
    /// running) — an idle/already-unavailable session has nothing to stop.
    pub fn on_session_end(&mut self) -> bool {
        if matches!(self.sampling, SamplingState::On) {
            self.sampling = SamplingState::Unavailable {
                reason: "the session ended — sampling stopped with it".to_string(),
            };
            true
        } else {
            false
        }
    }

    /// Ingest one sample into the matching ring/total. Pure — no I/O, no
    /// clock read (`at_ms` is the sampler's own epoch, not wall time — see
    /// `frust_drive::metrics::TimestampMs`). Always reports a visible
    /// change: a sample is always worth a redraw while its tab is showing.
    pub fn apply(&mut self, sample: MetricsSample) -> bool {
        match sample {
            MetricsSample::Cpu(s) => {
                self.bump_latest(s.at_ms);
                push_capped(
                    &mut self.cpu,
                    CpuPoint {
                        percent: s.percent,
                        at_ms: s.at_ms,
                    },
                );
            }
            MetricsSample::Mem(s) => {
                self.bump_latest(s.at_ms);
                push_capped(
                    &mut self.rss,
                    RssPoint {
                        rss_bytes: s.rss_bytes,
                        at_ms: s.at_ms,
                    },
                );
            }
            MetricsSample::Thermal(s) => {
                self.bump_latest(s.at_ms);
                self.thermal.insert(
                    s.zone_label,
                    ThermalPoint {
                        millideg_c: s.millideg_c,
                        at_ms: s.at_ms,
                    },
                );
            }
            MetricsSample::Net(s) => {
                self.bump_latest(s.at_ms);
                self.apply_net(s);
            }
        }
        true
    }

    fn bump_latest(&mut self, at_ms: u64) {
        self.latest_at_ms = Some(self.latest_at_ms.map_or(at_ms, |prev| prev.max(at_ms)));
    }

    /// Diff `sample` against the previous cumulative reading into a
    /// bytes/second rate, and roll [`Self::net_totals`] forward from
    /// [`Self::net_baseline`].
    ///
    /// **First sample**: nothing to diff against yet — no rate point, and
    /// the sample becomes the baseline (totals start at zero).
    ///
    /// **Counter reset**: a device reboot or an `adb forward` restart can
    /// make the cumulative counters go *backward* (or leave `at_ms`
    /// non-increasing) — treated as a reset, never a negative rate: no rate
    /// point is emitted for this tick, and the baseline rebases to the
    /// current sample, so totals resume counting from zero rather than
    /// underflowing.
    fn apply_net(&mut self, sample: NetSample) {
        let advances = matches!(self.last_net, Some(prev)
            if sample.rx_bytes >= prev.rx_bytes
                && sample.tx_bytes >= prev.tx_bytes
                && sample.at_ms > prev.at_ms);
        if advances {
            let prev = self.last_net.expect("checked by `advances`");
            let dt_s = (sample.at_ms - prev.at_ms) as f64 / 1000.0;
            push_capped(
                &mut self.net_rates,
                NetRatePoint {
                    rx_bps: (sample.rx_bytes - prev.rx_bytes) as f64 / dt_s,
                    tx_bps: (sample.tx_bytes - prev.tx_bytes) as f64 / dt_s,
                    at_ms: sample.at_ms,
                },
            );
        } else {
            // First sample, or the counters/clock went backward: rebase
            // rather than reporting a bogus (or negative) rate.
            self.net_baseline = Some(sample);
        }
        let baseline = self.net_baseline.unwrap_or(sample);
        self.net_totals = NetTotals {
            rx_bytes: sample.rx_bytes.saturating_sub(baseline.rx_bytes),
            tx_bytes: sample.tx_bytes.saturating_sub(baseline.tx_bytes),
        };
        self.last_net = Some(sample);
    }
}

fn push_capped<T>(ring: &mut VecDeque<T>, value: T) {
    ring.push_back(value);
    while ring.len() > METRICS_RING_CAP {
        ring.pop_front();
    }
}

/// The Network tab's honesty note (workbook §B12): what these counters are
/// and are not. Android's `adb shell cat /proc/net/dev` is **device-wide**;
/// desktop's `/proc/net/dev` is **namespace-wide** — see
/// `frust_drive::metrics`'s module doc, which this text is a UI-facing
/// paraphrase of. Rendered directly in the tab body (not a tooltip/doc link)
/// per §B12's ask that the caveat live where the numbers do.
pub fn network_honesty_note(identity: &MetricsIdentity) -> &'static str {
    match identity {
        MetricsIdentity::Android { .. } => {
            "Device-wide counters (adb shell cat /proc/net/dev) — every process on the \
             device, not just this app. Process-level, not request-level: request-level \
             logging requires app-side instrumentation."
        }
        MetricsIdentity::NotAndroid => {
            "Namespace-wide counters (/proc/net/dev) — every process sharing this network \
             namespace, not just this session. Process-level, not request-level: \
             request-level logging requires app-side instrumentation."
        }
    }
}

// ── Inspector tab (workbook §B12) ───────────────────────────────────────────

/// How deep a freshly-arrived *first* snapshot is expanded: every node at
/// depth `0..=1`, so the roots and their immediate children are open and the
/// grandchildren show as collapsed `▸` rows. Deeper levels stay closed — a
/// real app's tree is hundreds of nodes, and §B12's mockup opens exactly two
/// levels. A later snapshot never re-seeds: the user's own expansion wins
/// (see [`InspectorTab::apply`]'s reconciliation).
pub const INSPECTOR_AUTO_EXPAND_DEPTH: usize = 1;

/// Which pane of the Inspector tab has keyboard focus (`Tab` flips between
/// them, exactly like [`PerfFocus`] on the Performance tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InspectorFocus {
    /// The widget tree — `↑↓`/`j`/`k` move the selection here, `→`/`←`
    /// expand/collapse.
    #[default]
    Tree,
    /// The selected node's props pane.
    Props,
}

impl InspectorFocus {
    /// The other pane.
    pub fn toggle(self) -> Self {
        match self {
            InspectorFocus::Tree => InspectorFocus::Props,
            InspectorFocus::Props => InspectorFocus::Tree,
        }
    }
}

/// One *visible* row of the flattened widget tree — the shape the render
/// layer draws directly, with no second walk of the nested snapshot.
///
/// A row is produced only for a node whose every ancestor is expanded, so the
/// row list is exactly what is on screen (before scrolling) and every index
/// the engine clamps against is a visible index.
#[derive(Debug, Clone, PartialEq)]
pub struct InspectorRow {
    /// The node's protocol id — stable per widget for as long as it lives,
    /// which is what makes expansion/selection survive a refresh.
    pub id: u64,
    /// Nesting depth (`0` for a root), the render layer's indent unit.
    pub depth: usize,
    /// The wire's `type_name`, verbatim (module path included). The render
    /// layer shortens it for the tree and shows it in full in the props pane.
    pub type_name: String,
    /// The node's `debug_label`, when the app set one.
    pub debug_label: Option<String>,
    /// The node's own layout rect, when the wire carried one (the field is
    /// optional — an unlaid-out node has none).
    pub bounds: Option<RectPx>,
    /// How many children the node has (`0` = a leaf, which has no `▸`/`▾`
    /// affordance at all).
    pub child_count: usize,
    /// Whether this node is currently expanded (always `false` for a leaf).
    pub expanded: bool,
}

impl InspectorRow {
    /// Whether the row has an expand/collapse affordance.
    pub fn has_children(&self) -> bool {
        self.child_count > 0
    }
}

/// What the bridge reports back for one on-demand Inspector request
/// ([`super::Effect::DevtoolsFetchTree`] / [`super::Effect::DevtoolsFetchProps`]),
/// carried on `super::Message::DevtoolsInspector`.
#[derive(Debug, Clone, PartialEq)]
pub enum InspectorEvent {
    /// A `widget_tree` pull landed.
    TreeArrived(WidgetTreeDump),
    /// A `widget_props` pull landed for the node it names.
    PropsArrived(u64, WidgetProps),
    /// A pull failed (the request errored, or there was no live bridge to
    /// serve it). Deliberately un-attributed: the failure text is what §B12
    /// shows, and clearing *both* in-flight flags is what keeps `r` able to
    /// retry regardless of which pull died.
    Failed(String),
}

/// The Inspector tab's whole state: the last `widget_tree` snapshot, the
/// flattened visible rows derived from it, the expansion set, the selection,
/// pane focus, and the single-entry props cache for the selected node.
///
/// # Reconciliation across a refresh
///
/// `frust-core`'s inspect ids are stable per widget for as long as that
/// widget lives, so a fresh snapshot is reconciled *by id*: the expansion set
/// keeps every id still present (and drops the rest, so a long-lived session
/// can't accumulate ids for widgets that are gone), and the selection returns
/// to the row carrying the previously-selected id. If that id vanished (the
/// widget was torn down between pulls) the selection falls back to the same
/// *position* in the new row list, clamped — the nearest still-visible row —
/// rather than jumping to the top.
///
/// # Cost
///
/// Flattening is `O(visible rows)` and runs only when the tree or the
/// expansion set changes — never per frame. A moving selection re-flattens
/// nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InspectorTab {
    /// The last snapshot's roots (nested, exactly as the wire sent them).
    roots: Vec<WidgetNode>,
    /// The flattened visible rows derived from `roots` + `expanded`.
    rows: Vec<InspectorRow>,
    /// Ids of the currently-expanded nodes.
    expanded: BTreeSet<u64>,
    /// The selected row (an index into `rows`, always clamped in range).
    selected: usize,
    /// Which pane `Tab` moves between.
    pub focus: InspectorFocus,
    /// The props of *one* node — §B12's per-selection pull, not a bulk
    /// pre-fetch. Kept keyed by its own `WidgetProps::id`, so a response for
    /// a stale selection is recognizable rather than silently displayed.
    props: Option<WidgetProps>,
    /// Whether a `widget_tree` pull is in flight.
    tree_pending: bool,
    /// The node a `widget_props` pull is in flight for, if any.
    props_pending: Option<u64>,
    /// The last failure text, cleared by the next successful pull.
    error: Option<String>,
    /// Whether a snapshot has *ever* arrived (distinguishes "empty tree" from
    /// "nothing pulled yet", and gates the one-shot default expansion).
    loaded: bool,
    /// Whether an automatic pull has already been *attempted* since the last
    /// deliberate re-trigger — the latch that keeps [`Self::wants_tree`] from
    /// re-opening after a failure. Set by [`Self::begin_tree_fetch`], and
    /// **not** cleared by [`InspectorEvent::Failed`]: a failed pull leaves
    /// `tree_pending` false and `loaded` false, so without this latch every
    /// subsequent automatic check would re-fire the same failing request
    /// (see [`Self::wants_tree`]'s doc for the storm this prevents). Cleared
    /// only by [`Self::rearm_auto_pull`].
    auto_pull_attempted: bool,
}

impl InspectorTab {
    /// The visible rows, in render order.
    pub fn rows(&self) -> &[InspectorRow] {
        &self.rows
    }

    /// The selected row's index (`0` when there are no rows).
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The selected row, or `None` when the tree is empty.
    pub fn selected_row(&self) -> Option<&InspectorRow> {
        self.rows.get(self.selected)
    }

    /// The selected node's id, or `None` when the tree is empty.
    pub fn selected_id(&self) -> Option<u64> {
        self.selected_row().map(|row| row.id)
    }

    /// Whether a snapshot has ever arrived (an *empty* tree is still loaded).
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Whether a `widget_tree` pull is in flight.
    pub fn is_tree_pending(&self) -> bool {
        self.tree_pending
    }

    /// Whether a `widget_props` pull is in flight for the current selection.
    pub fn is_props_pending(&self) -> bool {
        self.props_pending.is_some() && self.props_pending == self.selected_id()
    }

    /// The cached props *for the current selection*, or `None` when none have
    /// arrived for it yet (a cache entry for another node never shows).
    pub fn selected_props(&self) -> Option<&WidgetProps> {
        let id = self.selected_id()?;
        self.props.as_ref().filter(|props| props.id == id)
    }

    /// The last failure text, if the most recent pull failed.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether an **automatic** pull is warranted: nothing loaded, nothing
    /// already in flight, and no automatic attempt made since the last
    /// deliberate re-trigger.
    ///
    /// That third clause is the once-only latch. A pull that fails clears
    /// `tree_pending` and never sets `loaded`, so the first two clauses alone
    /// re-open the gate forever — and `super::update`'s automatic call sites
    /// are message-driven, so a persistently failing pull would be re-issued
    /// at whatever rate those messages arrive (a frame-stats batch rate, in
    /// the worst case), each attempt blocking the bridge pump for a request
    /// timeout and flickering the pane. With the latch, a failed automatic
    /// pull is attempted exactly once and then waits for a deliberate
    /// re-trigger: `r` (an explicit refresh, which goes through
    /// [`Self::begin_tree_fetch`] directly and never consults this gate), or
    /// re-entering the Inspector tab ([`Self::rearm_auto_pull`]).
    pub fn wants_tree(&self) -> bool {
        !self.loaded && !self.tree_pending && !self.auto_pull_attempted
    }

    /// Re-arm the automatic pull: a *deliberate* user action — entering the
    /// Inspector tab (`3`, `[`/`]`, a tab-pill click, or re-opening DevTools
    /// onto it) — is allowed one fresh automatic attempt even after an
    /// earlier one failed, because the entry itself is the intent to look.
    /// Bounded by user input rate, unlike the message-driven checks
    /// [`Self::wants_tree`]'s latch exists to stop.
    ///
    /// A no-op once a snapshot has loaded (`wants_tree` stays false on
    /// `loaded`), so re-entering a healthy tab still never re-pulls.
    pub fn rearm_auto_pull(&mut self) {
        self.auto_pull_attempted = false;
    }

    /// Mark a `widget_tree` pull as started (`r`, or an automatic pull).
    /// Returns whether one should actually be issued — a second `r` while a
    /// pull is already in flight is a no-op rather than a duplicate request.
    ///
    /// Latches [`Self::wants_tree`] shut either way: every attempt counts as
    /// the automatic one, so a failure cannot re-open the automatic gate
    /// behind the user's back.
    pub fn begin_tree_fetch(&mut self) -> bool {
        if self.tree_pending {
            return false;
        }
        self.tree_pending = true;
        self.auto_pull_attempted = true;
        self.error = None;
        true
    }

    /// Mark a `widget_props` pull as started for the current selection,
    /// returning the id to request — §B12's second, on-demand call, fired
    /// when the selection moves onto a node whose props are neither cached
    /// nor already in flight. `None` means nothing needs requesting.
    pub fn begin_props_fetch(&mut self) -> Option<u64> {
        let id = self.selected_id()?;
        if self.props.as_ref().is_some_and(|props| props.id == id) {
            return None;
        }
        if self.props_pending == Some(id) {
            return None;
        }
        self.props_pending = Some(id);
        Some(id)
    }

    /// `Tab`: flip the focused pane. Always a change (two values).
    pub fn cycle_focus(&mut self) -> bool {
        self.focus = self.focus.toggle();
        true
    }

    /// `↑`/`↓`/`j`/`k`: move the selection `delta` visible rows, clamped to
    /// the row list (never wraps — a tree's ends are meaningful).
    pub fn select(&mut self, delta: isize) -> bool {
        if self.rows.is_empty() {
            let changed = self.selected != 0;
            self.selected = 0;
            return changed;
        }
        let next = (self.selected as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize;
        let changed = next != self.selected;
        self.selected = next;
        changed
    }

    /// Select a visible row directly by index (a row click), clamped.
    pub fn select_row(&mut self, index: usize) -> bool {
        if self.rows.is_empty() {
            return false;
        }
        let clamped = index.min(self.rows.len() - 1);
        let changed = clamped != self.selected;
        self.selected = clamped;
        changed
    }

    /// `→`/`Enter`/`Space`: expand the selected node. A leaf (or an
    /// already-open node) is a no-op.
    pub fn expand(&mut self) -> bool {
        let Some(row) = self.rows.get(self.selected) else {
            return false;
        };
        if !row.has_children() || row.expanded {
            return false;
        }
        let id = row.id;
        self.expanded.insert(id);
        self.reflatten_anchored(None);
        true
    }

    /// `←`: collapse the selected node. An already-collapsed node (or a leaf)
    /// is a no-op — v1 does not walk to the parent, which §B12 doesn't ask
    /// for.
    pub fn collapse(&mut self) -> bool {
        let Some(row) = self.rows.get(self.selected) else {
            return false;
        };
        if !row.expanded {
            return false;
        }
        let id = row.id;
        self.expanded.remove(&id);
        self.reflatten_anchored(Some(id));
        true
    }

    /// Toggle one node by id (the `▸`/`▾` click affordance — keyboard parity:
    /// `→`/`←` on the selected row). Collapsing a node that contains the
    /// selection moves the selection onto that node rather than losing it.
    pub fn toggle_node(&mut self, id: u64) -> bool {
        let Some(row) = self.rows.iter().find(|row| row.id == id) else {
            return false;
        };
        if !row.has_children() {
            return false;
        }
        if row.expanded {
            self.expanded.remove(&id);
        } else {
            self.expanded.insert(id);
        }
        self.reflatten_anchored(Some(id));
        true
    }

    /// Apply one bridge report. Returns whether the visible state changed.
    pub fn apply(&mut self, event: InspectorEvent) -> bool {
        match event {
            InspectorEvent::TreeArrived(dump) => {
                self.tree_pending = false;
                self.error = None;
                self.roots = dump.roots;
                if self.loaded {
                    // Keep the user's expansion, minus ids that are gone.
                    let live = self.live_ids();
                    self.expanded.retain(|id| live.contains(id));
                } else {
                    self.expanded.clear();
                    seed_expansion(&self.roots, 0, &mut self.expanded);
                    self.loaded = true;
                }
                // A refreshed node's props are a snapshot of the *previous*
                // pull — drop them so the selection's props are re-requested
                // (§B12: "refetched on every selection", and `r` is a fresh
                // look at everything).
                self.props = None;
                self.props_pending = None;
                self.reflatten_anchored(None);
                true
            }
            InspectorEvent::PropsArrived(id, props) => {
                let cleared = self.props_pending == Some(id);
                if cleared {
                    self.props_pending = None;
                }
                // A response for a selection that has already moved on is
                // dropped rather than cached: the cache is single-entry and
                // must always describe what the props pane is showing.
                let stored = self.selected_id() == Some(id);
                if stored {
                    self.props = Some(props);
                }
                cleared || stored
            }
            InspectorEvent::Failed(error) => {
                self.tree_pending = false;
                self.props_pending = None;
                let next = Some(error);
                let changed = self.error != next;
                self.error = next;
                changed
            }
        }
    }

    /// Every id in the current snapshot.
    fn live_ids(&self) -> BTreeSet<u64> {
        let mut ids = BTreeSet::new();
        collect_ids(&self.roots, &mut ids);
        ids
    }

    /// The visible index of `id`, if it has a row.
    fn row_index(&self, id: u64) -> Option<usize> {
        self.rows.iter().position(|row| row.id == id)
    }

    /// Re-derive [`Self::rows`] and restore the selection: onto the same node
    /// where its id survived, else onto `fallback_id` (the node whose
    /// collapse hid it), else onto the same position clamped into the new
    /// list.
    fn reflatten_anchored(&mut self, fallback_id: Option<u64>) {
        let anchor = self.selected_id();
        let position = self.selected;
        self.rows.clear();
        flatten(&self.roots, 0, &self.expanded, &mut self.rows);
        self.selected = anchor
            .and_then(|id| self.row_index(id))
            .or_else(|| fallback_id.and_then(|id| self.row_index(id)))
            .unwrap_or_else(|| position.min(self.rows.len().saturating_sub(1)));
    }
}

/// Walk `nodes` into visible rows, descending only through expanded nodes.
/// Recursive, like the wire shape itself — a widget tree is UI-sized (tens to
/// low hundreds of levels at the absolute worst), so no explicit stack is
/// warranted.
fn flatten(
    nodes: &[WidgetNode],
    depth: usize,
    expanded: &BTreeSet<u64>,
    rows: &mut Vec<InspectorRow>,
) {
    for node in nodes {
        let open = expanded.contains(&node.id) && !node.children.is_empty();
        rows.push(InspectorRow {
            id: node.id,
            depth,
            type_name: node.type_name.clone(),
            debug_label: node.debug_label.clone(),
            bounds: node.bounds,
            child_count: node.children.len(),
            expanded: open,
        });
        if open {
            flatten(&node.children, depth + 1, expanded, rows);
        }
    }
}

/// Seed the one-shot default expansion: every node with children down to
/// [`INSPECTOR_AUTO_EXPAND_DEPTH`].
fn seed_expansion(nodes: &[WidgetNode], depth: usize, expanded: &mut BTreeSet<u64>) {
    if depth > INSPECTOR_AUTO_EXPAND_DEPTH {
        return;
    }
    for node in nodes {
        if !node.children.is_empty() {
            expanded.insert(node.id);
        }
        seed_expansion(&node.children, depth + 1, expanded);
    }
}

/// Collect every id in a snapshot (expansion pruning).
fn collect_ids(nodes: &[WidgetNode], ids: &mut BTreeSet<u64>) {
    for node in nodes {
        ids.insert(node.id);
        collect_ids(&node.children, ids);
    }
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
    fn a_service_start_failure_line_is_captured_and_cleared_by_a_later_bind() {
        let mut state = debug_state();
        // An ordinary line is neither discovery nor failure.
        assert!(!state.ingest_line("app: booting up"));
        assert!(state.start_error.is_none());

        // The app reports its service could not start — captured, no connect.
        assert!(!state.ingest_line(
            "08-10 10:22:16.394 27868 27868 W frust: frust_shell_common::devtools: \
             frust-devtools: service did not start: Connection refused (os error 111)"
        ));
        assert_eq!(
            state.start_error.as_deref(),
            Some("Connection refused (os error 111)")
        );
        assert!(
            !state.wants_connect(),
            "a failure is not a port to connect to"
        );

        // A later successful bind supersedes the failure.
        assert!(state.ingest_line("frust-devtools listening on 38171 token abcd"));
        assert!(state.start_error.is_none());
        assert!(state.wants_connect());
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

    // ── System/Network metrics (workbook §B12) ─────────────────────────────

    fn android_metrics(serial: &str) -> MetricsState {
        MetricsState::new(Some(serial.to_string()))
    }

    #[test]
    fn a_non_android_identity_starts_already_unavailable() {
        let metrics = MetricsState::new(None);
        assert_eq!(metrics.identity, MetricsIdentity::NotAndroid);
        assert!(matches!(
            metrics.sampling,
            SamplingState::Unavailable { .. }
        ));
        assert!(
            !metrics.wants_start(),
            "a desktop identity never wants a start"
        );
        assert!(metrics.target().is_none());
    }

    #[test]
    fn an_android_identity_resolves_from_its_own_launch_lines_in_either_order() {
        let mut metrics = android_metrics("emulator-5554");
        assert_eq!(metrics.sampling, SamplingState::Off);
        assert!(!metrics.wants_start(), "neither pkg nor pid known yet");

        assert!(
            !metrics.ingest_line("Launching it.f0x.huddle…"),
            "pkg alone is not ready"
        );
        assert!(!metrics.wants_start());
        assert!(
            metrics.ingest_line("Streaming logs (pid 4242)"),
            "the pid completes the pair"
        );
        assert!(metrics.wants_start());
        assert_eq!(
            metrics.target(),
            Some((
                "emulator-5554".to_string(),
                "4242".to_string(),
                "it.f0x.huddle".to_string()
            ))
        );

        // Re-feeding either line again is not a fresh "became ready" edge.
        assert!(!metrics.ingest_line("Launching it.f0x.huddle…"));
        assert!(!metrics.ingest_line("Streaming logs (pid 4242)"));

        // An unrelated line is ignored.
        let mut fresh = android_metrics("emulator-5554");
        assert!(!fresh.ingest_line("app: booting up"));
    }

    #[test]
    fn a_started_sampler_stops_on_session_end_but_an_idle_one_has_nothing_to_stop() {
        let mut metrics = android_metrics("emulator-5554");
        metrics.ingest_line("Launching it.f0x.huddle…");
        metrics.ingest_line("Streaming logs (pid 4242)");
        metrics.begin_sampling();
        assert_eq!(metrics.sampling, SamplingState::On);
        assert!(metrics.on_session_end());
        assert!(matches!(
            metrics.sampling,
            SamplingState::Unavailable { .. }
        ));

        let mut idle = android_metrics("emulator-5554");
        assert!(!idle.on_session_end(), "nothing was ever started");

        let mut desktop = MetricsState::new(None);
        assert!(
            !desktop.on_session_end(),
            "an already-unavailable identity has nothing to stop either"
        );
    }

    fn cpu(percent: f32, at_ms: u64) -> MetricsSample {
        MetricsSample::Cpu(frust_drive::metrics::CpuSample { percent, at_ms })
    }

    fn mem(rss_bytes: u64, at_ms: u64) -> MetricsSample {
        MetricsSample::Mem(frust_drive::metrics::MemSample { rss_bytes, at_ms })
    }

    fn thermal(zone: &str, millideg_c: i64, at_ms: u64) -> MetricsSample {
        MetricsSample::Thermal(frust_drive::metrics::ThermalSample {
            zone_label: zone.to_string(),
            millideg_c,
            at_ms,
        })
    }

    fn net(rx_bytes: u64, tx_bytes: u64, at_ms: u64) -> MetricsSample {
        MetricsSample::Net(NetSample {
            rx_bytes,
            tx_bytes,
            at_ms,
        })
    }

    #[test]
    fn cpu_and_rss_rings_cap_and_keep_the_newest_samples() {
        let mut metrics = MetricsState::new(None);
        for n in 0..(METRICS_RING_CAP as u64 + 10) {
            metrics.apply(cpu(1.0, n));
            metrics.apply(mem(1000 + n, n));
        }
        assert_eq!(metrics.cpu.len(), METRICS_RING_CAP);
        assert_eq!(metrics.cpu.front().unwrap().at_ms, 10);
        assert_eq!(metrics.rss.len(), METRICS_RING_CAP);
        assert_eq!(
            metrics.rss.back().unwrap().rss_bytes,
            1000 + METRICS_RING_CAP as u64 + 9
        );
    }

    #[test]
    fn latest_at_ms_tracks_the_newest_sample_across_every_kind() {
        let mut metrics = MetricsState::new(None);
        assert_eq!(metrics.latest_at_ms, None);
        metrics.apply(cpu(5.0, 100));
        assert_eq!(metrics.latest_at_ms, Some(100));
        metrics.apply(mem(2048, 50));
        assert_eq!(
            metrics.latest_at_ms,
            Some(100),
            "an older sample never regresses it"
        );
        metrics.apply(thermal("cpu-0", 42000, 300));
        assert_eq!(metrics.latest_at_ms, Some(300));
    }

    #[test]
    fn thermal_keeps_only_the_latest_reading_per_zone() {
        let mut metrics = MetricsState::new(None);
        metrics.apply(thermal("cpu-0", 40000, 10));
        metrics.apply(thermal("cpu-1", 38000, 10));
        metrics.apply(thermal("cpu-0", 41500, 20));
        assert_eq!(metrics.thermal.len(), 2);
        assert_eq!(metrics.thermal["cpu-0"].millideg_c, 41500);
        assert_eq!(metrics.thermal["cpu-0"].at_ms, 20);
        assert_eq!(metrics.thermal["cpu-1"].millideg_c, 38000);
    }

    #[test]
    fn the_first_net_sample_emits_no_rate_and_zeroes_the_totals() {
        let mut metrics = MetricsState::new(None);
        assert!(!metrics.has_net_data());
        metrics.apply(net(1_000, 500, 0));
        assert!(metrics.has_net_data());
        assert!(metrics.net_rates.is_empty(), "nothing to diff against yet");
        assert_eq!(
            metrics.net_totals,
            NetTotals {
                rx_bytes: 0,
                tx_bytes: 0
            }
        );
    }

    #[test]
    fn a_second_net_sample_derives_a_rate_and_rolls_the_totals_forward() {
        let mut metrics = MetricsState::new(None);
        metrics.apply(net(1_000, 500, 0));
        metrics.apply(net(3_000, 1_500, 1_000)); // +2000 rx, +1000 tx over 1s
        assert_eq!(metrics.net_rates.len(), 1);
        let rate = metrics.net_rates.back().unwrap();
        assert!((rate.rx_bps - 2000.0).abs() < f64::EPSILON);
        assert!((rate.tx_bps - 1000.0).abs() < f64::EPSILON);
        assert_eq!(
            metrics.net_totals,
            NetTotals {
                rx_bytes: 2_000,
                tx_bytes: 1_000
            }
        );
    }

    #[test]
    fn a_backward_counter_is_treated_as_a_reset_not_a_negative_rate() {
        let mut metrics = MetricsState::new(None);
        metrics.apply(net(5_000, 5_000, 0));
        metrics.apply(net(6_000, 5_500, 1_000)); // one normal tick first
        assert_eq!(metrics.net_rates.len(), 1);

        // A reboot/forward-restart: counters go backward.
        metrics.apply(net(100, 50, 2_000));
        assert_eq!(
            metrics.net_rates.len(),
            1,
            "a reset tick emits no new rate point"
        );
        assert_eq!(
            metrics.net_totals,
            NetTotals {
                rx_bytes: 0,
                tx_bytes: 0
            },
            "totals rebase to zero at the reset point"
        );

        // The tick after a reset resumes normal diffing from the new baseline.
        metrics.apply(net(600, 350, 3_000));
        assert_eq!(metrics.net_rates.len(), 2);
        let rate = metrics.net_rates.back().unwrap();
        assert!((rate.rx_bps - 500.0).abs() < f64::EPSILON);
        assert_eq!(
            metrics.net_totals,
            NetTotals {
                rx_bytes: 500,
                tx_bytes: 300
            }
        );
    }

    #[test]
    fn a_non_advancing_clock_is_also_treated_as_a_reset() {
        let mut metrics = MetricsState::new(None);
        metrics.apply(net(1_000, 1_000, 500));
        // Same or earlier `at_ms` despite counters advancing: no wall-clock
        // delta to divide by, so this rebases rather than dividing by zero.
        metrics.apply(net(2_000, 1_500, 500));
        assert!(metrics.net_rates.is_empty());
        assert_eq!(
            metrics.net_totals,
            NetTotals {
                rx_bytes: 0,
                tx_bytes: 0
            }
        );
    }

    #[test]
    fn network_honesty_note_names_the_right_scope_per_platform() {
        assert!(
            network_honesty_note(&MetricsIdentity::NotAndroid)
                .to_lowercase()
                .contains("namespace-wide")
        );
        let android = MetricsIdentity::Android {
            serial: "emulator-5554".to_string(),
            pkg: None,
            pid: None,
        };
        assert!(
            network_honesty_note(&android)
                .to_lowercase()
                .contains("device-wide")
        );
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

    /// A window of `len` frames whose identities start at `first_n` — the
    /// sliding-window fixture: `window(100, 10)` and `window(105, 10)` are
    /// "the same chart, five frames later".
    fn window(first_n: u64, len: u64) -> Vec<PerfFrame> {
        (first_n..first_n + len)
            .map(|n| phased_frame(n, 16_000, false))
            .collect()
    }

    #[test]
    fn scrub_starts_from_the_tail_and_clamps_at_both_ends() {
        let w = window(100, 10);
        let mut perf = PerformanceTab::default();
        assert_eq!(perf.selected_n, None);

        // First press starts at the tail (the newest frame), not the oldest.
        assert!(perf.scrub(0, &w));
        assert_eq!(perf.selected_n, Some(109));

        assert!(perf.scrub(-1, &w));
        assert_eq!(perf.selected_n, Some(108));
        assert!(perf.scrub(1, &w));
        assert_eq!(perf.selected_n, Some(109));

        // Clamped at the newest end — repeated Right never overflows.
        assert!(!perf.scrub(1, &w), "already at the newest frame");
        assert_eq!(perf.selected_n, Some(109));

        // Clamped at the oldest end.
        for _ in 0..20 {
            perf.scrub(-1, &w);
        }
        assert_eq!(perf.selected_n, Some(100));
        assert!(!perf.scrub(-1, &w), "already at the oldest frame");
    }

    #[test]
    fn scrub_on_an_empty_window_clears_any_selection() {
        let mut perf = PerformanceTab {
            selected_n: Some(3),
            ..Default::default()
        };
        assert!(perf.scrub(1, &[]));
        assert_eq!(perf.selected_n, None);
    }

    #[test]
    fn a_pinned_frame_keeps_its_identity_as_the_window_slides() {
        // The exact walk scenario: pin one frame, then let five more arrive.
        let mut perf = PerformanceTab::default();
        let before = window(100, 10);
        assert!(perf.select_frame(108, &before));
        assert_eq!(perf.resolve(&before), Some(8));

        for shift in 1..=5u64 {
            let after = window(100 + shift, 10);
            assert!(
                !perf.retain_in_window(&after),
                "frame 108 is still retained after {shift} more frames"
            );
            assert_eq!(perf.selected_n, Some(108), "the pin never changes frame");
            assert_eq!(
                perf.resolve(&after),
                Some(8 - shift as usize),
                "it only moves *left* as the window slides under it"
            );
        }
    }

    #[test]
    fn scrub_steps_to_the_adjacent_retained_frame_by_identity() {
        let mut perf = PerformanceTab::default();
        let before = window(100, 10);
        assert!(perf.select_frame(105, &before));

        // Three more frames arrive: the pin is still 105, and one step left
        // lands on 104 — the adjacent *frame*, not "one column left of where
        // 105 used to be" (which would now be 107's column).
        let after = window(103, 10);
        assert!(perf.scrub(-1, &after));
        assert_eq!(perf.selected_n, Some(104));
        assert!(perf.scrub(1, &after));
        assert_eq!(perf.selected_n, Some(105));
    }

    #[test]
    fn a_pin_that_falls_out_of_the_window_drops_back_to_the_live_tail() {
        let mut perf = PerformanceTab::default();
        let before = window(100, 10);
        assert!(perf.select_frame(101, &before));

        // Frame 101 is gone from the window entirely: clear rather than
        // clamp onto the oldest column (which would name a fresh frame on
        // every batch — the walk this type exists to prevent).
        let after = window(110, 10);
        assert!(perf.retain_in_window(&after));
        assert_eq!(perf.selected_n, None);
        assert!(!perf.has_selection());
        assert_eq!(perf.resolve(&after), None, "reads as the live tail");
        assert!(
            !perf.retain_in_window(&after),
            "nothing pinned is nothing to drop"
        );
    }

    #[test]
    fn a_click_naming_a_frame_that_has_left_the_window_pins_nothing() {
        let mut perf = PerformanceTab::default();
        let before = window(100, 10);
        assert!(perf.select_frame(104, &before));
        assert!(
            !perf.select_frame(104, &before),
            "re-pinning the same frame"
        );

        // The click was registered against a column drawing frame 100; by
        // the time it is consumed the window has moved past it. The payload
        // is the frame's own identity, so this is recognizably stale rather
        // than silently pinning whatever slid into that column.
        let after = window(110, 10);
        assert!(!perf.select_frame(100, &after));
        assert_eq!(perf.selected_n, Some(104), "the old pin is left alone");
        assert!(perf.select_frame(115, &after));
        assert_eq!(perf.selected_n, Some(115));
    }

    #[test]
    fn clear_selection_is_the_esc_first_stage() {
        let mut perf = PerformanceTab {
            selected_n: Some(2),
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

    // ── Inspector tab ────────────────────────────────────────────────────

    fn node(id: u64, type_name: &str, children: Vec<WidgetNode>) -> WidgetNode {
        WidgetNode {
            id,
            type_name: type_name.to_string(),
            debug_label: None,
            bounds: Some(RectPx {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 20.0,
            }),
            children,
        }
    }

    /// ```text
    /// 1 Column                (d0, expanded by default)
    ///   2 Padding             (d1, expanded by default)
    ///     3 Text              (d2)
    ///     4 Row               (d2, collapsed — past the default depth)
    ///       5 Text            (d3, hidden)
    ///   6 ListView            (d1, expanded by default)
    ///     7 Text              (d2)
    /// ```
    fn tree() -> WidgetTreeDump {
        WidgetTreeDump {
            roots: vec![node(
                1,
                "frust_widgets::flex::FlexWidget",
                vec![
                    node(
                        2,
                        "frust_widgets::padding::PaddingWidget",
                        vec![
                            node(3, "frust_widgets::text::TextWidget", Vec::new()),
                            node(
                                4,
                                "frust_widgets::flex::FlexWidget",
                                vec![node(5, "frust_widgets::text::TextWidget", Vec::new())],
                            ),
                        ],
                    ),
                    node(
                        6,
                        "frust_widgets::list_view::ListViewWidget",
                        vec![node(7, "frust_widgets::text::TextWidget", Vec::new())],
                    ),
                ],
            )],
        }
    }

    fn loaded_inspector() -> InspectorTab {
        let mut inspector = InspectorTab::default();
        assert!(inspector.apply(InspectorEvent::TreeArrived(tree())));
        inspector
    }

    fn visible_ids(inspector: &InspectorTab) -> Vec<u64> {
        inspector.rows().iter().map(|row| row.id).collect()
    }

    #[test]
    fn a_first_snapshot_expands_the_roots_and_one_level_below_them() {
        let inspector = loaded_inspector();
        assert!(inspector.is_loaded());
        // Depth 0 (1) and depth 1 (2, 6) are expanded; 4's children stay
        // hidden because 4 sits at depth 2.
        assert_eq!(visible_ids(&inspector), vec![1, 2, 3, 4, 6, 7]);
        assert_eq!(inspector.rows()[0].depth, 0);
        assert_eq!(inspector.rows()[2].depth, 2);
        assert!(
            inspector.rows()[3].has_children(),
            "the Row node has a child"
        );
        assert!(!inspector.rows()[3].expanded);
        assert_eq!(inspector.selected_index(), 0);
    }

    #[test]
    fn expand_and_collapse_re_flatten_the_visible_rows() {
        let mut inspector = loaded_inspector();
        // Select the collapsed Row (id 4) and open it.
        assert!(inspector.select(3));
        assert_eq!(inspector.selected_id(), Some(4));
        assert!(inspector.expand());
        assert_eq!(visible_ids(&inspector), vec![1, 2, 3, 4, 5, 6, 7]);
        assert!(!inspector.expand(), "already expanded");

        assert!(inspector.collapse());
        assert_eq!(visible_ids(&inspector), vec![1, 2, 3, 4, 6, 7]);
        assert!(!inspector.collapse(), "already collapsed");
        assert_eq!(inspector.selected_id(), Some(4), "selection stays put");

        // A leaf has no affordance at all.
        assert!(inspector.select_row(2));
        assert!(!inspector.expand());
        assert!(!inspector.collapse());
    }

    #[test]
    fn collapsing_an_ancestor_moves_the_selection_onto_it() {
        let mut inspector = loaded_inspector();
        assert!(inspector.select_row(2)); // the Text at depth 2
        assert_eq!(inspector.selected_id(), Some(3));
        // Collapsing its parent hides it — the selection lands on the parent
        // rather than on whatever row inherits its index.
        assert!(inspector.toggle_node(2));
        assert_eq!(visible_ids(&inspector), vec![1, 2, 6, 7]);
        assert_eq!(inspector.selected_id(), Some(2));
    }

    #[test]
    fn selection_clamps_at_both_ends_and_on_an_empty_tree() {
        let mut inspector = loaded_inspector();
        assert!(!inspector.select(-1), "already at the top");
        assert!(inspector.select(99));
        assert_eq!(inspector.selected_index(), 5, "clamped to the last row");
        assert!(!inspector.select(1));
        assert!(inspector.select_row(0));
        assert!(inspector.select_row(99), "a click past the end clamps too");
        assert_eq!(inspector.selected_index(), 5);

        let mut empty = InspectorTab::default();
        assert!(!empty.select(1));
        assert!(!empty.select_row(3));
        assert_eq!(empty.selected_id(), None);
    }

    #[test]
    fn a_refresh_preserves_expansion_and_selection_by_id() {
        let mut inspector = loaded_inspector();
        inspector.select_row(3); // the Row (id 4)
        inspector.expand(); // open it, beyond the default depth
        inspector.select_row(4); // its child Text (id 5)
        assert_eq!(inspector.selected_id(), Some(5));

        // The same tree pulled again: ids persist, so both the user's extra
        // expansion and the selection come back.
        assert!(inspector.apply(InspectorEvent::TreeArrived(tree())));
        assert_eq!(visible_ids(&inspector), vec![1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(inspector.selected_id(), Some(5));
    }

    #[test]
    fn a_refresh_that_loses_the_selected_id_falls_back_to_the_nearest_row() {
        let mut inspector = loaded_inspector();
        inspector.select_row(3); // id 4
        assert_eq!(inspector.selected_id(), Some(4));

        // A snapshot without ids 4/5 at all (that subtree was torn down).
        let shrunk = WidgetTreeDump {
            roots: vec![node(
                1,
                "frust_widgets::flex::FlexWidget",
                vec![node(
                    2,
                    "frust_widgets::padding::PaddingWidget",
                    vec![node(3, "frust_widgets::text::TextWidget", Vec::new())],
                )],
            )],
        };
        inspector.apply(InspectorEvent::TreeArrived(shrunk));
        assert_eq!(visible_ids(&inspector), vec![1, 2, 3]);
        assert_eq!(
            inspector.selected_index(),
            2,
            "the old position, clamped into the shorter list"
        );
        // The vanished ids are pruned from the expansion set too, so a
        // long-lived session can't accumulate them.
        inspector.apply(InspectorEvent::TreeArrived(tree()));
        assert_eq!(
            visible_ids(&inspector),
            vec![1, 2, 3, 4, 6],
            "id 4's earlier expansion did not survive its absence"
        );
    }

    #[test]
    fn props_are_fetched_once_per_selection_and_cached() {
        let mut inspector = loaded_inspector();
        // The tree pull cleared any cache, so the first selection needs props.
        assert_eq!(inspector.begin_props_fetch(), Some(1));
        assert!(inspector.is_props_pending());
        assert_eq!(
            inspector.begin_props_fetch(),
            None,
            "already in flight for this node"
        );

        let props = WidgetProps {
            id: 1,
            entries: vec![("axis".to_string(), "Vertical".to_string())],
        };
        assert!(inspector.apply(InspectorEvent::PropsArrived(1, props)));
        assert!(!inspector.is_props_pending());
        assert_eq!(inspector.selected_props().map(|p| p.id), Some(1));
        assert_eq!(
            inspector.begin_props_fetch(),
            None,
            "already cached for this node"
        );

        // Moving the selection invalidates the cache and asks for the new one.
        inspector.select(1);
        assert_eq!(inspector.selected_props(), None);
        assert_eq!(inspector.begin_props_fetch(), Some(2));
    }

    #[test]
    fn a_props_response_for_a_stale_selection_is_dropped() {
        let mut inspector = loaded_inspector();
        inspector.begin_props_fetch(); // for id 1
        inspector.select(1); // the user moved on to id 2

        let stale = WidgetProps {
            id: 1,
            entries: vec![("axis".to_string(), "Vertical".to_string())],
        };
        inspector.apply(InspectorEvent::PropsArrived(1, stale));
        assert_eq!(
            inspector.selected_props(),
            None,
            "id 1's props never show under id 2"
        );
        // …and the current selection is still requestable.
        assert_eq!(inspector.begin_props_fetch(), Some(2));
    }

    #[test]
    fn tree_fetches_are_gated_on_one_in_flight_pull() {
        let mut inspector = InspectorTab::default();
        assert!(inspector.wants_tree(), "nothing loaded yet");
        assert!(inspector.begin_tree_fetch());
        assert!(inspector.is_tree_pending());
        assert!(!inspector.wants_tree(), "a pull is already in flight");
        assert!(!inspector.begin_tree_fetch(), "no duplicate request");

        inspector.apply(InspectorEvent::TreeArrived(tree()));
        assert!(!inspector.is_tree_pending());
        assert!(!inspector.wants_tree(), "the tab has its snapshot");
        // `r` still re-pulls on demand.
        assert!(inspector.begin_tree_fetch());
    }

    #[test]
    fn a_failed_pull_records_the_reason_and_frees_both_in_flight_flags() {
        let mut inspector = InspectorTab::default();
        inspector.begin_tree_fetch();
        assert!(inspector.apply(InspectorEvent::Failed("connection closed".to_string())));
        assert_eq!(inspector.error(), Some("connection closed"));
        assert!(!inspector.is_tree_pending());
        assert!(
            !inspector.apply(InspectorEvent::Failed("connection closed".to_string())),
            "the same failure again is not a visible change"
        );
        // Retrying clears it.
        assert!(inspector.begin_tree_fetch());
        assert_eq!(inspector.error(), None);
    }

    #[test]
    fn a_failed_pull_does_not_re_arm_the_automatic_one() {
        let mut inspector = InspectorTab::default();
        assert!(inspector.wants_tree());
        inspector.begin_tree_fetch();
        inspector.apply(InspectorEvent::Failed("connection closed".to_string()));

        // The failure freed `tree_pending` and never set `loaded`, so only
        // the attempted-once latch stands between a failing service and an
        // unbounded automatic-retry storm.
        assert!(!inspector.is_tree_pending());
        assert!(!inspector.is_loaded());
        assert!(
            !inspector.wants_tree(),
            "the automatic pull is spent until something deliberate re-arms it"
        );
        for _ in 0..50 {
            assert!(!inspector.wants_tree());
        }

        // `r` is an explicit refresh — it never consults the gate.
        assert!(inspector.begin_tree_fetch());
        inspector.apply(InspectorEvent::Failed("connection closed".to_string()));
        assert!(!inspector.wants_tree(), "and re-latches on its way through");
    }

    #[test]
    fn re_entering_the_tab_re_arms_one_more_automatic_attempt() {
        let mut inspector = InspectorTab::default();
        inspector.begin_tree_fetch();
        inspector.apply(InspectorEvent::Failed("connection closed".to_string()));
        assert!(!inspector.wants_tree());

        inspector.rearm_auto_pull();
        assert!(
            inspector.wants_tree(),
            "a tab entry is a user action, so it gets one fresh attempt"
        );
        inspector.begin_tree_fetch();
        inspector.apply(InspectorEvent::TreeArrived(tree()));

        // Once a snapshot has landed, re-arming changes nothing: `loaded`
        // keeps the automatic pull shut for good.
        inspector.rearm_auto_pull();
        assert!(!inspector.wants_tree());
    }

    #[test]
    fn focus_cycles_between_the_tree_and_the_props_pane() {
        let mut inspector = InspectorTab::default();
        assert_eq!(inspector.focus, InspectorFocus::Tree);
        assert!(inspector.cycle_focus());
        assert_eq!(inspector.focus, InspectorFocus::Props);
        assert!(inspector.cycle_focus());
        assert_eq!(inspector.focus, InspectorFocus::Tree);
    }

    #[test]
    fn an_empty_snapshot_is_loaded_with_no_rows() {
        let mut inspector = InspectorTab::default();
        inspector.apply(InspectorEvent::TreeArrived(WidgetTreeDump {
            roots: Vec::new(),
        }));
        assert!(inspector.is_loaded());
        assert!(inspector.rows().is_empty());
        assert_eq!(inspector.selected_id(), None);
        assert_eq!(inspector.begin_props_fetch(), None);
    }
}
