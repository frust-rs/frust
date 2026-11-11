//! Source-scan contract tests between `frust-file-dialogs`' Rust side and its
//! Android Kotlin host, because no cargo gate compiles Kotlin (only a real
//! Gradle build does) and `src/android.rs` builds for Android only. A plain
//! `std::fs` line scan, not a parser, correct for the hand-written shapes these
//! files take.
//!
//! Pinned here:
//! - every Kotlin result `const val` equals its `const NAME: jint` twin in
//!   `src/android.rs`;
//! - the JNI exports and the host class name match the Kotlin package and
//!   object, and each Rust -> Kotlin call's JNI signature matches its Kotlin
//!   declaration;
//! - the module manifest declares no permission and declares the init
//!   provider; the keep rules cover the host and the provider;
//! - the Activity Result contracts, the persisted read grant and the single
//!   in-flight slot launched on the main thread;
//! - the Rust wait has no deadline and every blocking call refuses the UI
//!   thread, `save_file` stays unsupported;
//! - cancel reaches the host, ends the wait locally, stops copies between
//!   chunks and listings between entries, and removes partial output;
//! - a user dismissal stays distinct from a cancel;
//! - progress is never forwarded under a lock on either side;
//! - an entry lookup resolves one listed folder per segment by exact display
//!   name, under the listing's name rules, and never builds a document id.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const KOTLIN_HOST: &str =
    "platform/android/src/main/kotlin/dev/frust/filedialogs/FrustFileDialogsHost.kt";
const KOTLIN_PROVIDER: &str =
    "platform/android/src/main/kotlin/dev/frust/filedialogs/FrustFileDialogsInitProvider.kt";
const MANIFEST: &str = "platform/android/src/main/AndroidManifest.xml";
const CONSUMER_RULES: &str = "platform/android/consumer-rules.pro";
const GRADLE: &str = "platform/android/build.gradle.kts";
const RUST_ANDROID: &str = "src/android.rs";

