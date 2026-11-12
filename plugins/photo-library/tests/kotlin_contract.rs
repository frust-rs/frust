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
//! - progress is never forwarded under a lock on either side;
//! - library browsing: the JNI calls and result objects match their Kotlin
//!   declarations, every blocking call refuses the UI thread, browsing never
//!   takes the pick slot, paging and the access check keep their shape, and
//!   the asked-flag keys and permission table match `frust-permissions`;
//! - cursor paging: the `assetsAfter` call and its `AssetsAfterResult` match,
//!   the keyset query takes no count, orders by the sort key then `_ID`,
//!   selects the rows after the cursor's key and `_ID` and reads one row past
//!   the page, never seeks, and the offset path keeps its count and seek;
//! - lookup by id: the `assetsById` call and its `AssetsByIdResult` match, the
//!   Rust arm decodes the page as a listing does and puts the rows back by
//!   position, and the host rejects a non-`content://media` id and queries
//!   image and video rows in chunks of at most 500 bound ids;
//! - thumbnail cancel: the load gets a real `CancellationSignal` held under its
//!   request id, `cancelThumbnail` fires it, a failure after the cancel answers
//!   `CANCELLED`, and the Rust hook calls the entry and types the code;
//! - originals and saves: the host calls, exports and codes match, neither
//!   touches pick state or a pick's executor, the export copies in cancellable
//!   chunks and deletes its partial file, the location-preserving open falls
//!   back rather than failing, and the writer publishes or deletes every row;
//! - decode: the host calls and result objects match, the pixels cross as one
//!   direct `ByteBuffer` (never a Java array) that the host never keeps, the
//!   decode uses the software allocator, a target size and
//!   `copyPixelsToBuffer`, a format with no decoder is typed, the UI thread is
//!   refused first, and the module's one `unsafe` block carries its SAFETY note;
//! - metadata: the `assetMetadata` call and its result match, the host opens
//!   the asset with the export's own open and reuses the decoder's EXIF read
//!   (no tag read of its own), closes its stream, reports no GPS from a
//!   redacted read, and the Rust arm refuses the UI thread and types its codes;
//! - change feed: the snapshot and observer calls, the `ChangesResult` fields,
//!   the `nativeOnLibraryChanged` export and `NO_GENERATION` match; the
//!   snapshot refuses the UI thread, checks access, reads the position before
//!   the rows and queries the browser's own collection and selection; the
//!   observer covers both collections, wakes from every `onChange` overload
//!   with no lock held, and unregisters outside its lock.
//!
//! The change feed's platform-free core (`src/android_changes.rs` outside its
//! JNI arm: the token, the generation filter and the id-set diff) is included
//! below as a module, so its own unit tests run here on the host (precedent:
//! `crates/frust-shell-android/tests/adpf.rs`).

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
const KOTLIN_BROWSER: &str =
    "platform/android/src/main/kotlin/dev/frust/photolibrary/MediaStoreBrowser.kt";
const RUST_ANDROID_LIBRARY: &str = "src/android_library.rs";
const KOTLIN_PERMISSIONS: &str =
    "../permissions/platform/android/src/main/kotlin/dev/frust/permissions/FrustPermissionsHost.kt";
const KOTLIN_WRITER: &str =
    "platform/android/src/main/kotlin/dev/frust/photolibrary/MediaStoreWriter.kt";
const RUST_ANDROID_ORIGINALS: &str = "src/android_originals.rs";
const KOTLIN_DECODER: &str =
    "platform/android/src/main/kotlin/dev/frust/photolibrary/ImageDecodeHost.kt";
const RUST_ANDROID_DECODE: &str = "src/android_decode.rs";
const KOTLIN_METADATA: &str =
    "platform/android/src/main/kotlin/dev/frust/photolibrary/AssetMetadataHost.kt";
const RUST_ANDROID_METADATA: &str = "src/android_metadata.rs";
const KOTLIN_CHANGES: &str =
    "platform/android/src/main/kotlin/dev/frust/photolibrary/MediaStoreChanges.kt";
const RUST_ANDROID_CHANGES: &str = "src/android_changes.rs";

// The change feed's token and diff logic, run on the host; its JNI arm is Android-only.
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
#[path = "../src/android_changes.rs"]
mod android_changes;

/// How many leading [`SHARED_CODES`] are result codes (the rest are filters and kinds).
const RESULT_CODES: usize = 16;

/// Every code the two sides share, by name.
const SHARED_CODES: &[&str] = &[
    "PENDING",
    "NO_ACTIVITY",
    "BUSY",
    "NOT_INITIALIZED",
    "LAUNCH_FAILED",
    "COPY_FAILED",
    "CANCELLED",
    "OK",
    "PERMISSION_DENIED",
    "NOT_FOUND",
    "QUERY_FAILED",
    "NOT_LIMITED",
    "EXPORT_FAILED",
    "SAVE_FAILED",
    "NO_DECODER",
    "DECODE_FAILED",
    "FILTER_IMAGES",
    "FILTER_VIDEOS",
    "FILTER_IMAGES_AND_VIDEOS",
    "KIND_IMAGE",
    "KIND_VIDEO",
    "COLOR_SRGB",
    "COLOR_DISPLAY_P3",
    "COLOR_OTHER",
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
    let mut rust = rust_jint_consts(&read(RUST_ANDROID));
    rust.extend(rust_jint_consts(&read(RUST_ANDROID_LIBRARY)));
    rust.extend(rust_jint_consts(&read(RUST_ANDROID_ORIGINALS)));
    rust.extend(rust_jint_consts(&read(RUST_ANDROID_DECODE)));
    rust.extend(rust_jint_consts(&read(RUST_ANDROID_METADATA)));
    // The originals, decode, metadata and change modules repeat some codes; `extend` would mask
    // a drift.
    for file in [
        RUST_ANDROID_ORIGINALS,
        RUST_ANDROID_DECODE,
        RUST_ANDROID_METADATA,
        RUST_ANDROID_CHANGES,
    ] {
        for (name, value) in rust_jint_consts(&read(file)) {
            assert_eq!(
                kotlin.get(&name),
                Some(&value),
                "{file}'s `{name}` must equal the Kotlin host's"
            );
        }
    }
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

/// `listOf("a", "b")` argument text of the first declaration line containing `decl`.
fn list_literal<'a>(contents: &'a str, decl: &str) -> &'a str {
    let line = contents
        .lines()
        .find(|line| line.contains(decl))
        .unwrap_or_else(|| panic!("source has no `{decl}`"));
    let start = line.find("listOf(").expect("a listOf declaration") + "listOf(".len();
    let end = line.rfind(')').expect("a closed listOf");
    &line[start..end]
}

#[test]
fn browse_jni_calls_match_the_kotlin_declarations() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_LIBRARY);
    for (decl, name, sig) in [
        (
            "@JvmStatic\n    fun albums(): AlbumsResult",
            "albums",
            "()Ldev/frust/photolibrary/FrustPhotoLibraryHost$AlbumsResult;",
        ),
        (
            "@JvmStatic\n    fun assets(albumId: String?, filter: Int, offset: Long, limit: Int, details: Boolean): AssetsResult",
            "assets",
            "(Ljava/lang/String;IJIZ)Ldev/frust/photolibrary/FrustPhotoLibraryHost$AssetsResult;",
        ),
        (
            "@JvmStatic\n    fun thumbnail(requestId: Long, id: String, width: Int, height: Int, fill: Boolean): ThumbnailResult",
            "thumbnail",
            "(JLjava/lang/String;IIZ)Ldev/frust/photolibrary/FrustPhotoLibraryHost$ThumbnailResult;",
        ),
        (
            "@JvmStatic\n    fun cancelThumbnail(requestId: Long) {",
            "cancelThumbnail",
            "(J)V",
        ),
        (
            "@JvmStatic\n    fun manageLimitedSelection(requestId: Long): Int",
            "manageLimitedSelection",
            "(J)I",
        ),
        (
            "@JvmStatic\n    fun assetsById(ids: Array<String>, details: Boolean): AssetsByIdResult",
            "assetsById",
            "([Ljava/lang/String;Z)Ldev/frust/photolibrary/FrustPhotoLibraryHost$AssetsByIdResult;",
        ),
    ] {
        assert!(kotlin.contains(decl), "Kotlin host must declare `{decl}`");
        let at = rust
            .find(&format!("jni_str!(\"{name}\")"))
            .unwrap_or_else(|| panic!("src/android_library.rs never calls `{name}`"));
        let window = &rust[at..(at + 300).min(rust.len())];
        assert!(
            window.contains(&format!("\"{sig}\"")),
            "src/android_library.rs must call `{name}` with `{sig}`"
        );
    }
}

/// `@JvmField val <name>: <kotlin type>` lines of the Kotlin class declared by
/// `class_marker`, up to the `)` that closes its constructor.
fn jvm_fields(contents: &str, class_marker: &str) -> Vec<(String, String)> {
    let start = contents
        .find(class_marker)
        .unwrap_or_else(|| panic!("source has no `{class_marker}`"));
    let end = start
        + contents[start..]
            .find("\n    )")
            .unwrap_or_else(|| panic!("`{class_marker}` constructor is not closed"));
    contents[start..end]
        .lines()
        .filter_map(|line| line.trim().strip_prefix("@JvmField val "))
        .map(|field| {
            let (name, ty) = field
                .trim_end_matches(',')
                .split_once(": ")
                .expect("a typed field");
            (name.to_string(), ty.to_string())
        })
        .collect()
}

