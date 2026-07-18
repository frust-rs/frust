//! Android-specific FFI helper: the opaque-handle liveness check.
//!
//! The platform-agnostic plumbing (`guard`, `sanitize_scale`, `logical_size`,
//! the `AppTree` erasure) lives in `forgekit-shell-common` and is imported at
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
/// (see `ForgeKitSurfaceView`), so `0` means "no native side yet / already
/// destroyed" and must be treated as a no-op rather than dereferenced.
#[inline]
pub(crate) fn handle_is_live(handle: i64) -> bool {
    handle != 0
}

/// A pointer gesture phase, decoupled from `forgekit_core::PointerPhase` so this
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

/// Map the action code the Kotlin `ForgeKitSurfaceView.onTouchEvent` sends into a
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
/// [`forgekit_core::FrameTime::from_nanos`] takes.
///
/// `frameTimeNanos` is documented as monotonic and non-negative in practice,
/// but the JNI boundary is untrusted input: a negative value clamps to `0`
/// rather than silently wrapping into a huge `u64` via `as` (which would
/// corrupt every later `FrameTime::saturating_sub` in the same episode).
#[inline]
pub(crate) fn frame_time_nanos_from_jlong(frame_time_nanos: i64) -> u64 {
    frame_time_nanos.max(0) as u64
}

/// A light/dark appearance flag, decoupled from `forgekit_theme::Brightness` so
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
/// the appearance state-machine transition task 08 wires up.
#[inline]
pub(crate) fn appearance_from_dark(dark: bool) -> Appearance {
    if dark {
        Appearance::Dark
    } else {
        Appearance::Light
    }
}

/// Normalise a raw JNI selection/composing index quintuple into the canonical
/// [`forgekit_core::event::EditingState`] field form (the mobile IME seam).
///
/// The indices arrive from Kotlin as **UTF-16 code units** (the Java-native unit
/// for `Editable`/`Selection`) and are passed through the `AppTree`/shell seam
/// *unchanged* — the seam itself is UTF-16 (see `EditingState`'s index-boundary
/// rule); the focused widget / `forgekit-text` (task 52) converts them to Rust
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

/// A plain, `jni`/`forgekit-core`-free view of the IME surface, ready to be
/// serialised into the `nativeImeState` JSON the Kotlin side parses.
///
/// Kept free of any `jni`/`ndk`/`kurbo` type so it (and [`build_ime_state_json`])
/// compiles and is unit-tested on the host. [`crate::jni_glue`] maps a
/// [`forgekit_core::event::ImeState`] onto this at the one Android-only call site.
///
/// Indices are UTF-16 code units (see [`normalize_ime_indices`]); `caret` is the
/// logical-pixel caret rectangle `(x, y, width, height)` for future IME candidate
/// placement, absent when no caret was published.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ImeJsonState {
    pub active: bool,
    pub text: String,
    pub sel_base: i32,
    pub sel_ext: i32,
    pub comp_base: i32,
    pub comp_ext: i32,
    pub caret: Option<(f32, f32, f32, f32)>,
}