/// Every code the two sides share, by name.
const SHARED_CODES: &[&str] = &[
    "PENDING",
    "NO_ACTIVITY",
    "BUSY",
    "NOT_INITIALIZED",
    "LAUNCH_FAILED",
    "ERROR",
    "CANCELLED",
    "SAVED",
    "DISMISSED",
    "ACCESS_LOST",
    "TOO_LARGE",
    "GRANT_FAILED",
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

/// The text of the function declared at `marker` (Kotlin `fun` or Rust `fn`),
/// through its closing brace at the same indentation.
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

/// The source with `//` comments removed, so a scan sees code only.
fn code(contents: &str) -> String {
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
    let mut values: Vec<_> = SHARED_CODES.iter().map(|n| rust[*n]).collect();
    values.sort_unstable();
    values.dedup();
    assert_eq!(values.len(), SHARED_CODES.len(), "codes must be distinct");
    assert!(read(KOTLIN_HOST).contains("const val NO_VALUE = -1L"));
    assert!(read(RUST_ANDROID).contains("const NO_VALUE: jlong = -1;"));
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
    assert_eq!(package, "dev.frust.filedialogs");
    assert!(kotlin.contains("\nobject FrustFileDialogsHost {"));

    let fqcn = format!("{package}.FrustFileDialogsHost");
    assert!(
        rust.contains(&format!("const HOST_CLASS_BINARY: &str = \"{fqcn}\";")),
        "src/android.rs must load `{fqcn}`"
    );

    for decl in [
        "external fun nativeOnPaths(requestId: Long, paths: Array<String>)",
        "external fun nativeOnTree(requestId: Long, uri: String?)",
        "external fun nativeOnCode(requestId: Long, code: Int)",
        "external fun nativeOnProgress(requestId: Long, done: Int, items: Int, fraction: Double)",
        "external fun nativeOnListing(\n        requestId: Long,\n        names: Array<String>,\n        relativePaths: Array<String>,\n        sizes: LongArray,\n        modified: LongArray,\n        uris: Array<String>,\n    )",
    ] {
        assert!(
            kotlin.contains(&format!("@JvmStatic\n    {decl}")),
            "Kotlin declaration changed: `{decl}`"
        );
    }

    let prefix = format!("Java_{}_FrustFileDialogsHost_", package.replace('.', "_"));
    let exports: [(&str, &[&str]); 5] = [
        (
            "nativeOnPaths",
            &["request_id: jlong,", "paths: JObjectArray<'local>,"],
        ),
        (
            "nativeOnTree",
            &["request_id: jlong,", "uri: JString<'local>,"],
        ),
        (
            "nativeOnListing",
            &[
                "request_id: jlong,",
                "names: JObjectArray<'local>,",
                "relative_paths: JObjectArray<'local>,",
                "sizes: JLongArray<'local>,",
                "modified: JLongArray<'local>,",
                "uris: JObjectArray<'local>,",
            ],
        ),
        ("nativeOnCode", &["request_id: jlong,", "code: jint,"]),
        (
            "nativeOnProgress",
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
fn rust_calls_match_the_kotlin_declarations() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID);
    for (method, sig, decl) in [
        (
            "openDocuments",
            "(J[Ljava/lang/String;ZLjava/lang/String;)I",
            "fun openDocuments(requestId: Long, mimes: Array<String>, multiple: Boolean, destDir: String): Int",
        ),
        (
            "exportFile",
            "(JLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)I",
            "fun exportFile(requestId: Long, sourcePath: String, suggestedName: String, mime: String): Int",
        ),
        ("pickTree", "(J)I", "fun pickTree(requestId: Long): Int"),
        (
            "listTree",
            "(JLjava/lang/String;ZII[Ljava/lang/String;Z)I",
            "fun listTree(\n        requestId: Long,\n        uri: String,\n        recursive: Boolean,\n        maxDepth: Int,\n        maxEntries: Int,\n        extensions: Array<String>,\n        includeHidden: Boolean,\n    ): Int",
        ),
        (
            "resolveEntry",
            "(JLjava/lang/String;[Ljava/lang/String;)I",
            "fun resolveEntry(requestId: Long, treeUri: String, segments: Array<String>): Int",
        ),
        (
            "copyDocument",
            "(JLjava/lang/String;Ljava/lang/String;J)I",
            "fun copyDocument(requestId: Long, uri: String, destPath: String, maxBytes: Long): Int",
        ),
        ("cancel", "(J)V", "fun cancel(requestId: Long)"),
        (
            "isTreeGranted",
            "(Ljava/lang/String;)Z",
            "fun isTreeGranted(uri: String): Boolean",
        ),
        (
            "releaseTree",
            "(Ljava/lang/String;)V",
            "fun releaseTree(uri: String)",
        ),
        (
            "releaseNewTree",
            "(JLjava/lang/String;)V",
            "fun releaseNewTree(requestId: Long, uri: String)",
        ),
        (
            "mimesForExtensions",
            "([Ljava/lang/String;)[Ljava/lang/String;",
            "fun mimesForExtensions(extensions: Array<String>): Array<String>",
        ),
        (
            "mimeForName",
            "(Ljava/lang/String;)Ljava/lang/String;",
            "fun mimeForName(name: String): String",
        ),
        (
            "cacheRoot",
            "()Ljava/lang/String;",
            "fun cacheRoot(): String?",
        ),
    ] {
        assert!(
            kotlin.contains(&format!("@JvmStatic\n    {decl}")),
            "Kotlin `{method}` must be `@JvmStatic` and declared `{decl}`"
        );
        let call = format!("jni_str!(\"{method}\"),\n");
        let at = rust
            .find(&call)
            .unwrap_or_else(|| panic!("src/android.rs must call `{method}`"));
        let after = &rust[at + call.len()..];
        let line = after.lines().next().unwrap_or("").trim();
        assert_eq!(
            line,
            format!("jni_sig!(\"{sig}\"),"),
            "`{method}`'s JNI signature must match its Kotlin declaration"
        );
    }
}

#[test]
fn module_manifest_declares_no_permission_and_the_init_provider() {
    let manifest = read(MANIFEST);
    let without_comment = manifest
        .split_once("-->")
        .map_or(manifest.as_str(), |(_, rest)| rest);
    assert!(
        !without_comment.contains("uses-permission"),
        "the Storage Access Framework needs no permission; the module must declare none"
    );
    assert!(
        manifest.contains("android:name=\"dev.frust.filedialogs.FrustFileDialogsInitProvider\"")
    );
    assert!(manifest.contains("android:authorities=\"${applicationId}.frustfiledialogsinit\""));
    assert!(manifest.contains("android:exported=\"false\""));
    assert!(manifest.contains("android:multiprocess=\"false\""));
    assert!(
        read(KOTLIN_PROVIDER).contains("class FrustFileDialogsInitProvider : ContentProvider()")
    );
}

#[test]
fn keep_rules_cover_the_host_and_the_provider() {
    let rules = read(CONSUMER_RULES);
    for line in [
        "-keep class dev.frust.filedialogs.FrustFileDialogsHost { *; }",
        "-keep class dev.frust.filedialogs.FrustFileDialogsHost$* { *; }",
        "-keepclasseswithmembernames class dev.frust.filedialogs.FrustFileDialogsHost {",
        "-keep class dev.frust.filedialogs.FrustFileDialogsInitProvider { *; }",
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
        "namespace = \"dev.frust.filedialogs\"",
        "compileSdk = 36",
        "minSdk = 29",
        "consumerProguardFiles(\"consumer-rules.pro\")",
        "buildConfig = false",
        "sourceCompatibility = JavaVersion.VERSION_17",
        "targetCompatibility = JavaVersion.VERSION_17",
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
fn the_dialogs_use_the_saf_contracts_and_persist_a_read_grant() {
    let kotlin = read(KOTLIN_HOST);
    for contract in [
        "ActivityResultContracts.OpenDocument()",
        "ActivityResultContracts.OpenMultipleDocuments()",
        "ActivityResultContracts.CreateDocument(mime)",
        "ActivityResultContracts.OpenDocumentTree()",
    ] {
        assert!(kotlin.contains(contract), "the host must use `{contract}`");
    }
    let on_tree = function(&kotlin, "private fun onTree(");
    assert!(
        on_tree
            .contains("takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)"),
        "a picked tree must take a persisted read grant"
    );
    assert!(
        function(&kotlin, "fun isTreeGranted(").contains("persistedUriPermissions"),
        "the grant check reads the persisted permissions"
    );
    assert!(
        function(&kotlin, "fun releaseTree(").contains("releasePersistableUriPermission"),
        "releasing drops the persisted permission"
    );
    let mimes = function(&kotlin, "fun mimesForExtensions(");
    assert!(mimes.contains("MimeTypeMap.getSingleton()") && mimes.contains("ANY_MIME"));
}

#[test]
fn one_dialog_is_in_flight_at_a_time_launched_on_the_main_thread() {
    let kotlin = read(KOTLIN_HOST);
    for marker in ["fun openDocuments(", "fun exportFile(", "fun pickTree("] {
        let entry = function(&kotlin, marker);
        assert!(
            entry.contains("claimInFlight(") && entry.contains("return busy()"),
            "`{marker}` must claim the slot or answer BUSY"
        );
        assert!(
            entry.contains("mainHandler.post"),
            "`{marker}` must launch on the main thread"
        );
    }
    let claim = function(&kotlin, "private fun claimInFlight(");
    assert!(
        claim.contains("if (inFlight != null) return false"),
        "no takeover: a second dialog must be refused"
    );
    assert!(kotlin.contains("activity.activityResultRegistry.register(key, contract)"));
    assert!(kotlin.contains("const val LAUNCHER_KEY_PREFIX = \"frust.filedialogs.\""));
    let destroyed = function(&kotlin, "override fun onActivityDestroyed(");
    assert!(
        destroyed.contains("releaseInFlightFor(activity)") && destroyed.contains("answerDismissed"),
        "a destroyed Activity must answer its dialog"
    );
}

#[test]
fn the_rust_wait_has_no_deadline_and_every_blocking_call_refuses_the_ui_thread() {
    let rust = read(RUST_ANDROID);
    let wait = function(&rust, "fn next_event(");
    assert!(wait.contains(".wait_while("));
    assert!(!rust.contains("wait_timeout"), "the wait has no deadline");
    for marker in [
        "pub(crate) fn show(",
        "pub(crate) fn pick_folder(",
        "pub(crate) fn restore_folder(",
        "pub(crate) fn list_folder(",
        "pub(crate) fn resolve_entry(",
        "pub(crate) fn export_file(",
        "pub(crate) fn copy_content_uri(",
        "pub(crate) fn read_content_uri(",
    ] {
        assert!(
            function(&rust, marker).contains("refuse_ui_thread()?;"),
            "`{marker}` must refuse the UI thread"
        );
    }
    let show = function(&rust, "pub(crate) fn show(");
    assert!(
        show.contains("Kind::SaveFile") && show.contains("Err(FileDialogError::Unsupported)"),
        "save_file stays unsupported on Android"
    );
    assert!(
        function(&rust, "fn cancel_request(").contains("deliver(request_id, Answer::Cancelled);")
    );
    assert!(
        !function(&rust, "fn cancel_request(").contains("refuse_ui_thread"),
        "a token may fire from the UI thread"
    );
}

#[test]
fn cancel_ends_the_wait_and_stops_the_work() {
    let kotlin = read(KOTLIN_HOST);
    let rust = code(&read(RUST_ANDROID));
    let request = function(&rust, "fn run_request(");
    assert!(
        request.contains("token.set_hook(move || cancel_request(request_id));"),
        "the hook must reach the host and end the wait"
    );
    assert!(
        request.find("if code != PENDING").unwrap() < request.find("set_hook").unwrap(),
        "the hook is armed only after PENDING"
    );
    assert!(
        request.contains("token.clear_hook();"),
        "a reused token never fires a stale hook"
    );

    let cancel = function(&kotlin, "fun cancel(requestId: Long)");
    assert!(cancel.contains("cancelInFlight(requestId)"));
    assert!(cancel.contains("jobs[requestId]?.cancelled = true"));

    let cancel_in_flight = function(&kotlin, "private fun cancelInFlight(");
    assert!(cancel_in_flight.contains("current.answered = true"));
    assert!(
        !cancel_in_flight.contains("inFlight = null"),
        "the slot stays held until the dialog's result arrives"
    );

    let claim = function(&kotlin, "private fun claimResult(");
    assert!(
        claim.contains("if (current.cancelled) {") && claim.contains("inFlight = null"),
        "a cancelled dialog's result frees the slot and is ignored"
    );

    let chunks = function(&kotlin, "private fun streamChunks(");
    assert!(chunks.contains("if (isCancelled()) return CANCELLED"));
    assert!(kotlin.contains("const val COPY_CHUNK_BYTES = 64 * 1024"));

    let walk = function(&kotlin, "private fun walkTree(");
    assert!(walk.contains("if (job.cancelled) return CANCELLED"));

    let created = function(&kotlin, "private fun onCreated(");
    assert!(
        created.contains("removeCreated(ctx, uri, false)"),
        "a document created behind a cancelled dialog is removed"
    );
    let export = function(&kotlin, "private fun runExport(");
    assert!(export.contains("if (code != SAVED) removeCreated(ctx, uri, opened)"));
    let copy = function(&kotlin, "private fun runCopy(");
    assert!(copy.contains("if (code != SAVED) partial.delete()"));
    let opened = function(&kotlin, "private fun onOpened(");
    assert!(opened.contains("written.forEach { it.delete() }"));
    assert!(
        function(&kotlin, "private fun onTree(").contains("releaseTree(uri.toString())"),
        "a grant taken behind a cancel is released"
    );
}

#[test]
fn work_runs_on_the_executor_never_the_main_thread() {
    let kotlin = read(KOTLIN_HOST);
    for marker in [
        "fun listTree(",
        "fun resolveEntry(",
        "fun copyDocument(",
        "private fun onOpened(",
        "private fun onCreated(",
    ] {
        assert!(
            function(&kotlin, marker).contains("workExecutor.execute"),
            "`{marker}` must hand its work to the executor"
        );
    }
}

#[test]
fn a_dismissal_stays_distinct_from_a_cancel() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID);
    assert!(function(&kotlin, "private fun onCreated(").contains("DISMISSED"));
    assert!(
        function(&kotlin, "private fun onOpened(").contains("notifyPaths(requestId, emptyArray())")
    );
    assert!(function(&kotlin, "private fun onTree(").contains("notifyTree(requestId, null)"));
    assert!(
        function(&rust, "pub(crate) fn export_file(")
            .contains("Answer::Code(DISMISSED) => Ok(ExportOutcome::Cancelled),")
    );
    assert!(function(&rust, "fn error_for(").contains("CANCELLED => FileDialogError::Cancelled,"));
}

#[test]
fn progress_is_never_forwarded_under_a_lock() {
    let kotlin = read(KOTLIN_HOST);
    let rust = code(&read(RUST_ANDROID));
    assert!(function(&kotlin, "private fun notifyProgress(").contains("nativeOnProgress("));
    for marker in [
        "private fun claimInFlight(",
        "private fun cancelInFlight(",
        "private fun claimResult(",
        "private fun releaseFlight(",
        "private fun releaseInFlightFor(",
    ] {
        let body = function(&kotlin, marker);
        assert!(
            !body.contains("notify") && !body.contains("native"),
            "`{marker}` holds the monitor and must not call out"
        );
    }
    let record = function(&rust, "fn record_progress(");
    assert!(
        record.contains("drop(waits);") && !record.contains("throttle"),
        "the export stores the value and never runs the callback"
    );
    let wait = function(&rust, "fn await_answer(");
    assert!(
        wait.contains("next_event(request_id)") && wait.contains("throttle.report(progress);"),
        "the waiter forwards progress after the lock is released"
    );
}

#[test]
fn copies_stay_inside_the_call_directory() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID);
    let name = function(&kotlin, "internal fun safeCopyName(");
    assert!(
        name.contains("it == '/'") && name.contains("it == '\\\\'") && name.contains("\"..\""),
        "a provider's display name must not be able to leave the directory"
    );
    assert!(
        function(&rust, "fn open_into(").contains("if path.parent() != Some(dir) {"),
        "a host path outside the call directory is refused"
    );
    assert!(rust.contains("const CACHE_SUBDIR: &str = \"frust-file-dialogs\";"));
}

