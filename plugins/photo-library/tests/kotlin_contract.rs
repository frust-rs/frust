//! Source-scan contract tests between `frust-photo-library`'s Rust side and
//! its Android Kotlin host, because no cargo gate compiles Kotlin (only a real
//! Gradle build does) and `src/android.rs` builds for Android only. Same shape
//! as `plugins/permissions/tests/kotlin_contract.rs`: a plain `std::fs` line
//! scan, not a parser, correct for the hand-written shapes these files take.
//!
//! Pinned here:
//! - every Kotlin result/filter `const val` equals its `const NAME: jint`
//!   twin in `src/android.rs`;
//! - the JNI exports and the host class name match the Kotlin package and
//!   object, and the Rust -> Kotlin `pick` and `cancel` calls' JNI signatures
//!   match their Kotlin declarations;
//! - the module manifest declares no permission and declares the init
//!   provider; the keep rules cover the host and the provider;
//! - the picker contracts and their limit handling (`maxItems` of at least 2,
//!   capped at the platform maximum, result truncated to the limit);
//! - the host never builds a file name from platform text; Rust renames the
//!   staging copies with the pick cache's sanitiser;
//! - one pick is in flight at a time, on both sides, launched on the main
//!   thread, and a destroyed Activity cancels its pick;
//! - the Rust wait has no deadline;
//! - cancel: the hook calls the host and ends the wait locally, the copy loop
//!   is chunked and checks the flag, `CANCELLED` maps to `Cancelled`;
//! - progress is never forwarded under a lock on either side.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const KOTLIN_HOST: &str =
    "platform/android/src/main/kotlin/dev/frust/photolibrary/FrustPhotoLibraryHost.kt";
const KOTLIN_PROVIDER: &str =
    "platform/android/src/main/kotlin/dev/frust/photolibrary/FrustPhotoLibraryInitProvider.kt";
const MANIFEST: &str = "platform/android/src/main/AndroidManifest.xml";
const CONSUMER_RULES: &str = "platform/android/consumer-rules.pro";
const GRADLE: &str = "platform/android/build.gradle.kts";
const RUST_ANDROID: &str = "src/android.rs";

/// How many leading [`SHARED_CODES`] are result codes (the rest are filters).
const RESULT_CODES: usize = 7;

/// Every code the two sides share, by name.
const SHARED_CODES: &[&str] = &[
    "PENDING",
    "NO_ACTIVITY",
    "BUSY",
    "NOT_INITIALIZED",
    "LAUNCH_FAILED",
    "COPY_FAILED",
    "CANCELLED",
    "FILTER_IMAGES",
    "FILTER_VIDEOS",
    "FILTER_IMAGES_AND_VIDEOS",
];

fn read(rel: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// `const val NAME = <int>` lines of a Kotlin file, by name.
fn kotlin_int_consts(contents: &str) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for line in contents.lines() {
        let Some(rest) = line.trim().strip_prefix("const val ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        if let Ok(value) = value.trim().parse::<i64>() {
            out.insert(name.trim().to_string(), value);
        }
    }
    out
}

/// `const val NAME = "<text>"` lines of a Kotlin file, by name.
fn kotlin_string_consts(contents: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in contents.lines() {
        let Some(rest) = line.trim().strip_prefix("const val ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        if let Some(text) = value
            .trim()
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
        {
            out.insert(name.trim().to_string(), text.to_string());
        }
    }
    out
}

/// `const NAME: jint = <int>;` lines of a Rust file, by name.
fn rust_jint_consts(contents: &str) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for line in contents.lines() {
        let Some(rest) = line.trim().strip_prefix("const ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(": jint =") else {
            continue;
        };
        if let Ok(value) = value.trim().trim_end_matches(';').trim().parse::<i64>() {
            out.insert(name.trim().to_string(), value);
        }
    }
    out
}

/// The text of the function `name` (Kotlin `fun` or Rust `fn`), from its
/// declaration line through its closing brace at the same indentation.
fn function<'a>(contents: &'a str, marker: &str) -> &'a str {
    let start = contents
        .find(marker)
        .unwrap_or_else(|| panic!("source has no `{marker}`"));
    let line_start = contents[..start].rfind('\n').map_or(0, |i| i + 1);
    let indent = &contents[line_start..start];
    let indent = &indent[..indent.len() - indent.trim_start().len()];
    let close = format!("\n{indent}}}");
    let end = contents[start..]
        .find(&close)
        .map(|i| start + i + close.len())
        .unwrap_or(contents.len());
    &contents[start..end]
}

