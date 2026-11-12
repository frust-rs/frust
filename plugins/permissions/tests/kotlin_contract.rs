//! Source-scan contract tests between `frust-permissions`' Rust side and its
//! Android Kotlin host, because no cargo gate compiles Kotlin (only a real
//! Gradle build does). Same shape as
//! `plugins/native-widgets/tests/kotlin_conformance.rs`: a plain `std::fs`
//! line scan, not a parser, correct for the hand-written shapes these files
//! take.
//!
//! Pinned here:
//! - every Kotlin result/kind `const val` equals its twin in
//!   `frust_permissions::codes::__contract`, and the two `openSettings` codes
//!   equal `src/android.rs`'s;
//! - every Kotlin permission set equals `codes::android_permissions` for the
//!   SDK band it serves, and the Kotlin file names no other permission;
//! - the JNI export and the host class name match the Kotlin package and
//!   object, and each Rust -> Kotlin call's JNI signature matches its Kotlin
//!   declaration;
//! - the module manifest declares no permission and declares the init
//!   provider; the keep rules cover the host and the provider;
//! - the asked-flag storage names, a contract other plugins rely on;
//! - the photo library's limited-access derivation reads grants only, so
//!   declaring `READ_MEDIA_VISUAL_USER_SELECTED` (which Android adds when an
//!   app declares `ACCESS_MEDIA_LOCATION`) cannot change it;
//! - a missing asked flag cannot pin a permission at NotDetermined: the
//!   rationale is read before the flag, and a fast denial is recorded as a
//!   system refusal (never for MediaLocation);
//! - MediaLocation reads Denied unless photo access is full;
//! - one request is in flight at a time, on both sides.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use frust_permissions::codes::{self, __contract};
use frust_permissions::{Permission, PhotoAccess};

const KOTLIN_HOST: &str =
    "platform/android/src/main/kotlin/dev/frust/permissions/FrustPermissionsHost.kt";
const KOTLIN_PROVIDER: &str =
    "platform/android/src/main/kotlin/dev/frust/permissions/FrustPermissionsInitProvider.kt";
const MANIFEST: &str = "platform/android/src/main/AndroidManifest.xml";
const CONSUMER_RULES: &str = "platform/android/consumer-rules.pro";
const RUST_ANDROID: &str = "src/android.rs";

fn read(rel: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// `const val NAME = <int>` lines of a Kotlin file, by name.
fn kotlin_int_consts(contents: &str) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for line in contents.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("const val ") else {
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
        let line = line.trim();
        let rest = line
            .strip_prefix("const val ")
            .or_else(|| line.strip_prefix("private const val "));
        let Some((name, value)) = rest.and_then(|r| r.split_once('=')) else {
            continue;
        };
        let value = value.trim();
        if let Some(text) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
            out.insert(name.trim().to_string(), text.to_string());
        }
    }
    out
}

/// The `private val NAME = listOf(...)` / `emptyList<String>()` permission
/// sets, by name.
fn kotlin_permission_sets(contents: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for line in contents.lines() {
        let Some(rest) = line.trim().strip_prefix("private val ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let items = if value == "emptyList<String>()" {
            Vec::new()
        } else if let Some(inner) = value
            .strip_prefix("listOf(")
            .and_then(|v| v.strip_suffix(')'))
        {
            inner
                .split(',')
                .map(|item| item.trim().trim_matches('"').to_string())
                .filter(|item| !item.is_empty())
                .collect()
        } else {
            continue;
        };
        out.insert(name.trim().to_string(), items);
    }
    out
}

/// Every `"android.permission.*"` literal in `contents`.
fn permission_literals(contents: &str) -> BTreeSet<String> {
    contents
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|s| s.starts_with("android.permission."))
        .map(str::to_string)
        .collect()
}

