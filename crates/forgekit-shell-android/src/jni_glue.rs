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
use jni::objects::JObject;
use jni::sys::{jfloat, jint, jlong};
use ndk::native_window::NativeWindow;

use forgekit_shell_common::{AppTree, guard};

use crate::app::AndroidAppHandle;

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

    let app = make_app();
    let handle = AndroidAppHandle::new(render_cx, renderer, window, physical, scale, app);

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
/// is ready). `frame_time_nanos` is unused in v0 (no animation clock yet).
pub fn native_on_frame(handle: jlong, _frame_time_nanos: jlong) {
    guard("nativeOnFrame", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.frame();
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

/// Compile-only smoke of [`crate::android_app!`]: exercises macro expansion on
/// the Android target (`cargo check --target aarch64-linux-android`), covering
/// acceptance criterion 2. Never invoked — its symbols would clash with a real
/// app's, so it lives behind `cfg(test)` where no `cdylib` links it.
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