#[test]
fn browse_result_fields_match_their_rust_reads() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_LIBRARY);
    let reads = [
        (
            "class AlbumsResult(",
            vec![
                ("code", "Int", "int_field"),
                ("ids", "Array<String>", "string_array_field"),
                ("titles", "Array<String?>", "string_array_field"),
                ("counts", "LongArray", "long_array_field"),
            ],
        ),
        (
            "class AssetsResult(",
            vec![
                ("code", "Int", "int_field"),
                ("total", "Long", "get_field"),
                ("ids", "Array<String>", "string_array_field"),
                ("kinds", "IntArray", "int_array_field"),
                ("widths", "IntArray", "int_array_field"),
                ("heights", "IntArray", "int_array_field"),
                ("dates", "LongArray", "long_array_field"),
                ("durations", "LongArray", "long_array_field"),
                ("modified", "LongArray", "long_array_field"),
                ("favorites", "IntArray", "int_array_field"),
                ("names", "Array<String?>", "string_array_field"),
                ("types", "Array<String?>", "string_array_field"),
            ],
        ),
        (
            "class ThumbnailResult(",
            vec![
                ("code", "Int", "int_field"),
                ("width", "Int", "int_field"),
                ("height", "Int", "int_field"),
                ("rowBytes", "Int", "int_field"),
                ("pixels", "ByteArray?", "get_field"),
            ],
        ),
    ];
    for (class, fields) in reads {
        let declared = jvm_fields(&kotlin, class);
        let expected: Vec<_> = fields
            .iter()
            .map(|(name, ty, _)| (name.to_string(), ty.to_string()))
            .collect();
        assert_eq!(declared, expected, "`{class}` fields changed");
        for (name, _, reader) in fields {
            let read = if reader == "get_field" {
                format!(".get_field(&result, jni_str!(\"{name}\")")
            } else {
                format!("{reader}(env, &result, jni_str!(\"{name}\"))")
            };
            assert!(
                rust.contains(&read),
                "no `{read}` in src/android_library.rs"
            );
        }
    }
    assert!(
        rust.contains(".get_field(&result, jni_str!(\"total\"), jni_sig!(\"J\"))"),
        "`total` is a Long"
    );
    assert!(
        rust.contains(".get_field(&result, jni_str!(\"pixels\"), jni_sig!(\"[B\"))"),
        "`pixels` is a byte array"
    );
    let strings = kotlin_string_consts(&kotlin);
    assert_eq!(
        strings.get("ALL_MEDIA_ALBUM_ID").map(String::as_str),
        Some("frust.all_media")
    );
    assert!(rust.contains("const ALL_MEDIA_ALBUM_ID: &str = \"frust.all_media\";"));
    assert!(
        read(RUST_ANDROID).contains("#[path = \"android_library.rs\"]\nmod android_library;"),
        "src/android.rs declares the browsing arm"
    );
}

#[test]
fn every_blocking_library_call_refuses_the_ui_thread() {
    let rust = rust_code(&read(RUST_ANDROID_LIBRARY));
    for name in [
        "pub(crate) fn albums(",
        "pub(crate) fn assets(",
        "pub(crate) fn assets_by_id(",
        "pub(crate) fn thumbnail(",
    ] {
        let body = function(&rust, name);
        let ui = body
            .find("ensure_off_ui_thread(env)?;")
            .unwrap_or_else(|| panic!("`{name}` must refuse the UI thread"));
        let call = body.find("call_static_method").unwrap();
        assert!(
            ui < call,
            "`{name}` checks the UI thread before the host call"
        );
        assert!(
            !body.contains("PickGuard"),
            "`{name}` is read-only and never takes the pick guard"
        );
    }
    let manage = function(&rust, "pub(crate) fn manage_limited_selection(");
    let ui = manage
        .find("ensure_off_ui_thread(env)")
        .expect("manage_limited_selection refuses the UI thread");
    let guard = manage
        .find("let _one_at_a_time = pick_cache::PickGuard::acquire()?;")
        .expect("manage_limited_selection takes the pick guard in a named guard");
    let host = manage.find("jni_str!(\"manageLimitedSelection\")").unwrap();
    assert!(ui < guard && guard < host);
    assert!(manage.contains("await_answer(request_id, None)"));

    let browser = read(KOTLIN_BROWSER);
    assert!(
        !browser.contains("claimInFlight"),
        "browsing never takes the pick slot"
    );
    let kotlin = read(KOTLIN_HOST);
    let manage = function(
        &kotlin,
        "fun manageLimitedSelection(requestId: Long): Int {",
    );
    assert!(manage.contains("if (!MediaStoreBrowser.isLimited(ctx)) return NOT_LIMITED"));
    assert!(
        manage.contains("if (!claimInFlight(requestId, activity)) {")
            && manage.contains("return BUSY"),
        "the selection prompt shares the pick's single in-flight slot"
    );
    assert!(manage.contains("mainHandler.post { launchSelectionPrompt("));
    let launch = function(&kotlin, "private fun launchSelectionPrompt(");
    assert!(launch.contains("val key = LAUNCHER_KEY_PREFIX + requestId"));
    assert!(launch.contains("ActivityResultContracts.RequestMultiplePermissions()"));
    assert!(launch.contains("MediaStoreBrowser.recordAsked(ctx, results.keys)"));
    assert!(
        launch.contains("notifyCancelled(requestId)"),
        "the prompt's close answers through the pick relay"
    );
}

#[test]
fn browsing_checks_access_and_pages_by_sdk() {
    let browser = read(KOTLIN_BROWSER);
    let guard = function(&browser, "private inline fun <T> browse(");
    assert!(
        guard.contains("if (ctx == null) return failed(FrustPhotoLibraryHost.NOT_INITIALIZED)")
    );
    assert!(
        guard.contains(
            "if (!hasAccess(ctx)) return failed(FrustPhotoLibraryHost.PERMISSION_DENIED)"
        )
    );
    assert!(guard.contains("catch (e: SecurityException)"));
    let page = function(&browser, "private fun queryPage(");
    assert!(page.contains("if (Build.VERSION.SDK_INT < 30) {"));
    assert!(page.contains("ContentResolver.QUERY_ARG_OFFSET"));
    assert!(page.contains("ContentResolver.QUERY_ARG_LIMIT"));
    let rows = function(&browser, "private fun readRows(");
    assert!(rows.contains("val first = if (Build.VERSION.SDK_INT < 30) offset else 0"));
    assert!(rows.contains("cursor.moveToPosition(first)"));
    assert!(
        browser.contains("MediaStore.Video.Media.EXTERNAL_CONTENT_URI"),
        "asset ids are typed collection URIs, which loadThumbnail resolves on every API level"
    );
    let thumb = function(&browser, "private fun loadThumbnail(");
    assert!(thumb.contains("loadThumbnail(uri, Size(width, height), signal)"));
    assert!(thumb.contains("catch (e: FileNotFoundException)"));
    assert!(thumb.contains("Bitmap.Config.ARGB_8888"));
    assert!(thumb.contains("copyPixelsToBuffer"));

    let rust = rust_code(&read(RUST_ANDROID_LIBRARY));
    let thumbnail_from = function(&rust, "fn thumbnail_from(");
    assert!(
        thumbnail_from.contains("pixels::unpremultiply_rgba8_in_place(&mut rgba);"),
        "the host's premultiplied rows become straight alpha"
    );
    let error = function(&rust, "fn browse_error(");
    assert!(error.contains("PERMISSION_DENIED => PhotoError::PermissionDenied,"));
    assert!(error.contains("NOT_FOUND => PhotoError::NotFound,"));
}

#[test]
fn a_thumbnail_load_is_cancellable_on_both_sides() {
    let browser = read(KOTLIN_BROWSER);
    let code = rust_code(&browser);
    assert!(
        !code.contains("loadThumbnail(uri, Size(width, height), null)"),
        "loadThumbnail never runs without a CancellationSignal"
    );
    assert!(browser.contains("import android.os.CancellationSignal"));
    let entry = function(&browser, "fun thumbnail(ctx: Context?, requestId: Long,");
    assert!(entry.contains("val signal = CancellationSignal()"));
    assert!(entry.contains("thumbnailSignals[requestId] = signal"));
    assert!(
        entry.contains("} finally {") && entry.contains("thumbnailSignals.remove(requestId)"),
        "a finished load drops its signal, so a late cancel finds nothing"
    );
    let load = function(&browser, "private fun loadThumbnail(");
    assert!(load.contains("loadThumbnail(uri, Size(width, height), signal)"));
    assert!(
        load.contains("catch (e: OperationCanceledException) {")
            && load.contains("thumbnailFailed(FrustPhotoLibraryHost.CANCELLED)"),
        "the cancel exception answers CANCELLED"
    );
    assert!(
        load.contains("thumbnailFailed(cancelledOr(signal, FrustPhotoLibraryHost.NOT_FOUND))"),
        "a missing-file failure after a cancel is a cancel"
    );
    assert!(load.contains(
        "if (signal.isCanceled) return@browse thumbnailFailed(FrustPhotoLibraryHost.CANCELLED)"
    ));
    let cancel = function(&browser, "fun cancelThumbnail(requestId: Long) {");
    assert!(cancel.contains("thumbnailSignals[requestId]?.cancel()"));

    let kotlin = rust_code(&read(KOTLIN_HOST));
    let at = kotlin
        .find("fun cancelThumbnail(requestId: Long) {")
        .unwrap();
    assert!(
        kotlin[at..(at + 200).min(kotlin.len())]
            .contains("MediaStoreBrowser.cancelThumbnail(requestId)"),
        "the host's cancel entry forwards to the browser"
    );
    let at = kotlin.find("fun thumbnail(requestId: Long,").unwrap();
    assert!(
        kotlin[at..(at + 300).min(kotlin.len())].contains(
            "MediaStoreBrowser.thumbnail(appContext, requestId, id, width, height, fill)"
        )
    );
    let host = read(KOTLIN_HOST);
    assert!(host.contains("`cancelThumbnail(requestId: Long)` — `(J)V`"));

    let rust = rust_code(&read(RUST_ANDROID_LIBRARY));
    let arm = function(&rust, "pub(crate) fn thumbnail(");
    let hook = arm
        .find("token.set_hook(move || cancel_thumbnail(request_id));")
        .expect("the token's hook cancels this request");
    let call = arm.find("jni_str!(\"thumbnail\")").unwrap();
    let clear = arm
        .find("token.clear_hook();")
        .expect("the hook is cleared");
    assert!(hook < call && call < clear);
    assert!(arm.contains("JValue::Long(request_id as jlong),"));
    let hook = function(&rust, "fn cancel_thumbnail(");
    assert!(hook.contains("jni_str!(\"cancelThumbnail\")"));
    let error = function(&rust, "fn browse_error(");
    assert!(error.contains("CANCELLED => PhotoError::Cancelled,"));
}

#[test]
fn asset_detail_columns_are_projected_by_api_level_and_details() {
    let browser = read(KOTLIN_BROWSER);
    let base = browser
        .split("private val ASSET_PROJECTION")
        .nth(1)
        .and_then(|rest| rest.split("private val DETAIL_COLUMNS").next())
        .expect("ASSET_PROJECTION");
    assert!(base.contains("MediaStore.MediaColumns.DATE_MODIFIED,"));
    assert!(
        !base.contains("IS_FAVORITE")
            && !base.contains("DISPLAY_NAME")
            && !base.contains("MIME_TYPE"),
        "the always-projected columns hold only what API 29 has and no detail column"
    );
    let detail = browser
        .split("private val DETAIL_COLUMNS")
        .nth(1)
        .and_then(|rest| rest.split("private fun projection").next())
        .expect("DETAIL_COLUMNS");
    assert!(detail.contains("MediaStore.MediaColumns.DISPLAY_NAME,"));
    assert!(detail.contains("MediaStore.MediaColumns.MIME_TYPE,"));
    let projection = function(&browser, "private fun projection(");
    assert!(
        projection.contains(
            "if (Build.VERSION.SDK_INT >= 30) columns += MediaStore.MediaColumns.IS_FAVORITE"
        ),
        "IS_FAVORITE is an API 30 column"
    );
    assert!(
        projection.contains("if (details) columns += DETAIL_COLUMNS"),
        "name and type are projected only with details"
    );
    let page = function(&browser, "private fun queryPage(");
    assert!(page.contains("val projection = projection(details)"));
    assert!(!page.contains("ASSET_PROJECTION"));
    let rows = function(&browser, "private fun readRows(");
    assert!(rows.contains("cursor.getColumnIndex(MediaStore.MediaColumns.IS_FAVORITE)"));
    assert!(rows.contains("if (favoriteAt < 0) FAVORITE_UNKNOWN"));
    assert!(rows.contains("val nameAt = if (details)"));
    assert!(rows.contains("val typeAt = if (details)"));
    let rust = rust_code(&read(RUST_ANDROID_LIBRARY));
    assert!(function(&rust, "pub(crate) fn assets(").contains("JValue::Bool(details)"));
    assert!(function(&rust, "fn assets_from(").contains("burst_id: None,"));
}