/// The text of the Kotlin function `name`, from its `fun` line through its
/// closing brace at the same indentation.
fn kotlin_fn<'a>(contents: &'a str, name: &str) -> &'a str {
    let marker = format!("fun {name}(");
    let start = contents
        .find(&marker)
        .unwrap_or_else(|| panic!("Kotlin host has no `{marker}`"));
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

#[test]
fn kotlin_codes_equal_their_rust_twins() {
    let kotlin = kotlin_int_consts(&read(KOTLIN_HOST));
    let rust = [
        ("NOT_DETERMINED", __contract::NOT_DETERMINED),
        ("GRANTED", __contract::GRANTED),
        ("LIMITED", __contract::LIMITED),
        ("DENIED", __contract::DENIED),
        ("BLOCKED", __contract::BLOCKED),
        ("RESTRICTED", __contract::RESTRICTED),
        ("PENDING", __contract::PENDING),
        ("NO_ACTIVITY", __contract::NO_ACTIVITY),
        ("NOT_DECLARED", __contract::NOT_DECLARED),
        ("PHOTOS_READ_WRITE", __contract::PHOTOS_READ_WRITE),
        ("PHOTOS_ADD_ONLY", __contract::PHOTOS_ADD_ONLY),
        ("CAMERA", __contract::CAMERA),
        ("MICROPHONE", __contract::MICROPHONE),
        ("MEDIA_LOCATION", __contract::MEDIA_LOCATION),
    ];
    for (name, value) in rust {
        assert_eq!(
            kotlin.get(name).copied(),
            Some(i64::from(value)),
            "Kotlin `{name}` must equal codes.rs's {value}"
        );
    }
}

#[test]
fn open_settings_codes_match_android_rs() {
    let kotlin = kotlin_int_consts(&read(KOTLIN_HOST));
    let rust = read(RUST_ANDROID);
    for name in ["SETTINGS_OPENED", "SETTINGS_FAILED"] {
        let value = kotlin
            .get(name)
            .unwrap_or_else(|| panic!("Kotlin host has no `{name}`"));
        let line = format!("const {name}: jint = {value};");
        assert!(
            rust.contains(&line),
            "src/android.rs must declare `{line}` to match the Kotlin host"
        );
    }
}

#[test]
fn kotlin_permission_sets_equal_codes_rs() {
    let sets = kotlin_permission_sets(&read(KOTLIN_HOST));
    let rw = Permission::Photos(PhotoAccess::ReadWrite);
    let all_sdks = 29..=36;
    let expected: [(&str, Permission, Vec<i32>); 7] = [
        ("PHOTOS_READ_WRITE_SDK29", rw, (29..=32).collect()),
        ("PHOTOS_READ_WRITE_SDK33", rw, vec![33]),
        ("PHOTOS_READ_WRITE_SDK34", rw, (34..=36).collect()),
        (
            "PHOTOS_ADD_ONLY_ALL",
            Permission::Photos(PhotoAccess::AddOnly),
            all_sdks.clone().collect(),
        ),
        ("CAMERA_ALL", Permission::Camera, all_sdks.clone().collect()),
        (
            "MICROPHONE_ALL",
            Permission::Microphone,
            all_sdks.clone().collect(),
        ),
        (
            "MEDIA_LOCATION_ALL",
            Permission::MediaLocation,
            all_sdks.collect(),
        ),
    ];
    assert_eq!(
        sets.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        expected.iter().map(|(n, _, _)| *n).collect::<BTreeSet<_>>(),
        "the Kotlin host's permission sets and this test's table must name the same sets"
    );
    for (name, permission, sdks) in &expected {
        for sdk in sdks {
            assert_eq!(
                sets[*name],
                codes::android_permissions(*permission, *sdk),
                "Kotlin `{name}` must equal codes::android_permissions({permission:?}, {sdk})"
            );
        }
    }
}