#[test]
fn folder_entries_are_content_uris_and_a_listing_answers_on_the_executor() {
    let rust = read(RUST_ANDROID);
    assert!(
        function(&rust, "fn folder_entry(")
            .contains("location: EntryLocation::ContentUri(entry.uri),")
    );
    assert!(function(&rust, "fn grant_for(").contains("local_path: None,"));
    let list = function(&rust, "pub(crate) fn list_folder(");
    assert!(
        list.contains("sort_by"),
        "a listing is sorted by relative path"
    );
}

#[test]
fn a_listing_skips_unsafe_names_and_bounds_its_walk() {
    let kotlin = read(KOTLIN_HOST);
    let walk = function(&kotlin, "private fun walkTree(");
    assert!(
        walk.contains("isSafeEntryName(child.name)"),
        "a provider's display name must be one safe component before it is listed"
    );
    let safe = function(&kotlin, "internal fun isSafeEntryName(");
    assert!(
        safe.contains("trimmed.isEmpty()")
            && safe.contains("trimmed == \".\"")
            && safe.contains("trimmed == \"..\"")
            && safe.contains("it == '/'")
            && safe.contains("it == '\\\\'")
            && safe.contains("it == '\\u0000'"),
        "empty, `.`, `..`, separators and NUL are refused"
    );
    assert!(
        !walk.contains("child.name} skipped")
            && function(&kotlin, "private fun truncatedVisit(")
                .contains("$skippedNames unsafe names"),
        "only a count of skipped names is logged"
    );
    assert!(
        walk.contains("query.maxEntries") && walk.contains("WALK_VISIT_MULTIPLE"),
        "the visit budget is a multiple of maxEntries"
    );
    assert!(
        walk.contains("if (seen >= budget) return truncatedVisit(skippedNames)"),
        "folders visited are bounded, not just entries returned"
    );
    assert!(
        walk.contains("readChildren(ctx, tree, dir.documentId, job, room)")
            && function(&kotlin, "private fun readChildren(").contains("children.size < limit"),
        "one folder's children are bounded too"
    );
}

