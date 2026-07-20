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
//! to four things, each with a safety comment: the calls into
//! `on_surface_created_from_metal_layer` (raw `CAMetalLayer*` → surface — at init
//! in [`create_handle`] and on surface recovery in [`recover_surface`]), the
//! `accessibility::IosA11yAdapter::new` construction in [`init_accessibility`]
//! (raw `UIView*` → adapter, handed to the now-safe
//! `IosAppHandle::attach_accessibility`), `Box::into_raw`/`from_raw` for the
//! opaque handle's lifetime, and reconstituting the raw handle pointer as a
//! `&mut`.

use std::ffi::{CStr, CString, c_char, c_void};
use std::io::Write;
use std::sync::Once;

use anyhow::{Context, Result, bail};

use frust_core::event::{EditingState, ImeState};
use frust_reactive::{ReactiveRuntime, push_deep_link};
use frust_render::{RenderContext, SurfaceRenderer};
use frust_shell_common::perf::{self, StartupSpans};
use frust_shell_common::{AppTree, guard};
use frust_text::TextContext;

use crate::accessibility::IosA11yAdapter;
use crate::app::IosAppHandle;
use crate::ffi_support::CaretRect;

/// Startup-span name (phase 10.D): `create_handle` is about to join the
/// background font-preload thread [`create_handle`] spawned at the top of its
/// own body (see that fn's doc — iOS has no `JNI_OnLoad`-equivalent
/// process-wide load hook to spawn it earlier from, so the thread is spawned
/// as early as possible inside `create_handle` itself instead, overlapping the
/// synchronous GPU surface/device bring-up below it). Mirrors the Android
/// shell's `SPAN_FONT_PREINIT_STARTED`/`SPAN_FONT_PREINIT_JOINED` pair.
const SPAN_FONT_PREINIT_STARTED: &str = "font_preinit_started";

/// Startup-span name (phase 10.D): the background font-preload join returned —
/// the pre-built [`TextContext`] was adopted, or `create_handle` fell back to
/// a synchronous [`TextContext::new`]. See [`SPAN_FONT_PREINIT_STARTED`].
const SPAN_FONT_PREINIT_JOINED: &str = "font_preinit_joined";

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
/// `FrustView` (phase-6d task 05, D3-ios).
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

