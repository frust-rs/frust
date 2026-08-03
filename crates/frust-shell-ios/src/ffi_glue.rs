//! The FFI boundary: the non-generic runtime the [`crate::ios_app!`]-stamped
//! `extern "C" fn frust_*`s delegate to, and where this crate's `unsafe` is
//! confined.
//!
//! The crate's `unsafe` surface is this module plus the `#[unsafe(no_mangle)]`
//! attributes the [`crate::ios_app!`] macro emits on its generated exports
//! (edition-2024 spells `no_mangle` as an unsafe attribute). Every entry point
//! here is wrapped in [`guard`](frust_shell_common::guard) so a panic is
//! caught and turned into a benign default instead of unwinding across the C-ABI
//! boundary (undefined behaviour). The `unsafe` *code* in this module is confined
//! to five things, each with a safety comment: the calls into
//! `on_surface_created_from_metal_layer` (raw `CAMetalLayer*` → surface — at init
//! in [`build_inline_executor`], on the render thread in [`install_surface`], and
//! on inline surface recovery in [`recover_surface`]), the
//! `accessibility::IosA11yAdapter::new` construction in [`init_accessibility`]
//! (raw `UIView*` → adapter, handed to the now-safe
//! `IosAppHandle::attach_accessibility`), `Box::into_raw`/`from_raw` for the
//! opaque handle's lifetime, reconstituting the raw handle pointer as a `&mut`,
//! the `unsafe impl Send` for [`SendableMetalLayer`] — the raw `CAMetalLayer*`
//! that crosses the UI→render channel in the split — and the
//! render-thread QoS self-boost `libc::pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0)`
//! at the top of [`render_loop`] (a bare libc call operating on
//! the calling thread only, best-effort and non-fatal).
//!
//! # Render-thread split
//!
//! When [`render_thread_enabled`](frust_shell_common::render_thread_enabled) is
//! set (the default; `FRUST_NO_RENDER_THREAD` opts out), [`create_handle`] spawns
//! the dedicated [`render_loop`] thread that owns the `RenderContext`/
//! `SurfaceRenderer` + surface and runs encode→acquire→submit; the UI thread
//! (CADisplayLink ticks) keeps rebuild→layout→paint and hands finished scenes
//! across the channel. Three iOS-specific contracts:
//!
//! - **Drawable acquisition moves to the render thread** — `nextDrawable` now runs
//!   off the UIKit main thread, so each frame's GPU work is wrapped in an
//!   `objc2::rc::autoreleasepool` drain (in [`render_scene`]) — that thread has no
//!   runloop pool.
//! - **`frust_pause` barriers on the render thread's ack** before returning: a
//!   suspended app that submits Metal work can be killed, so the UI thread blocks
//!   until the render thread has moved to `Paused` and quiesced (see
//!   [`IosAppHandle::pause`]).
//! - **The retained `CAMetalLayer` outlives the render thread**: [`frust_destroy`]
//!   joins the render thread (dropping its surface) *first*, before returning and
//!   letting Swift release the layer (see [`SendableMetalLayer`] and
//!   [`SplitExecutor`]'s `Drop`).
//!
//! Unlike Android (whose window is destroyed/recreated on rotation), iOS keeps the
//! same layer for the app's lifetime, so a lost surface self-heals **render-side**
//! from the retained pointer — there is no UI-side `SurfaceDestroyed` path.

use std::ffi::{CStr, CString, c_char, c_void};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Once};

use anyhow::{Context, Result, bail};

use frust_core::event::{EditingState, ImeContentType, ImeState};
use frust_reactive::{ReactiveRuntime, push_deep_link};
use frust_render::{RenderContext, SurfaceAlphaRequest, SurfacePhase, SurfaceRenderer};
use frust_scene::Scene;
use frust_shell_common::perf::{self, FrameStats, StartupSpans};
use frust_shell_common::{
    AppTree, RenderCommand, RenderPhase, RenderReceiver, SceneReturnSender, SurfaceMode,
    SurfaceModeWatcher, SurfaceSize, guard, next_render_phase, render_channel,
    render_thread_enabled, run_guarded_thread, scene_return_channel,
};
use frust_text::TextContext;

use crate::accessibility::IosA11yAdapter;
use crate::app::{
    FrameExecutor, InlineExecutor, IosAppHandle, PaintedScene, PresentHandoff, SplitExecutor,
    render_scene,
};
use crate::ffi_support::{CaretRect, publish_resolved_translucency};

/// Startup-span name: `create_handle` is about to join the
/// background font-preload thread [`create_handle`] spawned at the top of its
/// own body (see that fn's doc — iOS has no `JNI_OnLoad`-equivalent
/// process-wide load hook to spawn it earlier from, so the thread is spawned
/// as early as possible inside `create_handle` itself instead, overlapping the
/// synchronous GPU surface/device bring-up below it). Mirrors the Android
/// shell's `SPAN_FONT_PREINIT_STARTED`/`SPAN_FONT_PREINIT_JOINED` pair.
const SPAN_FONT_PREINIT_STARTED: &str = "font_preinit_started";

/// Startup-span name: the background font-preload join returned —
/// the pre-built [`TextContext`] was adopted, or `create_handle` fell back to
/// a synchronous [`TextContext::new`]. See [`SPAN_FONT_PREINIT_STARTED`].
const SPAN_FONT_PREINIT_JOINED: &str = "font_preinit_joined";

/// A minimal `log::Log` writing to stderr, installed once in [`init`].
///
/// Rationale: `simctl launch --console-pty` captures a simulator
/// process's stdout/stderr directly, so unified logging (`os_log`) is
/// unnecessary — a stderr sink is enough and adds zero new dependencies.
struct StderrLogger {
    level: log::LevelFilter,
}

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            // A failed write to stderr is nothing we can act on here; drop it.
            let _ = writeln!(
                std::io::stderr(),
                "[frust {}] {}",
                record.level(),
                record.args()
            );
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// Install the stderr logger exactly once per process, so `log::*` from any crate
/// in the graph reaches the simulator/device console. Default level is `Info`;
/// `FRUST_LOG` (e.g. `debug`, `trace`, `warn`) overrides it.
fn init_logger_once() {
    static LOGGER: Once = Once::new();
    LOGGER.call_once(|| {
        let level = std::env::var("FRUST_LOG")
            .ok()
            .and_then(|raw| raw.parse::<log::LevelFilter>().ok())
            .unwrap_or(log::LevelFilter::Info);
        // Leak the logger so it satisfies `set_logger`'s `&'static` bound; it
        // lives for the whole process, installed at most once.
        let logger: &'static StderrLogger = Box::leak(Box::new(StderrLogger { level }));
        if log::set_logger(logger).is_ok() {
            log::set_max_level(level);
        }
    });
}

/// Borrow the boxed [`IosAppHandle`] behind a raw pointer, or `None` if the Swift
/// side has no live native handle (null).
///
/// # Safety
///
/// When non-null, `handle` must be a pointer previously returned by [`init`] and
/// not yet passed to [`destroy`]. The returned reference must not outlive the
/// current native call (it aliases the boxed handle the Swift side still owns).
unsafe fn handle_mut<'a>(handle: *mut c_void) -> Option<&'a mut IosAppHandle> {
    if crate::ffi_support::handle_is_null(handle) {
        return None;
    }
    // SAFETY: guaranteed by this fn's safety contract — `handle` is a live
    // `Box<IosAppHandle>` pointer, uniquely reconstituted as a &mut for the
    // duration of one single-threaded (main-thread) native call.
    Some(unsafe { &mut *(handle as *mut IosAppHandle) })
}

/// `frust_init`: build the native handle for the app's `CAMetalLayer` and
/// return it to Swift as an opaque pointer.
///
/// `make_app` is supplied by the macro and erases the app's `State`/`app_logic`;
/// on any failure (null layer, GPU init error, panic) returns null, matching
/// Swift's "no native side yet" sentinel.
pub fn init(
    metal_layer: *mut c_void,
    width: u32,
    height: u32,
    scale: f32,
    make_app: impl FnOnce() -> Box<dyn AppTree>,
) -> *mut c_void {
    init_logger_once();
    guard("frust_init", std::ptr::null_mut(), || {
        match create_handle(metal_layer, width, height, scale, make_app) {
            Ok(handle) => handle,
            Err(err) => {
                log::error!("frust-shell-ios: frust_init failed: {err:#}");
                std::ptr::null_mut()
            }
        }
    })
}

