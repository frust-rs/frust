//! The desktop shell's frame executor — the render-path half of the frame loop
//! (plan phase 11.B, rollout 1/3), split out of [`app_handler`](crate::app_handler)
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
//!   verified in RESEARCH.md, *not* designed around any wgpu-`!Send` myth) and
//!   runs [`render_loop`]. The UI thread hands it finished frames across the
//!   depth-1 latest-wins [`render_channel`](frust_shell_common::render_channel)
//!   and drives surface lifecycle through owned [`RenderCommand`]s (with acks on
//!   the barriered ones). The render thread is the **single perf emitter**: it
//!   folds the UI thread's [`UiSpans`] together with its own [`RenderSpans`] via
//!   [`FramePasses::from_split`] and records the one frame.
//! - [`FrameExecutor::Inline`]: the pre-split fallback — the same
//!   [`RenderContext`]/[`SurfaceRenderer`] live on the UI thread and the
//!   encode→present tail runs synchronously inside `RedrawRequested`. Kept
//!   working (it is the fallback until 11.E validates the split); it shares
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

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use frust_core::FrameTime;
use frust_render::{
    AcquireOutcome, DetachedSurface, EncodeOutcome, FrameOutcome, RenderContext, SurfaceFactory,
    SurfacePhase, SurfaceRenderer,
};
use frust_scene::Scene;
use frust_shell_common::perf::{
    FramePasses, FrameStats, RenderSpans, SPAN_ADAPTER_READY, SPAN_DEVICE_READY,
    SPAN_FIRST_ENCODE_DONE, SPAN_FIRST_FRAME_PRESENTED, SPAN_FIRST_REBUILD_DONE, SPAN_INIT_ENTRY,
    SPAN_PIPELINE_CACHE_RESTORED, SPAN_RENDERER_READY, StartupSpans, UiSpans,
};
use frust_shell_common::{
    FrameMeta, RenderCommand, RenderPhase, RenderReceiver, RenderSender, SceneFrame,
    SceneReturnReceiver, SceneReturnSender, SurfaceSize, next_render_phase, render_channel,
    scene_return_channel,
};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

use crate::app_handler::ShellUserEvent;

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
}

/// The `SurfaceCreated` payload the UI thread hands the render thread — the `W`
/// type parameter of the [`render_channel`] (task 07's channel is generic over
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
            FrameExecutor::Inline(inline) => inline.renderer.on_surface_destroyed(),
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
                    .on_surface_changed(&inline.render_cx, size.width, size.height)
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
    /// tail synchronously (borrowing `scene`, so it is reused next frame — spec
    /// §7); the split moves the scene out into a [`SceneFrame`] and sends it
    /// across the channel (latest-wins), replacing it with a scene reclaimed off
    /// the render thread's give-back channel (review finding F5) — `reset()`,
    /// so its buffer is reused rather than reallocated — falling back to
    /// `Scene::new()` only when none is available yet (cold start, or the
    /// render thread hasn't given one back). `frame_time` and `size` populate
    /// the split's [`FrameMeta`]; inline ignores them.
    pub(crate) fn submit_frame(
        &mut self,
        window: &WindowHandle,
        scene: &mut Scene,
        base_color: peniko::Color,
        ui_spans: UiSpans,
        frame_time: FrameTime,
        size: SurfaceSize,
    ) {
        match self {
            FrameExecutor::Inline(inline) => {
                inline.submit_frame(window, scene, base_color, ui_spans)
            }
            FrameExecutor::Split(split) => {
                let replacement = split.take_reusable_scene();
                let painted = PaintedScene {
                    scene: std::mem::replace(scene, replacement),
                    base_color,
                };
                split.submit_frame(painted, ui_spans, frame_time, size);
            }
        }
    }
}

