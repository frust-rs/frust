//! The desktop shell's frame executor — the render-path half of the frame loop,
//! split out of [`app_handler`](crate::app_handler)
//! so the two-thread and single-thread paths share one encode→present tail.
//!
//! # The split
//!
//! The UI thread keeps `rebuild → layout → paint`; this module owns everything
//! after the finished [`Scene`] exists. Which thread the encode/acquire/submit
//! work runs on is chosen once at startup by the
//! [`render_thread_enabled`](frust_shell_common::render_thread_enabled) kill
//! switch (`FRUST_NO_RENDER_THREAD`):
//!
//! - [`FrameExecutor::Split`] (default): a dedicated `frust-render` thread owns
//!   the [`RenderContext`] + [`SurfaceRenderer`] wholesale (both `Send` —
//!   *not* designed around any wgpu-`!Send` myth) and
//!   runs [`render_loop`]. The UI thread hands it finished frames across the
//!   depth-1 latest-wins [`render_channel`](frust_shell_common::render_channel)
//!   and drives surface lifecycle through owned [`RenderCommand`]s (with acks on
//!   the barriered ones). The render thread is the **single perf emitter**: it
//!   folds the UI thread's [`UiSpans`] together with its own [`RenderSpans`] via
//!   [`FramePasses::from_split`] and records the one frame.
//! - [`FrameExecutor::Inline`]: the pre-split fallback — the same
//!   [`RenderContext`]/[`SurfaceRenderer`] live on the UI thread and the
//!   encode→present tail runs synchronously inside `RedrawRequested`. Kept
//!   working (it is the fallback for comparison against the split); it shares
//!   [`render_frame`] and the pipeline-cache helpers with the split path so the
//!   frame-pipeline logic is not forked.
//!
//! # The macOS surface-creation seam
//!
//! winit only yields a window handle on the main thread, so the render thread
//! cannot create its own wgpu surface. The split therefore creates the surface
//! on the **UI thread** — via a [`SurfaceFactory`] cloned off the render
//! thread's context — and hands the resulting `Send` [`DetachedSurface`] (paired
//! with the [`Window`] the render thread needs for `pre_present_notify`) across
//! the channel as the [`RenderCommand::SurfaceCreated`] payload; the render
//! thread installs it with [`SurfaceRenderer::on_surface_installed`]. Everything
//! GPU-side (device, swapchain config, blitter, blit/acquire/present) stays on
//! the render thread. A full re-create after `SurfaceLost` needs the main thread
//! too, so the render thread routes a request back through the [`EventLoopProxy`]
//! ([`ShellUserEvent::RenderRecreateSurface`]) and the UI thread re-runs the
//! detached creation — keeping winit's dirty-driven `ControlFlow::Wait` model
//! (idle CPU near zero).
//!
//! # Overlay surfaces (macOS, `FRUST_OVERLAYS`)
//!
//! With the switch on, each frame carries the overlay cuts its scene was cut
//! at, and the render side draws it segmented onto the main surface plus one
//! `OverlaySurface` per published overlay layer (see the *Overlay surfaces*
//! section below). Building that surface from the `CAMetalLayer*` is this
//! crate's one `unsafe` block, confined to [`build_overlay_surface`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use frust_core::FrameTime;
use frust_render::{
    AcquireOutcome, DetachedSurface, EncodeOutcome, FrameOutcome, RenderContext,
    SurfaceAlphaRequest, SurfaceFactory, SurfacePhase, SurfaceRenderer,
};
use frust_scene::Scene;
use frust_shell_common::perf::{
    FramePasses, FrameStats, GpuPasses, RenderSpans, SPAN_ADAPTER_READY, SPAN_DEVICE_READY,
    SPAN_FIRST_ENCODE_DONE, SPAN_FIRST_FRAME_PRESENTED, SPAN_FIRST_REBUILD_DONE, SPAN_INIT_ENTRY,
    SPAN_PIPELINE_CACHE_RESTORED, SPAN_RENDERER_READY, StartupSpans, UiSpans,
};
use frust_shell_common::render_split::publish_resident_pages;
use frust_shell_common::{
    FrameMeta, RenderCommand, RenderPhase, RenderReceiver, RenderSender, SceneFrame,
    SceneReturnReceiver, SceneReturnSender, SurfaceSize, next_render_phase, render_channel,
    scene_return_channel,
};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

use crate::app_handler::ShellUserEvent;
use crate::platform_view::OverlayCutSpec;

/// A shared [`Window`] handle.
type WindowHandle = Arc<Window>;

/// Upper bound on how long the UI thread blocks on the surface-destroy barrier
/// (`suspended`) before proceeding degraded rather than hanging.
///
/// **Conservative**: desktop has no platform watchdog (unlike iOS backgrounding
/// / Android ANR), so this is a generous safety cap on a wedged render thread,
/// not a value matched to any published deadline. The barrier normally returns
/// in microseconds (drop the surface's `wgpu` resources); a multi-second wait
/// means the render thread is stuck, and hanging the winit event loop is worse
/// than a degraded teardown.
const DESTROY_SURFACE_BARRIER_DEADLINE: Duration = Duration::from_secs(5);

/// One finished frame's payload crossing the UI→render handoff: the painted
/// [`Scene`] plus the clear color it was painted for (the live theme's surface
/// color, which must ride *with* the frame so a mid-frame theme flip clears to
/// the right color). This is the `S` type parameter of
/// [`SceneFrame`]/[`render_channel`]; both `Scene` and `peniko::Color` are
/// `Send`, keeping the handoff `Send`-clean with no `unsafe`.
pub(crate) struct PaintedScene {
    scene: Scene,
    base_color: peniko::Color,
    /// The overlay cuts this very scene was cut at (empty with overlays off),
    /// so the render side never pairs one frame's ranges with another's
    /// commands.
    overlay_cuts: Vec<OverlayCutSpec>,
}

/// The `SurfaceCreated` payload the UI thread hands the render thread — the `W`
/// type parameter of the [`render_channel`] (the channel is generic over
/// `W` precisely so each shell picks its own window payload; mobile shells pass
/// their raw `ANativeWindow`/`CAMetalLayer` pointer instead). Desktop pairs the
/// [`DetachedSurface`] the UI thread created (installed render-side) with the
/// [`Window`] the render thread keeps only for `pre_present_notify`. Both halves
/// are `Send`.
pub(crate) struct DesktopSurface {
    detached: DetachedSurface,
    window: WindowHandle,
}

/// The desktop shell's frame executor: either the render-thread split
/// ([`Self::Split`]) or the single-thread fallback ([`Self::Inline`]). Chosen
/// once at startup from the [`render_thread_enabled`](frust_shell_common::render_thread_enabled)
/// kill switch and owned by the winit `ApplicationHandler`.
pub(crate) enum FrameExecutor {
    /// Pre-split fallback: renderer + context on the UI thread. Boxed — the
    /// inline executor owns the whole render stack and dwarfs the split's
    /// thread-handle variant.
    Inline(Box<InlineExecutor>),
    /// The split: renderer + context moved to a dedicated render thread.
    Split(SplitExecutor),
}

impl FrameExecutor {
    /// Whether a live surface already exists (the `resumed` re-entry guard) — the
    /// inline path reads the renderer's phase directly; the split path tracks
    /// whether a `SurfaceCreated` has been sent since the last destroy.
    pub(crate) fn has_surface(&self) -> bool {
        match self {
            FrameExecutor::Inline(inline) => inline.renderer.phase() == SurfacePhase::SurfaceReady,
            FrameExecutor::Split(split) => split.surface_requested,
        }
    }

    /// The running count of frames the render side has actually presented
    /// (`FrameOutcome::Rendered`). The UI thread loads this once per frame and
    /// pushes it into `RenderRoot::set_presented_frames` before paint, so a
    /// widget measuring FPS reports the presented rate, not its paint cadence.
    /// Both variants share the counter with their render side via an
    /// `Arc<AtomicU64>` — the split's other clone lives on the render thread.
    pub(crate) fn presented_frames(&self) -> u64 {
        match self {
            FrameExecutor::Inline(inline) => inline.presented.load(Ordering::Relaxed),
            FrameExecutor::Split(split) => split.presented.load(Ordering::Relaxed),
        }
    }

    /// Whether the inline executor's last frame was dropped on a transient
    /// acquire with the surface still ready, cleared by the read. The split
    /// reports the same thing asynchronously as [`ShellUserEvent::RenderRetry`],
    /// so this is always `false` there.
    pub(crate) fn take_present_retry(&mut self) -> bool {
        match self {
            FrameExecutor::Inline(inline) => std::mem::take(&mut inline.present_retry),
            FrameExecutor::Split(_) => false,
        }
    }

