//! The FFI boundary: the non-generic runtime the [`crate::android_app!`]-stamped
//! `extern "system" fn`s delegate to, and where this crate's `unsafe` is
//! confined.
//!
//! The crate's `unsafe` surface is this module plus the
//! `#[unsafe(no_mangle)]` attributes the [`crate::android_app!`] macro emits on
//! its generated exports (edition-2024 spells `no_mangle` as an unsafe
//! attribute). Every entry point here is wrapped in
//! [`guard`](forgekit_shell_common::guard) so a panic is caught and turned into a
//! benign default instead of unwinding across the JNI boundary (undefined
//! behaviour). The `unsafe` *code* in this module is confined to three things,
//! each with a safety comment: `ANativeWindow_fromSurface` (raw handle →
//! `NativeWindow`), reconstituting the opaque `jlong` handle
//! (`Box::from_raw`/`&mut *`), and `Box::into_raw`/`from_raw` for the handle's
//! lifetime.

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::{Mutex, Once, OnceLock};
use std::thread::JoinHandle;

use accesskit_android::jni as ak_jni;
use anyhow::{Context, Result};
use jni::EnvUnowned;
use jni::errors::LogErrorAndDefault;
use jni::objects::{JObject, JString};
use jni::sys::{JNI_VERSION_1_6, jboolean, jfloat, jint, jlong, jstring};
use ndk::native_window::NativeWindow;

use forgekit_core::event::{EditingState, ImeState};
use forgekit_reactive::{ReactiveRuntime, push_deep_link};
use forgekit_shell_common::perf::{self, StartupSpans};
use forgekit_shell_common::{AppTree, guard};

use crate::app::AndroidAppHandle;
use crate::ffi_support::{
    ImeJsonState, build_ime_state_json, load_pipeline_cache, normalize_ime_indices,
    pipeline_cache_differs, pipeline_cache_path, write_pipeline_cache_atomic,
};

/// Startup-span name (task 13): the persisted pipeline-cache blob has been read
/// from disk (or found absent) and handed to the renderer, recorded just before
/// surface creation. Bracketed by task 08's cold-start recorder so the
/// warm-start win shows up in the `forgekit-perf startup` line between
/// `init_entry` and `renderer_ready`. A crate-local literal rather than a
/// `perf::SPAN_*` const because it is Android-pipeline-cache-specific.
const SPAN_CACHE_LOADED: &str = "cache_loaded";

/// Startup-span name (task 19): `create_handle` is about to join the background
/// GPU pre-init thread [`JNI_OnLoad`] spawned at native-library load. Paired
/// with [`SPAN_PREINIT_JOINED`] so the `forgekit-perf startup` line shows how
/// long `nativeInit` blocked on the pre-init — i.e. the wgpu adapter/device
/// bring-up NOT already overlapped by `nativeInit`'s own window-acquire +
/// cache-load work above it. A near-zero delta means full overlap (the device
/// was ready before the join); a large one means the pre-init was still running.
const SPAN_PREINIT_STARTED: &str = "preinit_started";

/// Startup-span name (task 19): the background GPU pre-init join returned — the
/// pre-built device was adopted, or `create_handle` fell back to synchronous
/// init. See [`SPAN_PREINIT_STARTED`].
const SPAN_PREINIT_JOINED: &str = "preinit_joined";

/// Task 19: the GPU pre-init join handle spawned by [`JNI_OnLoad`]. The
/// background thread builds a [`forgekit_render::RenderContext`] and creates its
/// logical device (wgpu instance + adapter + device, no surface — see
/// [`forgekit_render::RenderContext::ensure_device_headless`]) off the JVM main
/// thread, starting at native-library load, so [`create_handle`]'s
/// timeout-free `.join()` mostly adopts finished work instead of doing it
/// serially after `surfaceCreated`. Holds the thread's result:
/// `Some(RenderContext)` on success, `None` if device init failed on the thread.
/// [`take_preinit_context`] `take`s the handle exactly once; a second (absent)
/// take, a panicked thread, or a `None` result all fall back to a fresh
/// synchronous context.
type PreInitResult = Option<forgekit_render::RenderContext>;
static GPU_PREINIT: OnceLock<Mutex<Option<JoinHandle<PreInitResult>>>> = OnceLock::new();

