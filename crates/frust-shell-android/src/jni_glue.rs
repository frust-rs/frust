//! The FFI boundary: the non-generic runtime the [`crate::android_app!`]-stamped
//! `extern "system" fn`s delegate to, and where this crate's `unsafe` is
//! confined.
//!
//! The crate's `unsafe` surface is this module plus the
//! `#[unsafe(no_mangle)]` attributes the [`crate::android_app!`] macro emits on
//! its generated exports (edition-2024 spells `no_mangle` as an unsafe
//! attribute). Every entry point here is wrapped in
//! [`guard`](frust_shell_common::guard) so a panic is caught and turned into a
//! benign default instead of unwinding across the JNI boundary (undefined
//! behaviour). The `unsafe` *code* in this module is confined to four things,
//! each with a safety comment: `ANativeWindow_fromSurface` (raw handle →
//! `NativeWindow`), reconstituting the opaque `jlong` handle
//! (`Box::from_raw`/`&mut *`), `Box::into_raw`/`from_raw` for the handle's
//! lifetime, `frust_plugin::android::initialize` (handing the captured
//! `JavaVM` + application-`Context` pointers to the plugin platform bridge,
//! which forwards them to `ndk-context` and arms its pre-init flag — see
//! [`native_init_platform`]), the `on_surface_created_from_android_window`
//! calls that build a `wgpu::Surface` from a raw `ANativeWindow*` (the inline
//! path's [`build_inline_executor`]/[`native_on_surface_changed`], and the
//! render-thread split's [`install_surface`]), the `unsafe impl Send` for
//! [`SendableWindowPtr`] — the raw window pointer that crosses the UI→render
//! channel in the split (plan phase 11.B) — and the render-thread priority
//! self-boost `libc::setpriority(PRIO_PROCESS, gettid(), THREAD_PRIORITY_DISPLAY)`
//! at the top of [`render_loop`] (phase-11 fix F6: a bare libc syscall scoping
//! itself to the calling thread, best-effort and non-fatal).
//!
//! # Render-thread split (plan phase 11.B)
//!
//! When [`render_thread_enabled`](frust_shell_common::render_thread_enabled) is
//! set (the default; `FRUST_NO_RENDER_THREAD` opts out), [`create_handle`] spawns
//! the dedicated [`render_loop`] thread that owns the `RenderContext`/
//! `SurfaceRenderer` + surface and runs encode→acquire→submit; the UI thread
//! (Choreographer callbacks) keeps rebuild→layout→paint and hands finished scenes
//! across the channel. Surface lifecycle is owned commands with an ack barrier on
//! `SurfaceDestroyed`: the UI thread keeps the [`NativeWindow`] and releases it
//! only after the render thread acks dropping the surface built from its pointer,
//! so the `ANativeWindow` is never touched after release.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::thread::JoinHandle;

use accesskit_android::jni as ak_jni;
use anyhow::{Context, Result};
use jni::EnvUnowned;
use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JObject, JString};
use jni::refs::Global;
use jni::sys::{JNI_VERSION_1_6, jboolean, jfloat, jint, jlong, jstring};
use ndk::native_window::NativeWindow;

use frust_core::event::{EditingState, ImeState};
use frust_reactive::{ReactiveRuntime, handles_back, push_back_press, push_deep_link};
use frust_scene::Scene;
use frust_shell_common::perf::{self, FrameStats, StartupSpans};
use frust_shell_common::{
    AppTree, RenderCommand, RenderPhase, RenderReceiver, SceneReturnSender, SurfaceMode,
    SurfaceModeWatcher, SurfaceSize, ViewCommand, declare_host_translucent_surface, guard,
    next_render_phase, render_channel, render_thread_enabled, run_guarded_thread,
    scene_return_channel,
};
use frust_text::TextContext;

use crate::app::{AndroidAppHandle, FrameExecutor, InlineExecutor, PaintedScene, SplitExecutor};
use crate::ffi_support::{
    ImeJsonState, PlatformViewCommandJson, build_ime_state_json, build_platform_view_commands_json,
    load_pipeline_cache, normalize_ime_indices, pipeline_cache_differs, pipeline_cache_path,
    platform_view_commands_up_to_date, publish_resolved_translucency, write_pipeline_cache_atomic,
};

/// Startup-span name (task 13): the persisted pipeline-cache blob has been read
/// from disk (or found absent) and handed to the renderer, recorded just before
/// surface creation. Bracketed by task 08's cold-start recorder so the
/// warm-start win shows up in the `frust-perf startup` line between
/// `init_entry` and `renderer_ready`. A crate-local literal rather than a
/// `perf::SPAN_*` const because it is Android-pipeline-cache-specific.
const SPAN_CACHE_LOADED: &str = "cache_loaded";

/// Startup-span name (task 19): `create_handle` is about to join the background
/// GPU pre-init thread [`JNI_OnLoad`] spawned at native-library load. Paired
/// with [`SPAN_PREINIT_JOINED`] so the `frust-perf startup` line shows how
/// long `nativeInit` blocked on the pre-init — i.e. the wgpu adapter/device
/// bring-up NOT already overlapped by `nativeInit`'s own window-acquire +
/// cache-load work above it. A near-zero delta means full overlap (the device
/// was ready before the join); a large one means the pre-init was still running.
const SPAN_PREINIT_STARTED: &str = "preinit_started";

/// Startup-span name (task 19): the background GPU pre-init join returned — the
/// pre-built device was adopted, or `create_handle` fell back to synchronous
/// init. See [`SPAN_PREINIT_STARTED`].
const SPAN_PREINIT_JOINED: &str = "preinit_joined";

/// Startup-span name (phase 10.D): `create_handle` is about to join the
/// background font-preload thread [`JNI_OnLoad`] spawned at native-library
/// load (see [`spawn_font_preinit`]). Mirrors [`SPAN_PREINIT_STARTED`]'s shape
/// for the GPU pre-init thread — a near-zero delta to
/// [`SPAN_FONT_PREINIT_JOINED`] means the font DB/`TextContext` build fully
/// overlapped the window-acquire/GPU-init work above it.
const SPAN_FONT_PREINIT_STARTED: &str = "font_preinit_started";

/// Startup-span name (phase 10.D): the background font-preload join returned —
/// the pre-built [`TextContext`] was adopted, or `create_handle` fell back to
/// a synchronous [`TextContext::new`]. See [`SPAN_FONT_PREINIT_STARTED`].
const SPAN_FONT_PREINIT_JOINED: &str = "font_preinit_joined";

/// Task 19: the GPU pre-init join handle spawned by [`JNI_OnLoad`]. The
/// background thread builds a [`frust_render::RenderContext`] and creates its
/// logical device (wgpu instance + adapter + device, no surface — see
/// [`frust_render::RenderContext::ensure_device_headless`]) off the JVM main
/// thread, starting at native-library load, so [`create_handle`]'s
/// timeout-free `.join()` mostly adopts finished work instead of doing it
/// serially after `surfaceCreated`. Holds the thread's result:
/// `Some(RenderContext)` on success, `None` if device init failed on the thread.
/// [`take_preinit_context`] `take`s the handle exactly once; a second (absent)
/// take, a panicked thread, or a `None` result all fall back to a fresh
/// synchronous context.
type PreInitResult = Option<frust_render::RenderContext>;
static GPU_PREINIT: OnceLock<Mutex<Option<JoinHandle<PreInitResult>>>> = OnceLock::new();

/// Phase 10.D: the GPU pre-init thread's font-warmup counterpart, spawned by
/// [`JNI_OnLoad`] alongside it. Builds a [`TextContext`] (parley's
/// `FontContext`/`LayoutContext` — the font-DB load `Widget::layout`'s first
/// text pass would otherwise pay for) off the JVM main thread, overlapping the
/// same window the GPU adapter/device build overlaps. `TextContext` is
/// `!Sync` but plain owned data (no raw pointers), so it is `Send`-safe to hand
/// across this one thread boundary via the join below; nothing shares it
/// mutably across threads afterward — [`take_preinit_text_context`] takes it
/// exactly once and hands it to the UI thread that then owns it exclusively for
/// the handle's lifetime (a prewarm-then-move design, not shared mutable
/// state). Holds `Some(TextContext)` unconditionally — construction has no
/// fallible step — until [`take_preinit_text_context`] takes it.
static FONT_PREINIT: OnceLock<Mutex<Option<JoinHandle<TextContext>>>> = OnceLock::new();

