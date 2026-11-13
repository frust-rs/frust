//! Gradle task-name computation and `-P` property assembly:
//! `assemble<Flavor><Mode>`/`bundle<Flavor><Mode>` task selection (with
//! [`FlavorRoute`] deciding whether a `--flavor` is a Gradle product flavor
//! or a `frust.toml`-only one), plus the `-Pfrust.*` properties the Gradle
//! template's `build.gradle.kts.tmpl` consumes.

use std::collections::HashMap;
use std::path::Path;

use crate::android_build::AndroidArtifact;
use crate::build_info::BuildMode;

/// Gradle ABI names for every supported target platform, in the order
/// `commands::build::TARGET_PLATFORMS` maps `--target-platform` values —
/// what `build appbundle` always requests (an `.aab` packages every ABI).
pub const ALL_ABIS: &[&str] = &["arm64-v8a", "armeabi-v7a", "x86_64"];

/// Computes the Gradle task name for `target`/`mode`/`flavor`:
/// `assemble<Flavor><Mode>` for an APK, `bundle<Flavor><Mode>` for an App
/// Bundle. `flavor` is capitalized (first char upper, rest as given) and
/// omitted entirely when `None`.
pub fn task_name(target: &AndroidArtifact, mode: BuildMode, flavor: Option<&str>) -> String {
    let verb = match target {
        AndroidArtifact::Apk { .. } => "assemble",
        AndroidArtifact::Appbundle => "bundle",
    };
    let flavor_part = flavor.map(capitalize).unwrap_or_default();
    format!("{verb}{flavor_part}{}", mode.gradle_infix())
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// How a requested `--flavor` reaches Gradle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FlavorRoute {
    /// No `--flavor`: the plain `assemble<Mode>`/`bundle<Mode>` task.
    Unflavored,
    /// `android/app/build.gradle.kts` declares the flavor in
    /// `productFlavors`: the flavored task, as before `[flavors]` existed.
    Native(String),
    /// Declared only in `frust.toml`'s `[flavors]`: the plain task, with the
    /// identity carried through `local.properties` instead.
    ManifestOnly(String),
    /// Declared nowhere: the flavored task, so Gradle's own "task not
    /// found" failure (re-explained by the build lane) still surfaces.
    Unknown(String),
}

/// `<android_dir>/app/build.gradle.kts`'s text, `None` when unreadable.
pub(crate) fn read_app_script(android_dir: &Path) -> Option<String> {
    std::fs::read_to_string(android_dir.join("app").join("build.gradle.kts")).ok()
}

impl FlavorRoute {
    /// Routes `flavor` given the app module's build script text (`None` when
    /// it could not be read) and whether `frust.toml` declares the flavor
    /// (`AppIdentity::flavor_declared`). A flavor declared both as a product
    /// flavor and in `frust.toml` is an error, before Gradle runs.
    pub(crate) fn select(
        script: Option<&str>,
        flavor: Option<&str>,
        flavor_declared: bool,
    ) -> anyhow::Result<Self> {
        let Some(flavor) = flavor else {
            return Ok(Self::Unflavored);
        };
        let name = flavor.to_string();
        let native = script.is_some_and(|script| declares_product_flavor(script, flavor));
        Ok(match (native, flavor_declared) {
            (true, true) => anyhow::bail!(
                "{}",
                crate::identity::flavor_declared_twice(
                    flavor,
                    "as a Gradle product flavor in android/app/build.gradle.kts"
                )
            ),
            (true, false) => Self::Native(name),
            (false, true) => Self::ManifestOnly(name),
            (false, false) => Self::Unknown(name),
        })
    }

    /// The flavor the Gradle task and its output directory carry.
    pub(crate) fn gradle_flavor(&self) -> Option<&str> {
        match self {
            Self::Native(flavor) | Self::Unknown(flavor) => Some(flavor),
            Self::Unflavored | Self::ManifestOnly(_) => None,
        }
    }

    /// Whether a native Gradle product flavor was selected — one that may
    /// add its own `applicationIdSuffix`.
    pub(crate) fn is_native(&self) -> bool {
        matches!(self, Self::Native(_))
    }
}