#[test]
fn the_work_pool_is_bounded() {
    let kotlin = read(KOTLIN_HOST);
    assert!(kotlin.contains("Executors.newFixedThreadPool(WORK_THREADS)"));
    assert!(!kotlin.contains("newCachedThreadPool"));
    assert!(kotlin.contains("private const val WORK_THREADS = 4"));
}

#[test]
fn a_finished_export_stays_saved_through_a_cancel_race() {
    let rust = code(&read(RUST_ANDROID));
    let request = function(&rust, "fn run_request(");
    assert!(
        request.contains("&& !matches!(answer, Answer::Code(SAVED))"),
        "a cancel racing the host's SAVED answer must not turn it into Cancelled"
    );
    assert!(function(&rust, "pub(crate) fn export_file(").contains("Answer::Code(SAVED) => {"));
}

#[test]
fn a_cancel_race_releases_only_a_grant_the_request_newly_took() {
    let kotlin = read(KOTLIN_HOST);
    let rust = code(&read(RUST_ANDROID));
    let request = function(&rust, "fn run_request(");
    assert!(request.contains("release_new_tree(request_id, uri)"));
    assert!(
        !request.contains("release_tree("),
        "the late-result path never releases an older grant"
    );
    let tree = function(&kotlin, "private fun onTree(");
    assert!(tree.contains("val alreadyGranted = isTreeGranted(text)"));
    assert!(tree.contains("val newlyTaken = persisted && !alreadyGranted"));
    assert!(tree.contains("cleanup = { if (newlyTaken) releaseTree(uri.toString()) }"));
    assert!(tree.contains("TreeGrant(requestId, text)"));
    let release = function(&kotlin, "fun releaseNewTree(");
    assert!(release.contains("takeNewTreeGrant(requestId, uri)"));
    let take = function(&kotlin, "private fun takeNewTreeGrant(");
    assert!(
        take.contains("grant.requestId != requestId || grant.uri != uri"),
        "only the request that took the grant may release it"
    );
}