#[test]
fn kotlin_names_no_permission_outside_codes_rs() {
    let all = [
        Permission::Photos(PhotoAccess::ReadWrite),
        Permission::Photos(PhotoAccess::AddOnly),
        Permission::Camera,
        Permission::Microphone,
        Permission::MediaLocation,
    ];
    let known: BTreeSet<String> = all
        .iter()
        .flat_map(|p| (29..=36).flat_map(move |sdk| codes::android_permissions(*p, sdk).iter()))
        .map(|s| s.to_string())
        .collect();
    let named = permission_literals(&read(KOTLIN_HOST));
    let unknown: Vec<_> = named.difference(&known).collect();
    assert!(
        unknown.is_empty(),
        "the Kotlin host names permissions codes.rs does not: {unknown:?}"
    );
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
    assert_eq!(package, "dev.frust.permissions");
    assert!(kotlin.contains("\nobject FrustPermissionsHost {"));

    let fqcn = format!("{package}.FrustPermissionsHost");
    assert!(
        rust.contains(&format!("const HOST_CLASS_BINARY: &str = \"{fqcn}\";")),
        "src/android.rs must load `{fqcn}`"
    );

    assert!(
        kotlin.contains("external fun nativeOnPermissionResult(requestId: Long, code: Int)"),
        "Kotlin `nativeOnPermissionResult` signature changed"
    );
    let symbol = format!(
        "Java_{}_FrustPermissionsHost_nativeOnPermissionResult",
        package.replace('.', "_")
    );
    assert!(
        rust.contains(&format!("pub extern \"system\" fn {symbol}<")),
        "src/android.rs must export `{symbol}`"
    );
    let export = &rust[rust.find(&symbol).expect("export present")..];
    assert!(export.contains("request_id: jlong,") && export.contains("code: jint,"));
}

#[test]
fn rust_call_signatures_match_kotlin_declarations() {
    let kotlin = read(KOTLIN_HOST);
    let rust = read(RUST_ANDROID);
    let calls = [
        ("status", "(I)I", "fun status(kind: Int): Int"),
        (
            "request",
            "(JI)I",
            "fun request(requestId: Long, kind: Int): Int",
        ),
        (
            "undeclaredPermission",
            "(I)Ljava/lang/String;",
            "fun undeclaredPermission(kind: Int): String?",
        ),
        ("openSettings", "()I", "fun openSettings(): Int"),
    ];
    for (method, sig, declaration) in calls {
        let jvm_static = format!("@JvmStatic\n    {declaration}");
        assert!(
            kotlin.contains(&jvm_static),
            "Kotlin host must declare `@JvmStatic {declaration}`"
        );
        let call = format!("jni_str!(\"{method}\")");
        let at = rust
            .find(&call)
            .unwrap_or_else(|| panic!("src/android.rs never calls `{method}`"));
        let window = &rust[at..(at + 120).min(rust.len())];
        assert!(
            window.contains(&format!("jni_sig!(\"{sig}\")")),
            "src/android.rs must call `{method}` with `{sig}`"
        );
    }
}

#[test]
fn module_manifest_declares_no_permission_and_the_init_provider() {
    let manifest = read(MANIFEST);
    assert!(
        !manifest.contains("uses-permission"),
        "the module manifest must not declare permissions; the app declares them"
    );
    assert!(
        manifest.contains("android:name=\"dev.frust.permissions.FrustPermissionsInitProvider\"")
    );
    assert!(manifest.contains("android:authorities=\"${applicationId}.frustpermissionsinit\""));
    assert!(manifest.contains("android:exported=\"false\""));
    assert!(
        read(KOTLIN_PROVIDER).contains("class FrustPermissionsInitProvider : ContentProvider()")
    );
}

#[test]
fn keep_rules_cover_the_host_and_the_provider() {
    let rules = read(CONSUMER_RULES);
    for line in [
        "-keep class dev.frust.permissions.FrustPermissionsHost { *; }",
        "-keep class dev.frust.permissions.FrustPermissionsInitProvider { *; }",
    ] {
        assert!(
            rules.contains(line),
            "consumer-rules.pro must keep `{line}`"
        );
    }
}