#[test]
fn sort_key_is_the_reported_timestamp_and_checks_are_strict() {
    let browser = read(KOTLIN_BROWSER);
    let rows = function(&browser, "private fun readRows(");
    assert!(
        rows.contains("date = if (taken > 0) taken else cursor.getLong(5) * 1000,"),
        "the reported timestamp rule the sort key must mirror"
    );
    let sort = browser
        .split("private const val SORT_ORDER =")
        .nth(1)
        .and_then(|rest| rest.split("private val ASSET_PROJECTION").next())
        .expect("SORT_ORDER");
    for part in [
        "CASE WHEN ${MediaStore.MediaColumns.DATE_TAKEN} > 0 THEN ${MediaStore.MediaColumns.DATE_TAKEN} ",
        "ELSE ${MediaStore.MediaColumns.DATE_ADDED} * 1000 END DESC, ${BaseColumns._ID} DESC",
    ] {
        assert!(sort.contains(part), "sort key part missing: {part}");
    }

    let rust = rust_code(&read(RUST_ANDROID_LIBRARY));
    let albums = function(&rust, "fn albums_from(");
    assert!(
        albums.contains("let Some(id) = id else {") && albums.contains("mismatched_arrays()"),
        "a null album id is a protocol violation, as for assets"
    );
    assert!(!albums.contains("filter_map"));
    let thumbnail_from = function(&rust, "fn thumbnail_from(");
    assert!(thumbnail_from.contains("checked_mul(4)"));
    assert!(thumbnail_from.contains("stride.checked_mul(h as usize)"));
}

#[test]
fn asked_flag_keys_and_permissions_match_frust_permissions() {
    let browser = read(KOTLIN_BROWSER);
    let permissions = read(KOTLIN_PERMISSIONS);
    let ours = kotlin_string_consts(&browser);
    let theirs = kotlin_string_consts(&permissions);
    for name in ["PREFS_FILE", "ASKED_KEY_PREFIX"] {
        assert!(
            ours.contains_key(name),
            "MediaStoreBrowser has no `const val {name}`"
        );
        assert_eq!(
            ours.get(name),
            theirs.get(name),
            "`{name}` must equal frust-permissions'"
        );
    }
    assert!(
        function(&browser, "internal fun recordAsked(")
            .contains("editor.putBoolean(ASKED_KEY_PREFIX + it, true)"),
        "the asked flag is the same key and value frust-permissions reads"
    );
    for (ours_decl, theirs_decl) in [
        ("MEDIA_PERMISSIONS_SDK29 =", "PHOTOS_READ_WRITE_SDK29 ="),
        ("MEDIA_PERMISSIONS_SDK33 =", "PHOTOS_READ_WRITE_SDK33 ="),
        ("MEDIA_PERMISSIONS_SDK34 =", "PHOTOS_READ_WRITE_SDK34 ="),
    ] {
        assert_eq!(
            list_literal(&browser, ours_decl),
            list_literal(&permissions, theirs_decl),
            "the media permission table must match frust-permissions' `{theirs_decl}`"
        );
    }
}

#[test]
fn originals_jni_calls_match_the_kotlin_declarations() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_ORIGINALS);
    for (decl, name, sig) in [
        (
            "@JvmStatic\n    fun exportOriginal(requestId: Long, uri: String, destPath: String): Int",
            "exportOriginal",
            "(JLjava/lang/String;Ljava/lang/String;)I",
        ),
        (
            "@JvmStatic\n    fun cancelExport(requestId: Long) {",
            "cancelExport",
            "(J)V",
        ),
        (
            "@JvmStatic\n    fun saveToLibrary(requestId: Long, sourcePath: String, isVideo: Boolean, displayName: String, album: String?): Int",
            "saveToLibrary",
            "(JLjava/lang/String;ZLjava/lang/String;Ljava/lang/String;)I",
        ),
    ] {
        assert!(kotlin.contains(decl), "Kotlin host must declare `{decl}`");
        let at = rust
            .find(&format!("jni_str!(\"{name}\")"))
            .unwrap_or_else(|| panic!("src/android_originals.rs never calls `{name}`"));
        let window = &rust[at..(at + 200).min(rust.len())];
        assert!(
            window.contains(&format!("jni_sig!(\"{sig}\")")),
            "src/android_originals.rs must call `{name}` with `{sig}`"
        );
    }
    assert!(
        read(RUST_ANDROID).contains("#[path = \"android_originals.rs\"]\nmod android_originals;"),
        "src/android.rs declares the originals arm"
    );
}