    /// Bring the surface online for `window` at `size` (`resumed`). Inline
    /// creates + installs it synchronously; the split creates the
    /// [`DetachedSurface`] here on the UI thread (the window-handle step winit
    /// requires on the main thread) and sends it across for the render thread to
    /// install. Either path's surface-creation failure is fatal (returns `Err`
    /// for the shell to stash).
    pub(crate) fn ensure_surface(
        &mut self,
        window: &WindowHandle,
        size: SurfaceSize,
    ) -> Result<()> {
        match self {
            FrameExecutor::Inline(inline) => inline.ensure_surface(window, size),
            FrameExecutor::Split(split) => split.send_surface_created(window, size),
        }
    }

    /// Re-create the surface after a render-side `SurfaceLost` (split only,
    /// routed here from the render thread through the proxy). The UI thread
    /// re-runs the detached creation and hands a fresh surface across. A no-op
    /// for inline (it recovers synchronously in [`InlineExecutor::submit_frame`]).
    pub(crate) fn recreate_surface(&mut self, window: &WindowHandle, size: SurfaceSize) {
        if let FrameExecutor::Split(split) = self {
            // Best-effort: a failure here logs and waits for the next event
            // (mirrors the inline recover-or-log-and-continue contract).
            if let Err(err) = split.send_surface_created(window, size) {
                log::error!("frust: failed to recreate surface: {err}");
            }
        }
    }

    /// Tear the surface down (`suspended`). Inline drops it immediately; the
    /// split sends a barriered [`RenderCommand::SurfaceDestroyed`] and blocks
    /// until the render thread has released its surface resources.
    pub(crate) fn destroy_surface(&mut self) {
        match self {
            FrameExecutor::Inline(inline) => {
                drop_overlay_surfaces();
                inline.renderer.on_surface_destroyed();
                // The torn-down engine held no page the next surface will:
                // publish the empty ledger now, so the first paint after a
                // resume names none.
                publish_ledger(&mut inline.renderer);
            }
            FrameExecutor::Split(split) => split.destroy_surface(),
        }
    }

    /// Reconfigure the swapchain for a new physical `size` (`Resized`). Inline
    /// resizes directly; the split sends [`RenderCommand::SurfaceChanged`].
    pub(crate) fn resize_surface(&mut self, size: SurfaceSize) {
        match self {
            FrameExecutor::Inline(inline) => {
                inline
                    .renderer
                    .on_surface_changed(&inline.render_cx, size.width, size.height);
                note_scale(&mut inline.renderer, size.scale);
            }
            FrameExecutor::Split(split) => split.resize_surface(size),
        }
    }

    /// Record the `first_rebuild_done` startup milestone once. Inline records it
    /// on the UI thread right after the first rebuild; the split records it
    /// render-side when the first scene arrives (the render thread owns the
    /// startup line — see [`render_loop`]), so this is a no-op there.
    pub(crate) fn record_first_rebuild(&mut self) {
        if let FrameExecutor::Inline(inline) = self
            && !inline.first_rebuild_recorded
        {
            inline.startup.record(SPAN_FIRST_REBUILD_DONE);
            inline.first_rebuild_recorded = true;
        }
    }

    /// Hand one finished frame to the executor. Inline runs the encode→present
    /// tail synchronously (borrowing `scene`, so it is reused next frame);
    /// the split moves the scene out into a [`SceneFrame`] and sends it
    /// across the channel (latest-wins), replacing it with a scene reclaimed off
    /// the render thread's give-back channel — `reset()`,
    /// so its buffer is reused rather than reallocated — falling back to
    /// `Scene::new()` only when none is available yet (cold start, or the
    /// render thread hasn't given one back). `frame_time` and `size` populate
    /// the split's [`FrameMeta`]; inline ignores them. `paint_serial` is the
    /// paint's `PaintOutcome::paint_serial`, named to the renderer before it
    /// encodes this scene on either path (it rides the split's [`FrameMeta`]).
    /// `overlay_cuts` are the overlays' ranges in this scene
    /// (`crate::platform_view::DesktopPlatformViews::overlay_cuts`), empty with
    /// overlays off.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn submit_frame(
        &mut self,
        window: &WindowHandle,
        scene: &mut Scene,
        base_color: peniko::Color,
        ui_spans: UiSpans,
        frame_time: FrameTime,
        size: SurfaceSize,
        paint_serial: u64,
        overlay_cuts: Vec<OverlayCutSpec>,
    ) {
        match self {
            FrameExecutor::Inline(inline) => inline.submit_frame(
                window,
                scene,
                base_color,
                ui_spans,
                paint_serial,
                &overlay_cuts,
            ),
            FrameExecutor::Split(split) => {
                let replacement = split.take_reusable_scene();
                let painted = PaintedScene {
                    scene: std::mem::replace(scene, replacement),
                    base_color,
                    overlay_cuts,
                };
                split.submit_frame(painted, ui_spans, frame_time, size, paint_serial);
            }
        }
    }
}

/// Spawn the render thread and return the [`SplitExecutor`] handle to it.
/// The [`RenderContext`] is created here on the UI thread so its
/// [`SurfaceFactory`] (which the UI thread keeps for on-main-thread surface
/// creation) shares the same wgpu instance as the context the render thread
/// renders with; the context itself (`Send`) is then moved onto the thread.
pub(crate) fn spawn_render_thread(proxy: EventLoopProxy<ShellUserEvent>) -> SplitExecutor {
    let render_cx = RenderContext::new();
    let factory = render_cx.surface_factory();
    let (sender, receiver) = render_channel::<PaintedScene, DesktopSurface>();
    let (scene_return_tx, scene_return_rx) = scene_return_channel::<Scene>();
    // Presented-frame counter: one clone drives into the render thread
    // (bumped on each `FrameOutcome::Rendered`), one stays in the `SplitExecutor`
    // for the UI thread to read before paint.
    let presented = Arc::new(AtomicU64::new(0));
    let presented_render = Arc::clone(&presented);
    let join = std::thread::Builder::new()
        .name("frust-render".to_string())
        // Guard the loop so a dev-build panic logs and exits cleanly (dropping the
        // owned `RenderReceiver`, which drains any orphaned `Ack` — the barrier
        // deadlock fix). A no-op under the release `panic = "abort"` profile.
        .spawn(move || {
            frust_shell_common::run_guarded_thread("frust-render (desktop)", move || {
                render_loop(
                    receiver,
                    proxy,
                    render_cx,
                    scene_return_tx,
                    presented_render,
                )
            })
        })
        .expect("frust: failed to spawn render thread");
    SplitExecutor::new(sender, join, factory, scene_return_rx, presented)
}

/// The single-thread fallback executor (kill switch engaged): the
/// [`RenderContext`]/[`SurfaceRenderer`] and all perf recording live on the UI
/// thread, exactly as the pre-split shell did.
pub(crate) struct InlineExecutor {
    render_cx: RenderContext,
    renderer: SurfaceRenderer,
    startup: StartupSpans,
    frame_stats: FrameStats,
    renderer_spans_recorded: bool,
    first_rebuild_recorded: bool,
    first_encode_recorded: bool,
    first_frame_recorded: bool,
    /// Running count of presented frames. Inline renders on the UI
    /// thread, so this is bumped and read on the same thread — the `Arc<Atomic>`
    /// shape matches the split's cross-thread counter so `FrameExecutor` reads
    /// both variants uniformly.
    presented: Arc<AtomicU64>,
    /// Set when the last frame was skipped on a transient acquire while the
    /// surface was ready; [`FrameExecutor::take_present_retry`] reads and clears it.
    present_retry: bool,
}

impl InlineExecutor {
    /// Construct the fallback executor and record the `init_entry` startup
    /// milestone (mirrors the pre-split `run_desktop` entry span).
    pub(crate) fn new() -> Self {
        let mut startup = StartupSpans::begin();
        startup.record(SPAN_INIT_ENTRY);
        Self {
            render_cx: RenderContext::new(),
            renderer: SurfaceRenderer::new(),
            startup,
            frame_stats: FrameStats::new(),
            renderer_spans_recorded: false,
            first_rebuild_recorded: false,
            first_encode_recorded: false,
            first_frame_recorded: false,
            presented: Arc::new(AtomicU64::new(0)),
            present_retry: false,
        }
    }