/// The process [`JavaVM`](jni::JavaVM) pointer, captured at [`JNI_OnLoad`]
/// (task 01, plugin system). The JVM hands `JNI_OnLoad` the `JavaVM` before any
/// `nativeInit`, but the plugin platform bridge needs it later, at
/// [`native_init_platform`] — so stash the raw pointer here rather than
/// discarding it. `AtomicPtr` because `JNI_OnLoad` (library-load thread) writes
/// it and `nativeInitPlatform` (UI thread) reads it; the `JavaVM` itself is
/// process-lifetime and thread-safe. Null until `JNI_OnLoad` has run.
static JAVA_VM: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// One-shot guard for the plugin platform-handle install ([`native_init_platform`]):
/// [`frust_plugin::android::initialize`] must run exactly once per process (the
/// `ndk_context::initialize_android_context` it wraps panics on a second call),
/// but Kotlin re-invokes `nativeInitPlatform` after activity recreation — so
/// the second and later calls are a no-op. This `Once` also owns the
/// application-`Context` [`Global`]-ref idempotence ([`CONTEXT_GLOBAL`]).
static PLATFORM_INIT: Once = Once::new();

/// Holds the application-`Context` [`Global`] reference for the whole process,
/// so the raw jobject pointer handed to `ndk-context` at [`native_init_platform`]
/// stays valid for the process lifetime (a dropped `Global` would invalidate
/// it). Set exactly once, under [`PLATFORM_INIT`]; never taken out or dropped.
static CONTEXT_GLOBAL: OnceLock<Global<JObject<'static>>> = OnceLock::new();

/// Process-wide "a surface has begun being created" flag (platform-views
/// task 05): stored `true` at the top of [`create_handle`], read by
/// [`native_set_surface_mode`] to warn on a too-late latch call. The
/// translucent-surface opt-in ([`frust_shell_common::surface_mode`]'s module
/// docs' Latch contract) is pre-init-only — the surface format is fixed at
/// creation — so a call after this flag flips has no effect on the current
/// surface; best-effort (this flag flips once per process, at the first
/// `nativeInit`, not per-handle).
static ANY_SURFACE_CREATED: AtomicBool = AtomicBool::new(false);

/// Resolve the [`frust_render::SurfaceAlphaRequest`] a surface-creation call
/// should pass, from the process-wide translucent-surface latch
/// ([`SurfaceModeWatcher::current`], platform-views task 03/05):
/// [`SurfaceMode::Translucent`] resolves to
/// [`frust_render::SurfaceAlphaRequest::TranslucentPreferred`] (task 04's
/// capability-probed resolution table then picks the actual
/// `wgpu::CompositeAlphaMode`), [`SurfaceMode::Opaque`] (the default) to
/// today's unchanged [`frust_render::SurfaceAlphaRequest::Opaque`]. Read
/// fresh at each surface-creation call site rather than cached, since the
/// latch itself never reverts once set — a stale cached `false` read before
/// a same-process `nativeSetSurfaceMode(true)` call would otherwise survive
/// past it.
///
/// This produces the REQUEST only. Whether the surface actually came up
/// translucent is a separate, per-install value the UI thread reads from the
/// `translucent_resolved` flag [`install_surface`] publishes (review finding
/// M1) — never re-derive "am I translucent?" from this function.
fn surface_alpha_request() -> frust_render::SurfaceAlphaRequest {
    match SurfaceModeWatcher::current() {
        SurfaceMode::Translucent => frust_render::SurfaceAlphaRequest::TranslucentPreferred,
        SurfaceMode::Opaque => frust_render::SurfaceAlphaRequest::Opaque,
    }
}

/// `JNI_OnLoad`: the JVM calls this once when the native library is loaded, well
/// before the first `nativeInit` (task 19, spec §14 phase 7.E). It captures the
/// [`JavaVM`](jni::JavaVM) pointer for the plugin platform bridge (task 01 —
/// stashed in [`JAVA_VM`] for [`native_init_platform`]) and kicks off the
/// background GPU pre-init so wgpu adapter/device creation overlaps the JVM's
/// own Activity/Surface bring-up, plus the font-preload pre-init (phase 10.D)
/// so the `TextContext`/font-DB build overlaps the same window.
///
/// Defined here in the shell crate (not in the [`crate::android_app!`] macro)
/// deliberately: `JNI_OnLoad` is a single, process-wide symbol — a per-app
/// macro-emitted copy would collide. The macro-generated `nativeInit` references
/// [`native_init`] in this module, so this object is already pulled into the
/// generated `cdylib` link, carrying this export (and the sibling
/// [`native_init_platform`] one) with it.
///
/// Returns [`JNI_VERSION_1_6`] unconditionally: the JVM refuses to load a library
/// whose `JNI_OnLoad` reports an unsupported version, so even a panic inside the
/// (guarded) spawn must not change the returned value. The spawn does nothing
/// else blocking.
#[unsafe(no_mangle)]
pub extern "system" fn JNI_OnLoad(vm: *mut c_void, _reserved: *mut c_void) -> jint {
    init_logger_once();
    // Retain the VM pointer for `nativeInitPlatform`; a plain pointer store,
    // never dereferenced here. `Release` pairs with the `Acquire` load there.
    JAVA_VM.store(vm, Ordering::Release);
    guard("JNI_OnLoad", (), || {
        spawn_gpu_preinit();
        spawn_font_preinit();
    });
    JNI_VERSION_1_6
}

/// `nativeInitPlatform`: install the `(JavaVM, application Context)` pair into
/// `ndk-context`'s process-wide slot so any Rust code — most importantly
/// `frust-plugin`-backed platform plugins — can make JNI calls with zero
/// per-plugin native code (task 01, plugin system Phase 1).
///
/// Kotlin's generated `FrustSurfaceView` calls this with
/// `context.applicationContext` right before `nativeInit`. The **application**
/// context (not the Activity) is stored deliberately: it is stable across
/// activity recreation, so retaining it can't leak an Activity.
///
/// A hand-written, process-wide export (like [`JNI_OnLoad`]) rather than a
/// per-app `android_app!`-generated one — the handles are process state, not
/// per-`AppHandle` state. Routed through [`guard`] like every export so a panic
/// (e.g. a failed `new_global_ref`) can never unwind across the JNI boundary.
///
/// Idempotent via [`PLATFORM_INIT`]: [`frust_plugin::android::initialize`]
/// (wrapping `ndk_context::initialize_android_context`) panics if called twice,
/// and Kotlin re-runs this after activity recreation, so only the first call
/// installs the handles; the rest are no-ops.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeInitPlatform<'local>(
    env: EnvUnowned<'local>,
    _class: JClass<'local>,
    context: JObject<'local>,
) {
    native_init_platform(env, context)
}

/// Body of [`Java_dev_frust_FrustSurfaceView_nativeInitPlatform`], split out so
/// the export stays a thin `extern "system"` shim.
fn native_init_platform(mut env: EnvUnowned, context: JObject) {
    guard("nativeInitPlatform", (), || {
        PLATFORM_INIT.call_once(|| {
            let vm_ptr = JAVA_VM.load(Ordering::Acquire);
            if vm_ptr.is_null() {
                log::error!(
                    "frust-shell-android: nativeInitPlatform ran before JNI_OnLoad captured the \
                     JavaVM; platform plugins will report NotInitialized"
                );
                return;
            }

            // Promote the (application) context local ref to a process-lifetime
            // global ref so the pointer handed to `ndk-context` below stays
            // valid forever; a failed `new_global_ref` yields a null `Global`
            // and skips the install (plugins then see NotInitialized).
            let global = env
                .with_env(|env| env.new_global_ref(&context))
                .resolve::<LogErrorAndDefault>();
            let ctx_ptr = global.as_obj().as_raw();
            if ctx_ptr.is_null() {
                log::error!(
                    "frust-shell-android: nativeInitPlatform could not create a global ref for the \
                     application Context; platform plugins will report NotInitialized"
                );
                return;
            }
            // Hold the global for the process lifetime (never dropped).
            let _ = CONTEXT_GLOBAL.set(global);

            // SAFETY: `vm_ptr` is the live process `JavaVM` captured in
            // `JNI_OnLoad`; `ctx_ptr` is the just-leaked, process-lifetime
            // application-context global ref. `PLATFORM_INIT.call_once` makes
            // this the exactly-once call `frust_plugin::android::initialize`
            // (and the `ndk-context` install it wraps) requires.
            unsafe {
                frust_plugin::android::initialize(vm_ptr, ctx_ptr.cast::<c_void>());
            }
            log::debug!("frust-shell-android: plugin platform handles installed");
        });
    });
}