#[test]
fn originals_exports_match_their_kotlin_externals() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_ORIGINALS);
    let prefix = "Java_dev_frust_photolibrary_FrustPhotoLibraryHost_";
    let exports: [(&str, &str, &[&str]); 3] = [
        (
            "nativeOnExportProgress",
            "(requestId: Long, done: Long, total: Long)",
            &["request_id: jlong,", "done: jlong,", "total: jlong,"],
        ),
        (
            "nativeOnExportDone",
            "(requestId: Long, code: Int, mime: String?, name: String?)",
            &[
                "request_id: jlong,",
                "code: jint,",
                "mime: JString<'local>,",
                "name: JString<'local>,",
            ],
        ),
        (
            "nativeOnSaveDone",
            "(requestId: Long, code: Int, uri: String?)",
            &["request_id: jlong,", "code: jint,", "uri: JString<'local>,"],
        ),
    ];
    for (method, kotlin_params, params) in exports {
        assert!(
            kotlin.contains(&format!(
                "@JvmStatic\n    external fun {method}{kotlin_params}"
            )),
            "Kotlin `{method}` signature changed"
        );
        let symbol = format!("{prefix}{method}");
        let decl = format!("pub extern \"system\" fn {symbol}<");
        assert!(
            rust.contains(&decl),
            "src/android_originals.rs must export `{symbol}`"
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
    let rust_table = read(RUST_ANDROID);
    for member in [
        "exportOriginal",
        "cancelExport",
        "saveToLibrary",
        "nativeOnExportProgress",
        "nativeOnExportDone",
        "nativeOnSaveDone",
    ] {
        assert!(
            rust_table.contains(member),
            "src/android.rs's contract table must list `{member}`"
        );
        assert!(
            kotlin.contains(&format!("`{member}(")),
            "the Kotlin host's contract table must list `{member}`"
        );
    }
}

#[test]
fn originals_refuse_the_ui_thread_and_never_touch_pick_state() {
    let rust = rust_code(&read(RUST_ANDROID_ORIGINALS));
    assert!(
        !rust.contains("PickGuard"),
        "an export or save never takes the pick guard"
    );
    assert!(
        !rust.contains("super::WAITS") && !rust.contains("super::await_answer"),
        "export and save wait on their own request map"
    );
    for name in ["pub(crate) fn export_original(", "pub(crate) fn save("] {
        let body = function(&rust, name);
        let ui = body
            .find("ensure_off_ui_thread(env)")
            .unwrap_or_else(|| panic!("`{name}` must refuse the UI thread"));
        assert!(
            ui < body.find("request_id").expect("the call uses a request id"),
            "`{name}` checks the UI thread before minting a request"
        );
    }
    let kotlin = read(KOTLIN_HOST);
    let export = function(&kotlin, "fun exportOriginal(");
    assert!(export.contains("exports[requestId] = flight"));
    assert!(
        export.contains("exportExecutor.execute {"),
        "exports run on their own executor, never the pick's copy executor"
    );
    for pick_state in ["claimInFlight", "inFlight", "copyExecutor"] {
        assert!(
            !export.contains(pick_state),
            "exportOriginal must not use `{pick_state}`"
        );
    }
    assert!(
        kotlin.contains("Thread(runnable, \"frust-photo-library-export\")"),
        "the export executor is its own thread"
    );
    let save = function(&kotlin, "fun saveToLibrary(");
    assert!(!save.contains("InFlight") && !save.contains("copyExecutor"));
    let writer = read(KOTLIN_WRITER);
    assert!(
        !writer.contains("claimInFlight") && !writer.contains("copyExecutor"),
        "the writer never touches pick state"
    );
    assert!(writer.contains("Thread(runnable, \"frust-photo-library-save\")"));
}

#[test]
fn export_copies_in_cancellable_chunks_and_removes_partials() {
    let kotlin = read(KOTLIN_HOST);
    assert!(kotlin.contains("const val EXPORT_CHUNK_BYTES = 1024 * 1024"));
    let copy = function(&kotlin, "private fun copyOriginal(");
    let before_open = copy
        .find("if (flight.cancelled.get()) return CANCELLED")
        .unwrap();
    assert!(
        before_open < copy.find("openOriginal(ctx, uri)").unwrap(),
        "a cancelled export opens nothing"
    );
    let open_fails = copy
        .find("catch (e: FileNotFoundException)")
        .expect("a stale id answers NOT_FOUND from the open alone");
    let dest = copy.find("FileOutputStream(dest)").unwrap();
    assert!(
        open_fails < dest,
        "a missing destination is not mistaken for a stale asset"
    );
    assert!(copy.contains("ByteArray(EXPORT_CHUNK_BYTES)"));
    let body = &copy[dest..];
    let check = body
        .find("if (flight.cancelled.get()) return CANCELLED")
        .expect("the flag is checked in the loop");
    let read_at = body.find("source.read(buffer)").unwrap();
    assert!(check < read_at, "the flag is checked before every chunk");

    let run = function(&kotlin, "private fun runExport(");
    assert!(run.contains("if (!flight.started.compareAndSet(false, true)) return"));
    let delete = run
        .find("if (code != OK) dest.delete()")
        .expect("a cancelled or failed export deletes its partial file");
    let answer = run.find("notifyExportDone(").unwrap();
    assert!(
        delete < answer,
        "the partial file is gone before the answer"
    );
    assert!(run.contains("catch (e: SecurityException)"));
    assert!(run.contains("catch (e: Throwable)"));

    let cancel = function(&kotlin, "fun cancelExport(requestId: Long) {");
    assert!(cancel.contains("flight.cancelled.set(true)"));
    assert!(
        cancel.contains("if (flight.started.compareAndSet(false, true)) {")
            && cancel.contains("notifyExportDone(requestId, CANCELLED, null, null)"),
        "a cancel that wins the race against a queued export answers it"
    );
}

#[test]
fn export_names_are_sanitised_in_rust_and_never_replace_a_file() {
    let rust = rust_code(&read(RUST_ANDROID_ORIGINALS));
    assert!(rust.contains("const STAGING_PREFIX: &str = \".export-\";"));
    let finish = function(&rust, "fn finish_export(");
    let claimed = finish.find("claim_final_path(").unwrap();
    let rename = finish.find("fs::rename(staging, &path)").unwrap();
    assert!(claimed < rename);
    let claim = function(&rust, "fn claim_final_path(");
    assert!(claim.contains("pick_cache::copy_name("));
    assert!(
        claim.contains("create_new(true)"),
        "the name is reserved atomically, never replacing an earlier export"
    );
    assert!(claim.contains("io::ErrorKind::AlreadyExists => continue"));

    let kotlin = read(KOTLIN_HOST);
    assert!(
        !kotlin.contains("File(destPath, "),
        "the host writes only the path Rust chose"
    );
    let export = function(&rust, "pub(crate) fn export_original(");
    assert!(export.contains("token.clear_hook();"));
    assert!(
        export.contains("remove_file(&staging)"),
        "any failure removes the staging file"
    );
    assert!(
        function(&rust, "fn ensure_pending(").contains("remove(&request_id)"),
        "a refused request drops its slot"
    );
}

#[test]
fn cancel_reaches_the_host_and_export_progress_stays_off_the_lock() {
    let rust = rust_code(&read(RUST_ANDROID_ORIGINALS));
    assert!(
        !rust.contains("wait_timeout"),
        "the host answers every PENDING request exactly once, so the wait has no deadline"
    );
    let run = function(&rust, "fn run_export(");
    let pending = run.find("ensure_pending(request_id, code)?").unwrap();
    let hook = run
        .find("token.set_hook(move || cancel_export(request_id));")
        .expect("the export installs its cancel hook");
    assert!(
        pending < hook,
        "the hook is installed only once the host answered PENDING"
    );
    let raced = run.find("if originals::is_cancelled(options) {").unwrap();
    let finish = run.find("finish_export(id, staging, answer)").unwrap();
    assert!(raced < finish, "a cancel that raced the answer still wins");
    assert!(run.contains("options.progress.clone().map(originals::throttled)"));
    assert!(
        run.contains("originals::report_fraction(throttle, 0.0);")
            && run.contains("originals::report_fraction(throttle, 1.0);"),
        "the first and final deliveries are explicit"
    );
    let hook = function(&rust, "fn cancel_export(");
    assert!(hook.contains("jni_str!(\"cancelExport\")"));

    let export = function(
        &rust,
        "pub extern \"system\" fn Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnExportProgress<",
    );
    assert!(export.contains("record_progress(request_id as u64, (done, total));"));
    let record = function(&rust, "fn record_progress(");
    assert!(record.contains("ANSWERED.notify_all();"));
    let event = function(&rust, "fn next_event(");
    for body in [export, record, event] {
        assert!(
            !body.contains(".report(") && !body.contains("report_fraction"),
            "the JNI thread and the locked wait never run the caller's callback"
        );
    }
    let await_answer = function(&rust, "fn await_answer(");
    assert!(await_answer.contains("next_event(request_id)"));
    assert!(await_answer.contains("forward_progress(throttle, progress);"));
    assert!(!await_answer.contains("lock("));
    let forward = function(&rust, "fn forward_progress(");
    assert!(
        forward.contains("min(MAX_COPY_FRACTION)"),
        "1.0 is reserved for the final delivery on success"
    );
    assert!(
        forward.contains("if total > 0 {"),
        "an unknown total reports nothing mid-copy"
    );

    let kotlin = read(KOTLIN_HOST);
    assert_eq!(
        kotlin
            .matches("nativeOnExportDone(requestId, code, mime, name)")
            .count(),
        1,
        "the export's answer reaches Rust only through notifyExportDone"
    );
    let copy = function(&kotlin, "private fun copyOriginal(");
    assert!(
        copy.contains("if (total != null) notifyExportProgress(requestId, done, total)"),
        "progress is per chunk and only when the size is known"
    );
    for marker in [
        "private fun runExport(",
        "private fun copyOriginal(",
        "private fun notifyExportDone(",
        "private fun notifyExportProgress(",
    ] {
        let at = kotlin.find(marker).unwrap();
        assert!(
            !kotlin[..at].trim_end().ends_with("@Synchronized"),
            "`{marker}` must not hold the host's monitor across a native call"
        );
    }
}

#[test]
fn the_original_open_prefers_unredacted_bytes_and_never_fails_over_location() {
    let kotlin = read(KOTLIN_HOST);
    assert!(
        kotlin.contains(
            "private fun openOriginal(ctx: Context, uri: Uri): InputStream = openOriginalStream(ctx, uri).stream"
        ),
        "the export opens through the shared open"
    );
    let open = function(&kotlin, "internal fun openOriginalStream(");
    let gate = open
        .find("ctx.checkSelfPermission(Manifest.permission.ACCESS_MEDIA_LOCATION)")
        .expect("setRequireOriginal only with the grant");
    let require = open
        .find("MediaStore.setRequireOriginal(uri)")
        .expect("setRequireOriginal is used");
    assert!(gate < require);
    assert!(
        open.contains("catch (e: UnsupportedOperationException)"),
        "a refused original must not fail the export"
    );
    assert!(
        open.contains("?: throw IOException(\"no input stream\")"),
        "the plain URI remains the fallback"
    );
    assert_eq!(
        open.matches("${e.javaClass.name}").count(),
        open.matches("catch (").count(),
        "every fallback log names the exception class only"
    );
    assert!(
        !open.contains(", e)"),
        "the exception message can carry a path"
    );
    let manifest = read(MANIFEST);
    assert!(!manifest.contains("ACCESS_MEDIA_LOCATION"));
    let rust = rust_code(&read(RUST_ANDROID_ORIGINALS));
    assert!(function(&rust, "fn host_error(").contains("NOT_FOUND => PhotoError::NotFound,"));
}

#[test]
fn the_writer_publishes_or_deletes_every_row() {
    let writer = read(KOTLIN_WRITER);
    let insert = function(&writer, "private fun insertRow(");
    let pending = insert
        .find("put(MediaStore.MediaColumns.IS_PENDING, 1)")
        .expect("rows start pending");
    let insert_at = insert
        .find("resolver.insert(collection(isVideo), values)")
        .unwrap();
    assert!(pending < insert_at);
    let write = insert.find("writeRow(resolver, row, source)").unwrap();
    let publish = insert
        .find("put(MediaStore.MediaColumns.IS_PENDING, 0)")
        .unwrap();
    assert!(write < publish, "the row publishes after the bytes land");
    assert!(
        insert.contains("catch (e: Throwable) {") && insert.contains("deleteRow(resolver, row)"),
        "any failure deletes the row"
    );
    assert!(insert.contains(
        "ContentUris.withAppendedId(typedExternalUri(isVideo), ContentUris.parseId(row))"
    ));
    assert!(
        writer.contains("MediaStore.VOLUME_EXTERNAL_PRIMARY"),
        "the writable primary volume, from minSdk 29"
    );
    assert!(
        writer.contains("MediaStore.Images.Media.EXTERNAL_CONTENT_URI"),
        "save returns the typed `external` id browsing reports"
    );
    let write_row = function(&writer, "private fun writeRow(");
    let close = write_row.find("output.use {").unwrap();
    assert!(close < write_row.find("FileInputStream(source)").unwrap());
    let path = function(&writer, "private fun relativePath(");
    assert!(path.contains("Environment.DIRECTORY_MOVIES"));
    assert!(path.contains("Environment.DIRECTORY_PICTURES"));
    let folder = function(&writer, "internal fun folderName(");
    assert!(
        folder.contains("in \"/\\\\:*?"),
        "a folder is a single segment"
    );
    let save = function(&writer, "fun save(");
    assert!(!save.contains("sweep"));
    assert!(
        save.contains("FrustPhotoLibraryHost.notifySaveDone(requestId, code, uri)"),
        "every save answers once"
    );
    assert!(save.contains("catch (e: Throwable)"));
}

#[test]
fn the_writer_has_no_sweep_and_names_the_expiry_cleanup() {
    let writer = read(KOTLIN_WRITER);
    assert!(!writer.contains("sweepStalePending"));
    assert!(!writer.contains("STALE_PENDING_SECONDS"));
    assert!(!writer.contains("MATCH_INCLUDE"));
    assert!(
        writer.contains("DATE_EXPIRES"),
        "the class doc names Android's own cleanup of pending rows"
    );
}

#[test]
fn the_save_mime_falls_back_from_the_name_to_the_source() {
    let writer = read(KOTLIN_WRITER);
    let mime = function(&writer, "private fun mimeFor(");
    let name = mime.find("listOf(displayName, source.name)").unwrap();
    let default = mime.find("\"image/jpeg\"").unwrap();
    assert!(name < default, "name, then source, then the default");
    assert!(writer.contains("mimeFor(displayName, source, isVideo)"));
    assert!(writer.contains("put(MediaStore.MediaColumns.DISPLAY_NAME, displayName)"));
}

#[test]
fn export_ids_are_media_store_image_and_video_items() {
    let kotlin = read(KOTLIN_HOST);
    let export = function(&kotlin, "fun exportOriginal(");
    assert!(export.contains("if (!isExportableId(source)) return NOT_FOUND"));
    let check = function(&kotlin, "fun isExportableId(");
    assert!(check.contains("uri.authority != MediaStore.AUTHORITY"));
    assert!(check.contains("uri.query != null || uri.fragment != null"));
    assert!(check.contains("segments.size != 4"));
    assert!(check.contains("MediaStore.VOLUME_EXTERNAL_PRIMARY"));
    assert!(check.contains("segments[1] == \"images\" || segments[1] == \"video\""));
    assert!(check.contains("segments[2] == \"media\""));
    assert!(check.contains("all { it in '0'..'9' }"));
}

#[test]
fn decode_jni_calls_match_the_kotlin_declarations() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_DECODE);
    for (decl, name, sig) in [
        (
            "@JvmStatic\n    fun decodeHeader(requestId: Long, path: String, maxDimension: Int): DecodeHeader",
            "decodeHeader",
            "(JLjava/lang/String;I)Ldev/frust/photolibrary/FrustPhotoLibraryHost$DecodeHeader;",
        ),
        (
            "@JvmStatic\n    fun decodeInto(requestId: Long, path: String, width: Int, height: Int, srgb: Boolean, pixels: ByteBuffer): DecodeResult",
            "decodeInto",
            "(JLjava/lang/String;IIZLjava/nio/ByteBuffer;)Ldev/frust/photolibrary/FrustPhotoLibraryHost$DecodeResult;",
        ),
        (
            "@JvmStatic\n    fun cancelDecode(requestId: Long) {",
            "cancelDecode",
            "(J)V",
        ),
    ] {
        assert!(kotlin.contains(decl), "Kotlin host must declare `{decl}`");
        let at = rust
            .find(&format!("jni_str!(\"{name}\")"))
            .unwrap_or_else(|| panic!("src/android_decode.rs never calls `{name}`"));
        let window = &rust[at..(at + 200).min(rust.len())];
        assert!(
            window.contains(&format!("\"{sig}\"")),
            "src/android_decode.rs must call `{name}` with `{sig}`"
        );
        assert!(
            rust.contains(&format!("`{name}` | `(")),
            "src/android_decode.rs's contract table must list `{name}`"
        );
        assert!(
            kotlin.contains(&format!("`{name}(")),
            "the Kotlin host's contract table must list `{name}`"
        );
    }
    let host = rust_code(&kotlin);
    for (call, target) in [
        (
            "fun decodeHeader(",
            "ImageDecodeHost.header(appContext, requestId, path, maxDimension)",
        ),
        (
            "fun decodeInto(",
            "ImageDecodeHost.decodeInto(appContext, requestId, path, width, height, srgb, pixels)",
        ),
        ("fun cancelDecode(", "ImageDecodeHost.cancel(requestId)"),
    ] {
        let at = host.find(call).unwrap();
        let window = &host[at..(at + 300).min(host.len())];
        assert!(
            window.contains(target),
            "`{call}` must forward to `{target}`"
        );
    }
    assert!(
        read(RUST_ANDROID).contains(
            "#[cfg(feature = \"decode\")]\n#[path = \"android_decode.rs\"]\nmod android_decode;"
        ),
        "src/android.rs declares the decode arm behind the `decode` feature"
    );
}