/// `JNI_OnLoad`: the JVM calls this once when the native library is loaded, well
/// before the first `nativeInit` (task 19, spec §14 phase 7.E). Its only job is
/// to kick off the background GPU pre-init so wgpu adapter/device creation
/// overlaps the JVM's own Activity/Surface bring-up.
///
/// Defined here in the shell crate (not in the [`crate::android_app!`] macro)
/// deliberately: `JNI_OnLoad` is a single, process-wide symbol — a per-app
/// macro-emitted copy would collide. The macro-generated `nativeInit` references
/// [`native_init`] in this module, so this object is already pulled into the
/// generated `cdylib` link, carrying this export with it.
///
/// Returns [`JNI_VERSION_1_6`] unconditionally: the JVM refuses to load a library
/// whose `JNI_OnLoad` reports an unsupported version, so even a panic inside the
/// (guarded) spawn must not change the returned value. The spawn does nothing
/// else blocking.
#[unsafe(no_mangle)]
pub extern "system" fn JNI_OnLoad(_vm: *mut c_void, _reserved: *mut c_void) -> jint {
    init_logger_once();
    guard("JNI_OnLoad", (), spawn_gpu_preinit);
    JNI_VERSION_1_6
}

/// Spawn the single background GPU pre-init thread (task 19), best-effort and
/// single-shot: it builds a [`forgekit_render::RenderContext`] and creates its
/// device with no surface, off the JVM main thread, so [`create_handle`] can join
/// finished work. Idempotent — a second call (e.g. the library re-loaded in the
/// same process) never spawns a second thread. A failed device init on the
/// thread is logged and yields `None`; [`create_handle`] then falls back to a
/// synchronous build.
fn spawn_gpu_preinit() {
    let slot = GPU_PREINIT.get_or_init(|| Mutex::new(None));
    let Ok(mut slot_guard) = slot.lock() else {
        return; // a prior panic poisoned the lock; skip pre-init, nativeInit falls back
    };
    if slot_guard.is_some() {
        return; // already spawned this process
    }
    *slot_guard = Some(std::thread::spawn(|| {
        let mut render_cx = forgekit_render::RenderContext::new();
        match pollster::block_on(render_cx.ensure_device_headless()) {
            Ok(()) => Some(render_cx),
            Err(err) => {
                log::warn!(
                    "forgekit-shell-android: background GPU pre-init failed ({err:#}); \
                     nativeInit will fall back to synchronous GPU init"
                );
                None
            }
        }
    }));
}

/// Join the [`JNI_OnLoad`] GPU pre-init thread (task 19) and return the
/// [`forgekit_render::RenderContext`] [`create_handle`] should use: the pre-built
/// one (instance + adapter + device already created off-thread) when the
/// background init succeeded, or a fresh synchronous `RenderContext` on any
/// best-effort fallback case — pre-init absent (`JNI_OnLoad` never ran, or the
/// handle was already taken by a prior `nativeInit`), the thread panicked, or its
/// device init failed.
///
/// The `.join()` is timeout-free and never slower than the pre-task-19 status
/// quo: the same adapter/device work ran serially inside `nativeInit` before, so
/// at worst this blocks for the remainder of work already in flight. The
/// adopt-vs-fallback decision itself is the host-tested
/// [`crate::ffi_support::resolve_preinit`].
fn take_preinit_context() -> forgekit_render::RenderContext {
    let joined: Option<PreInitResult> = GPU_PREINIT
        .get()
        .and_then(|slot| slot.lock().ok().and_then(|mut g| g.take()))
        .and_then(|handle| match handle.join() {
            Ok(result) => Some(result),
            Err(_) => {
                log::warn!(
                    "forgekit-shell-android: GPU pre-init thread panicked; \
                     falling back to synchronous GPU init"
                );
                None
            }
        });
    crate::ffi_support::resolve_preinit(joined, forgekit_render::RenderContext::new)
}

/// Pump the process-wide [`ReactiveRuntime`]'s UI-thread local task queue, if
/// the runtime has been initialized (it may not be, e.g. a JNI call arriving
/// before the first `nativeInit`). Cheap when the queue is empty; called at the
/// top of every entry point that may observe controller-driven state (touch,
/// the IME trio) so their reads see the latest reconciled state, and — inside
/// [`crate::app::AndroidAppHandle::frame`] — before that function's
/// `SurfacePhase::SurfaceReady` early-return, so local tasks keep draining
/// through surface churn (a torn-down/not-yet-ready surface) instead of
/// stalling.
pub(crate) fn pump_reactive_runtime() {
    if let Some(rt) = ReactiveRuntime::get() {
        rt.pump_local();
    }
}

/// Initialise `android_logger` exactly once per process, so `log::*` from any
/// crate in the graph reaches logcat under the `forgekit` tag.
fn init_logger_once() {
    static LOGGER: Once = Once::new();
    LOGGER.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("forgekit"),
        );
    });
}