#[test]
fn asked_flag_storage_names_are_stable() {
    let strings = kotlin_string_consts(&read(KOTLIN_HOST));
    assert_eq!(
        strings.get("PREFS_FILE").map(String::as_str),
        Some("frust.permissions")
    );
    assert_eq!(
        strings.get("ASKED_KEY_PREFIX").map(String::as_str),
        Some("frust.permissions.asked.")
    );
}

#[test]
fn limited_derivation_ignores_what_the_manifest_declares() {
    let kotlin = read(KOTLIN_HOST);
    let derivation = kotlin_fn(&kotlin, "photosGrantCode");
    assert!(
        derivation
            .contains("photosGrantCode(sdk: Int, fullRead: Boolean, userSelected: Boolean): Int?"),
        "the derivation must take grant state only"
    );
    for needle in [
        "declared",
        "Declared",
        "requestedPermissions",
        "ctx",
        "Context",
    ] {
        assert!(
            !derivation.contains(needle),
            "photosGrantCode must not consult `{needle}`"
        );
    }
    assert!(derivation.contains("sdk >= 34 && userSelected -> LIMITED"));

    // The partial-access permission is optional in the declaration check, so
    // an app in compatibility mode is never reported as undeclared.
    let strings = kotlin_string_consts(&kotlin);
    assert_eq!(
        strings
            .get("READ_MEDIA_VISUAL_USER_SELECTED")
            .map(String::as_str),
        Some("android.permission.READ_MEDIA_VISUAL_USER_SELECTED")
    );
    assert!(kotlin.contains("OPTIONAL_DECLARATIONS = setOf(READ_MEDIA_VISUAL_USER_SELECTED)"));
    let undeclared = kotlin_fn(&kotlin, "firstUndeclared");
    assert!(undeclared.contains("it !in OPTIONAL_DECLARATIONS"));
}

#[test]
fn media_location_is_denied_without_full_photos_access() {
    let kotlin = read(KOTLIN_HOST);
    let current = kotlin_fn(&kotlin, "currentCode");
    assert!(
        current.contains("kind == MEDIA_LOCATION"),
        "currentCode must special-case MediaLocation"
    );
    assert!(
        current.contains("mediaLocationCode(photosCode(ctx, sdk))"),
        "the MediaLocation path must consult the Photos derivation"
    );
    let photos = kotlin_fn(&kotlin, "photosCode");
    assert!(photos.contains("photosGrantCode(sdk,"));

    let mapping = kotlin_fn(&kotlin, "mediaLocationCode");
    assert!(
        mapping.contains("if (photos != GRANTED) DENIED else null"),
        "any photo access short of GRANTED (partial or none) must map to DENIED"
    );
    assert!(
        !mapping.contains("LIMITED"),
        "the mapping must not single out partial access"
    );
    assert!(
        !mapping.contains("wasAsked") && !mapping.contains("markAsked"),
        "the photo-access decision must not use the asked flag"
    );
}

/// The byte offset of `needle` in `haystack`, failing with `what`.
fn offset(haystack: &str, needle: &str, what: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("{what}: `{needle}` not found"))
}

#[test]
fn rationale_is_read_before_the_not_determined_return() {
    let kotlin = read(KOTLIN_HOST);
    let current = kotlin_fn(&kotlin, "currentCode");
    let rationale = offset(
        current,
        "perms.filter { !isGranted(ctx, it) && rationale(activity, it) }",
        "currentCode must read the rationale of the non-granted permissions",
    );
    let not_determined = offset(
        current,
        "return NOT_DETERMINED",
        "currentCode must still return NOT_DETERMINED",
    );
    assert!(
        rationale < not_determined,
        "the rationale check must run before the NOT_DETERMINED return"
    );
    let mark = offset(
        current,
        "withRationale.forEach { markAsked(ctx, it) }",
        "a rationale must write the asked flag",
    );
    let denied = offset(current, "return DENIED", "a rationale must read DENIED");
    assert!(rationale < mark && mark < denied && denied < not_determined);
    assert!(
        !current.contains("perms.any { rationale(activity, it) }"),
        "the rationale is decided once, before the asked flag"
    );

    // Recording an answer: a denial with a rationale after it is never a
    // dismissed prompt.
    let record = kotlin_fn(&kotlin, "recordAnswers");
    assert!(record.contains("val rationaleAfter = !granted && rationale(activity, perm)"));
    assert!(
        record.contains("!prior.asked && !prior.rationale && !rationaleAfter"),
        "a rationale after the answer must rule out the dismissed-prompt inference"
    );
}