    fn ensure_surface(&mut self, window: &WindowHandle, size: SurfaceSize) -> Result<()> {
        // Inline runs on the UI thread, so the combined create-and-configure
        // `on_surface_created` (which reads the window handle) is fine here.
        // Overlay surfaces share the device the new surface may replace.
        drop_overlay_surfaces();
        let hit = load_pipeline_cache(&mut self.renderer);
        pollster::block_on(self.renderer.on_surface_created(
            &mut self.render_cx,
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            SurfaceAlphaRequest::Opaque,
        ))
        .context("frust: failed to create render surface")?;
        note_scale(&mut self.renderer, size.scale);
        persist_and_record(
            &self.renderer,
            &mut self.startup,
            &mut self.renderer_spans_recorded,
            hit,
        );
        publish_gpu_handle(&self.render_cx);
        Ok(())
    }

    fn submit_frame(
        &mut self,
        window: &WindowHandle,
        scene: &Scene,
        base_color: peniko::Color,
        ui_spans: UiSpans,
        paint_serial: u64,
        overlay_cuts: &[OverlayCutSpec],
    ) {
        let mut overlays = take_frame_overlays(
            &self.renderer,
            &mut self.render_cx,
            overlay_cuts,
            scene.commands().len(),
        );
        let outcome = render_frame(
            &mut self.renderer,
            &self.render_cx,
            window,
            scene,
            paint_serial,
            base_color,
            ui_spans,
            &mut self.frame_stats,
            &mut self.startup,
            &mut self.first_encode_recorded,
            &mut self.first_frame_recorded,
            overlays.as_mut(),
        );
        if let Some(overlays) = overlays {
            land_frame_overlays(overlays);
        }
        match outcome {
            // Stale swapchain (mid-resize): reconfigured internally — redraw
            // against the fresh configuration.
            Ok(FrameOutcome::Redraw) => window.request_redraw(),
            // Surface lost (rare on desktop): recreate from the same window and
            // redraw; log-and-continue on failure rather than killing the app.
            // Inline is on the UI thread, so `on_surface_created` works here.
            Ok(FrameOutcome::SurfaceLost) => {
                let size = window.inner_size();
                drop_overlay_surfaces();
                match pollster::block_on(self.renderer.on_surface_created(
                    &mut self.render_cx,
                    window.clone(),
                    size.width.max(1),
                    size.height.max(1),
                    SurfaceAlphaRequest::Opaque,
                )) {
                    Ok(()) => window.request_redraw(),
                    Err(err) => eprintln!("frust: failed to recreate surface: {err}"),
                }
            }
            // A presented frame: bump the shared counter the UI thread reads
            // before paint. `Skipped` presents nothing, so it doesn't.
            Ok(FrameOutcome::Rendered) => {
                self.presented.fetch_add(1, Ordering::Relaxed);
            }
            // With a ready surface, a skip is a transient acquire (`Timeout`/
            // `Occluded`): the frame is gone, so the shell owes a retry.
            Ok(FrameOutcome::Skipped) => {
                self.present_retry = self.renderer.phase() == SurfacePhase::SurfaceReady;
            }
            Err(err) => eprintln!("frust: render error: {err}"),
        }
    }
}

impl Default for InlineExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// The render-thread-split executor (default): the [`RenderSender`] half of the
/// channel, the [`SurfaceFactory`] the UI thread creates surfaces with, and the
/// thread's [`JoinHandle`].
pub(crate) struct SplitExecutor {
    /// `Option` so [`Drop`] can drop it *before* joining: dropping the sender is
    /// what signals the render loop to exit.
    sender: Option<RenderSender<PaintedScene, DesktopSurface>>,
    join: Option<JoinHandle<()>>,
    /// Creates the `Send` surface on the UI thread (the window-handle step winit
    /// requires on the main thread), for the render thread to install.
    factory: SurfaceFactory,
    /// Whether a [`RenderCommand::SurfaceCreated`] has been sent since the last
    /// destroy — the UI-side mirror of the renderer's phase for `has_surface`.
    surface_requested: bool,
    /// Monotonically increasing per-frame id stamped into [`FrameMeta`].
    frame_id: u64,
    /// The UI-side half of the render thread's give-back channel:
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
    /// render thread ([`render_loop`], which bumps it on each
    /// `FrameOutcome::Rendered`). A plain `Arc<AtomicU64>` — no channel/protocol,
    /// mirroring the `fatal`-flag pattern on the mobile shells.
    presented: Arc<AtomicU64>,
}