/// Acquire an [`ndk::native_window::NativeWindow`] from a Kotlin `Surface`.
///
/// # Safety
///
/// `env` and `surface` must be the valid `JNIEnv`/`Surface` the JVM passed into
/// the current native call (they are live for its duration). Returns `None` when
/// the platform yields a null window.
///
/// `ANativeWindow_fromSurface` returns a **new, already-acquired**
/// `ANativeWindow` reference; [`NativeWindow::from_ptr`] takes ownership of
/// exactly that reference (its `Drop` calls `ANativeWindow_release` once), so the
/// acquire/release is balanced with no extra clone.
unsafe fn native_window_from_surface(env: &EnvUnowned, surface: &JObject) -> Option<NativeWindow> {
    // SAFETY: `env`/`surface` are the JVM-owned, live handles for this call; the
    // pointer casts bridge the two `jni-sys` versions (`jni` uses 0.3, `ndk-sys`
    // 0.4) — both are `#[repr(C)]` opaque handles with identical layout.
    let raw =
        unsafe { ndk_sys::ANativeWindow_fromSurface(env.as_raw().cast(), surface.as_raw().cast()) };
    // SAFETY: `raw` is the freshly-acquired reference from `fromSurface`;
    // `from_ptr` adopts ownership of it (see this fn's safety doc).
    NonNull::new(raw).map(|ptr| unsafe { NativeWindow::from_ptr(ptr) })
}

/// The physical (pixel) size of a window, clamped to at least 1×1.
fn window_physical_size(window: &NativeWindow) -> (u32, u32) {
    (window.width().max(1) as u32, window.height().max(1) as u32)
}

/// Borrow the boxed [`AndroidAppHandle`] behind a `jlong`, or `None` if the JVM
/// side has no live native handle (`0`).
///
/// # Safety
///
/// When non-zero, `handle` must be a pointer previously returned by
/// [`native_init`] and not yet passed to [`native_on_destroy`]. The returned
/// reference must not outlive the current native call (it aliases the boxed
/// handle the JVM still owns).
unsafe fn handle_mut<'a>(handle: jlong) -> Option<&'a mut AndroidAppHandle> {
    if !crate::ffi_support::handle_is_live(handle) {
        return None;
    }
    // SAFETY: guaranteed by this fn's safety contract — `handle` is a live
    // `Box<AndroidAppHandle>` pointer, uniquely reconstituted as a &mut for the
    // duration of one single-threaded (UI-thread) native call.
    Some(unsafe { &mut *(handle as *mut AndroidAppHandle) })
}

/// `nativeInit`: build the native handle for the first surface and return it to
/// the JVM as an opaque `jlong` (spec §10.1).
///
/// `make_app` is supplied by the macro and erases the app's `State`/`app_logic`;
/// on any failure (null window, GPU init error, panic) returns `0`, matching
/// Kotlin's "no native side yet" sentinel.
///
/// `cache_dir` is the app's `context.cacheDir.absolutePath` (task 13), read into
/// a Rust `String` up front — before the surface handle is touched below — so
/// the `env` borrow is released early (mirrors [`native_ime_apply`]'s string
/// read). It is used to persist the wgpu pipeline cache across launches; an
/// unreadable/empty value simply disables persistence (a cold compile every
/// launch), never fails init.
pub fn native_init(
    mut env: EnvUnowned,
    surface: JObject,
    scale: jfloat,
    cache_dir: JString,
    make_app: impl FnOnce() -> Box<dyn AppTree>,
) -> jlong {
    init_logger_once();
    guard("nativeInit", 0, || {
        let cache_dir = env
            .with_env(|env| cache_dir.try_to_string(env))
            .resolve::<LogErrorAndDefault>();
        let cache_dir = (!cache_dir.is_empty()).then_some(cache_dir);
        match create_handle(&env, &surface, scale, cache_dir, make_app) {
            Ok(handle) => handle,
            Err(err) => {
                log::error!("forgekit-shell-android: nativeInit failed: {err:#}");
                0
            }
        }
    })
}

