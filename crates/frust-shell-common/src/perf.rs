//! Frame-timing and startup-span perf instrumentation shared by every shell.
//!
//! # What lives here
//!
//! - [`FrameStats`] — a per-shell recorder of one frame's pass durations
//!   (rebuild/layout/paint/encode/acquire/submit, plus a `skipped` marker the
//!   mobile dirty-gate sets), aggregated into a ring buffer
//!   plus running totals; [`FrameStats::summary`] reports p50/p95/p99 total
//!   frame time, per-pass p95, and frames-over-budget counts against the
//!   16.6ms/8.3ms (60Hz/120Hz) targets.
//! - [`StartupSpans`] — named monotonic timestamps from a shell's `begin()`
//!   epoch (native-lib load, init entry, adapter/device/renderer ready,
//!   first rebuild done, first frame presented — see the `SPAN_*` consts),
//!   summarized into one log line.
//! - [`enabled`] — the process-wide on/off switch every recording API is a
//!   no-op behind (see its own docs).
//! - [`raw_enabled`] — a second dial that, alongside
//!   [`enabled`], makes [`FrameStats::record`] additionally emit one
//!   `frust-perf raw` line per recorded frame (instead of only the
//!   rate-limited ~2s `frust-perf frame` summary [`FrameStats::emit_log`]
//!   already produces).
//! - [`mark_scenario_start`]/[`mark_scenario_end`] — the benchmark
//!   scenario-window edges a harness slices that per-frame series by. They
//!   ride [`enabled`] alone (not the raw dial), and they are *queued* rather
//!   than logged: [`FrameStats::record`] emits each one stamped with the
//!   number of the frame that actually carried it, so a window survives the
//!   render-thread split's UI/render interleaving. See
//!   [`mark_scenario_start`] for the route and the half-open window rule.
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
//! # Wiring
//!
//! This module ships the recorder + switch; the Android, iOS, and desktop
//! shells each construct and feed a [`FrameStats`]/[`StartupSpans`] of
//! their own.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Mutex;
#[cfg(feature = "perf-trace")]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Ring-buffer capacity for [`FrameStats`] — roughly 2 seconds of frames at
/// 60Hz, enough for a stable rolling percentile without unbounded growth.
pub const RING_CAPACITY: usize = 120;

/// The 60Hz frame budget (1000/60 ms), truncated to whole microseconds.
pub const BUDGET_60HZ: Duration = Duration::from_micros(16_667);

/// The 120Hz frame budget (1000/120 ms), truncated to whole microseconds.
pub const BUDGET_120HZ: Duration = Duration::from_micros(8_333);

/// Minimum span of recorded frame time between two [`FrameStats::emit_log`]
/// calls that [`FrameStats::should_emit`] requires — one line every ~2s of
/// frames, measured in accumulated frame time rather than
/// wall-clock time so it needs no clock of its own.
const EMIT_INTERVAL: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------
// On/off switch
// ---------------------------------------------------------------------
//
// The whole of this module is *runtime*-switchable via [`enabled`], but that
// runtime dial only exists when the crate's `perf-trace` cargo feature is on.
// Without the feature every `frust-perf`/`bench-scenario` string literal and
// its `log::info!` emission is `#[cfg]`-compiled out entirely (release-lean
// builds), and [`enabled`]/[`raw_enabled`] collapse to inlinable `false`
// constants so downstream `if enabled()` branches constant-fold away — the CLI
// turns the feature on for debug/profile builds and omits it for release (the
// Flutter-mode-parity split, see the release-lean plan). The public perf API
// (types, constructors, `record`/`summary`/`emit_log`/`mark_scenario_*`)
// compiles identically in both configurations; only the string-bearing
// emission internals are gated, so no shell call site changes.
//
// Sink decision (feeds the release-lean log-level ceiling): `frust-perf`
// lines keep flowing through the `log` facade (`log::info!`), NOT a bypassing
// `eprintln!`/platform sink. Rationale — this crate is deliberately
// platform-free (no `android_logger`/NDK), so it cannot replicate each
// platform's real channel (logcat on Android, the desktop/iOS stderr logger),
// and moving Android's lines off logcat would break the benchmark harness's
// log parsing; keeping `log::info!` guarantees byte-identical output on every
// platform. The consequence this MUST honor: perf is stripped from release
// by THIS FEATURE (off ⇒ code+strings gone), never by the log level. So
// `release_max_level_warn` must be applied to RELEASE ONLY (e.g. a
// CLI-toggled `log/release_max_level_warn` cargo feature enabled for
// `--release` and omitted for `--profile`), never as an always-on manifest
// feature: `release_max_level_*` keys off `debug_assertions`, which is OFF in
// the profile profile too, so an always-on ceiling would silence profile-mode
// perf lines. Release perf lines don't exist to strip
// (feature off), so a release-only ceiling only removes stray non-perf
// info/debug while profile keeps its `frust-perf` output intact.

/// The process-wide perf-instrumentation switch, cached after the first
/// call. `true` when either:
///
/// - the compile-time `FRUST_TRACE` define is set to a non-`"0"` value
///   (the `--define`/`--profile` path — `--profile` builds
///   pass `FRUST_TRACE=1` by default; `option_env!` reads whatever a
///   build script/cargo-ndk env var set at compile time), or
/// - the runtime `FRUST_TRACE` process environment variable is set to a
///   non-`"0"` value (desktop dev: `FRUST_TRACE=1 cargo run -p ...`) — the
///   compile-time-or-runtime convention every shipping `FRUST_*` knob
///   follows.
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
#[cfg(feature = "perf-trace")]
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| trace_switch(option_env!("FRUST_TRACE"), runtime_trace_var().as_deref()))
}

/// The `perf-trace` feature is off: perf instrumentation is compiled out of
/// this build, so the switch is a compile-time `false` constant — no
/// `OnceLock`, no environment read. `#[inline]` so every downstream
/// `if enabled()` branch constant-folds to nothing, taking the `frust-perf`
/// emission (and its strings) with it via LLVM dead-code elimination. Build
/// with `--features perf-trace` (what a debug/profile build does) to restore
/// the runtime `FRUST_TRACE` switch documented above.
#[cfg(not(feature = "perf-trace"))]
#[inline]
pub fn enabled() -> bool {
    false
}

/// Reads the runtime `FRUST_TRACE` env var, isolated into its own
/// function so [`enabled`]'s caching is the only thing that touches the
/// process environment — [`trace_switch`] itself stays a pure, directly
/// unit-testable function. Compiled only under `perf-trace` (the only caller,
/// [`enabled`]'s feature-on arm, is too).
#[cfg(feature = "perf-trace")]
fn runtime_trace_var() -> Option<String> {
    std::env::var("FRUST_TRACE").ok()
}

/// The pure decision [`enabled`] caches: non-empty and not the literal
/// string `"0"` counts as "set" for either the compile-time or runtime
/// value; either source being set is enough. Compiled only under `perf-trace`.
#[cfg(feature = "perf-trace")]
fn trace_switch(compile_time: Option<&str>, runtime: Option<&str>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if v != "0")
    }
    is_set_non_zero(compile_time) || is_set_non_zero(runtime)
}

/// The process-wide raw-per-frame-export switch, cached after
/// the first call — parsed exactly the same compile-time-or-runtime way as
/// [`enabled`] (via the same [`trace_switch`] decision), but reading
/// `FRUST_TRACE_RAW` instead of `FRUST_TRACE`. This is a **second dial**,
/// not a replacement: `FRUST_TRACE_RAW` being set implies nothing on its own
/// — [`FrameStats::record`]'s raw per-frame line only fires when [`enabled`]
/// is *also* true (a benchmark harness sets both `FRUST_TRACE=1` and
/// `FRUST_TRACE_RAW=1`; see [`FrameStats::new`]).
#[cfg(feature = "perf-trace")]
pub fn raw_enabled() -> bool {
    static RAW_ENABLED: OnceLock<bool> = OnceLock::new();
    *RAW_ENABLED.get_or_init(|| {
        trace_switch(
            option_env!("FRUST_TRACE_RAW"),
            runtime_trace_raw_var().as_deref(),
        )
    })
}

/// The `perf-trace` feature is off: the raw-per-frame dial is compiled out
/// alongside [`enabled`], so it is a compile-time `false` constant (see
/// [`enabled`]'s feature-off arm for the DCE rationale).
#[cfg(not(feature = "perf-trace"))]
#[inline]
pub fn raw_enabled() -> bool {
    false
}

/// Reads the runtime `FRUST_TRACE_RAW` env var, isolated for the same reason
/// [`runtime_trace_var`] is. Compiled only under `perf-trace`.
#[cfg(feature = "perf-trace")]
fn runtime_trace_raw_var() -> Option<String> {
    std::env::var("FRUST_TRACE_RAW").ok()
}

