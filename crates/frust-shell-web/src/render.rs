//! The browser shell's frame executor — everything after a finished
//! [`Scene`] exists: the surface's own lifecycle plus the
//! encode → acquire → submit tail, run synchronously inside the
//! `RedrawRequested` turn that produced the scene.
//!
//! # There is no render thread here, and there cannot be one
//!
//! The desktop core offers two frame executors and defaults to the split one:
//! a dedicated thread owns the `RenderContext`/`SurfaceRenderer` and the UI
//! thread hands it finished scenes across a channel. `wasm32-unknown-unknown`
//! has no OS threads, and `wgpu`'s handle types are not `Send`/`Sync` there at
//! all (its `build.rs` marks them so only on native; `frust-gpu` turns on
//! `fragile-send-sync-non-atomic-wasm` purely so its *own* threaded pipeline
//! warm-up type-checks, and that spawn then fails at runtime and falls back to
//! the inline path). So the browser is a first-class consumer of what desktop
//! keeps as a fallback: the single-thread inline executor. This module is that
//! path, and it deliberately mirrors `frust-shell-desktop`'s `InlineExecutor`
//! and its `render_frame` tail so the two do not drift.
//!
//! Three things the desktop inline executor does are absent by construction:
//!
//! * **No `pollster::block_on`.** `SurfaceRenderer::on_surface_created` is
//!   `async` and the browser has no blocking executor to drive it with —
//!   blocking the one JS thread is not a thing a page may do. Bring-up is
//!   `async` here all the way out to the caller, which drives it on the
//!   browser's own microtask queue.
//! * **No pipeline-cache I/O.** `frust-paths` writes files; a page has no
//!   filesystem. The renderer is left on its empty initial cache.
//! * **No `Arc<AtomicU64>` presented-frame counter.** The desktop shape exists
//!   to share that counter with a render thread. There is one thread here, so
//!   the counter is a plain field.
//!
//! # Cold-page adapter acquisition
//!
//! `RenderContext` asks for an adapter exactly once. On a freshly loaded
//! Chrome page the *first* `requestAdapter()` can answer `null` on a machine
//! that has a perfectly good adapter — a GPU-process warm-up race, not a
//! capability answer, reproduced directly over the devtools protocol by the
//! browser render probe under `examples/web-spike` (see its `RESULTS.md`).
//! A single-shot bring-up therefore reports a hard, permanent "no compatible
//! GPU adapter" for a machine that has one. [`WebFrameExecutor::ensure_surface_with_retry`]
//! closes that with a bounded retry whose every attempt is logged verbatim, so
//! a genuine refusal still reads as a refusal rather than being disguised as a
//! slow one.

use std::sync::Arc;

use frust_render::{
    AcquireOutcome, EncodeOutcome, FrameOutcome, RenderContext, SurfaceAlphaRequest, SurfacePhase,
    SurfaceRenderer,
};
use frust_scene::Scene;
use frust_shell_common::perf::{FramePasses, FrameStats, RenderSpans, UiSpans};
use web_time::{Duration, Instant};
use winit::window::Window;

/// How many times [`WebFrameExecutor::ensure_surface_with_retry`] asks for a
/// surface before reporting the failure as real.
///
/// Eight, spaced by [`BRINGUP_RETRY_INTERVAL`] — a ~1.2 s ceiling. Sized off
/// the browser render probe's measurement rather than guessed: a cold Chrome
/// page needed **two** attempts on the WebGPU arm and one on the WebGL2 arm,
/// so the budget is a wide margin over the observed cost, and short enough
/// that a machine with genuinely no adapter still fails inside a page load.
pub const BRINGUP_ATTEMPTS: u32 = 8;

/// How long to wait between surface bring-up attempts.
///
/// **Community-approximate**: Chrome publishes no timing for its GPU-process
/// warm-up, so this is the value the browser render probe measured as
/// sufficient (the second attempt succeeded at 150 ms every run), not a
/// documented deadline.
pub const BRINGUP_RETRY_INTERVAL: Duration = Duration::from_millis(150);

/// What the shell owes the loop after one frame — the browser-side reading of
/// [`FrameOutcome`], separated from the executor so the mapping is testable
/// without a GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFollowUp {
    /// Nothing further: the frame presented, or it was skipped because no
    /// renderable surface exists.
    Idle,
    /// Ask for another frame immediately — the swapchain was stale and has
    /// been reconfigured internally, so the scene must be drawn again against
    /// the fresh configuration.
    Redraw,
    /// The surface was lost and dropped; the shell must bring a new one up
    /// before it can draw again.
    RecreateSurface,
}