#[test]
fn decode_result_fields_match_their_rust_reads() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_DECODE);
    let metadata = read(RUST_ANDROID_METADATA);
    let reads = [
        (
            "class DecodeHeader(",
            vec![
                ("code", "Int", "int_field"),
                ("sourceWidth", "Int", "int_field"),
                ("sourceHeight", "Int", "int_field"),
                ("width", "Int", "int_field"),
                ("height", "Int", "int_field"),
            ],
        ),
        (
            "class DecodeResult(",
            vec![
                ("code", "Int", "int_field"),
                ("colorSpace", "Int", "int_field"),
                ("captureTime", "String?", "string_field"),
                ("cameraMake", "String?", "string_field"),
                ("cameraModel", "String?", "string_field"),
                ("lens", "String?", "string_field"),
                ("iso", "Int", "int_field"),
                ("exposureSeconds", "Double", "double_field"),
                ("aperture", "Double", "double_field"),
                ("focalLengthMm", "Double", "double_field"),
                ("gpsLatitude", "Double", "double_field"),
                ("gpsLongitude", "Double", "double_field"),
            ],
        ),
    ];
    // `DecodeHeader` is the decoder's own; `DecodeResult` is read once, in `read_result`, for
    // the decode and the metadata call alike.
    for ((class, fields), (file, source, result)) in reads.into_iter().zip([
        (RUST_ANDROID_DECODE, &rust, "&result"),
        (RUST_ANDROID_METADATA, &metadata, "result"),
    ]) {
        let declared = jvm_fields(&kotlin, class);
        let expected: Vec<_> = fields
            .iter()
            .map(|(name, ty, _)| (name.to_string(), ty.to_string()))
            .collect();
        assert_eq!(declared, expected, "`{class}` fields changed");
        for (name, _, reader) in fields {
            let read = format!("{reader}(env, {result}, jni_str!(\"{name}\"))");
            assert!(source.contains(&read), "no `{read}` in {file}");
        }
    }
    let shared = rust_code(&metadata);
    assert!(function(&shared, "pub(super) fn read_result(").contains("Ok(Filled {"));
    assert!(
        rust_code(&rust).contains("read_result(env, &result)"),
        "the decoder reads its result through the shared reader"
    );
    for (reader, sig) in [
        ("fn int_field(", "jni_sig!(\"I\")"),
        ("fn double_field(", "jni_sig!(\"D\")"),
        ("fn string_field(", "jni_sig!(\"Ljava/lang/String;\")"),
    ] {
        assert!(
            function(&metadata, reader).contains(sig),
            "`{reader}` reads `{sig}`"
        );
    }
    // Nested in the host, so its `FrustPhotoLibraryHost$*` keep rule covers the fields Rust
    // reads; Rust never loads `ImageDecodeHost` by name, so it needs no keep rule of its own.
    assert!(
        read(CONSUMER_RULES)
            .contains("-keep class dev.frust.photolibrary.FrustPhotoLibraryHost$* { *; }")
    );
    for file in [RUST_ANDROID, RUST_ANDROID_DECODE, RUST_ANDROID_METADATA] {
        let code = rust_code(&read(file));
        assert!(
            !code.contains("ImageDecodeHost") && !code.contains("AssetMetadataHost"),
            "{file} must reach the hosts through FrustPhotoLibraryHost only"
        );
    }
}

#[test]
fn decode_fills_one_direct_buffer_with_the_software_allocator() {
    let decoder = read(KOTLIN_DECODER);
    assert!(decoder.contains("package dev.frust.photolibrary\n"));
    assert!(decoder.contains("\ninternal object ImageDecodeHost {"));
    for call in [
        "decoder.setAllocator(ImageDecoder.ALLOCATOR_SOFTWARE)",
        "decoder.setTargetSize(width, height)",
        "decoder.setTargetColorSpace(targetColorSpace(info.colorSpace, srgb))",
        "pixels.order(ByteOrder.nativeOrder())",
        "bitmap.copyPixelsToBuffer(pixels)",
        "ImageDecoder.isMimeTypeSupported(type)",
        "throw HeaderRead()",
        "ExifInterface(path)",
    ] {
        assert!(
            decoder.contains(call),
            "ImageDecodeHost.kt must call `{call}`"
        );
    }
    let copy = function(&decoder, "private fun decodeAndCopy(");
    let cancelled = copy
        .find("if (!live.contains(requestId)) return null")
        .expect("the decode checks for a cancel before the copy");
    assert!(cancelled < copy.find("bitmap.copyPixelsToBuffer(pixels)").unwrap());
    assert!(copy.contains("pixels.capacity() != bitmap.byteCount"));
    assert!(copy.contains("bitmap.config != Bitmap.Config.ARGB_8888"));
    let into = function(&decoder, "fun decodeInto(");
    assert!(
        into.contains("} finally {\n            live.remove(requestId)"),
        "decodeInto always drops its request"
    );
    let header = function(&decoder, "fun header(");
    assert!(header.contains("live.add(requestId)"));
    // The buffer is a Rust allocation alive only for the `decodeInto` call: never stored.
    for line in decoder.lines().map(str::trim) {
        let property = line.starts_with("val ")
            || line.starts_with("var ")
            || line.starts_with("private val ")
            || line.starts_with("private var ");
        assert!(
            !(property && line.contains("ByteBuffer")),
            "ImageDecodeHost.kt must never keep the pixel buffer: `{line}`"
        );
    }
    assert!(
        !decoder.contains("ByteArray(width") && !decoder.contains("ByteBuffer.allocate"),
        "full-size pixels never go through a Java array"
    );
    assert!(
        !decoder.contains("Matrix") && !decoder.contains("TAG_ORIENTATION"),
        "ImageDecoder applies the orientation itself; nothing rotates again"
    );
    // The capture time is the original capture tag only, never the last-modified one.
    let exif = function(&decoder, "internal fun withExif(");
    assert!(exif.contains("exif?.getAttribute(ExifInterface.TAG_DATETIME_ORIGINAL),"));
    assert!(
        !exif.contains("ExifInterface.TAG_DATETIME)"),
        "capture_time must not fall back to the TIFF DateTime tag"
    );
    let codes = function(&decoder, "private fun codeFor(");
    assert!(codes.contains("is ImageDecoder.DecodeException ->"));
    assert!(codes.contains("FrustPhotoLibraryHost.DECODE_FAILED"));
    let check = function(&decoder, "private fun requireDecoder(");
    assert!(check.contains("throw Refused(FrustPhotoLibraryHost.NO_DECODER)"));

    let rust = read(RUST_ANDROID_DECODE);
    let code = rust_code(&rust);
    assert_eq!(
        code.matches("unsafe").count(),
        1,
        "src/android_decode.rs holds exactly one `unsafe` block"
    );
    let at = rust
        .find("unsafe { env.new_direct_byte_buffer(rgba.as_mut_ptr(), rgba.len()) }")
        .expect("the one `unsafe` block wraps the pixel buffer");
    let before = &rust[..at];
    let safety = before.rfind("// SAFETY:").expect("a SAFETY note");
    assert!(
        !before[safety..].contains("\n\n"),
        "the SAFETY note sits right above the block"
    );
    let note = &before[safety..];
    assert!(note.contains("alive and unmoved") && note.contains("never keeps it"));
    let into = function(&code, "fn decode_into(");
    let call = into.find("jni_str!(\"decodeInto\")").unwrap();
    let release = into
        .find("env.delete_local_ref(buffer);")
        .expect("the buffer's local reference is deleted");
    assert!(call < release, "the buffer is released after the call");
    assert!(into.contains("rgba: &mut [u8],"));
    assert!(!code.contains("[B") && !code.contains("JByteArray"));
    let finish = function(&code, "fn finish(");
    assert!(finish.contains("pixels::unpremultiply_rgba8_in_place(&mut rgba);"));
}

#[test]
fn decode_refuses_the_ui_thread_and_types_its_errors() {
    let android = rust_code(&read(RUST_ANDROID));
    let arm = function(&android, "pub(crate) fn decode_file(");
    let ui = arm
        .find("with_host(|env, _class| ensure_off_ui_thread(env))?;")
        .expect("decode_file refuses the UI thread");
    assert!(ui < arm.find("android_decode::decode(path, options)").unwrap());
    assert!(!arm.contains("Unsupported"), "the stub arm is gone");

    let rust = rust_code(&read(RUST_ANDROID_DECODE));
    let errors = function(&rust, "fn decode_error(");
    for mapping in [
        "NO_DECODER => PhotoError::Unsupported,",
        "DECODE_FAILED => PhotoError::Platform(",
        "CANCELLED => PhotoError::Cancelled,",
    ] {
        assert!(
            errors.contains(mapping),
            "decode_error must map `{mapping}`"
        );
    }
    let decode = function(&rust, "pub(super) fn decode(");
    let hook = decode
        .find("token.set_hook(move || cancel_decode(request_id));")
        .expect("the cancel hook reaches the host");
    assert!(hook < decode.find("run_decode(").unwrap());
    assert!(decode.contains("token.clear_hook();"));
    let run = function(&rust, "fn run_decode(");
    assert!(
        run.matches("if options.is_cancelled() {").count() >= 2,
        "the token is checked before the header and after the copy"
    );
    assert!(
        run.contains("cancel_decode(request_id);"),
        "a request abandoned after its header is dropped on the host"
    );
    let cancel = function(&rust, "fn cancel_decode(");
    assert!(cancel.contains("jni_str!(\"cancelDecode\")"));
}