/// The Rust source with `//` comments removed, so a scan sees code only.
fn rust_code(contents: &str) -> String {
    contents
        .lines()
        .map(|line| match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn kotlin_codes_equal_their_rust_twins() {
    let kotlin = kotlin_int_consts(&read(KOTLIN_HOST));
    let rust = rust_jint_consts(&read(RUST_ANDROID));
    for name in SHARED_CODES {
        let k = kotlin
            .get(*name)
            .unwrap_or_else(|| panic!("Kotlin host has no `const val {name}`"));
        let r = rust
            .get(*name)
            .unwrap_or_else(|| panic!("src/android.rs has no `const {name}: jint`"));
        assert_eq!(k, r, "Kotlin `{name}` must equal src/android.rs's");
    }
    let mut values: Vec<_> = SHARED_CODES[..RESULT_CODES]
        .iter()
        .map(|n| rust[*n])
        .collect();
    values.sort_unstable();
    values.dedup();
    assert_eq!(values.len(), RESULT_CODES, "result codes must be distinct");
}

#[test]
fn jni_names_match_the_kotlin_package_and_object() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID);
    let package = kotlin
        .lines()
        .find_map(|l| l.trim().strip_prefix("package "))
        .expect("Kotlin host declares a package")
        .trim();
    assert_eq!(package, "dev.frust.photolibrary");
    assert!(kotlin.contains("\nobject FrustPhotoLibraryHost {"));

    let fqcn = format!("{package}.FrustPhotoLibraryHost");
    assert!(
        rust.contains(&format!("const HOST_CLASS_BINARY: &str = \"{fqcn}\";")),
        "src/android.rs must load `{fqcn}`"
    );

    let result_decl = "external fun nativeOnPickResult(\n        requestId: Long,\n        paths: Array<String>,\n        names: Array<String?>,\n        mimes: Array<String?>,\n    )";
    assert!(
        kotlin.contains(&format!("@JvmStatic\n    {result_decl}")),
        "Kotlin `nativeOnPickResult` signature changed"
    );
    assert!(
        kotlin
            .contains("@JvmStatic\n    external fun nativeOnPickError(requestId: Long, code: Int)"),
        "Kotlin `nativeOnPickError` signature changed"
    );
    assert!(
        kotlin.contains(
            "@JvmStatic\n    external fun nativeOnPickProgress(requestId: Long, done: Int, items: Int, fraction: Double)"
        ),
        "Kotlin `nativeOnPickProgress` signature changed"
    );

    let prefix = format!("Java_{}_FrustPhotoLibraryHost_", package.replace('.', "_"));
    let exports: [(&str, &[&str]); 3] = [
        (
            "nativeOnPickResult",
            &[
                "request_id: jlong,",
                "paths: JObjectArray<'local>,",
                "names: JObjectArray<'local>,",
                "mimes: JObjectArray<'local>,",
            ],
        ),
        ("nativeOnPickError", &["request_id: jlong,", "code: jint,"]),
        (
            "nativeOnPickProgress",
            &[
                "request_id: jlong,",
                "done: jint,",
                "items: jint,",
                "fraction: jdouble,",
            ],
        ),
    ];
    for (method, params) in exports {
        let symbol = format!("{prefix}{method}");
        let decl = format!("pub extern \"system\" fn {symbol}<");
        assert!(
            rust.contains(&decl),
            "src/android.rs must export `{symbol}`"
        );
        let export = function(&rust, &decl);
        for param in params {
            assert!(
                export.contains(param),
                "`{symbol}` must take `{param}` to match the Kotlin declaration"
            );
        }
        assert!(
            export.contains(".resolve::<LogErrorAndDefault>()"),
            "`{symbol}` must never unwind into the JVM"
        );
    }
}

#[test]
fn rust_pick_call_matches_the_kotlin_declaration() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID);
    assert!(
        kotlin.contains(
            "@JvmStatic\n    fun pick(requestId: Long, filter: Int, limit: Int, destDir: String): Int"
        ),
        "Kotlin host must declare `@JvmStatic fun pick(requestId: Long, filter: Int, limit: Int, destDir: String): Int`"
    );
    let at = rust
        .find("jni_str!(\"pick\")")
        .expect("src/android.rs never calls `pick`");
    let window = &rust[at..(at + 120).min(rust.len())];
    assert!(
        window.contains("jni_sig!(\"(JIILjava/lang/String;)I\")"),
        "src/android.rs must call `pick` with `(JIILjava/lang/String;)I`"
    );

    assert!(
        kotlin.contains("@JvmStatic\n    fun cancel(requestId: Long) {"),
        "Kotlin host must declare `@JvmStatic fun cancel(requestId: Long)`"
    );
    let at = rust
        .find("jni_str!(\"cancel\")")
        .expect("src/android.rs never calls `cancel`");
    let window = &rust[at..(at + 80).min(rust.len())];
    assert!(
        window.contains("jni_sig!(\"(J)V\")"),
        "src/android.rs must call `cancel` with `(J)V`"
    );
}