/// Spawn the render thread and return the [`SplitExecutor`] handle to it (plan
/// phase 11.B). The [`RenderContext`] is created here on the UI thread so its
/// [`SurfaceFactory`] (which the UI thread keeps for on-main-thread surface
/// creation) shares the same wgpu instance as the context the render thread
/// renders with; the context itself (`Send`) is then moved onto the thread.
pub(crate) fn spawn_render_thread(proxy: EventLoopProxy<ShellUserEvent>) -> SplitExecutor {
    let render_cx = RenderContext::new();
    let factory = render_cx.surface_factory();
    let (sender, receiver) = render_channel::<PaintedScene, DesktopSurface>();
    let (scene_return_tx, scene_return_rx) = scene_return_channel::<Scene>();
    let join = std::thread::Builder::new()
        .name("frust-render".to_string())
        // Guard the loop so a dev-build panic logs and exits cleanly (dropping the
        // owned `RenderReceiver`, which drains any orphaned `Ack` — the barrier
        // deadlock fix). A no-op under the release `panic = "abort"` profile.
        .spawn(move || {
            frust_shell_common::run_guarded_thread("frust-render (desktop)", move || {
                render_loop(receiver, proxy, render_cx, scene_return_tx)
            })
        })
        .expect("frust: failed to spawn render thread");
    SplitExecutor::new(sender, join, factory, scene_return_rx)
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
        }
    }

    fn ensure_surface(&mut self, window: &WindowHandle, size: SurfaceSize) -> Result<()> {
        // Inline runs on the UI thread, so the combined create-and-configure
        // `on_surface_created` (which reads the window handle) is fine here.
        let hit = load_pipeline_cache(&mut self.renderer);
        pollster::block_on(self.renderer.on_surface_created(
            &mut self.render_cx,
            window.clone(),
            size.width.max(1),
            size.height.max(1),
        ))
        .context("frust: failed to create render surface")?;
        persist_and_record(
            &self.renderer,
            &mut self.startup,
            &mut self.renderer_spans_recorded,
            hit,
        );
        Ok(())
    }

    fn submit_frame(
        &mut self,
        window: &WindowHandle,
        scene: &Scene,
        base_color: peniko::Color,
        ui_spans: UiSpans,
    ) {
        let outcome = render_frame(
            &mut self.renderer,
            &self.render_cx,
            window,
            scene,
            base_color,
            ui_spans,
            &mut self.frame_stats,
            &mut self.startup,
            &mut self.first_encode_recorded,
            &mut self.first_frame_recorded,
        );
        match outcome {
            // Stale swapchain (mid-resize): reconfigured internally — redraw
            // against the fresh configuration.
            Ok(FrameOutcome::Redraw) => window.request_redraw(),
            // Surface lost (rare on desktop): recreate from the same window and
            // redraw; log-and-continue on failure rather than killing the app.
            // Inline is on the UI thread, so `on_surface_created` works here.
            Ok(FrameOutcome::SurfaceLost) => {
                let size = window.inner_size();
                match pollster::block_on(self.renderer.on_surface_created(
                    &mut self.render_cx,
                    window.clone(),
                    size.width.max(1),
                    size.height.max(1),
                )) {
                    Ok(()) => window.request_redraw(),
                    Err(err) => eprintln!("frust: failed to recreate surface: {err}"),
                }
            }
            Ok(FrameOutcome::Rendered) | Ok(FrameOutcome::Skipped) => {}
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
    /// The UI-side half of the render thread's give-back channel (review
    /// finding F5): polled once per [`Self::submit_frame`] for a scene the
    /// render thread has finished with, so its buffer is reused instead of
    /// reallocating a fresh `Scene` every frame.
    scene_return: SceneReturnReceiver<Scene>,
    /// A scene reclaimed from [`RenderSender::send_scene`]'s returned stale
    /// frame (the UI thread outran the render thread, overwriting an
    /// un-taken scene in the latest-wins slot) — checked before
    /// [`Self::scene_return`] on the next [`Self::take_reusable_scene`] call
    /// so that buffer is reused too, rather than dropped.
    spare_scene: Option<Scene>,
}

impl SplitExecutor {
    pub(crate) fn new(
        sender: RenderSender<PaintedScene, DesktopSurface>,
        join: JoinHandle<()>,
        factory: SurfaceFactory,
        scene_return: SceneReturnReceiver<Scene>,
    ) -> Self {
        Self {
            sender: Some(sender),
            join: Some(join),
            factory,
            surface_requested: false,
            frame_id: 0,
            scene_return,
            spare_scene: None,
        }
    }

    /// Reclaim a reusable, empty `Scene` for the next frame (review finding
    /// F5): prefer a scene already reclaimed from a stale [`Self::submit_frame`]
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
                ui_spans,
            });
            // The UI thread outran the render thread: the just-overwritten,
            // never-rendered stale frame's scene is still perfectly reusable —
            // reclaim its buffer instead of letting it drop (review finding F5).
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