#[test]
fn a_refused_persisted_grant_is_a_distinct_payload_free_outcome() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID);
    let tree = function(&kotlin, "private fun onTree(");
    assert!(
        tree.contains("notifyCode(requestId, GRANT_FAILED)")
            && !tree.contains("notifyCode(requestId, ERROR)"),
        "a failed takePersistableUriPermission is neither a success nor a generic error"
    );
    let error = function(&rust, "fn error_for(");
    assert!(
        error.contains("GRANT_FAILED => {") && error.contains("FileDialogError::Platform"),
        "it surfaces as Platform with a logged reason"
    );
    assert!(
        !tree.contains("uri}") && !tree.contains("${e.message"),
        "the log carries the exception class only"
    );
}

#[test]
fn an_entry_lookup_resolves_one_listed_folder_per_segment() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID);
    assert!(
        rust.contains(
            "//! | `resolveEntry` | `(long requestId, String treeUri, String[] segments) -> int` |"
        ),
        "the Rust contract table names resolveEntry"
    );
    assert!(
        kotlin.contains(" * | Rust to Kotlin | `resolveEntry(requestId: Long, treeUri: String, segments: Array<String>): Int` |"),
        "the Kotlin contract table names resolveEntry"
    );

    let entry = function(&kotlin, "fun resolveEntry(");
    assert!(
        entry.contains("jobs[requestId] = job")
            && entry.contains(
                "workExecutor.execute { runResolve(ctx, requestId, treeUri, path, job) }"
            ),
        "a lookup is a job on the work executor, cancellable by request id"
    );

    let run = function(&kotlin, "private fun runResolve(");
    assert!(
        run.contains("if (!isTreeGranted(treeUri)) {") && run.contains("ACCESS_LOST"),
        "a tree without a persisted grant is ACCESS_LOST"
    );
    assert!(run.contains("catch (e: SecurityException) {\n            ACCESS_LOST"));
    assert!(run.contains("catch (e: Exception) {") && run.contains("ERROR"));
    assert!(run.contains("jobs.remove(requestId)"));
    assert!(
        run.contains("notifyListing(requestId, listing) else notifyCode(requestId, failure)"),
        "the lookup answers through the listing export or a code, exactly once"
    );

    let resolve = function(&kotlin, "private fun resolveTree(");
    assert!(resolve.contains("DocumentsContract.getTreeDocumentId(tree)"));
    assert!(
        resolve.contains("readChildren(ctx, tree, folder, job, Int.MAX_VALUE)")
            && function(&kotlin, "private fun readChildren(")
                .contains("DocumentsContract.buildChildDocumentsUriUsingTree(tree, documentId)"),
        "each segment lists its folder through readChildren"
    );
    assert!(
        resolve.contains("children.firstOrNull { it.name == segment && isResolvable(it, last) }"),
        "display names match by equality, first in name order"
    );
    assert!(
        resolve.contains("DocumentsContract.buildDocumentUriUsingTree(tree, child.documentId)"),
        "the result's uri is built from the provider's own document id"
    );
    assert!(resolve.contains("listing.relativePaths.add(segments.joinToString(\"/\"))"));
    assert!(resolve.contains("if (job.cancelled) return CANCELLED"));
    for line in code(resolve)
        .lines()
        .filter(|l| l.contains("documentId") || l.contains("folder"))
    {
        assert!(
            !line.contains('+') && !line.contains('$') && !line.contains("\"/\""),
            "a document id is opaque and never built from path text: `{}`",
            line.trim()
        );
    }

    let rules = function(&kotlin, "private fun isResolvable(");
    assert!(
        rules.contains("isSafeEntryName(child.name)")
            && rules.contains("child.name.startsWith(\".\")"),
        "the listing's name and hidden rules apply"
    );
    assert!(
        rules.contains("DocumentsContract.Document.MIME_TYPE_DIR")
            && rules.contains("DocumentsContract.Document.FLAG_VIRTUAL_DOCUMENT == 0"),
        "folders lead, a non-virtual file ends the path"
    );

    let rust_entry = function(&rust, "pub(crate) fn resolve_entry(");
    assert!(rust_entry.contains("let Answer::Listing(mut entries) = answer else {"));
    assert!(
        rust_entry.contains("Ok(Some(folder_entry(entry)))")
            && rust_entry.contains("entry.relative_path == relative_path"),
        "one entry for the asked path maps through folder_entry"
    );
}