#[test]
fn metadata_jni_call_matches_the_kotlin_declaration() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_METADATA);
    let decl = "@JvmStatic\n    fun assetMetadata(id: String): DecodeResult";
    assert!(kotlin.contains(decl), "Kotlin host must declare `{decl}`");
    let at = rust
        .find("jni_str!(\"assetMetadata\")")
        .expect("src/android_metadata.rs never calls `assetMetadata`");
    let window = &rust[at..(at + 200).min(rust.len())];
    assert!(
        window.contains(
            "\"(Ljava/lang/String;)Ldev/frust/photolibrary/FrustPhotoLibraryHost$DecodeResult;\""
        ),
        "src/android_metadata.rs must call `assetMetadata` with its declared signature"
    );
    assert!(
        rust.contains("`assetMetadata` | `("),
        "src/android_metadata.rs's contract table must list `assetMetadata`"
    );
    assert!(
        kotlin.contains("`assetMetadata("),
        "the Kotlin host's contract table must list `assetMetadata`"
    );
    let host = rust_code(&kotlin);
    let at = host.find("fun assetMetadata(").unwrap();
    assert!(
        host[at..(at + 200).min(host.len())].contains("AssetMetadataHost.read(appContext, id)"),
        "`assetMetadata` must forward to `AssetMetadataHost.read`"
    );
    let android = read(RUST_ANDROID);
    assert!(
        android.contains(
            "#[cfg(any(feature = \"decode\", feature = \"library\"))]\n#[path = \"android_metadata.rs\"]\nmod android_metadata;"
        ),
        "src/android.rs declares the shared EXIF module for either feature"
    );
}

#[test]
fn metadata_host_reuses_the_export_open_and_the_shared_exif_read() {
    let host = read(KOTLIN_METADATA);
    assert!(host.contains("package dev.frust.photolibrary\n"));
    assert!(host.contains("\ninternal object AssetMetadataHost {"));
    let body = function(&host, "fun read(");
    let check = body
        .find("if (!FrustPhotoLibraryHost.isExportableId(uri))")
        .expect("only MediaStore image and video ids are read");
    let open = body
        .find("FrustPhotoLibraryHost.openOriginalStream(context, uri)")
        .expect("the asset opens through the export's own open");
    assert!(check < open);
    assert!(
        body.contains("opened.stream.use { input ->"),
        "the stream is closed on every path"
    );
    assert!(body.contains("ExifInterface(input)"));
    assert!(
        body.contains(
            "ImageDecodeHost.withExif(FrustPhotoLibraryHost.COLOR_OTHER, exif, opened.unredacted)"
        ),
        "the EXIF goes through the decoder's read, without GPS from a redacted read"
    );
    for (code, catch) in [
        ("NOT_FOUND", "catch (e: FileNotFoundException)"),
        ("PERMISSION_DENIED", "catch (e: SecurityException)"),
        ("DECODE_FAILED", "catch (e: Exception)"),
    ] {
        let at = body.find(catch).unwrap_or_else(|| panic!("no `{catch}`"));
        let window = &body[at..(at + 260).min(body.len())];
        assert!(
            window.contains(&format!("FrustPhotoLibraryHost.{code})")),
            "`{catch}` answers `{code}`"
        );
    }
    assert!(
        !host.contains("getAttribute") && !host.contains("TAG_"),
        "AssetMetadataHost.kt must not read tags of its own"
    );
    assert!(
        !host.contains(", e)"),
        "the exception message can carry a path"
    );

    let decoder = read(KOTLIN_DECODER);
    let exif = function(&decoder, "internal fun withExif(");
    assert!(exif.contains("withLocation: Boolean = true"));
    assert!(exif.contains("val located = exif?.takeIf { withLocation }"));
    assert_eq!(
        exif.matches("located?.let { coordinate(").count(),
        2,
        "both GPS values come from the location-checked read"
    );
    assert!(decoder.contains("internal fun decodeFailed(code: Int): DecodeResult"));

    let kotlin = read(KOTLIN_HOST);
    let open = function(&kotlin, "internal fun openOriginalStream(");
    assert!(open.contains("OriginalStream(it, true)"));
    assert!(open.contains("?: throw IOException(\"no input stream\"), false)"));
}

#[test]
fn metadata_refuses_the_ui_thread_and_types_its_codes() {
    let android = rust_code(&read(RUST_ANDROID));
    let arm = function(&android, "pub(crate) fn asset_metadata(");
    let ui = arm
        .find("with_host(|env, _class| ensure_off_ui_thread(env))?;")
        .expect("asset_metadata refuses the UI thread");
    assert!(
        ui < arm
            .find("android_metadata::asset_metadata(id, options)")
            .unwrap()
    );

    let rust = rust_code(&read(RUST_ANDROID_METADATA));
    let errors = function(&rust, "fn metadata_error(");
    for mapping in [
        "NOT_FOUND => PhotoError::NotFound,",
        "PERMISSION_DENIED => PhotoError::PermissionDenied,",
        "NOT_INITIALIZED => PhotoError::PlatformNotInitialized,",
        "DECODE_FAILED => PhotoError::Platform(",
        "CANCELLED => PhotoError::Cancelled,",
        "other => error_for(other),",
    ] {
        assert!(
            errors.contains(mapping),
            "metadata_error must map `{mapping}`"
        );
    }
    let result = function(&rust, "fn metadata_result(");
    assert!(
        result.contains("Ok(exif_from(filled).unwrap_or_default())"),
        "an asset with no EXIF is an empty value, not an error"
    );
    let call = function(&rust, "pub(super) fn asset_metadata(");
    let host = call.find("request_metadata(").unwrap();
    let cancel = call
        .find("if options.is_cancelled() {")
        .expect("a cancel that raced the answer wins");
    assert!(host < cancel);
    assert!(
        !rust.contains("unsafe"),
        "src/android_metadata.rs holds no `unsafe`"
    );
}

#[test]
fn change_feed_jni_calls_match_the_kotlin_declarations() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_CHANGES);
    for (decl, name, sig) in [
        (
            "@JvmStatic\n    fun changeSnapshot(withGenerations: Boolean): ChangesResult",
            "changeSnapshot",
            "(Z)Ldev/frust/photolibrary/FrustPhotoLibraryHost$ChangesResult;",
        ),
        (
            "@JvmStatic\n    fun startChangeObserver(): Int",
            "startChangeObserver",
            "()I",
        ),
        (
            "@JvmStatic\n    fun stopChangeObserver() {",
            "stopChangeObserver",
            "()V",
        ),
    ] {
        assert!(kotlin.contains(decl), "Kotlin host must declare `{decl}`");
        let at = rust
            .find(&format!("jni_str!(\"{name}\")"))
            .unwrap_or_else(|| panic!("src/android_changes.rs never calls `{name}`"));
        let window = &rust[at..(at + 200).min(rust.len())];
        assert!(
            window.contains(&format!("jni_sig!(\"{sig}\")")),
            "src/android_changes.rs must call `{name}` with `{sig}`"
        );
        assert!(
            rust.contains(&format!("`{name}` | `(")),
            "src/android_changes.rs's contract table must list `{name}`"
        );
        assert!(
            kotlin.contains(&format!("`{name}(")),
            "the Kotlin host's contract table must list `{name}`"
        );
    }
    let host = rust_code(&kotlin);
    for (entry, forward) in [
        (
            "fun changeSnapshot(",
            "MediaStoreChanges.snapshot(appContext, withGenerations)",
        ),
        (
            "fun startChangeObserver(",
            "MediaStoreChanges.startObserver(appContext)",
        ),
        (
            "fun stopChangeObserver(",
            "MediaStoreChanges.stopObserver(appContext)",
        ),
    ] {
        let at = host.find(entry).unwrap();
        assert!(
            host[at..(at + 200).min(host.len())].contains(forward),
            "`{entry}` must forward to `{forward}`"
        );
    }

    let android = read(RUST_ANDROID);
    assert!(
        android.contains(
            "#[cfg(feature = \"library\")]\n#[path = \"android_changes.rs\"]\nmod android_changes;"
        ),
        "src/android.rs declares the change feed under `library`"
    );
    assert!(
        android.contains(
            "pub(crate) use android_changes::{\n    changes_since, current_change_token, start_change_observer, stop_change_observer,\n};"
        ),
        "src/android.rs re-exports the four change-feed arms"
    );
    let code = rust_code(&android);
    assert!(
        !android.contains("placeholder") && !code.contains("fn current_change_token("),
        "the placeholder arms are gone from src/android.rs"
    );
}

#[test]
fn change_result_fields_match_their_rust_reads() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_CHANGES);
    let fields = [
        ("code", "Int", "int_field"),
        ("version", "String?", "string_field"),
        ("generation", "Long", "get_field"),
        ("ids", "LongArray", "long_array_field"),
        ("kinds", "IntArray", "int_array_field"),
        ("added", "LongArray", "long_array_field"),
        ("modified", "LongArray", "long_array_field"),
        ("imageBase", "String?", "string_field"),
        ("videoBase", "String?", "string_field"),
    ];
    let declared = jvm_fields(&kotlin, "class ChangesResult(");
    let expected: Vec<_> = fields
        .iter()
        .map(|(name, ty, _)| (name.to_string(), ty.to_string()))
        .collect();
    assert_eq!(declared, expected, "`ChangesResult` fields changed");
    for (name, _, reader) in fields {
        let read = if reader == "get_field" {
            format!(".get_field(&result, jni_str!(\"{name}\")")
        } else {
            format!("{reader}(env, &result, jni_str!(\"{name}\"))")
        };
        assert!(
            rust.contains(&read),
            "no `{read}` in src/android_changes.rs"
        );
    }
    assert!(
        rust.contains(".get_field(&result, jni_str!(\"generation\"), jni_sig!(\"J\"))"),
        "`generation` is a Long"
    );

    // The generation the host reports below API 30.
    let changes = read(KOTLIN_CHANGES);
    assert!(changes.contains("\n    const val NO_GENERATION = -1L\n"));
    assert!(rust.contains("\nconst NO_GENERATION: i64 = -1;\n"));
    let failed = function(&changes, "private fun failed(");
    assert!(failed.contains("ChangesResult(code, null, NO_GENERATION,"));
    let read_rows = function(&changes, "private fun read(");
    assert!(
        read_rows.contains("FrustPhotoLibraryHost.OK, MediaStore.getVersion(ctx), NO_GENERATION,"),
        "API 29 answers no generation"
    );
}

