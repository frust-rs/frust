//! Frame-timing and startup-span perf instrumentation shared by every shell
//! (spec §14 phase 7, plan phase 7.A).
//!
//! # What lives here
//!
//! - [`FrameStats`] — a per-shell recorder of one frame's pass durations
//!   (rebuild/layout/paint/encode+present, plus a `skipped` marker the
//!   mobile dirty-gate (task 16+) will set), aggregated into a ring buffer
//!   plus running totals; [`FrameStats::summary`] reports p50/p95/p99 total
//!   frame time, per-pass p95, and frames-over-budget counts against the
//!   16.6ms/8.3ms (60Hz/120Hz) targets.
//! - [`StartupSpans`] — named monotonic timestamps from a shell's `begin()`
//!   epoch (native-lib load, init entry, adapter/device/renderer ready,
//!   first rebuild done, first frame presented — see the `SPAN_*` consts),
//!   summarized into one log line.
//! - [`enabled`] — the process-wide on/off switch every recording API is a
//!   no-op behind (see its own docs).
//! - [`raw_enabled`] (Phase 9.C) — a second dial that, alongside
//!   [`enabled`], makes [`FrameStats::record`] additionally emit one
//!   `frust-perf raw` line per recorded frame (instead of only the
//!   rate-limited ~2s `frust-perf frame` summary [`FrameStats::emit_log`]
//!   already produces), plus [`mark_scenario_start`]/[`mark_scenario_end`]
//!   for a benchmark harness to slice that per-frame series into named
//!   scenarios.
//!
//! # Layering choice
//!
//! This lives in `frust-shell-common`, not `frust-core` — timing is
//! shell-owned by design (`docs/CODE_STANDARDS.md`'s "no `Instant::now()` in
//! `frust-core`/`frust-widgets`" rule binds the framework layers only;
//! a shell reading a wall clock to time its own passes is exactly the kind
//! of shell-facing responsibility this crate already carries alongside
//! `theme_override`/`ffi_support`). Every recording API takes an
//! already-measured [`std::time::Duration`] rather than reading a clock
//! itself, and [`StartupSpans`] is generic over an injectable [`Clock`] —
//! this crate's own logic stays fully host-testable without a real clock.
//!
//! # Wiring is a later task
//!
//! This module ships the recorder + switch only; no shell constructs or
//! feeds a [`FrameStats`]/[`StartupSpans`] yet (tasks 08/09/10 wire the
//! Android/iOS/desktop shells respectively).

use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Ring-buffer capacity for [`FrameStats`] — roughly 2 seconds of frames at
/// 60Hz, enough for a stable rolling percentile without unbounded growth.
pub const RING_CAPACITY: usize = 120;

/// The 60Hz frame budget (1000/60 ms), truncated to whole microseconds.
pub const BUDGET_60HZ: Duration = Duration::from_micros(16_667);

/// The 120Hz frame budget (1000/120 ms), truncated to whole microseconds.
pub const BUDGET_120HZ: Duration = Duration::from_micros(8_333);

/// Minimum span of recorded frame time between two [`FrameStats::emit_log`]
/// calls that [`FrameStats::should_emit`] requires — "one line every ~2s of
/// frames" per the task spec, measured in accumulated frame time rather than
/// wall-clock time so it needs no clock of its own.
const EMIT_INTERVAL: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------
// On/off switch
// ---------------------------------------------------------------------

/// The process-wide perf-instrumentation switch, cached after the first
/// call. `true` when either:
///
/// - the compile-time `FRUST_TRACE` define is set to a non-`"0"` value
///   (the `--define`/`--profile` path — task 03 makes `--profile` builds
///   pass `FRUST_TRACE=1` by default; `option_env!` reads whatever a
///   build script/cargo-ndk env var set at compile time), or
/// - the runtime `FRUST_TRACE` process environment variable is set to a
///   non-`"0"` value (desktop dev: `FRUST_TRACE=1 cargo run -p ...`,
///   mirroring `FRUST_RENDER_TIER`'s runtime-override convention).
///
/// Every recording API in this module (`FrameStats::record`,
/// `StartupSpans::record`, both `emit_log`s) is a cheap no-op when this is
/// `false` — no allocation, no clock read, on the hot path.
///
/// Cached in a `OnceLock`: the switch is read once per process and never
/// changes afterward, so this is intentionally not re-evaluatable at
/// runtime — a shell that needs to bypass the cache for testing should
/// construct a [`FrameStats`]/[`StartupSpans`] via the explicit
/// `*_enabled`/`begin_with` constructors instead of relying on this
/// function's cache.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| trace_switch(option_env!("FRUST_TRACE"), runtime_trace_var().as_deref()))
}

/// Reads the runtime `FRUST_TRACE` env var, isolated into its own
/// function so [`enabled`]'s caching is the only thing that touches the
/// process environment — [`trace_switch`] itself stays a pure, directly
/// unit-testable function.
fn runtime_trace_var() -> Option<String> {
    std::env::var("FRUST_TRACE").ok()
}

/// The pure decision [`enabled`] caches: non-empty and not the literal
/// string `"0"` counts as "set" for either the compile-time or runtime
/// value; either source being set is enough.
fn trace_switch(compile_time: Option<&str>, runtime: Option<&str>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if v != "0")
    }
    is_set_non_zero(compile_time) || is_set_non_zero(runtime)
}

