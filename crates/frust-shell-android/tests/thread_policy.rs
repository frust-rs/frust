//! Render-thread scheduling contract, checked on the host.
//!
//! The render thread is scheduled by `jni_glue::schedule_render_thread`, the
//! first statement of `render_loop`: by default it takes
//! `frust_reactive::thread_policy`'s frame class, which keeps the nice value the
//! thread inherited from the UI thread and asks for `THREAD_PRIORITY_DISPLAY`
//! only when that is better; under `FRUST_NO_THREAD_TUNING` it keeps the
//! previous fixed `setpriority(.., THREAD_PRIORITY_DISPLAY)`, which overwrites
//! the inherited value.
//!
//! `jni_glue` and the `frust-reactive` edge exist only in the Android build, so
//! a host test can reach neither. What the shim itself does to a thread — a
//! background thread ends up at a higher nice value than its spawner, a frame
//! thread never at a lower priority than it inherited — is tested against the
//! real kernel by `frust-reactive`'s own `thread_policy` and `runtime` tests on
//! a Linux host. This file pins the shell side of the contract on the source
//! (source-scan precedent: `crates/frust-drive/tests/print_free_cores.rs`):
//! the scheduling call comes before any frame work, and the fixed boost can
//! only run on the kill-switch arm, never after the frame class. Comment lines
//! are dropped before every scan, so a doc quoting a call never counts as one.
//! The iOS shell's `ffi_glue` follows the same shape and is checked beside it.

const ANDROID_GLUE: &str = include_str!("../src/jni_glue.rs");
const IOS_GLUE: &str = include_str!("../../frust-shell-ios/src/ffi_glue.rs");

/// `source` with every comment-only line dropped.
fn code(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of the first `fn` whose signature starts with `signature`: the
/// text between its opening `{` (the first one ending a line after the
/// signature) and the matching closing brace.
fn fn_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` not found"));
    let open = start
        + source[start..]
            .find("{\n")
            .unwrap_or_else(|| panic!("`{signature}` has no body"));
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("`{signature}` body is unbalanced");
}

/// The first non-blank line of `body`.
fn first_statement(body: &str) -> &str {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .expect("body has a statement")
}

/// Byte offset of `needle` in `haystack`, failing with `what` when absent.
fn position(haystack: &str, needle: &str, what: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("{what}: `{needle}` not found"))
}

/// The scheduling contract both shells share: the kill switch is read first,
/// its default arm applies the frame class and returns, and the one fixed
/// boost sits only after that return.
fn assert_schedules_through_the_frame_class(body: &str, fixed_boost: &str, shell: &str) {
    let switch = position(
        body,
        "render_split::thread_tuning_enabled()",
        &format!("{shell}: the kill switch"),
    );
    let class = position(
        body,
        "set_current_thread_class(ThreadClass::Frame)",
        &format!("{shell}: the frame class"),
    );
    let early_return = position(
        body,
        "return;",
        &format!("{shell}: the default arm's return"),
    );
    let boost = position(body, fixed_boost, &format!("{shell}: the off-arm boost"));
    assert!(
        switch < class && class < early_return,
        "{shell}: the frame class must be the default arm of the kill switch"
    );
    assert!(
        early_return < boost,
        "{shell}: the fixed boost must only run when the kill switch is engaged"
    );
    assert_eq!(
        body.matches(fixed_boost).count(),
        1,
        "{shell}: exactly one fixed-boost call, on the off-arm"
    );
}

#[test]
fn android_render_loop_schedules_its_thread_before_any_frame_work() {
    let source = code(ANDROID_GLUE);
    let body = fn_body(&source, "pub(crate) fn render_loop(");
    assert_eq!(first_statement(body), "schedule_render_thread();");
}

#[test]
fn android_render_thread_takes_the_frame_class_unless_tuning_is_off() {
    let source = code(ANDROID_GLUE);
    let body = fn_body(&source, "fn schedule_render_thread()");
    assert_schedules_through_the_frame_class(body, "libc::setpriority(", "android");
    assert!(
        body.contains("THREAD_PRIORITY_DISPLAY,"),
        "android: the off-arm keeps the published display nice value"
    );
}

#[test]
fn android_glue_sets_priority_only_in_the_scheduling_helper() {
    let source = code(ANDROID_GLUE);
    let helper = fn_body(&source, "fn schedule_render_thread()");
    assert_eq!(
        source.matches("libc::setpriority(").count(),
        helper.matches("libc::setpriority(").count(),
        "android: no setpriority call outside schedule_render_thread"
    );
}

#[test]
fn ios_render_loop_schedules_its_thread_before_any_frame_work() {
    let source = code(IOS_GLUE);
    let body = fn_body(&source, "pub(crate) fn render_loop(");
    assert_eq!(first_statement(body), "schedule_render_thread();");
}

#[test]
fn ios_render_thread_takes_the_frame_class_unless_tuning_is_off() {
    let source = code(IOS_GLUE);
    let body = fn_body(&source, "fn schedule_render_thread()");
    assert_schedules_through_the_frame_class(body, "libc::pthread_set_qos_class_self_np(", "ios");
}