/// `frust_init_accessibility`: attach the accesskit adapter to the app's
/// `FrustView`.
///
/// A distinct FFI entry from [`init`] because the accesskit `SubclassingAdapter`
/// dynamically subclasses the **UIView**, whereas [`init`] only receives the
/// `CAMetalLayer` (the GPU surface has no back-pointer to its owning view). Swift
/// calls this once, on the first layout, immediately after `frust_init`
/// returned a live handle and before the view is shown/focused — the window in
/// which `SubclassingAdapter::new` must run (see [`crate::accessibility`]).
///
/// Both a null handle and a null view are benign no-ops (defensive, mirroring
/// every other export): Swift only calls this with a non-null handle it just got
/// from `frust_init` and `self.view`, but the native side never trusts that.
/// Idempotent-adjacent: accesskit_ios panics if an adapter is *already* attached
/// to the view, so Swift calls this exactly once (latched by the `handle == nil`
/// first-init branch it lives in).
pub fn init_accessibility(handle: *mut c_void, view: *mut c_void) {
    guard("frust_init_accessibility", (), || {
        if crate::ffi_support::handle_is_null(view) {
            log::warn!(
                "frust-shell-ios: frust_init_accessibility called with a null view; \
                 accessibility not attached"
            );
            return;
        }
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            // The `unsafe` adapter construction is confined here, in `ffi_glue`
            // (this crate's sanctioned raw-pointer zone), rather than leaking into
            // `app.rs` — mirroring how the `on_surface_created_from_metal_layer`
            // calls stay in this module. The resulting adapter is handed to the
            // now-safe `IosAppHandle::attach_accessibility`.
            //
            // SAFETY: `view` is the app's live, unreleased `FrustView` (UIView)
            // pointer, delivered on the main thread on first layout before the view
            // is shown — the contract `IosA11yAdapter::new`/accesskit_ios require.
            let adapter = unsafe { IosA11yAdapter::new(view) };
            app.attach_accessibility(adapter);
        }
    });
}

/// A no-op [`frust_reactive::FrameWaker`] for the mobile shell: the
/// `CADisplayLink` loop already produces every frame regardless of a signal
/// write, so there is nothing useful for the waker to do (contrast the
/// desktop shell, which must nudge `ControlFlow::Wait` awake).
fn no_op_waker() -> frust_reactive::FrameWaker {
    std::sync::Arc::new(|| {})
}

/// Drains the UI-thread reactive local-task queue if the runtime has been
/// installed. A no-op before `frust_init` has run (nothing to pump yet) —
/// every call site below is reachable from Swift before init on a
/// misbehaving caller, so this stays defensive rather than assuming `Some`.
fn pump_reactive() {
    if let Some(rt) = ReactiveRuntime::get() {
        rt.pump_local();
    }
}

// ---------------------------------------------------------------------
// Render-thread split
// ---------------------------------------------------------------------

/// A raw `CAMetalLayer*` made `Send` so it can cross the UI→render-thread
/// scene-handoff channel as the `SurfaceCreated` payload — the
/// `W` type parameter of the shared
/// [`render_channel`](frust_shell_common::render_channel), which each shell picks
/// (desktop pairs a `DetachedSurface`; the mobile shells pass a raw pointer, since
/// surface creation from a `CAMetalLayer*`/`ANativeWindow*` has no main-thread
/// requirement — the render thread creates the surface itself).
///
/// # Safety
///
/// The wrapped pointer is the Swift-owned `CAMetalLayer*` retained by the UI
/// thread's [`IosAppHandle`] (`metal_layer`). Swift guarantees the layer outlives
/// the handle: it calls `frust_destroy` before releasing the view/layer, and
/// `frust_destroy` **joins the render thread first** (dropping the surface built
/// from this pointer) via [`SplitExecutor`]'s `Drop` — so the pointer is valid for
/// the whole lifetime of any surface the render thread creates from it, and the
/// render thread never touches it after Swift frees it. The two threads never use
/// it concurrently: the UI thread only reads the `IosAppHandle`'s pointer to
/// construct this wrapper; the render thread only reads it back to create/recreate
/// the surface ([`install_surface`]). That single-owner, join-ordered handoff is
/// what makes the `unsafe impl Send` sound.
pub(crate) struct SendableMetalLayer(*mut c_void);

// SAFETY: see the type's docs — the wrapped `CAMetalLayer*` is kept alive by Swift
// across the destroy-join and is never used by two threads concurrently.
unsafe impl Send for SendableMetalLayer {}

impl SendableMetalLayer {
    /// Wrap a raw `CAMetalLayer*`. The caller upholds the type's safety contract
    /// (the layer outlives every surface built from it, on the render thread);
    /// constructing the wrapper itself is a plain field store.
    pub(crate) fn new(ptr: *mut c_void) -> Self {
        Self(ptr)
    }

    /// The wrapped raw pointer, for [`install_surface`] to build the surface from.
    pub(crate) fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}

/// Install (or reinstall) a `wgpu::Surface` on `renderer` from the raw
/// `CAMetalLayer*` `metal_layer`, on the render thread. On the
/// **first** install it also records the adapter/device/renderer startup spans,
/// mirroring the pre-split `create_handle` flow (which now happens render-side in
/// the split). iOS/Metal has no pipeline cache (that is Vulkan-only), so — unlike
/// the Android shell's `install_surface` — there is no cache load/persist here.
///
/// The unsafe `on_surface_created_from_metal_layer` call is confined here (a
/// sanctioned zone); its `SAFETY` note states the cross-thread ownership contract
/// [`SendableMetalLayer`] documents.
#[allow(clippy::too_many_arguments)]
fn install_surface(
    renderer: &mut SurfaceRenderer,
    render_cx: &mut RenderContext,
    metal_layer: *mut c_void,
    width: u32,
    height: u32,
    startup_spans: &mut Option<StartupSpans>,
    first_install: bool,
    alpha: SurfaceAlphaRequest,
    translucent_resolved: &AtomicBool,
) -> Result<()> {
    // SAFETY: `metal_layer` is the Swift-owned `CAMetalLayer*` the UI thread's
    // `IosAppHandle` keeps alive until `frust_destroy` — which joins THIS render
    // thread first (dropping this surface) before Swift releases the layer — so it
    // outlives every surface created from it here. See `SendableMetalLayer`'s docs.
    pollster::block_on(unsafe {
        renderer.on_surface_created_from_metal_layer(
            render_cx,
            metal_layer,
            width.max(1),
            height.max(1),
            alpha,
        )
    })
    .context("frust-shell-ios: failed to create Metal render surface")?;
    // Publish the surface's RESOLVED translucency to the UI thread (review
    // finding M1): `alpha` above is only what we ASKED for — `frust-render`
    // resolves it against the layer's advertised alpha modes and can fall back
    // to an opaque swapchain. The UI thread reads this flag every frame
    // (`IosAppHandle::sync_translucent_resolved`) before choosing the base
    // color and pushing `set_surface_translucent`, so a fallback degrades to
    // the Mode A contract instead of `DestOut`-punching black rectangles.
    // Written on EVERY (re)install, including the render-side self-heal.
    publish_resolved_translucency(
        translucent_resolved,
        Some(renderer.surface_resolved_translucent()),
    );
    if first_install && let Some(spans) = startup_spans.as_mut() {
        spans.record(perf::SPAN_ADAPTER_READY);
        spans.record(perf::SPAN_DEVICE_READY);
        spans.record(perf::SPAN_RENDERER_READY);
    }
    Ok(())
}