/// The process-wide raw-per-frame-export switch (Phase 9.C), cached after
/// the first call — parsed exactly the same compile-time-or-runtime way as
/// [`enabled`] (via the same [`trace_switch`] decision), but reading
/// `FRUST_TRACE_RAW` instead of `FRUST_TRACE`. This is a **second dial**,
/// not a replacement: `FRUST_TRACE_RAW` being set implies nothing on its own
/// — [`FrameStats::record`]'s raw per-frame line only fires when [`enabled`]
/// is *also* true (a benchmark harness sets both `FRUST_TRACE=1` and
/// `FRUST_TRACE_RAW=1`; see [`FrameStats::new`]).
pub fn raw_enabled() -> bool {
    static RAW_ENABLED: OnceLock<bool> = OnceLock::new();
    *RAW_ENABLED.get_or_init(|| {
        trace_switch(
            option_env!("FRUST_TRACE_RAW"),
            runtime_trace_raw_var().as_deref(),
        )
    })
}

/// Reads the runtime `FRUST_TRACE_RAW` env var, isolated for the same reason
/// [`runtime_trace_var`] is.
fn runtime_trace_raw_var() -> Option<String> {
    std::env::var("FRUST_TRACE_RAW").ok()
}

// ---------------------------------------------------------------------
// FrameStats
// ---------------------------------------------------------------------

/// One frame's measured pass durations, as recorded by a shell's frame
/// callback (`AndroidAppHandle::frame` / `frust_render_frame` /
/// desktop's `RedrawRequested` handler — see `docs/ARCHITECTURE.md`'s Frame
/// pipeline). `skipped` is set by the mobile dirty-gate (a later task) for a
/// frame whose passes never ran; its pass durations are `Duration::ZERO` in
/// that case and it is excluded from the percentile computation in
/// [`FrameStats::summary`] (see that method's docs) while still counting
/// toward `total_frames`/`skipped_frames`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FramePasses {
    pub rebuild: Duration,
    pub layout: Duration,
    pub paint: Duration,
    pub encode_present: Duration,
    pub skipped: bool,
}

impl FramePasses {
    /// The sum of all four pass durations — the frame's total wall time.
    pub fn total(&self) -> Duration {
        self.rebuild + self.layout + self.paint + self.encode_present
    }
}

/// A rolling summary over [`FrameStats`]'s current ring-buffer window plus
/// the lifetime running counters — the shape [`FrameStats::emit_log`]'s log
/// line reports and tests assert against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSummary {
    /// Frames currently held in the ring buffer (`<= RING_CAPACITY`), i.e.
    /// how many samples the percentiles below are computed over.
    pub frame_count: usize,
    pub total_p50: Duration,
    pub total_p95: Duration,
    pub total_p99: Duration,
    pub rebuild_p95: Duration,
    pub layout_p95: Duration,
    pub paint_p95: Duration,
    pub encode_present_p95: Duration,
    /// Lifetime count of frames whose total exceeded [`BUDGET_60HZ`]
    /// (16.6ms) — a running total, not windowed to the ring buffer.
    pub over_60hz_budget: u64,
    /// Lifetime count of frames whose total exceeded [`BUDGET_120HZ`]
    /// (8.3ms) — a running total, not windowed to the ring buffer.
    pub over_120hz_budget: u64,
    /// Lifetime count of frames recorded with `FramePasses::skipped == true`
    /// — a running total, not windowed to the ring buffer.
    pub skipped_frames: u64,
}

/// Per-shell frame-timing recorder (spec §14 phase 7.A): a ring buffer of
/// the last [`RING_CAPACITY`] frames' [`FramePasses`] plus lifetime running
/// counters, aggregated on demand by [`FrameStats::summary`] and rate-limit
/// logged by [`FrameStats::should_emit`]/[`FrameStats::emit_log`].
///
/// No shell constructs one of these yet (see the module docs) — this is the
/// standalone, host-testable recorder tasks 08/09/10 wire in.
#[derive(Debug)]
pub struct FrameStats {
    enabled: bool,
    /// Raw-per-frame-export mode (Phase 9.C, see [`raw_enabled`]) — always
    /// `false` when `enabled` is `false` (the two-dial contract
    /// [`Self::with_capacity_enabled_and_raw`] enforces).
    raw: bool,
    /// Reused, cleared-and-rewritten each call so [`Self::record`]'s raw
    /// line never grows the allocation once its capacity settles —
    /// formatting only, no per-frame allocation growth (see
    /// `docs/CODE_STANDARDS.md`'s Instrumentation conventions).
    raw_buf: String,
    ring_capacity: usize,
    ring: VecDeque<FramePasses>,
    total_frames: u64,
    skipped_frames: u64,
    over_60hz: u64,
    over_120hz: u64,
    /// Accumulated frame total time since the last `emit_log` (or since
    /// construction) — how [`Self::should_emit`] rate-limits without
    /// needing its own clock (see [`EMIT_INTERVAL`]'s docs).
    since_last_emit: Duration,
}

impl FrameStats {
    /// A recorder honoring the process-wide [`enabled`]/[`raw_enabled`]
    /// switches — what every shell constructs.
    pub fn new() -> Self {
        let is_enabled = enabled();
        Self::with_capacity_enabled_and_raw(RING_CAPACITY, is_enabled, is_enabled && raw_enabled())
    }

