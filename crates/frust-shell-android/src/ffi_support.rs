//! Android-specific FFI helper: the opaque-handle liveness check.
//!
//! The platform-agnostic plumbing (`guard`, `sanitize_scale`, `logical_size`,
//! the `AppTree` erasure) lives in `frust-shell-common` and is imported at
//! its call sites. What remains here is JNI-handle specific: the sentinel check
//! for the `jlong` the JVM passes back into every native call.
//!
//! This carries no `jni`/`ndk`/GPU dependency so it compiles and is unit-tested
//! on the host (`cargo test --workspace`), even though the code that calls it
//! ([`crate::jni_glue`]) is `#[cfg(target_os = "android")]`.

use std::path::{Path, PathBuf};

/// Whether an opaque handle from the JVM points at a live native side.
///
/// Kotlin initialises `handle` to `0` and every native call is guarded on it
/// (see `FrustSurfaceView`), so `0` means "no native side yet / already
/// destroyed" and must be treated as a no-op rather than dereferenced.
#[inline]
pub(crate) fn handle_is_live(handle: i64) -> bool {
    handle != 0
}

/// A pointer gesture phase, decoupled from `frust_core::PointerPhase` so this
/// module stays host-testable (the core crate is Android-gated — see the crate's
/// `Cargo.toml`). [`crate::app`] maps this onto the core phase at the one
/// Android-only call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TouchPhase {
    /// A finger touched down.
    Down,
    /// A finger moved while down.
    Move,
    /// A finger lifted.
    Up,
    /// The gesture was cancelled by the platform.
    Cancel,
}

/// Map the action code the Kotlin `FrustSurfaceView.onTouchEvent` sends into a
/// [`TouchPhase`].
///
/// The Kotlin side normalises `MotionEvent.actionMasked` into a fixed ABI —
/// `ACTION_DOWN`/`ACTION_POINTER_DOWN` → `0`, `ACTION_MOVE` → `1`,
/// `ACTION_UP`/`ACTION_POINTER_UP` → `2`, `ACTION_CANCEL` → `3` — so this side
/// never sees a raw Android constant. Any unrecognised code (a future action we
/// don't map, or a corrupt value) is treated as [`TouchPhase::Cancel`]: the safe
/// default, since it releases any in-flight capture rather than stranding a
/// gesture as perpetually "down".
#[inline]
pub(crate) fn touch_phase_from_action(action: i32) -> TouchPhase {
    match action {
        0 => TouchPhase::Down,
        1 => TouchPhase::Move,
        2 => TouchPhase::Up,
        // 3 is the explicit `ACTION_CANCEL`; everything else falls back to it.
        _ => TouchPhase::Cancel,
    }
}

/// Convert Kotlin's `Choreographer.FrameCallback` `frameTimeNanos` (a JNI
/// `jlong`, signed 64-bit) into the unsigned nanosecond count
/// [`frust_core::FrameTime::from_nanos`] takes.
///
/// `frameTimeNanos` is documented as monotonic and non-negative in practice,
/// but the JNI boundary is untrusted input: a negative value clamps to `0`
/// rather than silently wrapping into a huge `u64` via `as` (which would
/// corrupt every later `FrameTime::saturating_sub` in the same episode).
#[inline]
pub(crate) fn frame_time_nanos_from_jlong(frame_time_nanos: i64) -> u64 {
    frame_time_nanos.max(0) as u64
}

/// A light/dark appearance flag, decoupled from `frust_theme::Brightness` so
/// this module stays host-testable (that crate is Android-gated — see the
/// crate's `Cargo.toml`). [`crate::app`] maps this onto `Brightness` at the one
/// Android-only call site ([`crate::app::AndroidAppHandle::set_appearance`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Appearance {
    /// The platform reports a light appearance preference.
    Light,
    /// The platform reports a dark appearance preference.
    Dark,
}

/// Map the `nativeSetAppearance` JNI boolean (Kotlin's
/// `(resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
/// Configuration.UI_MODE_NIGHT_YES`) into an [`Appearance`] — the pure core of
/// the appearance state-machine transition.
#[inline]
pub(crate) fn appearance_from_dark(dark: bool) -> Appearance {
    if dark {
        Appearance::Dark
    } else {
        Appearance::Light
    }
}

/// The back-press consume decision: given the
/// framework's current [`handles_back`](frust_reactive::handles_back) answer,
/// whether the native `nativeOnBackPress` callback should consume this press
/// (routing it into the app via `push_back_press`) and, equivalently, what
/// `jboolean` it returns to the Kotlin `OnBackPressedDispatcher`.
///
/// The mapping is intentionally identity — the framework consumes exactly when
/// it has advertised (via `setFrameworkHandlesBack`) that it will — but it is
/// named and host-tested here rather than inlined at the JNI boundary so the
/// contract has a single documented, testable decision point (the same reason
/// [`appearance_from_dark`]/[`touch_phase_from_action`] live here). `true` =
/// consume + return `JNI_TRUE` (framework pops on next rebuild); `false` = let
/// the default dispatcher finish the activity.
#[inline]
pub(crate) fn should_consume_back_press(handles_back: bool) -> bool {
    handles_back
}