/// The dedicated render thread's loop: create + own the
/// `RenderContext`/`SurfaceRenderer` + surface wholesale, drain lifecycle commands
/// and the freshest handed-off scene from the channel, and run
/// encode→acquire→submit for each frame — the single perf emitter (folding the UI
/// thread's `UiSpans` with its own render spans via [`render_scene`]). Owns the
/// startup line from `adapter_ready` onward (the UI thread recorded `init_entry` +
/// the font-preinit spans before moving the recorder here). Exits cleanly when the
/// `RenderSender` is dropped.
///
/// # iOS surface recovery
///
/// The `CAMetalLayer` is permanent (Swift never re-delivers it), so nothing
/// external re-drives surface creation after a `SurfaceLost`. This loop therefore
/// self-heals **render-side**: it retains the layer pointer + last size and, when a
/// frame leaves the renderer in `SurfaceLost`, recreates the surface here on the
/// same wakeup — bounded by [`MAX_RECREATE_ATTEMPTS`](crate::ffi_support::MAX_RECREATE_ATTEMPTS)
/// consecutive failures so a persistently-failing recreate cannot storm. (The
/// inline path keeps its UI-side `should_recreate_surface` recovery in
/// `frust_render_frame`/`frust_resize`.)
///
/// `fatal` is the per-shell fatal flag: this thread stores
/// `true` into it if the **first** surface install fails, so the UI thread's
/// [`render_frame`] returns [`FRAME_FATAL`](crate::ffi_support::FRAME_FATAL) and
/// Swift latches `initFailed` + invalidates its `CADisplayLink` (a first-install
/// failure — an incapable GPU/driver — is unrecoverable and otherwise leaves a
/// permanent black screen with no platform signal). Later reinstall/self-heal
/// failures stay log-only.
///
/// `translucent_resolved` is the resolved-translucency seam: this thread
/// creates the surface — including the self-heal above — so
/// only it can see whether the requested translucent alpha mode was actually
/// granted. It stores the outcome on every (re)install (and clears it on a
/// failed one) for the UI thread, which owns the `RenderRoot` and the
/// per-frame base color, to read each frame. Seeded from the REQUEST by
/// [`spawn_split_executor`], so the common (capable) case is Mode B from frame
/// 1 and only a real resolution can downgrade it.
///
/// `present` is the render side of the present-sync handoff:
/// when the host armed it, this thread hands each submitted frame to the UI
/// thread to present inside the platform-view `CATransaction` instead of
/// presenting here (see [`crate::app::PresentHandoff`]). Any parked frame is
/// dropped around a surface (re)install below — it belongs to a swapchain that
/// no longer exists.
///
/// `surface_reinstalled` is the UI thread's only notice that the self-heal above
/// happened: this thread sets it on every self-heal attempt
/// so the next `CADisplayLink` tick runs a real frame against the fresh
/// swapchain instead of being skipped by the now-actually-idling frame gate. See
/// [`crate::app::SplitExecutor::surface_reinstalled`] for the full rationale.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_loop(
    receiver: RenderReceiver<PaintedScene, SendableMetalLayer>,
    startup_spans: StartupSpans,
    fatal: Arc<AtomicBool>,
    scene_return: SceneReturnSender<Scene>,
    presented: Arc<AtomicU64>,
    alpha: SurfaceAlphaRequest,
    translucent_resolved: Arc<AtomicBool>,
    present: Arc<PresentHandoff>,
    surface_reinstalled: Arc<AtomicBool>,
) {
    // Render-thread QoS self-boost: tag this dedicated render
    // thread as user-interactive so the scheduler treats its GPU submit work at
    // the same tier as the UI thread (a plain `std::thread` starts at a lower,
    // utility-ish QoS). Best-effort — a non-zero return is logged, never fatal
    // (this loop never panics).
    //
    // SAFETY: `pthread_set_qos_class_self_np` is a plain libc call operating on
    // the calling thread only, with no memory-safety preconditions. A
    // sanctioned-unsafe FFI call confined to this module (see the module docs).
    unsafe {
        let rc =
            libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
        if rc != 0 {
            log::warn!(
                "frust-shell-ios: render-thread pthread_set_qos_class_self_np(USER_INTERACTIVE) \
                 failed (rc {rc}); continuing at default QoS"
            );
        }
    }

    let mut render_cx = RenderContext::new();
    let mut renderer = SurfaceRenderer::new();
    let mut startup_spans = Some(startup_spans);
    let mut frame_stats = FrameStats::new();
    let mut phase = RenderPhase::NoSurface;
    let mut first_install_done = false;
    let mut first_rebuild_recorded = false;
    // iOS self-recovery state (the layer is permanent — recreate from it on loss).
    let mut metal_layer: Option<*mut c_void> = None;
    let mut physical: (u32, u32) = (1, 1);
    let mut recreate_failures: u8 = 0;

    loop {
        let batch = receiver.wait_next();

        // Lifecycle commands first (FIFO), updating the phase machine.
        for command in batch.commands {
            phase = next_render_phase(phase, command.event());
            match command {
                RenderCommand::SurfaceCreated { window, size } => {
                    let ptr = window.as_ptr();
                    metal_layer = Some(ptr);
                    physical = (size.width, size.height);
                    let first_install = !first_install_done;
                    // Present-sync: a frame parked for the UI thread
                    // belongs to the swapchain about to be replaced — drop it
                    // rather than let the next tick present a frame from a dead
                    // surface.
                    present.clear();
                    match install_surface(
                        &mut renderer,
                        &mut render_cx,
                        ptr,
                        size.width,
                        size.height,
                        &mut startup_spans,
                        first_install,
                        alpha,
                        &translucent_resolved,
                    ) {
                        Ok(()) => {
                            first_install_done = true;
                            recreate_failures = 0;
                        }
                        Err(err) => {
                            // A failed (re)install leaves no surface whose
                            // translucency we can vouch for — clear the flag
                            // rather than leaving the previous surface's value
                            // standing (never punch a hole you
                            // can't prove is a window).
                            publish_resolved_translucency(&translucent_resolved, None);
                            log::error!(
                                "frust-shell-ios: render-thread surface install failed: {err:#}"
                            );
                            // First-install failure is fatal + unrecoverable (an
                            // incapable GPU/driver can't change mid-process).
                            // Signal the UI thread so `frust_render_frame` returns
                            // FRAME_FATAL and Swift stops the CADisplayLink instead
                            // of driving doomed frames against a permanent black
                            // screen. Later reinstall/self-heal
                            // failures below stay log-only.
                            if first_install {
                                fatal.store(true, Ordering::Release);
                            }
                        }
                    }
                }
                RenderCommand::SurfaceChanged { size } => {
                    physical = (size.width, size.height);
                    renderer.on_surface_changed(&render_cx, size.width, size.height);
                }
                RenderCommand::SurfaceDestroyed { ack } => {
                    // iOS never sends `SurfaceDestroyed` in v1 (the layer is
                    // permanent, so there is no UI-side destroy path); honor the
                    // barrier defensively so a stray one can never deadlock the UI
                    // thread.
                    renderer.on_surface_destroyed();
                    ack.acknowledge();
                }
                RenderCommand::Pause { ack } => {
                    // The iOS backgrounding barrier:
                    // `next_render_phase` has already moved us to `Paused`, so no
                    // scene handed off from here on is submitted. Acknowledge only
                    // now — so `frust_pause` returns (and the app may background)
                    // strictly AFTER the render thread has quiesced. No Metal
                    // submission from a suspended app.
                    ack.acknowledge();
                }
                RenderCommand::Resume => {}
            }
        }

        // Then the freshest scene, only if the phase allows submitting (a scene
        // handed off while paused is dropped from presentation, not silently
        // dropped altogether — see the give-back below).
        if let Some(frame) = batch.scene {
            if phase.can_render() {
                // The first handed-off scene marks the first UI frame produced —
                // the render thread's stand-in for `first_rebuild_done` (it owns
                // the startup line in the split).
                if !first_rebuild_recorded {
                    if let Some(spans) = startup_spans.as_mut() {
                        spans.record(perf::SPAN_FIRST_REBUILD_DONE);
                    }
                    first_rebuild_recorded = true;
                }
                // FFI-path perf convention: read `perf::enabled()` once per
                // frame, gate every `Instant::now()` behind it (inside
                // `render_scene`).
                let perf_on = perf::enabled();
                render_scene(
                    &mut renderer,
                    &render_cx,
                    &frame.scene.scene,
                    frame.scene.base_color,
                    frame.ui_spans,
                    &mut frame_stats,
                    &mut startup_spans,
                    perf_on,
                    &presented,
                    // Present-sync: park this frame under its own id so the UI
                    // thread can tell the platform-view release gate which
                    // frame it just put on screen.
                    Some((&present, frame.meta.frame_id)),
                );

                // iOS self-heals a lost surface from the retained layer (nothing
                // external re-drives creation), bounded by the failure budget so
                // a persistently-failing recreate cannot storm.
                if renderer.phase() == SurfacePhase::SurfaceLost
                    && recreate_failures < crate::ffi_support::MAX_RECREATE_ATTEMPTS
                    && let Some(ptr) = metal_layer
                {
                    // Same reason as the `SurfaceCreated` arm above: nothing
                    // parked for the old (lost) swapchain may survive into the
                    // new one.
                    present.clear();
                    // Tell the UI thread a self-heal happened
                    // — set for the ATTEMPT, not just a success: either way
                    // the frame that was in flight never presented, and the next
                    // tick must run rather than be skipped by the frame gate. On
                    // success it repaints the fresh swapchain; on failure it
                    // hands this arm another scene to retry from, still bounded
                    // by `MAX_RECREATE_ATTEMPTS` above (once the budget is spent
                    // this arm stops running, so the flag stops being set and the
                    // loop settles back to idle instead of storming).
                    surface_reinstalled.store(true, Ordering::Release);
                    match install_surface(
                        &mut renderer,
                        &mut render_cx,
                        ptr,
                        physical.0,
                        physical.1,
                        &mut startup_spans,
                        false,
                        alpha,
                        &translucent_resolved,
                    ) {
                        Ok(()) => recreate_failures = 0,
                        Err(err) => {
                            // Same clear-on-failure contract as the install arm
                            // above — a failed self-heal must not
                            // leave the lost surface's translucency standing.
                            publish_resolved_translucency(&translucent_resolved, None);
                            recreate_failures = recreate_failures.saturating_add(1);
                            log::error!(
                                "frust-shell-ios: render-thread surface recreate failed \
                                 ({recreate_failures}): {err:#}"
                            );
                        }
                    }
                }
            }
            // Give the drained scene back for the UI thread to reclaim —
            // whether it was actually rendered above or
            // phase-gated out (`Paused`/`NoSurface`); `render_scene`'s encode
            // step has already fully consumed the scene's commands by this
            // point, so its buffer is safe to reuse. Never silently dropped.
            scene_return.give_back(frame.scene.scene);
        }

        if batch.disconnected {
            break;
        }
    }
}