    /// Test/advanced seam: construct with an explicit enabled flag,
    /// bypassing [`enabled`]'s cache. Every shell should prefer [`Self::new`];
    /// this exists so tests can exercise both the enabled and disabled paths
    /// deterministically in the same process (`enabled()`'s `OnceLock` can
    /// only ever resolve once per process). Raw-export mode is left off; use
    /// [`Self::with_capacity_enabled_and_raw`] to exercise it.
    pub fn new_enabled(is_enabled: bool) -> Self {
        Self::with_capacity_enabled(RING_CAPACITY, is_enabled)
    }

    /// Test seam: a smaller ring capacity, so eviction behavior is
    /// exercisable without pushing [`RING_CAPACITY`] frames. Raw-export mode
    /// is left off; use [`Self::with_capacity_enabled_and_raw`] to exercise
    /// it.
    pub fn with_capacity_enabled(capacity: usize, is_enabled: bool) -> Self {
        Self::with_capacity_enabled_and_raw(capacity, is_enabled, false)
    }

    /// Test/advanced seam: construct with explicit enabled and raw-export
    /// flags, bypassing both [`enabled`]'s and [`raw_enabled`]'s caches (see
    /// [`Self::new_enabled`]'s docs for why a test needs to bypass the
    /// cache). `is_raw` only takes effect when `is_enabled` is also `true` —
    /// the same two-dial contract [`Self::new`] applies to the real
    /// `FRUST_TRACE`/`FRUST_TRACE_RAW` switches.
    pub fn with_capacity_enabled_and_raw(capacity: usize, is_enabled: bool, is_raw: bool) -> Self {
        let raw = is_enabled && is_raw;
        Self {
            enabled: is_enabled,
            raw,
            // Disabled: never reserve — nothing will ever be formatted into
            // it, mirroring the ring buffer's own no-reserve-when-disabled
            // reasoning below.
            raw_buf: if raw {
                String::with_capacity(160)
            } else {
                String::new()
            },
            ring_capacity: capacity,
            // Disabled: never reserve — nothing will ever be pushed, and
            // "allocates nothing after init" (acceptance criterion 2) holds
            // trivially for the whole recorder's lifetime, not just after
            // construction.
            ring: if is_enabled {
                VecDeque::with_capacity(capacity)
            } else {
                VecDeque::new()
            },
            total_frames: 0,
            skipped_frames: 0,
            over_60hz: 0,
            over_120hz: 0,
            since_last_emit: Duration::ZERO,
        }
    }

    /// Record one frame's pass durations. A cheap no-op (no allocation, no
    /// clock read — the caller already measured `passes`) when disabled.
    /// When raw-export mode is on (see [`raw_enabled`]), additionally
    /// formats and logs one `frust-perf raw` line for this frame — a skipped
    /// frame (`passes.skipped`) still gets a line (all-zero pass durations,
    /// `skipped=1`) so a harness can compute honest frame pacing across the
    /// mobile frame gate.
    pub fn record(&mut self, passes: FramePasses) {
        if !self.enabled {
            return;
        }

        self.total_frames += 1;
        if passes.skipped {
            self.skipped_frames += 1;
        }

        let total = passes.total();
        if total > BUDGET_60HZ {
            self.over_60hz += 1;
        }
        if total > BUDGET_120HZ {
            self.over_120hz += 1;
        }
        self.since_last_emit += total;

        if self.raw {
            format_raw_frame_line(&mut self.raw_buf, self.total_frames, &passes);
            log::info!("{}", self.raw_buf);
        }

        if self.ring.len() == self.ring_capacity {
            self.ring.pop_front();
        }
        self.ring.push_back(passes);
    }

    /// Total frames ever recorded (including skipped), regardless of the
    /// ring-buffer window.
    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    /// Aggregate the current ring-buffer window plus the lifetime running
    /// counters into a [`FrameSummary`].
    ///
    /// **Percentile semantics**: nearest-rank, computed over only the
    /// *non-skipped* frames currently in the ring buffer (a skipped frame's
    /// all-zero pass durations would otherwise silently pull percentiles
    /// down and misrepresent real frame cost) — for a sorted-ascending
    /// sample of `n` values, the `p`-th percentile is the value at 1-indexed
    /// rank `ceil(p * n / 100)`, computed with integer ceiling division
    /// (`(p * n).div_ceil(100)`) and clamped to `[1, n]`, so no floating-point
    /// rounding is involved. An empty (or all-skipped) window reports
    /// `Duration::ZERO` for every percentile field.
    pub fn summary(&self) -> FrameSummary {
        let active: Vec<&FramePasses> = self.ring.iter().filter(|p| !p.skipped).collect();

        let mut totals: Vec<Duration> = active.iter().map(|p| p.total()).collect();
        let mut rebuilds: Vec<Duration> = active.iter().map(|p| p.rebuild).collect();
        let mut layouts: Vec<Duration> = active.iter().map(|p| p.layout).collect();
        let mut paints: Vec<Duration> = active.iter().map(|p| p.paint).collect();
        let mut encodes: Vec<Duration> = active.iter().map(|p| p.encode_present).collect();
        totals.sort_unstable();
        rebuilds.sort_unstable();
        layouts.sort_unstable();
        paints.sort_unstable();
        encodes.sort_unstable();

        FrameSummary {
            frame_count: self.ring.len(),
            total_p50: nearest_rank_percentile(&totals, 50),
            total_p95: nearest_rank_percentile(&totals, 95),
            total_p99: nearest_rank_percentile(&totals, 99),
            rebuild_p95: nearest_rank_percentile(&rebuilds, 95),
            layout_p95: nearest_rank_percentile(&layouts, 95),
            paint_p95: nearest_rank_percentile(&paints, 95),
            encode_present_p95: nearest_rank_percentile(&encodes, 95),
            over_60hz_budget: self.over_60hz,
            over_120hz_budget: self.over_120hz,
            skipped_frames: self.skipped_frames,
        }
    }