impl Default for ImeJsonState {
    /// The "no focused editable" surface: inactive, empty, no selection/composing.
    fn default() -> Self {
        Self {
            active: false,
            text: String::new(),
            sel_base: -1,
            sel_ext: -1,
            comp_base: -1,
            comp_ext: -1,
            caret: None,
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
/// `ForgeKitSurfaceView` parses from `nativeImeState`.
///
/// Shape (indices UTF-16, caret logical px; absent caret ⇒ `null` components):
/// `{"active":bool,"text":"...","selBase":n,"selExt":n,"compBase":n,`
/// `"compExt":n,"caretX":f,"caretY":f,"caretW":f,"caretH":f}`.
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
        "{{\"active\":{},\"text\":\"{}\",\"selBase\":{},\"selExt\":{},\"compBase\":{},\"compExt\":{},\"caretX\":{},\"caretY\":{},\"caretW\":{},\"caretH\":{}}}",
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
    )
}

// ---------------------------------------------------------------------
// Pipeline-cache persistence (task 13): pure path / diff / IO helpers.
//
// The Android shell persists wgpu's `PipelineCache` blob across launches so
// second-and-later starts skip Vulkan shader-pipeline compilation (see
// `crate::jni_glue::create_handle`). These helpers are the host-testable core of
// that path — the on-disk location, the "did the blob change" check, and the
// atomic read/write. All I/O is best-effort: the caller logs-and-ignores every
// failure, since starting from an empty cache is always a correct fallback.
// ---------------------------------------------------------------------

/// The on-disk path of the persisted pipeline-cache blob:
/// `<cache_dir>/forgekit/pipeline_cache.bin`.
///
/// `cache_dir` is the app's `context.cacheDir.absolutePath`, delivered across the
/// JNI boundary by `nativeInit` (see `ForgeKitSurfaceView.surfaceCreated`). The
/// `forgekit` subdirectory keeps the framework's file out of the app's own cache
/// namespace.
pub(crate) fn pipeline_cache_path(cache_dir: &str) -> PathBuf {
    Path::new(cache_dir)
        .join("forgekit")
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!("pipeline_cache.bin.{}.tmp", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

// ---------------------------------------------------------------------
// GPU pre-init join resolution (task 19): the adopt-vs-fallback decision.
//
// `JNI_OnLoad` spawns a background thread that builds a `RenderContext` and
// creates its device (see `crate::jni_glue`); `nativeInit` joins it. This is the
// pure, host-testable core of that join's outcome handling — kept free of any
// `jni`/GPU dependency (the real `RenderContext` is an Android-only dependency)
// by staying generic over the context type.
// ---------------------------------------------------------------------

/// Resolve a joined `JNI_OnLoad` GPU pre-init outcome into the context
/// `create_handle` uses (task 19). The `joined` argument encodes three cases the
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

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        assert_eq!(
            build_ime_state_json(&state),
            "{\"active\":true,\"text\":\"hi\",\"selBase\":2,\"selExt\":2,\"compBase\":-1,\"compExt\":-1,\"caretX\":1.5,\"caretY\":2,\"caretW\":0,\"caretH\":10}"
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
        };
        assert_eq!(
            build_ime_state_json(&state),
            "{\"active\":false,\"text\":\"a\\\"b\",\"selBase\":-1,\"selExt\":-1,\"compBase\":-1,\"compExt\":-1,\"caretX\":null,\"caretY\":null,\"caretW\":null,\"caretH\":null}"
        );
    }

    #[test]
    fn build_ime_state_json_default_is_inactive_empty() {
        assert_eq!(
            build_ime_state_json(&ImeJsonState::default()),
            "{\"active\":false,\"text\":\"\",\"selBase\":-1,\"selExt\":-1,\"compBase\":-1,\"compExt\":-1,\"caretX\":null,\"caretY\":null,\"caretW\":null,\"caretH\":null}"
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

    // -----------------------------------------------------------------
    // Pipeline-cache persistence helpers (task 13)
    // -----------------------------------------------------------------

    /// A fresh, unique temp directory for a round-trip I/O test. Not cleaned up
    /// eagerly — the OS reaps the temp dir; each test uses a distinct path so
    /// parallel runs never collide.
    fn unique_temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("forgekit-pcache-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn pipeline_cache_path_is_under_forgekit_subdir() {
        let path = pipeline_cache_path("/data/user/0/it.f0x.demo/cache");
        assert!(path.ends_with("forgekit/pipeline_cache.bin"), "{path:?}");
        assert_eq!(
            path,
            Path::new("/data/user/0/it.f0x.demo/cache/forgekit/pipeline_cache.bin")
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
        let path = dir.join("forgekit").join("pipeline_cache.bin");
        assert_eq!(load_pipeline_cache(&path), None);
    }

    #[test]
    fn write_then_load_pipeline_cache_round_trips() {
        let dir = unique_temp_dir();
        // The parent `forgekit/` dir does not exist yet — the write must create it.
        let path = dir.join("forgekit").join("pipeline_cache.bin");
        let blob = vec![0u8, 1, 2, 3, 250, 251, 252, 253];
        write_pipeline_cache_atomic(&path, &blob).unwrap();
        assert!(path.exists(), "atomic write must create the target file");
        assert_eq!(load_pipeline_cache(&path), Some(blob));
    }

    #[test]
    fn write_pipeline_cache_overwrites_existing() {
        let dir = unique_temp_dir();
        let path = dir.join("forgekit").join("pipeline_cache.bin");
        write_pipeline_cache_atomic(&path, b"first version").unwrap();
        write_pipeline_cache_atomic(&path, b"second").unwrap();
        assert_eq!(load_pipeline_cache(&path), Some(b"second".to_vec()));
        // The temp sibling must not linger after a successful rename.
        let leftover = dir
            .join("forgekit")
            .join(format!("pipeline_cache.bin.{}.tmp", std::process::id()));
        assert!(!leftover.exists(), "temp file should be renamed away");
    }
}