#[test]
fn a_fast_denial_is_recorded_as_a_system_refusal_except_media_location() {
    let kotlin = read(KOTLIN_HOST);
    assert!(
        kotlin.contains("private const val AUTO_DENY_MS = 500L"),
        "the auto-denial bound must be the named 500 ms constant"
    );

    let refusal = kotlin_fn(&kotlin, "isSystemRefusal");
    assert!(refusal.contains(
        "isSystemRefusal(kind: Int, granted: Boolean, rationaleAfter: Boolean, elapsedMs: Long): Boolean"
    ));
    for needle in [
        "kind != MEDIA_LOCATION",
        "!granted",
        "!rationaleAfter",
        "elapsedMs < AUTO_DENY_MS",
    ] {
        assert!(
            refusal.contains(needle),
            "isSystemRefusal must require `{needle}`"
        );
    }

    let record = kotlin_fn(&kotlin, "recordAnswers");
    let decision = offset(
        record,
        "if (!dismissed || isSystemRefusal(kind, granted, rationaleAfter, elapsedMs)) {",
        "recordAnswers must record a system refusal",
    );
    assert!(
        record[decision..].contains("markAsked(ctx, perm)"),
        "a system refusal must write the asked flag"
    );

    let launch = kotlin_fn(&kotlin, "launch");
    let stamped = offset(
        launch,
        "launchedAt = SystemClock.elapsedRealtime()",
        "launch must record the launch time",
    );
    let launched = offset(
        launch,
        "registered.launch(",
        "launch must launch the launcher",
    );
    assert!(
        stamped < launched,
        "the launch time is taken before the launch"
    );
    assert!(launch.contains("val elapsedMs = SystemClock.elapsedRealtime() - launchedAt"));
    assert!(launch.contains("recordAnswers(ctx, activity, kind, results, before, elapsedMs)"));
}

#[test]
fn one_request_is_in_flight_at_a_time() {
    let kotlin = read(KOTLIN_HOST);
    let request = kotlin_fn(&kotlin, "request");
    let guard = offset(
        request,
        "if (!claimInFlight(requestId, activity)) {",
        "request must check the in-flight guard",
    );
    let current = offset(
        request,
        "return currentCode(ctx, kind, perms, activity)",
        "a guarded request must return the current status code",
    );
    let launch = offset(
        request,
        "launch(ctx, requestId,",
        "request must still launch an unguarded prompt",
    );
    assert!(
        guard < current && current < launch,
        "the in-flight guard must return the current code before any launch"
    );

    let claim = kotlin_fn(&kotlin, "claimInFlight");
    assert!(claim.contains("if (current != null && !current.activity.isDestroyed) return false"));
    let launch_fn = kotlin_fn(&kotlin, "launch");
    assert_eq!(
        launch_fn.matches("releaseInFlight(requestId)").count(),
        3,
        "the slot is freed when the Activity is gone, in the callback, and on a failed launch"
    );
    assert!(kotlin.contains("releaseInFlightFor(activity)"));

    // Rust: one process-wide mutex held for the whole request.
    let rust = read(RUST_ANDROID);
    assert!(rust.contains("static REQUEST_LOCK: Mutex<()> = Mutex::new(());"));
    let body = &rust[offset(
        &rust,
        "pub(crate) fn request(p: Permission)",
        "src/android.rs must define request",
    )..];
    let body = &body[..offset(body, "\n}\n", "request must end")];
    let held = offset(
        body,
        "let _one_at_a_time = lock(&REQUEST_LOCK);",
        "request must hold REQUEST_LOCK in a named guard",
    );
    let minted = offset(body, "NEXT_REQUEST_ID.fetch_add", "request must mint an id");
    let awaited = offset(body, "await_answer(request_id)", "request must wait");
    assert!(
        held < minted && minted < awaited,
        "REQUEST_LOCK must be held before the id is minted and across the wait"
    );
    let deliver = &rust[offset(&rust, "fn deliver(", "src/android.rs must define deliver")..];
    let deliver = &deliver[..offset(deliver, "\n}\n", "deliver must end")];
    assert!(
        !deliver.contains("REQUEST_LOCK"),
        "deliver must never take REQUEST_LOCK, or a held request could not be answered"
    );
}