#[test]
fn the_library_changed_export_matches_its_kotlin_external() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_CHANGES);
    assert!(
        kotlin.contains("@JvmStatic\n    external fun nativeOnLibraryChanged()\n"),
        "Kotlin `nativeOnLibraryChanged` signature changed"
    );
    let symbol = "Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnLibraryChanged";
    let decl = format!("pub extern \"system\" fn {symbol}<");
    assert!(
        rust.contains(&decl),
        "src/android_changes.rs must export `{symbol}`"
    );
    let export = function(&rust, &decl);
    assert!(
        export.contains("_class: JClass<'local>,\n    ) {"),
        "`{symbol}` takes no argument besides the class"
    );
    assert!(export.contains("crate::changes::notify_all();"));
    assert!(
        export.contains(".resolve::<LogErrorAndDefault>()"),
        "`{symbol}` must never unwind into the JVM"
    );
    assert!(
        rust.contains("#[unsafe(no_mangle)]\n    pub extern \"system\" fn Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnLibraryChanged<"),
        "the export keeps its JNI symbol"
    );
    let notify = function(&kotlin, "internal fun notifyLibraryChanged(");
    assert!(
        notify.contains("nativeOnLibraryChanged()") && notify.contains("catch (e: Throwable)"),
        "a missing symbol degrades to a log line"
    );
    assert!(
        kotlin.contains("`nativeOnLibraryChanged()`"),
        "the Kotlin host's contract table must list `nativeOnLibraryChanged`"
    );
}

#[test]
fn the_change_snapshot_refuses_the_ui_thread_and_checks_access() {
    let rust = rust_code(&read(RUST_ANDROID_CHANGES));
    let snapshot = function(&rust, "fn snapshot(with_generations: bool)");
    let ui = snapshot
        .find("ensure_off_ui_thread(env)?;")
        .expect("the snapshot refuses the UI thread");
    assert!(ui < snapshot.find("call_static_method").unwrap());
    for arm in [
        "pub(crate) fn current_change_token(",
        "pub(crate) fn changes_since(",
    ] {
        let body = function(&rust, arm);
        assert!(
            body.contains("snapshot(") || body.contains("current_change_token()?"),
            "`{arm}` reads through the UI-thread-checked snapshot"
        );
        assert!(!body.contains("call_static_method"));
    }
    let errors = function(&rust, "fn changes_error(");
    for mapping in [
        "NOT_INITIALIZED => PhotoError::PlatformNotInitialized,",
        "PERMISSION_DENIED => PhotoError::PermissionDenied,",
        "QUERY_FAILED => PhotoError::Platform(",
        "other => error_for(other),",
    ] {
        assert!(
            errors.contains(mapping),
            "changes_error must map `{mapping}`"
        );
    }
    assert!(
        !rust.contains("PickGuard") && !rust.contains("WAITS"),
        "the change feed never touches pick state"
    );

    let changes = read(KOTLIN_CHANGES);
    assert!(changes.contains("package dev.frust.photolibrary\n"));
    assert!(changes.contains("\ninternal object MediaStoreChanges {"));
    let guard = function(&changes, "fun snapshot(");
    assert!(
        guard.contains("if (ctx == null) return failed(FrustPhotoLibraryHost.NOT_INITIALIZED)")
    );
    assert!(
        guard.contains(
            "if (!hasAccess(ctx)) return failed(FrustPhotoLibraryHost.PERMISSION_DENIED)"
        )
    );
    assert!(guard.contains("catch (e: SecurityException)"));
    assert!(guard.contains("failed(FrustPhotoLibraryHost.QUERY_FAILED)"));
    let access = function(&changes, "private fun hasAccess(");
    assert!(
        access.contains("MediaStoreBrowser.mediaPermissions(ctx, Build.VERSION.SDK_INT)"),
        "the change feed checks the browser's declared permission subset"
    );
    let start = function(&changes, "fun startObserver(");
    assert!(start.contains("if (!hasAccess(ctx)) return FrustPhotoLibraryHost.PERMISSION_DENIED"));
    for pick_state in ["claimInFlight", "inFlight", "copyExecutor", ", e)"] {
        assert!(
            !changes.contains(pick_state),
            "MediaStoreChanges.kt must not use `{pick_state}`"
        );
    }
}

#[test]
fn the_change_snapshot_reads_the_position_first_from_the_browsers_query() {
    let changes = read(KOTLIN_CHANGES);
    let browser = read(KOTLIN_BROWSER);
    let files = "private val FILES_URI: Uri = MediaStore.Files.getContentUri(\"external\")";
    assert!(changes.contains(files) && browser.contains(files));
    let selection = "IN ($MEDIA_TYPE_IMAGE, $MEDIA_TYPE_VIDEO)";
    assert!(changes.contains(selection) && browser.contains(selection));
    for base in [
        "MediaStore.Images.Media.EXTERNAL_CONTENT_URI",
        "MediaStore.Video.Media.EXTERNAL_CONTENT_URI",
    ] {
        assert!(
            browser.contains(base),
            "the browser builds ids under `{base}`"
        );
        assert!(
            function(&changes, "private fun read(").contains(&format!("{base}.toString()")),
            "the snapshot reports `{base}`"
        );
    }
    let read_rows = function(&changes, "private fun read(");
    let sdk = read_rows.find("if (Build.VERSION.SDK_INT < 30) {").unwrap();
    let version = read_rows
        .find("MediaStore.getVersion(ctx, MediaStore.VOLUME_EXTERNAL_PRIMARY)")
        .unwrap();
    let generation = read_rows
        .find("MediaStore.getGeneration(ctx, MediaStore.VOLUME_EXTERNAL_PRIMARY)")
        .unwrap();
    let query = read_rows
        .find("ctx.contentResolver.query(FILES_URI, projection, SELECTION, null,")
        .unwrap();
    assert!(
        sdk < version && version < query && generation < query,
        "below API 30 there is no generation, and the position is read before the rows"
    );
    assert!(changes.contains("MediaStore.MediaColumns.GENERATION_ADDED"));
    assert!(changes.contains("MediaStore.MediaColumns.GENERATION_MODIFIED"));
    assert!(
        !changes.contains("DeletedFiles"),
        "deletions come from the id set"
    );
}

#[test]
fn the_observer_wakes_from_every_overload_and_stops_outside_its_lock() {
    let changes = read(KOTLIN_CHANGES);
    let start = function(&changes, "fun startObserver(");
    for collection in ["Images", "Video"] {
        assert!(
            start.contains(&format!(
                "resolver.registerContentObserver(MediaStore.{collection}.Media.EXTERNAL_CONTENT_URI, true, created)"
            )),
            "the observer covers the {collection} collection with descendants"
        );
    }
    assert!(start.contains("if (observer != null) return FrustPhotoLibraryHost.OK"));
    assert!(start.contains("unregister(ctx, created)"));
    // The wake drops callbacks from an observer that is not the published one, so publishing
    // after registering would drop, in Kotlin, a change delivered in between.
    let published = start
        .find("observer = created")
        .expect("the observer is published");
    let registered = start
        .find("resolver.registerContentObserver(")
        .expect("the observer is registered");
    assert!(
        published < registered,
        "the observer is published before it is registered, so an early wake reaches native"
    );
    let failure = start
        .find("if (code != FrustPhotoLibraryHost.OK) {")
        .expect("a failed registration is handled");
    let failed = &start[failure..];
    let reset = failed
        .find("observer = null")
        .expect("a failed registration unpublishes the observer");
    assert!(
        reset < failed.find("unregister(ctx, created)").unwrap(),
        "a failed registration unpublishes the observer before unregistering it"
    );

    let observer = function(
        &changes,
        "private class WakeObserver : ContentObserver(null) {",
    );
    let overloads: Vec<&str> = observer
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("override fun onChange("))
        .collect();
    assert_eq!(
        overloads,
        [
            "override fun onChange(selfChange: Boolean) = wake()",
            "override fun onChange(selfChange: Boolean, uri: Uri?) = wake()",
            "override fun onChange(selfChange: Boolean, uri: Uri?, flags: Int) = wake()",
            "override fun onChange(selfChange: Boolean, uris: Collection<Uri>, flags: Int) = wake()",
        ],
        "every onChange overload wakes, without calling super"
    );
    let wake = function(&changes, "private fun wake(");
    assert!(wake.contains("if (observer !== this) return"));
    assert!(wake.contains("FrustPhotoLibraryHost.notifyLibraryChanged()"));
    assert!(
        !wake.contains("synchronized"),
        "the wake holds no lock, so a subscriber may stop the observer from inside it"
    );

    let stop = function(&changes, "fun stopObserver(");
    let unlocked = stop
        .find("} ?: return")
        .expect("the lock is released first");
    assert!(
        stop[unlocked..].contains("unregister(ctx, current)"),
        "the observer is unregistered outside the lock"
    );

    let rust = rust_code(&read(RUST_ANDROID_CHANGES));
    let stop = function(&rust, "pub(crate) fn stop_change_observer(");
    assert!(stop.contains("jni_str!(\"stopChangeObserver\")"));
    let start = function(&rust, "pub(crate) fn start_change_observer(");
    assert!(start.contains("Err(changes_error(code))"));
}

#[test]
fn cursor_paging_jni_call_matches_the_kotlin_declaration() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_LIBRARY);
    let decl = "@JvmStatic\n    fun assetsAfter(albumId: String?, filter: Int, first: Boolean, afterKey: Long, afterId: Long, limit: Int, details: Boolean): AssetsAfterResult =";
    let sig = "(Ljava/lang/String;IZJJIZ)Ldev/frust/photolibrary/FrustPhotoLibraryHost$AssetsAfterResult;";
    assert!(kotlin.contains(decl), "Kotlin host must declare `{decl}`");
    assert!(
        kotlin.contains(&format!("`{sig}`")),
        "the contract table names the `assetsAfter` signature"
    );
    let at = kotlin.find(decl).unwrap();
    assert!(
        kotlin[at..(at + 400).min(kotlin.len())].contains(
            "MediaStoreBrowser.assetsAfter(appContext, albumId, filter, first, afterKey, afterId, limit, details)"
        ),
        "the host forwards to the browser"
    );
    let at = rust
        .find("jni_str!(\"assetsAfter\")")
        .expect("src/android_library.rs calls `assetsAfter`");
    assert!(
        rust[at..(at + 300).min(rust.len())].contains(&format!("\"{sig}\"")),
        "src/android_library.rs must call `assetsAfter` with `{sig}`"
    );
    let code = rust_code(&rust);
    let arm = function(&code, "pub(crate) fn assets_after(");
    let call = arm.find("call_static_method").unwrap();
    let values = &arm[call..];
    let order = [
        "JValue::Object(&album)",
        "JValue::Int(filter)",
        "JValue::Bool(first)",
        "JValue::Long(after_key)",
        "JValue::Long(after_id)",
        "JValue::Int(limit)",
        "JValue::Bool(details)",
    ];
    let mut last = 0;
    for value in order {
        let at = values
            .find(value)
            .unwrap_or_else(|| panic!("`assets_after` passes `{value}`"));
        assert!(at > last, "`{value}` is out of the declared order");
        last = at;
    }
}

