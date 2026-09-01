//! The shared render tail — encode → acquire → submit for one painted scene —
//! plus the signal slot the render side publishes its results through.
//!
//! Driven from both render paths: the inline executor calls it on the UI thread
//! and the split's [`crate::jni_glue::render_loop`] calls it on the render
//! thread, so the per-frame render logic is never forked.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use frust_render::{AcquireOutcome, EncodeOutcome, FrameOutcome, RenderContext, SurfaceRenderer};
use frust_scene::Scene;
use frust_shell_common::perf::{
    self, FramePasses, FrameStats, GpuPasses, RenderSpans, StartupSpans, UiSpans,
};

/// What the render side publishes about the frames it renders, read by the UI
/// thread — one instance shared by both sides through an `Arc`, the same
/// no-channel/no-protocol pattern as the `fatal` flag.
///
/// Three values, because they answer three different questions and none can
/// substitute for another:
///
/// - `count` — **how many** frames reached the screen, pushed into
///   `AppTree::set_presented_frames` so an FPS-measuring widget reports the
///   presented rate rather than the Choreographer's paint cadence.
/// - `frame_id` — **which** frame is on screen, the platform-view release
///   gate's input ([`AndroidAppHandle::platform_view_commands`](super::AndroidAppHandle::platform_view_commands)). The count
///   cannot answer this: the UI→render channel is depth-1 latest-wins, so
///   scenes the UI thread submitted are routinely overtaken and never rendered,
///   and the two clocks drift apart by exactly the dropped frames.
/// - `acquire_ewma_us` — **how deep the swapchain queue is**, as the wait the
///   render tail spends inside `acquire` (a quarter-weight EWMA in µs). This is
///   the regime discriminator the shape-aware scroll-sync tail switches on
///   ([`crate::sync_tail`]): acquire-bound means every frame is
///   equally late behind the same queue — a constant-shaped residual a hold can
///   close — while submit-bound means there is no queue to correct for. An
///   `Arc` field beside the two above, deliberately scoped to this instance
///   rather than a process-global.
#[derive(Debug, Default)]
pub(crate) struct RenderSignals {
    count: AtomicU64,
    frame_id: AtomicU64,
    acquire_ewma_us: AtomicU64,
}

impl RenderSignals {
    /// Record one presented frame. `fetch_max` on the id rather than `store`:
    /// ids only ever move forward, and a late write must never walk the release
    /// gate backwards.
    fn record_present(&self, frame_id: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.frame_id.fetch_max(frame_id, Ordering::Relaxed);
    }

    /// Fold one frame's acquire wait into the published EWMA. Called only for
    /// frames that actually acquired — a skipped frame's ~0 would drag the
    /// average toward zero exactly when the queue is idle-but-deep, which is the
    /// regime the tail exists to detect.
    fn record_acquire_wait(&self, micros: u64) {
        let previous = self.acquire_ewma_us.load(Ordering::Relaxed);
        // Quarter-weight, seeded by the first sample rather than ramping up
        // from zero (the regime must be readable within a few frames of a
        // scroll starting, not a few dozen).
        let next = if previous == 0 {
            micros
        } else {
            (3 * previous + micros) / 4
        };
        self.acquire_ewma_us.store(next, Ordering::Relaxed);
    }

    pub(super) fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    pub(super) fn frame_id(&self) -> u64 {
        self.frame_id.load(Ordering::Relaxed)
    }

    pub(super) fn acquire_ewma_us(&self) -> u64 {
        self.acquire_ewma_us.load(Ordering::Relaxed)
    }
}