/// Normalise a raw JNI selection/composing index quintuple into the canonical
/// [`frust_core::event::EditingState`] field form (the mobile IME seam).
///
/// The indices arrive from Kotlin as **UTF-16 code units** (the Java-native unit
/// for `Editable`/`Selection`) and are passed through the `AppTree`/shell seam
/// *unchanged* — the seam itself is UTF-16 (see `EditingState`'s index-boundary
/// rule); the focused widget / `frust-text` converts them to Rust
/// byte offsets at its own boundary. This helper only canonicalises the "none"
/// sentinel: any out-of-domain negative (`< -1`, e.g. a corrupt platform value)
/// is clamped to the single `-1` marker, so a bad value can never be misread as a
/// large positive index. A valid `0` (caret at the start) is preserved.
///
/// Returned as `(selection_base, selection_extent, composing_base,
/// composing_extent)`, matching the `EditingState` field order.
#[inline]
pub(crate) fn normalize_ime_indices(
    sel_base: i32,
    sel_ext: i32,
    comp_base: i32,
    comp_ext: i32,
) -> (i32, i32, i32, i32) {
    #[inline]
    fn norm(i: i32) -> i32 {
        if i < 0 { -1 } else { i }
    }
    (
        norm(sel_base),
        norm(sel_ext),
        norm(comp_base),
        norm(comp_ext),
    )
}

/// A plain, `jni`/`frust-core`-free view of the IME surface, ready to be
/// serialised into the `nativeImeState` JSON the Kotlin side parses.
///
/// Kept free of any `jni`/`ndk`/`kurbo` type so it (and [`build_ime_state_json`])
/// compiles and is unit-tested on the host. [`crate::jni_glue`] maps a
/// [`frust_core::event::ImeState`] onto this at the one Android-only call site.
///
/// Indices are UTF-16 code units (see [`normalize_ime_indices`]); `caret` is the
/// logical-pixel caret rectangle `(x, y, width, height)` for future IME candidate
/// placement, absent when no caret was published.
///
/// `content_type` is a **stable, explicitly-encoded wire string** — one of
/// `"normal"` / `"password"` / `"noSuggestions"` — never a `Debug` rendering of
/// `frust_core::event::ImeContentType` (that type's `Debug` impl exists on
/// `ImeState`, not here, but the discipline is the same one that motivated it:
/// a secret field's classification must never ride on a format that could
/// silently change shape). [`crate::jni_glue`]'s `content_type_wire` maps the
/// enum to this string at the one Android-only call site — the same split
/// [`ImeJsonState`] itself exists for (`frust_core` is Android-gated in this
/// crate's `Cargo.toml`, so this host-testable type cannot name the enum
/// directly). This is the same wire shape the iOS bridge's `ime_state_json`
/// emits under `"contentType"`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ImeJsonState {
    pub active: bool,
    pub text: String,
    pub sel_base: i32,
    pub sel_ext: i32,
    pub comp_base: i32,
    pub comp_ext: i32,
    pub caret: Option<(f32, f32, f32, f32)>,
    pub content_type: &'static str,
}

impl Default for ImeJsonState {
    /// The "no focused editable" surface: inactive, empty, no selection/composing,
    /// `"normal"` content type (no hint).
    fn default() -> Self {
        Self {
            active: false,
            text: String::new(),
            sel_base: -1,
            sel_ext: -1,
            comp_base: -1,
            comp_ext: -1,
            caret: None,
            content_type: "normal",
        }
    }
}

/// Escape a Rust string for embedding inside a JSON double-quoted string literal.
///
/// Hand-rolled (the shell carries no `serde`/`json` dependency): escapes the two
/// structural characters (`"`, `\`), the named control escapes
/// (`\b`/`\f`/`\n`/`\r`/`\t`), and any other C0 control (`< 0x20`) as `\u00XX`.
/// Printable non-ASCII (any `char >= 0x20`) passes through verbatim as UTF-8,
/// which a JSON string permits.
pub(crate) fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Format an `f32` as a JSON-legal number, substituting a finite `0` for any
/// non-finite value (`NaN`/`±inf` have no JSON representation and would produce
/// an unparseable token).
fn json_number(v: f32) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "0".to_string()
    }
}

/// Serialise an [`ImeJsonState`] into the exact JSON wire form the Kotlin
/// `FrustSurfaceView` parses from `nativeImeState`.
///
/// Shape (indices UTF-16, caret logical px; absent caret ⇒ `null` components):
/// `{"active":bool,"text":"...","selBase":n,"selExt":n,"compBase":n,`
/// `"compExt":n,"caretX":f,"caretY":f,"caretW":f,"caretH":f,"contentType":"..."}`.
pub(crate) fn build_ime_state_json(state: &ImeJsonState) -> String {
    let (caret_x, caret_y, caret_w, caret_h) = match state.caret {
        Some((x, y, w, h)) => (
            json_number(x),
            json_number(y),
            json_number(w),
            json_number(h),
        ),
        None => (
            "null".to_string(),
            "null".to_string(),
            "null".to_string(),
            "null".to_string(),
        ),
    };
    format!(
        "{{\"active\":{},\"text\":\"{}\",\"selBase\":{},\"selExt\":{},\"compBase\":{},\"compExt\":{},\"caretX\":{},\"caretY\":{},\"caretW\":{},\"caretH\":{},\"contentType\":\"{}\"}}",
        state.active,
        json_escape(&state.text),
        state.sel_base,
        state.sel_ext,
        state.comp_base,
        state.comp_ext,
        caret_x,
        caret_y,
        caret_w,
        caret_h,
        json_escape(state.content_type),
    )
}

