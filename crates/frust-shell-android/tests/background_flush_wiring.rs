//! Background font-cache flush wiring, checked on the source.
//!
//! Both mobile shells persist the font oracle's unwritten answers when the app
//! goes to the background, through the shared `frust_shell_common::BackgroundFlush`
//! latch: the flush runs once per background transition, and the latch is
//! re-armed on the foreground edge. The latch and its transitions are unit-tested
//! in `frust-shell-common`. What no host test reaches is the shell call sites:
//! they live in the Android `jni_glue` exports (`native_on_start`,
//! `native_on_stop`) and the iOS `ffi_glue` exports (`did_enter_background`,
//! `will_enter_foreground`) and the iOS executor helpers they call, all behind
//! target gates the host build does not exercise.
//!
//! This file pins that wiring on the source, following `thread_policy.rs`: each
//! edge function must call the latch entry point it owns, so the flush cannot
//! silently move out of the edge function. Bodies are matched by the function
//! name up to its `(`, so a visibility or `unsafe` prefix does not change the
//! anchor, and comment-only lines are dropped before every scan. A behavioural
//! seam, as the desktop shell has in its `app_handler` tests, would exercise the
//! wiring directly instead of by text; that is the better shape, but it is out
//! of scope for this check.

const ANDROID_GLUE: &str = include_str!("../src/jni_glue.rs");
const IOS_GLUE: &str = include_str!("../../frust-shell-ios/src/ffi_glue.rs");
const IOS_EXECUTOR: &str = include_str!("../../frust-shell-ios/src/app/executor.rs");

/// `source` with every comment-only line dropped.
fn code(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of the first `fn` whose signature starts with `signature`: the text
/// between its opening `{` and the matching closing brace.
fn fn_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` not found"));
    let open = start
        + source[start..]
            .find('{')
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

/// Fails unless the edge function `edge` calls `callee` inside its own body.
fn assert_edge_calls(body: &str, callee: &str, edge: &str) {
    assert!(
        body.contains(callee),
        "{edge} must call `{callee}` inside its own body"
    );
}

#[test]
fn android_native_on_stop_flushes_the_font_cache_at_the_hidden_edge() {
    let source = code(ANDROID_GLUE);
    let body = fn_body(&source, "fn native_on_stop(");
    assert_edge_calls(body, "went_to_background_outcome(", "android nativeOnStop");
}

#[test]
fn android_native_on_start_re_arms_the_font_cache_flush() {
    let source = code(ANDROID_GLUE);
    let body = fn_body(&source, "fn native_on_start(");
    assert_edge_calls(body, "came_to_foreground(", "android nativeOnStart");
}

#[test]
fn ios_did_enter_background_flushes_the_font_cache() {
    let source = code(IOS_GLUE);
    let body = fn_body(&source, "fn did_enter_background(");
    assert_edge_calls(
        body,
        "flush_font_cache_on_background(",
        "ios didEnterBackground",
    );
}

#[test]
fn ios_will_enter_foreground_re_arms_the_font_cache_flush() {
    let source = code(IOS_GLUE);
    let body = fn_body(&source, "fn will_enter_foreground(");
    assert_edge_calls(body, "font_cache_foregrounded(", "ios willEnterForeground");
}

#[test]
fn ios_flush_helper_runs_the_latched_flush() {
    let source = code(IOS_EXECUTOR);
    let body = fn_body(&source, "fn flush_font_cache_on_background(");
    assert_edge_calls(
        body,
        "went_to_background_outcome(",
        "ios flush_font_cache_on_background",
    );
}

#[test]
fn ios_foreground_helper_re_arms_the_latch() {
    let source = code(IOS_EXECUTOR);
    let body = fn_body(&source, "fn font_cache_foregrounded(");
    assert_edge_calls(body, "came_to_foreground(", "ios font_cache_foregrounded");
}