/// This surface's most recent real GPU pass timing, folded into the
/// [`GpuPasses`] shape [`FramePasses::with_gpu`] takes, or `None` when the
/// surface produces no such measurement (every tier but the engine one, an
/// `engine-tier` build whose device never got `TIMESTAMP_QUERY`, or simply no
/// reading landed yet — see [`SurfaceRenderer::gpu_pass_timings`]).
///
/// `pub(crate)` so both this module's [`render_scene`] and
/// [`super::executor`]'s gate-skip record share the one conversion.
pub(crate) fn gpu_passes(renderer: &SurfaceRenderer) -> Option<GpuPasses> {
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
/// (`frame_stats`), and stamping the first-encode / first-frame startup
/// milestones on `startup_spans`. Returns the encode span. Shared by the inline
/// path (UI thread) and the split path's [`crate::jni_glue::render_loop`] (render
/// thread) so the per-frame render logic is not forked — the exact pre-split
/// tail, only relocated.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_scene(
    renderer: &mut SurfaceRenderer,
    render_cx: &RenderContext,
    scene: &Scene,
    base_color: peniko::Color,
    ui: UiSpans,
    frame_stats: &mut FrameStats,
    startup_spans: &mut Option<StartupSpans>,
    perf_on: bool,
    signals: &RenderSignals,
    // The id of the frame this scene came from (`FrameMeta::frame_id` in the
    // split, the inline executor's own counter otherwise) — published on an
    // actual present so a platform-view geometry batch can be paired back to
    // the frame that produced it (the release gate; see `RenderSignals`).
    frame_id: u64,
) -> Duration {
    // Encode span (GPU/CPU encode, no swapchain touch).
    let encode_start = perf_on.then(Instant::now);
    let encode_outcome = renderer.encode(render_cx, scene, base_color);
    let encode_time = encode_start.map_or(Duration::ZERO, |t| t.elapsed());

    // First-frame decomposition: stamp the first encode-complete
    // boundary once (only when something was actually encoded).
    if matches!(encode_outcome, Ok(EncodeOutcome::Encoded))
        && let Some(spans) = startup_spans.as_mut()
        && !spans
            .spans()
            .iter()
            .any(|(n, _)| *n == perf::SPAN_FIRST_ENCODE_DONE)
    {
        spans.record(perf::SPAN_FIRST_ENCODE_DONE);
    }

    // Acquire span (blocking vsync/present wait).
    //
    // The one clock read on this path that is NOT gated behind `perf_on` (the
    // module's perf convention — see `docs/CODE_STANDARDS.md`'s Instrumentation
    // conventions): the acquire wait is not instrumentation here, it is the
    // shape-aware scroll-sync tail's live regime input (`RenderSignals::
    // acquire_ewma_us`), which must be readable in a plain
    // profile/release build with tracing off. One `Instant` pair per rendered
    // frame on the render thread; the perf span itself still resolves to
    // `Duration::ZERO` when tracing is off, so nothing else changes.
    let encoded = matches!(encode_outcome, Ok(EncodeOutcome::Encoded));
    let acquire_start = Instant::now();
    let acquire_result = match encode_outcome {
        Ok(EncodeOutcome::Encoded) => renderer.acquire(render_cx),
        Ok(EncodeOutcome::Skipped) => Ok(AcquireOutcome::Skipped),
        Err(err) => Err(err),
    };
    let acquire_elapsed = acquire_start.elapsed();
    if encoded {
        signals.record_acquire_wait(acquire_elapsed.as_micros() as u64);
    }
    let acquire_time = if perf_on {
        acquire_elapsed
    } else {
        Duration::ZERO
    };

    // Submit span (blit + queue-submit + present).
    let submit_start = perf_on.then(Instant::now);
    let render_result = match acquire_result {
        Ok(AcquireOutcome::Acquired) => renderer.submit(render_cx),
        Ok(AcquireOutcome::Reconfigured) => Ok(FrameOutcome::Redraw),
        Ok(AcquireOutcome::Lost) => Ok(FrameOutcome::SurfaceLost),
        Ok(AcquireOutcome::Skipped) => Ok(FrameOutcome::Skipped),
        Err(err) => Err(err),
    };
    let submit_time = submit_start.map_or(Duration::ZERO, |t| t.elapsed());

    match render_result {
        // Stale swapchain (e.g. mid-rotation): reconfigured internally; the next
        // frame draws against the fresh configuration.
        Ok(FrameOutcome::Redraw) => {}
        // Surface lost: dropped by the machine; wait for surfaceChanged to
        // recreate it (Android pairs loss with a destroy/create cycle).
        Ok(FrameOutcome::SurfaceLost) => {
            log::warn!("frust-shell-android: surface lost; awaiting surfaceChanged");
        }
        Ok(FrameOutcome::Rendered) => {
            // A presented frame: bump the shared counter the UI thread reads
            // before paint and publish WHICH frame is now on screen
            // for the platform-view release gate. `Skipped` presents nothing,
            // so it does neither.
            signals.record_present(frame_id);
            // First successful present: close out the cold-start recorder once.
            if let Some(mut spans) = startup_spans.take() {
                spans.record(perf::SPAN_FIRST_FRAME_PRESENTED);
                spans.emit_log();
            }
        }
        Ok(FrameOutcome::Skipped) => {}
        Err(err) => log::error!("frust-shell-android: render error: {err:#}"),
    }

    // One folded frame record through the single emitter, with this surface's
    // real GPU pass timing attached when it produces one (engine tier,
    // `perf-trace`, a device that offered `TIMESTAMP_QUERY`).
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

    encode_time
}
