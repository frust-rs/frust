//! The render-path half of the iOS frame loop: the two executors
//! ([`FrameExecutor::Split`], the default, and [`FrameExecutor::Inline`], the
//! `FRUST_NO_RENDER_THREAD` fallback), the scene payload they hand across the
//! split, and the shared encode→acquire→submit tail ([`render_scene`]) both run.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use frust_core::FrameTime;
use frust_render::{
    AcquireOutcome, EncodeOutcome, FrameOutcome, RenderContext, SurfacePhase, SurfaceRenderer,
};
use frust_scene::Scene;
use frust_shell_common::perf::{
    self, FramePasses, FrameStats, GpuPasses, RenderSpans, StartupSpans, UiSpans,
};
use frust_shell_common::{
    FrameMeta, RenderCommand, RenderSender, SceneFrame, SceneReturnReceiver, SurfaceSize,
};
use objc2::rc::autoreleasepool;

use super::present_sync::PresentHandoff;

/// The render thread's **pacing trace**: one diagnostic line per rendered
/// frame decomposing where that frame's wall clock went between two presents,
/// behind a dial of its own (`FRUST_PACE_TRACE`, parsed exactly like
/// `FRUST_TRACE`) on top of `perf-trace` + [`perf::enabled`].
///
/// # What it answers
///
/// `acquire_us` alone cannot say *why* the swapchain made the render thread
/// wait: a wait for the display's next refresh and a wait for the layer's
/// drawable pool to release one look identical from inside the span. The two
/// are told apart by what surrounds the span — how long this thread sat idle
/// before asking (`idle_us`), and whether the interval between consecutive
/// presents (`p2p_us`) is one refresh period or two. A wait anchored to an
/// absolute refresh deadline shrinks when the thread arrives late; a wait for a
/// pool release does not, and instead pins `loop_us` to one period however
/// early the ask was.
///
/// # What it answered (iPhone SE, A13, iOS 26, 60 Hz — 2026-09-06,
/// act_000001a070c81738SE728A0X)
///
/// Refresh-deadline anchored, not pool starvation: over 7,269 S3 frames
/// `acq_us` regressed on `idle_us` with a slope of −0.90, so the wait shrinks
/// almost 1:1 with a later ask. And nothing is being dropped — `p2p_us` p50
/// 16.67 ms, 0.06 % of intervals past 25 ms, none past 41 ms. The S3 reading
/// that "3.5 % of frames take two vsyncs" (`total_us` > 20 ms) therefore
/// counts frames whose excess IS that wait: the same series' CPU-work row
/// (`total − acquire`, `summarize.py --frust-work-row`) has zero frames over
/// 20 ms, and S2 — never suspected of anything — sits at 18.9 % by the same
/// `total_us` rule at a locked 60.8 fps.
///
/// `p2p_us` is the only field here that measures the *presented* cadence.
/// Every other per-frame number this shell reports — including
/// `FramePasses::total`, which sums UI-thread and render-thread spans that run
/// concurrently under the split — is a cost, not an interval, and a cost above
/// one refresh period is not by itself a dropped frame.
///
/// # Cost
///
/// Zero when the dial is off: no clock read of its own (every instant is one
/// the surrounding spans already took under `perf_on`), one cached `bool`
/// test, and a reused thread-local buffer when it is on. The dial is a
/// diagnostic, not a benchmark mode — a measured run leaves it off so its
/// per-frame line cannot perturb what is being measured.
#[cfg(feature = "perf-trace")]
mod pace_trace {
    use std::cell::{Cell, RefCell};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    /// Prefix every line carries, so a capture greps this series apart from
    /// `frust-perf raw`/`frust-perf frame`. A literal here (not a `perf::SPAN_*`
    /// const) because it names a diagnostic line shape, not a span.
    const PREFIX: &str = "frust-perf ios-pace";