/// Spawn the single background GPU pre-init thread (task 19), best-effort and
/// single-shot: it builds a [`frust_render::RenderContext`] and creates its
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
        let mut render_cx = frust_render::RenderContext::new();
        match pollster::block_on(render_cx.ensure_device_headless()) {
            Ok(()) => Some(render_cx),
            Err(err) => {
                log::warn!(
                    "frust-shell-android: background GPU pre-init failed ({err:#}); \
                     nativeInit will fall back to synchronous GPU init"
                );
                None
            }
        }
    }));
}

/// Join the [`JNI_OnLoad`] GPU pre-init thread (task 19) and return the
/// [`frust_render::RenderContext`] [`create_handle`] should use: the pre-built
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
fn take_preinit_context() -> frust_render::RenderContext {
    let joined: Option<PreInitResult> = GPU_PREINIT
        .get()
        .and_then(|slot| slot.lock().ok().and_then(|mut g| g.take()))
        .and_then(|handle| match handle.join() {
            Ok(result) => Some(result),
            Err(_) => {
                log::warn!(
                    "frust-shell-android: GPU pre-init thread panicked; \
                     falling back to synchronous GPU init"
                );
                None
            }
        });
    crate::ffi_support::resolve_preinit(joined, frust_render::RenderContext::new)
}

/// Spawn the single background font-preload thread (phase 10.D), best-effort
/// and single-shot, mirroring [`spawn_gpu_preinit`]'s shape exactly: it builds
/// a [`TextContext`] (parley font-DB/`FontContext` + `LayoutContext`
/// construction) off the JVM main thread during the same `JNI_OnLoad` window
/// the GPU pre-init overlaps, so [`create_handle`] can join finished work
/// instead of paying the font-load span on the first `layout` pass.
/// Idempotent — a second call (e.g. the library re-loaded in the same process)
/// never spawns a second thread. `TextContext::new` has no fallible step, so
/// this thread cannot fail — only panic, handled at the join site
/// ([`take_preinit_text_context`]).
fn spawn_font_preinit() {
    let slot = FONT_PREINIT.get_or_init(|| Mutex::new(None));
    let Ok(mut slot_guard) = slot.lock() else {
        return; // a prior panic poisoned the lock; skip pre-init, nativeInit falls back
    };
    if slot_guard.is_some() {
        return; // already spawned this process
    }
    *slot_guard = Some(std::thread::spawn(TextContext::new));
}

