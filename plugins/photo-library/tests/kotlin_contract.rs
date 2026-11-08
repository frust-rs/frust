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
//!   object, and the Rust -> Kotlin `pick` call's JNI signature matches its
//!   Kotlin declaration;
//! - the module manifest declares no permission and declares the init
//!   provider; the keep rules cover the host and the provider;
//! - the picker contracts and their limit handling (`maxItems` of at least 2,
//!   capped at the platform maximum, result truncated to the limit);
//! - the host never builds a file name from platform text; Rust renames the
//!   staging copies with the pick cache's sanitiser;
//! - one pick is in flight at a time, on both sides, launched on the main
//!   thread, and a destroyed Activity cancels its pick;
//! - the Rust wait has no deadline.

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

/// Every code the two sides share, by name.
const SHARED_CODES: &[&str] = &[
    "PENDING",
    "NO_ACTIVITY",
    "BUSY",
    "NOT_INITIALIZED",
    "LAUNCH_FAILED",
    "COPY_FAILED",
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
    let mut values: Vec<_> = SHARED_CODES[..6].iter().map(|n| rust[*n]).collect();
    values.sort_unstable();
    values.dedup();
    assert_eq!(values.len(), 6, "result codes must be distinct");
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

    let prefix = format!("Java_{}_FrustPhotoLibraryHost_", package.replace('.', "_"));
    let exports: [(&str, &[&str]); 2] = [
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

    let rust = rust_code(&read(RUST_ANDROID));
    let copy_name = function(&rust, "fn copy_name(");
    assert!(copy_name.contains("pick_cache::sanitize_file_name("));
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
    assert!(rust.contains("static PICK_LOCK: Mutex<()> = Mutex::new(());"));
    let pick = function(&rust, "pub(crate) fn pick(");
    let guard = pick
        .find("let _one_at_a_time = match PICK_LOCK.try_lock() {")
        .expect("pick must try_lock PICK_LOCK in a named guard");
    assert!(pick.contains("Err(TryLockError::WouldBlock) => return Err(busy()),"));
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
        !deliver.contains("PICK_LOCK"),
        "deliver must never take PICK_LOCK, or a held pick could not be answered"
    );
    assert!(
        deliver.contains("Some(slot @ None) =>"),
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
    let await_answer = function(&rust, "fn await_answer(");
    assert!(await_answer.contains(".wait_while("));
}