    /// Whether at least [`EMIT_INTERVAL`] of frame time has accumulated
    /// since the last [`Self::emit_log`] (or construction) — the "one line
    /// every ~2s of frames" rate limit. Always `false` when disabled.
    pub fn should_emit(&self) -> bool {
        self.enabled && self.since_last_emit >= EMIT_INTERVAL
    }

    /// Emit one structured `frust-perf frame ...` line via `log::info!`
    /// and reset the [`Self::should_emit`] accumulator. A no-op when
    /// disabled. A shell calls this only when [`Self::should_emit`] is
    /// `true` (it does not check it itself, so a test can force an emission
    /// regardless of accumulated time).
    pub fn emit_log(&mut self) {
        if !self.enabled {
            return;
        }
        let s = self.summary();
        log::info!(
            "frust-perf frame n={} total_p50_ms={} total_p95_ms={} total_p99_ms={} \
             rebuild_p95_ms={} layout_p95_ms={} paint_p95_ms={} encode_present_p95_ms={} \
             over_60hz={} over_120hz={} skipped={} total_frames={}",
            s.frame_count,
            s.total_p50.as_millis(),
            s.total_p95.as_millis(),
            s.total_p99.as_millis(),
            s.rebuild_p95.as_millis(),
            s.layout_p95.as_millis(),
            s.paint_p95.as_millis(),
            s.encode_present_p95.as_millis(),
            s.over_60hz_budget,
            s.over_120hz_budget,
            s.skipped_frames,
            self.total_frames,
        );
        self.since_last_emit = Duration::ZERO;
    }
}

impl Default for FrameStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Nearest-rank percentile over an ascending-sorted sample (see
/// [`FrameStats::summary`]'s docs for the exact formula). `p` is a whole
/// percentage (`50`/`95`/`99`); `sorted` must already be ascending.
fn nearest_rank_percentile(sorted: &[Duration], p: u32) -> Duration {
    let n = sorted.len() as u32;
    if n == 0 {
        return Duration::ZERO;
    }
    let rank = (p * n).div_ceil(100);
    let rank = rank.clamp(1, n);
    sorted[(rank - 1) as usize]
}

// ---------------------------------------------------------------------
// Raw per-frame export + scenario markers (Phase 9.C)
// ---------------------------------------------------------------------

/// Log-line prefix for [`FrameStats::record`]'s raw per-frame export line —
/// parallels the `frust-perf frame`/`frust-perf startup` prefixes
/// [`FrameStats::emit_log`]/[`StartupSpans::emit_log`] already use.
const RAW_FRAME_PREFIX: &str = "frust-perf raw";

/// Formats one `frust-perf raw` line into `buf` (cleared first) for frame
/// index `n` (1-indexed — [`FrameStats::record`] passes its running
/// `total_frames` counter, post-increment) and `passes`. Kept separate from
/// `record`'s logging call so the line shape is directly unit-testable
/// without a log-capture harness, and so the caller can reuse one
/// growth-free buffer across every frame instead of formatting a fresh
/// `String` per call (see `docs/CODE_STANDARDS.md`'s Instrumentation
/// conventions — formatting only, no allocation growth on the hot path).
/// Field order: `n`, `total_us`, `rebuild_us`, `layout_us`, `paint_us`,
/// `encode_present_us`, `skipped` (`0`/`1`) — microsecond resolution so a
/// sub-millisecond pass still shows nonzero.
fn format_raw_frame_line(buf: &mut String, n: u64, passes: &FramePasses) {
    use std::fmt::Write as _;
    buf.clear();
    let _ = write!(
        buf,
        "{RAW_FRAME_PREFIX} n={n} total_us={} rebuild_us={} layout_us={} paint_us={} \
         encode_present_us={} skipped={}",
        passes.total().as_micros(),
        passes.rebuild.as_micros(),
        passes.layout.as_micros(),
        passes.paint.as_micros(),
        passes.encode_present.as_micros(),
        u8::from(passes.skipped),
    );
}

/// Which edge of a benchmark scenario window [`mark_scenario_start`]/
/// [`mark_scenario_end`] stamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerEdge {
    Start,
    End,
}

impl MarkerEdge {
    const fn prefix(self) -> &'static str {
        match self {
            MarkerEdge::Start => "bench-scenario-start",
            MarkerEdge::End => "bench-scenario-end",
        }
    }
}

