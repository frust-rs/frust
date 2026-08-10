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
//! needs plus [`DevtoolsState::frames`] (Performance's ring). The remaining
//! three tabs each add their own payload slot here — a System metrics/CPU
//! sample, an Inspector widget-tree snapshot + selection, a Network counter
//! ring — fed by new [`ConnEvent`] variants the bridge forwards. Adding a
//! slot is additive: nothing outside this module reads the ring or the
//! connection state except through the accessors below and
//! [`DevtoolsState::phase`], whose five values are the §B12 screen set and
//! are matched exhaustively by the render and key-routing layers.

use std::collections::VecDeque;

use frust_devtools_protocol::{Capability, Discovery, FrameStats};
use frust_drive::build_info::BuildMode;

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
}