/// Fallible body of [`native_init`], separated so the happy path reads top-down.
///
/// `cache_dir` (when `Some`) is the app cache directory the pipeline-cache blob
/// is loaded from before GPU init and saved back to after renderer creation
/// (task 13, spec §14 phase 7.B) — best-effort and Vulkan-only.
fn create_handle(
    env: &EnvUnowned,
    surface: &JObject,
    scale: jfloat,
    cache_dir: Option<String>,
    make_app: impl FnOnce() -> Box<dyn AppTree>,
) -> Result<jlong> {
    // Cold-start span recorder (task 08, spec §14 phase 7.A). `begin()` marks
    // the epoch; every span below is a delta from here, closed out by
    // `AndroidAppHandle::frame`'s first successful render (see that method).
    // A no-op recorder (allocates nothing further) when `perf::enabled()` is
    // false.
    let mut startup_spans = StartupSpans::begin();
    startup_spans.record(perf::SPAN_INIT_ENTRY);

    // SAFETY: `env`/`surface` are the live JVM handles for this call.
    let window = unsafe { native_window_from_surface(env, surface) }
        .context("forgekit-shell-android: ANativeWindow_fromSurface returned null")?;
    let physical = window_physical_size(&window);

    let mut renderer = forgekit_render::SurfaceRenderer::new();

    // Pipeline-cache persistence (task 13, spec §14 phase 7.B): restore the blob
    // a prior launch persisted so vello's Vulkan shader pipelines are reused
    // rather than recompiled on this warm start. Must be set BEFORE the surface
    // install below (that's where vello's renderer — and its pipeline cache — is
    // created). Best-effort and Vulkan-only: `forgekit-render` validates the
    // blob against the live adapter at install time and silently starts from an
    // empty cache on any mismatch, or on adapters without `PIPELINE_CACHE`
    // (Metal/desktop — there is no iOS counterpart). `loaded_cache` (framed
    // bytes read verbatim from disk) is retained for the differs-check after
    // renderer creation.
    //
    // Done BEFORE the pre-init join below on purpose: this disk read overlaps the
    // background GPU thread's adapter/device work, shrinking the join wait (task
    // 19). The renderer build — and its pipeline cache — deliberately stays here
    // at `nativeInit`, NOT on the pre-init thread: the pipeline cache needs
    // `cache_dir`, which only arrives with `nativeInit` (task 13 option a) and is
    // unknown at `JNI_OnLoad` time, so pre-init covers instance/adapter/device
    // only and the renderer keeps its cache seeding. See the task-19 completion
    // note for the pre-init/renderer split decision.
    let cache_path = cache_dir.as_deref().map(pipeline_cache_path);
    let loaded_cache = cache_path.as_deref().and_then(load_pipeline_cache);
    renderer.set_initial_pipeline_cache_data(loaded_cache.clone());
    startup_spans.record(SPAN_CACHE_LOADED);

    // Adopt the `RenderContext` the `JNI_OnLoad` background thread has been
    // building (wgpu instance + adapter + device) since native-library load
    // (task 19, spec §14 phase 7.E), or fall back to a fresh synchronous one on
    // any best-effort miss (pre-init absent/panicked/failed). Bracketed by
    // `preinit_started`/`preinit_joined` so the `forgekit-perf startup` line
    // shows how much of the GPU bring-up overlapped the window-acquire +
    // cache-load work above — a near-zero window means full overlap. The
    // `.join()` is never slower than the status quo (that adapter/device work ran
    // serially here before). The surface install below reuses this device via
    // `RenderContext::ensure_device`'s `is_surface_supported` check (Android's
    // singular Vulkan adapter — see `ensure_device_headless`'s doc).
    startup_spans.record(SPAN_PREINIT_STARTED);
    let mut render_cx = take_preinit_context();
    startup_spans.record(SPAN_PREINIT_JOINED);

    let window_ptr = window.ptr().as_ptr().cast::<c_void>();

    // SAFETY: `window_ptr` comes from the just-acquired `window`, which is moved
    // into the returned `AndroidAppHandle` and (by that struct's field-drop
    // order) outlives the surface and all its textures (spec §8.1).
    pollster::block_on(unsafe {
        renderer.on_surface_created_from_android_window(
            &mut render_cx,
            window_ptr,
            physical.0,
            physical.1,
        )
    })
    .context("forgekit-shell-android: failed to create Android render surface")?;

    // With task 19's pre-init, the adapter + device were (usually) already built
    // on the `JNI_OnLoad` background thread and adopted at the join above, so the
    // `on_surface_created_from_android_window` call just now only did the vello
    // renderer + surface setup (`ensure_device` reused the pre-built device via
    // its `is_surface_supported` check). The adapter/device spans therefore mark
    // "confirmed ready post-join", and the real GPU-overlap signal is the
    // `preinit_started`→`preinit_joined` window plus `renderer_ready`. On a
    // pre-init miss (fallback path) adapter/device were instead created here,
    // synchronously, exactly as before task 19 — the spans stay meaningful either
    // way. Still recorded at one checkpoint (no intermediate hook into the vello
    // setup) so the perf-line schema task 08 established stays stable across
    // shells.
    startup_spans.record(perf::SPAN_ADAPTER_READY);
    startup_spans.record(perf::SPAN_DEVICE_READY);
    startup_spans.record(perf::SPAN_RENDERER_READY);

    // Persist the pipeline cache the driver populated while creating vello's
    // renderer above, if it changed from what we loaded (task 13). Spawns a
    // detached background thread for the write so the first frame never blocks on
    // disk I/O; a no-op on Metal/desktop (`pipeline_cache_data()` returns `None`
    // without `PIPELINE_CACHE`) and when nothing changed. All failures logged and
    // ignored — persistence is best-effort.
    if let Some(path) = cache_path {
        persist_pipeline_cache_if_changed(path, loaded_cache.as_deref(), &renderer);
    }

    // Process-once (the runtime's own `OnceLock` provides that property; a
    // repeat call — e.g. an activity recreated in the same process — just
    // re-marks the calling thread as the UI thread and swaps in a fresh no-op
    // waker). Must run BEFORE `make_app()`: a `State`'s own construction (a
    // future `Component::init()`) may create signals/controllers that need the
    // runtime already installed.
    //
    // `init` claims the *calling* thread as the UI thread for `spawn_local`.
    // Every JNI call (including a `nativeInit` after activity recreation)
    // arrives on the same JVM main thread that invoked `nativeInit` the first
    // time, so re-init here always runs on the thread already claimed — safe by
    // construction, not by accident.
    let rt = ReactiveRuntime::init(std::sync::Arc::new(|| {
        // No-op: Choreographer already posts every frame regardless, so there
        // is nothing for `spawn_local`'s wake-up to nudge on Android.
    }));

    // Construct the app AND its handle under the root `Owner`. `make_app` runs
    // `Component::init` (via `new_boxed_app_with`'s state factory), and
    // `AndroidAppHandle::new` runs the initial `rebuild()` — both must see an
    // ambient `Owner` or `provide_context`/`on_cleanup` silently no-op. A root
    // component has no enclosing component to supply one, so it registers
    // against the root owner (process lifetime, never disposed), mirroring the
    // desktop shell's per-frame `with_owner` wrap and the facade `run()` init.
    let handle = rt.with_owner(|| {
        let app = make_app();
        AndroidAppHandle::new(
            render_cx,
            renderer,
            window,
            physical,
            scale,
            app,
            startup_spans,
        )
    });

    // SAFETY: hand a uniquely-owned boxed handle to the JVM as `jlong`; it is
    // reclaimed exactly once in `native_on_destroy`.
    Ok(Box::into_raw(Box::new(handle)) as jlong)
}