// ---------------------------------------------------------------------
// FrameStats
// ---------------------------------------------------------------------

/// One frame's measured pass durations, as recorded by a shell's frame
/// callback (`AndroidAppHandle::frame` / `frust_render_frame` /
/// desktop's `RedrawRequested` handler — see `docs/ARCHITECTURE.md`'s Frame
/// pipeline). `skipped` is set by the mobile dirty-gate for a
/// frame whose passes never ran; its pass durations are `Duration::ZERO` in
/// that case and it is excluded from the percentile computation in
/// [`FrameStats::summary`] (see that method's docs) while still counting
/// toward `total_frames`/`skipped_frames`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FramePasses {
    pub rebuild: Duration,
    pub layout: Duration,
    pub paint: Duration,
    /// GPU/CPU encode cost — the [`SurfaceRenderer::encode`] span
    /// (`vello` encode + `render_to_texture`, or the CPU rasterize+upload),
    /// with no swapchain-acquire wait folded in. Split out from the old
    /// combined `encode_present` so encode work and the vsync/
    /// present wait are separately attributable — the number the
    /// render-thread-split GO/NO-GO decision is made on.
    ///
    /// [`SurfaceRenderer::encode`]: https://docs.rs/frust-render
    pub encode: Duration,
    /// Swapchain-**acquire** cost — the [`SurfaceRenderer::acquire`] span,
    /// dominated by the blocking vsync wait (per the surface's present mode).
    /// Split out from the old combined `present` so the blocking
    /// vsync wait is attributable separately from the blit/submit work below —
    /// the S5 GPU-saturation-vs-blit-cost question. `acquire + submit` equals the
    /// old v2 `present` span, so cross-baseline math is unchanged.
    ///
    /// [`SurfaceRenderer::acquire`]: https://docs.rs/frust-render
    pub acquire: Duration,
    /// Blit + queue-submit + present cost — the [`SurfaceRenderer::submit`]
    /// span, the GPU/driver work after the swapchain texture is acquired. See
    /// [`Self::acquire`] for the split rationale.
    ///
    /// [`SurfaceRenderer::submit`]: https://docs.rs/frust-render
    pub submit: Duration,
    pub skipped: bool,
    /// This frame's real GPU time per pass, when the renderer measured it —
    /// see [`GpuPasses`]. `None` is the ordinary case and is what the raw line
    /// reports as `gpu_q=0`.
    ///
    /// Deliberately outside [`Self::total`]: GPU passes run *concurrently*
    /// with the CPU spans above, so adding them would double-count the frame.
    /// It is also not carried by [`RenderSpans`] — a shell attaches it with
    /// [`Self::with_gpu`] after folding its two measured halves together,
    /// which keeps the split's own reassembly about the spans it measured.
    pub gpu: Option<GpuPasses>,
}

/// One frame's real GPU time, split by the spans the frust-owned render engine
/// names — the counterpart of the CPU spans in [`FramePasses`], measured on the
/// GPU's own clock rather than inferred from CPU wall time around a submit.
///
/// Produced only in a build whose device asked for GPU timestamps; every
/// other frame carries `None` and reports `gpu_q=0`.
/// A span may legitimately read zero — a frame with no off-screen layer does no
/// composite work — so a zero is a measurement, not a gap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuPasses {
    /// Work recorded ahead of the frame's own passes (the glyph-atlas replay).
    pub prepass: Duration,
    /// The frame's own surface passes: clear, opaque strips, alpha strips, and
    /// the hole punch.
    pub main: Duration,
    /// Off-screen layer pages and filter passes.
    pub composite: Duration,
    /// The present-side conversion or blit into the swapchain.
    pub blit: Duration,
}

impl GpuPasses {
    /// The sum of the four spans — the frame's *attributed* GPU pass time.
    ///
    /// Queue and driver gaps between passes belong to no pass and are not
    /// folded in, so this is a lower bound on the frame's whole GPU cost
    /// rather than an estimate of it.
    pub fn total(&self) -> Duration {
        self.prepass
            .saturating_add(self.main)
            .saturating_add(self.composite)
            .saturating_add(self.blit)
    }
}

impl FramePasses {
    /// The sum of all six pass durations — the frame's total wall time.
    ///
    /// The GPU spans in [`Self::gpu`] are deliberately excluded; see that
    /// field.
    pub fn total(&self) -> Duration {
        self.rebuild + self.layout + self.paint + self.encode + self.acquire + self.submit
    }

    /// Attaches this frame's measured GPU pass times.
    ///
    /// A builder rather than a field on [`RenderSpans`]: every shell already
    /// builds its `RenderSpans` by struct literal, and the GPU reading is
    /// optional per frame and per tier, so `FramePasses::from_split(ui,
    /// render).with_gpu(gpu)` adds it without touching the spans a shell
    /// measured itself.
    #[must_use]
    pub fn with_gpu(mut self, gpu: GpuPasses) -> Self {
        self.gpu = Some(gpu);
        self
    }

    /// Recombine a render-thread-split frame's two half-measurements into the
    /// one [`FramePasses`] the single emitter records.
    ///
    /// In the render-thread split the UI thread measures
    /// `rebuild`/`layout`/`paint` ([`UiSpans`]) while the render thread measures
    /// `encode`/`acquire`/`submit` ([`RenderSpans`]); the UI half rides across
    /// the scene-handoff channel
    /// ([`SceneFrame`](crate::render_split::SceneFrame)) so the render thread —
    /// the **single emitter** — can fold both halves into one frame record.
    /// This is a pure reassembly of the *existing* six fields: it changes no
    /// wire format (the raw v3 line [`format_raw_frame_line`] emits is byte-for-
    /// byte identical to a single-thread frame's), it only moves *where* each
    /// span is measured. The `skipped` flag comes from the UI half (the frame
    /// gate is UI-side — see [`crate::frame_gate`]).
    pub fn from_split(ui: UiSpans, render: RenderSpans) -> Self {
        Self {
            rebuild: ui.rebuild,
            layout: ui.layout,
            paint: ui.paint,
            encode: render.encode,
            acquire: render.acquire,
            submit: render.submit,
            skipped: ui.skipped,
            // Not a measured span either half carries: the render thread reads
            // it off the renderer after the fact and attaches it with
            // [`FramePasses::with_gpu`].
            gpu: None,
        }
    }
}

/// The UI-thread half of a render-thread-split frame's timing: the
/// `rebuild`/`layout`/`paint` spans measured on the UI thread,
/// plus the frame gate's `skipped` verdict (the gate stays UI-side — see
/// [`crate::frame_gate`]). Rides the scene-handoff channel across to the
/// render thread, which folds it together with its own [`RenderSpans`] via
/// [`FramePasses::from_split`] and records the result through the one emitter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiSpans {
    pub rebuild: Duration,
    pub layout: Duration,
    pub paint: Duration,
    /// The frame gate's skip verdict (a skipped frame carries all-zero spans);
    /// preserved through [`FramePasses::from_split`] into the recorded frame.
    pub skipped: bool,
}

/// The render-thread half of a render-thread-split frame's timing: the
/// `encode`/`acquire`/`submit` spans measured on the render thread,
/// folded together with the UI thread's [`UiSpans`] via
/// [`FramePasses::from_split`]. See [`FramePasses::encode`]/[`FramePasses::acquire`]/
/// [`FramePasses::submit`] for each span's exact boundary (the v3 attribution
/// this split preserves unchanged).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderSpans {
    pub encode: Duration,
    pub acquire: Duration,
    pub submit: Duration,
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
    /// p95 of the [`FramePasses::encode`] span (GPU/CPU encode, no vsync wait).
    pub encode_p95: Duration,
    /// p95 of the [`FramePasses::acquire`] span (swapchain-acquire/vsync wait).
    pub acquire_p95: Duration,
    /// p95 of the [`FramePasses::submit`] span (blit + queue-submit + present).
    pub submit_p95: Duration,
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