#[test]
fn module_manifest_declares_no_permission_and_the_init_provider() {
    let manifest = read(MANIFEST);
    assert!(
        !manifest.contains("uses-permission"),
        "the photo picker needs no permission; the module manifest must declare none"
    );
    assert!(
        manifest.contains("android:name=\"dev.frust.photolibrary.FrustPhotoLibraryInitProvider\"")
    );
    assert!(manifest.contains("android:authorities=\"${applicationId}.frustphotolibraryinit\""));
    assert!(manifest.contains("android:exported=\"false\""));
    assert!(
        read(KOTLIN_PROVIDER).contains("class FrustPhotoLibraryInitProvider : ContentProvider()")
    );
}

#[test]
fn keep_rules_cover_the_host_and_the_provider() {
    let rules = read(CONSUMER_RULES);
    for line in [
        "-keep class dev.frust.photolibrary.FrustPhotoLibraryHost { *; }",
        "-keep class dev.frust.photolibrary.FrustPhotoLibraryInitProvider { *; }",
    ] {
        assert!(
            rules.contains(line),
            "consumer-rules.pro must keep `{line}`"
        );
    }
}

#[test]
fn gradle_module_matches_the_plugin_module_shape() {
    let gradle = read(GRADLE);
    for line in [
        "id(\"com.android.library\")",
        "namespace = \"dev.frust.photolibrary\"",
        "compileSdk = 36",
        "minSdk = 29",
        "consumerProguardFiles(\"consumer-rules.pro\")",
        "implementation(project(\":frust-embedding\"))",
    ] {
        assert!(
            gradle.contains(line),
            "build.gradle.kts must contain `{line}`"
        );
    }
    let code: String = gradle
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect();
    assert!(
        !code.contains("org.jetbrains.kotlin.android"),
        "AGP 9 has built-in Kotlin; no Kotlin plugin is applied"
    );
}

#[test]
fn picker_contracts_and_limit_handling() {
    let kotlin = read(KOTLIN_HOST);
    let launch = function(&kotlin, "private fun launch(");
    assert!(launch.contains("if (limit == 1) {"));
    assert!(launch.contains("ActivityResultContracts.PickVisualMedia()"));
    assert!(launch.contains("ActivityResultContracts.PickMultipleVisualMedia()"));
    assert!(launch.contains(
        "ActivityResultContracts.PickMultipleVisualMedia(multipleMaxItems(limit, platformMaxItems()))"
    ));
    assert!(
        kotlin.contains("maxOf(limit, 2).coerceAtMost(maxOf(platformMax, 2))"),
        "PickMultipleVisualMedia throws unless maxItems > 1, and the system picker \
         throws above MediaStore.getPickImagesMaxLimit()"
    );
    assert!(kotlin.contains("MediaStore.getPickImagesMaxLimit()"));
    for media in ["ImageOnly", "VideoOnly", "ImageAndVideo"] {
        assert!(kotlin.contains(&format!("ActivityResultContracts.PickVisualMedia.{media}")));
    }
    let on_picked = function(&kotlin, "private fun onPicked(");
    assert!(
        on_picked.contains("if (limit > 0) uris.take(limit) else uris"),
        "the result must be truncated to the limit (the ACTION_OPEN_DOCUMENT fallback has no maximum)"
    );
}

#[test]
fn the_host_never_names_a_file_from_platform_text() {
    let kotlin = read(KOTLIN_HOST);
    let strings = kotlin_string_consts(&kotlin);
    let staging = strings
        .get("STAGING_PREFIX")
        .expect("Kotlin host declares STAGING_PREFIX");
    assert!(
        staging.starts_with('.'),
        "a staging name must start with a dot, which the sanitiser never produces"
    );
    let on_picked = function(&kotlin, "private fun onPicked(");
    assert!(on_picked.contains("File(destDir, STAGING_PREFIX + index)"));
    assert_eq!(
        kotlin.matches("File(destDir, ").count(),
        1,
        "the staging path is the only file the host names inside destDir"
    );

    let cache = rust_code(&read("src/pick_cache.rs"));
    let copy_name = function(&cache, "fn copy_name(");
    assert!(copy_name.contains("sanitize_file_name("));
    let rust = rust_code(&read(RUST_ANDROID));
    let finish = function(&rust, "fn finish_copies(");
    assert!(finish.contains("staging.parent() != Some(dir)"));
    assert!(finish.contains("fs::rename(&staging, &path)?;"));
    assert!(finish.contains("is_copy: true,"));
    assert!(finish.contains("asset_id: None,"));
}