/// Read one [`FrameOutcome`] as the follow-up the loop owes.
///
/// A pure function rather than a `match` inside the executor so the browser's
/// recovery policy is assertable on the build host — the recovery paths
/// (`Redraw`, `SurfaceLost`) are precisely the ones a browser exercises most
/// and a GPU test can least easily provoke.
pub fn follow_up_for(outcome: FrameOutcome) -> FrameFollowUp {
    match outcome {
        FrameOutcome::Redraw => FrameFollowUp::Redraw,
        FrameOutcome::SurfaceLost => FrameFollowUp::RecreateSurface,
        FrameOutcome::Rendered | FrameOutcome::Skipped => FrameFollowUp::Idle,
    }
}

/// How long to wait before bring-up attempt number `attempt` (1-based) is
/// retried, or `None` when `attempt` was the last one the budget allows.
///
/// Split out of the retry loop so the budget is checkable without a browser:
/// a constant interval, not a backoff, because the failure it works around is
/// a fixed one-time process warm-up rather than contention that gets worse
/// under load.
pub fn bringup_retry_delay(attempt: u32) -> Option<Duration> {
    (attempt < BRINGUP_ATTEMPTS).then_some(BRINGUP_RETRY_INTERVAL)
}

/// The browser's frame executor: the `RenderContext`/`SurfaceRenderer` pair and
/// the per-frame timing recorder, all on the one thread a page has.
pub struct WebFrameExecutor {
    render_cx: RenderContext,
    renderer: SurfaceRenderer,
    frame_stats: FrameStats,
    /// Running count of frames actually presented (`FrameOutcome::Rendered`),
    /// pushed into `RenderRoot::set_presented_frames` before each paint so a
    /// widget measuring FPS reports the presented rate rather than its own
    /// paint cadence. A plain `u64`, not the desktop split's
    /// `Arc<AtomicU64>` — bumped and read on the same (only) thread.
    presented: u64,
    /// The physical size the live swapchain is configured for, or `None`
    /// before a surface exists — the change gate behind [`Self::ensure_size`].
    configured: Option<(u32, u32)>,
}

impl WebFrameExecutor {
    /// Construct the executor. Creates the `wgpu` instance; no adapter, device
    /// or surface exists until [`Self::ensure_surface`] runs.
    pub fn new() -> Self {
        Self {
            render_cx: RenderContext::new(),
            renderer: SurfaceRenderer::new(),
            frame_stats: FrameStats::new(),
            presented: 0,
            configured: None,
        }
    }

    /// Whether a configured surface is live and frames can be drawn.
    pub fn has_surface(&self) -> bool {
        self.renderer.phase() == SurfacePhase::SurfaceReady
    }

    /// The running count of presented frames — see [`Self::presented`].
    pub fn presented_frames(&self) -> u64 {
        self.presented
    }

    /// Bring a surface online over `window`'s canvas at `width` x `height`
    /// **physical** (device) pixels, in one attempt.
    ///
    /// The error is flattened to a `String` rather than propagated as its own
    /// type: every caller's only recourse is to report it (a browser console
    /// line, a retry log), and flattening here keeps `anyhow` out of this
    /// crate's dependency set for a value nothing matches on.
    pub async fn ensure_surface(
        &mut self,
        window: Arc<Window>,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        self.renderer
            .on_surface_created(
                &mut self.render_cx,
                window,
                width.max(1),
                height.max(1),
                // Opaque: the canvas composites against the page, and an
                // alpha-blended swapchain would let page background bleed
                // through every un-opaque pixel the app paints. A
                // deliberately translucent host canvas is a host-page
                // decision this shell does not have a seam for yet.
                SurfaceAlphaRequest::Opaque,
            )
            .await
            .map_err(|err| format!("{err:#}"))?;
        self.configured = Some((width.max(1), height.max(1)));
        Ok(())
    }

    /// Reconcile the swapchain against the canvas's *current* physical size,
    /// reconfiguring only on an actual change.
    ///
    /// Called once per frame, and load-bearing on this host in a way it would
    /// not be on a desktop one. winit's web backend reports
    /// `Window::inner_size()` as **0x0** until its `ResizeObserver` has
    /// delivered a first observation of the canvas — which is *after* the
    /// `resumed` that creates the window, and typically after the
    /// asynchronous surface bring-up has already started. So the surface's
    /// first configuration is necessarily made against a size that is not yet
    /// the truth, and a `Resized` arriving while the bring-up future is still
    /// in flight has no surface to apply itself to. Reconciling every frame
    /// makes the wrong first size self-correcting instead of permanent (the
    /// symptom otherwise is a 1x1 swapchain and a blank canvas for the life of
    /// the page).
    ///
    /// A zero dimension is ignored rather than clamped, so an unmeasured
    /// canvas never latches as "configured" and the first real measurement
    /// still counts as a change.
    pub fn ensure_size(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || self.configured == Some((width, height)) {
            return;
        }
        self.resize_surface(width, height);
    }