/// Fallible body of [`init`], separated so the happy path reads top-down.
fn create_handle(
    metal_layer: *mut c_void,
    width: u32,
    height: u32,
    scale: f32,
    make_app: impl FnOnce() -> Box<dyn AppTree>,
) -> Result<*mut c_void> {
    // Startup-span recorder: begins its epoch
    // right here. Every call against it is a no-op when perf instrumentation
    // is disabled (see `perf::enabled`). There is no iOS-side hook for
    // `SPAN_NATIVE_LIB_LOAD` — by the time this function runs, the dylib is
    // already loaded and executing Rust — so `SPAN_INIT_ENTRY` is the first
    // span this shell records (mirrors the Android shell's `create_handle`).
    let mut startup = StartupSpans::begin();
    startup.record(perf::SPAN_INIT_ENTRY);

    if crate::ffi_support::handle_is_null(metal_layer) {
        bail!("frust-shell-ios: frust_init called with a null CAMetalLayer");
    }

    // Font/`TextContext` warmup: iOS has no `JNI_OnLoad`-style
    // process-wide load hook to start this earlier from (unlike the Android
    // shell — `frust_init` is the earliest Rust entry point Swift ever calls),
    // so spawn the background thread here, as the very first thing, right
    // before the GPU surface/device bring-up below — the iOS counterpart to
    // Android's pre-init overlap window. `TextContext::new` has no fallible step,
    // so this thread cannot fail, only panic (handled at the join inside the
    // executor builders). Joined as late as possible (right before layout needs
    // it) to maximize overlap with the GPU work.
    let font_preinit = std::thread::spawn(TextContext::new);
    let physical = (width.max(1), height.max(1));

    // Read the translucent opt-in latch BEFORE any
    // surface is created: `frust_set_surface_mode` must be called before
    // `frust_init` for it to take effect (see
    // `frust_shell_common::surface_mode`'s module docs — a one-way,
    // pre-surface-creation latch). Mapped once here and threaded through both
    // executor paths' surface-creation call sites plus the handle's recreate
    // logic — this is the REQUEST only; the base color and the hole-punch
    // contract follow the RESOLVED outcome (`translucent_resolved` below).
    // `TranslucentPreferred` resolves to `PostMultiplied` on Metal — the
    // shipping mode (see `SurfaceAlphaRequest`'s doc comment
    // for the alpha-semantics caveat: vello outputs premultiplied, and
    // whether wgpu's Metal backend converts under `PostMultiplied` awaits
    // on-device verification — nothing speculative is built here).
    let alpha = match SurfaceModeWatcher::current() {
        SurfaceMode::Translucent => SurfaceAlphaRequest::TranslucentPreferred,
        SurfaceMode::Opaque => SurfaceAlphaRequest::Opaque,
    };

    // Build the render-path executor, chosen once by the
    // `FRUST_NO_RENDER_THREAD` kill switch:
    //
    // - Split (default): spawn the dedicated render thread that owns the
    //   `RenderContext`/`SurfaceRenderer` + surface and does all GPU work
    //   (drawable acquisition, encode, present) — so `frust_init` returns without
    //   blocking on adapter/device/surface bring-up (it happens render-side). The
    //   `startup` recorder is moved into the render thread, which owns the startup
    //   line from `adapter_ready` on.
    // - Inline (kill switch engaged): create the surface + renderer on this UI
    //   thread exactly as the pre-split shell did, recording the full startup line
    //   here.
    //
    // Both retain `metal_layer` in `IosAppHandle` (Swift owns it and releases it
    // only after `frust_destroy` — which, in the split, joins the render thread
    // first, dropping the surface built from the pointer).
    // The resolved-translucency seam. SEEDED FROM THE
    // REQUEST: the surface is created asynchronously on the render thread in
    // the default split, and an all-but-certain grant (Metal resolves
    // `PostMultiplied`) should not cost a Mode-A flash on frame 1 — so the
    // optimistic value stands until the render thread reports a real
    // resolution, which can only ever downgrade it. One clone per surface
    // owner (render thread or inline renderer), one in the handle for the UI
    // thread's per-frame read.
    let translucent_resolved = Arc::new(AtomicBool::new(
        alpha == SurfaceAlphaRequest::TranslucentPreferred,
    ));

    // Read the present-sync latch beside the translucency one
    // above, and for the same reason: `frust_set_present_sync` must precede
    // `frust_init`, and the choice is fixed for the surface's lifetime. Armed,
    // the render thread parks each submitted frame for the UI thread to present
    // inside the platform-view `CATransaction` (see `PresentHandoff`); unarmed
    // — the default — the slot is inert and the render tail is byte-for-byte
    // today's. Never armed on the inline path: that tail already presents on
    // the UI thread.
    let present = Arc::new(PresentHandoff::new(present_sync_enabled()));

    let (executor, text_ctx) = if render_thread_enabled() {
        spawn_split_executor(
            startup,
            metal_layer,
            physical,
            scale,
            font_preinit,
            alpha,
            Arc::clone(&translucent_resolved),
            present,
        )
    } else {
        build_inline_executor(
            startup,
            metal_layer,
            physical,
            font_preinit,
            alpha,
            &translucent_resolved,
        )?
    };

    // Process-wide reactive runtime init (idempotent — `ReactiveRuntime::init`'s
    // own `OnceLock` provides the process-once property). The Swift-side
    // `handle`/`initFailed` guards are only per-view-controller: a locale
    // change, split-screen resize, or an init retry after a prior failure can
    // re-enter `frust_init` in the same process (the project's own
    // g2-swift-init-latch history shows this happens), and a repeat call here
    // must be benign rather than rebuilding the background tokio runtime. Runs
    // BEFORE app construction so a `Component::init` (a future task) creating
    // signals/controllers has a live runtime to create them against. Runs on this
    // UI thread (never the render thread): `init` claims the *calling* thread as
    // the UI thread for `spawn_local`, and every C-ABI call arrives on the main
    // thread. The continuous `CADisplayLink` loop already ticks every frame
    // regardless of a signal write, so the waker is a no-op (mirrors Android).
    let rt = ReactiveRuntime::init(no_op_waker());

    // Construct the app AND its handle under the root `Owner`. `make_app` runs
    // `Component::init` (via `new_boxed_app_with`'s state factory), and
    // `IosAppHandle::new` runs the initial `rebuild()` — both must see an
    // ambient `Owner` or `provide_context`/`on_cleanup` silently no-op. A root
    // component has no enclosing component to supply one, so it registers
    // against the root owner (process lifetime, never disposed), mirroring the
    // desktop shell's per-frame `with_owner` wrap and the facade `run()` init.
    // Retain `metal_layer` in the handle so a later `SurfaceLost` can be recovered
    // by recreating the surface from it (iOS never re-delivers the layer). The
    // initial `rebuild()` records `first_rebuild_done` via
    // `IosAppHandle::new` → `FrameExecutor::record_first_rebuild` — inline records
    // it here, the split records it render-side on the first handed-off scene.
    let handle = rt.with_owner(|| {
        let app = make_app();
        IosAppHandle::new(
            executor,
            text_ctx,
            metal_layer,
            physical,
            scale,
            alpha,
            translucent_resolved,
            app,
        )
    });

    // SAFETY: hand a uniquely-owned boxed handle to Swift as a raw pointer; it is
    // reclaimed exactly once in `destroy`.
    Ok(Box::into_raw(Box::new(handle)) as *mut c_void)
}

/// Build the **inline** (`FRUST_NO_RENDER_THREAD`) executor: create the renderer +
/// surface on this UI thread and record the full startup line, exactly as the
/// pre-split shell did (kill-switch path). Returns the executor
/// plus the joined [`TextContext`] the handle needs for layout.
fn build_inline_executor(
    mut startup: StartupSpans,
    metal_layer: *mut c_void,
    physical: (u32, u32),
    font_preinit: std::thread::JoinHandle<TextContext>,
    alpha: SurfaceAlphaRequest,
    translucent_resolved: &AtomicBool,
) -> Result<(FrameExecutor, TextContext)> {
    let mut render_cx = RenderContext::new();
    let mut renderer = SurfaceRenderer::new();

    // SAFETY: `metal_layer` is a valid, live `CAMetalLayer*` per the FFI contract
    // (owned by the Swift `UIView`, guaranteed to outlive this handle because the
    // Swift side calls `frust_destroy` before releasing the view/layer).
    pollster::block_on(unsafe {
        renderer.on_surface_created_from_metal_layer(
            &mut render_cx,
            metal_layer,
            physical.0,
            physical.1,
            alpha,
        )
    })
    .context("frust-shell-ios: failed to create Metal render surface")?;
    // Replace the request-seeded optimism with the real resolution —
    // on this path the renderer lives on the UI thread, so the
    // handle's per-frame sync re-reads it from the renderer anyway; storing it
    // here keeps the flag correct for the construction-time push too.
    publish_resolved_translucency(
        translucent_resolved,
        Some(renderer.surface_resolved_translucent()),
    );
    // `RenderContext::ensure_device` creates the adapter, the logical device, and
    // this call's surface/renderer readiness in one async chain with no
    // finer-grained seam — all three spans land at this single point (an
    // acknowledged granularity limit, same shape as the Android shell).
    startup.record(perf::SPAN_ADAPTER_READY);
    startup.record(perf::SPAN_DEVICE_READY);
    startup.record(perf::SPAN_RENDERER_READY);

    // Join the font-preload thread as late as possible (max overlap with the GPU
    // work above). Best-effort: a panicked thread falls back to a synchronous
    // `TextContext::new` with a log line — kill nothing, defer nothing silently.
    startup.record(SPAN_FONT_PREINIT_STARTED);
    let text_ctx = font_preinit.join().unwrap_or_else(|_| {
        log::warn!(
            "frust-shell-ios: font pre-init thread panicked; \
             falling back to synchronous TextContext::new"
        );
        TextContext::new()
    });
    startup.record(SPAN_FONT_PREINIT_JOINED);

    let executor =
        FrameExecutor::Inline(Box::new(InlineExecutor::new(render_cx, renderer, startup)));
    Ok((executor, text_ctx))
}