#[test]
fn one_pick_is_in_flight_at_a_time() {
    let kotlin = read(KOTLIN_HOST);
    let pick = function(&kotlin, "fun pick(");
    assert!(
        pick.contains("if (!claimInFlight(requestId, activity)) {") && pick.contains("return BUSY"),
        "a second pick must get BUSY from the Kotlin host"
    );
    assert!(
        pick.contains("mainHandler.post { launch("),
        "the picker launches on the main thread"
    );
    assert!(pick.contains("return PENDING"));
    let claim = function(&kotlin, "private fun claimInFlight(");
    assert!(claim.contains("if (inFlight != null) return false"));
    let launch = function(&kotlin, "private fun launch(");
    assert!(launch.contains("val key = LAUNCHER_KEY_PREFIX + requestId"));
    assert_eq!(
        kotlin_string_consts(&kotlin)
            .get("LAUNCHER_KEY_PREFIX")
            .map(String::as_str),
        Some("frust.photolibrary.")
    );
    let destroyed = function(&kotlin, "override fun onActivityDestroyed(");
    assert!(
        destroyed.contains("releaseInFlightFor(activity)?.let { notifyCancelled(it) }"),
        "a destroyed Activity must cancel its pick so the Rust wait ends"
    );

    let rust = rust_code(&read(RUST_ANDROID));
    assert!(
        rust_code(&read("src/pick_cache.rs")).contains("static IN_FLIGHT: AtomicBool"),
        "the crate-wide guard lives in the pick cache"
    );
    let pick = function(&rust, "pub(crate) fn pick(");
    let guard = pick
        .find("let _one_at_a_time = pick_cache::PickGuard::acquire()?;")
        .expect("pick must take the crate-wide PickGuard in a named guard");
    let ui = pick
        .find("ensure_off_ui_thread")
        .expect("pick checks the UI thread");
    let dir = pick
        .find("pick_cache::new_call_dir()")
        .expect("pick creates its call directory");
    assert!(
        ui < guard && guard < dir,
        "the UI-thread check comes first, then the lock, then the directory"
    );
    let deliver = function(&rust, "fn deliver(");
    assert!(
        !deliver.contains("PickGuard"),
        "deliver must never take the PickGuard, or a held pick could not be answered"
    );
    assert!(
        deliver.contains("answer: slot @ None,") && deliver.contains("*slot = Some(answer);"),
        "the first answer for an id wins"
    );
}

#[test]
fn the_rust_wait_has_no_deadline() {
    let rust = rust_code(&read(RUST_ANDROID));
    assert!(
        !rust.contains("wait_timeout"),
        "a person may keep the picker open indefinitely; the host always answers"
    );
    let next_event = function(&rust, "fn next_event(");
    assert!(next_event.contains(".wait_while("));
}