    /// [`Self::ensure_surface`] with the cold-page adapter retry — the entry
    /// point the browser event loop actually calls. Answers the attempt number
    /// that succeeded, so the shell can log how cold the page was.
    ///
    /// Every failed attempt is logged at `warn` with its verbatim error, so a
    /// genuine refusal (no WebGPU, no WebGL2, a blocklisted GPU) shows the same
    /// message [`BRINGUP_ATTEMPTS`] times and the final error names which
    /// attempt gave up — the retry can never disguise a refusal as slowness.
    ///
    /// `wasm32`-only: the wait between attempts is a browser timer, and there
    /// is no non-browser caller — a native host of this crate would have no
    /// canvas to bring a surface up over in the first place.
    #[cfg(target_arch = "wasm32")]
    pub async fn ensure_surface_with_retry(
        &mut self,
        window: Arc<Window>,
        width: u32,
        height: u32,
    ) -> Result<u32, String> {
        let mut last = String::new();
        for attempt in 1..=BRINGUP_ATTEMPTS {
            match self
                .ensure_surface(Arc::clone(&window), width, height)
                .await
            {
                Ok(()) => return Ok(attempt),
                Err(err) => {
                    log::warn!(
                        "frust: surface bring-up attempt {attempt}/{BRINGUP_ATTEMPTS} failed: {err}"
                    );
                    last = err;
                    match bringup_retry_delay(attempt) {
                        Some(delay) => sleep(delay).await,
                        None => break,
                    }
                }
            }
        }
        Err(format!(
            "frust: gave up bringing the render surface up after {BRINGUP_ATTEMPTS} \
             attempts; last error: {last}"
        ))
    }

    /// Reconfigure the swapchain for a new **physical** size, unconditionally.
    /// [`Self::ensure_size`] is the change-gated form the frame path uses.
    pub fn resize_surface(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        self.renderer
            .on_surface_changed(&self.render_cx, width, height);
        self.configured = Some((width, height));
    }

    /// The physical size the live swapchain is configured for, or `None`
    /// before a surface exists.
    pub fn configured_size(&self) -> Option<(u32, u32)> {
        self.configured
    }

    /// Drop the surface's GPU resources, leaving the executor in
    /// `SurfacePhase::NoSurface`.
    pub fn destroy_surface(&mut self) {
        self.renderer.on_surface_destroyed();
        self.configured = None;
    }

    /// Run one frame's encode → acquire → submit tail for an already painted
    /// `scene`, record the folded frame timing, and answer what the loop owes
    /// next.
    ///
    /// A render error is log-and-continue, never fatal: one bad frame must not
    /// kill a page.
    pub fn submit_frame(
        &mut self,
        scene: &Scene,
        base_color: peniko::Color,
        ui_spans: UiSpans,
    ) -> FrameFollowUp {
        match render_frame(
            &mut self.renderer,
            &self.render_cx,
            scene,
            base_color,
            ui_spans,
            &mut self.frame_stats,
        ) {
            Ok(FrameOutcome::Rendered) => {
                self.presented = self.presented.saturating_add(1);
                FrameFollowUp::Idle
            }
            Ok(outcome) => follow_up_for(outcome),
            Err(err) => {
                log::error!("frust: render error: {err}");
                FrameFollowUp::Idle
            }
        }
    }
}