/// Join the [`JNI_OnLoad`] font-preload thread (phase 10.D) and return the
/// [`TextContext`] [`create_handle`] should use: the pre-built one (font-DB
/// already loaded off-thread) when the background build finished, or a fresh
/// synchronous [`TextContext::new`] on any best-effort fallback case — pre-init
/// absent (`JNI_OnLoad` never ran, or the handle was already taken by a prior
/// `nativeInit`) or the thread panicked. Kill nothing, defer nothing silently:
/// a fallback here degrades to the cold path with a log line, never a
/// crash/block.
///
/// The `.join()` is never slower than the pre-phase-10.D status quo: the same
/// `TextContext::new()` work ran synchronously inside `AndroidAppHandle::new`
/// before, so at worst this blocks for the remainder of work already in
/// flight.
fn take_preinit_text_context() -> TextContext {
    let joined = FONT_PREINIT
        .get()
        .and_then(|slot| slot.lock().ok().and_then(|mut g| g.take()))
        .and_then(|handle| match handle.join() {
            Ok(text_ctx) => Some(text_ctx),
            Err(_) => {
                log::warn!(
                    "frust-shell-android: font pre-init thread panicked; \
                     falling back to synchronous TextContext::new"
                );
                None
            }
        });
    joined.unwrap_or_else(|| {
        log::debug!(
            "frust-shell-android: no font pre-init result available; \
             building TextContext synchronously"
        );
        TextContext::new()
    })
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
/// crate in the graph reaches logcat under the `frust` tag.
fn init_logger_once() {
    static LOGGER: Once = Once::new();
    LOGGER.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("frust"),
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

// ---------------------------------------------------------------------
// Render-thread split (plan phase 11.B)
// ---------------------------------------------------------------------

/// A raw `ANativeWindow*` made `Send` so it can cross the UI→render-thread
/// scene-handoff channel as the `SurfaceCreated` payload (plan phase 11.B) — the
/// `W` type parameter of the shared
/// [`render_channel`](frust_shell_common::render_channel), which each shell picks
/// (desktop pairs a `DetachedSurface`; Android passes this raw pointer, since
/// `ANativeWindow_fromSurface`/surface creation from a pointer has no
/// main-thread requirement, unlike winit's window handle).
///
/// # Safety
///
/// The wrapped pointer is a valid, acquired `ANativeWindow*` owned by the UI
/// thread's [`NativeWindow`] (stored in [`AndroidAppHandle`]). The UI thread
/// keeps that `NativeWindow` alive until the render thread acknowledges a
/// [`RenderCommand::SurfaceDestroyed`] — dropping the surface built from this
/// pointer *first* (the ack barrier in [`AndroidAppHandle::destroy_surface`]/
/// [`AndroidAppHandle::split_recreate_surface`]) — so the pointer is valid for
/// the whole lifetime of any surface the render thread creates from it. The two
/// threads never touch it concurrently: the UI thread only reads a
/// `NativeWindow` pointer to construct this wrapper; the render thread only reads
/// it back to create the surface ([`install_surface`]). That single-owner,
/// barrier-ordered handoff is what makes the `unsafe impl Send` sound.
pub(crate) struct SendableWindowPtr(*mut c_void);

// SAFETY: see the type's docs — the wrapped `ANativeWindow*` is kept alive by the
// UI thread across the `SurfaceDestroyed` ack barrier and is never used by two
// threads concurrently.
unsafe impl Send for SendableWindowPtr {}

impl SendableWindowPtr {
    /// Wrap a raw `ANativeWindow*`. The caller upholds the type's safety
    /// contract (the pointer's window outlives every surface built from it, on
    /// the render thread); constructing the wrapper itself is a plain field
    /// store.
    pub(crate) fn new(ptr: *mut c_void) -> Self {
        Self(ptr)
    }

    /// The wrapped raw pointer, for [`install_surface`] to build the surface from.
    pub(crate) fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}

/// Install (or reinstall) a `wgpu::Surface` on `renderer` from the raw
/// `ANativeWindow*` `window_ptr`, on the render thread (plan phase 11.B). On the
/// **first** install it also seeds + persists the pipeline cache and records the
/// cache/adapter/device/renderer startup spans, mirroring the pre-split
/// [`create_handle`] flow (which now happens render-side in the split).
///
/// The unsafe `on_surface_created_from_android_window` call is confined here (a
/// sanctioned zone); its `SAFETY` note states the cross-thread ownership
/// contract [`SendableWindowPtr`] documents.
#[allow(clippy::too_many_arguments)]
fn install_surface(
    renderer: &mut frust_render::SurfaceRenderer,
    render_cx: &mut frust_render::RenderContext,
    window_ptr: *mut c_void,
    width: u32,
    height: u32,
    cache_path: Option<&Path>,
    startup_spans: &mut Option<StartupSpans>,
    first_install: bool,
    translucent_resolved: &AtomicBool,
) -> Result<()> {
    // Load + seed the pipeline cache before the surface (and thus vello's
    // renderer + pipeline cache) is created — first install only. The cache
    // spans are recorded here, before surface creation, so their deltas keep the
    // "cache loaded before GPU bring-up" ordering the pre-split line had.
    let loaded_cache = if first_install {
        let lc = cache_path.and_then(load_pipeline_cache);
        renderer.set_initial_pipeline_cache_data(lc.clone());
        if let Some(spans) = startup_spans.as_mut() {
            spans.record(SPAN_CACHE_LOADED);
            if lc.is_some() {
                spans.record(perf::SPAN_PIPELINE_CACHE_RESTORED);
            }
        }
        lc
    } else {
        None
    };

    // SAFETY: `window_ptr` is a valid, acquired `ANativeWindow*` the UI thread's
    // `NativeWindow` (in `AndroidAppHandle`) keeps alive until it receives this
    // surface's `SurfaceDestroyed` ack — so it outlives the surface created here
    // (spec §8.1). See `SendableWindowPtr`'s safety docs for the full contract.
    pollster::block_on(unsafe {
        renderer.on_surface_created_from_android_window(
            render_cx,
            window_ptr,
            width.max(1),
            height.max(1),
            surface_alpha_request(),
        )
    })
    .context("frust-shell-android: failed to create Android render surface")?;

    // Publish the surface's RESOLVED translucency to the UI thread (review
    // finding M1): `surface_alpha_request()` above is only what we ASKED for —
    // `frust-render` resolves it against the platform's advertised alpha modes
    // and can fall back to an opaque swapchain. The UI thread reads this flag
    // every frame (`AndroidAppHandle::sync_translucent_resolved`) before
    // choosing the base color and pushing `set_surface_translucent`, so a
    // fallback degrades to the Mode A contract instead of `DestOut`-punching
    // black rectangles. Written on EVERY (re)install, never only the first.
    publish_resolved_translucency(
        translucent_resolved,
        Some(renderer.surface_resolved_translucent()),
    );

    if first_install {
        if let Some(spans) = startup_spans.as_mut() {
            spans.record(perf::SPAN_ADAPTER_READY);
            spans.record(perf::SPAN_DEVICE_READY);
            spans.record(perf::SPAN_RENDERER_READY);
        }
        if let Some(path) = cache_path {
            persist_pipeline_cache_if_changed(
                path.to_path_buf(),
                loaded_cache.as_deref(),
                renderer,
            );
        }
    }
    Ok(())
}

/// Android render-thread nice value: `android.os.Process.THREAD_PRIORITY_DISPLAY`
/// (phase-11 fix F6).
///
/// `-4` is a **published platform constant** — the nice value Android's own
/// UI/display pipeline threads run at, one band above the default (`0`) and below
/// `THREAD_PRIORITY_URGENT_DISPLAY` (`-8`)
/// (developer.android.com/reference/android/os/Process#THREAD_PRIORITY_DISPLAY,
/// retrieved 2026-07-22). [`render_loop`] self-boosts to it via
/// `setpriority(PRIO_PROCESS, gettid(), ..)` so a busy UI thread can't starve the
/// GPU submit path; a backgrounded cpuset can legitimately refuse it, so a failure
/// is logged, never fatal.
const THREAD_PRIORITY_DISPLAY: libc::c_int = -4;

/// The dedicated render thread's loop (plan phase 11.B): adopt the `JNI_OnLoad`
/// GPU pre-init [`RenderContext`](frust_render::RenderContext) *on this thread*,
/// own the `SurfaceRenderer` + surface wholesale, drain lifecycle commands and
/// the freshest handed-off scene from the channel, and run encode→acquire→submit
/// for each frame — the single perf emitter (folding the UI thread's [`UiSpans`]
/// with its own render spans via [`crate::app::render_scene`]). Owns the whole
/// startup line from `preinit_started` onward (the UI thread recorded
/// `init_entry` + the font-preinit spans before moving the recorder here). Exits
/// cleanly when the [`RenderSender`](frust_shell_common::RenderSender) is dropped.
///
/// `fatal` is the per-shell fatal flag (phase-11 fix F2): this thread stores
/// `true` into it if the **first** surface install fails, so the UI thread's
/// [`native_on_frame`] returns `false` and Kotlin stops the Choreographer loop
/// (a first-install failure — an incapable GPU/driver — is unrecoverable and
/// otherwise leaves a permanent black screen with no platform signal). Later
/// reinstall failures stay log-only.
///
/// `translucent_resolved` is the resolved-translucency seam (review finding
/// M1): this thread creates the surface, so only it can see whether the
/// requested translucent alpha mode was actually granted. It stores the
/// outcome on every (re)install (and clears it on a failed one) for the UI
/// thread — which owns the `RenderRoot` and the per-frame base color — to read
/// each frame. Seeded from the REQUEST by
/// [`spawn_split_executor`], so the common (capable) case is Mode B from frame
/// 1 and only a real resolution can downgrade it.
///
/// [`UiSpans`]: frust_shell_common::perf::UiSpans
pub(crate) fn render_loop(
    receiver: RenderReceiver<PaintedScene, SendableWindowPtr>,
    startup_spans: StartupSpans,
    cache_dir: Option<String>,
    fatal: Arc<AtomicBool>,
    scene_return: SceneReturnSender<Scene>,
    presented: Arc<AtomicU64>,
    translucent_resolved: Arc<AtomicBool>,
) {
    // Render-thread priority self-boost (phase-11 fix F6): raise this dedicated
    // render thread to the display band so a busy UI thread can't starve the GPU
    // submit path. Best-effort — a backgrounded cpuset can refuse it, so a
    // non-zero return is logged, never fatal (this loop never panics).
    //
    // SAFETY: `setpriority`/`gettid` are plain libc syscalls with no
    // memory-safety preconditions; `gettid()` returns this very thread's kernel
    // id and `PRIO_PROCESS` scopes the call to it alone. A sanctioned-unsafe FFI
    // call confined to this module (see the module docs).
    unsafe {
        if libc::setpriority(
            libc::PRIO_PROCESS,
            libc::gettid() as libc::id_t,
            THREAD_PRIORITY_DISPLAY,
        ) != 0
        {
            log::warn!(
                "frust-shell-android: render-thread setpriority(THREAD_PRIORITY_DISPLAY) failed \
                 ({}); continuing at default priority",
                std::io::Error::last_os_error()
            );
        }
    }

    let cache_path = cache_dir.as_deref().map(pipeline_cache_path);
    let mut startup_spans = Some(startup_spans);

    // Adopt the background GPU pre-init context here — the "pre-init handoff lands
    // on the render thread" contract (plan phase 11.B). Bracketed by the
    // preinit_started/joined spans exactly as the pre-split `create_handle` did.
    if let Some(spans) = startup_spans.as_mut() {
        spans.record(SPAN_PREINIT_STARTED);
    }
    let mut render_cx = take_preinit_context();
    if let Some(spans) = startup_spans.as_mut() {
        spans.record(SPAN_PREINIT_JOINED);
    }

    let mut renderer = frust_render::SurfaceRenderer::new();
    let mut frame_stats = FrameStats::new();
    let mut phase = RenderPhase::NoSurface;
    let mut first_install_done = false;
    let mut first_rebuild_recorded = false;

    loop {
        let batch = receiver.wait_next();

        // Lifecycle commands first (FIFO), updating the phase machine.
        for command in batch.commands {
            phase = next_render_phase(phase, command.event());
            match command {
                RenderCommand::SurfaceCreated { window, size } => {
                    let first_install = !first_install_done;
                    match install_surface(
                        &mut renderer,
                        &mut render_cx,
                        window.as_ptr(),
                        size.width,
                        size.height,
                        cache_path.as_deref(),
                        &mut startup_spans,
                        first_install,
                        &translucent_resolved,
                    ) {
                        Ok(()) => first_install_done = true,
                        Err(err) => {
                            // A failed (re)install leaves no surface whose
                            // translucency we can vouch for — clear the flag
                            // rather than leaving the previous surface's value
                            // standing (review M1: never punch a hole you
                            // can't prove is a window).
                            publish_resolved_translucency(&translucent_resolved, None);
                            log::error!(
                                "frust-shell-android: render-thread surface install failed: {err:#}"
                            );
                            // First-install failure is fatal + unrecoverable (the
                            // one observed cause — an incapable GPU/driver — can't
                            // change mid-process). Signal the UI thread so
                            // `nativeOnFrame` returns false and Kotlin stops the
                            // Choreographer loop instead of driving doomed frames
                            // against a permanent black screen (phase-11 fix F2).
                            // Later reinstall failures stay log-only.
                            if first_install {
                                fatal.store(true, Ordering::Release);
                            }
                        }
                    }
                }
                RenderCommand::SurfaceChanged { size } => {
                    renderer.on_surface_changed(&render_cx, size.width, size.height);
                }
                RenderCommand::SurfaceDestroyed { ack } => {
                    // Drop the surface resources FIRST, then acknowledge — the UI
                    // thread blocks on this ack before releasing the
                    // `ANativeWindow`, so the window is never touched after
                    // release (the plan's Android surface-lifecycle-race hazard).
                    renderer.on_surface_destroyed();
                    ack.acknowledge();
                }
                RenderCommand::Pause { ack } => {
                    // Android v1 never sends `Pause` (Kotlin owns start/stop of the
                    // Choreographer loop); honor the barrier defensively so a stray
                    // one can never deadlock the UI thread.
                    ack.acknowledge();
                }
                RenderCommand::Resume => {}
            }
        }

        // Then the freshest scene, only if the phase allows submitting (a scene
        // handed off while paused/destroyed is dropped from presentation, not
        // silently dropped altogether — see the give-back below).
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
                crate::app::render_scene(
                    &mut renderer,
                    &render_cx,
                    &frame.scene.scene,
                    frame.scene.base_color,
                    frame.ui_spans,
                    &mut frame_stats,
                    &mut startup_spans,
                    perf_on,
                    &presented,
                );
            }
            // Give the drained scene back for the UI thread to reclaim (review
            // finding F5) — whether it was actually rendered above or
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
                log::error!("frust-shell-android: nativeInit failed: {err:#}");
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
    // Mark that a surface has begun being created in this process (task 05):
    // read by `native_set_surface_mode` to warn on a too-late latch call. Set
    // unconditionally here, before the fallible steps below, since the latch
    // contract only cares "was init attempted", not whether it succeeded.
    ANY_SURFACE_CREATED.store(true, Ordering::Release);

    // Cold-start span recorder (task 08, spec §14 phase 7.A). `begin()` marks
    // the epoch; every span below is a delta from here, closed out by the first
    // successful render. A no-op recorder (allocates nothing further) when
    // `perf::enabled()` is false.
    let mut startup_spans = StartupSpans::begin();
    startup_spans.record(perf::SPAN_INIT_ENTRY);

    // SAFETY: `env`/`surface` are the live JVM handles for this call.
    let window = unsafe { native_window_from_surface(env, surface) }
        .context("frust-shell-android: ANativeWindow_fromSurface returned null")?;
    let physical = window_physical_size(&window);

    // Build the render-path executor (plan phase 11.B), chosen once by the
    // `FRUST_NO_RENDER_THREAD` kill switch:
    //
    // - Split (default): spawn the dedicated render thread that owns the
    //   `RenderContext`/`SurfaceRenderer` + surface and does all GPU work — the
    //   GPU pre-init handoff lands *on that thread* — so `nativeInit` returns
    //   without blocking the UI thread on adapter/device/pipeline bring-up. The
    //   `startup_spans` recorder is moved into the render thread, which owns the
    //   startup line from `preinit_started` on.
    // - Inline (kill switch engaged): create the surface + renderer on this UI
    //   thread exactly as the pre-split shell did, recording the full startup
    //   line here.
    //
    // Both retain `window` in `AndroidAppHandle` (the UI thread owns the
    // `NativeWindow` and releases it only after the render thread — split — acks
    // dropping the surface built from its pointer).
    // The resolved-translucency seam (review finding M1). SEEDED FROM THE
    // REQUEST: the surface is created asynchronously on the render thread in
    // the default split, and an all-but-certain grant (the shipped Android
    // config resolves `Inherit`) should not cost a Mode-A flash on frame 1 —
    // so the optimistic value stands until the render thread reports a real
    // resolution, which can only ever downgrade it. One clone per surface
    // owner (render thread or inline renderer), one in the handle for the UI
    // thread's per-frame read.
    let translucent_resolved = Arc::new(AtomicBool::new(
        SurfaceModeWatcher::current() == SurfaceMode::Translucent,
    ));

    let (executor, text_ctx) = if render_thread_enabled() {
        spawn_split_executor(
            startup_spans,
            &window,
            physical,
            scale,
            cache_dir,
            Arc::clone(&translucent_resolved),
        )
    } else {
        build_inline_executor(
            startup_spans,
            &window,
            physical,
            cache_dir,
            &translucent_resolved,
        )?
    };

    // Process-once (the runtime's own `OnceLock` provides that property; a
    // repeat call — e.g. an activity recreated in the same process — just
    // re-marks the calling thread as the UI thread and swaps in a fresh no-op
    // waker). Must run BEFORE `make_app()`: a `State`'s own construction (a
    // future `Component::init()`) may create signals/controllers that need the
    // runtime already installed. Runs on this UI thread (never the render
    // thread): `init` claims the *calling* thread as the UI thread for
    // `spawn_local`, and every JNI call arrives on the JVM main thread.
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
            executor,
            text_ctx,
            window,
            physical,
            scale,
            translucent_resolved,
            app,
        )
    });

    // SAFETY: hand a uniquely-owned boxed handle to the JVM as `jlong`; it is
    // reclaimed exactly once in `native_on_destroy`.
    Ok(Box::into_raw(Box::new(handle)) as jlong)
}