impl SplitExecutor {
    pub(crate) fn new(
        sender: RenderSender<PaintedScene, DesktopSurface>,
        join: JoinHandle<()>,
        factory: SurfaceFactory,
        scene_return: SceneReturnReceiver<Scene>,
        presented: Arc<AtomicU64>,
    ) -> Self {
        Self {
            sender: Some(sender),
            join: Some(join),
            factory,
            surface_requested: false,
            frame_id: 0,
            scene_return,
            spare_scene: None,
            presented,
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

    /// Create the surface on the UI thread and send it across for the render
    /// thread to install. Used for the initial `resumed` creation and for
    /// `SurfaceLost` re-creation (both need the main thread).
    fn send_surface_created(&mut self, window: &WindowHandle, size: SurfaceSize) -> Result<()> {
        let detached = self
            .factory
            .create_detached_surface(window.clone())
            .context("frust: failed to create render surface")?;
        if let Some(sender) = self.sender.as_ref() {
            sender.send_command(RenderCommand::SurfaceCreated {
                window: DesktopSurface {
                    detached,
                    window: window.clone(),
                },
                size,
            });
        }
        self.surface_requested = true;
        Ok(())
    }

    fn destroy_surface(&mut self) {
        if let Some(sender) = self.sender.as_ref() {
            // Block until the render thread has dropped its surface resources
            // before the shell proceeds (the barrier contract), but bounded so a
            // wedged render thread degrades instead of hanging the UI thread.
            if !sender
                .destroy_surface()
                .wait_timeout(DESTROY_SURFACE_BARRIER_DEADLINE)
            {
                log::error!(
                    "frust: surface-destroy barrier timed out after \
                     {DESTROY_SURFACE_BARRIER_DEADLINE:?}; proceeding (degraded)"
                );
            }
        }
        self.surface_requested = false;
    }

    fn resize_surface(&mut self, size: SurfaceSize) {
        if let Some(sender) = self.sender.as_ref() {
            sender.send_command(RenderCommand::SurfaceChanged { size });
        }
    }

    fn submit_frame(
        &mut self,
        painted: PaintedScene,
        ui_spans: UiSpans,
        frame_time: FrameTime,
        size: SurfaceSize,
        paint_serial: u64,
    ) {
        self.frame_id += 1;
        if let Some(sender) = self.sender.as_ref() {
            let stale = sender.send_scene(SceneFrame {
                scene: painted,
                meta: FrameMeta {
                    frame_time,
                    size,
                    frame_id: self.frame_id,
                    paint_serial,
                },
                ui_spans,
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
}

impl Drop for SplitExecutor {
    fn drop(&mut self) {
        // Drop the sender first: that signals the render loop's `wait_next` to
        // wake with a disconnection and exit. Then join so the render thread's
        // final present + best-effort pipeline-cache persist complete, and its
        // `Arc<Window>` is released, before the shell drops its own (so the
        // window is destroyed on the main thread).
        self.sender.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The dedicated render thread's loop: own the
/// [`RenderContext`] + [`SurfaceRenderer`] wholesale, install surfaces handed
/// over from the UI thread, drain lifecycle commands and the freshest scene from
/// the channel, and run encode→acquire→submit for each frame — the single perf
/// emitter (folding [`UiSpans`] with its own [`RenderSpans`]). Exits cleanly
/// when the [`RenderSender`] is dropped.
fn render_loop(
    receiver: RenderReceiver<PaintedScene, DesktopSurface>,
    proxy: EventLoopProxy<ShellUserEvent>,
    mut render_cx: RenderContext,
    scene_return: SceneReturnSender<Scene>,
    presented: Arc<AtomicU64>,
) {
    let mut renderer = SurfaceRenderer::new();
    // Kept only for `pre_present_notify` (surface creation/recovery is UI-side).
    // Cleared on `SurfaceDestroyed`.
    let mut window: Option<WindowHandle> = None;
    // The render thread's view of surface lifecycle — gates whether a handed-off
    // scene is submitted or dropped (a leftover scene after a pause/destroy must
    // not be presented).
    let mut phase = RenderPhase::NoSurface;

    let mut frame_stats = FrameStats::new();
    // The render thread owns the startup line in the split; its epoch is this
    // thread's spawn (≈ `run_desktop` entry — only the milestone *spacing*
    // matters, and this thread is spawned at shell startup).
    let mut startup = StartupSpans::begin();
    startup.record(SPAN_INIT_ENTRY);
    let mut renderer_spans_recorded = false;
    let mut first_rebuild_recorded = false;
    let mut first_encode_recorded = false;
    let mut first_frame_recorded = false;

    loop {
        let batch = receiver.wait_next();

        // Lifecycle commands first (FIFO), updating the phase machine.
        for command in batch.commands {
            phase = next_render_phase(phase, command.event());
            match command {
                RenderCommand::SurfaceCreated {
                    window: surface,
                    size,
                } => {
                    let DesktopSurface {
                        detached,
                        window: w,
                    } = surface;
                    // The overlays share the device the new surface may
                    // replace: rebuilt on their layers by the next frame.
                    drop_overlay_surfaces();
                    if let Err(err) = install_detached(
                        &mut renderer,
                        &mut render_cx,
                        detached,
                        size.width,
                        size.height,
                        &mut startup,
                        &mut renderer_spans_recorded,
                    ) {
                        // A render-side install failure is fatal, exactly as the
                        // inline path treats surface creation — report it to the
                        // UI thread so `run_desktop` surfaces it as `Err`, then
                        // exit.
                        let _ = proxy.send_event(ShellUserEvent::RenderFatal(err));
                        return;
                    }
                    note_scale(&mut renderer, size.scale);
                    window = Some(w);
                }
                RenderCommand::SurfaceChanged { size } => {
                    renderer.on_surface_changed(&render_cx, size.width, size.height);
                    note_scale(&mut renderer, size.scale);
                }
                RenderCommand::SurfaceDestroyed { ack } => {
                    drop_overlay_surfaces();
                    renderer.on_surface_destroyed();
                    // The empty ledger is published BEFORE the barrier is
                    // acknowledged, so the UI thread's first paint after a
                    // resume already sees it and names no torn-down page.
                    publish_ledger(&mut renderer);
                    window = None;
                    // Acknowledge only after the surface resources are dropped —
                    // the window-release barrier.
                    ack.acknowledge();
                }
                RenderCommand::Pause { ack } => {
                    // Desktop never sends `Pause`; honor the barrier defensively
                    // so a stray one can never deadlock the UI thread.
                    ack.acknowledge();
                }
                RenderCommand::Resume => {}
            }
        }

        // Then the freshest scene, only if the phase allows submitting.
        if let Some(frame) = batch.scene {
            if phase.can_render()
                && let Some(win) = window.as_ref()
            {
                // The first handed-off scene marks the first UI frame produced —
                // the render thread's stand-in for `first_rebuild_done` (it owns
                // the startup line in the split).
                if !first_rebuild_recorded {
                    startup.record(SPAN_FIRST_REBUILD_DONE);
                    first_rebuild_recorded = true;
                }
                // Overlay surfaces (`FRUST_OVERLAYS`, macOS): built/resized
                // and taken out for this frame, which then renders segmented.
                // `None` keeps the single-surface tail unchanged.
                let mut overlays = take_frame_overlays(
                    &renderer,
                    &mut render_cx,
                    &frame.scene.overlay_cuts,
                    frame.scene.scene.commands().len(),
                );
                let outcome = render_frame(
                    &mut renderer,
                    &render_cx,
                    win,
                    &frame.scene.scene,
                    frame.meta.paint_serial,
                    frame.scene.base_color,
                    frame.ui_spans,
                    &mut frame_stats,
                    &mut startup,
                    &mut first_encode_recorded,
                    &mut first_frame_recorded,
                    overlays.as_mut(),
                );
                // Presented: hand the overlay surfaces back (a withdraw
                // waiting on one proceeds).
                if let Some(overlays) = overlays {
                    land_frame_overlays(overlays);
                }
                match outcome {
                    // Stale swapchain: reconfigured internally — ask the UI thread
                    // (via the proxy) to repaint so a fresh scene is handed off,
                    // keeping winit's dirty-driven `Wait` model intact.
                    Ok(FrameOutcome::Redraw) => {
                        let _ = proxy.send_event(ShellUserEvent::RenderNeedsRedraw);
                    }
                    // Surface lost: re-creation needs the main thread (the window
                    // handle), so hand the request back to the UI thread, which
                    // re-runs the detached creation and sends a fresh
                    // `SurfaceCreated`. Until then the renderer's own phase gate
                    // (now `SurfaceLost`) skips frames safely.
                    Ok(FrameOutcome::SurfaceLost) => {
                        let _ = proxy.send_event(ShellUserEvent::RenderRecreateSurface);
                    }
                    // A presented frame: bump the shared counter the UI thread
                    // reads before paint. `Skipped` presents nothing.
                    Ok(FrameOutcome::Rendered) => {
                        presented.fetch_add(1, Ordering::Relaxed);
                    }
                    // A transient acquire (`Timeout`/`Occluded`, e.g. for the
                    // length of macOS's fullscreen-exit animation) dropped this
                    // frame with the surface still ready. Nothing else will
                    // replace it on an idle screen, so ask the UI thread for a
                    // backed-off retry.
                    Ok(FrameOutcome::Skipped) => {
                        if renderer.phase() == SurfacePhase::SurfaceReady {
                            let _ = proxy.send_event(ShellUserEvent::RenderRetry);
                        }
                    }
                    Err(err) => log::error!("frust: render error: {err}"),
                }
            }
            // Give the drained scene back for the UI thread to reclaim —
            // whether it was actually rendered above, phase-gated
            // out (`Paused`/`NoSurface`), or the window wasn't ready yet; `encode`
            // has already fully consumed the scene's commands by this point, so
            // its buffer is safe to reuse. Never silently dropped.
            scene_return.give_back(frame.scene.scene);
        }

        if batch.disconnected {
            break;
        }
    }
}

/// Load the persisted pipeline cache into `renderer` **before** surface
/// creation (the init-time cache contract); returns whether a blob was restored
/// (the warm-start hit for the startup line). No-op on macOS/Metal; seeds
/// warm-start Vulkan shader compilation on Linux/Windows. Shared by both paths.
fn load_pipeline_cache(renderer: &mut SurfaceRenderer) -> bool {
    let cache_data = crate::cache::load_cache();
    let hit = cache_data.is_some();
    if cache_data.is_some() {
        renderer.set_initial_pipeline_cache_data(cache_data);
    }
    hit
}

/// Persist the compiled pipeline cache (best-effort, background thread) and
/// record the adapter/device/renderer + pipeline-cache startup milestones on the
/// first creation only. Shared by both paths so the cache + span logic is not
/// forked; run **after** the surface is installed.
fn persist_and_record(
    renderer: &SurfaceRenderer,
    startup: &mut StartupSpans,
    renderer_spans_recorded: &mut bool,
    pipeline_cache_hit: bool,
) {
    if let Some(cache) = renderer.pipeline_cache_data() {
        std::thread::spawn(move || crate::cache::save_cache(&cache));
    }
    // Adapter/device/renderer creation all happen inside the single install
    // call — record all three at that one observable boundary, on the first
    // successful creation only (a later suspend/resume recreation is not
    // re-recorded).
    if !*renderer_spans_recorded {
        startup.record(SPAN_ADAPTER_READY);
        startup.record(SPAN_DEVICE_READY);
        if pipeline_cache_hit {
            startup.record(SPAN_PIPELINE_CACHE_RESTORED);
        }
        startup.record(SPAN_RENDERER_READY);
        *renderer_spans_recorded = true;
    }
}

/// Install a UI-thread-created [`DetachedSurface`] into the render thread's
/// renderer, wrapping it in the same pipeline-cache load-before / persist-after
/// contract the inline path uses (via [`load_pipeline_cache`]/[`persist_and_record`]).
fn install_detached(
    renderer: &mut SurfaceRenderer,
    render_cx: &mut RenderContext,
    detached: DetachedSurface,
    width: u32,
    height: u32,
    startup: &mut StartupSpans,
    renderer_spans_recorded: &mut bool,
) -> Result<()> {
    let hit = load_pipeline_cache(renderer);
    pollster::block_on(renderer.on_surface_installed(
        render_cx,
        detached,
        width,
        height,
        SurfaceAlphaRequest::Opaque,
    ))
    .context("frust: failed to install render surface")?;
    persist_and_record(renderer, startup, renderer_spans_recorded, hit);
    publish_gpu_handle(render_cx);
    Ok(())
}

/// Install this render context's live GPU device into the process-wide slot
/// (`frust_shell_common::gpu`) the facade's `frust::gpu::with_context` reads
/// through, once — the first time a surface (and so a device) exists. A
/// no-op past the first successful call from either executor (the slot is
/// install-once; see its own module docs), and compiled to nothing under a
/// default (non-`gpu`) build of this crate.
///
/// `device_handle` is panic-free at both call sites
/// ([`InlineExecutor::ensure_surface`], [`install_detached`]): both run this
/// strictly after a successful `on_surface_created`/`on_surface_installed`,
/// exactly the precondition `RenderContext::device_handle`'s own doc comment
/// names. Cloning it is cheap — `DeviceHandle`'s wgpu resources are
/// themselves `Arc`-backed, so this hands the slot another handle to the
/// same device, never a second one.
#[cfg(feature = "gpu")]
fn publish_gpu_handle(render_cx: &RenderContext) {
    frust_shell_common::gpu::install_gpu_handle(render_cx.device_handle().clone());
}

#[cfg(not(feature = "gpu"))]
fn publish_gpu_handle(_render_cx: &RenderContext) {}

/// This surface's most recent real GPU pass timing, folded into the
/// [`GpuPasses`] shape [`FramePasses::with_gpu`] takes, or `None` when the
/// surface produces no such measurement (a device that never offered
/// `TIMESTAMP_QUERY`, or simply no reading landed yet — see [`SurfaceRenderer::gpu_pass_timings`]).
fn gpu_passes(renderer: &SurfaceRenderer) -> Option<GpuPasses> {
    renderer.gpu_pass_timings().map(|timings| GpuPasses {
        prepass: timings.prepass,
        main: timings.main,
        composite: timings.composite,
        blit: timings.blit,
    })
}

/// Publish the renderer's resident-key ledger for the UI thread's next paint
/// when it changed since the last publish — after a frame, and at a surface
/// destroy or device-scale change (both of which leave it empty).
///
/// The slot is process-global (one renderer per process); see
/// [`publish_resident_pages`].
fn publish_ledger(renderer: &mut SurfaceRenderer) {
    if let Some(resident) = renderer.take_retained_ledger() {
        publish_resident_pages(
            resident.frame,
            resident
                .pages
                .iter()
                .map(|page| (page.key, page.extent_px, page.scale)),
        );
    }
}

/// Hand the renderer the device scale a surface create or resize carries
/// (winit folds `ScaleFactorChanged` into the `Resized` that reaches
/// [`FrameExecutor::resize_surface`]): a changed scale clears the retained
/// store, and its empty ledger is published at once.
fn note_scale(renderer: &mut SurfaceRenderer, scale: f64) {
    if renderer.note_device_scale(scale) {
        publish_ledger(renderer);
    }
}

/// [`publish_ledger`] after a frame, answering whether that frame was refused
/// for drawing a retained page the renderer no longer holds.
fn publish_retained(renderer: &mut SurfaceRenderer) -> bool {
    publish_ledger(renderer);
    renderer.take_retained_refusal()
}

/// Run the encode→acquire→submit tail for one painted `scene`, timing each span
/// with its own `Instant`, recording the folded [`FramePasses`] through the
/// single emitter, and stamping the first-encode/first-frame startup milestones.
/// Returns the [`FrameOutcome`] for the caller to act on (redraw / recover /
/// nothing) — the caller differs only in *how* it requests the follow-up redraw
/// (UI-side `request_redraw` inline; the proxy in the split). Shared by both
/// paths so the frame-pipeline logic is not forked.
///
/// `overlays` (macOS, `FRUST_OVERLAYS`) makes the frame segmented — encoded
/// with the overlays' cuts, submitted once for every target, presented main
/// first — and `None` keeps the single-surface tail exactly as it was.
#[allow(clippy::too_many_arguments)]
fn render_frame(
    renderer: &mut SurfaceRenderer,
    render_cx: &RenderContext,
    window: &Window,
    scene: &Scene,
    paint_serial: u64,
    base_color: peniko::Color,
    ui_spans: UiSpans,
    frame_stats: &mut FrameStats,
    startup: &mut StartupSpans,
    first_encode_recorded: &mut bool,
    first_frame_recorded: &mut bool,
    mut overlays: Option<&mut FrameOverlays>,
) -> Result<FrameOutcome> {
    // Install the platform importers once the device exists; a no-op on every
    // later frame.
    #[cfg(target_os = "macos")]
    frust_render::native_import::install(render_cx);

    window.pre_present_notify();

    // Encode span (GPU/CPU encode, no swapchain touch). Each span is its own
    // `Instant` read; desktop's frame budget is generous enough not to gate them
    // behind `perf::enabled()` (see docs/CODE_STANDARDS.md Instrumentation).
    // The paint serial this scene was painted under, so the ledger published
    // once it is committed names it (see `SurfaceRenderer::note_scene_serial`).
    renderer.note_scene_serial(paint_serial);
    let encode_start = Instant::now();
    let encode_outcome = match overlays.as_deref() {
        None => renderer.encode(render_cx, scene, base_color),
        Some(overlays) => {
            renderer.encode_segmented(render_cx, scene, base_color, &overlays.ranges())
        }
    };
    let encode_dur = encode_start.elapsed();

    if matches!(encode_outcome, Ok(EncodeOutcome::Encoded)) && !*first_encode_recorded {
        startup.record(SPAN_FIRST_ENCODE_DONE);
        *first_encode_recorded = true;
    }

    // Acquire span (blocking vsync/present wait).
    let acquire_start = Instant::now();
    let acquire_outcome = match encode_outcome {
        Ok(EncodeOutcome::Encoded) => renderer.acquire(render_cx),
        // Nothing encoded (no renderable surface): surface the same `Skipped`
        // the combined path would have, without acquiring.
        Ok(EncodeOutcome::Skipped) => Ok(AcquireOutcome::Skipped),
        Err(err) => Err(err),
    };
    let acquire_dur = acquire_start.elapsed();

    // Submit span (blit + queue-submit + present).
    let submit_start = Instant::now();
    let render_outcome = match acquire_outcome {
        Ok(AcquireOutcome::Acquired) => match overlays.as_mut() {
            None => renderer.submit(render_cx),
            Some(overlays) => submit_with_overlays(renderer, render_cx, overlays),
        },
        Ok(AcquireOutcome::Reconfigured) => Ok(FrameOutcome::Redraw),
        Ok(AcquireOutcome::Lost) => Ok(FrameOutcome::SurfaceLost),
        Ok(AcquireOutcome::Skipped) => Ok(FrameOutcome::Skipped),
        Err(err) => Err(err),
    };
    let submit_dur = submit_start.elapsed();

    // The resident-key ledger after this frame, for the UI thread's next
    // paint. A frame the engine refused for drawing a page it no longer holds
    // presented nothing and emptied the store, so it is reported as owing a
    // redraw: both callers then ask the UI thread for one more paint, against
    // the empty ledger just published, instead of leaving an idle window on
    // the frame before.
    let refused_retained = publish_retained(renderer);
    let render_outcome = match render_outcome {
        Ok(FrameOutcome::Skipped) if refused_retained => Ok(FrameOutcome::Redraw),
        other => other,
    };

    // One folded frame record through the single emitter:
    // the UI thread's rebuild/layout/paint + this thread's encode/acquire/submit,
    // with this surface's real GPU pass timing attached when it produces one
    // (engine tier, `perf-trace`, a device that offered `TIMESTAMP_QUERY`).
    let mut passes = FramePasses::from_split(
        ui_spans,
        RenderSpans {
            encode: encode_dur,
            acquire: acquire_dur,
            submit: submit_dur,
        },
    );
    if let Some(gpu) = gpu_passes(renderer) {
        passes = passes.with_gpu(gpu);
    }
    frame_stats.record(passes);
    if frame_stats.should_emit() {
        frame_stats.emit_log();
    }

    if matches!(render_outcome, Ok(FrameOutcome::Rendered)) && !*first_frame_recorded {
        startup.record(SPAN_FIRST_FRAME_PRESENTED);
        startup.emit_log();
        *first_frame_recorded = true;
    }

    render_outcome
}

// ---------------------------------------------------------------------------
// Overlay surfaces (`FRUST_OVERLAYS`, macOS only, default off)
// ---------------------------------------------------------------------------
//
// A macOS window is Mode A: its hosted `NSView`s sit above the wgpu surface,
// so frust paint recorded after a slot hides behind it. With the switch on the
// UI thread cuts each frame at its slots (`crate::platform_view`), the macOS
// host stacks an input-transparent `CAMetalLayer` view directly above each
// overlay's slot and publishes its layer here
// (`crate::extensions::publish_overlay_layer`), and the render side builds an
// `OverlaySurface` on it and draws the frame segmented — one compiled frame,
// one submit — presenting main, then the overlays bottom to top.

/// The overlay switch's name, shared with the mobile hosts.
const OVERLAYS_ENV_VAR: &str = "FRUST_OVERLAYS";

/// Engine refusals logged per overlay before its log goes quiet.
const OVERLAY_ERRORS_LOGGED: u32 = 3;

/// Rendered overlay frames between two `frust-overlay skew` lines.
const OVERLAY_SKEW_EVERY: u32 = 120;

/// How long the UI thread waits for an in-flight overlay surface before
/// withdrawing its layer anyway: two of `nextDrawable`'s one-second timeouts.
/// Reached only if the render thread died mid-frame — a live one hands its
/// surfaces back within a frame.
const OVERLAY_LANDING_WAIT: Duration = Duration::from_secs(2);

/// Whether one value of the switch turns overlays on: blank, `0`, `off` or
/// `false` (case-insensitive, whitespace ignored) is off, anything else on —
/// Android's `media` arm included, which means nothing more than "on" here.
pub(crate) fn overlay_switch_value_on(value: &str) -> bool {
    let value = value.trim();
    !(value.is_empty()
        || value == "0"
        || value.eq_ignore_ascii_case("off")
        || value.eq_ignore_ascii_case("false"))
}

/// Resolve the switch from its sources in priority order — the first
/// non-blank one decides (a runtime `0` switches a baked-in value off); none
/// set is off.
pub(crate) fn overlay_switch_from<'a>(sources: impl IntoIterator<Item = Option<&'a str>>) -> bool {
    sources
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
        .is_some_and(overlay_switch_value_on)
}

/// Whether overlay surfaces are on — macOS only (the one desktop host with an
/// overlay view), `FRUST_OVERLAYS` at runtime, then as a compile-time
/// `--define`, then the `FRUST_OVERLAY_SPIKE` alias. Read once and cached.
pub(crate) fn overlays_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        if !cfg!(target_os = "macos") {
            return false;
        }
        let runtime = std::env::var(OVERLAYS_ENV_VAR).ok();
        let alias = frust_render::overlay::overlay_spike_value();
        overlay_switch_from([
            runtime.as_deref(),
            option_env!("FRUST_OVERLAYS"),
            alias.as_deref(),
        ])
    })
}

/// One overlay's two halves: the layer the macOS host published and the
/// surface the render side built on it.
#[derive(Debug)]
struct OverlayEntry<S> {
    /// Declared before `layer`: fields drop in order, so even an implicit drop
    /// releases the surface before the layer is forgotten.
    surface: Option<S>,
    /// The `CAMetalLayer*`'s address (an exposed-provenance integer, so the
    /// table needs no `Send` claim of its own).
    layer: usize,
    /// The layer's drawable size, physical px.
    size: (u32, u32),
    /// Building on this layer was refused (logged once); retried only for a
    /// new layer or a new main surface.
    refused: bool,
    /// Taken out by the render side for the length of a frame.
    in_flight: bool,
    /// Engine refusals, for the latched log.
    errors: u32,
    /// Whether the first kept-on-main cut was logged.
    unservable_logged: bool,
}

/// What [`OverlayTable::publish`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayerChange {
    /// A new layer: any surface on the previous one was dropped first.
    Adopted,
    /// The layer held, at a new size.
    Resized,
    /// The layer held, at its size.
    Unchanged,
    /// A different layer while the old one's surface is in flight: nothing
    /// changed — wait for it to land, then retry.
    Busy,
}

/// The published overlay layers by the differ's overlay id, with the surface
/// built on each — shared by the UI thread (publish/withdraw) and the render
/// side (build, resize, draw). The render side takes the live surfaces **out**
/// for the length of a frame ([`Self::take_for_frame`]) and hands them back
/// ([`Self::land`]), so the lock is never held across a drawable acquire; a
/// withdraw waits for its surface to land, then drops it before the layer may
/// go. The invariant every method keeps: a surface is dropped before its layer
/// is let go.
#[derive(Debug)]
pub(crate) struct OverlayTable<S> {
    entries: std::collections::BTreeMap<u64, OverlayEntry<S>>,
}

impl<S> OverlayTable<S> {
    /// An empty table (`const`, for the process-global).
    pub(crate) const fn new() -> Self {
        Self {
            entries: std::collections::BTreeMap::new(),
        }
    }

    /// Overlay `id`'s layer at `size` physical px.
    pub(crate) fn publish(&mut self, id: u64, layer: usize, size: (u32, u32)) -> LayerChange {
        if let Some(entry) = self.entries.get_mut(&id) {
            if entry.layer == layer {
                if entry.size == size {
                    return LayerChange::Unchanged;
                }
                entry.size = size;
                return LayerChange::Resized;
            }
            if entry.in_flight {
                return LayerChange::Busy;
            }
            entry.surface = None;
        }
        self.entries.insert(
            id,
            OverlayEntry {
                surface: None,
                layer,
                size,
                refused: false,
                in_flight: false,
                errors: 0,
                unservable_logged: false,
            },
        );
        LayerChange::Adopted
    }

    /// Whether overlay `id`'s surface is out on the render side.
    pub(crate) fn is_in_flight(&self, id: u64) -> bool {
        self.entries.get(&id).is_some_and(|entry| entry.in_flight)
    }

    /// Drop overlay `id`'s surface, then forget its layer; `false` for an
    /// unknown overlay or one in flight (wait, then retry).
    pub(crate) fn withdraw(&mut self, id: u64) -> bool {
        if self.is_in_flight(id) {
            return false;
        }
        match self.entries.remove(&id) {
            Some(mut entry) => {
                entry.surface = None;
                true
            }
            None => false,
        }
    }

    /// The main surface went away: drop every surface held here and keep the
    /// layers, so the next frame rebuilds them (retrying a refusal).
    pub(crate) fn drop_surfaces(&mut self) -> usize {
        let mut dropped = 0;
        for entry in self.entries.values_mut() {
            if entry.surface.take().is_some() {
                dropped += 1;
            }
            entry.refused = false;
        }
        dropped
    }

    /// Ahead of a frame: resize every live surface, build one where a layer
    /// has none (and was not refused), then take the live ones out, marked in
    /// flight, ascending id. Answers them and this call's refusals.
    pub(crate) fn take_for_frame<E>(
        &mut self,
        mut build: impl FnMut(usize, (u32, u32)) -> Result<S, E>,
        mut resize: impl FnMut(&mut S, (u32, u32)),
    ) -> OverlayTake<S, E> {
        let mut flying = Vec::new();
        let mut refusals = Vec::new();
        for (&id, entry) in &mut self.entries {
            if entry.in_flight {
                continue;
            }
            match entry.surface.as_mut() {
                Some(surface) => resize(surface, entry.size),
                None if !entry.refused => match build(entry.layer, entry.size) {
                    Ok(surface) => entry.surface = Some(surface),
                    Err(error) => {
                        entry.refused = true;
                        refusals.push((id, error));
                    }
                },
                None => {}
            }
            if let Some(surface) = entry.surface.take() {
                entry.in_flight = true;
                flying.push((id, surface));
            }
        }
        (flying, refusals)
    }

    /// Hand the surfaces [`Self::take_for_frame`] took back; one whose overlay
    /// is gone meanwhile is dropped (answers how many).
    pub(crate) fn land(&mut self, flying: Vec<(u64, S)>) -> usize {
        let mut orphaned = 0;
        for (id, surface) in flying {
            match self.entries.get_mut(&id) {
                Some(entry) => {
                    entry.surface = Some(surface);
                    entry.in_flight = false;
                }
                None => {
                    drop(surface);
                    orphaned += 1;
                }
            }
        }
        orphaned
    }

    /// Count an engine refusal; the running count while within `log_limit`.
    pub(crate) fn note_error(&mut self, id: u64, log_limit: u32) -> Option<u32> {
        let entry = self.entries.get_mut(&id)?;
        entry.errors = entry.errors.saturating_add(1);
        (entry.errors <= log_limit).then_some(entry.errors)
    }

    /// `true` exactly once per overlay: its first kept-on-main cut.
    pub(crate) fn note_unservable(&mut self, id: u64) -> bool {
        self.entries
            .get_mut(&id)
            .is_some_and(|entry| !std::mem::replace(&mut entry.unservable_logged, true))
    }

    /// Whether no overlay is published.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// [`OverlayTable::take_for_frame`]'s answer: the surfaces taken out and the
/// refusals made.
pub(crate) type OverlayTake<S, E> = (Vec<(u64, S)>, Vec<(u64, E)>);

/// The process-wide overlay table (one window, one render surface).
static OVERLAY_TABLE: std::sync::Mutex<OverlayTable<frust_render::overlay::OverlaySurface>> =
    std::sync::Mutex::new(OverlayTable::new());

/// Signalled whenever the render side hands its in-flight surfaces back.
static OVERLAY_LANDED: std::sync::Condvar = std::sync::Condvar::new();

/// The main-to-last-overlay present gap, folded per frame.
static OVERLAY_SKEW: std::sync::Mutex<(u32, u64, u64)> = std::sync::Mutex::new((0, 0, 0));

fn overlay_table()
-> std::sync::MutexGuard<'static, OverlayTable<frust_render::overlay::OverlaySurface>> {
    OVERLAY_TABLE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Publish overlay `id`'s layer (UI thread) — see
/// [`crate::extensions::publish_overlay_layer`]. Answers whether the render
/// side has something new to build or resize (a frame is owed).
pub(crate) fn publish_overlay_layer(id: u64, layer: usize, size: (u32, u32)) -> bool {
    let size = (size.0.max(1), size.1.max(1));
    let mut table = overlay_table();
    let mut change = table.publish(id, layer, size);
    if change == LayerChange::Busy {
        table = wait_overlay_landed(table, id);
        change = table.publish(id, layer, size);
    }
    match change {
        LayerChange::Adopted => {
            log::info!(
                "frust-overlay: overlay {id} layer published ({}x{} px)",
                size.0,
                size.1
            );
            true
        }
        LayerChange::Resized => true,
        LayerChange::Unchanged => false,
        LayerChange::Busy => {
            log::error!("frust-overlay: overlay {id} could not replace its layer");
            false
        }
    }
}

/// Withdraw overlay `id`'s layer (UI thread) — see
/// [`crate::extensions::withdraw_overlay_layer`]: wait out a frame drawing it,
/// then drop its surface; on return the layer may be released.
pub(crate) fn withdraw_overlay_layer(id: u64) {
    let mut table = wait_overlay_landed(overlay_table(), id);
    if table.withdraw(id) {
        log::info!("frust-overlay: overlay {id} layer withdrawn");
    }
}

fn wait_overlay_landed(
    mut table: std::sync::MutexGuard<'static, OverlayTable<frust_render::overlay::OverlaySurface>>,
    id: u64,
) -> std::sync::MutexGuard<'static, OverlayTable<frust_render::overlay::OverlaySurface>> {
    let deadline = Instant::now() + OVERLAY_LANDING_WAIT;
    while table.is_in_flight(id) {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            log::error!(
                "frust-overlay: overlay {id} still in flight after {OVERLAY_LANDING_WAIT:?}; \
                 proceeding without it"
            );
            break;
        };
        table = OVERLAY_LANDED
            .wait_timeout(table, left)
            .map(|(guard, _)| guard)
            .unwrap_or_else(|poisoned| poisoned.into_inner().0);
    }
    table
}

/// Drop every overlay surface, keeping the layers — the main surface is being
/// (re)installed or torn down, and the overlays share its device.
fn drop_overlay_surfaces() {
    if !overlays_enabled() {
        return;
    }
    let dropped = overlay_table().drop_surfaces();
    if dropped > 0 {
        log::debug!("frust-overlay: dropped {dropped} overlay surface(s) with the main surface");
    }
}

/// One frame's overlays, taken out of the table for the length of the frame:
/// the live surfaces and the cut each draws of this frame's scene.
struct FrameOverlays {
    surfaces: Vec<(u64, frust_render::overlay::OverlaySurface)>,
    cuts: Vec<OverlayCutSpec>,
    refused: Vec<(u64, String)>,
    unservable: Vec<(u64, String)>,
}

impl FrameOverlays {
    /// The cut ranges, in order — `encode_segmented`'s cuts.
    fn ranges(&self) -> Vec<std::ops::Range<usize>> {
        self.cuts.iter().map(|(_, range)| range.clone()).collect()
    }

    /// One `OverlayCut` per cut, in [`Self::ranges`]' order.
    fn overlay_cuts(&mut self) -> Vec<frust_render::overlay::OverlayCut<'_>> {
        let mut live: std::collections::BTreeMap<u64, &mut frust_render::overlay::OverlaySurface> =
            self.surfaces
                .iter_mut()
                .map(|(id, surface)| (*id, surface))
                .collect();
        self.cuts
            .iter()
            .filter_map(|(overlay_id, range)| {
                live.remove(overlay_id)
                    .map(|overlay| frust_render::overlay::OverlayCut {
                        overlay,
                        range: range.clone(),
                        overlay_id: *overlay_id,
                    })
            })
            .collect()
    }

    /// Note each cut's engine refusal or kept-on-main classification.
    fn note_outcomes(&mut self, outcomes: &[frust_render::overlay::OverlayCutOutcome]) {
        for outcome in outcomes {
            if let Err(error) = &outcome.result {
                self.refused.push((outcome.overlay_id, error.to_string()));
            }
            if !outcome.serviceability.is_servable() {
                self.unservable
                    .push((outcome.overlay_id, outcome.serviceability.to_string()));
            }
        }
    }
}

/// The cuts one segmented frame draws: one per overlay with a live surface,
/// carrying the range the frame carried (clamped to its `len` commands) — or
/// an empty range at the end (a transparent clear) when it carried none. An
/// overlay the frame names without a live surface gets no cut: its content
/// stays on the main surface. Ascending by range.
fn assemble_overlay_cuts(
    frame: &[OverlayCutSpec],
    live: impl IntoIterator<Item = u64>,
    len: usize,
) -> Vec<OverlayCutSpec> {
    let mut cuts: Vec<OverlayCutSpec> = live
        .into_iter()
        .map(|id| {
            let range = frame
                .iter()
                .find(|(cut, _)| *cut == id)
                .map(|(_, range)| range.start.min(len)..range.end.min(len))
                .filter(|range| range.start < range.end)
                .unwrap_or(len..len);
            (id, range)
        })
        .collect();
    cuts.sort_by_key(|(id, range)| (range.start, range.end, *id));
    cuts
}

/// Ahead of a frame of `len` commands cut at `frame_cuts`: build or resize
/// each published layer's surface and take the live ones out. `None` — one
/// cached read with overlays off — when none is live; the frame then takes the
/// single-surface path unchanged.
fn take_frame_overlays(
    renderer: &SurfaceRenderer,
    render_cx: &mut RenderContext,
    frame_cuts: &[OverlayCutSpec],
    len: usize,
) -> Option<FrameOverlays> {
    if !overlays_enabled() {
        return None;
    }
    let mut table = overlay_table();
    if table.is_empty() {
        return None;
    }
    let (surfaces, refusals) = table.take_for_frame(
        |layer, size| build_overlay_surface(render_cx, renderer, layer, size),
        |surface, (width, height)| surface.resize(width, height),
    );
    drop(table);
    for (overlay_id, error) in refusals {
        log::warn!(
            "frust-overlay: overlay {overlay_id} refused ({error}); its content stays on the main \
             surface"
        );
    }
    if surfaces.is_empty() {
        return None;
    }
    let cuts = assemble_overlay_cuts(frame_cuts, surfaces.iter().map(|(id, _)| *id), len);
    Some(FrameOverlays {
        surfaces,
        cuts,
        refused: Vec::new(),
        unservable: Vec::new(),
    })
}

/// Every [`OVERLAY_SKEW_EVERY`] segmented frames, one `frust-overlay cost`
/// line per overlay: its most recent GPU pass times (a `perf-trace` build on
/// an adapter with timestamp queries; nothing is logged without a reading).
fn log_overlay_cost(surfaces: &[(u64, frust_render::overlay::OverlaySurface)]) {
    static FRAMES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let frame = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    if !frame.is_multiple_of(OVERLAY_SKEW_EVERY) {
        return;
    }
    for (overlay_id, surface) in surfaces {
        if let Some(gpu) = surface.renderer().gpu_pass_timings() {
            log::info!(
                "frust-overlay cost overlay={overlay_id} gpu_prepass_us={} gpu_main_us={} \
                 gpu_composite_us={} size={:?}",
                gpu.prepass.as_micros(),
                gpu.main.as_micros(),
                gpu.composite.as_micros(),
                surface.size(),
            );
        }
    }
}

/// Hand a frame's overlay surfaces back once presented, waking a waiting
/// withdraw, and log what the frame noted (latched per overlay).
fn land_frame_overlays(overlays: FrameOverlays) {
    let FrameOverlays {
        surfaces,
        refused,
        unservable,
        ..
    } = overlays;
    log_overlay_cost(&surfaces);
    let mut table = overlay_table();
    table.land(surfaces);
    for (overlay_id, error) in refused {
        if let Some(count) = table.note_error(overlay_id, OVERLAY_ERRORS_LOGGED) {
            log::warn!("frust-overlay: overlay {overlay_id} frame refused ({count}): {error}");
        }
    }
    for (overlay_id, reason) in unservable {
        if table.note_unservable(overlay_id) {
            log::info!(
                "frust-overlay: overlay {overlay_id} content {reason}; it draws under the native \
                 view while that lasts (logged once)"
            );
        }
    }
    drop(table);
    OVERLAY_LANDED.notify_all();
}

/// Fold one frame's main-to-last-overlay present gap; a `frust-overlay skew`
/// line every [`OVERLAY_SKEW_EVERY`] frames.
fn record_overlay_presents(delta: Duration, overlays: usize) {
    let delta = u64::try_from(delta.as_micros()).unwrap_or(u64::MAX);
    let mut window = OVERLAY_SKEW
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (frames, sum, max) = &mut *window;
    *frames += 1;
    *sum = sum.saturating_add(delta);
    *max = (*max).max(delta);
    if *frames >= OVERLAY_SKEW_EVERY {
        log::info!(
            "frust-overlay skew frames={} avg_us={} max_us={} overlays={overlays}",
            *frames,
            *sum / u64::from(*frames),
            *max,
        );
        *window = (0, 0, 0);
    }
}

/// Build an overlay surface on the `CAMetalLayer*` at `layer` (an
/// exposed-provenance address the macOS host published), `size` physical px,
/// sharing the main surface's device.
///
/// **This crate's one `unsafe` block** — the sanctioned zone where the
/// `CALayer` pointer crosses into a `wgpu::Surface`
/// (`OverlaySurface::new`); nothing else in `frust-shell-desktop` holds any.
///
/// # Safety
///
/// Upheld by the publish/withdraw protocol, not by the caller of this safe
/// function: the macOS host publishes a layer its overlay view retains, and
/// before it releases that view it calls `withdraw_overlay_layer`, which waits
/// out a frame drawing the surface and drops the surface under the table's
/// lock. So the layer outlives the surface built here and every frame acquired
/// from it.
#[cfg(target_os = "macos")]
fn build_overlay_surface(
    render_cx: &mut RenderContext,
    renderer: &SurfaceRenderer,
    layer: usize,
    (width, height): (u32, u32),
) -> Result<frust_render::overlay::OverlaySurface, frust_render::overlay::OverlayError> {
    let layer = std::ptr::with_exposed_provenance_mut::<std::ffi::c_void>(layer);
    // SAFETY: see this function's `# Safety` — the published layer is alive
    // until its withdraw has dropped this surface.
    let surface = unsafe {
        frust_render::overlay::OverlaySurface::new(
            render_cx,
            Some(renderer),
            frust_render::overlay::OverlayTarget::CoreAnimationLayer(layer),
            width,
            height,
        )
    };
    if surface.is_ok() {
        log::info!(
            "frust-overlay: overlay surface up on CAMetalLayer {layer:p} ({width}x{height} px)"
        );
    }
    surface
}

/// Off macOS no host publishes a layer (`overlays_enabled` is `false`), so
/// nothing is ever built.
#[cfg(not(target_os = "macos"))]
fn build_overlay_surface(
    _render_cx: &mut RenderContext,
    _renderer: &SurfaceRenderer,
    _layer: usize,
    _size: (u32, u32),
) -> Result<frust_render::overlay::OverlaySurface, frust_render::overlay::OverlayError> {
    Err(frust_render::overlay::OverlayError::UnsupportedSurface)
}

/// The submit of a segmented frame: one encoder, one queue submit for the main
/// segment and every overlay's range, then the presents on this thread — main
/// first, then the overlays in cut order (bottom to top), the gap folded into
/// the skew log. Answers the main surface's outcome.
fn submit_with_overlays(
    renderer: &mut SurfaceRenderer,
    render_cx: &RenderContext,
    overlays: &mut FrameOverlays,
) -> Result<FrameOutcome> {
    let (outcome, presents) = {
        let mut cuts = overlays.overlay_cuts();
        renderer.submit_segmented_deferred(render_cx, &mut cuts)?
    };
    overlays.note_outcomes(&outcome.overlays);
    let frust_render::overlay::SegmentedPresents {
        main,
        overlays: overlay_frames,
    } = presents;
    let main_at = main.map(|frame| {
        frame.present();
        Instant::now()
    });
    let count = overlay_frames.len();
    for (_, frame) in overlay_frames {
        frame.present();
    }
    if let Some(main_at) = main_at
        && count > 0
    {
        static FIRST: std::sync::Once = std::sync::Once::new();
        FIRST.call_once(|| {
            log::info!(
                "frust-overlay: first segmented frame presented: main + {count} overlay(s), \
                 cuts {:?}, {} submit(s)",
                overlays.cuts,
                outcome.submits
            );
        });
        record_overlay_presents(main_at.elapsed(), count);
    }
    Ok(outcome.frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlay_switch_reads_the_first_non_empty_source() {
        assert!(!overlay_switch_from([None, None, None]));
        assert!(!overlay_switch_from([Some("0"), Some("1"), None]));
        assert!(overlay_switch_from([Some(" "), None, Some("1")]));
        for off in ["off", "OFF", "false", " False ", "0"] {
            assert!(!overlay_switch_value_on(off), "{off:?}");
        }
        for on in ["1", "on", "media", "3"] {
            assert!(overlay_switch_value_on(on), "{on:?}");
        }
    }

    #[test]
    fn overlays_stay_off_without_a_macos_host() {
        if !cfg!(target_os = "macos") {
            assert!(!overlays_enabled());
        }
    }

    /// A surface double logging its own drop.
    struct Tracked(String, std::rc::Rc<std::cell::RefCell<Vec<String>>>);

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.1.borrow_mut().push(format!("drop {}", self.0));
        }
    }

