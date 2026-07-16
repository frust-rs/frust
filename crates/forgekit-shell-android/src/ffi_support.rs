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

#[cfg(test)]
mod tests {
    use super::*;

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
}