/// The dedicated render thread's loop (plan phase 11.B): own the
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
                    window = Some(w);
                }
                RenderCommand::SurfaceChanged { size } => {
                    renderer.on_surface_changed(&render_cx, size.width, size.height);
                }
                RenderCommand::SurfaceDestroyed { ack } => {
                    renderer.on_surface_destroyed();
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
                let outcome = render_frame(
                    &mut renderer,
                    &render_cx,
                    win,
                    &frame.scene.scene,
                    frame.scene.base_color,
                    frame.ui_spans,
                    &mut frame_stats,
                    &mut startup,
                    &mut first_encode_recorded,
                    &mut first_frame_recorded,
                );
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
                    Ok(FrameOutcome::Rendered) | Ok(FrameOutcome::Skipped) => {}
                    Err(err) => log::error!("frust: render error: {err}"),
                }
            }
            // Give the drained scene back for the UI thread to reclaim (review
            // finding F5) — whether it was actually rendered above, phase-gated
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
    pollster::block_on(renderer.on_surface_installed(render_cx, detached, width, height))
        .context("frust: failed to install render surface")?;
    persist_and_record(renderer, startup, renderer_spans_recorded, hit);
    Ok(())
}

/// Run the encode→acquire→submit tail for one painted `scene`, timing each span
/// with its own `Instant`, recording the folded [`FramePasses`] through the
/// single emitter, and stamping the first-encode/first-frame startup milestones.
/// Returns the [`FrameOutcome`] for the caller to act on (redraw / recover /
/// nothing) — the caller differs only in *how* it requests the follow-up redraw
/// (UI-side `request_redraw` inline; the proxy in the split). Shared by both
/// paths so the frame-pipeline logic is not forked.
#[allow(clippy::too_many_arguments)]
fn render_frame(
    renderer: &mut SurfaceRenderer,
    render_cx: &RenderContext,
    window: &Window,
    scene: &Scene,
    base_color: peniko::Color,
    ui_spans: UiSpans,
    frame_stats: &mut FrameStats,
    startup: &mut StartupSpans,
    first_encode_recorded: &mut bool,
    first_frame_recorded: &mut bool,
) -> Result<FrameOutcome> {
    window.pre_present_notify();

    // Encode span (GPU/CPU encode, no swapchain touch). Each span is its own
    // `Instant` read; desktop's frame budget is generous enough not to gate them
    // behind `perf::enabled()` (see docs/CODE_STANDARDS.md Instrumentation).
    let encode_start = Instant::now();
    let encode_outcome = renderer.encode(render_cx, scene, base_color);
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
        Ok(AcquireOutcome::Acquired) => renderer.submit(render_cx),
        Ok(AcquireOutcome::Reconfigured) => Ok(FrameOutcome::Redraw),
        Ok(AcquireOutcome::Lost) => Ok(FrameOutcome::SurfaceLost),
        Ok(AcquireOutcome::Skipped) => Ok(FrameOutcome::Skipped),
        Err(err) => Err(err),
    };
    let submit_dur = submit_start.elapsed();

    // One folded frame record through the single emitter (plan phase 11.B.3):
    // the UI thread's rebuild/layout/paint + this thread's encode/acquire/submit.
    frame_stats.record(FramePasses::from_split(
        ui_spans,
        RenderSpans {
            encode: encode_dur,
            acquire: acquire_dur,
            submit: submit_dur,
        },
    ));
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