#[test]
fn cancel_ends_the_wait_and_stops_the_copies() {
    let rust = rust_code(&read(RUST_ANDROID));
    let pick = function(&rust, "pub(crate) fn pick(");
    let early = pick
        .find("if is_cancelled(options) {")
        .expect("an already-cancelled token fails before anything runs");
    let ui = pick.find("ensure_off_ui_thread").unwrap();
    assert!(early < ui);
    let run = pick.find("pick_into(options, &dir)").unwrap();
    let clear = pick
        .find("token.clear_hook();")
        .expect("pick clears the token hook on every exit");
    assert!(run < clear);

    let pick_into = function(&rust, "fn pick_into(");
    let pending = pick_into.find("if code != PENDING {").unwrap();
    let hook = pick_into
        .find("token.set_hook(move || cancel_request(request_id));")
        .expect("pick installs the cancel hook");
    assert!(
        pending < hook,
        "the hook is installed only once the host answered PENDING"
    );
    assert!(pick_into.contains("Answer::Cancelled => Err(PhotoError::Cancelled),"));

    let hook = function(&rust, "fn cancel_request(");
    let host = hook.find("jni_str!(\"cancel\")").unwrap();
    let local = hook
        .find("deliver(request_id, Answer::Cancelled);")
        .expect("the hook ends the wait locally, without waiting for the host");
    assert!(
        host < local,
        "the host flag is set before the local answer frees the call dir"
    );
    assert!(
        function(&rust, "fn error_for(").contains("CANCELLED => PhotoError::Cancelled,"),
        "the host's CANCELLED code maps to PhotoError::Cancelled"
    );

    let kotlin = read(KOTLIN_HOST);
    assert!(kotlin.contains("const val COPY_CHUNK_BYTES = 64 * 1024"));
    let chunks = function(&kotlin, "private fun copyChunks(");
    assert!(chunks.contains("ByteArray(COPY_CHUNK_BYTES)"));
    let check = chunks.find("if (flight.cancelled) return false").unwrap();
    let read_at = chunks.find("source.read(buffer)").unwrap();
    assert!(check < read_at, "the flag is checked before every chunk");
    assert!(
        !kotlin.contains(".copyTo("),
        "an unchunked copy could not stop a large video promptly"
    );
    let copy_all = function(&kotlin, "private fun copyAll(");
    assert!(
        copy_all.contains("if (flight.cancelled) return null"),
        "the flag is checked between items"
    );
    let on_picked = function(&kotlin, "private fun onPicked(");
    assert!(on_picked.contains("val flight = detachInFlight(requestId) ?: return"));
    let cancelled = on_picked
        .find("true -> {")
        .expect("a cancelled copy answers CANCELLED");
    let tail = &on_picked[cancelled..];
    let delete = tail.find("staging.forEach { it.delete() }").unwrap();
    let answer = tail.find("notifyError(requestId, CANCELLED)").unwrap();
    assert!(
        delete < answer,
        "staging files are deleted before CANCELLED is answered"
    );
    assert!(on_picked.contains("when (releaseCopied(flight)) {"));
    let cancel = function(&kotlin, "fun cancel(requestId: Long) {");
    assert!(cancel.contains("if (cancelInFlight(requestId)) notifyError(requestId, CANCELLED)"));
    let mark = function(&kotlin, "private fun cancelInFlight(");
    assert!(mark.contains("current.cancelled = true"));
    let launch = function(&kotlin, "private fun launch(");
    assert!(
        launch.contains("if (!holdsInFlight(requestId)) return"),
        "a pick cancelled before its launch ran opens no picker"
    );
}

#[test]
fn progress_is_never_forwarded_under_a_lock() {
    let rust = rust_code(&read(RUST_ANDROID));
    let export = function(
        &rust,
        "pub extern \"system\" fn Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnPickProgress<",
    );
    assert!(export.contains("record_progress(request_id as u64, progress);"));
    let record = function(&rust, "fn record_progress(");
    assert!(record.contains("ANSWERED.notify_all();"));
    for body in [export, record, function(&rust, "fn next_event(")] {
        assert!(
            !body.contains(".report(") && !body.contains("progress)("),
            "the JNI thread and the locked wait never run the caller's callback"
        );
    }
    let await_answer = function(&rust, "fn await_answer(");
    assert!(
        await_answer.contains("next_event(request_id)") && await_answer.contains(".report("),
        "the waiting thread forwards progress through the throttle"
    );
    assert!(
        !await_answer.contains("lock("),
        "progress is forwarded after the WAITS lock is released"
    );
    let pick_into = function(&rust, "fn pick_into(");
    assert!(pick_into.contains("options.progress.clone().map(ProgressThrottle::new)"));
    assert!(
        pick_into.contains("done: items.len(),") && pick_into.contains("fraction: 1.0,"),
        "the final report is {{items, done: items, fraction: 1.0}}"
    );

    let kotlin = read(KOTLIN_HOST);
    let notify = function(&kotlin, "private fun notifyProgress(");
    assert!(notify.contains("nativeOnPickProgress(requestId, done, items, fraction)"));
    assert_eq!(
        kotlin.matches("nativeOnPickProgress(requestId, ").count(),
        1,
        "progress reaches Rust only through notifyProgress"
    );
    for marker in [
        "private fun notifyProgress(",
        "private fun copyAll(",
        "private fun copyChunks(",
    ] {
        let at = kotlin.find(marker).unwrap();
        let before = kotlin[..at].trim_end();
        assert!(
            !before.ends_with("@Synchronized"),
            "`{marker}` must not hold the host's monitor"
        );
    }
    let send = function(&kotlin, "fun send() {");
    assert!(
        send.contains("done >= items -> 1.0"),
        "the last item reports 1.0"
    );
    let copy_all = function(&kotlin, "private fun copyAll(");
    let start = copy_all.find("progress.send()").unwrap();
    let first_copy = copy_all.find("openInputStream").unwrap();
    assert!(start < first_copy, "progress starts before the first copy");
    assert!(copy_all.contains("progress.itemDone()"));
    assert!(kotlin.contains("OpenableColumns.SIZE"));
}