/// Whether the Kotlin build script `script` declares `flavor` inside a
/// `productFlavors { … }` block, as `create("<flavor>")`,
/// `register("<flavor>")` or `"<flavor>" {`. A text scan, not a Kotlin
/// parser: comments are skipped (the template ships a commented example)
/// and braces inside string literals do not count.
pub(crate) fn declares_product_flavor(script: &str, flavor: &str) -> bool {
    let code = strip_comments(script);
    let quoted = format!("\"{flavor}\"");
    let mut from = 0;
    while let Some(at) = code[from..].find("productFlavors") {
        let after = from + at + "productFlavors".len();
        from = after;
        let rest = code[after..].trim_start();
        if !rest.starts_with('{') {
            continue;
        }
        let open = code.len() - rest.len();
        let Some(body) = block_body(&code, open) else {
            continue;
        };
        if body_declares(body, &quoted) {
            return true;
        }
    }
    false
}

/// Whether the Kotlin build script `script` names the `local.properties` key
/// `key` as a string literal outside comments — how the generated template
/// reads `frust.applicationId`/`frust.appLabel`. A text scan, like
/// [`declares_product_flavor`].
pub(crate) fn reads_property(script: &str, key: &str) -> bool {
    strip_comments(script).contains(&format!("\"{key}\""))
}

/// Whether a `productFlavors` block body names `quoted` (`"<flavor>"`) as a
/// `create(…)`/`register(…)` argument or as a `"<flavor>" {` block.
fn body_declares(body: &str, quoted: &str) -> bool {
    let mut from = 0;
    while let Some(at) = body[from..].find(quoted) {
        let start = from + at;
        let end = start + quoted.len();
        from = end;
        let before = body[..start].trim_end();
        let after = body[end..].trim_start();
        let called = (before.ends_with("create(") || before.ends_with("register("))
            && after.starts_with(')');
        if called || after.starts_with('{') {
            return true;
        }
    }
    false
}

/// The text between the `{` at byte `open` of `code` and its matching `}`,
/// skipping braces inside string literals; `None` when it never closes.
fn block_body(code: &str, open: usize) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (at, c) in code[open..].char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&code[open + 1..open + at]);
                }
            }
            _ => {}
        }
    }
    None
}

/// `script` with its `//` and `/* */` comments blanked out (each replaced
/// by one space, newlines kept), string literals left intact.
fn strip_comments(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut chars = script.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = '\0';
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                    }
                    if last == '*' && next == '/' {
                        break;
                    }
                    last = next;
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    out
}

/// Assembles the `-P` properties the Gradle template's `build.gradle.kts.tmpl`
/// consumes: `-Pfrust.targetPlatforms=<abi,csv>` (always),
/// `-Pfrust.splitPerAbi=true|false` (APK only),
/// `-Pfrust.defines=<base64("K=V;K=V")>` when `defines` is non-empty, and
/// `-Pfrust.cargoFeatures=<base64("feat,feat")>` for the resolved cargo
/// `features` — the exact seam `-Pfrust.defines` rides, so a feature flag
/// can never fall foul of the Android release env-drop Known Issue (it's a
/// build arg, not an environment variable).
///
/// `features` is the release-lean-preflight-resolved list
/// (`cargo_manifest::resolve_release_features`), not the raw
/// `BuildMode::cargo_features` — a legacy app that doesn't declare `lean`
/// arrives here with `lean` already dropped, so the `cargoFeatures` prop is
/// simply omitted (empty → no prop) rather than passing an undeclared feature
/// to `cargo ndk`. That resolved list also carries the front-end's
/// `--features` passthrough, appended after the mode's own selection, so the
/// extras ride this one CSV rather than a second property: the Gradle side
/// decodes the whole value into a single `--features <csv>` argument, which is
/// exactly cargo's own comma-separated syntax.
///
/// Because Gradle passes the CSV to `cargo ndk` verbatim, `features` must
/// already be spelled with the app's facade dependency key
/// ([`crate::facade_key::FacadeKey::map`], as `build` and `android_run` do):
/// `frust-ui/devtools` for an app depending on `frust-ui = ".."`. This
/// function encodes what it is given and maps nothing itself.
pub fn gradle_properties(
    target: &AndroidArtifact,
    defines: &HashMap<String, String>,
    features: &[&str],
) -> Vec<String> {
    let mut props = Vec::new();

    match target {
        AndroidArtifact::Apk {
            split_per_abi,
            abis,
        } => {
            props.push(format!("-Pfrust.targetPlatforms={}", abis.join(",")));
            props.push(format!("-Pfrust.splitPerAbi={split_per_abi}"));
        }
        AndroidArtifact::Appbundle => {
            props.push(format!("-Pfrust.targetPlatforms={}", ALL_ABIS.join(",")));
        }
    }

    if !defines.is_empty() {
        props.push(format!("-Pfrust.defines={}", encode_defines(defines)));
    }

    if let Some(features_b64) = encode_features(features) {
        props.push(format!("-Pfrust.cargoFeatures={features_b64}"));
    }

    props
}