    /// The dial, cached for the process like [`perf::enabled`]'s own: set the
    /// compile-time `FRUST_PACE_TRACE` define (`frust build ios --define
    /// FRUST_PACE_TRACE=1`) or the runtime env var to any value but `"0"`.
    ///
    /// [`perf::enabled`]: frust_shell_common::perf::enabled
    pub(super) fn enabled() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| {
            fn set(value: Option<&str>) -> bool {
                matches!(value, Some(v) if v != "0")
            }
            set(option_env!("FRUST_PACE_TRACE"))
                || set(std::env::var("FRUST_PACE_TRACE").ok().as_deref())
        })
    }

    /// What one frame's line needs from the frames before it. `Copy`, in a
    /// `Cell`, on the render thread that produces every frame — no lock, no
    /// allocation.
    #[derive(Clone, Copy, Default)]
    struct Prev {
        /// End of the previous frame's acquire — the base of `loop_us`.
        acquire_end: Option<Instant>,
        /// End of the previous **presenting** frame's submit — the instant
        /// its present was issued, and so the base of `p2p_us`. Unlike
        /// [`Self::acquire_end`]/[`Self::loop_end`], a frame that presented
        /// nothing (`Redraw`/`SurfaceLost`/`Skipped`/`Err` — see
        /// [`record`]'s `presented` parameter) leaves this untouched: `p2p_us`
        /// measures the presented cadence specifically, so its base may only
        /// ever advance on a frame that actually presented (an earlier unconditional
        /// advance let one long present-to-present gap get reported as two
        /// short, misleading intervals).
        submit_end: Option<Instant>,
        /// End of the previous loop iteration's last render-thread work (its
        /// submit) — the base of `idle_us`, which is therefore the time this
        /// thread had nothing to do but wait for the UI thread's next scene.
        /// Tracked separately from [`Self::submit_end`] so a future iteration
        /// that ends on something other than the submit still reports the idle
        /// window correctly.
        loop_end: Option<Instant>,
    }

    thread_local! {
        static PREV: Cell<Prev> = const { Cell::new(Prev {
            acquire_end: None,
            submit_end: None,
            loop_end: None,
        }) };
        static SEQ: Cell<u64> = const { Cell::new(0) };
        /// Reused across frames so the hot path formats without growing an
        /// allocation, matching `perf`'s own raw-line buffer discipline.
        static BUF: RefCell<String> = const { RefCell::new(String::new()) };
    }

    /// Write one frame's `frust-perf ios-pace` line into `buf` (cleared
    /// first) and return the next [`Prev`] state, from explicit inputs — the
    /// pure core [`record`] wraps around the thread-local buffer/`PREV`/`SEQ`
    /// state and unit tests call directly with a scratch `String` and an
    /// explicit `n`/`prev`, no thread-local or `log` dependency (mirrors
    /// `frust-engine::renderer::EncodeTrace::line`'s directly-tested-formatter
    /// shape).
    ///
    /// `presented` is whether *this* frame actually presented
    /// (`FrameOutcome::Rendered` on the inline path; the UI-thread present
    /// under armed present-sync — see [`super::pace_record`]'s caller). When
    /// `false`, `p2p_us` reports the literal `NA` sentinel rather than a
    /// number (a wrong-looking interval reads as measured; `NA` cannot), and
    /// the returned [`Prev::submit_end`] carries `prev.submit_end` through
    /// unchanged — the base advances only on a frame that actually presented.
    ///
    /// Every argument is one distinct field of the one frame this line
    /// reports on (mirrors [`render_scene`]'s and `ffi_glue::install_surface`'s
    /// own `#[allow(clippy::too_many_arguments)]` for the same frame-tail
    /// shape) — bundling them into a struct here would only relocate the same
    /// count onto a constructor at each of this function's three call sites
    /// (`record` and the two `#[cfg(test)]` callers below).
    #[allow(clippy::too_many_arguments)]
    fn write_line(
        buf: &mut String,
        n: u64,
        prev: Prev,
        wake: Instant,
        acquire_start: Instant,
        acquire: Duration,
        submit_start: Instant,
        submit: Duration,
        presented: bool,
    ) -> Prev {
        use std::fmt::Write as _;

        let acquire_end = acquire_start + acquire;
        let submit_end = submit_start + submit;
        // A missing base is the first frame of the series (or the first after a
        // surface episode); it reports 0 rather than a fabricated interval, and
        // the analysis discards frame 1 exactly as every other first-frame
        // number is discarded.
        let since = |base: Option<Instant>, at: Instant| {
            base.map_or(Duration::ZERO, |b| at.saturating_duration_since(b))
        };
        buf.clear();
        let _ = write!(
            buf,
            "{PREFIX} n={n} wake_us={} idle_us={} acq_us={} sub_us={} \
             loop_us={} p2p_us=",
            acquire_start.saturating_duration_since(wake).as_micros(),
            since(prev.loop_end, wake).as_micros(),
            acquire.as_micros(),
            submit.as_micros(),
            since(prev.acquire_end, acquire_end).as_micros(),
        );
        if presented {
            let _ = write!(buf, "{}", since(prev.submit_end, submit_end).as_micros());
        } else {
            buf.push_str("NA");
        }
        Prev {
            acquire_end: Some(acquire_end),
            submit_end: if presented {
                Some(submit_end)
            } else {
                prev.submit_end
            },
            loop_end: Some(submit_end),
        }
    }

    /// Emit one frame's line. `wake` is when the render thread entered the
    /// frame tail with a scene in hand; the remaining instants/durations are
    /// the spans the caller already measured. See [`write_line`]'s doc for
    /// `presented`.
    pub(super) fn record(
        wake: Instant,
        acquire_start: Instant,
        acquire: Duration,
        submit_start: Instant,
        submit: Duration,
        presented: bool,
    ) {
        let prev = PREV.with(Cell::get);
        let n = SEQ.with(|seq| {
            let n = seq.get() + 1;
            seq.set(n);
            n
        });
        let next = BUF.with_borrow_mut(|buf| {
            let next = write_line(
                buf,
                n,
                prev,
                wake,
                acquire_start,
                acquire,
                submit_start,
                submit,
                presented,
            );
            log::info!("{buf}");
            next
        });
        PREV.set(next);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The `frust-perf ios-pace` line is a contract: a capture is graded
        /// by grepping its fields, so the field order and names are pinned
        /// here rather than only by the formatter (mirrors
        /// `frust-engine::renderer`'s
        /// `an_encode_trace_line_reports_every_column_in_order`).
        #[test]
        fn ios_pace_line_reports_every_field_in_order() {
            let t0 = Instant::now();
            let wake = t0;
            let acquire_start = t0 + Duration::from_micros(1_000);
            let acquire = Duration::from_micros(2_000);
            let submit_start = acquire_start + acquire;
            let submit = Duration::from_micros(500);
            let prev = Prev {
                acquire_end: Some(t0.checked_sub(Duration::from_micros(2_000)).unwrap()),
                submit_end: Some(t0.checked_sub(Duration::from_micros(3_500)).unwrap()),
                loop_end: Some(t0.checked_sub(Duration::from_micros(4_000)).unwrap()),
            };
            let mut buf = String::new();
            let next = write_line(
                &mut buf,
                7,
                prev,
                wake,
                acquire_start,
                acquire,
                submit_start,
                submit,
                true,
            );
            assert_eq!(
                buf,
                "frust-perf ios-pace n=7 wake_us=1000 idle_us=4000 acq_us=2000 \
                 sub_us=500 loop_us=5000 p2p_us=7000"
            );
            let acquire_end = acquire_start + acquire;
            let submit_end = submit_start + submit;
            assert_eq!(next.acquire_end, Some(acquire_end));
            assert_eq!(next.submit_end, Some(submit_end));
            assert_eq!(next.loop_end, Some(submit_end));
        }

        /// A missing base (no prior frame — the first of the series, or the
        /// first after a surface episode) reports `0` rather than a
        /// fabricated interval, for every `since()`-derived field.
        #[test]
        fn since_returns_zero_on_missing_base() {
            let t0 = Instant::now();
            let wake = t0;
            let acquire_start = t0 + Duration::from_micros(100);
            let acquire = Duration::from_micros(50);
            let submit_start = acquire_start + acquire;
            let submit = Duration::from_micros(10);
            let mut buf = String::new();
            let _ = write_line(
                &mut buf,
                1,
                Prev::default(),
                wake,
                acquire_start,
                acquire,
                submit_start,
                submit,
                true,
            );
            assert_eq!(
                buf,
                "frust-perf ios-pace n=1 wake_us=100 idle_us=0 acq_us=50 \
                 sub_us=10 loop_us=0 p2p_us=0"
            );
        }

        /// The fix this type exists for: a frame that presented nothing
        /// reports `p2p_us` as the `NA` sentinel, never a number, and does
        /// NOT advance `submit_end` — the next presenting frame's `p2p_us`
        /// must still measure from the last REAL present, not from this
        /// frame's near-zero submit.
        #[test]
        fn a_non_presenting_frame_does_not_advance_the_base() {
            let t0 = Instant::now();
            let prior_submit_end = t0.checked_sub(Duration::from_millis(30)).unwrap();
            let prev = Prev {
                acquire_end: Some(t0.checked_sub(Duration::from_micros(500)).unwrap()),
                submit_end: Some(prior_submit_end),
                loop_end: Some(t0.checked_sub(Duration::from_micros(500)).unwrap()),
            };
            let wake = t0;
            let acquire_start = t0 + Duration::from_micros(10);
            let acquire = Duration::from_micros(5);
            let submit_start = acquire_start + acquire;
            // A non-presenting frame's submit is near-zero (e.g. a
            // reconfigure/skip) — exactly the shape that used to corrupt the
            // next presenting frame's `p2p_us`.
            let submit = Duration::from_micros(1);
            let mut buf = String::new();
            let next = write_line(
                &mut buf,
                2,
                prev,
                wake,
                acquire_start,
                acquire,
                submit_start,
                submit,
                false,
            );
            assert!(
                buf.ends_with("p2p_us=NA"),
                "a non-presenting frame must report p2p_us as absent, not an interval: {buf}"
            );
            assert_eq!(
                next.submit_end,
                Some(prior_submit_end),
                "the base must not advance on a frame that presented nothing"
            );
            // acquire_end/loop_end are unaffected by the presented gate — they
            // track the frame's own acquire/submit work regardless of outcome.
            let acquire_end = acquire_start + acquire;
            let submit_end = submit_start + submit;
            assert_eq!(next.acquire_end, Some(acquire_end));
            assert_eq!(next.loop_end, Some(submit_end));
        }
    }
}