/// Per-shell frame-timing recorder: a ring buffer of
/// the last [`RING_CAPACITY`] frames' [`FramePasses`] plus lifetime running
/// counters, aggregated on demand by [`FrameStats::summary`] and rate-limit
/// logged by [`FrameStats::should_emit`]/[`FrameStats::emit_log`].
///
/// The Android, iOS, and desktop shells each construct one of these — a
/// standalone, host-testable recorder.
#[derive(Debug)]
pub struct FrameStats {
    enabled: bool,
    /// Raw-per-frame-export mode (see [`raw_enabled`]) — always
    /// `false` when `enabled` is `false` (the two-dial contract
    /// [`Self::with_capacity_enabled_and_raw`] enforces). Only *read* by the
    /// `perf-trace`-gated raw emission path in [`Self::record`], so it is dead
    /// in a release-lean (feature-off) build (still written by constructors).
    #[cfg_attr(not(feature = "perf-trace"), allow(dead_code))]
    raw: bool,
    /// Reused, cleared-and-rewritten each call so [`Self::record`]'s raw
    /// line never grows the allocation once its capacity settles —
    /// formatting only, no per-frame allocation growth (see
    /// `docs/CODE_STANDARDS.md`'s Instrumentation conventions). Only ever
    /// *read* by the `perf-trace`-gated raw emission path, so it is dead in a
    /// release-lean (feature-off) build — the field stays (constructors still
    /// size it) but the lint is silenced there.
    #[cfg_attr(not(feature = "perf-trace"), allow(dead_code))]
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
    /// Test-only mirror of every scenario-marker line [`Self::record`] has
    /// logged, in emission order — the seam the marker tests assert against
    /// so they never have to install a global `log` sink (which would make
    /// them order-dependent on every other test in the process). Absent
    /// from any non-test build, so it costs a shipped binary nothing, and
    /// absent from a feature-off test build too — with the emission itself
    /// compiled out there, a buffer of emitted lines would be a field
    /// nothing ever writes.
    #[cfg(all(test, feature = "perf-trace"))]
    marker_log: Vec<String>,
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
            // Sized for the longest line the formatter writes — the v4 shape
            // with every `gpu_*_us` field present — so even a GPU-timed frame
            // never grows the allocation after the first one.
            raw_buf: if raw {
                String::with_capacity(288)
            } else {
                String::new()
            },
            ring_capacity: capacity,
            // Disabled: never reserve — nothing will ever be pushed, so
            // "allocates nothing after init" holds
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
            #[cfg(all(test, feature = "perf-trace"))]
            marker_log: Vec::new(),
        }
    }

    /// Record one frame's pass durations. A cheap no-op (no allocation, no
    /// clock read — the caller already measured `passes`) when disabled.
    /// When raw-export mode is on (see [`raw_enabled`]), additionally
    /// formats and logs one `frust-perf raw` line for this frame — a skipped
    /// frame (`passes.skipped`) still gets a line (all-zero pass durations,
    /// `skipped=1`) so a harness can compute honest frame pacing across the
    /// mobile frame gate.
    ///
    /// This is also the single point where a benchmark scenario marker is
    /// emitted: every marker this frame carries (see [`stage_markers`] and
    /// [`take_pending_markers`]) is logged, in the order it was raised,
    /// stamped with **this** frame's `n` and immediately ahead of this
    /// frame's own `frust-perf raw` line — so a marker's frame number names
    /// the recorded frame that actually carried it, not a guess made on
    /// whichever thread raised it (see [`mark_scenario_start`]).
    pub fn record(&mut self, passes: FramePasses) {
        // Devtools frame stats fan out BEFORE the perf switch below: a devtools
        // client subscribing to them is its own opt-in, independent of
        // `FRUST_TRACE`. This is also the one site every shell's frame pipeline
        // already funnels through — desktop, Android and iOS, inline and
        // render-thread-split alike — so the publish has no per-shell copy to
        // drift. Non-blocking, and one relaxed atomic load with no service
        // running (see `crate::devtools::publish_frame`).
        #[cfg(feature = "devtools")]
        crate::devtools::publish_frame(&passes);

        if !self.enabled {
            return;
        }

        self.total_frames += 1;

        // Ahead of everything else this frame emits (the `frust-perf raw`
        // line below): a window's `start n=k` must precede frame k's own raw
        // line in the log, and the harness's half-open `[start_n, end_n)`
        // rule reads the numbers, not the positions.
        #[cfg(feature = "perf-trace")]
        self.emit_scenario_markers();

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

        #[cfg(feature = "perf-trace")]
        if self.raw {
            format_raw_frame_line(&mut self.raw_buf, self.total_frames, &passes);
            log::info!("{}", self.raw_buf);
        }

        if self.ring.len() == self.ring_capacity {
            self.ring.pop_front();
        }
        self.ring.push_back(passes);
    }

    /// Emit every scenario marker riding this frame, stamped with the frame
    /// number [`Self::record`] just assigned it.
    ///
    /// Two sources, and which of them applies is decided by whether this
    /// process ever handed a frame across [`crate::render_split`]:
    ///
    /// - [`STAGED_MARKERS`] — this (render) thread's markers, put there by
    ///   [`stage_markers`] when the render channel handed over the very frame
    ///   now being recorded. Always drained.
    /// - [`PENDING_MARKERS`] — the process-wide raise queue, drained here
    ///   **only** while [`MARKERS_VIA_CHANNEL`] is still `false`, i.e. the
    ///   inline (no render thread) executor, where the thread that raises a
    ///   marker is the same one that records the frame. Once a channel has
    ///   carried a frame, that queue belongs to [`RenderSender::send_scene`]
    ///   alone and draining it here would steal a later frame's markers.
    ///
    /// Both drains are `is_empty` fast paths: a frame carrying no marker
    /// allocates nothing.
    ///
    /// [`RenderSender::send_scene`]: crate::render_split::RenderSender::send_scene
    #[cfg(feature = "perf-trace")]
    fn emit_scenario_markers(&mut self) {
        let mut markers = take_staged_markers();
        if !MARKERS_VIA_CHANNEL.load(Ordering::Relaxed) {
            let pending = take_pending_markers();
            if !pending.is_empty() {
                markers.extend(pending);
            }
        }
        for marker in markers {
            let line = format_scenario_marker(marker.edge, &marker.name, self.total_frames);
            log::info!("{line}");
            #[cfg(test)]
            self.marker_log.push(line);
        }
    }

    /// Test-only view of the scenario-marker lines [`Self::record`] has
    /// emitted so far, in order — see [`Self::marker_log`]'s docs.
    /// `pub(crate)` so `render_split`'s channel tests can assert the lines a
    /// frame carried across the handoff.
    #[cfg(all(test, feature = "perf-trace"))]
    pub(crate) fn marker_log(&self) -> &[String] {
        &self.marker_log
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
        let mut encodes: Vec<Duration> = active.iter().map(|p| p.encode).collect();
        let mut acquires: Vec<Duration> = active.iter().map(|p| p.acquire).collect();
        let mut submits: Vec<Duration> = active.iter().map(|p| p.submit).collect();
        totals.sort_unstable();
        rebuilds.sort_unstable();
        layouts.sort_unstable();
        paints.sort_unstable();
        encodes.sort_unstable();
        acquires.sort_unstable();
        submits.sort_unstable();

        FrameSummary {
            frame_count: self.ring.len(),
            total_p50: nearest_rank_percentile(&totals, 50),
            total_p95: nearest_rank_percentile(&totals, 95),
            total_p99: nearest_rank_percentile(&totals, 99),
            rebuild_p95: nearest_rank_percentile(&rebuilds, 95),
            layout_p95: nearest_rank_percentile(&layouts, 95),
            paint_p95: nearest_rank_percentile(&paints, 95),
            encode_p95: nearest_rank_percentile(&encodes, 95),
            acquire_p95: nearest_rank_percentile(&acquires, 95),
            submit_p95: nearest_rank_percentile(&submits, 95),
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
        // The `frust-perf frame` line (and the strings it carries) exists only
        // under `perf-trace`; the accumulator reset below is plain bookkeeping
        // and stays in every build so [`Self::should_emit`]'s rate limit
        // behaves identically whether or not the feature is compiled in.
        #[cfg(feature = "perf-trace")]
        {
            let s = self.summary();
            log::info!(
                "frust-perf frame n={} total_p50_ms={} total_p95_ms={} total_p99_ms={} \
                 rebuild_p95_ms={} layout_p95_ms={} paint_p95_ms={} encode_p95_ms={} \
                 acquire_p95_ms={} submit_p95_ms={} \
                 over_60hz={} over_120hz={} skipped={} total_frames={}",
                s.frame_count,
                s.total_p50.as_millis(),
                s.total_p95.as_millis(),
                s.total_p99.as_millis(),
                s.rebuild_p95.as_millis(),
                s.layout_p95.as_millis(),
                s.paint_p95.as_millis(),
                s.encode_p95.as_millis(),
                s.acquire_p95.as_millis(),
                s.submit_p95.as_millis(),
                s.over_60hz_budget,
                s.over_120hz_budget,
                s.skipped_frames,
                self.total_frames,
            );
        }
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
// Raw per-frame export + scenario markers
// ---------------------------------------------------------------------

/// Log-line prefix for [`FrameStats::record`]'s raw per-frame export line —
/// parallels the `frust-perf frame`/`frust-perf startup` prefixes
/// [`FrameStats::emit_log`]/[`StartupSpans::emit_log`] already use. A
/// `frust-perf` string literal, so it compiles only under `perf-trace`
/// (release-lean builds carry no `frust-perf` bytes).
#[cfg(feature = "perf-trace")]
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
/// `encode_us`, `acquire_us`, `submit_us`, `skipped` (`0`/`1`), `gpu_q`
/// (`0`/`1`), then — only when `gpu_q=1` — `gpu_total_us`, `gpu_prepass_us`,
/// `gpu_main_us`, `gpu_composite_us`, `gpu_blit_us`. Microsecond resolution so
/// a sub-millisecond pass still shows nonzero.
///
/// **Format v4 (2026-09-01):** real GPU time per pass ([`GpuPasses`]) is
/// appended after `skipped`, additively — every v3 field keeps its name,
/// meaning and position, so a v3 parser reads a v4 line unchanged and a v4
/// parser reads a v3 line as `gpu_q=0`. `gpu_q` states whether this frame
/// carries a GPU reading at all; the five `gpu_*_us` fields are **omitted
/// entirely** when it is `0`, rather than written as zeros, so a series with no
/// GPU timing never produces a column of zeros that reads like a measurement.
///
/// **Format v3 (2026-07-22):** the single `present_us` field of v2
/// was split into separate `acquire_us` + `submit_us` fields (no combined field
/// is kept); `acquire_us + submit_us` equals the old v2 `present_us` for
/// cross-baseline math. **Format v2 (2026-07-21):** the single
/// `encode_present_us` field of v1 was split into `encode_us` + `present_us`.
/// Any harness parsing this line must handle the current field set; see
/// `benchmarks/PROTOCOL.md`'s format-change note. Compiled only under
/// `perf-trace` (it bears the `frust-perf raw` prefix).
#[cfg(feature = "perf-trace")]
fn format_raw_frame_line(buf: &mut String, n: u64, passes: &FramePasses) {
    use std::fmt::Write as _;
    buf.clear();
    let _ = write!(
        buf,
        "{RAW_FRAME_PREFIX} n={n} total_us={} rebuild_us={} layout_us={} paint_us={} \
         encode_us={} acquire_us={} submit_us={} skipped={} gpu_q={}",
        passes.total().as_micros(),
        passes.rebuild.as_micros(),
        passes.layout.as_micros(),
        passes.paint.as_micros(),
        passes.encode.as_micros(),
        passes.acquire.as_micros(),
        passes.submit.as_micros(),
        u8::from(passes.skipped),
        u8::from(passes.gpu.is_some()),
    );
    if let Some(gpu) = passes.gpu {
        let _ = write!(
            buf,
            " gpu_total_us={} gpu_prepass_us={} gpu_main_us={} gpu_composite_us={} \
             gpu_blit_us={}",
            gpu.total().as_micros(),
            gpu.prepass.as_micros(),
            gpu.main.as_micros(),
            gpu.composite.as_micros(),
            gpu.blit.as_micros(),
        );
    }
}

/// Which edge of a benchmark scenario window [`mark_scenario_start`]/
/// [`mark_scenario_end`] raises. Compiled unconditionally — a
/// [`ScenarioMarker`] carries one, and that type crosses into
/// [`crate::render_split`]'s inbox in every build; only the
/// `bench-scenario-*` string literals it maps to live behind `perf-trace`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerEdge {
    Start,
    End,
}

#[cfg(feature = "perf-trace")]
impl MarkerEdge {
    const fn prefix(self) -> &'static str {
        match self {
            MarkerEdge::Start => "bench-scenario-start",
            MarkerEdge::End => "bench-scenario-end",
        }
    }
}