/// Persist the current pipeline-cache blob to `path` on a background thread if it
/// differs from `loaded` (task 13).
///
/// Reads [`SurfaceRenderer::pipeline_cache_data`](forgekit_render::SurfaceRenderer::pipeline_cache_data)
/// (framed + adapter-fingerprinted; `None` without Vulkan `PIPELINE_CACHE`), and
/// spawns a **detached** writer thread so the first frame never waits on disk
/// I/O — the write outlives this function and the returned handle by design.
/// Every failure is logged and ignored: a failed persist only costs the next
/// launch its cold-compile time.
fn persist_pipeline_cache_if_changed(
    path: PathBuf,
    loaded: Option<&[u8]>,
    renderer: &forgekit_render::SurfaceRenderer,
) {
    let Some(data) = renderer.pipeline_cache_data() else {
        return; // no cache to persist (Metal/desktop, or nothing compiled)
    };
    if !pipeline_cache_differs(loaded, &data) {
        return; // unchanged since load — skip the rewrite
    }
    std::thread::spawn(move || match write_pipeline_cache_atomic(&path, &data) {
        Ok(()) => log::debug!(
            "forgekit-shell-android: persisted pipeline cache ({} bytes) to {}",
            data.len(),
            path.display()
        ),
        Err(err) => log::warn!(
            "forgekit-shell-android: failed to persist pipeline cache to {}: {err}",
            path.display()
        ),
    });
}

/// `nativeOnSurfaceChanged`: recreate the surface against a new window, or resize
/// it in place when the underlying window is unchanged.
pub fn native_on_surface_changed(
    env: EnvUnowned,
    handle: jlong,
    surface: JObject,
    width: jint,
    height: jint,
) {
    guard("nativeOnSurfaceChanged", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        let Some(app) = (unsafe { handle_mut(handle) }) else {
            return;
        };
        let physical = (width.max(1) as u32, height.max(1) as u32);

        // SAFETY: `env`/`surface` are the live JVM handles for this call.
        let Some(new_window) = (unsafe { native_window_from_surface(&env, &surface) }) else {
            log::error!("forgekit-shell-android: surfaceChanged with a null window");
            return;
        };

        // Same underlying window ⇒ a plain resize; a different (or first) window
        // ⇒ (re)create the surface against it.
        if app.window().map(NativeWindow::ptr) == Some(new_window.ptr()) {
            // Drop the extra reference `fromSurface` just acquired; the stored
            // window keeps the surface alive.
            drop(new_window);
            app.resize(physical);
            return;
        }

        let window_ptr = new_window.ptr().as_ptr().cast::<c_void>();
        let result = {
            let (render_cx, renderer) = app.renderer_mut();
            // SAFETY: `window_ptr` is from `new_window`, which is moved into the
            // handle via `set_window` below (and thus outlives the surface); the
            // previous surface is torn down inside this call before the previous
            // window is released.
            pollster::block_on(unsafe {
                renderer.on_surface_created_from_android_window(
                    render_cx, window_ptr, physical.0, physical.1,
                )
            })
        };
        match result {
            Ok(()) => app.set_window(new_window, physical),
            Err(err) => {
                log::error!("forgekit-shell-android: surfaceChanged recreate failed: {err:#}")
            }
        }
    });
}

