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
//!   render-thread split's UI/render interleaving. The whole route is
//!   `perf-trace`-only (a release-lean build has no marker code at all) and
//!   **per-thread** — a marker belongs to the thread that raised it until
//!   that thread hands a frame off or records one. See
//!   [`mark_scenario_start`] for the route, the half-open window rule, and
//!   what happens to a marker raised on a thread that does neither.
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

#[cfg(all(test, feature = "perf-trace"))]
use std::cell::Cell;
#[cfg(feature = "perf-trace")]
use std::cell::RefCell;
use std::collections::VecDeque;
#[cfg(feature = "perf-trace")]
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
    /// logged (and the queue-overflow notice beside them, when one was
    /// emitted), in emission order — the seam the marker tests assert
    /// against so they never have to install a global `log` sink (which
    /// would make them order-dependent on every other test). Absent
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
    /// emitted: every marker this frame carries — staged on this thread by
    /// the scene handoff, or raised on this thread when there is no render
    /// thread — is logged, in the order it was raised, stamped with **this**
    /// frame's `n` and immediately ahead of this frame's own `frust-perf
    /// raw` line, so a marker's frame number names the recorded frame that
    /// actually carried it, not a guess made on whichever thread raised it
    /// (see [`mark_scenario_start`]).
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
    /// Two sources, both belonging to **this** thread — which is what makes
    /// the split and the inline executor one rule rather than two:
    ///
    /// - [`STAGED`] — markers put there by [`stage_markers`] when the render
    ///   channel handed this thread the very frame now being recorded. The
    ///   render-thread-split leg.
    /// - [`PENDING`] — this thread's own raise queue. The inline (no render
    ///   thread) executor's leg, where the thread that raises a marker is the
    ///   same one that records the frame. On a render thread this is
    ///   *structurally* empty: a render thread raises no marker of its own,
    ///   and another thread's queue is unreachable from here, so nothing can
    ///   steal a marker still being built on the UI thread.
    ///
    /// Both drains are `is_empty` fast paths: a frame carrying no marker
    /// allocates nothing.
    ///
    /// A marker line's shape is fixed (see [`format_scenario_marker`]); the
    /// only extra line this can emit is the queue-overflow notice
    /// [`format_marker_overflow_line`] writes, and only for a frame whose
    /// markers arrived with a nonzero drop count (see [`MARKER_QUEUE_CAP`]).
    #[cfg(feature = "perf-trace")]
    fn emit_scenario_markers(&mut self) {
        let mut markers = take_staged_markers();
        markers.absorb(take_pending_markers());
        for marker in markers.markers {
            let line = format_scenario_marker(marker.edge, &marker.name, self.total_frames);
            log::info!("{line}");
            #[cfg(test)]
            self.marker_log.push(line);
        }
        if markers.dropped > 0 {
            let line = format_marker_overflow_line(self.total_frames, markers.dropped);
            log::warn!("{line}");
            #[cfg(test)]
            self.marker_log.push(line);
        }
    }

    /// Test-only view of the marker lines [`Self::record`] has emitted so
    /// far, in order — see [`Self::marker_log`]'s docs.
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
/// [`mark_scenario_end`] raises. Compiled only under `perf-trace`, like
/// every other piece of the marker route: a release-lean build carries
/// neither the queues below nor the `bench-scenario-*` string literals this
/// maps to.
#[cfg(feature = "perf-trace")]
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
/// Deliberately opaque and `pub(crate)`: the route it travels
/// ([`take_pending_markers`] → [`stage_markers`]) is this crate's own
/// plumbing, so nothing outside the crate can read the edge, rewrite the
/// name, or inject a marker that was never raised — the emitted line's shape
/// stays this module's business alone. `Box<str>` rather than `String`: a
/// marker name is never appended to after it is raised.
#[cfg(feature = "perf-trace")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScenarioMarker {
    edge: MarkerEdge,
    name: Box<str>,
}