/// One finished frame's payload crossing the UI→render-thread handoff in the
/// split: the painted [`Scene`] plus the clear color it was
/// painted for (the live theme's surface color — it must ride *with* the frame so
/// a mid-frame theme flip clears to the right color, mirroring the desktop and
/// Android shells' `PaintedScene`). This is the `S` type parameter of
/// [`SceneFrame`]/[`render_channel`](frust_shell_common::render_channel); both
/// `Scene` and `peniko::Color` are `Send`, keeping the handoff `Send`-clean with
/// no `unsafe`.
pub(crate) struct PaintedScene {
    pub(crate) scene: Scene,
    pub(crate) base_color: peniko::Color,
}

/// The render-path half of the iOS frame loop: either the
/// render-thread split ([`Self::Split`], default) or the pre-split inline fallback
/// ([`Self::Inline`], `FRUST_NO_RENDER_THREAD`). Chosen once at construction from
/// [`render_thread_enabled`](frust_shell_common::render_thread_enabled) and owned
/// by [`crate::app::IosAppHandle`].
pub(crate) enum FrameExecutor {
    /// Pre-split fallback: the [`RenderContext`]/[`SurfaceRenderer`] and all perf
    /// recording live on the UI thread, and the encode→acquire→submit tail runs
    /// synchronously inside [`crate::app::IosAppHandle::frame`]. Boxed — it owns
    /// the whole render stack and dwarfs the split's thread-handle variant.
    Inline(Box<InlineExecutor>),
    /// The split: the renderer + context moved to a dedicated render thread; the
    /// UI thread hands it finished frames over the channel.
    Split(SplitExecutor),
}

/// The single-thread fallback executor (kill switch engaged): the
/// [`RenderContext`]/[`SurfaceRenderer`] and all perf recording live on the UI
/// thread, exactly as the pre-split shell did.
pub(crate) struct InlineExecutor {
    /// The render context, borrowed out by
    /// [`crate::app::IosAppHandle::inline_renderer_mut`] for the FFI layer's
    /// `unsafe` surface recreation.
    pub(super) render_cx: RenderContext,
    /// The surface renderer, likewise borrowed out for the inline path's
    /// surface recreation and read for the live surface's phase/resolved alpha.
    pub(super) renderer: SurfaceRenderer,
    frame_stats: FrameStats,
    /// The cold-start span recorder begun in [`crate::ffi_glue::create_handle`];
    /// `Option::take`n on the first successful present (records
    /// [`perf::SPAN_FIRST_FRAME_PRESENTED`] + emits — inside [`render_scene`]),
    /// `None` thereafter.
    startup_spans: Option<StartupSpans>,
    /// Running count of presented frames. Inline renders on the UI
    /// thread, so this is bumped and read on the same thread — the `Arc<Atomic>`
    /// shape matches the split's cross-thread counter so `FrameExecutor` reads
    /// both variants uniformly.
    presented: Arc<AtomicU64>,
}