impl Default for WebFrameExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Run the encode → acquire → submit tail for one painted `scene`, timing each
/// span with its own clock read and recording the folded [`FramePasses`]
/// through the frame-stats emitter.
///
/// The three-phase shape is the desktop core's verbatim, and for the same
/// reason: `encode` is a scene hand-off with no swapchain touch, `acquire` is
/// the blocking present/vsync wait, and `submit` carries the engine's real
/// per-frame CPU work plus the queue submit — three spans that answer three
/// different questions and are useless folded together.
///
/// **No GPU pass timings are attached.** `FramePasses::with_gpu` wants a
/// `TIMESTAMP_QUERY` reading, and no shipping browser exposes that WebGPU
/// feature to a page by default, so `SurfaceRenderer::gpu_pass_timings` has
/// nothing to answer with here and the CPU spans below are the whole picture.
fn render_frame(
    renderer: &mut SurfaceRenderer,
    render_cx: &RenderContext,
    scene: &Scene,
    base_color: peniko::Color,
    ui_spans: UiSpans,
    frame_stats: &mut FrameStats,
) -> Result<FrameOutcome, String> {
    // Deliberately no `Window::pre_present_notify` before the tail: it is a
    // hint to a native compositor about an imminent buffer swap, and the
    // browser's compositor is driven by the `requestAnimationFrame` callback
    // this whole turn already runs inside. winit's web backend implements it
    // as a no-op.
    let encode_start = Instant::now();
    let encode_outcome = renderer.encode(render_cx, scene, base_color);
    let encode = encode_start.elapsed();

    let acquire_start = Instant::now();
    let acquire_outcome = match encode_outcome {
        Ok(EncodeOutcome::Encoded) => renderer.acquire(render_cx),
        // Nothing encoded (no renderable surface): surface the same `Skipped`
        // the combined path would have, without acquiring.
        Ok(EncodeOutcome::Skipped) => Ok(AcquireOutcome::Skipped),
        Err(err) => Err(err),
    };
    let acquire = acquire_start.elapsed();

    let submit_start = Instant::now();
    let render_outcome = match acquire_outcome {
        Ok(AcquireOutcome::Acquired) => renderer.submit(render_cx),
        Ok(AcquireOutcome::Reconfigured) => Ok(FrameOutcome::Redraw),
        Ok(AcquireOutcome::Lost) => Ok(FrameOutcome::SurfaceLost),
        Ok(AcquireOutcome::Skipped) => Ok(FrameOutcome::Skipped),
        Err(err) => Err(err),
    };
    let submit = submit_start.elapsed();

    frame_stats.record(FramePasses::from_split(
        ui_spans,
        RenderSpans {
            encode,
            acquire,
            submit,
        },
    ));
    if frame_stats.should_emit() {
        frame_stats.emit_log();
    }

    // Flattened to a `String` for the same reason `ensure_surface`'s is: the
    // caller's only recourse is to log it, and flattening at this one seam
    // keeps `anyhow` out of this crate's dependency set for a value nothing
    // ever matches on.
    render_outcome.map_err(|err| format!("{err:#}"))
}

/// Awaits `duration` of wall-clock time on the browser's own timer.
///
/// `std::thread::sleep` does not exist on this target and nothing in the graph
/// carries a timer future, so this is the `setTimeout`-into-`Promise`-into-
/// `JsFuture` idiom spelled out. Its only caller is the surface bring-up
/// retry; a `setTimeout` chain of at most [`BRINGUP_ATTEMPTS`] hops during page
/// start is nowhere near the nesting depth that triggers a browser's timer
/// clamp, so the interval it waits is the interval it asked for.
#[cfg(target_arch = "wasm32")]
async fn sleep(duration: Duration) {
    let millis = i32::try_from(duration.as_millis()).unwrap_or(i32::MAX);
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, millis);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_presented_or_skipped_frame_owes_the_loop_nothing() {
        assert_eq!(follow_up_for(FrameOutcome::Rendered), FrameFollowUp::Idle);
        assert_eq!(follow_up_for(FrameOutcome::Skipped), FrameFollowUp::Idle);
    }

    #[test]
    fn a_stale_swapchain_owes_an_immediate_redraw() {
        assert_eq!(follow_up_for(FrameOutcome::Redraw), FrameFollowUp::Redraw);
    }

    #[test]
    fn a_lost_surface_owes_a_rebuild_of_the_surface_not_just_a_redraw() {
        // Distinguishing these two is the whole point of the mapping: asking
        // for a redraw against a dropped surface would spin the loop against
        // a renderer that can only answer `Skipped`.
        assert_eq!(
            follow_up_for(FrameOutcome::SurfaceLost),
            FrameFollowUp::RecreateSurface
        );
    }

    #[test]
    fn every_attempt_but_the_last_is_followed_by_a_wait() {
        for attempt in 1..BRINGUP_ATTEMPTS {
            assert_eq!(
                bringup_retry_delay(attempt),
                Some(BRINGUP_RETRY_INTERVAL),
                "attempt {attempt} is not the last, so it must be retried"
            );
        }
    }

    #[test]
    fn the_last_attempt_is_not_followed_by_a_wait() {
        assert_eq!(bringup_retry_delay(BRINGUP_ATTEMPTS), None);
        assert_eq!(
            bringup_retry_delay(BRINGUP_ATTEMPTS + 1),
            None,
            "an out-of-budget attempt number never re-arms the loop"
        );
    }

    #[test]
    fn the_retry_budget_fits_inside_a_page_load() {
        // The bound the constants promise: a machine with genuinely no
        // adapter must report that failure while the page is still starting,
        // not after a multi-second stall.
        let ceiling = BRINGUP_RETRY_INTERVAL * (BRINGUP_ATTEMPTS - 1);
        assert!(
            ceiling < Duration::from_secs(2),
            "bring-up may stall for at most ~1.2s, got {ceiling:?}"
        );
    }
}
