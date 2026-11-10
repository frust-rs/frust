//! Source-scan contract between `frust-background-task`'s Rust side and its Android
//! Kotlin, because no cargo gate compiles Kotlin (only a real Gradle build does). A plain
//! `std::fs` scan with comments stripped, correct for the hand-written shapes these files
//! take.
//!
//! Pinned here: the JNI names and signatures on both sides; the `shortService` type, the
//! channel, notification id and watchdog constant; that nothing can escape either start
//! call; that the service reaches `startForeground` before any stop; the deferred
//! notification; the manifest; and the R8 keep rules.

use std::fs;
use std::path::Path;

const HOST: &str =
    "platform/android/src/main/kotlin/dev/frust/backgroundtask/FrustBackgroundTaskHost.kt";
const SERVICE: &str =
    "platform/android/src/main/kotlin/dev/frust/backgroundtask/FrustBackgroundTaskService.kt";
const MANIFEST: &str = "platform/android/src/main/AndroidManifest.xml";
const CONSUMER_RULES: &str = "platform/android/consumer-rules.pro";
const RUST_ANDROID: &str = "src/android.rs";

fn read(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// `src` without its comment lines.
fn code(src: &str) -> String {
    src.lines()
        .filter(|line| {
            let line = line.trim_start();
            !(line.starts_with("//") || line.starts_with("/*") || line.starts_with('*'))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The brace-matched body following the first occurrence of `signature` in `src`.
fn body<'a>(src: &'a str, signature: &str) -> &'a str {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("no `{signature}`"));
    let open = start + src[start..].find('{').unwrap_or_else(|| panic!("no body"));
    let mut depth = 0;
    for (offset, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced body after `{signature}`");
}

fn index_of(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not found"))
}

#[test]
fn host_declares_the_jni_contract() {
    let host = code(&read(HOST));
    assert!(host.contains("package dev.frust.backgroundtask"));
    assert!(host.contains("object FrustBackgroundTaskHost"));
    assert!(
        host.contains(
            "@JvmStatic\n    fun begin(context: Context, id: Long, name: String): Boolean"
        )
    );
    assert!(host.contains("@JvmStatic\n    fun end(context: Context, id: Long)"));
    assert!(host.contains("@JvmStatic\n    external fun nativeOnTimeout(ids: LongArray)"));
}

#[test]
fn rust_calls_match_the_kotlin_contract() {
    let rust = read(RUST_ANDROID);
    assert!(rust.contains("\"dev.frust.backgroundtask.FrustBackgroundTaskHost\""));
    assert!(rust.contains("jni_sig!(\"(Landroid/content/Context;JLjava/lang/String;)Z\")"));
    assert!(rust.contains("jni_sig!(\"(Landroid/content/Context;J)V\")"));
    assert!(rust.contains(
        "pub extern \"system\" fn Java_dev_frust_backgroundtask_FrustBackgroundTaskHost_nativeOnTimeout"
    ));
    assert!(rust.contains("ids: JLongArray<'local>"));
}

#[test]
fn begin_does_no_classloader_lookup() {
    let rust = code(&read(RUST_ANDROID));
    let begin = body(&rust, "fn host_begin");
    assert!(!begin.contains("loadClass") && !begin.contains("load_host_class"));
    assert!(begin.contains("HOST_CLASS.get()"));
}

#[test]
fn a_pending_exception_is_cleared_after_the_static_calls() {
    let rust = code(&read(RUST_ANDROID));
    for function in ["fn host_begin", "fn host_end"] {
        let scope = body(&rust, function);
        assert!(scope.contains("env.exception_clear()"), "{function}");
    }
}

#[test]
fn constants_are_the_agreed_values() {
    let host = code(&read(HOST));
    assert!(host.contains("const val CHANNEL_ID = \"frust_background_work\""));
    assert!(host.contains("const val NOTIFICATION_ID = 0x46525354"));
    assert!(host.contains("const val WATCHDOG_MS = 150_000L"));
}

#[test]
fn service_runs_as_a_short_service_and_handles_both_timeouts() {
    let service = code(&read(SERVICE));
    assert!(service.contains("class FrustBackgroundTaskService : Service()"));
    assert!(service.contains("ServiceInfo.FOREGROUND_SERVICE_TYPE_SHORT_SERVICE"));
    assert!(service.contains("Build.VERSION_CODES.UPSIDE_DOWN_CAKE"));
    assert!(service.contains("START_NOT_STICKY"));
    for signature in [
        "override fun onTimeout(startId: Int) {",
        "override fun onTimeout(startId: Int, fgsType: Int) {",
    ] {
        assert!(
            body(&service, signature).contains("FrustBackgroundTaskHost.timeout()"),
            "{signature}"
        );
    }
}

#[test]
fn nothing_escapes_the_begin_start_call() {
    let host = code(&read(HOST));
    let begin = body(&host, "fun begin(");
    let start = index_of(begin, "context.startForegroundService(");
    assert!(index_of(begin, "try {") < start);
    assert!(start < index_of(begin, "catch (e: Exception)"));
}

#[test]
fn a_null_component_is_a_refusal() {
    let host = code(&read(HOST));
    let begin = body(&host, "fun begin(");
    let start = index_of(begin, "context.startForegroundService(");
    let null_check = index_of(begin, ") != null");
    assert!(start < null_check);
    assert!(null_check < index_of(begin, "catch (e: Exception)"));
    assert!(begin.contains("return false"));
}

#[test]
fn start_foreground_is_guarded_and_precedes_any_stop() {
    let service = code(&read(SERVICE));
    let command = body(&service, "override fun onStartCommand");
    let start = index_of(command, "ServiceCompat.startForeground(");
    let first_stop = command
        .find("stop")
        .expect("onStartCommand stops when idle");
    assert!(start < first_stop, "a stop precedes startForeground");
    assert!(index_of(command, "try {") < start);
    assert!(start < index_of(command, "catch (e: Exception)"));
    assert!(command.contains("FrustBackgroundTaskHost.isIdle()"));
}

#[test]
fn a_late_refusal_expires_the_live_tasks_and_stops() {
    let service = code(&read(SERVICE));
    let command = body(&service, "override fun onStartCommand");
    let catch = body(command, "catch (e: Exception)");
    assert!(catch.contains("FrustBackgroundTaskHost.expireLive()"));
    assert!(catch.contains("stopSelf(lastStartId)"));
    let host = code(&read(HOST));
    assert!(body(&host, "fun expireLive()").contains("nativeOnTimeout(ids)"));
}

#[test]
fn end_only_stops_a_service_already_in_the_foreground() {
    let host = code(&read(HOST));
    let end = body(&host, "fun end(");
    assert!(end.contains("mainHandler.post"));
    assert!(end.contains("stopIfForeground()"));
    assert!(!end.contains("stopNow") && !end.contains("stopSelf"));
    let service = code(&read(SERVICE));
    assert!(body(&service, "fun stopIfForeground()").contains("if (inForeground) stopNow()"));
}

#[test]
fn stopping_names_the_last_start_id() {
    let service = code(&read(SERVICE));
    assert!(body(&service, "fun stopNow()").contains("stopSelf(lastStartId)"));
    assert!(
        !service.contains("stopSelf()"),
        "a bare stopSelf drops an in-flight begin"
    );
}

#[test]
fn the_notification_is_deferred_and_never_immediate() {
    let service = read(SERVICE);
    assert!(
        service.contains(
            "setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_DEFERRED)"
        )
    );
    for file in [HOST, SERVICE] {
        assert!(
            !read(file).contains("FOREGROUND_SERVICE_IMMEDIATE"),
            "{file}"
        );
    }
    let service = code(&service);
    assert!(!service.contains("addAction"));
    assert!(!service.contains("setCategory"));
}

#[test]
fn the_channel_is_built_by_the_service_not_by_begin() {
    let host = code(&read(HOST));
    assert!(!body(&host, "fun begin(").contains("Notification"));
    let service = code(&read(SERVICE));
    assert!(service.contains("IMPORTANCE_LOW"));
    assert!(service.contains("PRIORITY_LOW"));
}

#[test]
fn manifest_declares_the_permission_and_a_private_short_service() {
    let manifest = code(&read(MANIFEST));
    assert!(
        manifest
            .contains("<uses-permission android:name=\"android.permission.FOREGROUND_SERVICE\"")
    );
    assert!(manifest.contains("android:name=\".FrustBackgroundTaskService\""));
    assert!(manifest.contains("android:foregroundServiceType=\"shortService\""));
    assert!(manifest.contains("android:exported=\"false\""));
    assert!(!manifest.contains("POST_NOTIFICATIONS"));
}

#[test]
fn keep_rules_cover_both_classes() {
    let rules = read(CONSUMER_RULES);
    assert!(rules.contains("-keep class dev.frust.backgroundtask.FrustBackgroundTaskHost { *; }"));
    assert!(
        rules.contains("-keep class dev.frust.backgroundtask.FrustBackgroundTaskService { *; }")
    );
}