/// Build the **inline** (`FRUST_NO_RENDER_THREAD`) executor: create the renderer +
/// surface on this UI thread and record the full startup line, exactly as the
/// pre-split shell did (plan phase 11.B, kill-switch path). Returns the executor
/// plus the joined [`TextContext`] the handle needs for layout.
///
/// `cache_dir` (when `Some`) is the app cache directory the pipeline-cache blob
/// is loaded from before GPU init and saved back to after renderer creation
/// (task 13, spec §14 phase 7.B) — best-effort and Vulkan-only.
fn build_inline_executor(
    mut startup_spans: StartupSpans,
    window: &NativeWindow,
    physical: (u32, u32),
    cache_dir: Option<String>,
    translucent_resolved: &AtomicBool,
) -> Result<(FrameExecutor, TextContext)> {
    let mut renderer = frust_render::SurfaceRenderer::new();

    // Pipeline-cache persistence (task 13): restore the blob a prior launch
    // persisted so vello's Vulkan shader pipelines are reused rather than
    // recompiled. Set BEFORE the surface install (where vello's renderer + cache
    // are created). Done before the pre-init join so the disk read overlaps the
    // background GPU thread's adapter/device work.
    let cache_path = cache_dir.as_deref().map(pipeline_cache_path);
    let loaded_cache = cache_path.as_deref().and_then(load_pipeline_cache);

    // Log loaded blob size + validation outcome (perf-gated, task 03).
    if perf::enabled() {
        if let Some(ref blob) = loaded_cache {
            log::info!(
                "frust-shell-android: loaded pipeline cache blob ({} bytes, validation: pending)",
                blob.len()
            );
        } else {
            log::info!("frust-shell-android: pipeline cache blob not found (cold start)");
        }
    }

    renderer.set_initial_pipeline_cache_data(loaded_cache.clone());
    startup_spans.record(SPAN_CACHE_LOADED);
    if loaded_cache.is_some() {
        startup_spans.record(perf::SPAN_PIPELINE_CACHE_RESTORED);
    }

    // Adopt the `JNI_OnLoad` GPU pre-init context (task 19), or fall back to a
    // fresh synchronous one. Bracketed by preinit_started/joined.
    startup_spans.record(SPAN_PREINIT_STARTED);
    let mut render_cx = take_preinit_context();
    startup_spans.record(SPAN_PREINIT_JOINED);

    let window_ptr = window.ptr().as_ptr().cast::<c_void>();

    // SAFETY: `window_ptr` comes from `window`, which is moved into the returned
    // `AndroidAppHandle` and (by that struct's `executor`-before-`window`
    // field-drop order) outlives the surface and all its textures (spec §8.1).
    pollster::block_on(unsafe {
        renderer.on_surface_created_from_android_window(
            &mut render_cx,
            window_ptr,
            physical.0,
            physical.1,
            surface_alpha_request(),
        )
    })
    .context("frust-shell-android: failed to create Android render surface")?;

    // Replace the request-seeded optimism with the real resolution (review
    // finding M1) — on this path the renderer lives on the UI thread, so the
    // handle's per-frame sync re-reads it from the renderer anyway; storing it
    // here keeps the flag correct for the construction-time push too.
    publish_resolved_translucency(
        translucent_resolved,
        Some(renderer.surface_resolved_translucent()),
    );

    startup_spans.record(perf::SPAN_ADAPTER_READY);
    startup_spans.record(perf::SPAN_DEVICE_READY);
    startup_spans.record(perf::SPAN_RENDERER_READY);

    // Persist the pipeline cache the driver populated, if it changed (task 13).
    if let Some(path) = cache_path {
        persist_pipeline_cache_if_changed(path, loaded_cache.as_deref(), &renderer);
    }

    // Font/`TextContext` warmup (phase 10.D): join the font-preload thread as
    // late as possible (max overlap with the GPU work above).
    startup_spans.record(SPAN_FONT_PREINIT_STARTED);
    let text_ctx = take_preinit_text_context();
    startup_spans.record(SPAN_FONT_PREINIT_JOINED);

    let executor = FrameExecutor::Inline(Box::new(InlineExecutor::new(
        render_cx,
        renderer,
        startup_spans,
    )));
    Ok((executor, text_ctx))
}