/// Fallible body of [`init`], separated so the happy path reads top-down.
fn create_handle(
    metal_layer: *mut c_void,
    width: u32,
    height: u32,
    scale: f32,
    make_app: impl FnOnce() -> Box<dyn AppTree>,
) -> Result<*mut c_void> {
    // Startup-span recorder (spec §14 phase 7.A task 09): begins its epoch
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

    // Font/`TextContext` warmup (phase 10.D): iOS has no `JNI_OnLoad`-style
    // process-wide load hook to start this earlier from (unlike the Android
    // shell — `frust_init` is the earliest Rust entry point Swift ever calls),
    // so spawn the background thread here, as the very first thing, right
    // before the synchronous GPU surface/device bring-up below — the iOS
    // counterpart to Android's pre-init overlap window. `TextContext::new` has
    // no fallible step, so this thread cannot fail, only panic (handled at the
    // join below). Joined as late as possible (right before
    // `IosAppHandle::new` needs it) to maximize overlap with the GPU work.
    let font_preinit = std::thread::spawn(TextContext::new);

    let mut render_cx = RenderContext::new();
    let mut renderer = SurfaceRenderer::new();
    let physical = (width.max(1), height.max(1));

    // SAFETY: `metal_layer` is a valid, live `CAMetalLayer*` per the FFI contract
    // (owned by the Swift `UIView`, guaranteed to outlive this handle because the
    // Swift side calls `frust_destroy` before releasing the view/layer).
    pollster::block_on(unsafe {
        renderer.on_surface_created_from_metal_layer(
            &mut render_cx,
            metal_layer,
            physical.0,
            physical.1,
        )
    })
    .context("frust-shell-ios: failed to create Metal render surface")?;
    // `RenderContext::ensure_device` (see `frust-render/src/context.rs`)
    // creates the adapter, the logical device, and this call's surface/
    // renderer readiness in one async chain with no finer-grained seam
    // exposed to a shell — all three spans land at this single point rather
    // than three distinct timestamps, an acknowledged granularity limit
    // (the Android shell's `on_surface_created_from_android_window` has the
    // same shape).
    startup.record(perf::SPAN_ADAPTER_READY);
    startup.record(perf::SPAN_DEVICE_READY);
    startup.record(perf::SPAN_RENDERER_READY);

    // Process-wide reactive runtime init (idempotent — `ReactiveRuntime::init`'s
    // own `OnceLock` provides the process-once property). The Swift-side
    // `handle`/`initFailed` guards are only per-view-controller: a locale
    // change, split-screen resize, or an init retry after a prior failure can
    // re-enter `frust_init` in the same process (the project's own
    // g2-swift-init-latch history shows this happens), and a repeat call here
    // must be benign rather than rebuilding the background tokio runtime. Runs
    // BEFORE app construction so a `Component::init` (a future task) creating
    // signals/controllers has a live runtime to create them against. The
    // continuous `CADisplayLink` loop already ticks every frame regardless of a
    // signal write, so the waker is a no-op (mirrors the Android shell).
    let rt = ReactiveRuntime::init(no_op_waker());

    // Join the font-preload thread spawned at the top of this function, as
    // late as possible — right before `IosAppHandle::new` actually needs the
    // result — so the join has the maximum window to have already completed
    // on its own (it started before the surface/device bring-up above, which
    // itself is not-trivial synchronous work). Best-effort: a panicked thread
    // falls back to a synchronous `TextContext::new` with a log line — kill
    // nothing, defer nothing silently (never a crash/block).
    startup.record(SPAN_FONT_PREINIT_STARTED);
    let text_ctx = font_preinit.join().unwrap_or_else(|_| {
        log::warn!(
            "frust-shell-ios: font pre-init thread panicked; \
             falling back to synchronous TextContext::new"
        );
        TextContext::new()
    });
    startup.record(SPAN_FONT_PREINIT_JOINED);

    // Construct the app AND its handle under the root `Owner`. `make_app` runs
    // `Component::init` (via `new_boxed_app_with`'s state factory), and
    // `IosAppHandle::new` runs the initial `rebuild()` — both must see an
    // ambient `Owner` or `provide_context`/`on_cleanup` silently no-op. A root
    // component has no enclosing component to supply one, so it registers
    // against the root owner (process lifetime, never disposed), mirroring the
    // desktop shell's per-frame `with_owner` wrap and the facade `run()` init.
    // Retain `metal_layer` in the handle so a later `SurfaceLost` can be recovered
    // by recreating the surface from it (iOS never re-delivers the layer).
    let mut handle = rt.with_owner(|| {
        let app = make_app();
        IosAppHandle::new(
            render_cx,
            renderer,
            text_ctx,
            metal_layer,
            physical,
            scale,
            app,
        )
    });
    // `IosAppHandle::new` runs the app's first `rebuild()` synchronously as
    // its last step before returning, so recording the span here is
    // effectively the same instant as the rebuild's completion.
    startup.record(perf::SPAN_FIRST_REBUILD_DONE);
    // Stash for `render_frame` to complete once this handle actually
    // presents its first frame (see `IosAppHandle::latch_first_frame_presented`).
    handle.set_startup_spans(startup);

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
/// process killed); a loss during backgrounding recovers after `frust_resume`.
/// This is the sole recovery call into `on_surface_created_from_metal_layer`
/// outside [`create_handle`], and keeps the `unsafe` confined to this module.
fn recover_surface(app: &mut IosAppHandle, physical: (u32, u32), scale: f32) {
    // Read the retained pointer before taking the `&mut` borrow of the renderer.
    let metal_layer = app.metal_layer();
    let result = {
        let (render_cx, renderer) = app.renderer_mut();
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
            )
        })
    };
    match result {
        Ok(()) => app.set_surface(physical, scale),
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
/// `CAMetalLayer` survives, so a live surface is a plain in-place resize; a
/// `SurfaceLost` surface is instead recreated from the retained layer at the
/// incoming dimensions (self-recovery — see [`recover_surface`]).
pub fn resize(handle: *mut c_void, width: u32, height: u32, scale: f32) {
    guard("frust_resize", (), || {
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

/// `frust_render_frame`: run one `CADisplayLink`-driven frame (no-op unless the
/// surface is ready and the app is not paused). If the surface was lost, first
/// recreate it from the retained layer at the last-known size/scale (self-recovery
/// — see [`recover_surface`]) so a `SurfaceLost` is no longer a permanent black
/// screen.
///
/// `timestamp_ns` is the `CADisplayLink` tick's `timestamp` (`CFTimeInterval`
/// seconds), converted to nanoseconds by the Swift caller
/// (`UInt64(link.timestamp * 1_000_000_000)`) — the shell-owned monotonic
/// frame clock threaded into [`frust_core::FrameTime`] (spec §8).
pub fn render_frame(handle: *mut c_void, timestamp_ns: u64) {
    guard("frust_render_frame", (), || {
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
            // `frame` reports whether this tick actually presented a frame
            // (`FrameOutcome::Rendered`); the first-presented-frame startup
            // span (spec §14 phase 7.A task 09) is latched here, exactly
            // once, the first time it does — see
            // `IosAppHandle::latch_first_frame_presented`'s idempotency doc.
            if app.frame(timestamp_ns) {
                app.latch_first_frame_presented();
            }
        }
    });
}

/// `frust_dispatch_touch`: deliver one touch contact to the tree (spec §9).
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
/// mirror into the focused widget (the mobile state-sync path — spec §9 / Phase
/// 4B). Routed to the focused widget as an `ImeEvent::ApplyEditingState` via
/// [`AppTree::ime_apply`].
///
/// `text` is the mirror's UTF-8 bytes; `sel_*`/`comp_*` are **UTF-16 code-unit**
/// indices (the platform-native unit the `NSMutableString` mirror counts in),
/// passed opaquely through the [`EditingState`] shell seam — the widget /
/// `frust-text` converts them to Rust byte offsets at its own boundary (task
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
        // docs and task 08's design notes); drain any queued completions
        // immediately on foreground instead of waiting for the next
        // `frust_render_frame` tick.
        pump_reactive();
    });
}

/// `frust_set_appearance`: flip the app's theme brightness (a dark-mode
/// change reported via `traitCollectionDidChange`), re-publishing it through
/// both delivery paths (mirrors task 05's desktop `apply_theme`). `dark` is
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

/// `frust_set_insets`: deliver the platform's window insets (device-parity
/// task 06 — RESEARCH.md "Insets / SafeArea"). The eight `f32`s are two per-edge
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
/// `templates/app/ios.tmpl`'s `SceneDelegate`/`FrustViewController`
/// queue-until-handle-ready contract, task 07) into the process-wide
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

    fn test_logic(state: &mut TestState) -> impl frust_core::view::View<TestState> + use<> {
        state.n += 1;
        frust_widgets::text(format!("{}", state.n))
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
    ) -> impl frust_core::view::View<NonDefaultState> + use<> {
        state.n += 1;
        frust_widgets::text(format!("{}", state.n))
    }

    crate::ios_app!(NonDefaultState, init_state, test_logic);
}