impl InlineExecutor {
    /// Build the fallback executor around the already-created, `SurfaceReady`
    /// renderer + context (surface creation and the early startup spans happened
    /// in [`crate::ffi_glue::create_handle`], which hands `startup_spans` over
    /// here to finish).
    pub(crate) fn new(
        render_cx: RenderContext,
        renderer: SurfaceRenderer,
        startup_spans: StartupSpans,
    ) -> Self {
        Self {
            render_cx,
            renderer,
            frame_stats: FrameStats::new(),
            startup_spans: Some(startup_spans),
            presented: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Run the encode→acquire→submit tail synchronously for `scene`, recording
    /// the folded frame and returning the encode span for the UI-side deadline
    /// estimate.
    fn submit_frame(
        &mut self,
        scene: &Scene,
        base_color: peniko::Color,
        ui: UiSpans,
        perf_on: bool,
    ) -> Duration {
        // Owned directly (no lock involved): the inline path never shares
        // `startup_spans` with another thread, unlike the split's
        // `StartupRecorder::Shared` — see that type's docs.
        let (encode_time, _startup_retired) = render_scene(
            &mut self.renderer,
            &self.render_cx,
            scene,
            base_color,
            ui,
            &mut self.frame_stats,
            crate::ffi_glue::StartupRecorder::Owned(&mut self.startup_spans),
            perf_on,
            &self.presented,
            // Present-sync never applies inline: this tail already runs on the
            // UI thread inside the `CADisplayLink` tick, so wgpu's own present
            // is *already* issued on the transaction-committing thread — fully
            // in sync with no deferral needed. Deferring it a tick here would
            // only add latency.
            None,
        );
        encode_time
    }

    /// Record a gate-skipped frame (all-zero pass durations) so the skip counter
    /// accumulates in the perf line, mirroring the pre-split inline behavior.
    ///
    /// Still asks for this surface's real GPU pass timing: the reading lags
    /// the calling frame by design (see [`SurfaceRenderer::gpu_pass_timings`]),
    /// so a run of gate-skipped ticks would otherwise show `gpu_q=0` gaps in
    /// the log even while the ring keeps a perfectly good recent reading.
    fn record_skip(&mut self) {
        let mut passes = FramePasses {
            skipped: true,
            ..FramePasses::default()
        };
        if let Some(gpu) = gpu_passes(&self.renderer) {
            passes = passes.with_gpu(gpu);
        }
        self.frame_stats.record(passes);
        if self.frame_stats.should_emit() {
            self.frame_stats.emit_log();
        }
    }

    /// Record [`perf::SPAN_FIRST_REBUILD_DONE`] right after the initial rebuild
    /// (the UI thread owns the startup line in the inline path).
    fn record_first_rebuild(&mut self) {
        if let Some(spans) = self.startup_spans.as_mut() {
            spans.record(perf::SPAN_FIRST_REBUILD_DONE);
        }
    }
}

/// Upper bound on how long `frust_pause` blocks the UI (main) thread on the
/// [`RenderCommand::Pause`] barrier before letting the app background degraded
/// rather than hanging behind a wedged render thread.
///
/// **Community-approximate**: UIKit's app-suspension watchdog (the deadline for
/// returning from `applicationDidEnterBackground` before the system kills the
/// process, `0x8badf00d`) is not a published constant; the community-converged
/// estimate is ~5s of background-transition grace. 2s stays safely under that so
/// the barrier degrades before the watchdog fires; the barrier normally returns
/// in microseconds (the render thread quiesces to `Paused`), so a multi-second
/// wait means the render thread is stuck and hanging risks the process kill this
/// barrier exists to prevent.
const PAUSE_BARRIER_DEADLINE: Duration = Duration::from_secs(2);

/// The render-thread-split executor (default): the UI-thread [`RenderSender`] half
/// of the scene-handoff channel plus the render thread's [`JoinHandle`]. The
/// render thread owns the [`RenderContext`]/[`SurfaceRenderer`], the surface, the
/// [`FrameStats`] recorder (the single perf emitter), and the startup line — see
/// [`crate::ffi_glue::render_loop`].
pub(crate) struct SplitExecutor {
    /// `Option` so [`Drop`] can drop it *before* joining: dropping the sender is
    /// what signals the render loop to exit.
    sender: Option<RenderSender<PaintedScene, crate::ffi_glue::SendableMetalLayer>>,
    join: Option<JoinHandle<()>>,
    /// The UI-side mirror of "a surface exists" — the render thread owns the real
    /// `SurfacePhase`, so the UI thread can't query it; this gates
    /// [`crate::app::IosAppHandle::frame`]'s not-ready early return in place of
    /// `renderer.phase()`.
    /// Always `true` after construction on iOS (the `CAMetalLayer` is permanent
    /// and the render thread self-heals a lost surface — no UI-side destroy path).
    surface_active: bool,
    /// Monotonically increasing per-frame id stamped into [`FrameMeta`].
    frame_id: u64,
    /// Fatal-signal flag: the render thread stores `true` here
    /// if its **first** surface install fails — unrecoverable (an incapable
    /// GPU/driver can't change mid-process). The UI thread reads it via
    /// [`crate::app::IosAppHandle::render_fatal`] each `frust_render_frame` and
    /// returns [`FRAME_FATAL`](crate::ffi_support::FRAME_FATAL) so Swift latches
    /// `initFailed` + invalidates its `CADisplayLink`. One clone here, one in the
    /// render thread ([`crate::ffi_glue::render_loop`]).
    pub(super) fatal: Arc<AtomicBool>,
    /// The UI-side half of the render thread's scene give-back channel:
    /// polled once per [`Self::submit_frame`] for a scene the
    /// render thread has finished with, so its buffer is reused instead of
    /// reallocating a fresh `Scene` every frame.
    scene_return: SceneReturnReceiver<Scene>,
    /// A scene reclaimed from [`RenderSender::send_scene`]'s returned stale
    /// frame (the UI thread outran the render thread, overwriting an
    /// un-taken scene in the latest-wins slot) — checked before
    /// [`Self::scene_return`] on the next [`Self::take_reusable_scene`] call
    /// so that buffer is reused too, rather than dropped.
    spare_scene: Option<Scene>,
    /// Running count of presented frames: one clone here (read by the
    /// UI thread before paint via `FrameExecutor::presented_frames`), one on the
    /// render thread ([`crate::ffi_glue::render_loop`], which bumps it on each
    /// `FrameOutcome::Rendered`). A plain `Arc<AtomicU64>` — no channel/protocol,
    /// mirroring the `fatal`-flag pattern.
    presented: Arc<AtomicU64>,
    /// The present-sync handoff slot: one clone here (the UI
    /// thread takes and presents from it in
    /// [`crate::app::IosAppHandle::present_pending_frame`]), one on the render
    /// thread ([`crate::ffi_glue::render_loop`], which parks each submitted frame
    /// in it instead of presenting when armed). Inert — never written, always
    /// empty — when the host did not arm present-sync, which is the default.
    /// See [`PresentHandoff`].
    pub(super) present: Arc<PresentHandoff>,
    /// The render thread's "I just (re)installed the surface myself" signal:
    /// set by [`crate::ffi_glue::render_loop`]'s
    /// `SurfaceLost` self-heal arm, taken once per frame by
    /// [`FrameExecutor::take_surface_reinstalled`] and fed into
    /// [`FrameInputs::surface_changed_or_resized`].
    ///
    /// # Why the flag exists
    ///
    /// iOS is the one shell whose surface loss has **no platform entry point**:
    /// the `CAMetalLayer` is permanent, so nothing external re-drives creation
    /// and the render thread heals itself (see [`crate::ffi_glue::render_loop`]'s
    /// iOS surface recovery). Android's equivalent arrives as a Kotlin
    /// `surfaceChanged`, which latches its own gate input. Before the frame gate
    /// actually idled on iOS that gap was invisible — the next of an unbroken
    /// stream of frames repainted the healed surface within a tick. Now that a
    /// resting screen produces no frames at all, a self-heal with no UI-side
    /// signal would leave the fresh swapchain un-painted (and, under
    /// present-sync, the platform-view batch paired with the lost frame held)
    /// until some unrelated dirtiness happened along. So the render thread says
    /// so, and the next tick runs a real frame.
    ///
    /// A plain `Arc<AtomicBool>` — one clone here, one on the render thread —
    /// the same shape as [`Self::fatal`]/[`Self::presented`], no channel or
    /// protocol. Inert on the inline path, which recovers UI-side in
    /// `frust_render_frame`/`frust_resize` (and re-opens the gate's warmup
    /// through `IosAppHandle::set_surface`) instead.
    surface_reinstalled: Arc<AtomicBool>,
}

impl SplitExecutor {
    pub(crate) fn new(
        sender: RenderSender<PaintedScene, crate::ffi_glue::SendableMetalLayer>,
        join: JoinHandle<()>,
        fatal: Arc<AtomicBool>,
        scene_return: SceneReturnReceiver<Scene>,
        presented: Arc<AtomicU64>,
        present: Arc<PresentHandoff>,
        surface_reinstalled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            sender: Some(sender),
            join: Some(join),
            surface_active: true,
            frame_id: 0,
            fatal,
            scene_return,
            spare_scene: None,
            presented,
            present,
            surface_reinstalled,
        }
    }

    /// Reclaim a reusable, empty `Scene` for the next frame: prefer a scene
    /// already reclaimed from a stale [`Self::submit_frame`]
    /// give-back ([`Self::spare_scene`]), else poll the render thread's
    /// give-back channel ([`Self::scene_return`]), else allocate a fresh one.
    /// Either reclaimed scene is [`Scene::reset`] before being handed out —
    /// clearing its commands while keeping the backing `Vec` capacity, which
    /// is the whole point of reusing it over `Scene::new()`.
    fn take_reusable_scene(&mut self) -> Scene {
        if let Some(spare) = self.spare_scene.take() {
            return spare;
        }
        if let Some(mut returned) = self.scene_return.try_recv() {
            returned.reset();
            return returned;
        }
        Scene::new()
    }

    /// Hand one finished frame to the render thread (latest-wins).
    fn submit_frame(
        &mut self,
        painted: PaintedScene,
        ui: UiSpans,
        frame_time: FrameTime,
        size: SurfaceSize,
    ) {
        self.frame_id += 1;
        if let Some(sender) = self.sender.as_ref() {
            let stale = sender.send_scene(SceneFrame {
                scene: painted,
                meta: FrameMeta {
                    frame_time,
                    size,
                    frame_id: self.frame_id,
                },
                ui_spans: ui,
            });
            // The UI thread outran the render thread: the just-overwritten,
            // never-rendered stale frame's scene is still perfectly reusable —
            // reclaim its buffer instead of letting it drop.
            if let Some(stale_frame) = stale {
                let mut reclaimed = stale_frame.scene.scene;
                reclaimed.reset();
                self.spare_scene = Some(reclaimed);
            }
        }
    }

    /// Send a [`RenderCommand::SurfaceChanged`] (the layer-owned in-place resize),
    /// fire-and-forget.
    pub(super) fn resize(&mut self, size: SurfaceSize) {
        if let Some(sender) = self.sender.as_ref() {
            sender.send_command(RenderCommand::SurfaceChanged { size });
        }
    }

    /// The **iOS backgrounding barrier**: send a
    /// barriered [`RenderCommand::Pause`] and **block** until the render thread
    /// has acknowledged it. The render loop processes the `Pause` only after any
    /// in-flight frame's submit completes, moves its [`RenderPhase`] to `Paused`
    /// (so any scene still in the latest-wins slot is dropped, not submitted), and
    /// only *then* acks — so when this returns, the render thread is guaranteed to
    /// be parked, submitting no more Metal work. The caller (`frust_pause`) must
    /// not let the app background until this returns: Metal submission from a
    /// suspended app can get the process killed.
    pub(super) fn pause_barrier(&mut self) {
        if let Some(sender) = self.sender.as_ref() {
            // Bounded so a wedged render thread degrades instead of blocking the
            // main thread past the backgrounding watchdog (see
            // `PAUSE_BARRIER_DEADLINE`).
            if !sender.pause().wait_timeout(PAUSE_BARRIER_DEADLINE) {
                log::error!(
                    "frust-shell-ios: pause barrier timed out after \
                     {PAUSE_BARRIER_DEADLINE:?}; backgrounding (degraded)"
                );
            }
        }
    }

    /// Send a fire-and-forget [`RenderCommand::Resume`]: the render thread returns
    /// to `Active` and resumes submitting handed-off scenes.
    pub(super) fn resume(&mut self) {
        if let Some(sender) = self.sender.as_ref() {
            sender.send_command(RenderCommand::Resume);
        }
    }
}

impl Drop for SplitExecutor {
    fn drop(&mut self) {
        // Present-sync teardown: release any parked, unpresented frame
        // FIRST, so the drawable it holds is gone before the render thread (and
        // with it the surface it came from) is torn down below. Presenting it
        // here would be wrong — `frust_destroy` runs as the view goes away.
        self.present.clear();
        // Destroy-join ordering: drop the sender first
        // — that signals the render loop's `wait_next` to wake with a
        // disconnection and exit, dropping its `SurfaceRenderer` (and the
        // `wgpu::Surface` built from the retained `CAMetalLayer` pointer). Then
        // join, so the surface is fully torn down BEFORE this returns — and, since
        // this runs inside `frust_destroy` (which drops the handle), before
        // `frust_destroy` returns and Swift releases the layer. So the render
        // thread can never touch the layer after Swift frees it.
        self.sender.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl FrameExecutor {
    /// Whether a live surface exists — the [`crate::app::IosAppHandle::frame`]
    /// not-ready gate.
    /// Inline reads the renderer's phase directly; the split tracks a UI-side
    /// `surface_active` flag (the render thread owns the real phase).
    pub(super) fn has_surface(&self) -> bool {
        match self {
            FrameExecutor::Inline(inline) => inline.renderer.phase() == SurfacePhase::SurfaceReady,
            FrameExecutor::Split(split) => split.surface_active,
        }
    }

    /// The running count of frames the render side has actually presented
    /// (`FrameOutcome::Rendered`). The UI thread loads this once per frame and
    /// pushes it into `AppTree::set_presented_frames` before paint, so a widget
    /// measuring FPS reports the presented rate — under the split, below the
    /// `CADisplayLink` paint cadence. Both variants share the counter
    /// with their render side via an `Arc<AtomicU64>`.
    pub(super) fn presented_frames(&self) -> u64 {
        match self {
            FrameExecutor::Inline(inline) => inline.presented.load(Ordering::Relaxed),
            FrameExecutor::Split(split) => split.presented.load(Ordering::Relaxed),
        }
    }

    /// The id of the last frame **handed to** the render side — the release
    /// gate's submission cursor, against which a batch whose own frame was
    /// dropped by the depth-1 latest-wins channel is declared stale
    /// (`MAX_FRAMES_IN_FLIGHT`). The next frame to be submitted is therefore
    /// this + 1, which is the id a batch ingested during that frame's paint is
    /// paired with.
    ///
    /// `0` on the inline path, which stamps no frame ids — harmless, because
    /// the gate that reads this is only ever armed in the split (see
    /// `IosAppHandle::present_sync`).
    pub(super) fn submitted_frame_id(&self) -> u64 {
        match self {
            FrameExecutor::Inline(_) => 0,
            FrameExecutor::Split(split) => split.frame_id,
        }
    }

    /// Take (and clear) the render thread's surface-self-heal signal — `true`
    /// exactly once per render-side surface (re)install attempt (see
    /// [`SplitExecutor::surface_reinstalled`] for why iOS needs it and
    /// Android does not). Always `false` on the inline path, which recovers
    /// UI-side instead.
    ///
    /// `swap` rather than a load: the signal must be consumed by the one frame
    /// it forces to run, or a single self-heal would keep forcing `Run` forever
    /// — the very defect this fix exists to close.
    pub(super) fn take_surface_reinstalled(&self) -> bool {
        match self {
            FrameExecutor::Inline(_) => false,
            FrameExecutor::Split(split) => split.surface_reinstalled.swap(false, Ordering::AcqRel),
        }
    }

    /// Record the `first_rebuild_done` startup milestone after the initial
    /// rebuild. Inline records it here on the UI thread; the split records it
    /// render-side when the first scene arrives (see [`crate::ffi_glue::render_loop`]),
    /// so this is a no-op there.
    pub(super) fn record_first_rebuild(&mut self) {
        if let FrameExecutor::Inline(inline) = self {
            inline.record_first_rebuild();
        }
    }

    /// Record a gate-skipped frame. Inline accumulates it in its UI-side
    /// `FrameStats`; the split sends **nothing** on a skip (the render thread is
    /// the single emitter and never sees skipped frames — split mode records no
    /// skip frames; known, logged for 11.E), so this is a no-op there.
    pub(super) fn record_skip(&mut self) {
        if let FrameExecutor::Inline(inline) = self {
            inline.record_skip();
        }
    }

    /// Hand one finished frame to the executor. Inline runs the encode→present
    /// tail synchronously (borrowing `scene`, reused next frame) and returns its
    /// encode span; the split moves the scene out into a [`SceneFrame`] and sends
    /// it across the channel, replacing it with a scene reclaimed off the render
    /// thread's give-back channel — `reset()`, so its buffer
    /// is reused rather than reallocated — falling back to `Scene::new()` only
    /// when none is available yet, and returning `Duration::ZERO` (encode is
    /// off-thread, so it does not count against the UI thread's deadline).
    pub(super) fn submit_frame(
        &mut self,
        scene: &mut Scene,
        base_color: peniko::Color,
        ui: UiSpans,
        frame_time: FrameTime,
        size: SurfaceSize,
        perf_on: bool,
    ) -> Duration {
        match self {
            FrameExecutor::Inline(inline) => inline.submit_frame(scene, base_color, ui, perf_on),
            FrameExecutor::Split(split) => {
                let replacement = split.take_reusable_scene();
                let painted = PaintedScene {
                    scene: std::mem::replace(scene, replacement),
                    base_color,
                };
                split.submit_frame(painted, ui, frame_time, size);
                Duration::ZERO
            }
        }
    }
}

/// Feed the render thread's pacing trace one frame's spans — a no-op unless
/// both `perf-trace` and the [`pace_trace`] dial are on, and unconditionally
/// nothing in a build without the feature (the whole module, its strings
/// included, is compiled out).
///
/// Takes the `Option<Instant>`s the frame path already holds rather than
/// reading a clock of its own: with `perf_on` false they are `None` and there
/// is nothing to report, which is the same answer the dial would give.
///
/// `presented` is whether this frame actually presented
/// (`FrameOutcome::Rendered`) — see [`pace_trace`]'s `write_line` doc for why
/// `p2p_us`'s base may only ever advance on such a frame.
#[cfg(feature = "perf-trace")]
fn pace_record(
    wake: Option<Instant>,
    acquire_start: Option<Instant>,
    acquire: Duration,
    submit_start: Option<Instant>,
    submit: Duration,
    presented: bool,
) {
    if !pace_trace::enabled() {
        return;
    }
    if let (Some(wake), Some(acquire_start), Some(submit_start)) =
        (wake, acquire_start, submit_start)
    {
        pace_trace::record(
            wake,
            acquire_start,
            acquire,
            submit_start,
            submit,
            presented,
        );
    }
}

/// The `perf-trace`-off arm of [`pace_record`]: an inlinable no-op, so the
/// call site folds away with the rest of the perf route (see
/// `frust_shell_common::perf::enabled`'s feature-off rationale).
#[cfg(not(feature = "perf-trace"))]
#[inline]
fn pace_record(
    _wake: Option<Instant>,
    _acquire_start: Option<Instant>,
    _acquire: Duration,
    _submit_start: Option<Instant>,
    _submit: Duration,
    _presented: bool,
) {
}

/// This surface's most recent real GPU pass timing, folded into the
/// [`GpuPasses`] shape [`FramePasses::with_gpu`] takes, or `None` when the
/// surface produces no such measurement (a device that never offered
/// `TIMESTAMP_QUERY`, or simply no reading landed yet — see [`SurfaceRenderer::gpu_pass_timings`]).
///
/// Shared by [`render_scene`] and [`InlineExecutor::record_skip`] below, the
/// two frame-record sites in this module.
fn gpu_passes(renderer: &SurfaceRenderer) -> Option<GpuPasses> {
    renderer.gpu_pass_timings().map(|timings| GpuPasses {
        prepass: timings.prepass,
        main: timings.main,
        composite: timings.composite,
        blit: timings.blit,
    })
}

/// Run the encode→acquire→submit tail for one painted `scene`, timing each span
/// behind `perf_on` (the FFI-path perf convention: zero clock reads when
/// disabled), recording the folded [`FramePasses`] through the single emitter
/// (`frame_stats`), and stamping the first-encode / first-frame startup milestones
/// through `startup_spans`. Returns the encode span and whether this call just
/// retired the startup recorder (see
/// [`crate::ffi_glue::StartupRecorder::take_and_emit`]). Shared by the inline
/// path (UI thread) and the split path's [`crate::ffi_glue::render_loop`]
/// (render thread) so the per-frame render logic is not forked — the exact
/// pre-split tail, only relocated.
///
/// `startup_spans` is a [`crate::ffi_glue::StartupRecorder`] rather than a bare
/// `Option<StartupSpans>` so the split's shared-with-the-UI-thread variant can
/// keep its lock scoped to each individual record/take call — never held
/// across this function's own blocking GPU tail (acquire + submit) below. See
/// that type's docs for the hazard this avoids.
///
/// # Autorelease pool
///
/// The swapchain **acquire** (`nextDrawable`) and Metal command submission happen
/// on *this* thread — in the split that is the dedicated render thread, which has
/// no UIKit runloop draining an autorelease pool each iteration. So the whole GPU
/// tail is wrapped in an explicit [`autoreleasepool`] drain: without it every
/// frame's autoreleased `CAMetalDrawable` (and other Metal temporaries) would
/// accumulate and exhaust the layer's small drawable pool — a hard stall. On the
/// inline path (the UIKit main thread) this nests inside the runloop's own pool,
/// which is harmless — the iOS-specific hazard the split adds.
///
/// # Present-sync
///
/// `present` is the render side of the [`PresentHandoff`] slot, paired with the
/// id of the frame being rendered (`None` on the inline path, which needs no
/// deferral). When it is `Some` **and armed**, the submit step becomes
/// `submit_deferred`: the frame's GPU work is submitted here as usual, but its
/// `[drawable present]` is parked — under its own frame id — for the UI thread
/// to issue inside the platform-view transaction. The
/// `presented` counter and the `first_frame_presented` startup span are still
/// stamped here, so on that path both run up to one display-link tick ahead of
/// the actual present — a constant offset that leaves the *rate* those values
/// report unchanged (iOS uses the counter for FPS reporting only; the frame-id
/// gate that needs a true presented count is Android-only).
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_scene(
    renderer: &mut SurfaceRenderer,
    render_cx: &RenderContext,
    scene: &Scene,
    base_color: peniko::Color,
    ui: UiSpans,
    frame_stats: &mut FrameStats,
    mut startup_spans: crate::ffi_glue::StartupRecorder<'_>,
    perf_on: bool,
    presented: &AtomicU64,
    present: Option<(&PresentHandoff, u64)>,
) -> (Duration, bool) {
    autoreleasepool(|_pool| {
        // Encode span (GPU/CPU encode, no swapchain touch).
        let encode_start = perf_on.then(Instant::now);
        let encode_outcome = renderer.encode(render_cx, scene, base_color);
        let encode_time = encode_start.map_or(Duration::ZERO, |t| t.elapsed());

        // First-frame decomposition: stamp the first encode-complete boundary
        // once (only when something was actually encoded). `record_once`
        // locks (for the split's `Shared` variant) only for this call's own
        // short body — never across the acquire/submit tail below.
        if matches!(encode_outcome, Ok(EncodeOutcome::Encoded)) {
            startup_spans.record_once(perf::SPAN_FIRST_ENCODE_DONE);
        }

        // Acquire span (blocking vsync/present wait — `nextDrawable`).
        let acquire_start = perf_on.then(Instant::now);
        let acquire_result = match encode_outcome {
            Ok(EncodeOutcome::Encoded) => renderer.acquire(render_cx),
            Ok(EncodeOutcome::Skipped) => Ok(AcquireOutcome::Skipped),
            Err(err) => Err(err),
        };
        let acquire_time = acquire_start.map_or(Duration::ZERO, |t| t.elapsed());

        // Submit span (blit + queue-submit + present — the present itself is
        // deferred to the UI thread under present-sync, see this fn's docs).
        let submit_start = perf_on.then(Instant::now);
        let render_result = match acquire_result {
            Ok(AcquireOutcome::Acquired) => {
                match present.filter(|(handoff, _)| handoff.is_armed()) {
                    Some((handoff, frame_id)) => {
                        renderer.submit_deferred(render_cx).map(|(outcome, frame)| {
                            if let Some(frame) = frame {
                                handoff.store(frame_id, frame);
                            }
                            outcome
                        })
                    }
                    None => renderer.submit(render_cx),
                }
            }
            Ok(AcquireOutcome::Reconfigured) => Ok(FrameOutcome::Redraw),
            Ok(AcquireOutcome::Lost) => Ok(FrameOutcome::SurfaceLost),
            Ok(AcquireOutcome::Skipped) => Ok(FrameOutcome::Skipped),
            Err(err) => Err(err),
        };
        let submit_time = submit_start.map_or(Duration::ZERO, |t| t.elapsed());

        let mut startup_retired = false;
        let mut presented_this_frame = false;
        match render_result {
            // Stale swapchain (e.g. mid-rotation): reconfigured internally; the
            // next tick draws against the fresh configuration.
            Ok(FrameOutcome::Redraw) => {}
            // Surface lost: dropped by the machine. The render loop self-heals from
            // the retained `CAMetalLayer` pointer on this same wakeup (split); the
            // inline path recovers on the next FFI entry (see `ffi_glue`).
            Ok(FrameOutcome::SurfaceLost) => {
                log::warn!("frust-shell-ios: surface lost; recreating");
            }
            Ok(FrameOutcome::Rendered) => {
                presented_this_frame = true;
                // A presented frame: bump the shared counter the UI thread reads
                // before paint. `Skipped` presents nothing, so it doesn't.
                presented.fetch_add(1, Ordering::Relaxed);
                // First successful present: close out the cold-start recorder
                // once. `take_and_emit` locks (for `Shared`) only for the
                // `take()` itself — the record+emit that follow run on the
                // now-fully-owned local value, off any mutex entirely.
                startup_retired = startup_spans.take_and_emit(perf::SPAN_FIRST_FRAME_PRESENTED);
            }
            Ok(FrameOutcome::Skipped) => {}
            Err(err) => log::error!("frust-shell-ios: render error: {err:#}"),
        }

        // The pacing trace's own line, after the outcome match so it knows
        // whether this frame presented (see [`pace_trace`]'s `write_line` doc
        // for why `p2p_us`'s base may only advance then) — the durations it
        // reports are the same ones measured above regardless of where in
        // this function the call sits. `encode_start` doubles as the frame's
        // wake instant — it is read at the top of this tail, the moment the
        // render thread has a scene.
        pace_record(
            encode_start,
            acquire_start,
            acquire_time,
            submit_start,
            submit_time,
            presented_this_frame,
        );

        // One folded frame record through the single emitter, with this
        // surface's real GPU pass timing attached when it produces one
        // (engine tier, `perf-trace`, a device that offered `TIMESTAMP_QUERY`).
        let mut passes = FramePasses::from_split(
            ui,
            RenderSpans {
                encode: encode_time,
                acquire: acquire_time,
                submit: submit_time,
            },
        );
        if let Some(gpu) = gpu_passes(renderer) {
            passes = passes.with_gpu(gpu);
        }
        frame_stats.record(passes);
        if frame_stats.should_emit() {
            frame_stats.emit_log();
        }

        (encode_time, startup_retired)
    })
}