#[test]
fn cursor_page_fields_match_their_rust_reads() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_LIBRARY);
    assert_eq!(
        jvm_fields(&kotlin, "class AssetsAfterResult("),
        [
            ("code", "Int"),
            ("page", "AssetsResult"),
            ("lastKey", "Long"),
            ("lastId", "Long"),
            ("hasMore", "Boolean"),
        ]
        .map(|(name, ty)| (name.to_string(), ty.to_string()))
        .to_vec(),
        "`AssetsAfterResult` fields changed"
    );
    let arm = function(&rust, "pub(crate) fn assets_after(");
    for read in [
        "int_field(env, &result, jni_str!(\"code\"))",
        ".get_field(&result, jni_str!(\"lastKey\"), jni_sig!(\"J\"))",
        ".get_field(&result, jni_str!(\"lastId\"), jni_sig!(\"J\"))",
        ".get_field(&result, jni_str!(\"hasMore\"), jni_sig!(\"Z\"))",
        "jni_sig!(\"Ldev/frust/photolibrary/FrustPhotoLibraryHost$AssetsResult;\")",
        "asset_columns(env, page)?",
    ] {
        assert!(arm.contains(read), "no `{read}` in `assets_after`");
    }
    assert!(
        function(&rust, "pub(crate) fn assets(").contains("asset_columns(env, result)?"),
        "both pages read their arrays through one reader"
    );
}

#[test]
fn assets_by_id_result_and_host_match_their_rust_reads() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID_LIBRARY);
    let sig =
        "([Ljava/lang/String;Z)Ldev/frust/photolibrary/FrustPhotoLibraryHost$AssetsByIdResult;";
    assert!(
        kotlin.contains(&format!("`{sig}`")),
        "the contract table names the `assetsById` signature"
    );
    let decl = "fun assetsById(ids: Array<String>, details: Boolean): AssetsByIdResult =";
    let at = kotlin.find(decl).expect("the host declares `assetsById`");
    assert!(
        kotlin[at..(at + 200).min(kotlin.len())]
            .contains("MediaStoreBrowser.assetsById(appContext, ids, details)"),
        "the host forwards to the browser"
    );
    assert_eq!(
        jvm_fields(&kotlin, "class AssetsByIdResult("),
        [
            ("code", "Int"),
            ("positions", "IntArray"),
            ("page", "AssetsResult"),
        ]
        .map(|(name, ty)| (name.to_string(), ty.to_string()))
        .to_vec(),
        "`AssetsByIdResult` fields changed"
    );
    let code = rust_code(&rust);
    let arm = function(&code, "pub(crate) fn assets_by_id(");
    for read in [
        "int_field(env, &result, jni_str!(\"code\"))",
        "int_array_field(env, &result, jni_str!(\"positions\"))",
        "jni_sig!(\"Ldev/frust/photolibrary/FrustPhotoLibraryHost$AssetsResult;\")",
        "asset_columns(env, page)?",
        "assets_from(columns, 0, details)",
        "crate::in_input_order(ids.len(), &positions, page.items)",
        "JValue::Object(&raw_ids), JValue::Bool(details)",
    ] {
        assert!(arm.contains(read), "no `{read}` in `assets_by_id`");
    }
    assert!(
        !arm.contains("PickGuard"),
        "`assets_by_id` is read-only and never takes the pick guard"
    );
}

#[test]
fn assets_by_id_queries_media_rows_in_bounded_chunks() {
    let browser = read(KOTLIN_BROWSER);
    assert!(browser.contains("internal const val BY_ID_CHUNK = 500"));
    let row_id = function(&browser, "internal fun rowIdOf(");
    assert!(
        row_id.contains(
            "uri.scheme != ContentResolver.SCHEME_CONTENT || uri.authority != MediaStore.AUTHORITY"
        ),
        "an id that is not a `content://media` URI has no row, as for a thumbnail"
    );
    let by_id = function(&browser, "fun assetsById(ctx: Context?,");
    assert!(by_id.contains("browse(ctx, \"assetsById\", ::assetsByIdFailed)"));
    assert!(
        by_id.contains("mediaSelection(FrustPhotoLibraryHost.FILTER_IMAGES_AND_VIDEOS)"),
        "only image and video rows answer, as in a listing"
    );
    assert!(by_id.contains("wanted.chunked(BY_ID_CHUNK)"));
    assert!(by_id.contains("${BaseColumns._ID} IN ($placeholders)"));
    assert!(by_id.contains("val projection = projection(details)"));
    assert!(by_id.contains("readRows(c, 0, chunk.size, details)"));
    assert!(by_id.contains("assetsPage(rows, 0)"));
}

/// The concatenated string literals of the Kotlin `const val` declared by
/// `decl`, up to the blank line that ends it.
fn kotlin_const_text(contents: &str, decl: &str) -> String {
    let start = contents
        .find(decl)
        .unwrap_or_else(|| panic!("source has no `{decl}`"));
    let body = &contents[start + decl.len()..];
    let body = &body[..body.find("\n\n").unwrap_or(body.len())];
    body.split('"').skip(1).step_by(2).collect()
}

#[test]
fn the_keyset_query_counts_nothing_and_orders_by_the_key_then_the_id() {
    let browser = read(KOTLIN_BROWSER);
    let key = kotlin_const_text(&browser, "private const val SORT_KEY =");
    let order = kotlin_const_text(&browser, "private const val SORT_ORDER =");
    assert_eq!(
        order,
        format!("{key} DESC, ${{BaseColumns._ID}} DESC"),
        "the keyset compares the key the page is ordered by, then `_ID`"
    );
    assert!(key.starts_with("CASE WHEN ") && key.ends_with(" END"));

    let page = rust_code(&browser);
    let page = function(&page, "fun assetsAfter(");
    assert!(
        !page.contains("countRows")
            && !page.to_uppercase().contains("COUNT")
            && !page.contains(".count"),
        "the keyset page takes no row count"
    );
    assert!(page.contains(
        "val selection = if (first) scoped else \"$scoped AND ${keysetSelection(afterKey, afterId)}\""
    ));
    assert!(
        page.contains("val window = pageSize + 1")
            && page.contains("readRows(c, 0, window, details)")
            && page.contains("val more = rows.size > pageSize"),
        "one row past the page says whether another follows, read from the first row"
    );
    assert!(page.contains("last?.date ?: 0, last?.rowId ?: 0, more"));

    let keyset = function(&browser, "internal fun keysetSelection(");
    assert!(
        keyset.contains(
            "\"(($SORT_KEY) < $afterKey OR (($SORT_KEY) = $afterKey AND ${BaseColumns._ID} < $afterId))\""
        ),
        "the rows after the cursor: an older key, or the same key and a smaller id"
    );

    let query = function(&browser, "private fun queryAfter(");
    assert_eq!(
        query.matches(", SORT_ORDER)").count(),
        2,
        "both API branches order by SORT_ORDER"
    );
    assert!(query.contains("if (Build.VERSION.SDK_INT < 30) {"));
    assert!(query.contains("putInt(ContentResolver.QUERY_ARG_LIMIT, window)"));
    assert!(
        !query.contains("QUERY_ARG_OFFSET") && !query.contains("moveToPosition"),
        "the keyset page never offsets or seeks"
    );
    let rows = function(&browser, "private fun readRows(");
    assert!(rows.contains("val rowId = cursor.getLong(0)"));
    assert!(rows.contains("rowId = rowId,"));

    // The offset path keeps its count and its seek.
    let offset = function(&browser, "fun assets(ctx: Context?,");
    assert!(offset.contains("countRows(resolver, selection, args)"));
    assert!(
        offset.contains("queryPage(resolver, selection, args, offset.toInt(), limit, details)")
    );
    assert!(!offset.contains("keysetSelection"));
}

#[test]
fn cursor_paging_refuses_the_ui_thread_and_checks_the_cursor_first() {
    let rust = rust_code(&read(RUST_ANDROID_LIBRARY));
    let arm = function(&rust, "pub(crate) fn assets_after(");
    let cursor = arm
        .find("cursor.keyset_for(query)")
        .expect("the cursor is checked against the query");
    let host = arm.find("with_host(").unwrap();
    let ui = arm.find("ensure_off_ui_thread(env)?;").unwrap();
    let call = arm.find("call_static_method").unwrap();
    assert!(cursor < host && host < ui && ui < call);
    assert!(!arm.contains("PickGuard"), "cursor paging is read-only");
    assert!(arm.contains("keyset_page(query, page.items, more, last_key, last_id)"));
    assert!(
        !function(&rust, "pub(crate) fn assets(").contains("assetsAfter"),
        "the offset path makes only its own call"
    );
}

#[test]
fn media_permissions_use_the_declared_subset() {
    let browser = read(KOTLIN_BROWSER);

    let narrow = function(&browser, "internal fun narrowPermissions(");
    assert!(narrow.contains("if (sdk < 33) return candidates"));
    assert!(narrow.contains("candidates.filter { it in declared }"));
    assert!(narrow.contains("it != READ_MEDIA_VISUAL_USER_SELECTED"));
    assert!(
        narrow.contains("else emptyList()"),
        "no declared media type means nothing is requested"
    );

    let media = function(&browser, "internal fun mediaPermissions(");
    assert!(media.contains("narrowPermissions(sdk, manifestPermissions(ctx))"));
    assert!(
        function(&browser, "private fun manifestPermissions(").contains("GET_PERMISSIONS"),
        "the declared set comes from the manifest"
    );
    assert!(browser.contains(
        "private const val READ_MEDIA_VISUAL_USER_SELECTED = \"android.permission.READ_MEDIA_VISUAL_USER_SELECTED\""
    ));

    // The listing and by-id paths share the browse guard, which checks the subset.
    assert!(
        function(&browser, "private fun hasAccess(")
            .contains("mediaPermissions(ctx, Build.VERSION.SDK_INT)")
    );
    assert!(
        !browser.contains("mediaPermissions(Build.VERSION.SDK_INT)"),
        "every caller passes the context so the declared subset applies"
    );

    // The selection prompt requests the subset only.
    let host = read(KOTLIN_HOST);
    assert!(host.contains(
        "launcher.launch(MediaStoreBrowser.mediaPermissions(ctx, Build.VERSION.SDK_INT).toTypedArray())"
    ));
}
