//! The FFI boundary: the non-generic runtime the [`crate::ios_app!`]-stamped
//! `extern "C" fn forgekit_*`s delegate to, and where this crate's `unsafe` is
//! confined.
//!
//! The crate's `unsafe` surface is this module plus the `#[unsafe(no_mangle)]`
//! attributes the [`crate::ios_app!`] macro emits on its generated exports
//! (edition-2024 spells `no_mangle` as an unsafe attribute). Every entry point
//! here is wrapped in [`guard`](forgekit_shell_common::guard) so a panic is
//! caught and turned into a benign default instead of unwinding across the C-ABI
//! boundary (undefined behaviour). The `unsafe` *code* in this module is confined
//! to three things, each with a safety comment: the calls into
//! `on_surface_created_from_metal_layer` (raw `CAMetalLayer*` → surface — at init
//! in [`create_handle`] and on surface recovery in [`recover_surface`]),
//! `Box::into_raw`/`from_raw` for the opaque handle's lifetime, and reconstituting
//! the raw handle pointer as a `&mut`.

use std::ffi::c_void;
use std::io::Write;
use std::sync::Once;

use anyhow::{Context, Result, bail};

use forgekit_render::{RenderContext, SurfaceRenderer};
use forgekit_shell_common::{AppTree, guard};

use crate::app::IosAppHandle;

/// A minimal `log::Log` writing to stderr, installed once in [`init`].
///
/// Rationale (RESEARCH.md): `simctl launch --console-pty` captures a simulator
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
                "[forgekit {}] {}",
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
/// `FORGEKIT_LOG` (e.g. `debug`, `trace`, `warn`) overrides it.
fn init_logger_once() {
    static LOGGER: Once = Once::new();
    LOGGER.call_once(|| {
        let level = std::env::var("FORGEKIT_LOG")
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

/// `forgekit_init`: build the native handle for the app's `CAMetalLayer` and
/// return it to Swift as an opaque pointer (spec §10.2).
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
    guard(
        "forgekit_init",
        std::ptr::null_mut(),
        || match create_handle(metal_layer, width, height, scale, make_app) {
            Ok(handle) => handle,
            Err(err) => {
                log::error!("forgekit-shell-ios: forgekit_init failed: {err:#}");
                std::ptr::null_mut()
            }
        },
    )
}

/// Fallible body of [`init`], separated so the happy path reads top-down.
fn create_handle(
    metal_layer: *mut c_void,
    width: u32,
    height: u32,
    scale: f32,
    make_app: impl FnOnce() -> Box<dyn AppTree>,
) -> Result<*mut c_void> {
    if crate::ffi_support::handle_is_null(metal_layer) {
        bail!("forgekit-shell-ios: forgekit_init called with a null CAMetalLayer");
    }

    let mut render_cx = RenderContext::new();
    let mut renderer = SurfaceRenderer::new();
    let physical = (width.max(1), height.max(1));

    // SAFETY: `metal_layer` is a valid, live `CAMetalLayer*` per the FFI contract
    // (owned by the Swift `UIView`, guaranteed to outlive this handle because the
    // Swift side calls `forgekit_destroy` before releasing the view/layer).
    pollster::block_on(unsafe {
        renderer.on_surface_created_from_metal_layer(
            &mut render_cx,
            metal_layer,
            physical.0,
            physical.1,
        )
    })
    .context("forgekit-shell-ios: failed to create Metal render surface")?;

    let app = make_app();
    // Retain `metal_layer` in the handle so a later `SurfaceLost` can be recovered
    // by recreating the surface from it (iOS never re-delivers the layer).
    let handle = IosAppHandle::new(render_cx, renderer, metal_layer, physical, scale, app);

    // SAFETY: hand a uniquely-owned boxed handle to Swift as a raw pointer; it is
    // reclaimed exactly once in `destroy`.
    Ok(Box::into_raw(Box::new(handle)) as *mut c_void)
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
/// process killed); a loss during backgrounding recovers after `forgekit_resume`.
/// This is the sole recovery call into `on_surface_created_from_metal_layer`
/// outside [`create_handle`], and keeps the `unsafe` confined to this module.
fn recover_surface(app: &mut IosAppHandle, physical: (u32, u32), scale: f32) {
    // Read the retained pointer before taking the `&mut` borrow of the renderer.
    let metal_layer = app.metal_layer();
    let result = {
        let (render_cx, renderer) = app.renderer_mut();
        // SAFETY: `metal_layer` is the Swift-owned `CAMetalLayer*` this handle was
        // created with; Swift guarantees it outlives the handle (it calls
        // `forgekit_destroy` before releasing the view/layer), so recreating a
        // surface from it — exactly as `create_handle` did at init — is sound.
        pollster::block_on(unsafe {
            renderer.on_surface_created_from_metal_layer(
                render_cx,
                metal_layer,
                physical.0,
                physical.1,
            )
        })
    };
    match result {
        Ok(()) => app.set_surface(physical, scale),
        Err(err) => {
            app.record_recreate_failure();
            if app.recreate_failures() >= crate::ffi_support::MAX_RECREATE_ATTEMPTS {
                log::error!(
                    "forgekit-shell-ios: surface recreate failed {} times: {err:#}; \
                     giving up for this SurfaceLost episode (rendering disabled)",
                    app.recreate_failures()
                );
            } else {
                log::error!("forgekit-shell-ios: surface recreate failed: {err:#}");
            }
        }
    }
}

/// `forgekit_resize`: resize the live surface (rotation / bounds change). The
/// `CAMetalLayer` survives, so a live surface is a plain in-place resize; a
/// `SurfaceLost` surface is instead recreated from the retained layer at the
/// incoming dimensions (self-recovery — see [`recover_surface`]).
pub fn resize(handle: *mut c_void, width: u32, height: u32, scale: f32) {
    guard("forgekit_resize", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            let physical = (width.max(1), height.max(1));
            if crate::ffi_support::should_recreate_surface(
                app.phase(),
                app.paused(),
                app.recreate_failures(),
            ) {
                recover_surface(app, physical, scale);
            } else {
                app.resize(physical, scale);
            }
        }
    });
}

/// `forgekit_render_frame`: run one `CADisplayLink`-driven frame (no-op unless the
/// surface is ready and the app is not paused). If the surface was lost, first
/// recreate it from the retained layer at the last-known size/scale (self-recovery
/// — see [`recover_surface`]) so a `SurfaceLost` is no longer a permanent black
/// screen.
pub fn render_frame(handle: *mut c_void) {
    guard("forgekit_render_frame", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            if crate::ffi_support::should_recreate_surface(
                app.phase(),
                app.paused(),
                app.recreate_failures(),
            ) {
                let (physical, scale) = (app.physical(), app.scale());
                recover_surface(app, physical, scale);
            }
            app.frame();
        }
    });
}

