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

use std::ffi::{CStr, CString, c_char, c_void};
use std::io::Write;
use std::sync::Once;

use anyhow::{Context, Result, bail};

use forgekit_core::event::{EditingState, ImeState};
use forgekit_reactive::ReactiveRuntime;
use forgekit_render::{RenderContext, SurfaceRenderer};
use forgekit_shell_common::{AppTree, guard};

use crate::app::IosAppHandle;
use crate::ffi_support::CaretRect;

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

/// A no-op [`forgekit_reactive::FrameWaker`] for the mobile shell: the
/// `CADisplayLink` loop already produces every frame regardless of a signal
/// write, so there is nothing useful for the waker to do (contrast the
/// desktop shell, which must nudge `ControlFlow::Wait` awake).
fn no_op_waker() -> forgekit_reactive::FrameWaker {
    std::sync::Arc::new(|| {})
}

/// Drains the UI-thread reactive local-task queue if the runtime has been
/// installed. A no-op before `forgekit_init` has run (nothing to pump yet) —
/// every call site below is reachable from Swift before init on a
/// misbehaving caller, so this stays defensive rather than assuming `Some`.
fn pump_reactive() {
    if let Some(rt) = ReactiveRuntime::get() {
        rt.pump_local();
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

    // Process-wide reactive runtime init (idempotent — `ReactiveRuntime::init`'s
    // own `OnceLock` provides the process-once property). The Swift-side
    // `handle`/`initFailed` guards are only per-view-controller: a locale
    // change, split-screen resize, or an init retry after a prior failure can
    // re-enter `forgekit_init` in the same process (the project's own
    // g2-swift-init-latch history shows this happens), and a repeat call here
    // must be benign rather than rebuilding the background tokio runtime. Runs
    // BEFORE app construction so a `Component::init` (a future task) creating
    // signals/controllers has a live runtime to create them against. The
    // continuous `CADisplayLink` loop already ticks every frame regardless of a
    // signal write, so the waker is a no-op (mirrors the Android shell).
    ReactiveRuntime::init(no_op_waker());

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

/// `forgekit_dispatch_touch`: deliver one touch contact to the tree (spec §9).
///
/// `phase` is the fixed code the Swift `ForgeKitView` touch overrides send
/// (`0`=began, `1`=moved, `2`=ended, `3`=cancelled — see
/// [`crate::ffi_support::touch_phase_from_code`]); `x`/`y` are logical points
/// (`touch.location(in:)`), passed straight through (no scale division — see the
/// asymmetry note on [`IosAppHandle::dispatch_touch`]). First-touch only in v1.
pub fn dispatch_touch(handle: *mut c_void, phase: u32, x: f32, y: f32) {
    guard("forgekit_dispatch_touch", (), || {
        // Cheap; keeps controller-driven updates fresh between CADisplayLink
        // frames rather than waiting for the next `forgekit_render_frame`.
        pump_reactive();
        // SAFETY: `handle` is a live handle for this call (see `handle_mut`).
        if let Some(app) = unsafe { handle_mut(handle) } {
            let touch_phase = crate::ffi_support::touch_phase_from_code(phase);
            app.dispatch_touch(touch_phase, x, y);
        }
    });
}

/// `forgekit_ime_apply`: push a whole editing state from the Swift `UITextInput`
/// mirror into the focused widget (the mobile state-sync path — spec §9 / Phase
/// 4B). Routed to the focused widget as an `ImeEvent::ApplyEditingState` via
/// [`AppTree::ime_apply`].
///
/// `text` is the mirror's UTF-8 bytes; `sel_*`/`comp_*` are **UTF-16 code-unit**
/// indices (the platform-native unit the `NSMutableString` mirror counts in),
/// passed opaquely through the [`EditingState`] shell seam — the widget /
/// `forgekit-text` converts them to Rust byte offsets at its own boundary (task
/// 52 owns the conversion). `-1` denotes "none" for the composing region.
///
/// **Return-key contract:** the Swift side maps the `.done` Return key to an
/// `insertText("\n")`, so a lone `"\n"` insertion arriving here is the submit
/// gesture; the `TextInput` widget (task 53) treats a single-line newline insert
/// as its `on_submit` trigger.
pub fn ime_apply(
    handle: *mut c_void,
    text: *const c_char,
    sel_base: i32,
    sel_ext: i32,
    comp_base: i32,
    comp_ext: i32,
) {
    guard("forgekit_ime_apply", (), || {
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

/// `forgekit_ime_state_json`: the focused field's IME surface as a heap-allocated,
/// caller-freed JSON C string (the Swift bridge parses it to drive
/// `becomeFirstResponder`, seed its mirror, and reconcile after each edit).
///
/// Shape matches the Android bridge — see [`crate::ffi_support::ime_state_json`].
/// Returns a fresh `CString` the caller **must** release via
/// [`string_free`]/`forgekit_string_free`; a null return (no live handle, or a
/// text containing an interior NUL) is the "no editing state" sentinel the Swift
/// side treats as inactive.
pub fn ime_state_json(handle: *mut c_void) -> *mut c_char {
    guard("forgekit_ime_state_json", std::ptr::null_mut(), || {
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

/// `forgekit_string_free`: release a C string previously returned by
/// [`ime_state_json`]/`forgekit_ime_state_json`. Idempotent on null.
pub fn string_free(s: *mut c_char) {
    guard("forgekit_string_free", (), || {
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

/// Convert the focused widget's published [`ImeState`] (or its absence) into the
/// bridge JSON, mapping the logical-pixel caret rect into the flat
/// caretX/Y/W/H the Swift side expects.
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
            )
        }
        // No focused field / no published surface: the inactive sentinel.
        None => crate::ffi_support::ime_state_json(false, "", -1, -1, -1, -1, None),
    }
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
        // While backgrounded, CADisplayLink is paused so nothing pumps and
        // tokio timers stall (accepted gap — see `ReactiveRuntime::pump_local`
        // docs and task 08's design notes); drain any queued completions
        // immediately on foreground instead of waiting for the next
        // `forgekit_render_frame` tick.
        pump_reactive();
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

/// Compile-only smoke of [`crate::ios_app!`]'s 2-arg (`Default`-state) arm:
/// exercises macro expansion on the iOS target
/// (`cargo check --target aarch64-apple-ios-sim --tests`), covering the macro
/// half of the acceptance criteria. Never invoked — its symbols would clash
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

/// Compile-only smoke of [`crate::ios_app!`]'s 3-arg state-factory arm, with a
/// state type that deliberately has **no** `Default` impl — the only way it can
/// build is through the supplied `$state_init` closure (acceptance criterion
/// 2). Lives in its own module (distinct from [`macro_expansion`]'s 2-arg
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
    ) -> impl forgekit_core::view::View<NonDefaultState> + use<> {
        state.n += 1;
        forgekit_widgets::text(format!("{}", state.n))
    }

    crate::ios_app!(NonDefaultState, init_state, test_logic);
}