/// One raised-but-not-yet-emitted scenario-window edge: what
/// [`mark_scenario_start`]/[`mark_scenario_end`] queue and
/// [`FrameStats::record`] eventually logs, stamped with the frame that
/// carried it.
///
/// Deliberately opaque — a caller can only move one of these along the
/// route ([`take_pending_markers`] → [`stage_markers`]); it can neither read
/// the edge nor rewrite the name, so the emitted line's shape stays this
/// module's business alone. `Box<str>` rather than `String`: a marker name
/// is never appended to after it is raised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioMarker {
    /// Read only by the `perf-trace`-gated emission path
    /// ([`FrameStats::record`]), so it is dead in a release-lean
    /// (feature-off) build — the field stays (the route still carries it)
    /// but the lint is silenced there, the same way [`FrameStats::raw_buf`]
    /// handles it.
    #[cfg_attr(not(feature = "perf-trace"), allow(dead_code))]
    edge: MarkerEdge,
    /// The scenario name, read only by the same gated emission path — see
    /// [`Self::edge`].
    #[cfg_attr(not(feature = "perf-trace"), allow(dead_code))]
    name: Box<str>,
}

/// The process-wide queue every raised marker lands in first
/// ([`mark_scenario_start`]/[`mark_scenario_end`] push, nothing else does).
///
/// A marker is raised by benchmark scenario code (`s3_table.rs` and
/// friends) from whichever thread is building the frame the marker belongs
/// to; it must come out attached to the *recorded* frame that carried that
/// build's scene through the pipeline. This queue is the first leg of that
/// route. Its second leg depends on the executor:
///
/// - **render-thread split** — [`RenderSender::send_scene`] drains it into
///   the inbox alongside the scene it is handing over (and flips
///   [`MARKERS_VIA_CHANNEL`]), so the markers travel with that handoff.
/// - **inline** (no render thread) — nothing ever drains it there, so
///   [`FrameStats::record`] drains it directly, on the same thread that
///   raised it, for the frame it is recording right now.
///
/// A `Mutex` (not an atomic) because the payload is a list; contention is
/// nil in practice — one raiser thread, one drainer — and the lock is only
/// ever taken while perf is enabled. Lock order is **inbox → this**, never
/// the reverse: `render_split` takes this one while holding its inbox lock,
/// and [`FrameStats::record`] takes no inbox lock at all.
///
/// [`RenderSender::send_scene`]: crate::render_split::RenderSender::send_scene
static PENDING_MARKERS: Mutex<Vec<ScenarioMarker>> = Mutex::new(Vec::new());

/// Whether [`PENDING_MARKERS`] is worth locking — the lock-free fast path
/// [`take_pending_markers`] checks first.
///
/// [`RenderSender::send_scene`] calls that function on **every** frame,
/// including in a release-lean build where perf is compiled out entirely, so
/// the empty case must cost a relaxed load and nothing more: a marker-only
/// mutex acquired once per frame forever would be exactly the kind of
/// always-on cost the `perf-trace` gating exists to avoid.
///
/// Set under the lock after a push, cleared under the lock before a drain,
/// so a pusher waiting on the lock always ends up re-setting it. Relaxed is
/// enough — a missed flip on a foreign thread only defers that marker to the
/// next handoff, which is the documented best-effort behaviour for a marker
/// raised off the frame-producing thread anyway.
///
/// [`RenderSender::send_scene`]: crate::render_split::RenderSender::send_scene
static ANY_PENDING_MARKER: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The markers handed to *this* thread for the very next frame it
    /// records — the render thread's leg of the route (see
    /// [`PENDING_MARKERS`]). [`stage_markers`] appends,
    /// [`FrameStats::record`] drains. Thread-local rather than shared: the
    /// draining thread is by construction the one that took the batch out of
    /// the inbox and is about to render and record it, so no lock is needed
    /// and no other thread's frame can pick these up by accident.
    static STAGED_MARKERS: RefCell<Vec<ScenarioMarker>> = const { RefCell::new(Vec::new()) };
}

/// Set once the first frame crosses [`crate::render_split`] (see
/// [`markers_route_via_channel`]) and never cleared: from then on
/// [`PENDING_MARKERS`] belongs to the channel's handoff, and
/// [`FrameStats::record`] must not drain it itself — doing so would attach a
/// marker raised for a *future* frame to whatever frame the render thread
/// happens to be recording now, which is precisely the cross-thread guess
/// this whole route exists to remove.
///
/// One-way and process-wide because the choice is: a shell either runs the
/// render-thread split for its whole life or runs inline for its whole life
/// (`render_thread_enabled`), and a benchmark process has exactly one
/// enabled [`FrameStats`] (the single-emitter contract in
/// [`FramePasses::from_split`]'s docs). Relaxed ordering: it guards no other
/// memory, and the store happens on the UI thread before the same frame's
/// markers reach the render thread through the inbox mutex, which supplies
/// the ordering that actually matters.
static MARKERS_VIA_CHANNEL: AtomicBool = AtomicBool::new(false);