/// The most markers any one leg of the route ([`PENDING`], [`STAGED`],
/// [`crate::render_split`]'s inbox queue) holds at once.
///
/// A leg only grows while markers are raised faster than frames are
/// recorded, which a benchmark process (two markers per scenario operation,
/// about one operation per frame) never does for long; a leg that reaches
/// this cap therefore means something is already wrong — markers raised on a
/// thread that hands no frame off, or a render thread that has stopped
/// recording — and the honest response is to bound the memory rather than
/// accumulate forever. On overflow the **oldest** marker is dropped (a live
/// window needs the newest edges) and the drop is counted; the count travels
/// with the batch and [`FrameStats::record`] reports it on the next frame
/// that carries markers, as its own `frust-perf marker-overflow` line —
/// never folded into a `bench-scenario-*` line, whose shape is a parsed wire
/// format (`benchmarks/PROTOCOL.md` §7).
#[cfg(feature = "perf-trace")]
pub(crate) const MARKER_QUEUE_CAP: usize = 256;

/// One leg of the marker route: markers in raise order, plus how many were
/// dropped to keep the leg inside [`MARKER_QUEUE_CAP`].
///
/// The drop count rides with the markers ([`Self::absorb`] sums it) rather
/// than living in a counter of its own, so a drop that happened on the UI
/// thread is still reported by the render thread that records the frame
/// those markers landed on.
#[cfg(feature = "perf-trace")]
#[derive(Debug, Default)]
pub(crate) struct MarkerQueue {
    markers: VecDeque<ScenarioMarker>,
    dropped: u64,
}

#[cfg(feature = "perf-trace")]
impl MarkerQueue {
    /// An empty queue that has allocated nothing — `const` so the
    /// thread-locals below initialize with no lazy first-use branch.
    pub(crate) const fn new() -> Self {
        Self {
            markers: VecDeque::new(),
            dropped: 0,
        }
    }

    /// Nothing to carry: no markers **and** no drop count. A queue holding
    /// only a drop count cannot happen today (a drop happens on a push,
    /// which leaves the pushed marker behind), but treating the count as
    /// payload keeps a caller from stranding one behind an emptiness fast
    /// path.
    pub(crate) fn is_empty(&self) -> bool {
        self.markers.is_empty() && self.dropped == 0
    }