/// Spawn the **split** (default) render thread and return its executor handle.
/// The GPU work — surface creation, drawable acquisition,
/// encode, present, adapter/device/renderer spans — happens *on the render thread*
/// ([`render_loop`]), so `frust_init` never blocks the UI thread on it. Only the
/// font-preload join (needed by UI-side layout) stays on this UI thread. Returns
/// the executor plus the joined [`TextContext`].
#[allow(clippy::too_many_arguments)]
fn spawn_split_executor(
    mut startup: StartupSpans,
    metal_layer: *mut c_void,
    physical: (u32, u32),
    scale: f32,
    font_preinit: std::thread::JoinHandle<TextContext>,
    alpha: SurfaceAlphaRequest,
    translucent_resolved: Arc<AtomicBool>,
    present: Arc<PresentHandoff>,
) -> (FrameExecutor, TextContext) {
    // Font/`TextContext` warmup stays UI-side — layout runs on the UI
    // thread. The GPU work is off-thread now, so this join's ordering vs surface
    // bring-up no longer matters; record it before the recorder moves into the
    // render thread below.
    startup.record(SPAN_FONT_PREINIT_STARTED);
    let text_ctx = font_preinit.join().unwrap_or_else(|_| {
        log::warn!(
            "frust-shell-ios: font pre-init thread panicked; \
             falling back to synchronous TextContext::new"
        );
        TextContext::new()
    });
    startup.record(SPAN_FONT_PREINIT_JOINED);

    let (sender, receiver) = render_channel::<PaintedScene, SendableMetalLayer>();
    let (scene_return_tx, scene_return_rx) = scene_return_channel::<Scene>();
    let size = SurfaceSize {
        width: physical.0,
        height: physical.1,
        scale: scale as f64,
    };

    // Per-shell fatal flag: one clone lives in the render
    // thread (set on a first-install failure), one in the `SplitExecutor` (read
    // by `frust_render_frame`). A plain `Arc<AtomicBool>` — no channel/protocol.
    let fatal = Arc::new(AtomicBool::new(false));
    let fatal_render = Arc::clone(&fatal);

    // Presented-frame counter: one clone drives into the render thread
    // (bumped on each `FrameOutcome::Rendered`), one stays in the `SplitExecutor`
    // for the UI thread to read before paint. Mirrors the `fatal` flag's shape.
    let presented = Arc::new(AtomicU64::new(0));
    let presented_render = Arc::clone(&presented);

    // Present-sync handoff: one clone into the render thread (parks
    // each submitted frame when armed), one in the `SplitExecutor` for the UI
    // thread's `frust_present_frame` to take from. Same `Arc`-shared-slot shape
    // as the two counters above — no channel, no protocol.
    let present_render = Arc::clone(&present);

    // Surface-self-heal signal: one clone into the render
    // thread (set on each render-side `SurfaceLost` recreate attempt), one in the
    // `SplitExecutor` for the UI thread to take once per frame — the same
    // `Arc<AtomicBool>` shape as the `fatal` flag above. Created here rather than
    // in `create_handle` because only these two owners ever touch it.
    let surface_reinstalled = Arc::new(AtomicBool::new(false));
    let surface_reinstalled_render = Arc::clone(&surface_reinstalled);

    // Move `startup` (init_entry + font spans already recorded) into the render
    // thread, which owns the rest of the startup line. The raw `metal_layer`
    // pointer is NOT captured by the closure (it is `!Send`); it crosses the
    // channel wrapped in `SendableMetalLayer` via the `SurfaceCreated` command
    // sent from this UI thread below.
    let join = std::thread::Builder::new()
        .name("frust-render".to_string())
        // Guard the loop so a dev-build panic logs and exits cleanly (dropping the
        // owned `RenderReceiver`, which drains any orphaned `Ack` — the barrier
        // deadlock fix). A no-op under the release `panic = "abort"` profile.
        .spawn(move || {
            run_guarded_thread("frust-render (ios)", move || {
                render_loop(
                    receiver,
                    startup,
                    fatal_render,
                    scene_return_tx,
                    presented_render,
                    alpha,
                    translucent_resolved,
                    present_render,
                    surface_reinstalled_render,
                )
            })
        })
        .expect("frust-shell-ios: failed to spawn render thread");

    // Hand the initial surface to the render thread. The UI thread keeps
    // `metal_layer` alive (Swift owns it, released only after `frust_destroy` joins
    // this thread — see `SendableMetalLayer`).
    sender.send_command(RenderCommand::SurfaceCreated {
        window: SendableMetalLayer::new(metal_layer),
        size,
    });

    (
        FrameExecutor::Split(SplitExecutor::new(
            sender,
            join,
            fatal,
            scene_return_rx,
            presented,
            present,
            surface_reinstalled,
        )),
        text_ctx,
    )
}

/// Recreate a lost surface from the handle's retained `CAMetalLayer` at the given
/// physical size/scale, self-healing the `SurfaceLost` terminal state (iOS keeps
/// the same layer for the app's lifetime, so nothing external re-drives creation).
///
/// On success the handle's layout inputs are refreshed via
/// [`IosAppHandle::set_surface`] (which also resets the failure budget). On
/// failure the phase stays `SurfaceLost` and the failure is recorded: the
/// `should_recreate_surface` gate at both call sites retries on later
/// `render_frame`/`resize` entries only while under
/// [`ffi_support::MAX_RECREATE_ATTEMPTS`](crate::ffi_support::MAX_RECREATE_ATTEMPTS)
/// consecutive failures — a persistently-failing recreate degrades to a single
/// "giving up" log line instead of per-frame blocking GPU retries. The same gate
/// skips recreation entirely while paused (backgrounded GPU work can get the
/// process killed); a loss during backgrounding recovers after `frust_resume`.
/// This is the sole recovery call into `on_surface_created_from_metal_layer`
/// outside [`create_handle`], and keeps the `unsafe` confined to this module.
fn recover_surface(app: &mut IosAppHandle, physical: (u32, u32), scale: f32) {
    // Read the retained pointer (and the latched alpha request) before taking
    // the `&mut` borrow of the renderer below.
    let metal_layer = app.metal_layer();
    let alpha = app.surface_alpha();
    let result = {
        // Inline-only: the render-thread split self-heals render-side (see
        // `render_loop`), so its `inline_renderer_mut()` is `None` and this is
        // never reached in the split (the call sites gate on `!executor_is_split()`).
        let Some((render_cx, renderer)) = app.inline_renderer_mut() else {
            return;
        };
        // SAFETY: `metal_layer` is the Swift-owned `CAMetalLayer*` this handle was
        // created with; Swift guarantees it outlives the handle (it calls
        // `frust_destroy` before releasing the view/layer), so recreating a
        // surface from it — exactly as `create_handle` did at init — is sound.
        pollster::block_on(unsafe {
            renderer.on_surface_created_from_metal_layer(
                render_cx,
                metal_layer,
                physical.0,
                physical.1,
                alpha,
            )
        })
    };
    match result {
        Ok(()) => {
            app.set_surface(physical, scale);
            // Re-emit Create+Update for every live platform-view slot:
            // a defensive resync after any surface
            // disruption, mirroring the split's would-be replay (see the
            // module docs' iOS surface recovery note — the split self-heals
            // render-side with no UI-side signal to drive this from, an
            // accepted v1 gap; the inline path has full state access here).
            app.reset_platform_views_for_surface_recreate();
        }
        Err(err) => {
            app.record_recreate_failure();
            if app.recreate_failures() >= crate::ffi_support::MAX_RECREATE_ATTEMPTS {
                log::error!(
                    "frust-shell-ios: surface recreate failed {} times: {err:#}; \
                     giving up for this SurfaceLost episode (rendering disabled)",
                    app.recreate_failures()
                );
            } else {
                log::error!("frust-shell-ios: surface recreate failed: {err:#}");
            }
        }
    }
}

/// `frust_resize`: resize the live surface (rotation / bounds change). The
/// `CAMetalLayer` survives, so a live surface is a plain in-place resize (routed as
/// a `SurfaceChanged` command in the split). In the inline path a `SurfaceLost`
/// surface is instead recreated from the retained layer at the incoming dimensions
/// (self-recovery — see [`recover_surface`]); the split self-heals render-side, so
/// it always routes a plain resize.
pub fn resize(handle: *mut c_void, width: u32, height: u32, scale: f32) {
    guard("frust_resize", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            let physical = (width.max(1), height.max(1));
            // Inline-only surface recovery (the split self-heals render-side).
            let recover = !app.executor_is_split()
                && app.inline_phase().is_some_and(|phase| {
                    crate::ffi_support::should_recreate_surface(
                        phase,
                        app.paused(),
                        app.recreate_failures(),
                    )
                });
            if recover {
                recover_surface(app, physical, scale);
            } else {
                app.resize(physical, scale);
            }
        }
    });
}