/// Formats one scenario-marker line — separated from the `mark_scenario_*`
/// functions' logging call for the same directly-unit-testable reason
/// [`format_raw_frame_line`] is.
fn format_scenario_marker(edge: MarkerEdge, name: &str) -> String {
    format!("{} {name}", edge.prefix())
}

/// Stamps a `bench-scenario-start <name>` marker into the same raw-export
/// stream [`FrameStats::record`]'s per-frame lines land in, so an external
/// benchmark harness can slice the per-frame series into named scenarios
/// without needing to hold a [`FrameStats`] handle itself (a marker is a
/// scenario-boundary event, not a per-frame one, so it is a free function
/// rather than a method). A no-op unless both [`enabled`] and
/// [`raw_enabled`] are `true` — the same two-dial gating
/// [`FrameStats::new`]'s raw path uses.
pub fn mark_scenario_start(name: &str) {
    if enabled() && raw_enabled() {
        log::info!("{}", format_scenario_marker(MarkerEdge::Start, name));
    }
}

/// Stamps a `bench-scenario-end <name>` marker — see
/// [`mark_scenario_start`]'s docs (gating and rationale are identical).
pub fn mark_scenario_end(name: &str) {
    if enabled() && raw_enabled() {
        log::info!("{}", format_scenario_marker(MarkerEdge::End, name));
    }
}

// ---------------------------------------------------------------------
// StartupSpans
// ---------------------------------------------------------------------

/// Startup-span name: native library load (process/JNI load, or the C-ABI
/// equivalent on iOS).
pub const SPAN_NATIVE_LIB_LOAD: &str = "native_lib_load";
/// Startup-span name: entry into the shell's init function (`nativeInit` /
/// `frust_init` / the desktop app-construction entry point).
pub const SPAN_INIT_ENTRY: &str = "init_entry";
/// Startup-span name: the wgpu adapter is acquired.
pub const SPAN_ADAPTER_READY: &str = "adapter_ready";
/// Startup-span name: the wgpu logical device is acquired.
pub const SPAN_DEVICE_READY: &str = "device_ready";
/// Startup-span name: the vello renderer (and surface) are ready to
/// present.
pub const SPAN_RENDERER_READY: &str = "renderer_ready";
/// Startup-span name: the app's first `rebuild` pass has completed.
pub const SPAN_FIRST_REBUILD_DONE: &str = "first_rebuild_done";
/// Startup-span name: the app's first frame has been presented to the
/// surface.
pub const SPAN_FIRST_FRAME_PRESENTED: &str = "first_frame_presented";

/// A monotonic clock reading, injectable so [`StartupSpans`]'s deltas are
/// deterministically testable without a real clock. Only differences
/// between successive readings are meaningful — the absolute value has no
/// defined epoch.
pub trait Clock {
    fn now(&mut self) -> Duration;
}

/// Any `FnMut() -> Duration` closure is a [`Clock`] — the lighter-weight
/// option for a one-off test fake (see the module docs' "injectable clock"
/// note).
impl<F: FnMut() -> Duration> Clock for F {
    fn now(&mut self) -> Duration {
        self()
    }
}

/// The production [`Clock`]: wraps [`std::time::Instant`], monotonic for
/// the lifetime of the process.
#[derive(Debug)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&mut self) -> Duration {
        self.start.elapsed()
    }
}

/// Named monotonic timestamps from a `begin()` epoch (spec §14 phase 7.A) —
/// a shell records one named span at each startup milestone (see the
/// `SPAN_*` consts), then calls [`Self::emit_log`] once for a single
/// summary line.
pub struct StartupSpans<C: Clock = SystemClock> {
    enabled: bool,
    clock: C,
    begin: Duration,
    spans: Vec<(&'static str, Duration)>,
}

impl StartupSpans<SystemClock> {
    /// Begin a span recorder honoring the process-wide [`enabled`] switch,
    /// using the real system clock — what every shell constructs.
    pub fn begin() -> Self {
        Self::begin_with_enabled(SystemClock::new(), enabled())
    }
}

impl<C: Clock> StartupSpans<C> {
    /// Test/advanced seam: begin with an explicit clock and enabled flag,
    /// bypassing [`enabled`]'s cache (see [`FrameStats::new_enabled`]'s docs
    /// for why).
    pub fn begin_with_enabled(mut clock: C, is_enabled: bool) -> Self {
        let begin = clock.now();
        Self {
            enabled: is_enabled,
            clock,
            begin,
            spans: Vec::new(),
        }
    }

    /// Record `name` at the current clock reading, as a delta from
    /// [`Self::begin`]'s epoch. A no-op (no clock read, no allocation) when
    /// disabled.
    pub fn record(&mut self, name: &'static str) {
        if !self.enabled {
            return;
        }
        let now = self.clock.now();
        self.spans.push((name, now.saturating_sub(self.begin)));
    }