/// Spawn the **split** (default) render thread and return its executor handle
/// (plan phase 11.B). The GPU work — pre-init context adoption, surface creation,
/// pipeline cache, adapter/device/renderer spans — happens *on the render thread*
/// ([`render_loop`]), so `nativeInit` never blocks the UI thread on it. Only the
/// font-preload join (needed by UI-side layout) and the reactive-runtime claim
/// stay on this UI thread. Returns the executor plus the joined [`TextContext`].
fn spawn_split_executor(
    mut startup_spans: StartupSpans,
    window: &NativeWindow,
    physical: (u32, u32),
    scale: jfloat,
    cache_dir: Option<String>,
    translucent_resolved: Arc<AtomicBool>,
) -> (FrameExecutor, TextContext) {
    // Font/`TextContext` warmup (phase 10.D) stays UI-side — layout runs on the
    // UI thread. The GPU work is off-thread now, so this join's ordering vs GPU
    // bring-up no longer matters; record it before the recorder is moved into the
    // render thread below.
    startup_spans.record(SPAN_FONT_PREINIT_STARTED);
    let text_ctx = take_preinit_text_context();
    startup_spans.record(SPAN_FONT_PREINIT_JOINED);

    let (sender, receiver) = render_channel::<PaintedScene, SendableWindowPtr>();
    let (scene_return_tx, scene_return_rx) = scene_return_channel::<Scene>();
    let window_ptr = window.ptr().as_ptr().cast::<c_void>();
    let size = SurfaceSize {
        width: physical.0,
        height: physical.1,
        scale: scale as f64,
    };

    // Per-shell fatal flag (phase-11 fix F2): one clone lives in the render
    // thread (set on a first-install failure), one in the `SplitExecutor` (read
    // by `native_on_frame`). A plain `Arc<AtomicBool>` — no channel/protocol.
    let fatal = Arc::new(AtomicBool::new(false));
    let fatal_render = Arc::clone(&fatal);

    // Presented-frame counter (task 10): one clone drives into the render thread
    // (bumped on each `FrameOutcome::Rendered`), one stays in the `SplitExecutor`
    // for the UI thread to read before paint. Mirrors the `fatal` flag's shape.
    let presented = Arc::new(AtomicU64::new(0));
    let presented_render = Arc::clone(&presented);

    // Move `startup_spans` (init_entry + font spans already recorded) into the
    // render thread, which owns the rest of the startup line.
    let join = std::thread::Builder::new()
        .name("frust-render".to_string())
        // Guard the loop so a dev-build panic logs and exits cleanly (dropping the
        // owned `RenderReceiver`, which drains any orphaned `Ack` — the barrier
        // deadlock fix). A no-op under the release `panic = "abort"` profile.
        .spawn(move || {
            run_guarded_thread("frust-render (android)", move || {
                render_loop(
                    receiver,
                    startup_spans,
                    cache_dir,
                    fatal_render,
                    scene_return_tx,
                    presented_render,
                    translucent_resolved,
                )
            })
        })
        .expect("frust-shell-android: failed to spawn render thread");

    // Hand the initial surface to the render thread. The UI thread keeps `window`
    // (in the returned handle) alive until the render thread acks a later
    // `SurfaceDestroyed` (the window-release barrier — see `SendableWindowPtr`).
    sender.send_command(RenderCommand::SurfaceCreated {
        window: SendableWindowPtr::new(window_ptr),
        size,
    });

    (
        FrameExecutor::Split(SplitExecutor::new(
            sender,
            join,
            fatal,
            scene_return_rx,
            presented,
        )),
        text_ctx,
    )
}

/// Persist the current pipeline-cache blob to `path` on a background thread if it
/// differs from `loaded` (task 13).
///
/// Reads [`SurfaceRenderer::pipeline_cache_data`](frust_render::SurfaceRenderer::pipeline_cache_data)
/// (framed + adapter-fingerprinted; `None` without Vulkan `PIPELINE_CACHE`), and
/// spawns a **detached** writer thread so the first frame never waits on disk
/// I/O — the write outlives this function and the returned handle by design.
/// Every failure is logged and ignored: a failed persist only costs the next
/// launch its cold-compile time.
fn persist_pipeline_cache_if_changed(
    path: PathBuf,
    loaded: Option<&[u8]>,
    renderer: &frust_render::SurfaceRenderer,
) {
    let Some(data) = renderer.pipeline_cache_data() else {
        return; // no cache to persist (Metal/desktop, or nothing compiled)
    };
    let cache_changed = pipeline_cache_differs(loaded, &data);
    if !cache_changed {
        return; // unchanged since load — skip the rewrite
    }
    let data_len = data.len();
    let perf_enabled = perf::enabled();
    std::thread::spawn(move || match write_pipeline_cache_atomic(&path, &data) {
        Ok(()) => {
            log::debug!(
                "frust-shell-android: persisted pipeline cache ({} bytes) to {}",
                data_len,
                path.display()
            );
            // Log persisted size + whether it changed (perf-gated, task 03).
            if perf_enabled {
                log::info!(
                    "frust-shell-android: persisted pipeline cache ({} bytes, changed: {})",
                    data_len,
                    cache_changed
                );
            }
        }
        Err(err) => log::warn!(
            "frust-shell-android: failed to persist pipeline cache to {}: {err}",
            path.display()
        ),
    });
}