/// `frust_render_frame`: run one `CADisplayLink`-driven frame (no-op unless the
/// surface is ready and the app is not paused). In the inline path, if the surface
/// was lost, first recreate it from the retained layer at the last-known
/// size/scale (self-recovery — see [`recover_surface`]) so a `SurfaceLost` is no
/// longer a permanent black screen; the split self-heals render-side (see
/// [`render_loop`]), so no UI-side recovery runs there.
///
/// `timestamp_ns` is the `CADisplayLink` tick's `timestamp` (`CFTimeInterval`
/// seconds), converted to nanoseconds by the Swift caller
/// (`UInt64(link.timestamp * 1_000_000_000)`) — the shell-owned monotonic
/// frame clock threaded into [`frust_core::FrameTime`].
///
/// Returns a `u8`: [`FRAME_FATAL`](crate::ffi_support::FRAME_FATAL)
/// (`0`) = a fatal render-thread failure (first-surface install could not
/// succeed — see [`render_loop`]/[`IosAppHandle::render_fatal`]), on which Swift's
/// `renderFrame` latches `initFailed` and invalidates its `CADisplayLink`;
/// [`FRAME_ALIVE`](crate::ffi_support::FRAME_ALIVE) (`1`) = keep driving frames.
/// The bridging header's `void` return becomes `uint8_t` in lockstep.
pub fn render_frame(handle: *mut c_void, timestamp_ns: u64) -> u8 {
    // Benign default `FRAME_ALIVE` on a caught panic: a single frame's failure
    // must not stop the CADisplayLink — only a render-thread FATAL does.
    guard(
        "frust_render_frame",
        crate::ffi_support::FRAME_ALIVE,
        || {
            // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
            let Some(app) = (unsafe { handle_mut(handle) }) else {
                return crate::ffi_support::FRAME_ALIVE; // no live handle — nothing faulted
            };
            // Inline-only surface recovery (the split self-heals render-side).
            let recover = !app.executor_is_split()
                && app.inline_phase().is_some_and(|phase| {
                    crate::ffi_support::should_recreate_surface(
                        phase,
                        app.paused(),
                        app.recreate_failures(),
                    )
                });
            if recover {
                let (physical, scale) = (app.physical(), app.scale());
                recover_surface(app, physical, scale);
            }
            // In the split, the first-presented-frame startup span is recorded
            // render-side inside `render_scene` (the render thread is the single perf
            // emitter); in the inline path it is recorded there too, since both paths
            // share `render_scene`. So `frame` returns nothing — the liveness signal
            // comes from the fatal flag the render thread sets.
            app.frame(timestamp_ns);
            crate::ffi_support::frame_liveness_signal(app.render_fatal())
        },
    )
}

/// `frust_dispatch_touch`: deliver one touch contact to the tree.
///
/// `phase` is the fixed code the Swift `FrustView` touch overrides send
/// (`0`=began, `1`=moved, `2`=ended, `3`=cancelled — see
/// [`crate::ffi_support::touch_phase_from_code`]); `x`/`y` are logical points
/// (`touch.location(in:)`), passed straight through (no scale division — see the
/// asymmetry note on [`IosAppHandle::dispatch_touch`]). First-touch only in v1.
pub fn dispatch_touch(handle: *mut c_void, phase: u32, x: f32, y: f32) {
    guard("frust_dispatch_touch", (), || {
        // Cheap; keeps controller-driven updates fresh between CADisplayLink
        // frames rather than waiting for the next `frust_render_frame`.
        pump_reactive();
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            let touch_phase = crate::ffi_support::touch_phase_from_code(phase);
            app.dispatch_touch(touch_phase, x, y);
        }
    });
}

/// `frust_ime_apply`: push a whole editing state from the Swift `UITextInput`
/// mirror into the focused widget (the mobile state-sync path). Routed to the
/// focused widget as an `ImeEvent::ApplyEditingState` via
/// [`AppTree::ime_apply`].
///
/// `text` is the mirror's UTF-8 bytes; `sel_*`/`comp_*` are **UTF-16 code-unit**
/// indices (the platform-native unit the `NSMutableString` mirror counts in),
/// passed opaquely through the [`EditingState`] shell seam — the widget /
/// `frust-text` converts them to Rust byte offsets at its own boundary.
/// `-1` denotes "none" for the composing region.
///
/// **Return-key contract:** the Swift side maps the `.done` Return key to an
/// `insertText("\n")`, so a lone `"\n"` insertion arriving here is the submit
/// gesture; the `TextInput` widget treats a single-line newline insert
/// as its `on_submit` trigger.
pub fn ime_apply(
    handle: *mut c_void,
    text: *const c_char,
    sel_base: i32,
    sel_ext: i32,
    comp_base: i32,
    comp_ext: i32,
) {
    guard("frust_ime_apply", (), || {
        // Cheap; keeps controller-driven updates fresh between frames.
        pump_reactive();
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            // SAFETY: `text` is the Swift mirror's UTF-8 C string, valid and
            // NUL-terminated for the duration of this call (or null → empty).
            let text = unsafe { cstr_to_string(text) };
            let state = EditingState {
                text,
                selection_base: sel_base,
                selection_extent: sel_ext,
                composing_base: comp_base,
                composing_extent: comp_ext,
            };
            let _ = app.ime_apply(state);
        }
    });
}

/// `frust_ime_state_json`: the focused field's IME surface as a heap-allocated,
/// caller-freed JSON C string (the Swift bridge parses it to drive
/// `becomeFirstResponder`, seed its mirror, and reconcile after each edit).
///
/// Shape matches the Android bridge — see [`crate::ffi_support::ime_state_json`].
/// Returns a fresh `CString` the caller **must** release via
/// [`string_free`]/`frust_string_free`; a null return (no live handle, or a
/// text containing an interior NUL) is the "no editing state" sentinel the Swift
/// side treats as inactive.
pub fn ime_state_json(handle: *mut c_void) -> *mut c_char {
    guard("frust_ime_state_json", std::ptr::null_mut(), || {
        // Cheap; keeps controller-driven updates fresh between frames.
        pump_reactive();
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        let json = match unsafe { handle_mut(handle) } {
            Some(app) => ime_state_to_json(app.ime_state()),
            None => ime_state_to_json(None),
        };
        match CString::new(json) {
            // Hand the caller ownership of the C string; reclaimed in `string_free`.
            Ok(cstr) => cstr.into_raw(),
            // An interior NUL (not expected in editing text) can't cross as a C
            // string; fall back to the "no state" sentinel rather than corrupting.
            Err(_) => std::ptr::null_mut(),
        }
    })
}

/// `frust_string_free`: release a C string previously returned by
/// [`ime_state_json`]/`frust_ime_state_json`. Idempotent on null.
pub fn string_free(s: *mut c_char) {
    guard("frust_string_free", (), || {
        if s.is_null() {
            return;
        }
        // SAFETY: `s` was produced by `CString::into_raw` in `ime_state_json` and
        // is reclaimed exactly once here (the Swift side calls this exactly once
        // per non-null result, via `defer`).
        drop(unsafe { CString::from_raw(s) });
    });
}

/// Decode a Swift-supplied UTF-8 C string into an owned `String` (empty on null).
///
/// # Safety
///
/// When non-null, `ptr` must be a valid, NUL-terminated C string that stays live
/// for the duration of this call. Invalid UTF-8 is replaced lossily rather than
/// rejected (the mirror is always well-formed UTF-16 → UTF-8 in practice).
unsafe fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: guaranteed by this fn's contract — `ptr` is a live, NUL-terminated
    // C string for this call.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Map [`ImeContentType`] onto the stable wire string
/// [`crate::ffi_support::ime_state_json`] embeds as `"contentType"`.
///
/// `ImeContentType` is `#[non_exhaustive]`, and this crate is not the crate
/// that defines it, so (unlike `ImeContentType::is_secret`/
/// `suppresses_suggestions`, defined alongside the enum) a wildcard arm is
/// mandatory here — `rustc` will not let this bridge compile against a future
/// variant it hasn't seen. Rather than let that wildcard silently default to
/// `"normal"` (exactly the downgrade the type's own docs warn about), it
/// fails closed using the same [`ImeContentType::is_secret`]/
/// [`ImeContentType::suppresses_suggestions`] predicates the enum's docs
/// mandate matching on for this reason: an unrecognized variant becomes the
/// *strictest* wire string its own predicates justify, never the loosest.
/// Mirrors `frust_shell_android::jni_glue::content_type_wire` byte-for-byte
/// (same four wire strings, same fail-closed rule).
fn content_type_wire(content_type: ImeContentType) -> &'static str {
    match content_type {
        ImeContentType::Normal => "normal",
        ImeContentType::Password => "password",
        ImeContentType::NoSuggestions => "noSuggestions",
        ImeContentType::Terminal => "terminal",
        other => {
            if other.is_secret() {
                "password"
            } else if other.suppresses_suggestions() {
                "noSuggestions"
            } else {
                "normal"
            }
        }
    }
}

/// Convert the focused widget's published [`ImeState`] (or its absence) into the
/// bridge JSON, mapping the logical-pixel caret rect into the flat
/// caretX/Y/W/H the Swift side expects and [`ImeContentType`] into the
/// `"contentType"` wire string (see [`content_type_wire`]).
fn ime_state_to_json(state: Option<ImeState>) -> String {
    match state {
        Some(s) => {
            let caret = s.caret.map(|r| CaretRect {
                x: r.x0 as f32,
                y: r.y0 as f32,
                w: r.width() as f32,
                h: r.height() as f32,
            });
            crate::ffi_support::ime_state_json(
                s.active,
                &s.editing.text,
                s.editing.selection_base,
                s.editing.selection_extent,
                s.editing.composing_base,
                s.editing.composing_extent,
                caret,
                content_type_wire(s.content_type),
            )
        }
        // No focused field / no published surface: the inactive sentinel.
        None => crate::ffi_support::ime_state_json(
            false,
            "",
            -1,
            -1,
            -1,
            -1,
            None,
            content_type_wire(ImeContentType::Normal),
        ),
    }
}

/// `frust_pause`: app backgrounded — stop submitting frames (see
/// [`IosAppHandle::pause`]).
pub fn pause(handle: *mut c_void) {
    guard("frust_pause", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.pause();
        }
    });
}