    /// Queue one marker, dropping the oldest if that would exceed
    /// [`MARKER_QUEUE_CAP`].
    fn push(&mut self, marker: ScenarioMarker) {
        if self.markers.len() >= MARKER_QUEUE_CAP {
            self.markers.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.markers.push_back(marker);
    }

    /// Move `other`'s markers (oldest first) and its drop count into this
    /// queue, enforcing the cap again on the way in.
    pub(crate) fn absorb(&mut self, other: MarkerQueue) {
        self.dropped = self.dropped.saturating_add(other.dropped);
        for marker in other.markers {
            self.push(marker);
        }
    }

    /// Take everything queued here, leaving this queue empty.
    pub(crate) fn take(&mut self) -> MarkerQueue {
        std::mem::take(self)
    }

    /// Forget everything queued here, drop count included — for a leg whose
    /// markers no frame will ever name.
    pub(crate) fn clear(&mut self) {
        self.markers.clear();
        self.dropped = 0;
    }
}

#[cfg(feature = "perf-trace")]
thread_local! {
    /// The markers **this** thread has raised and not yet handed on:
    /// [`mark_scenario_start`]/[`mark_scenario_end`] push here, nothing else
    /// does.
    ///
    /// Per-thread rather than process-wide, which is what collapses the two
    /// executors into one rule instead of a shared queue plus a flag
    /// deciding who owns it:
    ///
    /// - **render-thread split** — the UI thread raises markers and is also
    ///   the thread that calls [`RenderSender::send_scene`], which moves this
    ///   queue into the inbox so the markers travel with that handoff. The
    ///   render thread's own copy of this queue is never pushed to, so
    ///   [`FrameStats::record`] draining it over there can steal nothing.
    /// - **inline** (no render thread) — one thread raises and records, so
    ///   [`FrameStats::record`] drains this queue directly, for the frame it
    ///   is recording right now.
    ///
    /// Bounded at [`MARKER_QUEUE_CAP`].
    ///
    /// [`RenderSender::send_scene`]: crate::render_split::RenderSender::send_scene
    static PENDING: RefCell<MarkerQueue> = const { RefCell::new(MarkerQueue::new()) };

    /// The markers handed to *this* thread for the very next frame it
    /// records — the render thread's leg of the route. [`stage_markers`]
    /// appends, [`FrameStats::record`] drains. Per-thread for the same reason
    /// [`PENDING`] is: the draining thread is by construction the one that
    /// took the batch out of the inbox and is about to render and record it,
    /// so no lock is needed and no other thread's frame can pick these up by
    /// accident. Bounded at [`MARKER_QUEUE_CAP`].
    static STAGED: RefCell<MarkerQueue> = const { RefCell::new(MarkerQueue::new()) };
}

/// Take the calling thread's raised-but-not-handed-on markers, leaving its
/// queue empty — [`RenderSender::send_scene`] calls this to move them into
/// the inbox beside the scene they belong to, and calls it again (discarding
/// the result) on its dead-receiver path, so a queue no frame can ever name
/// is not left growing.
///
/// [`markers_enabled`] first: this runs on every frame of every split shell,
/// including in a `perf-trace` build running with `FRUST_TRACE` off, so the
/// disabled case must cost one cached bool read and no thread-local access
/// at all.
///
/// [`RenderSender::send_scene`]: crate::render_split::RenderSender::send_scene
#[cfg(feature = "perf-trace")]
#[inline]
pub(crate) fn take_pending_markers() -> MarkerQueue {
    if !markers_enabled() {
        return MarkerQueue::new();
    }
    PENDING.with(|pending| pending.borrow_mut().take())
}

/// Hand markers to the calling thread's next recorded frame, appending to
/// whatever is already staged there (see [`STAGED`]). Called by the render
/// thread as it takes a scene out of the inbox; the frame it records next is
/// the one that scene belongs to.
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
#[cfg(feature = "perf-trace")]
pub(crate) fn stage_markers(markers: MarkerQueue) {
    if markers.is_empty() {
        return;
    }
    STAGED.with(|staged| staged.borrow_mut().absorb(markers));
}

/// Take this thread's staged markers, leaving the slot empty. `is_empty`
/// fast path first: the overwhelmingly common frame carries no marker and
/// must not allocate.
#[cfg(feature = "perf-trace")]
fn take_staged_markers() -> MarkerQueue {
    STAGED.with(|staged| {
        let mut staged = staged.borrow_mut();
        if staged.is_empty() {
            MarkerQueue::new()
        } else {
            staged.take()
        }
    })
}

#[cfg(all(test, feature = "perf-trace"))]
thread_local! {
    /// Test-only override of [`markers_enabled`] for the **calling** thread:
    /// [`enabled`] caches a process environment read in a `OnceLock` and can
    /// never be flipped back, so a test that needs a marker actually queued
    /// sets this instead of relaxing production gating.
    ///
    /// Per-thread like the queues it gates, which is what lets marker tests
    /// run beside every other test in the process with no lock at all: a
    /// two-thread test arms it on each thread that raises markers (see
    /// [`marker_test_guard`]).
    static MARKERS_FORCE_ENABLED: Cell<bool> = const { Cell::new(false) };
}

/// The RAII handle [`marker_test_guard`] returns: disarms the force switch
/// and empties this thread's queues on drop, so one test's leftovers can
/// never leak into the next test that runs on the same thread.
#[cfg(all(test, feature = "perf-trace"))]
pub(crate) struct MarkerTestGuard {
    _private: (),
}

#[cfg(all(test, feature = "perf-trace"))]
impl Drop for MarkerTestGuard {
    fn drop(&mut self) {
        MARKERS_FORCE_ENABLED.with(|forced| forced.set(false));
        reset_marker_route();
    }
}

/// Empty both of the calling thread's marker queues.
#[cfg(all(test, feature = "perf-trace"))]
fn reset_marker_route() {
    PENDING.with(|pending| pending.borrow_mut().clear());
    STAGED.with(|staged| staged.borrow_mut().clear());
}

/// Start this thread's use of the marker route from a pristine queue pair
/// and (when `force_enabled`) with markers queueing as though `FRUST_TRACE`
/// were set. `pub(crate)` because `render_split`'s channel tests drive the
/// same route from the other end.
///
/// There is no lock: every piece of state this arms belongs to the calling
/// thread, so two marker tests running in parallel cannot see each other's
/// queues. A test that spawns its own UI thread must call this on **that**
/// thread too — the force switch no more crosses a thread boundary than a
/// queue does.
#[cfg(all(test, feature = "perf-trace"))]
pub(crate) fn marker_test_guard(force_enabled: bool) -> MarkerTestGuard {
    reset_marker_route();
    MARKERS_FORCE_ENABLED.with(|forced| forced.set(force_enabled));
    MarkerTestGuard { _private: () }
}

/// Whether a raised marker is queued at all. The one-dial gate
/// ([`enabled`], not [`enabled`]-and-[`raw_enabled`]): a marker is emitted
/// by [`FrameStats::record`] itself, which runs whenever perf is on, so
/// tying markers to the raw-export dial would mean a scenario window that
/// exists in one capture and silently not in another.
#[cfg(feature = "perf-trace")]
#[inline]
fn markers_enabled() -> bool {
    #[cfg(test)]
    if MARKERS_FORCE_ENABLED.with(Cell::get) {
        return true;
    }
    enabled()
}

/// Queue one marker onto the calling thread's [`PENDING`] queue — the single
/// body behind both [`mark_scenario_start`] and [`mark_scenario_end`], so the
/// two edges can never drift apart.
#[cfg(feature = "perf-trace")]
fn push_marker(edge: MarkerEdge, name: &str) {
    if !markers_enabled() {
        return;
    }
    PENDING.with(|pending| {
        pending.borrow_mut().push(ScenarioMarker {
            edge,
            name: name.into(),
        });
    });
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

/// Prefix of the queue-overflow notice (see [`MARKER_QUEUE_CAP`]).
/// Deliberately neither the `frust-perf raw`/`frust-perf op`/`frust-perf
/// plugin` prefix nor a `bench-scenario-*` one, so a harness scanning the
/// same log stream (`benchmarks/harness/stats.py`) reads this as an
/// unrelated line rather than as a malformed record of a shape it parses.
#[cfg(feature = "perf-trace")]
const MARKER_OVERFLOW_PREFIX: &str = "frust-perf marker-overflow";

/// Formats the notice that `dropped` markers were discarded to keep a route
/// leg inside [`MARKER_QUEUE_CAP`], stamped with the frame whose markers
/// carried the count in. Kept out of the `bench-scenario-*` line itself
/// because that line's `<prefix> n=<u64> <name>` shape is a parsed wire
/// format: `stats.py`'s `parse_marker_line` reads everything after the `n=`
/// token as the scenario name, so an appended field would rename the window
/// rather than annotate it.
#[cfg(feature = "perf-trace")]
fn format_marker_overflow_line(frame: u64, dropped: u64) -> String {
    format!("{MARKER_OVERFLOW_PREFIX} n={frame} dropped={dropped}")
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
/// - **Raise a marker on the thread that produces frames.** A marker is
///   queued on the calling thread and leaves it only when that same thread
///   hands a frame across the render channel (the split's UI thread) or
///   records one itself (the inline executor). Raised anywhere else — a
///   `spawn_blocking` pool thread, a plugin callback thread — it belongs to
///   a queue no frame will ever be recorded from, and is therefore never
///   emitted. That is the price of never guessing a frame number across a
///   thread boundary: a marker attached to a frame the raiser is not
///   producing would be exactly the cross-thread guess this route exists to
///   remove. A scenario measuring off-thread work brackets it from the
///   build that shows the result, not from inside the worker.
///
/// A no-op unless [`enabled`] is `true`; unlike the raw per-frame line it
/// does **not** additionally require [`raw_enabled`], so a scenario window
/// is present in every perf-enabled capture. A build without the
/// `perf-trace` feature compiles the whole route out, leaving this an empty
/// function.
#[cfg_attr(not(feature = "perf-trace"), allow(unused_variables))]
pub fn mark_scenario_start(name: &str) {
    #[cfg(feature = "perf-trace")]
    push_marker(MarkerEdge::Start, name);
}

/// Raise a `bench-scenario-end` marker — the closing edge of the window
/// [`mark_scenario_start`] opened; see its docs, which cover the gating, the
/// emitted `n`, and the half-open `[start_n, end_n)` rule identically. In
/// the S3 convention this is raised in the build *after* the measured one,
/// so its `n` is one past the window's last frame.
#[cfg_attr(not(feature = "perf-trace"), allow(unused_variables))]
pub fn mark_scenario_end(name: &str) {
    #[cfg(feature = "perf-trace")]
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

    #[cfg(feature = "perf-trace")]
    #[test]
    fn markers_raised_while_perf_is_disabled_accumulate_nothing() {
        // Perf off (the guard leaves the force switch clear, and no test-run
        // process sets FRUST_TRACE): raising markers must be safe AND must
        // queue nothing at all — a queue nobody ever drains is the one way
        // this route could leak in a shipped build.
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
    fn inline_executor_drains_this_threads_pending_queue_at_record() {
        // The inline (no render thread) path: one thread raises and records,
        // so `record` drains that thread's own queue for the frame it is
        // recording right now. No route flag decides this — the queue it
        // drains is simply the queue the markers were raised into.
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
    fn staged_markers_ride_the_next_recorded_frame_in_order() {
        // The render thread's leg in isolation: whatever `stage_markers` was
        // handed comes out on the next frame this thread records, in the
        // order it was raised, and only once.
        let _guard = marker_test_guard(true);
        let mut stats = FrameStats::with_capacity_enabled_and_raw(4, true, true);

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

        stage_markers(take_pending_markers());
        stats.record(passes(10, 2, 2, 2));

        assert!(stats.marker_log().is_empty());
    }

    #[cfg(feature = "perf-trace")]
    #[test]
    fn an_overflowing_queue_drops_the_oldest_markers_and_reports_the_count() {
        // The bound (`MARKER_QUEUE_CAP`): a queue that is raised into far
        // faster than frames are recorded must stop growing, keep the
        // NEWEST edges (a live window needs those), and say how many it
        // discarded — on a line of its own, never inside a
        // `bench-scenario-*` line, whose shape is a parsed wire format.
        let _guard = marker_test_guard(true);
        let mut stats = FrameStats::with_capacity_enabled_and_raw(4, true, true);

        let raised = MARKER_QUEUE_CAP + 3;
        for i in 0..raised {
            mark_scenario_start(&format!("s3-op{i}"));
        }
        stats.record(passes(10, 2, 2, 2));

        let log = stats.marker_log();
        assert_eq!(
            log.len(),
            MARKER_QUEUE_CAP + 1,
            "a capped queue's worth of markers, plus one overflow notice"
        );
        assert_eq!(
            log[0], "bench-scenario-start n=1 s3-op3",
            "the three OLDEST were dropped, and the survivors keep the \
             unannotated marker shape"
        );
        assert_eq!(
            log[MARKER_QUEUE_CAP - 1],
            format!("bench-scenario-start n=1 s3-op{}", raised - 1),
            "the newest raised marker survived"
        );
        assert_eq!(
            log[MARKER_QUEUE_CAP],
            "frust-perf marker-overflow n=1 dropped=3"
        );

        // The count travels with the batch that carried it, so a later frame
        // does not re-report it.
        mark_scenario_end("s3-done");
        stats.record(passes(10, 2, 2, 2));
        assert_eq!(
            stats.marker_log()[MARKER_QUEUE_CAP + 1..],
            ["bench-scenario-end n=2 s3-done"]
        );
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