/// Declare that scenario markers now travel with the scene handoff rather
/// than being drained at [`FrameStats::record`] — called by
/// [`RenderSender::send_scene`] on every send (cheap, idempotent, relaxed;
/// there is no un-declaring it).
///
/// [`RenderSender::send_scene`]: crate::render_split::RenderSender::send_scene
pub fn markers_route_via_channel() {
    // Load first: after the first frame this is a clean read of a shared
    // line every frame instead of a store that dirties it.
    if !MARKERS_VIA_CHANNEL.load(Ordering::Relaxed) {
        MARKERS_VIA_CHANNEL.store(true, Ordering::Relaxed);
    }
}

/// Drain every marker raised since the last drain, oldest first — the
/// second leg of the route [`PENDING_MARKERS`] documents. Returns an empty
/// `Vec` (no allocation, and no lock — see [`ANY_PENDING_MARKER`]) when
/// nothing is pending, so a caller may call this once per frame
/// unconditionally.
pub fn take_pending_markers() -> Vec<ScenarioMarker> {
    if !ANY_PENDING_MARKER.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let mut pending = PENDING_MARKERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ANY_PENDING_MARKER.store(false, Ordering::Relaxed);
    if pending.is_empty() {
        return Vec::new();
    }
    std::mem::take(&mut *pending)
}

/// Hand markers to the calling thread's next recorded frame, appending to
/// whatever is already staged there (see [`STAGED_MARKERS`]). Called by the
/// render thread as it takes a scene out of the inbox; the frame it records
/// next is the one that scene belongs to.
///
/// **Accepted best-effort edge**: a batch taken but then *not* rendered (the
/// render loop's [`RenderPhase`] cannot render it — a paused or
/// surface-less phase) records no frame, so its markers stay staged and
/// attach to the next frame this thread does record. A scenario window
/// bracketed across such a gap is therefore attributed to the first frame
/// that really rendered after it, which is the closest honest answer
/// available without inventing a frame that was never drawn.
///
/// [`RenderPhase`]: crate::render_split::RenderPhase
pub fn stage_markers(markers: Vec<ScenarioMarker>) {
    if markers.is_empty() {
        return;
    }
    STAGED_MARKERS.with(|staged| staged.borrow_mut().extend(markers));
}

/// Take this thread's staged markers, leaving the slot empty. `is_empty`
/// fast path first: the overwhelmingly common frame carries no marker and
/// must not allocate. Only the `perf-trace` emission path drains this.
#[cfg(feature = "perf-trace")]
fn take_staged_markers() -> Vec<ScenarioMarker> {
    STAGED_MARKERS.with(|staged| {
        let mut staged = staged.borrow_mut();
        if staged.is_empty() {
            Vec::new()
        } else {
            std::mem::take(&mut *staged)
        }
    })
}

/// Test-only override of [`markers_enabled`]: [`enabled`] caches a process
/// environment read in a `OnceLock` and can never be flipped back, so a test
/// that needs a marker actually queued sets this instead of relaxing
/// production gating. Only ever touched while [`MARKER_TEST_LOCK`] is held —
/// see [`marker_test_guard`].
#[cfg(test)]
static MARKERS_FORCE_ENABLED: AtomicBool = AtomicBool::new(false);

/// Serializes every test that touches the process-wide marker route
/// ([`PENDING_MARKERS`], [`MARKERS_VIA_CHANNEL`], [`MARKERS_FORCE_ENABLED`]
/// and this thread's [`STAGED_MARKERS`]) — the crate's established
/// convention for a global the code under test owns (`theme_override`'s
/// `TEST_LOCK`). `cargo test` runs test functions on parallel threads, so a
/// participant that skips this lock can corrupt another's queue; take it via
/// [`marker_test_guard`], never by hand.
#[cfg(test)]
static MARKER_TEST_LOCK: Mutex<()> = Mutex::new(());

/// The RAII handle [`marker_test_guard`] returns: holds
/// [`MARKER_TEST_LOCK`] and restores the marker route to its pristine state
/// on drop, so one test's leftovers can never leak into the next.
#[cfg(test)]
pub(crate) struct MarkerTestGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for MarkerTestGuard {
    fn drop(&mut self) {
        MARKERS_FORCE_ENABLED.store(false, Ordering::Relaxed);
        reset_marker_route();
    }
}

/// Clear every piece of marker state this thread and this process can see.
#[cfg(test)]
fn reset_marker_route() {
    PENDING_MARKERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    ANY_PENDING_MARKER.store(false, Ordering::Relaxed);
    STAGED_MARKERS.with(|staged| staged.borrow_mut().clear());
    MARKERS_VIA_CHANNEL.store(false, Ordering::Relaxed);
}

/// Take exclusive use of the marker route for the duration of a test,
/// starting from a pristine queue and (when `force_enabled`) with markers
/// queueing as though `FRUST_TRACE` were set. `pub(crate)` because
/// `render_split`'s channel tests drive the same route from the other end.
///
/// A poisoned lock is adopted rather than propagated: one failing test must
/// not cascade into every later one.
#[cfg(test)]
pub(crate) fn marker_test_guard(force_enabled: bool) -> MarkerTestGuard {
    let lock = MARKER_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reset_marker_route();
    MARKERS_FORCE_ENABLED.store(force_enabled, Ordering::Relaxed);
    MarkerTestGuard { _lock: lock }
}

/// Whether a raised marker is queued at all. The one-dial gate
/// ([`enabled`], not [`enabled`]-and-[`raw_enabled`]): a marker is now
/// emitted by [`FrameStats::record`] itself, which runs whenever perf is on,
/// so tying markers to the raw-export dial would mean a scenario window that
/// exists in one capture and silently not in another. When perf is off this
/// folds to a `false` constant and every caller below it disappears.
#[inline]
fn markers_enabled() -> bool {
    #[cfg(test)]
    if MARKERS_FORCE_ENABLED.load(Ordering::Relaxed) {
        return true;
    }
    enabled()
}

/// Queue one marker onto [`PENDING_MARKERS`] — the single body behind both
/// [`mark_scenario_start`] and [`mark_scenario_end`], so the two edges can
/// never drift apart.
fn push_marker(edge: MarkerEdge, name: &str) {
    if !markers_enabled() {
        return;
    }
    let mut pending = PENDING_MARKERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    pending.push(ScenarioMarker {
        edge,
        name: name.into(),
    });
    ANY_PENDING_MARKER.store(true, Ordering::Relaxed);
}

/// Formats one scenario-marker line — separated from the emission call for
/// the same directly-unit-testable reason [`format_raw_frame_line`] is.
/// Compiled only under `perf-trace` (it emits the `bench-scenario-*`
/// prefixes).
///
/// **Shape (2026-09-06):** `<prefix> n=<frame> <name>`, where `frame` is the
/// 1-indexed counter of the frame that carried this marker through the
/// pipeline — the very same counter [`format_raw_frame_line`] writes as a
/// raw line's own `n=`, because both are stamped by the same
/// [`FrameStats::record`] call. There is no config toggle back to the older
/// name-only `<prefix> <name>` shape; a series captured before this change
/// is still name-only and a harness parsing it falls back to log-position
/// bracketing (`benchmarks/harness/stats.py`'s `slice_scenario`).
#[cfg(feature = "perf-trace")]
fn format_scenario_marker(edge: MarkerEdge, name: &str, frame: u64) -> String {
    format!("{} n={frame} {name}", edge.prefix())
}

/// Raise a `bench-scenario-start` marker for `name` — the opening edge of a
/// benchmark scenario window, so an external harness can slice the
/// per-frame `frust-perf raw` series into named scenarios without holding a
/// [`FrameStats`] handle itself (a marker is a scenario-boundary event, not
/// a per-frame one, hence a free function rather than a method).
///
/// # What the emitted `n` means
///
/// Raising a marker does **not** log it. The marker is queued, travels with
/// the frame the calling build hands off, and is logged by
/// [`FrameStats::record`] as `bench-scenario-start n=<frame> <name>` where
/// `<frame>` is the number of the frame that actually carried it through
/// the pipeline — immediately ahead of that frame's own `frust-perf raw`
/// line. Nothing here guesses a frame number across a thread boundary,
/// which is the whole point: on the render-thread split the UI thread that
/// raises a marker cannot know whether the render thread has recorded the
/// previous frame yet.
///
/// The window is **half-open**. `start` is raised in the build that also
/// applies the operation being measured, so `start n=k` names the window's
/// first frame; `end` is raised in the *next* build (the S3 convention —
/// `benchmarks/frust_bench/src/scenarios/s3_table.rs`), so `end n=k+1`
/// names the first frame *after* the window. A harness attributes the
/// frames with `start_n <= n < end_n` to the window — one frame, for the S3
/// shape (see `benchmarks/PROTOCOL.md` §7).
///
/// Two consequences worth knowing:
///
/// - Under the channel's depth-1 latest-wins slot, a build whose scene is
///   replaced before the render thread takes it never becomes a frame of
///   its own; its markers ride the frame that superseded it — the frame
///   that actually drew that build's result. If a window's `start` and
///   `end` both land on that one frame, the half-open window is empty,
///   which is the honest answer: the operation's own frame was dropped.
/// - A marker raised on a thread that hands no frame off (neither the UI
///   thread of a split executor nor the inline executor's own thread)
///   attaches to the next frame handed off after it — best effort, and the
///   only case where the attribution is approximate.
///
/// A no-op unless [`enabled`] is `true`; unlike the raw per-frame line it
/// does **not** additionally require [`raw_enabled`], so a scenario window
/// is present in every perf-enabled capture.
pub fn mark_scenario_start(name: &str) {
    push_marker(MarkerEdge::Start, name);
}