/// Encodes `defines` as `base64("K=V;K=V")`, sorted by key for a
/// deterministic, testable output (a `HashMap`'s natural iteration order is
/// not stable) — the exact value the Gradle template's `build.gradle.kts.tmpl`
/// decodes with `Base64.getDecoder()`.
fn encode_defines(defines: &HashMap<String, String>) -> String {
    let mut keys: Vec<&String> = defines.keys().collect();
    keys.sort();
    let joined = keys
        .iter()
        .map(|k| format!("{k}={}", defines[*k]))
        .collect::<Vec<_>>()
        .join(";");
    base64_encode(joined.as_bytes())
}

/// Encodes the resolved cargo `features` as `base64("feat,feat")` — the exact
/// value the Gradle template's `build.gradle.kts.tmpl` decodes with
/// `Base64.getDecoder()` and appends to the cargo-ndk invocation as
/// `--features <csv>`. `None` when there are no features to pass (the
/// release-lean preflight can legitimately produce an empty list for a legacy
/// app whose `lean` feature was dropped — see
/// `cargo_manifest::resolve_release_features`), in which case no
/// `-Pfrust.cargoFeatures` prop is emitted at all.
fn encode_features(features: &[&str]) -> Option<String> {
    if features.is_empty() {
        return None;
    }
    Some(base64_encode(features.join(",").as_bytes()))
}