// ---------------------------------------------------------------------
// Platform-view command JSON: the host-testable
// core of `nativePlatformViewCommands`.
//
// `frust-shell-common` (which owns `ViewCommand`/`PlatformViewState`) is only
// a dependency of this crate under `cfg(target_os = "android")` (see this
// crate's `Cargo.toml`), so it cannot appear in a host-compiled signature —
// the same constraint `ImeJsonState` above exists to work around.
// `PlatformViewCommandJson` is the plain, dependency-free mirror of
// `frust_shell_common::ViewCommand`; `crate::jni_glue`'s
// `platform_view_commands_to_json` maps the real differ output onto this at
// the one Android-only call site, exactly like `ime_state_to_json` above.
// ---------------------------------------------------------------------

/// A plain, `frust-shell-common`/JNI-free view of one platform-view command,
/// ready to be serialised into the `nativePlatformViewCommands` JSON both
/// mobile shells serve, byte-identically (a frozen wire contract).
///
/// Rect/clip tuples are `(x, y, width, height)` **physical px** — already
/// scaled at the mapping site ([`crate::jni_glue`]'s
/// `platform_view_commands_to_json`), mirroring [`ImeJsonState`]'s caret rect
/// (the physical-at-FFI/logical-inside rule, applied outbound).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PlatformViewCommandJson {
    /// Mirrors `frust_shell_common::ViewCommand::Create`.
    Create {
        slot_id: u64,
        view_type: String,
        params_json: String,
        interactive: bool,
    },
    /// Mirrors `frust_shell_common::ViewCommand::Update`.
    Update {
        slot_id: u64,
        rect: (f32, f32, f32, f32),
        clip: Option<(f32, f32, f32, f32)>,
        visible: bool,
        shields: Vec<(f32, f32, f32, f32)>,
    },
    /// Mirrors `frust_shell_common::ViewCommand::UpdateParams`.
    UpdateParams { slot_id: u64, params_json: String },
    /// Mirrors `frust_shell_common::ViewCommand::Dispose`.
    Dispose { slot_id: u64 },
}

/// Whether `nativePlatformViewCommands`'s no-change fast path applies:
/// nothing has been produced since the generation Kotlin last acknowledged
/// (round-tripped back as `ack_generation`) — the differ's own
/// `acknowledge`-then-peek contract (`frust_shell_common::platform_view`'s
/// module docs) reduces to exactly this equality check once the ack has been
/// applied. A `true` here means `nativePlatformViewCommands` returns a null
/// `jstring` instead of a JSON payload, mirroring `nativeSystemUiState`'s
/// cheapness bar (a no-change poll costs one JNI call, no allocation).
#[inline]
pub(crate) fn platform_view_commands_up_to_date(generation: u64, ack_generation: u64) -> bool {
    generation == ack_generation
}

/// Format one `(x, y, width, height)` rect tuple as the JSON wire array
/// `[x,y,w,h]`, sanitising any non-finite component to `0` (mirrors
/// [`json_number`]'s NaN/inf guard — a rect crossing this boundary must
/// always be parseable JSON).
fn platform_view_rect_json((x, y, w, h): (f32, f32, f32, f32)) -> String {
    format!(
        "[{},{},{},{}]",
        json_number(x),
        json_number(y),
        json_number(w),
        json_number(h),
    )
}