/// `frust_resume`: app foregrounded — resume submitting frames (see
/// [`IosAppHandle::resume`]).
pub fn resume(handle: *mut c_void) {
    guard("frust_resume", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.resume();
        }
        // While backgrounded, CADisplayLink is paused so nothing pumps and
        // tokio timers stall (accepted gap — see `ReactiveRuntime::pump_local`
        // docs); drain any queued completions
        // immediately on foreground instead of waiting for the next
        // `frust_render_frame` tick.
        pump_reactive();
    });
}

/// `frust_set_appearance`: flip the app's theme brightness (a dark-mode
/// change reported via `traitCollectionDidChange`), re-publishing it through
/// both delivery paths (mirrors the desktop shell's `apply_theme`). `dark` is
/// `0`/`1` — no existing bool-ish C-ABI precedent in this crate to match, so a
/// plain `u8` (see `Runner-Bridging-Header.h`). The continuous `CADisplayLink`
/// loop repaints the next tick with no extra wake needed. A missing handle is
/// a no-op.
pub fn set_appearance(handle: *mut c_void, dark: u8) {
    guard("frust_set_appearance", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.set_appearance(dark != 0);
        }
    });
}

/// `frust_set_reduce_motion`: apply the platform's reduced-motion
/// accessibility preference to the active theme's `MotionScheme`, re-publishing
/// it through both delivery paths — the reduced-motion twin of
/// [`set_appearance`].
///
/// `reduce` is Swift's `UIAccessibility.isReduceMotionEnabled` read, as `0`/`1`
/// (the same no-`<stdbool.h>` convention `frust_set_appearance`'s `dark`
/// follows). Reduced motion is NOT a `UITraitCollection` trait, so Swift
/// sources it from `UIAccessibility.reduceMotionStatusDidChangeNotification`
/// rather than `traitCollectionDidChange`. The continuous `CADisplayLink` loop
/// repaints the next tick with no extra wake needed. A missing handle is a
/// no-op.
pub fn set_reduce_motion(handle: *mut c_void, reduce: u8) {
    guard("frust_set_reduce_motion", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.set_reduce_motion(reduce != 0);
        }
    });
}

/// `frust_set_insets`: deliver the platform's window insets. The eight `f32`s
/// are two per-edge
/// sets in the order [`logical_insets`](frust_shell_common::logical_insets)
/// expects — `view_padding` (`vp_*`: Swift assembles this from the view's
/// `safeAreaInsets`) then `view_insets` (`vi_*`: the keyboard frame), each
/// `left`/`top`/`right`/`bottom`.
///
/// Values are **logical points** (UIKit's coordinate space — no scale division,
/// the same asymmetry `frust_dispatch_touch` follows), converted to the
/// framework's logical `WindowInsets` by [`IosAppHandle::set_insets`] and pushed
/// onto the render root (no-op-guarded, marks `LAYOUT | PAINT` on a real change).
/// A missing handle is a no-op.
#[allow(clippy::too_many_arguments)]
pub fn set_insets(
    handle: *mut c_void,
    vp_l: f32,
    vp_t: f32,
    vp_r: f32,
    vp_b: f32,
    vi_l: f32,
    vi_t: f32,
    vi_r: f32,
    vi_b: f32,
) {
    guard("frust_set_insets", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.set_insets([
                vp_l as f64,
                vp_t as f64,
                vp_r as f64,
                vp_b as f64,
                vi_l as f64,
                vi_t as f64,
                vi_r as f64,
                vi_b as f64,
            ]);
        }
    });
}

/// `frust_on_deep_link`: deliver a platform deep link (cold-start, from
/// `SceneDelegate.scene(_:willConnectTo:options:)`'s
/// `connectionOptions.urlContexts`, or running, from
/// `SceneDelegate.scene(_:openURLContexts:)` — see
/// `platform/ios/FrustEmbedding/Sources/FrustEmbedding/FrustSceneDelegate.swift`/`FrustViewController.swift`
/// queue-until-handle-ready contract) into the process-wide
/// deep-link source ([`frust_reactive::push_deep_link`]).
///
/// `url` is the Swift `URL.absoluteString`'s UTF-8 C string; malformed/null
/// input decodes lossily (empty on null) via [`cstr_to_string`], the same
/// policy [`ime_apply`]'s text conversion already uses. A missing handle is
/// a no-op: Swift's own queue-until-handle-ready contract means this should
/// not normally be reachable with a null handle, but the native side stays
/// defensive (mirrors every other export here).
pub fn on_deep_link(handle: *mut c_void, url: *const c_char) {
    guard("frust_on_deep_link", (), || {
        // Cheap; keeps controller-driven updates fresh between frames.
        pump_reactive();
        // SAFETY: `url` is a valid, NUL-terminated UTF-8 C string for this
        // call (or null → empty), the same contract `ime_apply`'s `text`
        // parameter uses.
        let url = unsafe { cstr_to_string(url) };
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if unsafe { handle_mut(handle) }.is_some() {
            push_deep_link(url);
        }
    });
}

/// `frust_system_ui_state`: peek the process-wide system-UI override slot
/// (`frust_shell_common::system_ui`) for `FrustViewController`'s
/// per-`CADisplayLink`-tick poll, returning [`encoded_state`]'s packed
/// `(generation, mode)` `u64` verbatim — the packing scheme and its tests
/// live in `frust-shell-common` (see its module docs for the exact bit
/// layout).
///
/// Takes `handle` and ignores it: the slot is process-global (a shell-wide
/// override, not a per-app-instance one — mirrors [`crate::app::IosAppHandle`]
/// carrying no such state of its own), but every other export in this crate's
/// surface takes the opaque handle as its first argument, so this follows
/// that shape for calling-convention uniformity rather than being the one
/// no-argument export — a caller-side no-op branch on a live handle isn't
/// needed either way since the read never touches it.
///
/// # iOS semantic mapping
///
/// `FrustViewController` decodes the returned `u64` and, on a
/// generation change, stores the decoded flags and calls
/// `setNeedsStatusBarAppearanceUpdate()` / `setNeedsUpdateOfHomeIndicatorAutoHidden()`
/// so UIKit re-queries `prefersStatusBarHidden`/`prefersHomeIndicatorAutoHidden`
/// next layout pass:
///
/// | [`frust_shell_common::SystemUiMode`] | status bar | home indicator |
/// |---|---|---|
/// | `Immersive` / `ImmersiveSticky` / `LeanBack` | hidden | auto-hidden |
/// | `Manual { top, bottom }` | hidden iff `!top` | auto-hidden iff `!bottom` |
/// | `EdgeToEdge` | visible | not auto-hidden |
///
/// iOS has no sticky/non-sticky or lean-back distinction and no *force*-hide
/// of the home indicator — every hiding mode above folds to the same
/// status-bar-hidden + home-indicator-auto-hide behavior, and the system
/// (not the app) decides when a swipe re-reveals it (see
/// `frust_shell_common::system_ui`'s module docs, "Platform behavior
/// differences").
pub fn system_ui_state(handle: *mut c_void) -> u64 {
    guard("frust_system_ui_state", 0, || {
        let _ = handle;
        frust_shell_common::encoded_state()
    })
}

/// `frust_set_surface_mode`: declare the process-wide translucent-surface
/// latch — see
/// `frust_shell_common::surface_mode`'s module docs for the one-way,
/// pre-surface-creation latch contract. `translucent` is `0`/`1` (no
/// `<stdbool.h>` precedent in this crate's ABI — mirrors
/// [`set_appearance`]'s `dark: u8`).
///
/// Callable **only** from the generated `FrustViewController`'s
/// `translucentSurface`-gated branch — the same branch that
/// already set `CAMetalLayer.isOpaque = false` and arranged the
/// native-sibling subview order — and always **before** `frust_init`, ahead
/// of constructing the native handle. Calling this without that layer
/// configuration already in place is a host-template bug, not a supported
/// opt-in; this is why the underlying
/// `declare_host_translucent_surface` is not re-exported past
/// `frust-shell-common`. Takes no handle argument: the slot is
/// process-global (mirrors [`system_ui_state`]'s shape), and there is
/// nothing to guard against a null handle here since the read never touches
/// one. A call after the surface already exists has no effect on that
/// surface (the latch is read once, at surface-creation time, inside
/// [`create_handle`]).
///
/// Declaring only sets the *request*: a layer advertising no translucent alpha
/// mode still comes up opaque, and the paint contract follows the RESOLVED
/// outcome ([`SurfaceRenderer::surface_resolved_translucent`]), not this latch.
pub fn set_surface_mode(translucent: u8) {
    guard("frust_set_surface_mode", (), || {
        if translucent != 0 {
            frust_shell_common::declare_host_translucent_surface();
        }
    });
}

/// The process-global present-sync latch, written by
/// [`set_present_sync`] and read once per handle in [`create_handle`].
///
/// Deliberately **iOS-local** rather than a `frust-shell-common` module beside
/// `surface_mode`: the two platforms need opposite corrections for the same
/// defect — Android delays the *view* to meet the surface (the frame-id gate),
/// iOS delays the *surface* to meet the view — so a shared "platform-view sync"
/// knob would be structurally wrong. Nothing outside this shell can observe or
/// set it.
static PRESENT_SYNC: AtomicBool = AtomicBool::new(false);