/// `nativeOnSurfaceDestroyed`: drop the surface and release its window (spec §8.1).
pub fn native_on_surface_destroyed(handle: jlong) {
    guard("nativeOnSurfaceDestroyed", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.destroy_surface();
        }
    });
}

/// `nativeOnFrame`: run one Choreographer-driven frame (no-op unless the surface
/// is ready). `frame_time_nanos` is Kotlin's `Choreographer.FrameCallback`
/// timestamp (`System.nanoTime()`-based, monotonic); a negative value (should
/// never happen, but the JNI boundary is untrusted input) clamps to `0` rather
/// than wrapping through the `as u64` cast.
pub fn native_on_frame(handle: jlong, frame_time_nanos: jlong) {
    guard("nativeOnFrame", (), || {
        let frame_time_nanos = crate::ffi_support::frame_time_nanos_from_jlong(frame_time_nanos);
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.frame(frame_time_nanos);
        }
    });
}

/// `nativeOnTouch`: deliver one touch contact to the tree (spec §9).
///
/// `action` is the normalised phase code the Kotlin side sends
/// (`0`=down, `1`=move, `2`=up, `3`=cancel — see
/// [`crate::ffi_support::touch_phase_from_action`]); `x`/`y` are physical,
/// view-local pixels (`MotionEvent.x`/`.y`), converted to logical space inside
/// [`AndroidAppHandle::dispatch_touch`]. Single-pointer in v1: Kotlin forwards
/// only the primary pointer.
pub fn native_on_touch(handle: jlong, action: jint, x: jfloat, y: jfloat) {
    guard("nativeOnTouch", (), || {
        pump_reactive_runtime();
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            let phase = crate::ffi_support::touch_phase_from_action(action);
            app.dispatch_touch(phase, x, y);
        }
    });
}

/// `nativeOnResume`: activity resumed. Bookkeeping only in v0 — the Choreographer
/// loop is started/stopped in Kotlin (spec Phase 2 §10.1).
pub fn native_on_resume(handle: jlong) {
    guard("nativeOnResume", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if unsafe { handle_mut(handle) }.is_some() {
            log::debug!("forgekit-shell-android: onResume");
        }
    });
}

/// `nativeOnPause`: activity paused. Bookkeeping only in v0 (see [`native_on_resume`]).
pub fn native_on_pause(handle: jlong) {
    guard("nativeOnPause", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if unsafe { handle_mut(handle) }.is_some() {
            log::debug!("forgekit-shell-android: onPause");
        }
    });
}

/// `nativeOnDestroy`: reclaim and drop the boxed handle. Idempotent from the
/// JVM's side because Kotlin zeroes its `handle` right after calling this.
pub fn native_on_destroy(handle: jlong) {
    guard("nativeOnDestroy", (), || {
        if !crate::ffi_support::handle_is_live(handle) {
            return;
        }
        // SAFETY: `handle` was produced by `Box::into_raw` in `create_handle` and
        // is reclaimed exactly once here; the JVM zeroes its copy afterwards so it
        // is never passed back in.
        drop(unsafe { Box::from_raw(handle as *mut AndroidAppHandle) });
    });
}

/// `nativeImeApply`: push a whole platform editing state into the focused widget
/// (the mobile IME state-sync path, spec §14 Phase 4).
///
/// `text` is the Kotlin mirror `Editable`'s content, read into a Rust `String`
/// (Java MUTF-8/UTF-16 → UTF-8) through the JNI string API. The four indices are
/// **UTF-16 code units** (Java-native) and cross the `AppTree`/shell seam
/// unchanged — [`EditingState`]'s indices are UTF-16 at this seam and the focused
/// widget converts them to Rust byte offsets. [`normalize_ime_indices`] only
/// canonicalises the `-1` "none" sentinel. A missing handle is a no-op.
pub fn native_ime_apply(
    mut env: EnvUnowned,
    handle: jlong,
    text: JString,
    sel_base: jint,
    sel_ext: jint,
    comp_base: jint,
    comp_ext: jint,
) {
    guard("nativeImeApply", (), || {
        pump_reactive_runtime();
        // Read the Java string first (releases the `env` borrow before we touch
        // the handle); an unreadable/`null` string falls back to empty.
        let text = env
            .with_env(|env| text.try_to_string(env))
            .resolve::<LogErrorAndDefault>();

        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        let Some(app) = (unsafe { handle_mut(handle) }) else {
            return;
        };
        let (selection_base, selection_extent, composing_base, composing_extent) =
            normalize_ime_indices(sel_base, sel_ext, comp_base, comp_ext);
        let state = EditingState {
            text,
            selection_base,
            selection_extent,
            composing_base,
            composing_extent,
        };
        app.ime_apply(state);
    });
}