#[test]
fn scanners_read_the_shapes_they_claim() {
    let sample = "    const val A = 1\n    const val B = -1\n    const val S = \"x\"\n    \
                  private val L = listOf(\"p.a\", \"p.b\")\n    \
                  private val E = emptyList<String>()\n";
    let ints = kotlin_int_consts(sample);
    assert_eq!(ints.get("A"), Some(&1));
    assert_eq!(ints.get("B"), Some(&-1));
    assert!(!ints.contains_key("S"));
    assert_eq!(
        kotlin_string_consts(sample).get("S").map(String::as_str),
        Some("x")
    );
    let sets = kotlin_permission_sets(sample);
    assert_eq!(sets["L"], ["p.a", "p.b"]);
    assert!(sets["E"].is_empty());
}

#[test]
fn photos_use_the_declared_media_subset() {
    let kotlin = read(KOTLIN_HOST);
    let strings = kotlin_string_consts(&kotlin);
    assert_eq!(
        strings.get("READ_MEDIA_VIDEO").map(String::as_str),
        Some("android.permission.READ_MEDIA_VIDEO")
    );

    // Status, request and the naming read all go through the narrowed set.
    for entry in ["status", "request", "undeclaredPermission"] {
        let body = kotlin_fn(&kotlin, entry);
        assert!(
            body.contains("declaredPermissionsFor(ctx, kind, Build.VERSION.SDK_INT)"),
            "`{entry}` must use the declared media subset"
        );
        assert!(
            !body.contains("permissionsFor(kind, Build.VERSION.SDK_INT)"),
            "`{entry}` must not read the full candidate set"
        );
    }

    // Only API 33+ narrows, and only Photos read-write.
    let narrowed = kotlin_fn(&kotlin, "declaredPermissionsFor");
    assert!(narrowed.contains("kind != PHOTOS_READ_WRITE || sdk < 33"));
    assert!(narrowed.contains("narrowMediaPermissions(perms, declaredPermissions(ctx))"));

    // With a media type declared the subset is used; with neither, the full
    // set comes back so `firstUndeclared` names READ_MEDIA_IMAGES.
    let narrow = kotlin_fn(&kotlin, "narrowMediaPermissions");
    assert!(narrow.contains("perms.filter { it in declared }"));
    assert!(narrow.contains("it != READ_MEDIA_VISUAL_USER_SELECTED"));
    assert!(narrow.contains("if (hasMediaType) subset else perms"));
    assert!(
        kotlin.contains("PHOTOS_READ_WRITE_SDK33 = listOf(\"android.permission.READ_MEDIA_IMAGES\", \"android.permission.READ_MEDIA_VIDEO\")"),
        "READ_MEDIA_IMAGES must stay first so it is the name reported"
    );

    // Either declared media type granted is full read access.
    let photos = kotlin_fn(&kotlin, "photosCode");
    assert!(
        photos.contains("isGranted(ctx, READ_MEDIA_IMAGES) || isGranted(ctx, READ_MEDIA_VIDEO)")
    );
}