/// `nativeOnSurfaceChanged`: recreate the surface against a new window, or resize
/// it in place when the underlying window is unchanged.
///
/// `density` is the display's `resources.displayMetrics.density` for this
/// configuration, forwarded so a config change that alters the device pixel
/// ratio (a display move, a font-scale-driven density change) re-sanitizes the
/// stored scale used for layout/paint/inset math. It arrives alongside the
/// surface dimensions on the same `surfaceChanged` because a size change and a
/// density change are delivered together by Android's `SurfaceHolder.Callback`.
/// This is a **breaking** signature change from the pre-parity export: the
/// Kotlin `external` declaration and `FrustSurfaceView` call site gain the
/// trailing `density` argument in the same phase (task 08).
pub fn native_on_surface_changed(
    env: EnvUnowned,
    handle: jlong,
    surface: JObject,
    width: jint,
    height: jint,
    density: jfloat,
) {
    guard("nativeOnSurfaceChanged", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        let Some(app) = (unsafe { handle_mut(handle) }) else {
            return;
        };
        let physical = (width.max(1) as u32, height.max(1) as u32);

        // SAFETY: `env`/`surface` are the live JVM handles for this call.
        let Some(new_window) = (unsafe { native_window_from_surface(&env, &surface) }) else {
            log::error!("frust-shell-android: surfaceChanged with a null window");
            return;
        };

        // Same underlying window ⇒ a plain resize; a different (or first) window
        // ⇒ (re)create the surface against it.
        if app.window().map(NativeWindow::ptr) == Some(new_window.ptr()) {
            // Drop the extra reference `fromSurface` just acquired; the stored
            // window keeps the surface alive.
            drop(new_window);
            app.resize_surface(physical, density);
            return;
        }

        // Recreate against the new window. The render-thread split routes this
        // entirely through owned channel commands (safe, in `app`): a barriered
        // `SurfaceDestroyed` drops the old render-side surface before the old
        // window is released, then a `SurfaceCreated` hands the new pointer over.
        // The inline path drives the `unsafe` surface creation here (the sanctioned
        // FFI zone), then commits the new window with `set_window`.
        if app.executor_is_split() {
            app.split_recreate_surface(new_window, physical, density);
            return;
        }

        let window_ptr = new_window.ptr().as_ptr().cast::<c_void>();
        let result = {
            let Some((render_cx, renderer)) = app.inline_renderer_mut() else {
                // Unreachable: `executor_is_split()` was false just above.
                return;
            };
            // SAFETY: `window_ptr` is from `new_window`, which is moved into the
            // handle via `set_window` below (and thus outlives the surface); the
            // previous surface is torn down inside this call before the previous
            // window is released.
            pollster::block_on(unsafe {
                renderer.on_surface_created_from_android_window(
                    render_cx,
                    window_ptr,
                    physical.0,
                    physical.1,
                    surface_alpha_request(),
                )
            })
        };
        match result {
            Ok(()) => app.set_window(new_window, physical, density),
            Err(err) => {
                log::error!("frust-shell-android: surfaceChanged recreate failed: {err:#}")
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
///
/// Returns a `jboolean` (a real `bool` in this `jni` crate): `true` = keep
/// driving frames, `false` = a **fatal** render-thread failure (a first-surface
/// install that could not succeed — see [`render_loop`]/[`AndroidAppHandle::render_fatal`]),
/// on which Kotlin's `doFrame` stops the Choreographer loop rather than driving
/// doomed frames against a permanent black screen (phase-11 fix F2). This is a
/// signature-shape change moving in lockstep with the Kotlin `external`
/// declaration (the JNI symbol name is unchanged), following the
/// `nativeOnSurfaceChanged`-density precedent.
pub fn native_on_frame(handle: jlong, frame_time_nanos: jlong) -> jboolean {
    // Benign default `true` on a caught panic: a single frame's failure must not
    // stop the loop — only a render-thread FATAL does.
    guard("nativeOnFrame", true, || {
        let frame_time_nanos = crate::ffi_support::frame_time_nanos_from_jlong(frame_time_nanos);
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        let Some(app) = (unsafe { handle_mut(handle) }) else {
            return true; // no live handle — nothing has faulted; keep the loop alive
        };
        app.frame(frame_time_nanos);
        // `false` (fatal) tells Kotlin to stop the loop; `true` keeps driving.
        !app.render_fatal()
    })
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
            log::debug!("frust-shell-android: onResume");
        }
    });
}

/// `nativeOnPause`: activity paused. Bookkeeping only in v0 (see
/// [`native_on_resume`]), plus (platform-views task 05) hiding every
/// currently-visible platform-view slot: a backgrounded app's native
/// sibling views should disappear with it rather than linger on top of
/// whatever now shows behind the (possibly composited-away) frust surface.
pub fn native_on_pause(handle: jlong) {
    guard("nativeOnPause", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            log::debug!("frust-shell-android: onPause");
            app.suspend_platform_views();
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

/// `nativeSystemUiState`: return the process-wide system-UI override slot's
/// packed `(generation, mode)` state (task 09) for Kotlin's `doFrame` to poll,
/// mirroring the proven `nativeImeState`-in-`doFrame` per-frame-poll idiom.
///
/// Returns [`frust_shell_common::encoded_state`] verbatim — the packing
/// scheme (`(generation << 8) | mode_bits`) and its unit tests live in
/// `frust_shell_common::system_ui`'s module docs, which task 14's Kotlin
/// decoder is written against; this export is a thin, `guard`-wrapped
/// one-liner over that already-tested encoding fn, per this task's contract.
/// The slot is process-global (task 03), not per-handle, so no
/// `AndroidAppHandle` lookup is needed beyond the standard liveness check
/// every native call makes: a missing handle returns `0` (generation `0`,
/// `EdgeToEdge` — "nothing to apply", matching the slot's own initial state
/// per the module docs) rather than a live peek, so a torn-down native side
/// never reports a stale mode as pending.
pub fn native_system_ui_state(handle: jlong) -> jlong {
    guard("nativeSystemUiState", 0, || {
        if !crate::ffi_support::handle_is_live(handle) {
            return 0;
        }
        frust_shell_common::encoded_state() as jlong
    })
}

/// `nativeSetSurfaceMode`: declare a translucent (alpha-channel) GPU surface
/// before the surface is created (platform-views task 05).
///
/// The generated Kotlin glue calls this **only** from the same
/// `FRUST_TRANSLUCENT_SURFACE`-gated branch that already set
/// `SurfaceHolder`'s `PixelFormat.TRANSLUCENT` and arranged the
/// native-sibling z-order, and always **before** `nativeInit` (template
/// task 08's contract) — forwarding straight to
/// [`declare_host_translucent_surface`], the process-wide, one-way pre-init
/// latch (`frust_shell_common::surface_mode`'s module docs' Latch contract:
/// the surface format is fixed at creation, so there is no "revert" call and
/// no live re-flip). Calling this from anywhere other than that host-glue
/// branch — e.g. without the matching `PixelFormat` already set — is a
/// host-template bug, not a supported opt-in (review M3); this is why the
/// underlying function is not re-exported past `frust-shell-common`. A call
/// after [`ANY_SURFACE_CREATED`] is already set (i.e. after some
/// `nativeInit` in this process has begun) is logged and is a no-op for the
/// current surface — it cannot retroactively change a format already
/// chosen. `translucent == false` is always a no-op too: there is nothing to
/// "un-latch".
///
/// Declaring only sets the *request*: a device advertising no translucent
/// alpha mode still comes up opaque, and the paint contract follows the
/// RESOLVED outcome (`frust_render::SurfaceRenderer::surface_resolved_translucent`,
/// review finding M1), not this latch.
pub fn native_set_surface_mode(translucent: jboolean) {
    guard("nativeSetSurfaceMode", (), || {
        if !translucent {
            return;
        }
        if ANY_SURFACE_CREATED.load(Ordering::Acquire) {
            log::warn!(
                "frust-shell-android: nativeSetSurfaceMode(true) called after a surface \
                 already exists in this process; the translucent-surface latch is \
                 pre-init-only (v1) and has no effect on the current surface"
            );
            return;
        }
        declare_host_translucent_surface();
    });
}

/// `nativePlatformViewCommands`: return the native-sibling-compositor command
/// backlog (task 03's differ) as JSON, for Kotlin's per-frame poll —
/// mirrors [`native_ime_state`]'s shape exactly (fresh JSON per call,
/// JNI-owned `JString`, guard-wrapped, `null` on a missing handle).
///
/// `ack_generation` is the generation Kotlin's own last successful poll
/// returned (round-tripped back on the next call, `0` on the first ever
/// poll); this first [`AndroidAppHandle::acknowledge_platform_view_commands`]s
/// it — compacting the differ's backlog
/// (`frust_shell_common::platform_view`'s module docs' Generation/
/// acknowledgement section) — then serializes the resulting
/// `(generation, &[ViewCommand])` snapshot. A negative `ack_generation`
/// (untrusted JNI input) clamps to `0` rather than wrapping through the
/// `as u64` cast.
///
/// **Frozen JSON wire contract** (byte-identical, served by task 06's iOS
/// shell too — see [`build_platform_view_commands_json`]):
///
/// ```text
/// {"generation":7,"commands":[
///  {"op":"create","slot":3,"viewType":"dev.frust.XFactory","params":"{...}"},
///  {"op":"update","slot":3,"rect":[x,y,w,h],"clip":[x,y,w,h]|null,"visible":true},
///  {"op":"updateParams","slot":3,"params":"{...}"},
///  {"op":"dispose","slot":3}]}
/// ```
///
/// Rects are **PHYSICAL px** — [`platform_view_commands_to_json`] multiplies
/// the differ's logical-px rects by this handle's stored scale factor at
/// this boundary (the physical-at-FFI/logical-inside rule, applied outbound
/// — mirrors [`native_on_insets_changed`]'s inbound direction) so Kotlin does
/// zero density math.
///
/// Returns a null `jstring` on the no-change fast path
/// ([`platform_view_commands_up_to_date`] — the `nativeSystemUiState`
/// cheapness bar: a no-change poll costs one JNI call and no allocation) or
/// when there is no live native handle.
pub fn native_platform_view_commands(
    mut env: EnvUnowned,
    handle: jlong,
    ack_generation: jlong,
) -> jstring {
    guard("nativePlatformViewCommands", std::ptr::null_mut(), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        let Some(app) = (unsafe { handle_mut(handle) }) else {
            return std::ptr::null_mut();
        };
        let ack_generation = ack_generation.max(0) as u64;
        app.acknowledge_platform_view_commands(ack_generation);
        let (generation, commands) = app.platform_view_commands();
        if platform_view_commands_up_to_date(generation, ack_generation) {
            return std::ptr::null_mut(); // no-change fast path
        }
        let json_commands = platform_view_commands_to_json(commands, app.sanitized_scale());
        let json = build_platform_view_commands_json(generation, &json_commands);
        env.with_env(|env| Ok::<JObject, jni::errors::Error>(JString::new(env, &json)?.into()))
            .resolve::<LogErrorAndDefault>()
            .into_raw()
    })
}

/// Map the differ's [`ViewCommand`] backlog (task 03) onto the host-testable
/// [`PlatformViewCommandJson`] the JSON builder consumes, converting each
/// rect/clip from the differ's logical px to **physical** px at this FFI
/// boundary — mirrors [`ime_state_to_json`]'s caret-rect conversion.
fn platform_view_commands_to_json(
    commands: &[ViewCommand],
    scale: f64,
) -> Vec<PlatformViewCommandJson> {
    commands
        .iter()
        .map(|cmd| match cmd {
            ViewCommand::Create {
                slot_id,
                view_type,
                params_json,
            } => PlatformViewCommandJson::Create {
                slot_id: *slot_id,
                view_type: view_type.clone(),
                params_json: params_json.clone(),
            },
            ViewCommand::Update {
                slot_id,
                rect,
                clip,
                visible,
            } => PlatformViewCommandJson::Update {
                slot_id: *slot_id,
                rect: scale_rect(*rect, scale),
                clip: clip.map(|c| scale_rect(c, scale)),
                visible: *visible,
            },
            ViewCommand::UpdateParams {
                slot_id,
                params_json,
            } => PlatformViewCommandJson::UpdateParams {
                slot_id: *slot_id,
                params_json: params_json.clone(),
            },
            ViewCommand::Dispose { slot_id } => {
                PlatformViewCommandJson::Dispose { slot_id: *slot_id }
            }
        })
        .collect()
}

/// Scale one differ rect (logical px) to physical px, as the `(x, y, width,
/// height)` tuple [`PlatformViewCommandJson`]'s JSON builder expects.
fn scale_rect(rect: kurbo::Rect, scale: f64) -> (f32, f32, f32, f32) {
    (
        (rect.x0 * scale) as f32,
        (rect.y0 * scale) as f32,
        (rect.width() * scale) as f32,
        (rect.height() * scale) as f32,
    )
}

/// `nativeOnDeepLink`: deliver a platform deep link (cold-start, forwarded
/// from `FrustActivity.onCreate`'s `intent?.data`, or running, from
/// `FrustActivity.onNewIntent` — see `platform/android/frust-embedding/src/main/kotlin/dev/frust/`'s
/// `FrustActivity`/`FrustSurfaceView` queue-until-handle-ready contract,
/// task 07) into the process-wide deep-link source
/// ([`frust_reactive::push_deep_link`]).
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

/// `nativeOnInsetsChanged`: deliver the platform window insets (device px) into
/// the retained tree (device-parity task 06, RESEARCH.md "Insets / SafeArea").
///
/// The eight `jfloat`s are two per-edge sets in the order `WindowInsets` /
/// [`logical_insets`](frust_shell_common::logical_insets) expect —
/// `view_padding` (`vp_*`: system-bar/cutout occlusion, from Android's
/// `WindowInsetsCompat.Type.systemBars() | displayCutout()`) then `view_insets`
/// (`vi_*`: the fully-obscured IME area, from `Type.ime()`), each `left`/`top`/
/// `right`/`bottom`. Values are **physical px** (`Insets` are pixel-valued); the
/// handle converts them to logical px with its stored scale and pushes them onto
/// the render root ([`AndroidAppHandle::set_insets`]), which skips a no-op push
/// (`WindowInsets` is `PartialEq`) and, on a real change, marks `LAYOUT | PAINT`
/// pending so the next frame relayouts — the same dirtiness path a `set_theme`
/// uses (task 01), so no new frame-gate input is needed. A missing handle is a
/// no-op (Kotlin only pushes insets after `nativeInit` yields a live handle).
#[allow(clippy::too_many_arguments)]
pub fn native_on_insets_changed(
    handle: jlong,
    vp_l: jfloat,
    vp_t: jfloat,
    vp_r: jfloat,
    vp_b: jfloat,
    vi_l: jfloat,
    vi_t: jfloat,
    vi_r: jfloat,
    vi_b: jfloat,
) {
    guard("nativeOnInsetsChanged", (), || {
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

/// `nativeOnBackPress`: the Android hardware/gesture back contract (device-parity
/// task 06, RESEARCH.md "Android back"). Returns whether the framework consumed
/// the press: `JNI_TRUE` (this `jni`'s `jboolean` is a real `bool`) means Kotlin
/// should NOT finish the activity — the framework will pop on its next rebuild;
/// `JNI_FALSE` lets the default `OnBackPressedDispatcher` run (activity finish).
///
/// The read of [`handles_back`] is synchronous while the [`push_back_press`] pop
/// is applied on the next rebuild. [`handles_back`] consults the facade's live
/// can-pop provider (device-parity fix F2 — see `frust_reactive::back`'s
/// timing note), so it reflects the navigator's CURRENT stack depth at press
/// time rather than a stale previous-frame snapshot: a press arriving right
/// after a page push is decided against the real depth, not a rebuild-time flag
/// the frame gate might not yet have refreshed. A root-level back still reports
/// `false` and falls through, and the navigator's own `len > 1` guard keeps a
/// mis-predicted pop a safe no-op. Pumps the reactive local-task queue first,
/// like the other input entry points, so a just-drained task's state is observed
/// before the decision. A missing handle returns `false` (no live app ⇒ let the
/// platform exit).
pub fn native_on_back_press(handle: jlong) -> jboolean {
    guard("nativeOnBackPress", false, || {
        pump_reactive_runtime();
        if !crate::ffi_support::handle_is_live(handle) {
            return false;
        }
        let consume = crate::ffi_support::should_consume_back_press(handles_back());
        if consume {
            push_back_press();
        }
        consume
    })
}

/// `nativeInitAccessibility`: attach the accesskit Android adapter to the host
/// `FrustSurfaceView` (phase 6d D3, spec §9). Called by Kotlin's
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
                log::error!("frust-shell-android: nativeInitAccessibility: invalid JNIEnv: {err}");
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

    fn test_logic(state: &mut TestState) -> impl frust_core::view::View<TestState> + use<> {
        state.n += 1;
        frust_widgets::text(format!("{}", state.n))
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
    ) -> impl frust_core::view::View<NonDefaultState> + use<> {
        state.n += 1;
        frust_widgets::text(format!("{}", state.n))
    }

    crate::android_app!(NonDefaultState, make_state, test_logic);
}