/// `nativeImeState`: return the focused widget's published IME surface as JSON
/// for the Kotlin side to reconcile against its mirror and drive the `IMM`.
///
/// The JSON (built host-testably by [`build_ime_state_json`]) carries `active`,
/// the editing state (UTF-16 indices), and the logical-px caret rect. Returns a
/// null `jstring` only when there is no live native handle; a live handle with no
/// focused editable yields an inactive-state JSON (so Kotlin can hide the
/// keyboard) rather than null.
pub fn native_ime_state(mut env: EnvUnowned, handle: jlong) -> jstring {
    guard("nativeImeState", std::ptr::null_mut(), || {
        pump_reactive_runtime();
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        let Some(app) = (unsafe { handle_mut(handle) }) else {
            return std::ptr::null_mut();
        };
        let json = build_ime_state_json(&ime_state_to_json(app.ime_state()));
        env.with_env(|env| Ok::<JObject, jni::errors::Error>(JString::new(env, &json)?.into()))
            .resolve::<LogErrorAndDefault>()
            .into_raw()
    })
}

/// `nativeImeAction`: forward a soft-keyboard editor action
/// (`performEditorAction`, e.g. `IME_ACTION_DONE`) as an `Enter` key press down
/// the focus path.
///
/// `action` is retained for ABI stability and future differentiation; v1
/// configures only `IME_ACTION_DONE`, so any action maps to `Enter` (a benign,
/// single-line "submit"). A missing handle is a no-op.
pub fn native_ime_action(handle: jlong, action: jint) {
    guard("nativeImeAction", (), || {
        pump_reactive_runtime();
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.ime_action(action);
        }
    });
}

/// `nativeSetAppearance`: flip the app's theme brightness (config/uiMode
/// change), re-publishing it through both delivery paths (mirrors task 05's
/// desktop `apply_theme`). `dark` is a JNI `jboolean` (this `jni` crate's
/// `jni-sys` 0.4 backing type is a real `bool`, not the `u8` older bindings
/// use); the continuous Choreographer loop repaints the next tick with no
/// extra wake needed. A missing handle is a no-op.
pub fn native_set_appearance(handle: jlong, dark: jboolean) {
    guard("nativeSetAppearance", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.set_appearance(dark);
        }
    });
}

/// `nativeOnDeepLink`: deliver a platform deep link (cold-start, forwarded
/// from `MainActivity.onCreate`'s `intent?.data`, or running, from
/// `MainActivity.onNewIntent` — see `templates/app/android.tmpl`'s
/// `MainActivity`/`ForgeKitSurfaceView` queue-until-handle-ready contract,
/// task 07) into the process-wide deep-link source
/// ([`forgekit_reactive::push_deep_link`]).
///
/// `url` is the Kotlin `Intent.data` `Uri`'s `toString()`, read into a Rust
/// `String` through the JNI string API (Java MUTF-8/UTF-16 → UTF-8); an
/// unreadable/malformed/`null` value falls back to empty rather than
/// failing the call — the same lossy-on-malformed-input policy
/// [`native_ime_apply`]'s text conversion already uses. A missing handle is
/// a no-op: Kotlin's own queue-until-handle-ready contract means this
/// should not normally be reachable with a null handle, but the native side
/// stays defensive (mirrors every other `native_*` entry point here).
pub fn native_on_deep_link(mut env: EnvUnowned, handle: jlong, url: JString) {
    guard("nativeOnDeepLink", (), || {
        pump_reactive_runtime();
        let url = env
            .with_env(|env| url.try_to_string(env))
            .resolve::<LogErrorAndDefault>();

        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if unsafe { handle_mut(handle) }.is_some() {
            push_deep_link(url);
        }
    });
}