    /// The recorded `(name, delta)` pairs in insertion order.
    pub fn spans(&self) -> &[(&'static str, Duration)] {
        &self.spans
    }

    /// Emit one `frust-perf startup ...` line via `log::info!` containing
    /// every recorded span's name and millisecond delta, in insertion
    /// order. A no-op when disabled or when nothing has been recorded.
    pub fn emit_log(&self) {
        if !self.enabled || self.spans.is_empty() {
            return;
        }
        let mut line = String::from("frust-perf startup");
        for (name, delta) in &self.spans {
            line.push_str(&format!(" {name}={}ms", delta.as_millis()));
        }
        log::info!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // trace_switch (pure, directly testable — enabled()'s OnceLock cache
    // is deliberately NOT re-tested here, see enabled()'s docs)
    // ---------------------------------------------------------------

    #[test]
    fn trace_switch_off_when_neither_set() {
        assert!(!trace_switch(None, None));
    }

    #[test]
    fn trace_switch_on_when_compile_time_set_non_zero() {
        assert!(trace_switch(Some("1"), None));
    }

    #[test]
    fn trace_switch_on_when_runtime_set_non_zero() {
        assert!(trace_switch(None, Some("1")));
    }

    #[test]
    fn trace_switch_off_when_either_is_literal_zero_and_other_unset() {
        assert!(!trace_switch(Some("0"), None));
        assert!(!trace_switch(None, Some("0")));
    }

    #[test]
    fn trace_switch_on_when_either_source_wins() {
        // Compile-time "0" (effectively off) but runtime "1": still on.
        assert!(trace_switch(Some("0"), Some("1")));
        assert!(trace_switch(Some("1"), Some("0")));
    }

    // ---------------------------------------------------------------
    // nearest_rank_percentile
    // ---------------------------------------------------------------

    #[test]
    fn percentile_known_distribution_1_to_100ms() {
        // A sorted sample of 1ms..=100ms (n = 100): nearest-rank with
        // ceil(p*n/100) 1-indexed rank means p50 -> rank 50 -> value 50ms;
        // p95 -> rank 95 -> value 95ms; p99 -> rank 99 -> value 99ms.
        let sorted: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        assert_eq!(
            nearest_rank_percentile(&sorted, 50),
            Duration::from_millis(50)
        );
        assert_eq!(
            nearest_rank_percentile(&sorted, 95),
            Duration::from_millis(95)
        );
        assert_eq!(
            nearest_rank_percentile(&sorted, 99),
            Duration::from_millis(99)
        );
    }

    #[test]
    fn percentile_small_sample_rounds_up_rank() {
        // n = 4: rank(50) = ceil(200/100) = 2 -> index 1 -> value 2ms.
        // rank(95) = ceil(380/100) = 4 -> index 3 -> value 4ms.
        let sorted: Vec<Duration> = (1..=4).map(Duration::from_millis).collect();
        assert_eq!(
            nearest_rank_percentile(&sorted, 50),
            Duration::from_millis(2)
        );
        assert_eq!(
            nearest_rank_percentile(&sorted, 95),
            Duration::from_millis(4)
        );
    }

    #[test]
    fn percentile_single_value_returns_it_for_every_percentile() {
        let sorted = [Duration::from_millis(42)];
        assert_eq!(
            nearest_rank_percentile(&sorted, 50),
            Duration::from_millis(42)
        );
        assert_eq!(
            nearest_rank_percentile(&sorted, 99),
            Duration::from_millis(42)
        );
    }

    #[test]
    fn percentile_empty_sample_is_zero() {
        assert_eq!(nearest_rank_percentile(&[], 50), Duration::ZERO);
    }

    // ---------------------------------------------------------------
    // FrameStats
    // ---------------------------------------------------------------

    fn passes(rebuild_ms: u64, layout_ms: u64, paint_ms: u64, encode_ms: u64) -> FramePasses {
        FramePasses {
            rebuild: Duration::from_millis(rebuild_ms),
            layout: Duration::from_millis(layout_ms),
            paint: Duration::from_millis(paint_ms),
            encode_present: Duration::from_millis(encode_ms),
            skipped: false,
        }
    }

    #[test]
    fn disabled_recorder_records_nothing_and_never_allocates() {
        let mut stats = FrameStats::new_enabled(false);
        for _ in 0..500 {
            stats.record(passes(20, 5, 5, 2));
        }
        assert_eq!(stats.total_frames(), 0);
        assert_eq!(
            stats.ring.capacity(),
            0,
            "disabled recorder must never reserve ring capacity"
        );
        let s = stats.summary();
        assert_eq!(s.frame_count, 0);
        assert_eq!(s.total_p50, Duration::ZERO);
        assert!(!stats.should_emit());
    }

    #[test]
    fn over_budget_counters_are_exact() {
        let mut stats = FrameStats::new_enabled(true);
        // Under both budgets: 5ms total.
        stats.record(passes(2, 1, 1, 1));
        // Over 8.3ms but under 16.6ms: 10ms total.
        stats.record(passes(4, 2, 2, 2));
        // Over both: 20ms total.
        stats.record(passes(10, 5, 3, 2));

        let s = stats.summary();
        assert_eq!(
            s.over_120hz_budget, 2,
            "10ms and 20ms frames exceed the 8.3ms budget"
        );
        assert_eq!(
            s.over_60hz_budget, 1,
            "only the 20ms frame exceeds the 16.6ms budget"
        );
    }

    #[test]
    fn skipped_frames_counted_but_excluded_from_percentiles() {
        let mut stats = FrameStats::new_enabled(true);
        stats.record(passes(10, 2, 2, 2)); // 16ms, non-skipped
        stats.record(FramePasses {
            skipped: true,
            ..Default::default()
        }); // 0ms, skipped
        stats.record(passes(10, 2, 2, 2)); // 16ms, non-skipped

        let s = stats.summary();
        assert_eq!(s.skipped_frames, 1);
        assert_eq!(stats.total_frames(), 3);
        // Percentile window only has the two 16ms non-skipped frames — a
        // skipped frame's 0ms would otherwise pull this down.
        assert_eq!(s.frame_count, 3, "ring buffer holds all recorded frames...");
        assert_eq!(
            s.total_p50,
            Duration::from_millis(16),
            "...but percentiles exclude the skipped one"
        );
    }

    #[test]
    fn ring_buffer_evicts_oldest_beyond_capacity() {
        let mut stats = FrameStats::with_capacity_enabled(3, true);
        stats.record(passes(1, 0, 0, 0));
        stats.record(passes(2, 0, 0, 0));
        stats.record(passes(3, 0, 0, 0));
        stats.record(passes(4, 0, 0, 0)); // evicts the 1ms frame

        let s = stats.summary();
        assert_eq!(s.frame_count, 3);
        // Sample is now {2, 3, 4}ms: p50 (rank ceil(150/100)=2) -> 3ms.
        assert_eq!(s.total_p50, Duration::from_millis(3));
        // Running total_frames is unaffected by eviction.
        assert_eq!(stats.total_frames(), 4);
    }

    #[test]
    fn should_emit_rate_limits_on_accumulated_frame_time() {
        let mut stats = FrameStats::new_enabled(true);
        assert!(!stats.should_emit());
        // 100 frames * 16ms = 1600ms < the 2s interval.
        for _ in 0..100 {
            stats.record(passes(10, 3, 2, 1));
        }
        assert!(!stats.should_emit());
        // 25 more frames pushes accumulated time past 2000ms.
        for _ in 0..25 {
            stats.record(passes(10, 3, 2, 1));
        }
        assert!(stats.should_emit());

        stats.emit_log();
        assert!(!stats.should_emit(), "emit_log resets the accumulator");
    }

    // ---------------------------------------------------------------
    // Raw per-frame export + scenario markers (Phase 9.C)
    // ---------------------------------------------------------------

    /// A raw per-frame line's parsed fields — this module's own round-trip
    /// check that [`format_raw_frame_line`]'s shape is exactly what a
    /// `key=value`-splitting harness would expect; not part of the crate's
    /// public API (the real harness is a separate process parsing
    /// `stdout`/`logcat` text, not a Rust consumer of this module).
    struct ParsedRawFrameLine {
        n: u64,
        total_us: u128,
        rebuild_us: u128,
        layout_us: u128,
        paint_us: u128,
        encode_present_us: u128,
        skipped: bool,
    }

    fn parse_raw_frame_line(line: &str) -> Option<ParsedRawFrameLine> {
        let rest = line.strip_prefix(RAW_FRAME_PREFIX)?.trim_start();
        let mut n = None;
        let mut total_us = None;
        let mut rebuild_us = None;
        let mut layout_us = None;
        let mut paint_us = None;
        let mut encode_present_us = None;
        let mut skipped = None;
        for field in rest.split_whitespace() {
            let (key, value) = field.split_once('=')?;
            match key {
                "n" => n = value.parse().ok(),
                "total_us" => total_us = value.parse().ok(),
                "rebuild_us" => rebuild_us = value.parse().ok(),
                "layout_us" => layout_us = value.parse().ok(),
                "paint_us" => paint_us = value.parse().ok(),
                "encode_present_us" => encode_present_us = value.parse().ok(),
                "skipped" => skipped = value.parse::<u8>().ok().map(|v| v != 0),
                _ => {}
            }
        }
        Some(ParsedRawFrameLine {
            n: n?,
            total_us: total_us?,
            rebuild_us: rebuild_us?,
            layout_us: layout_us?,
            paint_us: paint_us?,
            encode_present_us: encode_present_us?,
            skipped: skipped?,
        })
    }

    #[test]
    fn raw_frame_line_format_round_trips() {
        let mut buf = String::new();
        let p = FramePasses {
            rebuild: Duration::from_micros(1234),
            layout: Duration::from_micros(200),
            paint: Duration::from_micros(300),
            encode_present: Duration::from_micros(50),
            skipped: false,
        };
        format_raw_frame_line(&mut buf, 42, &p);
        assert!(buf.starts_with(RAW_FRAME_PREFIX));

        let parsed = parse_raw_frame_line(&buf).expect("line must parse");
        assert_eq!(parsed.n, 42);
        assert_eq!(parsed.total_us, p.total().as_micros());
        assert_eq!(parsed.rebuild_us, 1234);
        assert_eq!(parsed.layout_us, 200);
        assert_eq!(parsed.paint_us, 300);
        assert_eq!(parsed.encode_present_us, 50);
        assert!(!parsed.skipped);
    }

    #[test]
    fn raw_frame_line_represents_skipped_flag() {
        let mut buf = String::new();
        let p = FramePasses {
            skipped: true,
            ..Default::default()
        };
        format_raw_frame_line(&mut buf, 7, &p);

        let parsed = parse_raw_frame_line(&buf).expect("line must parse");
        assert!(parsed.skipped, "skipped frame must still be represented");
        assert_eq!(parsed.total_us, 0);
        assert_eq!(parsed.n, 7);
    }

    #[test]
    fn raw_enabled_record_formats_one_line_per_frame() {
        let mut stats = FrameStats::with_capacity_enabled_and_raw(4, true, true);
        stats.record(passes(10, 2, 2, 2));
        let parsed = parse_raw_frame_line(&stats.raw_buf).expect("line must parse");
        assert_eq!(parsed.n, 1);

        stats.record(passes(5, 1, 1, 1));
        let parsed = parse_raw_frame_line(&stats.raw_buf).expect("line must parse");
        assert_eq!(parsed.n, 2, "frame index advances per recorded frame");
    }

    #[test]
    fn raw_disabled_record_never_touches_raw_line_buffer() {
        let mut stats = FrameStats::with_capacity_enabled_and_raw(4, true, false);
        for _ in 0..5 {
            stats.record(passes(1, 1, 1, 1));
        }
        assert!(
            stats.raw_buf.is_empty(),
            "raw-export off must never format into the raw line buffer"
        );
        assert_eq!(stats.total_frames(), 5, "non-raw recording still happens");
    }

    #[test]
    fn raw_requires_enabled_too() {
        // enabled=false + is_raw=true: the whole recorder (including raw)
        // stays off — enabled() gates raw_enabled(), not the other way
        // around.
        let mut stats = FrameStats::with_capacity_enabled_and_raw(4, false, true);
        stats.record(passes(10, 2, 2, 2));
        assert_eq!(stats.total_frames(), 0, "disabled recorder still no-ops");
        assert!(stats.raw_buf.is_empty());
    }

    #[test]
    fn scenario_marker_format_start_and_end() {
        assert_eq!(
            format_scenario_marker(MarkerEdge::Start, "cold_start"),
            "bench-scenario-start cold_start"
        );
        assert_eq!(
            format_scenario_marker(MarkerEdge::End, "cold_start"),
            "bench-scenario-end cold_start"
        );
    }

    #[test]
    fn mark_scenario_functions_do_not_panic_when_disabled() {
        // Process env has neither FRUST_TRACE nor FRUST_TRACE_RAW set in a
        // normal test run, so these are no-ops; the assertion here is just
        // that calling them is safe (no capture harness to check the log
        // line against — see format_scenario_marker's direct test above).
        mark_scenario_start("smoke");
        mark_scenario_end("smoke");
    }

    #[test]
    fn disabled_should_emit_and_emit_log_are_no_ops() {
        let mut stats = FrameStats::new_enabled(false);
        assert!(!stats.should_emit());
        stats.emit_log(); // must not panic; nothing to assert on directly
        assert_eq!(stats.total_frames(), 0);
    }

    // ---------------------------------------------------------------
    // StartupSpans
    // ---------------------------------------------------------------

    /// A deterministic fake clock advancing by a fixed step each call.
    struct FakeClock {
        elapsed: Duration,
        step: Duration,
    }

    impl Clock for FakeClock {
        fn now(&mut self) -> Duration {
            let now = self.elapsed;
            self.elapsed += self.step;
            now
        }
    }

    #[test]
    fn spans_recorded_in_order_with_monotonic_nonnegative_deltas() {
        let clock = FakeClock {
            elapsed: Duration::ZERO,
            step: Duration::from_millis(10),
        };
        let mut spans = StartupSpans::begin_with_enabled(clock, true);
        spans.record(SPAN_INIT_ENTRY);
        spans.record(SPAN_ADAPTER_READY);
        spans.record(SPAN_DEVICE_READY);
        spans.record(SPAN_RENDERER_READY);

        let recorded = spans.spans();
        assert_eq!(recorded.len(), 4);
        assert_eq!(recorded[0].0, SPAN_INIT_ENTRY);
        assert_eq!(recorded[3].0, SPAN_RENDERER_READY);

        let mut last = Duration::ZERO;
        for (_, delta) in recorded {
            assert!(*delta >= last, "deltas must be non-decreasing");
            assert!(*delta >= Duration::ZERO);
            last = *delta;
        }
        // begin() consumed the first fake-clock reading (0ms), so the first
        // recorded span is 10ms, the second 20ms, etc.
        assert_eq!(recorded[0].1, Duration::from_millis(10));
        assert_eq!(recorded[1].1, Duration::from_millis(20));
        assert_eq!(recorded[2].1, Duration::from_millis(30));
        assert_eq!(recorded[3].1, Duration::from_millis(40));
    }

    #[test]
    fn disabled_spans_record_nothing() {
        let clock = FakeClock {
            elapsed: Duration::ZERO,
            step: Duration::from_millis(10),
        };
        let mut spans = StartupSpans::begin_with_enabled(clock, false);
        spans.record(SPAN_INIT_ENTRY);
        assert!(spans.spans().is_empty());
        spans.emit_log(); // no-op, must not panic
    }

    #[test]
    fn closure_clock_satisfies_clock_trait() {
        let mut n = 0u64;
        let clock = move || {
            n += 10;
            Duration::from_millis(n)
        };
        let mut spans = StartupSpans::begin_with_enabled(clock, true);
        spans.record(SPAN_FIRST_REBUILD_DONE);
        spans.record(SPAN_FIRST_FRAME_PRESENTED);
        assert_eq!(spans.spans().len(), 2);
        assert!(spans.spans()[1].1 >= spans.spans()[0].1);
    }
}
