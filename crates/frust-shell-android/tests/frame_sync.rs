//! Per-frame JNI state sync contract, checked on the host.
//!
//! `nativeFrameSync` returns one packed word per `doFrame` so the Kotlin side
//! makes the IME, clipboard, system-UI and brightness calls only for the bits
//! that are set. The latch behind it is unit-tested in `ffi_support`; this
//! file pins the pieces a host build cannot reach, on the source (source-scan
//! precedent: `tests/thread_policy.rs`): the export is declared additively in
//! `lib.rs`, its body lives in `jni_glue.rs`, the existing exports it gates
//! survive unchanged, and `doFrame` calls it once and gates the polls on its
//! bits. The bit layout is pinned so Kotlin and Rust cannot drift apart.

const LIB: &str = include_str!("../src/lib.rs");
const GLUE: &str = include_str!("../src/jni_glue.rs");
const SUPPORT: &str = include_str!("../src/ffi_support.rs");
const KOTLIN: &str = include_str!(
    "../platform/android/frust-embedding/src/main/kotlin/dev/frust/FrustSurfaceView.kt"
);

/// `source` with every comment-only line dropped.
fn code(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The text from the first occurrence of `signature` to the end of the brace
/// block that follows it.
fn block<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` not found"));
    let open = start + source[start..].find('{').expect("block opens");
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..open + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated block after `{signature}`");
}

#[test]
fn the_bit_layout_is_pinned_on_both_sides() {
    let layout = [
        ("FRAME_SYNC_IME_CHANGED", 0),
        ("FRAME_SYNC_CLIPBOARD_WRITE", 1),
        ("FRAME_SYNC_PASTE_REQUEST", 2),
        ("FRAME_SYNC_SYSTEM_UI_CHANGED", 3),
        ("FRAME_SYNC_DARK_CHANGED", 4),
    ];
    let rust = code(SUPPORT);
    let kotlin = code(KOTLIN);
    for (name, bit) in layout {
        assert!(
            rust.contains(&format!("const {name}: i64 = 1 << {bit};")),
            "{name} must be bit {bit} in ffi_support.rs"
        );
        assert!(
            kotlin.contains(&format!("const val {name} = 1L shl {bit}")),
            "{name} must be bit {bit} in FrustSurfaceView.kt"
        );
    }
}

#[test]
fn the_export_is_declared_in_lib_and_its_body_lives_in_the_glue() {
    let lib = code(LIB);
    assert!(
        lib.contains("fn Java_dev_frust_FrustSurfaceView_nativeFrameSync<'local>("),
        "lib.rs declares the export"
    );
    assert!(
        lib.contains("$crate::jni_glue::native_frame_sync(handle)"),
        "the export delegates to the glue body"
    );
    assert!(
        code(GLUE).contains("pub fn native_frame_sync(handle: jlong) -> jlong {"),
        "jni_glue.rs holds the body"
    );
}

#[test]
fn the_gated_exports_are_all_still_declared() {
    let lib = code(LIB);
    let glue = code(GLUE);
    for export in [
        "nativeImeState",
        "nativeAppIsDark",
        "nativeSystemUiState",
        "nativePlatformViewCommands",
    ] {
        assert!(
            lib.contains(&format!("fn Java_dev_frust_FrustSurfaceView_{export}")),
            "{export} must keep its generated export"
        );
    }
    for export in ["nativeTakeClipboardWrite", "nativeTakePasteRequest"] {
        assert!(
            glue.contains(&format!("fn Java_dev_frust_FrustSurfaceView_{export}")),
            "{export} must keep its hand-written export"
        );
    }
}

#[test]
fn the_clipboard_takers_serve_a_value_the_sync_already_drained() {
    let glue = code(GLUE);
    let write = block(&glue, "fn native_take_clipboard_write(");
    assert!(
        write.contains("frame_sync_latch().take_clipboard_write()"),
        "the held write is served before the app slot is drained"
    );
    let paste = block(&glue, "fn native_take_paste_request(");
    assert!(
        paste.contains("frame_sync_latch().take_paste_request()"),
        "the held paste request is served before the app slot is drained"
    );
}

#[test]
fn ime_side_inputs_mark_the_ime_poll_due() {
    let glue = code(GLUE);
    for (signature, label) in [
        ("pub fn native_ime_apply(", "nativeImeApply"),
        ("pub fn native_ime_action(", "nativeImeAction"),
        ("fn native_edit_command(", "nativeEditCommand"),
    ] {
        assert!(
            block(&glue, signature).contains("frame_sync_latch().mark_ime_dirty()"),
            "{label} must mark the IME poll due"
        );
    }
    assert!(
        block(&glue, "pub fn native_on_destroy(").contains("frame_sync_latch().reset()"),
        "destroying the handle resets the latch"
    );
}

#[test]
fn do_frame_syncs_once_and_gates_the_polls_on_the_bits() {
    let kotlin = code(KOTLIN);
    let do_frame = block(&kotlin, "override fun doFrame(");
    assert!(
        !do_frame.contains("nativeFrameSync("),
        "doFrame reaches the sync only through the poll helpers"
    );
    assert_eq!(
        kotlin.matches("nativeFrameSync(handle)").count(),
        2,
        "one call on the plain path, one on the timed path"
    );
    let due = block(&kotlin, "private fun pollsDue(");
    for (bit, poll) in [
        ("FRAME_SYNC_IME_CHANGED", "pollImeAfterDispatch()"),
        ("FRAME_SYNC_SYSTEM_UI_CHANGED", "pollSystemUiState()"),
        ("FRAME_SYNC_DARK_CHANGED", "pollAppBrightness()"),
    ] {
        let gate = due.find(&format!("sync and {bit} != 0L")).expect(bit);
        let call = due.find(poll).expect(poll);
        assert!(gate < call, "{poll} must sit behind {bit}");
    }
    assert!(
        due.contains("sync and (FRAME_SYNC_CLIPBOARD_WRITE or FRAME_SYNC_PASTE_REQUEST) != 0L"),
        "the clipboard drain is gated on its two bits"
    );
    // Order is part of the contract: the IME reconcile settles the mirror
    // before a paste dispatched by the clipboard drain reads it.
    let ime = due.find("pollImeAfterDispatch()").unwrap();
    let clipboard = due.find("syncClipboard(sync)").unwrap();
    let system_ui = due.find("pollSystemUiState()").unwrap();
    let dark = due.find("pollAppBrightness()").unwrap();
    assert!(ime < clipboard && clipboard < system_ui && system_ui < dark);
    // The platform-view poll is the one crossing the word does not gate.
    assert!(due.contains("nativePlatformViewCommands(handle, host.ackedGeneration)"));
}

#[test]
fn clipboard_natives_are_called_only_for_their_bits() {
    let kotlin = code(KOTLIN);
    let body = block(&kotlin, "private fun syncClipboard(");
    let write_gate = body
        .find("sync and FRAME_SYNC_CLIPBOARD_WRITE != 0L")
        .unwrap();
    let write_call = body.find("nativeTakeClipboardWrite(handle)").unwrap();
    assert!(write_gate < write_call);
    let paste_gate = body
        .find("sync and FRAME_SYNC_PASTE_REQUEST != 0L")
        .unwrap();
    let paste_call = body.find("nativeTakePasteRequest(handle)").unwrap();
    assert!(paste_gate < paste_call);
}
