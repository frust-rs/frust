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
use std::ptr::NonNull;
use std::sync::Once;

use anyhow::{Context, Result};
use jni::EnvUnowned;
use jni::errors::LogErrorAndDefault;
use jni::objects::{JObject, JString};
use jni::sys::{jboolean, jfloat, jint, jlong, jstring};
use ndk::native_window::NativeWindow;

use forgekit_core::event::{EditingState, ImeState};
use forgekit_reactive::{ReactiveRuntime, push_deep_link};
use forgekit_shell_common::{AppTree, guard};

use crate::app::AndroidAppHandle;
use crate::ffi_support::{ImeJsonState, build_ime_state_json, normalize_ime_indices};

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
pub fn native_init(
    env: EnvUnowned,
    surface: JObject,
    scale: jfloat,
    make_app: impl FnOnce() -> Box<dyn AppTree>,
) -> jlong {
    init_logger_once();
    guard("nativeInit", 0, || {
        match create_handle(&env, &surface, scale, make_app) {
            Ok(handle) => handle,
            Err(err) => {
                log::error!("forgekit-shell-android: nativeInit failed: {err:#}");
                0
            }
        }
    })
}

/// Fallible body of [`native_init`], separated so the happy path reads top-down.
fn create_handle(
    env: &EnvUnowned,
    surface: &JObject,
    scale: jfloat,
    make_app: impl FnOnce() -> Box<dyn AppTree>,
) -> Result<jlong> {
    // SAFETY: `env`/`surface` are the live JVM handles for this call.
    let window = unsafe { native_window_from_surface(env, surface) }
        .context("forgekit-shell-android: ANativeWindow_fromSurface returned null")?;
    let physical = window_physical_size(&window);

    let mut render_cx = forgekit_render::RenderContext::new();
    let mut renderer = forgekit_render::SurfaceRenderer::new();
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
        AndroidAppHandle::new(render_cx, renderer, window, physical, scale, app)
    });

    // SAFETY: hand a uniquely-owned boxed handle to the JVM as `jlong`; it is
    // reclaimed exactly once in `native_on_destroy`.
    Ok(Box::into_raw(Box::new(handle)) as jlong)
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