/// Raise a `bench-scenario-end` marker — the closing edge of the window
/// [`mark_scenario_start`] opened; see its docs, which cover the gating, the
/// emitted `n`, and the half-open `[start_n, end_n)` rule identically. In
/// the S3 convention this is raised in the build *after* the measured one,
/// so its `n` is one past the window's last frame.
pub fn mark_scenario_end(name: &str) {
    push_marker(MarkerEdge::End, name);
}

/// Emit one already-formatted benchmark trace line into the same
/// `log::info!` stream the per-frame `frust-perf raw` lines and the
/// `bench-scenario-*` markers land in.
///
/// This is the frust counterpart to the Flutter bench's `benchEmit`
/// (`benchmarks/flutter_bench/lib/bench/perf.dart`): a benchmark scenario that
/// records a per-operation measurement (e.g. S8's
/// `frust-perf plugin op=write type=bool n=0 us=12` per-op latency lines) hands
/// this an already-formatted single line, which the harness parses alongside
/// the frame series. Centralizing every bench line behind one gated sink keeps
/// per-op emission on the same two-dial switch the markers use — a no-op unless
/// both [`enabled`] and [`raw_enabled`] are `true`.
#[cfg_attr(not(feature = "perf-trace"), allow(unused_variables))]
pub fn bench_emit(line: &str) {
    #[cfg(feature = "perf-trace")]
    if enabled() && raw_enabled() {
        log::info!("{line}");
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
/// Startup-span name: a persisted GPU
/// pipeline cache blob was restored before surface creation — its *presence* in
/// the startup line is the warm-start (cache-**hit**) signal, its *absence* the
/// cold-start (cache-**miss**) one, so a slow first frame can be attributed to
/// shader-pipeline compilation vs a warm cache. Recorded only on a hit, right
/// before the surface (and thus the pipeline) is built.
pub const SPAN_PIPELINE_CACHE_RESTORED: &str = "pipeline_cache_restored";
/// Startup-span name: the app's first `rebuild` pass has completed.
pub const SPAN_FIRST_REBUILD_DONE: &str = "first_rebuild_done";
/// Startup-span name: the app's first
/// frame's GPU/CPU **encode** has completed — the boundary between the first
/// frame's paint/encode work and its swapchain-acquire (present) wait, so a
/// first-frame outlier (a 3646ms-class span) decomposes into encode vs present
/// exactly as the per-frame [`FramePasses`] split does.
pub const SPAN_FIRST_ENCODE_DONE: &str = "first_encode_done";
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

/// Named monotonic timestamps from a `begin()` epoch —
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
        // The `frust-perf startup` line is a gated string literal; a
        // release-lean (feature-off) build compiles the body away entirely
        // (and `enabled` is a `false` constant there anyway). Written as a
        // positive guard rather than an early return so the feature-off body
        // is simply empty, with no dangling `return`.
        if self.enabled && !self.spans.is_empty() {
            #[cfg(feature = "perf-trace")]
            {
                let mut line = String::from("frust-perf startup");
                for (name, delta) in &self.spans {
                    line.push_str(&format!(" {name}={}ms", delta.as_millis()));
                }
                log::info!("{line}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // trace_switch (pure, directly testable — enabled()'s OnceLock cache
    // is deliberately NOT re-tested here, see enabled()'s docs). The
    // `trace_switch` decision only exists under `perf-trace`, so these run
    // in the feature-on configuration only.
    // ---------------------------------------------------------------

    #[cfg(feature = "perf-trace")]
    #[test]
    fn trace_switch_off_when_neither_set() {
        assert!(!trace_switch(None, None));
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn trace_switch_on_when_compile_time_set_non_zero() {
        assert!(trace_switch(Some("1"), None));
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn trace_switch_on_when_runtime_set_non_zero() {
        assert!(trace_switch(None, Some("1")));
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn trace_switch_off_when_either_is_literal_zero_and_other_unset() {
        assert!(!trace_switch(Some("0"), None));
        assert!(!trace_switch(None, Some("0")));
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn trace_switch_on_when_either_source_wins() {
        // Compile-time "0" (effectively off) but runtime "1": still on.
        assert!(trace_switch(Some("0"), Some("1")));
        assert!(trace_switch(Some("1"), Some("0")));
    }

    // ---------------------------------------------------------------
    // Compile-out switch (perf-trace off): the runtime dial collapses to a
    // `false` constant and the whole public perf API stays callable + inert.
    // ---------------------------------------------------------------

    #[cfg(not(feature = "perf-trace"))]
    #[test]
    fn enabled_and_raw_enabled_are_const_false_without_feature() {
        assert!(!enabled(), "perf-trace off ⇒ enabled() is a false constant");
        assert!(
            !raw_enabled(),
            "perf-trace off ⇒ raw_enabled() is a false constant"
        );
    }

    #[cfg(not(feature = "perf-trace"))]
    #[test]
    fn disabled_build_public_api_is_callable_and_inert() {
        // Every public perf entry point still exists and is safe to call in a
        // release-lean build — it just records/emits nothing (no shell call
        // site changes between the two configurations).
        let mut stats = FrameStats::new();
        stats.record(passes(20, 5, 5, 2));
        assert_eq!(stats.total_frames(), 0, "feature-off new() is disabled");
        assert_eq!(stats.summary().frame_count, 0);
        assert!(!stats.should_emit());
        stats.emit_log(); // no-op, must not panic

        let mut spans = StartupSpans::begin();
        spans.record(SPAN_INIT_ENTRY);
        assert!(spans.spans().is_empty(), "feature-off begin() is disabled");
        spans.emit_log(); // no-op, must not panic

        // Free-function emitters are inert no-ops (no strings compiled in).
        mark_scenario_start("smoke");
        mark_scenario_end("smoke");
        bench_emit("smoke op=write");
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

    /// The `encode_ms` argument feeds the `encode` span; the `acquire`/`submit`
    /// spans are left zero so existing total-time assertions are unchanged by the
    /// v3 field split — only the per-span attribution moved.
    fn passes(rebuild_ms: u64, layout_ms: u64, paint_ms: u64, encode_ms: u64) -> FramePasses {
        FramePasses {
            rebuild: Duration::from_millis(rebuild_ms),
            layout: Duration::from_millis(layout_ms),
            paint: Duration::from_millis(paint_ms),
            encode: Duration::from_millis(encode_ms),
            acquire: Duration::ZERO,
            submit: Duration::ZERO,
            skipped: false,
            gpu: None,
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
    fn summary_attributes_encode_acquire_and_submit_spans_separately() {
        // The old combined present is now two spans (acquire +
        // submit) beside encode. A frame that spends 6ms encoding, 9ms on the
        // blocking acquire (vsync wait), and 3ms on the blit/submit must report
        // each p95 independently — not one conflated number.
        let mut stats = FrameStats::new_enabled(true);
        stats.record(FramePasses {
            rebuild: Duration::from_millis(2),
            layout: Duration::from_millis(1),
            paint: Duration::from_millis(1),
            encode: Duration::from_millis(6),
            acquire: Duration::from_millis(9),
            submit: Duration::from_millis(3),
            skipped: false,
            gpu: None,
        });
        let s = stats.summary();
        assert_eq!(
            s.encode_p95,
            Duration::from_millis(6),
            "encode span attributed"
        );
        assert_eq!(
            s.acquire_p95,
            Duration::from_millis(9),
            "acquire span attributed"
        );
        assert_eq!(
            s.submit_p95,
            Duration::from_millis(3),
            "submit span attributed"
        );
        // Total still sums every span (22ms here).
        assert_eq!(s.total_p95, Duration::from_millis(22));
    }

    #[test]
    fn from_split_recombines_the_two_half_frames_without_changing_the_record() {
        // The UI thread measures rebuild/layout/paint, the render
        // thread measures encode/acquire/submit. `from_split` folds them into
        // the exact same FramePasses a single-thread frame would have built —
        // the render-thread split moves *where* spans are measured, not the
        // recorded shape or wire format.
        let ui = UiSpans {
            rebuild: Duration::from_millis(2),
            layout: Duration::from_millis(1),
            paint: Duration::from_millis(1),
            skipped: false,
        };
        let render = RenderSpans {
            encode: Duration::from_millis(6),
            acquire: Duration::from_millis(9),
            submit: Duration::from_millis(3),
        };
        let split = FramePasses::from_split(ui, render);
        let whole = FramePasses {
            rebuild: Duration::from_millis(2),
            layout: Duration::from_millis(1),
            paint: Duration::from_millis(1),
            encode: Duration::from_millis(6),
            acquire: Duration::from_millis(9),
            submit: Duration::from_millis(3),
            skipped: false,
            gpu: None,
        };
        assert_eq!(split, whole, "split reassembly must equal the whole frame");
        assert_eq!(split.total(), Duration::from_millis(22));
    }

    /// The raw v3 wire line a split frame produces is byte-for-byte identical
    /// to the single-thread frame's — asserted separately because
    /// [`format_raw_frame_line`] is `perf-trace`-gated emission.
    #[cfg(feature = "perf-trace")]
    #[test]
    fn from_split_v3_wire_format_is_identical_to_single_thread() {
        let ui = UiSpans {
            rebuild: Duration::from_millis(2),
            layout: Duration::from_millis(1),
            paint: Duration::from_millis(1),
            skipped: false,
        };
        let render = RenderSpans {
            encode: Duration::from_millis(6),
            acquire: Duration::from_millis(9),
            submit: Duration::from_millis(3),
        };
        let split = FramePasses::from_split(ui, render);
        let whole = FramePasses {
            rebuild: Duration::from_millis(2),
            layout: Duration::from_millis(1),
            paint: Duration::from_millis(1),
            encode: Duration::from_millis(6),
            acquire: Duration::from_millis(9),
            submit: Duration::from_millis(3),
            skipped: false,
            gpu: None,
        };
        let mut split_line = String::new();
        let mut whole_line = String::new();
        format_raw_frame_line(&mut split_line, 1, &split);
        format_raw_frame_line(&mut whole_line, 1, &whole);
        assert_eq!(
            split_line, whole_line,
            "v3 wire format is unchanged by the split"
        );
    }

    #[test]
    fn from_split_preserves_the_ui_side_skipped_verdict() {
        // The frame gate is UI-side, so a skipped frame's verdict rides in on
        // the UiSpans half and must survive the fold.
        let ui = UiSpans {
            skipped: true,
            ..Default::default()
        };
        let split = FramePasses::from_split(ui, RenderSpans::default());
        assert!(split.skipped, "the UI-side skip verdict must be preserved");
        assert_eq!(split.total(), Duration::ZERO);
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

    #[test]
    fn a_reassembled_split_frame_carries_no_gpu_reading_until_one_is_attached() {
        let split = FramePasses::from_split(
            UiSpans {
                rebuild: Duration::from_millis(2),
                ..Default::default()
            },
            RenderSpans {
                encode: Duration::from_millis(6),
                acquire: Duration::from_millis(9),
                submit: Duration::from_millis(3),
            },
        );
        assert_eq!(split.gpu, None, "neither half measures GPU time");

        let gpu = GpuPasses {
            prepass: Duration::from_micros(10),
            main: Duration::from_micros(20),
            composite: Duration::from_micros(30),
            blit: Duration::from_micros(40),
        };
        let timed = split.with_gpu(gpu);
        assert_eq!(timed.gpu, Some(gpu));
        assert_eq!(timed.total(), split.total(), "GPU time is not frame time");
        assert_eq!(gpu.total(), Duration::from_micros(100));
        // Every other field is untouched by the attachment.
        assert_eq!(FramePasses { gpu: None, ..timed }, split);
    }

    #[test]
    fn an_all_zero_gpu_reading_is_still_a_reading() {
        // A frame that drew nothing off-screen genuinely spent no composite
        // time; the distinction between "measured zero" and "not measured" is
        // the `Option`, never a zero value.
        let timed = FramePasses::default().with_gpu(GpuPasses::default());
        assert_eq!(timed.gpu, Some(GpuPasses::default()));
        assert_eq!(timed.gpu.map(|gpu| gpu.total()), Some(Duration::ZERO));
        assert_eq!(FramePasses::default().gpu, None);
    }

    // ---------------------------------------------------------------
    // Raw per-frame export + scenario markers
    // ---------------------------------------------------------------

    /// A raw per-frame line's parsed fields — this module's own round-trip
    /// check that [`format_raw_frame_line`]'s shape is exactly what a
    /// `key=value`-splitting harness would expect; not part of the crate's
    /// public API (the real harness is a separate process parsing
    /// `stdout`/`logcat` text, not a Rust consumer of this module). Gated with
    /// the emission it exercises.
    #[cfg(feature = "perf-trace")]
    struct ParsedRawFrameLine {
        n: u64,
        total_us: u128,
        rebuild_us: u128,
        layout_us: u128,
        paint_us: u128,
        encode_us: u128,
        acquire_us: u128,
        submit_us: u128,
        skipped: bool,
        /// v4's `gpu_q` marker, plus every `gpu_*_us` field it gates, in the
        /// order the line wrote them — absent as a group whenever `gpu_q=0`.
        gpu_q: bool,
        gpu: Vec<(String, u128)>,
    }

    #[cfg(feature = "perf-trace")]
    fn parse_raw_frame_line(line: &str) -> Option<ParsedRawFrameLine> {
        let rest = line.strip_prefix(RAW_FRAME_PREFIX)?.trim_start();
        let mut n = None;
        let mut total_us = None;
        let mut rebuild_us = None;
        let mut layout_us = None;
        let mut paint_us = None;
        let mut encode_us = None;
        let mut acquire_us = None;
        let mut submit_us = None;
        let mut skipped = None;
        let mut gpu_q = None;
        let mut gpu = Vec::new();
        for field in rest.split_whitespace() {
            let (key, value) = field.split_once('=')?;
            match key {
                "n" => n = value.parse().ok(),
                "total_us" => total_us = value.parse().ok(),
                "rebuild_us" => rebuild_us = value.parse().ok(),
                "layout_us" => layout_us = value.parse().ok(),
                "paint_us" => paint_us = value.parse().ok(),
                "encode_us" => encode_us = value.parse().ok(),
                "acquire_us" => acquire_us = value.parse().ok(),
                "submit_us" => submit_us = value.parse().ok(),
                "skipped" => skipped = value.parse::<u8>().ok().map(|v| v != 0),
                "gpu_q" => gpu_q = value.parse::<u8>().ok().map(|v| v != 0),
                other if other.starts_with("gpu_") => {
                    gpu.push((other.to_string(), value.parse().ok()?));
                }
                _ => {}
            }
        }
        Some(ParsedRawFrameLine {
            n: n?,
            total_us: total_us?,
            rebuild_us: rebuild_us?,
            layout_us: layout_us?,
            paint_us: paint_us?,
            encode_us: encode_us?,
            acquire_us: acquire_us?,
            submit_us: submit_us?,
            skipped: skipped?,
            gpu_q: gpu_q?,
            gpu,
        })
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn raw_frame_line_format_round_trips() {
        let mut buf = String::new();
        let p = FramePasses {
            rebuild: Duration::from_micros(1234),
            layout: Duration::from_micros(200),
            paint: Duration::from_micros(300),
            encode: Duration::from_micros(50),
            acquire: Duration::from_micros(80),
            submit: Duration::from_micros(40),
            skipped: false,
            gpu: None,
        };
        format_raw_frame_line(&mut buf, 42, &p);
        assert!(buf.starts_with(RAW_FRAME_PREFIX));

        let parsed = parse_raw_frame_line(&buf).expect("line must parse");
        assert_eq!(parsed.n, 42);
        assert_eq!(parsed.total_us, p.total().as_micros());
        assert_eq!(parsed.rebuild_us, 1234);
        assert_eq!(parsed.layout_us, 200);
        assert_eq!(parsed.paint_us, 300);
        assert_eq!(parsed.encode_us, 50);
        assert_eq!(parsed.acquire_us, 80);
        assert_eq!(parsed.submit_us, 40);
        assert!(!parsed.skipped);
        assert!(!parsed.gpu_q, "a frame with no GPU reading reports gpu_q=0");
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn a_frame_without_a_gpu_reading_writes_gpu_q_zero_and_no_gpu_columns() {
        // The no-TIMESTAMP_QUERY shape: the marker says there is no reading,
        // and the five `gpu_*_us` fields are absent rather than written as
        // zeros — a column of zeros in a raw series reads like a measured
        // result, which is exactly what this must not produce.
        let mut buf = String::new();
        format_raw_frame_line(&mut buf, 3, &passes(10, 2, 2, 2));

        assert!(buf.contains(" gpu_q=0"));
        assert!(
            !buf.contains("gpu_total_us"),
            "no gpu_* column may be emitted without a reading: {buf}"
        );
        let parsed = parse_raw_frame_line(&buf).expect("line must parse");
        assert!(!parsed.gpu_q);
        assert!(parsed.gpu.is_empty());
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn a_gpu_timed_frame_appends_every_span_in_the_declared_order() {
        let gpu = GpuPasses {
            prepass: Duration::from_micros(120),
            main: Duration::from_micros(2400),
            composite: Duration::from_micros(650),
            blit: Duration::from_micros(75),
        };
        let mut buf = String::new();
        format_raw_frame_line(&mut buf, 9, &passes(4, 1, 1, 2).with_gpu(gpu));

        let parsed = parse_raw_frame_line(&buf).expect("line must parse");
        assert!(parsed.gpu_q);
        assert_eq!(
            parsed.gpu,
            vec![
                ("gpu_total_us".to_string(), 3245),
                ("gpu_prepass_us".to_string(), 120),
                ("gpu_main_us".to_string(), 2400),
                ("gpu_composite_us".to_string(), 650),
                ("gpu_blit_us".to_string(), 75),
            ],
            "field order is wire contract: {buf}"
        );
        assert_eq!(gpu.total(), Duration::from_micros(3245));
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn the_v3_prefix_of_a_v4_line_is_byte_identical() {
        // v4 is additive: appending the GPU fields must not perturb one byte
        // of what a v3 parser reads, so every series already captured stays
        // comparable against one captured after this change.
        let base = passes(4, 1, 1, 2);
        let mut v3_line = String::new();
        let mut v4_line = String::new();
        format_raw_frame_line(&mut v3_line, 11, &base);
        format_raw_frame_line(
            &mut v4_line,
            11,
            &base.with_gpu(GpuPasses {
                main: Duration::from_micros(1),
                ..GpuPasses::default()
            }),
        );

        let (v3_head, v3_marker) = v3_line
            .rsplit_once(" gpu_q=")
            .expect("every v4 line carries the marker");
        assert_eq!(v3_marker, "0");
        assert!(
            v4_line.starts_with(v3_head),
            "the v3 field set must be byte-identical:\n{v3_line}\n{v4_line}"
        );
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn a_gpu_timed_frames_total_excludes_the_gpu_spans() {
        // GPU passes run concurrently with the CPU spans, so folding them into
        // `total_us` would double-count the frame and break every percentile
        // computed off it.
        let base = passes(4, 1, 1, 2);
        let timed = base.with_gpu(GpuPasses {
            main: Duration::from_millis(50),
            ..GpuPasses::default()
        });
        assert_eq!(timed.total(), base.total());

        let mut buf = String::new();
        format_raw_frame_line(&mut buf, 1, &timed);
        let parsed = parse_raw_frame_line(&buf).expect("line must parse");
        assert_eq!(parsed.total_us, base.total().as_micros());
    }

    #[cfg(feature = "perf-trace")]
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

    #[cfg(feature = "perf-trace")]
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

    #[cfg(feature = "perf-trace")]
    #[test]
    fn scenario_marker_format_start_and_end() {
        assert_eq!(
            format_scenario_marker(MarkerEdge::Start, "cold_start", 5),
            "bench-scenario-start n=5 cold_start"
        );
        assert_eq!(
            format_scenario_marker(MarkerEdge::End, "cold_start", 6),
            "bench-scenario-end n=6 cold_start"
        );
    }

    #[test]
    fn markers_raised_while_perf_is_disabled_accumulate_nothing() {
        // Perf off (the guard leaves the force switch clear, and no test-run
        // process sets FRUST_TRACE): raising markers must be safe AND must
        // queue nothing at all — an unbounded queue nobody ever drains is
        // the one way this route could leak in a shipped build.
        let _guard = marker_test_guard(false);

        mark_scenario_start("smoke");
        mark_scenario_end("smoke");

        assert!(
            take_pending_markers().is_empty(),
            "a disabled marker must never reach the queue"
        );
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn inline_executor_drains_the_pending_queue_at_record_on_its_own_thread() {
        // The inline (no render thread) path: nothing ever calls
        // `markers_route_via_channel`, so `record` itself drains the queue —
        // on the very thread that raised the markers, for the frame it is
        // recording right now.
        let _guard = marker_test_guard(true);
        let mut stats = FrameStats::with_capacity_enabled_and_raw(4, true, true);

        mark_scenario_start("s3-create1k");
        stats.record(passes(10, 2, 2, 2));
        assert_eq!(
            stats.marker_log(),
            ["bench-scenario-start n=1 s3-create1k"],
            "the marker rides the frame being recorded when it was raised"
        );

        // The next build closes the window: S3 raises `end` one build later,
        // so it lands on frame 2 and the half-open window is exactly frame 1.
        mark_scenario_end("s3-create1k");
        stats.record(passes(10, 2, 2, 2));
        assert_eq!(
            stats.marker_log(),
            [
                "bench-scenario-start n=1 s3-create1k",
                "bench-scenario-end n=2 s3-create1k",
            ]
        );

        // A frame carrying no marker adds no line.
        stats.record(passes(10, 2, 2, 2));
        assert_eq!(stats.marker_log().len(), 2);
        assert_eq!(stats.total_frames(), 3);
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn markers_are_emitted_with_raw_export_off() {
        // The raw-export dial gates the per-frame line, not the markers: a
        // window must exist in every perf-enabled capture, not only in the
        // ones a harness asked for raw frames in.
        let _guard = marker_test_guard(true);
        let mut stats = FrameStats::with_capacity_enabled_and_raw(4, true, false);

        mark_scenario_start("s8-write");
        stats.record(passes(10, 2, 2, 2));

        assert_eq!(stats.marker_log(), ["bench-scenario-start n=1 s8-write"]);
        assert!(
            stats.raw_buf.is_empty(),
            "raw export stays off — only the marker line was emitted"
        );
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn a_disabled_recorder_emits_no_marker_line() {
        // `record`'s own disabled early-return covers the markers too: a
        // recorder that counts no frames can stamp no frame number.
        let _guard = marker_test_guard(true);
        let mut stats = FrameStats::with_capacity_enabled_and_raw(4, false, true);

        mark_scenario_start("s3-update");
        stats.record(passes(10, 2, 2, 2));

        assert!(stats.marker_log().is_empty());
        assert_eq!(stats.total_frames(), 0);
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn record_leaves_the_pending_queue_alone_once_markers_route_via_channel() {
        // Once a frame has crossed `render_split`, the queue belongs to
        // `send_scene`; draining it at `record` would attach a marker raised
        // for a frame still being built to whatever the render thread is
        // recording now — the cross-thread guess this route removes.
        let _guard = marker_test_guard(true);
        let mut stats = FrameStats::with_capacity_enabled_and_raw(4, true, true);

        markers_route_via_channel();
        mark_scenario_start("s3-update");
        stats.record(passes(10, 2, 2, 2));

        assert!(
            stats.marker_log().is_empty(),
            "record must not steal a channel-routed marker"
        );
        assert_eq!(
            take_pending_markers().len(),
            1,
            "the marker is still queued for send_scene to carry"
        );
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn staged_markers_ride_the_next_recorded_frame_in_order() {
        // The render thread's leg: whatever `stage_markers` was handed comes
        // out on the next frame this thread records, in the order it was
        // raised, and only once.
        let _guard = marker_test_guard(true);
        let mut stats = FrameStats::with_capacity_enabled_and_raw(4, true, true);

        markers_route_via_channel();
        mark_scenario_start("s3-clear");
        mark_scenario_end("s3-clear");
        stage_markers(take_pending_markers());

        stats.record(passes(10, 2, 2, 2));
        assert_eq!(
            stats.marker_log(),
            [
                "bench-scenario-start n=1 s3-clear",
                "bench-scenario-end n=1 s3-clear",
            ],
            "both edges collapsing onto one frame is the empty half-open \
             window a dropped op frame honestly produces"
        );

        stats.record(passes(10, 2, 2, 2));
        assert_eq!(stats.marker_log().len(), 2, "staged markers emit once");
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn stage_markers_of_an_empty_batch_is_a_no_op() {
        let _guard = marker_test_guard(true);
        let mut stats = FrameStats::with_capacity_enabled_and_raw(4, true, true);

        markers_route_via_channel();
        stage_markers(take_pending_markers());
        stats.record(passes(10, 2, 2, 2));

        assert!(stats.marker_log().is_empty());
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