/// `frust_set_present_sync`: declare that this host presents the frust surface
/// inside the `CATransaction` that commits hosted platform-view geometry,
/// **with the render-thread split left on**.
///
/// `enabled` is `0`/`1` (the same no-`<stdbool.h>` convention as
/// [`set_surface_mode`]/[`set_appearance`]). Takes no handle: the slot is
/// process-global and must be latched **before** `frust_init`, since it is read
/// once when the handle's executor is built.
///
/// Callable **only** from `FrustViewController`'s
/// `synchronizesPresentWithPlatformViews`-gated branch — the same branch that
/// sets `CAMetalLayer.presentsWithTransaction = true`. The two must move
/// together, exactly like the `translucentSurface` seam above: arming this
/// without the layer flag defers each present by a tick for no benefit (a
/// plain, eager present issued late), and setting the layer flag without arming
/// this stops presentation dead under the split (the drawable is handed to a
/// thread that commits no transaction — measured, §3.2). Neither half is
/// reachable from app Rust; this is a host build-time choice.
///
/// The UI thread must then call `frust_present_frame` once per display-link
/// tick, after `FrustViewHost.poll` — see [`present_frame`].
pub fn set_present_sync(enabled: u8) {
    guard("frust_set_present_sync", (), || {
        PRESENT_SYNC.store(enabled != 0, Ordering::Release);
    });
}

/// Read the present-sync latch (see [`PRESENT_SYNC`]) at handle-construction
/// time.
fn present_sync_enabled() -> bool {
    PRESENT_SYNC.load(Ordering::Acquire)
}

/// `frust_present_frame`: present the frame the render thread parked for the UI
/// thread — the UI-thread half of present-sync (see
/// [`set_present_sync`]).
///
/// Swift calls this once per `CADisplayLink` tick, **after**
/// `FrustViewHost.poll` has applied this frame's platform-view geometry, so the
/// `[drawable present]` and the sibling views' geometry land in one
/// `CATransaction` committed by this thread. Cheap and safe to call
/// unconditionally: with present-sync unarmed (the default), on the inline
/// render path, or on a gate-skipped tick, nothing is ever parked and this is a
/// mutex check plus a return. A null/dead handle is a benign no-op.
pub fn present_frame(handle: *mut c_void) {
    guard("frust_present_frame", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.present_pending_frame();
        }
    });
}

/// `frust_platform_view_commands_json`: the platform-view command backlog
/// as a heap-allocated, caller-freed JSON C string — the iOS counterpart to
/// `frust-shell-android`'s
/// `nativePlatformViewCommands`, byte-identical schema (see
/// [`crate::ffi_support::platform_view_commands_json`]'s doc comment).
///
/// `ack_generation` is the generation the Swift side last finished
/// applying (round-tripped from a prior call's decoded `"generation"`
/// field, `0` on the very first call) — compacts the differ's backlog on
/// the way in. Returns null on the no-change fast path
/// (`generation == ack_generation`) or when there is no live handle;
/// otherwise a fresh `CString` the caller **must** release via
/// [`string_free`]/`frust_string_free` (identical ownership contract to
/// [`ime_state_json`]).
pub fn platform_view_commands_json(handle: *mut c_void, ack_generation: u64) -> *mut c_char {
    guard(
        "frust_platform_view_commands_json",
        std::ptr::null_mut(),
        || {
            // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
            let Some(app) = (unsafe { handle_mut(handle) }) else {
                return std::ptr::null_mut();
            };
            match app.platform_view_commands_json(ack_generation) {
                Some(json) => match CString::new(json) {
                    // Hand the caller ownership; reclaimed in `string_free`.
                    Ok(cstr) => cstr.into_raw(),
                    // An interior NUL can't cross as a C string — benign null
                    // fallback, mirroring `ime_state_json`.
                    Err(_) => std::ptr::null_mut(),
                },
                None => std::ptr::null_mut(),
            }
        },
    )
}

/// `frust_destroy`: reclaim and drop the boxed handle (which drops the surface;
/// the Swift-owned layer is released separately, afterwards). Idempotent from
/// Swift's side because it nulls its handle right after calling this.
pub fn destroy(handle: *mut c_void) {
    guard("frust_destroy", (), || {
        if crate::ffi_support::handle_is_null(handle) {
            return;
        }
        // SAFETY: `handle` was produced by `Box::into_raw` in `create_handle` and
        // is reclaimed exactly once here; the Swift side nulls its copy afterwards
        // so it is never passed back in.
        drop(unsafe { Box::from_raw(handle as *mut IosAppHandle) });
    });
}

/// Compile-only smoke of [`crate::ios_app!`]'s 2-arg (`Default`-state) arm:
/// exercises macro expansion on the iOS target
/// (`cargo check --target aarch64-apple-ios-sim --tests`). Never invoked — its
/// symbols would clash with a real app's, so it lives behind `cfg(test)` where
/// no `cdylib`/`staticlib` links it.
#[cfg(test)]
mod macro_expansion {
    #[derive(Default)]
    struct TestState {
        n: u32,
    }

    fn test_logic(state: &mut TestState) -> impl frust_core::view::View<TestState> + use<> {
        state.n += 1;
        frust_widgets::text(format!("{}", state.n))
    }

    crate::ios_app!(TestState, test_logic);
}

/// Compile-only smoke of [`crate::ios_app!`]'s 3-arg state-factory arm, with a
/// state type that deliberately has **no** `Default` impl — the only way it can
/// build is through the supplied `$state_init` closure. Lives in its own
/// module (distinct from [`macro_expansion`]'s 2-arg
/// invocation) so the two expansions' same-named `extern "C"` items don't
/// collide as module-scoped Rust items; the underlying `#[no_mangle]` symbol
/// clash this would cause at *link* time never arises because this module is
/// exercised only by `cargo check --tests`, which never links.
#[cfg(test)]
mod macro_expansion_state_factory {
    struct NonDefaultState {
        n: u32,
    }

    fn init_state() -> NonDefaultState {
        NonDefaultState { n: 0 }
    }

    fn test_logic(
        state: &mut NonDefaultState,
    ) -> impl frust_core::view::View<NonDefaultState> + use<> {
        state.n += 1;
        frust_widgets::text(format!("{}", state.n))
    }

    crate::ios_app!(NonDefaultState, init_state, test_logic);
}

/// `content_type_wire`/`ime_state_to_json` coverage. Compiled (and would run)
/// only under `#[cfg(target_os = "ios")]` — this crate's C-ABI/`frust-core`
/// dependency is iOS-target-gated (see the crate's `Cargo.toml`), so unlike
/// [`crate::ffi_support`]'s tests this module never executes on the Linux
/// host `cargo test --workspace` runs on; it is verified only by
/// `cargo check --all-targets --target aarch64-apple-ios-sim -p
/// frust-shell-ios` (compiles, does not run) until an iOS test runner exists.
#[cfg(test)]
mod ime_content_type_wire {
    use frust_core::event::{EditingState, ImeContentType, ImeState};

    use super::{content_type_wire, ime_state_to_json};

    #[test]
    fn every_variant_maps_to_its_stable_wire_string() {
        assert_eq!(content_type_wire(ImeContentType::Normal), "normal");
        assert_eq!(content_type_wire(ImeContentType::Password), "password");
        assert_eq!(
            content_type_wire(ImeContentType::NoSuggestions),
            "noSuggestions"
        );
        assert_eq!(content_type_wire(ImeContentType::Terminal), "terminal");
    }

    #[test]
    fn absent_state_encodes_as_normal() {
        // No focused field: the inactive sentinel must not default to a
        // secret classification (that would be over-restrictive, not a leak,
        // but it's still the wrong default — "normal" is what "no field" is).
        assert!(ime_state_to_json(None).contains(r#""contentType":"normal""#));
    }

    #[test]
    fn published_password_state_carries_it_through_to_json() {
        let state = ImeState {
            active: true,
            editing: EditingState {
                text: "hunter2".to_string(),
                selection_base: 7,
                selection_extent: 7,
                composing_base: -1,
                composing_extent: -1,
            },
            caret: None,
            content_type: ImeContentType::Password,
        };
        let json = ime_state_to_json(Some(state));
        assert!(json.contains(r#""contentType":"password""#));
        // The leak this finding closes is the suggestion strip, not the
        // published text — the core deliberately still carries the real
        // text for the platform mirror (see `ImeState` docs); confirm this
        // bridge doesn't (re)introduce redaction that would desync it.
        assert!(json.contains(r#""text":"hunter2""#));
    }

    #[test]
    fn published_no_suggestions_state_carries_it_through_to_json() {
        let state = ImeState {
            active: true,
            editing: EditingState {
                text: "AB12-CD34".to_string(),
                selection_base: 9,
                selection_extent: 9,
                composing_base: -1,
                composing_extent: -1,
            },
            caret: None,
            content_type: ImeContentType::NoSuggestions,
        };
        let json = ime_state_to_json(Some(state));
        assert!(json.contains(r#""contentType":"noSuggestions""#));
    }

    #[test]
    fn published_terminal_state_carries_it_through_to_json() {
        let state = ImeState {
            active: true,
            editing: EditingState {
                text: "ls -la".to_string(),
                selection_base: 6,
                selection_extent: 6,
                composing_base: -1,
                composing_extent: -1,
            },
            caret: None,
            content_type: ImeContentType::Terminal,
        };
        let json = ime_state_to_json(Some(state));
        assert!(json.contains(r#""contentType":"terminal""#));
    }
}