const BASE64_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A hand-rolled standard (RFC 4648, padded) base64 encoder — matches
/// `java.util.Base64.getDecoder()`'s expected alphabet on the Gradle side
/// (CODE_STANDARDS: hand-roll at the platform-template boundary rather than
/// add a dependency for a single call site).
fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(BASE64_ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(BASE64_ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[((n >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[(n & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apk(abis: &[&str], split_per_abi: bool) -> AndroidArtifact {
        AndroidArtifact::Apk {
            split_per_abi,
            abis: abis.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn task_name_table() {
        let cases: &[(&str, AndroidArtifact, BuildMode, Option<&str>, &str)] = &[
            (
                "apk debug no flavor",
                apk(&["arm64-v8a"], false),
                BuildMode::Debug,
                None,
                "assembleDebug",
            ),
            (
                "apk release no flavor",
                apk(&["arm64-v8a"], false),
                BuildMode::Release,
                None,
                "assembleRelease",
            ),
            (
                "apk profile no flavor",
                apk(&["arm64-v8a"], false),
                BuildMode::Profile,
                None,
                "assembleProfile",
            ),
            (
                "apk release with flavor",
                apk(&["arm64-v8a"], false),
                BuildMode::Release,
                Some("paid"),
                "assemblePaidRelease",
            ),
            (
                "apk debug with already-capitalized flavor",
                apk(&["arm64-v8a"], false),
                BuildMode::Debug,
                Some("Paid"),
                "assemblePaidDebug",
            ),
            (
                "appbundle release no flavor",
                AndroidArtifact::Appbundle,
                BuildMode::Release,
                None,
                "bundleRelease",
            ),
            (
                "appbundle release with flavor",
                AndroidArtifact::Appbundle,
                BuildMode::Release,
                Some("paid"),
                "bundlePaidRelease",
            ),
            (
                "appbundle profile with flavor",
                AndroidArtifact::Appbundle,
                BuildMode::Profile,
                Some("free"),
                "bundleFreeProfile",
            ),
        ];

        for (label, target, mode, flavor, expected) in cases {
            assert_eq!(
                &task_name(target, *mode, *flavor),
                expected,
                "case: {label}"
            );
        }
    }

    /// A Kotlin build script declaring `dev` with `create`, `paid` with
    /// `register` and `free` as a `"free" {` block, beside a commented-out
    /// `create("staging")` and a string holding a stray brace.
    const SCRIPT_DECLARING: &str = r#"
android {
    namespace = "com.example.app"
    // productFlavors { create("staging") }
    flavorDimensions += "env"
    productFlavors {
        create("dev") { dimension = "env"; applicationIdSuffix = ".dev" }
        register( "paid" ) {
            dimension = "env"
            resValue("string", "brace", "}")
        }
        "free" {
            dimension = "env"
        }
    }
    /* productFlavors { create("beta") } */
}
"#;

    #[test]
    fn product_flavor_scan_finds_create_register_and_block_declarations() {
        for flavor in ["dev", "paid", "free"] {
            assert!(
                declares_product_flavor(SCRIPT_DECLARING, flavor),
                "{flavor}"
            );
        }
        for flavor in ["staging", "beta", "env", "brace", "de"] {
            assert!(
                !declares_product_flavor(SCRIPT_DECLARING, flavor),
                "{flavor}"
            );
        }
    }

    /// The generated template: its only `productFlavors` block is the
    /// commented example, so no flavor is native.
    #[test]
    fn product_flavor_scan_ignores_the_templates_commented_example() {
        let template = include_str!("../../templates/app/android.tmpl/app/build.gradle.kts.tmpl");
        assert!(
            template.contains("//     create(\"dev\")"),
            "the example moved"
        );
        for flavor in ["dev", "prod"] {
            assert!(!declares_product_flavor(template, flavor), "{flavor}");
        }
    }

    /// A flavor named outside `productFlavors` (a build type, a signing
    /// config) is not a product flavor.
    #[test]
    fn product_flavor_scan_needs_the_product_flavors_block() {
        let script = r#"
android {
    buildTypes { create("dev") { initWith(getByName("debug")) } }
    signingConfigs { create("paid") { } }
    productFlavors {
    }
}
"#;
        assert!(!declares_product_flavor(script, "dev"));
        assert!(!declares_product_flavor(script, "paid"));
        assert!(!declares_product_flavor(
            "productFlavors { create(\"dev\")",
            "dev"
        ));
    }

    /// A flavor that is both a product flavor and a `[flavors]` entry is
    /// refused, naming the flavor and both declarations.
    #[test]
    fn a_flavor_declared_natively_and_in_frust_toml_is_refused() {
        let err = FlavorRoute::select(Some(SCRIPT_DECLARING), Some("dev"), true).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("flavor `dev`"), "{message}");
        assert!(
            message.contains("Gradle product flavor in android/app/build.gradle.kts"),
            "{message}"
        );
        assert!(
            message.contains("`[flavors.dev]` in frust.toml"),
            "{message}"
        );
    }

    #[test]
    fn the_property_scan_needs_a_read_outside_comments() {
        let template = include_str!("../../templates/app/android.tmpl/app/build.gradle.kts.tmpl");
        assert!(reads_property(template, "frust.applicationId"));
        assert!(reads_property(template, "frust.appLabel"));
        let unmigrated = "// localProperties.getProperty(\"frust.applicationId\", \"x\")\n\
                          android { defaultConfig { applicationId = \"dev.f0x.myapp\" } }\n";
        assert!(!reads_property(unmigrated, "frust.applicationId"));
        assert!(!reads_property(unmigrated, "frust.appLabel"));
        assert!(!reads_property(
            "val x = getProperty(\"frust.applicationIdX\")",
            "frust.applicationId"
        ));
    }

    #[test]
    fn flavor_route_prefers_native_then_frust_toml_then_unknown() {
        let release = BuildMode::Release;
        let target = apk(&["arm64-v8a"], false);
        /// (label, build script, `--flavor`, declared in frust.toml, route, task)
        type Case<'a> = (
            &'a str,
            Option<&'a str>,
            Option<&'a str>,
            bool,
            FlavorRoute,
            &'a str,
        );
        let cases: &[Case] = &[
            (
                "no flavor",
                Some(SCRIPT_DECLARING),
                None,
                false,
                FlavorRoute::Unflavored,
                "assembleRelease",
            ),
            (
                "native flavor",
                Some(SCRIPT_DECLARING),
                Some("dev"),
                false,
                FlavorRoute::Native("dev".into()),
                "assembleDevRelease",
            ),
            (
                "frust.toml-only flavor",
                Some(SCRIPT_DECLARING),
                Some("staging"),
                true,
                FlavorRoute::ManifestOnly("staging".into()),
                "assembleRelease",
            ),
            (
                "frust.toml-only flavor, unreadable script",
                None,
                Some("staging"),
                true,
                FlavorRoute::ManifestOnly("staging".into()),
                "assembleRelease",
            ),
            (
                "unknown flavor",
                Some(SCRIPT_DECLARING),
                Some("staging"),
                false,
                FlavorRoute::Unknown("staging".into()),
                "assembleStagingRelease",
            ),
        ];
        for (label, script, flavor, declared, route, task) in cases {
            let got = FlavorRoute::select(*script, *flavor, *declared).unwrap();
            assert_eq!(&got, route, "case: {label}");
            assert_eq!(
                &task_name(&target, release, got.gradle_flavor()),
                task,
                "case: {label}"
            );
            assert_eq!(
                got.is_native(),
                matches!(route, FlavorRoute::Native(_)),
                "case: {label}"
            );
        }
    }

    #[test]
    fn gradle_properties_apk_includes_target_platforms_and_split_per_abi() {
        let target = apk(&["arm64-v8a", "x86_64"], true);
        let props = gradle_properties(&target, &HashMap::new(), BuildMode::Debug.cargo_features());
        assert_eq!(
            props,
            vec![
                "-Pfrust.targetPlatforms=arm64-v8a,x86_64".to_string(),
                "-Pfrust.splitPerAbi=true".to_string(),
                // debug selects `frust/perf-trace,frust/devtools`, base64-encoded.
                "-Pfrust.cargoFeatures=ZnJ1c3QvcGVyZi10cmFjZSxmcnVzdC9kZXZ0b29scw==".to_string(),
            ]
        );
    }

    #[test]
    fn gradle_properties_appbundle_always_requests_all_abis_and_omits_split_flag() {
        let props = gradle_properties(
            &AndroidArtifact::Appbundle,
            &HashMap::new(),
            BuildMode::Debug.cargo_features(),
        );
        assert_eq!(
            props,
            vec![
                "-Pfrust.targetPlatforms=arm64-v8a,armeabi-v7a,x86_64".to_string(),
                "-Pfrust.cargoFeatures=ZnJ1c3QvcGVyZi10cmFjZSxmcnVzdC9kZXZ0b29scw==".to_string(),
            ]
        );
    }

    #[test]
    fn gradle_properties_omits_defines_prop_when_empty() {
        let target = apk(&["arm64-v8a"], false);
        let props = gradle_properties(&target, &HashMap::new(), BuildMode::Debug.cargo_features());
        assert!(props.iter().all(|p| !p.starts_with("-Pfrust.defines=")));
    }

    #[test]
    fn gradle_properties_appends_base64_defines_when_present() {
        let target = apk(&["arm64-v8a"], false);
        let mut defines = HashMap::new();
        defines.insert("A".to_string(), "1".to_string());
        defines.insert("B".to_string(), "2".to_string());
        let props = gradle_properties(&target, &defines, BuildMode::Debug.cargo_features());
        let defines_prop = props
            .iter()
            .find(|p| p.starts_with("-Pfrust.defines="))
            .expect("defines prop present");
        // "A=1;B=2" (sorted by key) base64-encoded — verified by hand
        // against the standard RFC 4648 alphabet.
        assert_eq!(defines_prop, "-Pfrust.defines=QT0xO0I9Mg==");
    }

    #[test]
    fn gradle_properties_debug_and_profile_carry_perf_trace_and_devtools_features() {
        let target = apk(&["arm64-v8a"], false);
        for mode in [BuildMode::Debug, BuildMode::Profile] {
            let props = gradle_properties(&target, &HashMap::new(), mode.cargo_features());
            // base64("frust/perf-trace,frust/devtools") — the whole CSV rides
            // one prop, so the encoding covers both features at once.
            assert!(
                props.contains(
                    &"-Pfrust.cargoFeatures=ZnJ1c3QvcGVyZi10cmFjZSxmcnVzdC9kZXZ0b29scw=="
                        .to_string()
                ),
                "mode {mode:?}: {props:?}"
            );
        }
    }

    /// A key-mapped list rides the CSV exactly as given: decoded, the prop is
    /// `frust-ui/perf-trace,frust-ui/devtools`.
    #[test]
    fn gradle_properties_encode_a_key_mapped_feature_list_verbatim() {
        let target = apk(&["arm64-v8a"], false);
        let mapped =
            BuildMode::Debug.cargo_features_for(&crate::facade_key::FacadeKey::new("frust-ui"));
        let refs: Vec<&str> = mapped.iter().map(String::as_str).collect();
        let props = gradle_properties(&target, &HashMap::new(), &refs);
        assert!(
            props.contains(
                &"-Pfrust.cargoFeatures=ZnJ1c3QtdWkvcGVyZi10cmFjZSxmcnVzdC11aS9kZXZ0b29scw=="
                    .to_string()
            ),
            "{props:?}"
        );
        assert_eq!(
            encode_features(&refs).as_deref(),
            Some(base64_encode(b"frust-ui/perf-trace,frust-ui/devtools").as_str())
        );
    }

    #[test]
    fn gradle_properties_release_carries_lean_not_perf_trace_or_devtools() {
        let target = apk(&["arm64-v8a"], false);
        let props = gradle_properties(
            &target,
            &HashMap::new(),
            BuildMode::Release.cargo_features(),
        );
        let feature_prop = props
            .iter()
            .find(|p| p.starts_with("-Pfrust.cargoFeatures="))
            .expect("cargoFeatures prop present");
        // base64("lean") — the log ceiling, NOT perf-trace and NOT devtools.
        assert_eq!(feature_prop, "-Pfrust.cargoFeatures=bGVhbg==");
        assert_ne!(
            feature_prop,
            "-Pfrust.cargoFeatures=ZnJ1c3QvcGVyZi10cmFjZSxmcnVzdC9kZXZ0b29scw=="
        );
    }

    #[test]
    fn gradle_properties_omits_cargo_features_prop_for_dropped_lean_legacy_app() {
        // The release-lean preflight resolves a legacy app (no declared
        // `lean`) to an empty feature list; `gradle_properties` must then
        // emit no `-Pfrust.cargoFeatures` prop at all rather than an empty
        // one — no undeclared feature ever reaches `cargo ndk`.
        let target = apk(&["arm64-v8a"], false);
        let props = gradle_properties(&target, &HashMap::new(), &[]);
        assert!(
            props
                .iter()
                .all(|p| !p.starts_with("-Pfrust.cargoFeatures=")),
            "{props:?}"
        );
    }

    #[test]
    fn base64_encode_known_value() {
        // "A=B" -> 0x41 0x3D 0x42 -> 6-bit groups 010000 010011 110101
        // 000010 -> Q T 1 C
        assert_eq!(base64_encode(b"A=B"), "QT1C");
    }

    #[test]
    fn base64_encode_round_trips_via_reference_decoder() {
        let cases = ["", "A", "AB", "ABC", "API_URL=https://example.com;DEBUG=1"];
        for case in cases {
            let encoded = base64_encode(case.as_bytes());
            let decoded = base64_decode(&encoded);
            assert_eq!(decoded, case.as_bytes(), "case: {case:?}");
        }
    }

    /// Minimal standard-alphabet base64 decoder, used only by
    /// [`base64_encode_round_trips_via_reference_decoder`] to independently
    /// verify [`base64_encode`] — not used by production code (Gradle/Kotlin
    /// owns decoding on the consuming side).
    fn base64_decode(input: &str) -> Vec<u8> {
        fn value(c: u8) -> Option<u8> {
            BASE64_ALPHABET
                .iter()
                .position(|&b| b == c)
                .map(|p| p as u8)
        }
        let trimmed = input.trim_end_matches('=');
        let mut out = Vec::new();
        let mut buf = 0u32;
        let mut bits = 0u32;
        for c in trimmed.bytes() {
            let v = value(c).expect("valid base64 char");
            buf = (buf << 6) | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buf >> bits) as u8);
            }
        }
        out
    }
}