/// `nativeInitAccessibility`: attach the accesskit Android adapter to the host
/// `ForgeKitSurfaceView` (phase 6d D3, spec §9). Called by Kotlin's
/// `surfaceCreated` right after a successful `nativeInit`, passing the view
/// (`this`) as the accessibility host.
///
/// This is a **separate, best-effort** export rather than being folded into
/// `nativeInit` deliberately: it isolates any JNI hiccup constructing the
/// adapter — most notably a missing `dev.accesskit.android.Delegate` class —
/// inside its own [`guard`], so accessibility failing to initialise degrades to
/// "no a11y" instead of failing app startup (`nativeInit`'s own contract stays
/// untouched). A missing handle, or a second call (idempotent — see
/// [`AndroidAppHandle::attach_accessibility`]), is a no-op.
///
/// # jni version bridge
///
/// accesskit_android 0.7.5 is built against `jni` 0.21 while this shell uses
/// `jni` 0.22 — two distinct crate versions. Their `JNIEnv`/`jobject` handles
/// are both `#[repr(C)]` opaque pointers with identical layout, so the raw
/// pointers cross between them by a plain cast (the same technique
/// [`native_window_from_surface`] uses to bridge `jni`/`ndk-sys`).
pub fn native_init_accessibility(env: EnvUnowned, handle: jlong, view: JObject) {
    guard("nativeInitAccessibility", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        let Some(app) = (unsafe { handle_mut(handle) }) else {
            return;
        };

        // Bridge this call's `jni` 0.22 boundary handles into the `jni` 0.21
        // types `accesskit_android::InjectingAdapter::new` expects.
        //
        // SAFETY: `env` is the JVM-owned env pointer for this native call — a
        // valid attachment of the current (UI) thread; the cast only re-labels
        // its jni-sys version. `from_raw` does a null check and wraps the
        // pointer without taking ownership.
        let mut ak_env = match unsafe { ak_jni::JNIEnv::from_raw(env.as_raw().cast()) } {
            Ok(ak_env) => ak_env,
            Err(err) => {
                log::error!(
                    "forgekit-shell-android: nativeInitAccessibility: invalid JNIEnv: {err}"
                );
                return;
            }
        };
        // SAFETY: `view` is the live, non-null host-`View` jobject Kotlin passed
        // as `this`; `JObject::from_raw` just wraps the (cast) raw handle as a
        // borrowed local ref (its `Drop` is a no-op), so no double-free.
        let ak_view = unsafe { ak_jni::objects::JObject::from_raw(view.as_raw().cast()) };
        app.attach_accessibility(&mut ak_env, &ak_view);
    });
}

/// Map the focused widget's published [`ImeState`] (or its absence) onto the
/// plain, host-testable [`ImeJsonState`] the JSON builder consumes.
///
/// `None` (nothing focused / no surface published) becomes the inactive default.
/// The caret [`kurbo::Rect`] flattens to `(x, y, width, height)` logical pixels,
/// dropped when any component is non-finite (it would not serialise as JSON).
fn ime_state_to_json(state: Option<ImeState>) -> ImeJsonState {
    let Some(state) = state else {
        return ImeJsonState::default();
    };
    let caret = state.caret.and_then(|r| {
        let (x, y, w, h) = (
            r.x0 as f32,
            r.y0 as f32,
            r.width() as f32,
            r.height() as f32,
        );
        (x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()).then_some((x, y, w, h))
    });
    ImeJsonState {
        active: state.active,
        text: state.editing.text,
        sel_base: state.editing.selection_base,
        sel_ext: state.editing.selection_extent,
        comp_base: state.editing.composing_base,
        comp_ext: state.editing.composing_extent,
        caret,
    }
}

/// Compile-only smoke of [`crate::android_app!`]'s 2-arg (`Default`-state) arm:
/// exercises macro expansion on the Android target (`cargo check --target
/// aarch64-linux-android`). Never invoked — its symbols would clash with a real
/// app's (and with [`macro_expansion_factory`]'s below, both stamping out the
/// same fixed JNI names), so it lives behind `cfg(test)`, which is never linked
/// into a `cdylib`.
#[cfg(test)]
mod macro_expansion {
    #[derive(Default)]
    struct TestState {
        n: u32,
    }

    fn test_logic(state: &mut TestState) -> impl forgekit_core::view::View<TestState> + use<> {
        state.n += 1;
        forgekit_widgets::text(format!("{}", state.n))
    }

    crate::android_app!(TestState, test_logic);
}

/// Compile-only smoke of [`crate::android_app!`]'s 3-arg (state-factory) arm,
/// covering acceptance criterion 2: a `State` with **no** `Default` impl —
/// the only way to construct it is through the factory closure passed as the
/// macro's second argument. See [`macro_expansion`] for why this lives in its
/// own `cfg(test)`-only module (both expand to the same fixed JNI symbol names,
/// which is fine for `cargo check` — the two are never actually linked
/// together).
#[cfg(test)]
mod macro_expansion_factory {
    struct NonDefaultState {
        n: u32,
    }

    fn make_state() -> NonDefaultState {
        NonDefaultState { n: 1 }
    }

    fn test_logic(
        state: &mut NonDefaultState,
    ) -> impl forgekit_core::view::View<NonDefaultState> + use<> {
        state.n += 1;
        forgekit_widgets::text(format!("{}", state.n))
    }

    crate::android_app!(NonDefaultState, make_state, test_logic);
}