/// Serialise one [`PlatformViewCommandJson`] into its JSON object — see
/// [`build_platform_view_commands_json`] for the full wire shape.
fn platform_view_command_json(cmd: &PlatformViewCommandJson) -> String {
    match cmd {
        PlatformViewCommandJson::Create {
            slot_id,
            view_type,
            params_json,
            interactive,
        } => format!(
            "{{\"op\":\"create\",\"slot\":{slot_id},\"viewType\":\"{}\",\"params\":\"{}\",\"interactive\":{interactive}}}",
            json_escape(view_type),
            json_escape(params_json),
        ),
        PlatformViewCommandJson::Update {
            slot_id,
            rect,
            clip,
            visible,
            shields,
        } => {
            let shields_json = shields
                .iter()
                .map(|s| platform_view_rect_json(*s))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{{\"op\":\"update\",\"slot\":{slot_id},\"rect\":{},\"clip\":{},\"visible\":{visible},\"shields\":[{shields_json}]}}",
                platform_view_rect_json(*rect),
                clip.map_or_else(|| "null".to_string(), platform_view_rect_json),
            )
        }
        PlatformViewCommandJson::UpdateParams {
            slot_id,
            params_json,
        } => format!(
            "{{\"op\":\"updateParams\",\"slot\":{slot_id},\"params\":\"{}\"}}",
            json_escape(params_json),
        ),
        PlatformViewCommandJson::Dispose { slot_id } => {
            format!("{{\"op\":\"dispose\",\"slot\":{slot_id}}}")
        }
    }
}

/// Serialise a whole not-yet-acknowledged platform-view command backlog into
/// the `nativePlatformViewCommands` JSON both mobile shells serve
/// byte-identically (a frozen contract; the iOS shell mirrors it exactly):
///
/// `{"generation":N,"commands":[{"op":"create","slot":N,"viewType":"...","params":"..."},`
/// `{"op":"update","slot":N,"rect":[x,y,w,h],"clip":[x,y,w,h]|null,"visible":bool},`
/// `{"op":"updateParams","slot":N,"params":"..."},{"op":"dispose","slot":N}]}`.
///
/// An empty `commands` slice still yields a valid (if pointless — the caller
/// should have taken the [`platform_view_commands_up_to_date`] fast path
/// instead) `{"generation":N,"commands":[]}`.
pub(crate) fn build_platform_view_commands_json(
    generation: u64,
    commands: &[PlatformViewCommandJson],
) -> String {
    let mut cmds = String::new();
    for (i, cmd) in commands.iter().enumerate() {
        if i > 0 {
            cmds.push(',');
        }
        cmds.push_str(&platform_view_command_json(cmd));
    }
    format!("{{\"generation\":{generation},\"commands\":[{cmds}]}}")
}

// ---------------------------------------------------------------------
// Pipeline-cache persistence: pure path / diff / IO helpers.
//
// The Android shell persists wgpu's `PipelineCache` blob across launches so
// second-and-later starts skip Vulkan shader-pipeline compilation (see
// `crate::jni_glue::create_handle`). These helpers are the host-testable core of
// that path — the on-disk location, the "did the blob change" check, and the
// atomic read/write. All I/O is best-effort: the caller logs-and-ignores every
// failure, since starting from an empty cache is always a correct fallback.
// ---------------------------------------------------------------------

/// The on-disk path of the persisted pipeline-cache blob:
/// `<cache_dir>/frust/pipeline_cache.bin`.
///
/// `cache_dir` is the app's `context.cacheDir.absolutePath`, delivered across the
/// JNI boundary by `nativeInit` (see `FrustSurfaceView.surfaceCreated`). The
/// `frust` subdirectory keeps the framework's file out of the app's own cache
/// namespace.
pub(crate) fn pipeline_cache_path(cache_dir: &str) -> PathBuf {
    Path::new(cache_dir)
        .join("frust")
        .join("pipeline_cache.bin")
}

/// Whether a freshly read cache blob differs from the one loaded at startup, so
/// an unchanged blob is never rewritten (once the cache stabilises across
/// launches, no disk churn). A `None` `loaded` (no prior blob on disk) always
/// differs from any `new` payload.
pub(crate) fn pipeline_cache_differs(loaded: Option<&[u8]>, new: &[u8]) -> bool {
    loaded != Some(new)
}

/// Read the persisted pipeline-cache blob, or `None` if it is absent or
/// unreadable. Best-effort: a missing/corrupt file is treated as a cold start
/// (the renderer starts from an empty cache), never an error.
pub(crate) fn load_pipeline_cache(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// Atomically persist `data` to `path`: create the parent directory, write to a
/// process-unique temp sibling, then rename it over `path`.
///
/// The rename is an atomic swap on the same filesystem, so a concurrent reader
/// (a parallel launch) never observes a half-written file. Returns the
/// underlying `io::Error` on any failure so the caller can log it; persistence
/// is best-effort and a failure only means the next launch pays the cold-compile
/// cost again.
pub(crate) fn write_pipeline_cache_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    frust_paths::atomic_write(path, data)
}

// ---------------------------------------------------------------------
// GPU pre-init join resolution: the adopt-vs-fallback decision.
//
// `JNI_OnLoad` spawns a background thread that builds a `RenderContext` and
// creates its device (see `crate::jni_glue`); `nativeInit` joins it. This is the
// pure, host-testable core of that join's outcome handling — kept free of any
// `jni`/GPU dependency (the real `RenderContext` is an Android-only dependency)
// by staying generic over the context type.
// ---------------------------------------------------------------------

/// Resolve a joined `JNI_OnLoad` GPU pre-init outcome into the context
/// `create_handle` uses. The `joined` argument encodes three cases the
/// caller has already flattened the thread-join into:
///
/// - `Some(Some(ctx))` — the pre-init thread finished and produced a ready GPU
///   context: **adopt it**.
/// - `Some(None)` — the thread ran but its device init failed: **fall back** to a
///   freshly built context via `fresh`.
/// - `None` — no pre-init was present (`JNI_OnLoad` never ran, or a prior
///   `nativeInit` already took the handle) or the thread panicked (the caller
///   maps a join panic to `None`): **fall back**.
///
/// This is the strictly best-effort, single-shot contract the task requires — a
/// panicked or absent pre-init is never fatal, it just costs this launch the
/// synchronous GPU-init path it always had.
pub(crate) fn resolve_preinit<T>(joined: Option<Option<T>>, fresh: impl FnOnce() -> T) -> T {
    match joined {
        Some(Some(ctx)) => ctx,
        _ => fresh(),
    }
}

/// Publish a surface (re)install's **resolved** translucency onto the shared,
/// cross-thread flag the UI thread reads each frame.
///
/// `resolved` is `Some(translucent)` for a successful install — the value
/// `frust_render::SurfaceRenderer::surface_resolved_translucent` reports for
/// the surface that just went live — and `None` for a FAILED one, which stores
/// `false`: with no surface whose alpha mode we can vouch for, the safe
/// contract is the opaque Mode A one (never punch a hole you can't prove is a
/// window), and leaving the previous surface's value standing would be exactly
/// the stale-truth bug this seam exists to remove.
///
/// `Release` pairs with [`read_resolved_translucency`]'s `Acquire` so the UI
/// thread observing a store also observes everything the render thread did
/// before it.
#[inline]
pub(crate) fn publish_resolved_translucency(
    flag: &std::sync::atomic::AtomicBool,
    resolved: Option<bool>,
) {
    flag.store(
        resolved.unwrap_or(false),
        std::sync::atomic::Ordering::Release,
    );
}

/// The UI-thread half of [`publish_resolved_translucency`]: read the live
/// surface's resolved translucency before choosing this frame's base color and
/// pushing `RenderRoot::set_surface_translucent`.
///
/// Until the render thread's first install lands, this reads the
/// construction-time seed — the app's REQUEST — so the capable common case
/// renders Mode B from frame 1; a fallback downgrades it within one frame of
/// the install (the one-optimistic-frame window, documented at the field).
#[inline]
pub(crate) fn read_resolved_translucency(flag: &std::sync::atomic::AtomicBool) -> bool {
    flag.load(std::sync::atomic::Ordering::Acquire)
}

/// This frame's base clear color, chosen from the surface's **resolved**
/// translucency.
///
/// A surface that really came up translucent clears to `transparent` so a
/// native sibling view behind it shows through wherever nothing painted (Mode
/// B); everything else — including a translucency request the platform
/// refused — clears to the theme's opaque surface color, exactly as before
/// platform views existed.
///
/// Generic over the color type purely so this decision stays host-testable:
/// `peniko` is an Android-gated dependency of this crate, and this module
/// compiles on every host (see the module docs).
#[inline]
pub(crate) fn base_clear_color<C>(
    resolved_translucent: bool,
    transparent: C,
    opaque_surface: C,
) -> C {
    if resolved_translucent {
        transparent
    } else {
        opaque_surface
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn resolved_translucency_handoff_survives_a_fake_install_sequence() {
        // The Arc<AtomicBool> handoff the render-thread split runs on, driven
        // here with FAKE installs so it is host-testable (the real
        // `install_surface` needs a GPU and an `ANativeWindow`).
        //
        // Seeded from the REQUEST: the app asked for translucency, so frame 1
        // renders Mode B optimistically rather than flashing opaque while the
        // render thread is still creating the surface.
        let flag = AtomicBool::new(true);
        assert!(read_resolved_translucency(&flag));

        // A real resolution can only ever downgrade it: this device advertised
        // no translucent alpha mode, so the surface came up opaque.
        publish_resolved_translucency(&flag, Some(false));
        assert!(
            !read_resolved_translucency(&flag),
            "the UI thread must observe a render-thread downgrade"
        );
        assert_eq!(
            base_clear_color(read_resolved_translucency(&flag), "TRANSPARENT", "surface"),
            "surface",
            "a fallback-to-opaque surface keeps the opaque theme base color"
        );

        // A capable reinstall (e.g. after a rotation) restores Mode B.
        publish_resolved_translucency(&flag, Some(true));
        assert!(read_resolved_translucency(&flag));
        assert_eq!(
            base_clear_color(read_resolved_translucency(&flag), "TRANSPARENT", "surface"),
            "TRANSPARENT"
        );

        // A FAILED reinstall clears it rather than leaving the previous
        // surface's `true` standing.
        publish_resolved_translucency(&flag, None);
        assert!(!read_resolved_translucency(&flag));
    }

    #[test]
    fn resolve_preinit_adopts_a_ready_context() {
        // Pre-init succeeded: the pre-built context is adopted, `fresh` unused.
        let ctx = resolve_preinit(Some(Some(42)), || panic!("fresh must not run"));
        assert_eq!(ctx, 42);
    }

    #[test]
    fn resolve_preinit_falls_back_when_device_init_failed() {
        // `Some(None)`: the thread ran but device init failed on it (the
        // "poisoned result" case) — build fresh synchronously.
        let ctx = resolve_preinit(Some(None), || 7);
        assert_eq!(ctx, 7);
    }

    #[test]
    fn resolve_preinit_falls_back_when_absent_or_panicked() {
        // `None`: no pre-init spawned / already taken, or the thread panicked
        // (mapped to `None` by the caller) — both take the fresh path.
        let ctx = resolve_preinit(None, || 9);
        assert_eq!(ctx, 9);
    }

    #[test]
    fn zero_handle_is_not_live() {
        assert!(!handle_is_live(0));
    }

    #[test]
    fn nonzero_handle_is_live() {
        assert!(handle_is_live(1));
        assert!(handle_is_live(-1));
        assert!(handle_is_live(0x7fff_ffff_ffff_ffff));
    }

    #[test]
    fn touch_action_codes_map_to_phases() {
        assert_eq!(touch_phase_from_action(0), TouchPhase::Down);
        assert_eq!(touch_phase_from_action(1), TouchPhase::Move);
        assert_eq!(touch_phase_from_action(2), TouchPhase::Up);
        assert_eq!(touch_phase_from_action(3), TouchPhase::Cancel);
    }

    #[test]
    fn unknown_touch_action_falls_back_to_cancel() {
        // A future/corrupt code must release capture, not strand a "down".
        assert_eq!(touch_phase_from_action(4), TouchPhase::Cancel);
        assert_eq!(touch_phase_from_action(-1), TouchPhase::Cancel);
        assert_eq!(touch_phase_from_action(i32::MAX), TouchPhase::Cancel);
    }

    #[test]
    fn frame_time_nanos_passes_positive_values_through() {
        assert_eq!(frame_time_nanos_from_jlong(0), 0);
        assert_eq!(frame_time_nanos_from_jlong(1), 1);
        assert_eq!(frame_time_nanos_from_jlong(i64::MAX), i64::MAX as u64);
    }

    #[test]
    fn frame_time_nanos_clamps_negative_to_zero() {
        // A corrupt/negative `jlong` must clamp to `0`, never wrap to a huge
        // `u64` via `as`.
        assert_eq!(frame_time_nanos_from_jlong(-1), 0);
        assert_eq!(frame_time_nanos_from_jlong(i64::MIN), 0);
    }

    #[test]
    fn appearance_from_dark_maps_true_to_dark() {
        // init light -> set dark -> the transition Rust's brightness flip applies.
        assert_eq!(appearance_from_dark(false), Appearance::Light);
        assert_eq!(appearance_from_dark(true), Appearance::Dark);
    }

    #[test]
    fn back_press_consume_mirrors_handles_back() {
        // The framework consumes a back press exactly when it advertised it
        // would (`handles_back`); otherwise the platform dispatcher exits.
        assert!(
            should_consume_back_press(true),
            "handles_back=true -> consume + return JNI_TRUE (framework pops)"
        );
        assert!(
            !should_consume_back_press(false),
            "handles_back=false -> return JNI_FALSE (activity finishes)"
        );
    }

    #[test]
    fn ime_indices_pass_valid_values_through_unchanged() {
        // UTF-16 units cross the seam unchanged (incl. a valid 0 caret).
        assert_eq!(normalize_ime_indices(0, 0, -1, -1), (0, 0, -1, -1));
        assert_eq!(normalize_ime_indices(3, 5, 3, 5), (3, 5, 3, 5));
        assert_eq!(normalize_ime_indices(0, 12, -1, -1), (0, 12, -1, -1));
    }

    #[test]
    fn ime_indices_clamp_out_of_domain_negatives_to_none() {
        // Any `< -1` (corrupt platform value) collapses to the `-1` sentinel so
        // it can never be misread as a large positive index.
        assert_eq!(normalize_ime_indices(-1, -1, -1, -1), (-1, -1, -1, -1));
        assert_eq!(
            normalize_ime_indices(-2, -100, i32::MIN, -3),
            (-1, -1, -1, -1)
        );
        assert_eq!(normalize_ime_indices(-5, 4, -1, -9), (-1, 4, -1, -1));
    }

    #[test]
    fn json_escape_handles_quotes_and_backslashes() {
        assert_eq!(json_escape(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(json_escape(r"a\b"), r"a\\b");
        assert_eq!(json_escape("tab\tnew\nline"), "tab\\tnew\\nline");
        assert_eq!(json_escape("\u{08}\u{0c}\r"), "\\b\\f\\r");
    }

    #[test]
    fn json_escape_emits_control_chars_as_unicode_escapes() {
        // A bare C0 control (NUL, unit separator) must become `\u00XX`.
        assert_eq!(json_escape("\u{00}"), "\\u0000");
        assert_eq!(json_escape("\u{1f}"), "\\u001f");
    }

    #[test]
    fn json_escape_passes_printable_unicode_through() {
        // Multi-byte UTF-8 (accent, CJK, astral emoji) is valid inside a JSON
        // string and must not be escaped.
        assert_eq!(json_escape("café 日本語 😀"), "café 日本語 😀");
    }

    #[test]
    fn build_ime_state_json_with_caret() {
        let state = ImeJsonState {
            active: true,
            text: "hi".to_string(),
            sel_base: 2,
            sel_ext: 2,
            comp_base: -1,
            comp_ext: -1,
            caret: Some((1.5, 2.0, 0.0, 10.0)),
            content_type: "normal",
        };
        assert_eq!(
            build_ime_state_json(&state),
            "{\"active\":true,\"text\":\"hi\",\"selBase\":2,\"selExt\":2,\"compBase\":-1,\"compExt\":-1,\"caretX\":1.5,\"caretY\":2,\"caretW\":0,\"caretH\":10,\"contentType\":\"normal\"}"
        );
    }

    #[test]
    fn build_ime_state_json_without_caret_emits_nulls_and_escapes_text() {
        let state = ImeJsonState {
            active: false,
            text: "a\"b".to_string(),
            sel_base: -1,
            sel_ext: -1,
            comp_base: -1,
            comp_ext: -1,
            caret: None,
            content_type: "normal",
        };
        assert_eq!(
            build_ime_state_json(&state),
            "{\"active\":false,\"text\":\"a\\\"b\",\"selBase\":-1,\"selExt\":-1,\"compBase\":-1,\"compExt\":-1,\"caretX\":null,\"caretY\":null,\"caretW\":null,\"caretH\":null,\"contentType\":\"normal\"}"
        );
    }

    #[test]
    fn build_ime_state_json_default_is_inactive_empty() {
        assert_eq!(
            build_ime_state_json(&ImeJsonState::default()),
            "{\"active\":false,\"text\":\"\",\"selBase\":-1,\"selExt\":-1,\"compBase\":-1,\"compExt\":-1,\"caretX\":null,\"caretY\":null,\"caretW\":null,\"caretH\":null,\"contentType\":\"normal\"}"
        );
    }

    #[test]
    fn build_ime_state_json_sanitizes_non_finite_caret() {
        let state = ImeJsonState {
            caret: Some((f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 4.0)),
            ..ImeJsonState::default()
        };
        // Non-finite caret components must not emit an unparseable JSON token.
        let json = build_ime_state_json(&state);
        assert!(
            json.contains("\"caretX\":0,\"caretY\":0,\"caretW\":0,\"caretH\":4"),
            "{json}"
        );
        assert!(!json.contains("NaN") && !json.contains("inf"), "{json}");
    }

    #[test]
    fn build_ime_state_json_encodes_content_type_password() {
        let state = ImeJsonState {
            content_type: "password",
            ..ImeJsonState::default()
        };
        assert!(
            build_ime_state_json(&state).contains("\"contentType\":\"password\""),
            "password content type must ride the JSON DTO explicitly, not via Debug"
        );
    }

    #[test]
    fn build_ime_state_json_encodes_content_type_no_suggestions() {
        let state = ImeJsonState {
            content_type: "noSuggestions",
            ..ImeJsonState::default()
        };
        assert!(build_ime_state_json(&state).contains("\"contentType\":\"noSuggestions\""));
    }

    #[test]
    fn build_ime_state_json_encodes_content_type_normal() {
        let state = ImeJsonState {
            content_type: "normal",
            ..ImeJsonState::default()
        };
        assert!(build_ime_state_json(&state).contains("\"contentType\":\"normal\""));
    }

    // -----------------------------------------------------------------
    // Platform-view command JSON
    // -----------------------------------------------------------------

    #[test]
    fn platform_view_json_create_golden() {
        let json = build_platform_view_commands_json(
            1,
            &[PlatformViewCommandJson::Create {
                slot_id: 3,
                view_type: "dev.frust.MapFactory".to_string(),
                params_json: "{\"style\":\"dark\"}".to_string(),
                interactive: false,
            }],
        );
        assert_eq!(
            json,
            "{\"generation\":1,\"commands\":[{\"op\":\"create\",\"slot\":3,\"viewType\":\"dev.frust.MapFactory\",\"params\":\"{\\\"style\\\":\\\"dark\\\"}\",\"interactive\":false}]}"
        );
    }

    #[test]
    fn platform_view_json_update_golden_with_clip() {
        let json = build_platform_view_commands_json(
            2,
            &[PlatformViewCommandJson::Update {
                slot_id: 3,
                rect: (0.0, 10.0, 100.0, 200.0),
                clip: Some((0.0, 10.0, 50.0, 200.0)),
                visible: true,
                shields: Vec::new(),
            }],
        );
        assert_eq!(
            json,
            "{\"generation\":2,\"commands\":[{\"op\":\"update\",\"slot\":3,\"rect\":[0,10,100,200],\"clip\":[0,10,50,200],\"visible\":true,\"shields\":[]}]}"
        );
    }

    #[test]
    fn platform_view_json_update_golden_without_clip_is_null() {
        let json = build_platform_view_commands_json(
            3,
            &[PlatformViewCommandJson::Update {
                slot_id: 1,
                rect: (0.0, 0.0, 10.0, 10.0),
                clip: None,
                visible: false,
                shields: Vec::new(),
            }],
        );
        assert_eq!(
            json,
            "{\"generation\":3,\"commands\":[{\"op\":\"update\",\"slot\":1,\"rect\":[0,0,10,10],\"clip\":null,\"visible\":false,\"shields\":[]}]}"
        );
    }

    #[test]
    fn platform_view_json_update_params_golden() {
        let json = build_platform_view_commands_json(
            4,
            &[PlatformViewCommandJson::UpdateParams {
                slot_id: 5,
                params_json: "{\"a\":2}".to_string(),
            }],
        );
        assert_eq!(
            json,
            "{\"generation\":4,\"commands\":[{\"op\":\"updateParams\",\"slot\":5,\"params\":\"{\\\"a\\\":2}\"}]}"
        );
    }

    #[test]
    fn platform_view_json_dispose_golden() {
        let json = build_platform_view_commands_json(
            5,
            &[PlatformViewCommandJson::Dispose { slot_id: 3 }],
        );
        assert_eq!(
            json,
            "{\"generation\":5,\"commands\":[{\"op\":\"dispose\",\"slot\":3}]}"
        );
    }

    #[test]
    fn platform_view_json_escapes_quotes_and_backslashes_in_view_type_and_params() {
        let json = build_platform_view_commands_json(
            1,
            &[PlatformViewCommandJson::Create {
                slot_id: 1,
                view_type: "dev.frust.\"Weird\"Factory".to_string(),
                params_json: r"a\b".to_string(),
                interactive: false,
            }],
        );
        assert!(
            json.contains(r#""viewType":"dev.frust.\"Weird\"Factory""#),
            "{json}"
        );
        assert!(json.contains(r#""params":"a\\b""#), "{json}");
    }

    #[test]
    fn platform_view_json_multiple_commands_in_one_batch() {
        let json = build_platform_view_commands_json(
            7,
            &[
                PlatformViewCommandJson::Create {
                    slot_id: 3,
                    view_type: "dev.frust.XFactory".to_string(),
                    params_json: String::new(),
                    interactive: false,
                },
                PlatformViewCommandJson::Update {
                    slot_id: 3,
                    rect: (0.0, 0.0, 10.0, 10.0),
                    clip: None,
                    visible: true,
                    shields: Vec::new(),
                },
            ],
        );
        assert_eq!(
            json,
            "{\"generation\":7,\"commands\":[{\"op\":\"create\",\"slot\":3,\"viewType\":\"dev.frust.XFactory\",\"params\":\"\",\"interactive\":false},{\"op\":\"update\",\"slot\":3,\"rect\":[0,0,10,10],\"clip\":null,\"visible\":true,\"shields\":[]}]}"
        );
    }

    #[test]
    fn platform_view_json_empty_backlog_is_a_valid_empty_array() {
        assert_eq!(
            build_platform_view_commands_json(0, &[]),
            "{\"generation\":0,\"commands\":[]}"
        );
    }

    #[test]
    fn platform_view_rect_json_sanitizes_non_finite_components() {
        let json = build_platform_view_commands_json(
            1,
            &[PlatformViewCommandJson::Update {
                slot_id: 1,
                rect: (f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 4.0),
                clip: None,
                visible: true,
                shields: Vec::new(),
            }],
        );
        assert!(json.contains("\"rect\":[0,0,0,4]"), "{json}");
        assert!(!json.contains("NaN") && !json.contains("inf"), "{json}");
    }

    #[test]
    fn platform_view_up_to_date_is_generation_equality() {
        assert!(platform_view_commands_up_to_date(0, 0));
        assert!(platform_view_commands_up_to_date(5, 5));
        assert!(!platform_view_commands_up_to_date(6, 5));
        assert!(!platform_view_commands_up_to_date(5, 6));
    }

    /// The "ack compaction round-trip" host-tested at this crate's
    /// boundary: `frust_shell_common::PlatformViewState`'s own compaction
    /// logic is exhaustively unit-tested in that (non-target-gated) crate
    /// (`acknowledge_compacts_the_backlog` and friends) — it isn't
    /// reachable from this crate's host build at all, since
    /// `frust-shell-common` is only a dependency of `frust-shell-android`
    /// under `cfg(target_os = "android")` (this crate's `Cargo.toml`). What
    /// IS host-testable here is the JSON layer's half of the round trip:
    /// Kotlin polls generation N, gets a payload; it round-trips N back as
    /// `ack_generation`; a second poll with nothing new must take the
    /// no-change fast path rather than re-serializing an (empty) payload.
    #[test]
    fn platform_view_ack_round_trip_at_the_json_layer() {
        let (generation, commands) = (
            3u64,
            vec![PlatformViewCommandJson::Create {
                slot_id: 1,
                view_type: "dev.frust.X".to_string(),
                params_json: String::new(),
                interactive: false,
            }],
        );
        // First poll: Kotlin's ack_generation starts at 0 (nothing seen yet)
        // — not up to date, so it gets the real JSON payload.
        assert!(!platform_view_commands_up_to_date(generation, 0));
        let first_json = build_platform_view_commands_json(generation, &commands);
        assert!(first_json.contains("\"op\":\"create\""));

        // Kotlin round-trips `generation` back as its next `ack_generation`;
        // nothing new was produced in between (the differ's own generation
        // is still 3) — this must be the no-change fast path (null jstring
        // at the JNI layer, tested here as the boolean the export branches
        // on).
        assert!(platform_view_commands_up_to_date(generation, generation));
    }

    // -----------------------------------------------------------------
    // Pipeline-cache persistence helpers
    // -----------------------------------------------------------------

    /// A fresh, unique temp directory for a round-trip I/O test. Not cleaned up
    /// eagerly — the OS reaps the temp dir; each test uses a distinct path so
    /// parallel runs never collide.
    fn unique_temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("frust-pcache-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn pipeline_cache_path_is_under_frust_subdir() {
        let path = pipeline_cache_path("/data/user/0/it.f0x.demo/cache");
        assert!(path.ends_with("frust/pipeline_cache.bin"), "{path:?}");
        assert_eq!(
            path,
            Path::new("/data/user/0/it.f0x.demo/cache/frust/pipeline_cache.bin")
        );
    }

    #[test]
    fn pipeline_cache_differs_reports_changes() {
        // No prior blob always differs from any new payload.
        assert!(pipeline_cache_differs(None, b"anything"));
        assert!(pipeline_cache_differs(None, b""));
        // Identical bytes do not differ (skip the rewrite).
        assert!(!pipeline_cache_differs(Some(b"same"), b"same"));
        assert!(!pipeline_cache_differs(Some(b""), b""));
        // Any content or length change differs.
        assert!(pipeline_cache_differs(Some(b"old"), b"new"));
        assert!(pipeline_cache_differs(Some(b"short"), b"shorter-plus"));
    }

    #[test]
    fn load_missing_pipeline_cache_returns_none() {
        let dir = unique_temp_dir();
        let path = dir.join("frust").join("pipeline_cache.bin");
        assert_eq!(load_pipeline_cache(&path), None);
    }

    #[test]
    fn write_then_load_pipeline_cache_round_trips() {
        let dir = unique_temp_dir();
        // The parent `frust/` dir does not exist yet — the write must create it.
        let path = dir.join("frust").join("pipeline_cache.bin");
        let blob = vec![0u8, 1, 2, 3, 250, 251, 252, 253];
        write_pipeline_cache_atomic(&path, &blob).unwrap();
        assert!(path.exists(), "atomic write must create the target file");
        assert_eq!(load_pipeline_cache(&path), Some(blob));
    }

    #[test]
    fn write_pipeline_cache_overwrites_existing() {
        let dir = unique_temp_dir();
        let path = dir.join("frust").join("pipeline_cache.bin");
        write_pipeline_cache_atomic(&path, b"first version").unwrap();
        write_pipeline_cache_atomic(&path, b"second").unwrap();
        assert_eq!(load_pipeline_cache(&path), Some(b"second".to_vec()));
        // The temp sibling must not linger after a successful rename.
        // frust_paths::atomic_write uses extension format: `.tmp.<pid>`
        let leftover = dir
            .join("frust")
            .join(format!("pipeline_cache.tmp.{}", std::process::id()));
        assert!(!leftover.exists(), "temp file should be renamed away");
    }
}