/// `forgekit_pause`: app backgrounded — stop submitting frames (see
/// [`IosAppHandle::pause`]).
pub fn pause(handle: *mut c_void) {
    guard("forgekit_pause", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.pause();
        }
    });
}

/// `forgekit_resume`: app foregrounded — resume submitting frames (see
/// [`IosAppHandle::resume`]).
pub fn resume(handle: *mut c_void) {
    guard("forgekit_resume", (), || {
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            app.resume();
        }
    });
}

/// `forgekit_destroy`: reclaim and drop the boxed handle (which drops the surface;
/// the Swift-owned layer is released separately, afterwards). Idempotent from
/// Swift's side because it nulls its handle right after calling this.
pub fn destroy(handle: *mut c_void) {
    guard("forgekit_destroy", (), || {
        if crate::ffi_support::handle_is_null(handle) {
            return;
        }
        // SAFETY: `handle` was produced by `Box::into_raw` in `create_handle` and
        // is reclaimed exactly once here; the Swift side nulls its copy afterwards
        // so it is never passed back in.
        drop(unsafe { Box::from_raw(handle as *mut IosAppHandle) });
    });
}

/// Compile-only smoke of [`crate::ios_app!`]: exercises macro expansion on the iOS
/// target (`cargo check --target aarch64-apple-ios-sim --tests`), covering the
/// macro half of the acceptance criteria. Never invoked — its symbols would clash
/// with a real app's, so it lives behind `cfg(test)` where no `cdylib`/`staticlib`
/// links it.
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

    crate::ios_app!(TestState, test_logic);
}