    type Log = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    fn build(log: &Log) -> impl FnMut(usize, (u32, u32)) -> Result<Tracked, String> + '_ {
        move |layer, _| {
            if layer == 0 {
                return Err("refused".to_string());
            }
            Ok(Tracked(format!("{layer:#x}"), log.clone()))
        }
    }

    #[test]
    fn a_withdraw_waits_for_the_frame_then_drops_the_surface_first() {
        let log = Log::default();
        let mut table: OverlayTable<Tracked> = OverlayTable::new();
        assert_eq!(table.publish(1, 0x10, (8, 6)), LayerChange::Adopted);
        assert_eq!(table.publish(1, 0x10, (8, 6)), LayerChange::Unchanged);
        let (flying, refusals) = table.take_for_frame(build(&log), |_, _| {});
        assert!(refusals.is_empty());
        // Mid-frame: a resize of the same layer lands, a replacement or a
        // withdraw waits.
        assert_eq!(table.publish(1, 0x10, (16, 12)), LayerChange::Resized);
        assert_eq!(table.publish(1, 0x20, (8, 6)), LayerChange::Busy);
        assert!(!table.withdraw(1));
        assert!(log.borrow().is_empty());
        assert_eq!(table.land(flying), 0);
        assert!(table.withdraw(1));
        assert_eq!(log.borrow().as_slice(), ["drop 0x10"]);
        assert!(table.is_empty());
    }

    #[test]
    fn the_main_surface_going_away_keeps_the_layers_and_retries_refusals() {
        let log = Log::default();
        let mut table: OverlayTable<Tracked> = OverlayTable::new();
        table.publish(1, 0x10, (1, 1));
        table.publish(2, 0, (1, 1));
        let (flying, refusals) = table.take_for_frame(build(&log), |_, _| {});
        assert_eq!(refusals.len(), 1);
        table.land(flying);
        let (flying, refusals) = table.take_for_frame(build(&log), |_, _| {});
        assert!(refusals.is_empty(), "a refusal is not retried every frame");
        table.land(flying);
        assert_eq!(table.drop_surfaces(), 1);
        assert_eq!(log.borrow().as_slice(), ["drop 0x10"]);
        let (flying, refusals) = table.take_for_frame(build(&log), |_, _| {});
        assert_eq!((flying.len(), refusals.len()), (1, 1));
        table.land(flying);
        assert_eq!(table.note_error(1, 1), Some(1));
        assert_eq!(table.note_error(1, 1), None);
        assert!(table.note_unservable(1));
        assert!(!table.note_unservable(1));
    }

    #[test]
    fn a_frame_cuts_only_live_surfaces_with_its_own_ranges() {
        let frame = [(1, 10..20), (2, 30..90), (3, 5..8)];
        assert_eq!(
            assemble_overlay_cuts(&frame, [1, 2, 4], 60),
            vec![(1, 10..20), (2, 30..60), (4, 60..60)]
        );
    }
}
