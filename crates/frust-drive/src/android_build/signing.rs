//! Release-signing gate: `frust build --release` refuses to produce an
//! artifact that would silently be debug-signed.
//!
//! The Gradle template debug-signs with a warning when it finds no usable
//! signing material, so an Android-Studio-driven build stays usable; the CLI
//! makes the same situation a hard error with guided keytool instructions.
//!
//! **The gate verifies material, not a filename.** It resolves the four
//! values a signing config needs — `storeFile`, `storePassword`, `keyAlias`,
//! `keyPassword` — from the project's Java properties file (optionally
//! key-prefixed) with per-value environment-variable fallbacks, then checks
//! that the keystore path actually resolves to a file on disk. A stub
//! `key.properties` written just to appease the gate therefore fails here
//! instead of producing a green, debug-signed release: with the shipped
//! template, Gradle wires a release signing config only when
//! `key.properties` is non-empty, so "all four values present" is the same
//! condition Gradle tests, and "the keystore exists" is the condition Gradle
//! would blow up on next.
//!
//! **Where the material lives is configuration, not a constant.**
//! `frust.toml`'s `[signing]` section (see [`crate::manifest::SigningSection`])
//! moves the properties path, adds a key prefix, or names environment
//! variables, so a project whose Gradle reads a relocated keystore rig passes
//! the gate without a legacy `android/key.properties`.
//!
//! **Honest limits.** Nothing here parses `build.gradle.kts` — `[signing]`
//! describes what the project's own Gradle does, and the gate believes that
//! description. It also cannot tell a real JKS from an empty file of the same
//! name; verifying keystore contents would mean shelling out to `keytool`
//! with the store password on the command line. Both cases degrade into a
//! Gradle-side failure, never into a green build with a debug-signed
//! artifact. `[signing] external = true` waives the gate outright for a
//! project that signs through machinery Frust cannot see, and says so on
//! every release build.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::build_info::BuildMode;
use crate::doctor::{EnvLookup, RealEnv};
use crate::manifest::{SigningEnv, SigningSection};

/// Properties file the generated `build.gradle.kts` reads, relative to
/// `android/`. `[signing] key-properties` overrides it.
const DEFAULT_KEY_PROPERTIES: &str = "key.properties";

/// One value a release signing config needs. The property name is the
/// Gradle-template key; the manifest field is the `[signing.env]` key that
/// names its environment-variable fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Field {
    StoreFile,
    StorePassword,
    KeyAlias,
    KeyPassword,
}

impl Field {
    const ALL: [Field; 4] = [
        Field::StoreFile,
        Field::StorePassword,
        Field::KeyAlias,
        Field::KeyPassword,
    ];

    /// The key read from the properties file (after any `[signing] prefix`).
    fn property(self) -> &'static str {
        match self {
            Field::StoreFile => "storeFile",
            Field::StorePassword => "storePassword",
            Field::KeyAlias => "keyAlias",
            Field::KeyPassword => "keyPassword",
        }
    }

    /// The `[signing.env]` field naming this value's env-var fallback.
    fn manifest_field(self) -> &'static str {
        match self {
            Field::StoreFile => "store-file",
            Field::StorePassword => "store-password",
            Field::KeyAlias => "key-alias",
            Field::KeyPassword => "key-password",
        }
    }

    fn env_var(self, env: &SigningEnv) -> Option<&str> {
        match self {
            Field::StoreFile => env.store_file.as_deref(),
            Field::StorePassword => env.store_password.as_deref(),
            Field::KeyAlias => env.key_alias.as_deref(),
            Field::KeyPassword => env.key_password.as_deref(),
        }
    }
}

/// Gates [`BuildMode::Release`] on release signing material that actually
/// resolves; profile/debug builds are never gated. Reads `[signing]` from
/// `<project_root>/frust.toml` (absent manifest or section == the scaffolded
/// default, `android/key.properties`).
///
/// `on_line` carries the one advisory this gate can emit (the
/// `external = true` waiver) — this is a print-free core like the pipelines
/// that call it.
pub fn check_release_signing(
    project_root: &Path,
    mode: BuildMode,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    check_with_env(project_root, mode, &RealEnv, on_line)
}

/// The testable core of [`check_release_signing`], taking an injected
/// [`EnvLookup`] so `[signing.env]` fallbacks can be exercised with a
/// `crate::doctor::FakeEnv` instead of the real process environment.
fn check_with_env(
    project_root: &Path,
    mode: BuildMode,
    env: &dyn EnvLookup,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    if mode != BuildMode::Release {
        return Ok(());
    }
    let signing = crate::manifest::load_optional(project_root)?
        .and_then(|manifest| manifest.signing)
        .unwrap_or_default();
    verify(project_root, &signing, env, on_line)
}

fn verify(
    project_root: &Path,
    signing: &SigningSection,
    env: &dyn EnvLookup,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    if signing.external {
        on_line(
            "warning: `[signing] external = true` in frust.toml — the release-signing gate is \
             waived, so Frust cannot promise this artifact is release-signed.",
        );
        return Ok(());
    }

    let android_dir = project_root.join("android");
    let configured = signing.key_properties.as_deref();
    let key_properties = resolve_path(&android_dir, configured.unwrap_or(DEFAULT_KEY_PROPERTIES));
    let properties = read_properties(&key_properties)?;

    let mut resolved: HashMap<Field, String> = HashMap::new();
    let mut missing: Vec<Field> = Vec::new();
    for field in Field::ALL {
        match resolve_field(field, signing, properties.as_ref(), env) {
            Some(value) => {
                resolved.insert(field, value);
            }
            None => missing.push(field),
        }
    }

    if !missing.is_empty() {
        bail!(unresolved_message(&key_properties, signing, &missing));
    }

    let store_file = &resolved[&Field::StoreFile];
    let candidates = store_file_candidates(project_root, &android_dir, &key_properties, store_file);
    if !candidates.iter().any(|c| c.is_file()) {
        bail!(missing_keystore_message(store_file, &candidates));
    }

    Ok(())
}

/// Resolves one value: the properties file first (the same precedence a
/// Gradle rig uses), then the `[signing.env]`-named environment variable.
/// Blank values count as absent — an empty `storePassword=` line is not
/// signing material.
fn resolve_field(
    field: Field,
    signing: &SigningSection,
    properties: Option<&HashMap<String, String>>,
    env: &dyn EnvLookup,
) -> Option<String> {
    let key = match signing.prefix.as_deref() {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}.{}", field.property()),
        _ => field.property().to_string(),
    };
    if let Some(value) = properties
        .and_then(|p| p.get(&key))
        .map(String::as_str)
        .and_then(non_empty)
    {
        return Some(value);
    }
    let name = signing.env.as_ref().and_then(|e| field.env_var(e))?;
    env.get(name).as_deref().and_then(non_empty)
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Joins `relative` under `base`, leaving an absolute path untouched — the
/// same shape Gradle's `rootProject.file(…)` has, with `android/` as the
/// root project.
fn resolve_path(base: &Path, relative: &str) -> PathBuf {
    let path = Path::new(relative);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// Every base a relative `storeFile` plausibly resolves against: the
/// properties file's own directory (the relocated-rig convention), the
/// Gradle root project (`android/`, what the shipped template uses), and the
/// Frust project root. The gate accepts the value if any of them lands on a
/// real file rather than guessing which base the project's Gradle picked.
fn store_file_candidates(
    project_root: &Path,
    android_dir: &Path,
    key_properties: &Path,
    store_file: &str,
) -> Vec<PathBuf> {
    let path = Path::new(store_file);
    if path.is_absolute() {
        return vec![path.to_path_buf()];
    }
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Some(parent) = key_properties.parent() {
        bases.push(parent.to_path_buf());
    }
    bases.push(android_dir.to_path_buf());
    bases.push(project_root.to_path_buf());

    let mut candidates: Vec<PathBuf> = Vec::new();
    for base in bases {
        let candidate = base.join(path);
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

/// Reads a Java properties file into a map: `key=value` or `key:value`, `#`
/// and `!` comments and blank lines skipped, keys and values trimmed. No
/// line continuations or `\uXXXX` escapes — a signing properties file is
/// four flat lines in practice, and a value this parser mangles fails the
/// gate rather than passing it. A missing file is `Ok(None)`; the values may
/// still come from `[signing.env]`.
fn read_properties(path: &Path) -> Result<Option<HashMap<String, String>>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|err| anyhow::anyhow!("reading `{}`: {err}", path.display()))?;
    let mut map = HashMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        let Some(split) = line.find(['=', ':']) else {
            continue;
        };
        let key = line[..split].trim();
        if key.is_empty() {
            continue;
        }
        map.insert(key.to_string(), line[split + 1..].trim().to_string());
    }
    Ok(Some(map))
}

/// The unresolved-material error: names every missing value, where it was
/// looked for, and — for a default-configured project — the exact keytool
/// one-liner and `key.properties` shape that fixes it.
fn unresolved_message(
    key_properties: &Path,
    signing: &SigningSection,
    missing: &[Field],
) -> String {
    let prefix = signing.prefix.as_deref().unwrap_or("");
    let dotted = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}.")
    };
    let listed = missing
        .iter()
        .map(|f| {
            let env = match signing.env.as_ref().and_then(|e| f.env_var(e)) {
                Some(name) => format!(" (or ${name})"),
                None => format!(
                    " (or name an env var with `[signing.env] {}`)",
                    f.manifest_field()
                ),
            };
            format!("\t{dotted}{}{env}\n", f.property())
        })
        .collect::<String>();

    let mut message = format!(
        "release build has no usable signing material — Frust will not ship a debug-signed \
         release.\n\nUnresolved:\n\n{listed}\nLooked in `{}`",
        key_properties.display()
    );
    if signing.env.is_some() {
        message.push_str(" and the environment variables named by `[signing.env]`");
    }
    message.push_str(".\n\n");

    if signing.key_properties.is_none() && signing.prefix.is_none() && signing.env.is_none() {
        message.push_str(
            "Generate an upload keystore:\n\n\
             \tkeytool -genkey -v -keystore ~/upload-keystore.jks -keyalg RSA -storetype JKS \
-keysize 2048 -validity 10000 -alias upload\n\n\
             Then create the file above with:\n\n\
             \tstorePassword=<keystore password>\n\
             \tkeyPassword=<key password>\n\
             \tkeyAlias=upload\n\
             \tstoreFile=<path to the .jks file>\n\n\
             `key.properties` must not be committed.\n\n\
             If your Gradle build reads signing from somewhere else, point Frust at it with \
             `[signing]` in frust.toml (`key-properties`, `prefix`, `[signing.env]`).",
        );
    } else {
        message.push_str(
            "`[signing]` in frust.toml points Frust at this rig — check `key-properties`, \
             `prefix`, and the `[signing.env]` variable names against what your Gradle build \
             actually reads.",
        );
    }
    message
}

/// The keystore-not-found error: the declared value plus every base tried,
/// so a wrong-base path is obvious rather than a bare "not found".
fn missing_keystore_message(store_file: &str, candidates: &[PathBuf]) -> String {
    let tried = candidates
        .iter()
        .map(|c| format!("\t{}\n", c.display()))
        .collect::<String>();
    let mut message = format!(
        "release build declares keystore `{store_file}`, but no file exists there — a release \
         build would be debug-signed or fail in Gradle.\n\nTried:\n\n{tried}"
    );
    if store_file.starts_with('~') {
        message.push_str(
            "\nNote: `~` is not expanded when a properties file is read — write the absolute \
             path instead.\n",
        );
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::FakeEnv;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-signing-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("android")).unwrap();
        dir
    }

    /// Writes a placeholder keystore file at `<project>/<relative>` — the
    /// gate checks that a file exists there, not that it is a valid JKS.
    fn touch_keystore(project: &Path, relative: &str) {
        let path = project.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "not-a-real-jks").unwrap();
    }

    fn write(path: PathBuf, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn check(project: &Path, mode: BuildMode) -> Result<Vec<String>> {
        check_env(project, mode, FakeEnv::new())
    }

    fn check_env(project: &Path, mode: BuildMode, env: FakeEnv) -> Result<Vec<String>> {
        let mut lines = Vec::new();
        check_with_env(project, mode, &env, &mut |line| {
            lines.push(line.to_string())
        })?;
        Ok(lines)
    }

    #[test]
    fn debug_and_profile_never_gated() {
        let dir = unique_temp_dir("debug-profile-skip");
        check(&dir, BuildMode::Debug).unwrap();
        check(&dir, BuildMode::Profile).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_without_key_properties_errs_with_keytool_command() {
        let dir = unique_temp_dir("release-missing");
        let err = check(&dir, BuildMode::Release).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains(
                "keytool -genkey -v -keystore ~/upload-keystore.jks -keyalg RSA -storetype JKS \
-keysize 2048 -validity 10000 -alias upload"
            ),
            "{message}"
        );
        assert!(message.contains("storePassword"), "{message}");
        assert!(message.contains("keyPassword"), "{message}");
        assert!(message.contains("keyAlias"), "{message}");
        assert!(message.contains("storeFile"), "{message}");
        assert!(message.contains("must not be committed"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_with_complete_key_properties_and_a_real_keystore_proceeds() {
        let dir = unique_temp_dir("release-present");
        touch_keystore(&dir, "android/upload.jks");
        write(
            dir.join("android/key.properties"),
            "storePassword=pw\nkeyPassword=pw\nkeyAlias=upload\nstoreFile=upload.jks\n",
        );
        assert!(check(&dir, BuildMode::Release).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stub_key_properties_written_to_appease_the_gate_still_fails() {
        // The false positive this gate exists to close: with the shipped
        // Gradle template a one-key `key.properties` yields a *debug-signed*
        // release, so "the file exists" must not be a pass.
        let dir = unique_temp_dir("stub");
        write(dir.join("android/key.properties"), "keyAlias=upload\n");
        let err = check(&dir, BuildMode::Release).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("no usable signing material"), "{message}");
        let listed = message
            .split_once("Unresolved:")
            .and_then(|(_, rest)| rest.split_once("Looked in"))
            .map(|(listed, _)| listed.to_string())
            .unwrap_or_else(|| panic!("{message}"));
        assert!(listed.contains("storeFile"), "{listed}");
        assert!(listed.contains("storePassword"), "{listed}");
        assert!(listed.contains("keyPassword"), "{listed}");
        // The one key the stub *did* supply is not reported as missing.
        assert!(!listed.contains("keyAlias"), "{listed}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_blank_value_counts_as_missing() {
        let dir = unique_temp_dir("blank");
        touch_keystore(&dir, "android/upload.jks");
        write(
            dir.join("android/key.properties"),
            "storePassword=\nkeyPassword=pw\nkeyAlias=upload\nstoreFile=upload.jks\n",
        );
        let err = check(&dir, BuildMode::Release).unwrap_err();
        assert!(err.to_string().contains("storePassword"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_declared_keystore_that_does_not_exist_fails_with_every_base_tried() {
        let dir = unique_temp_dir("no-keystore");
        write(
            dir.join("android/key.properties"),
            "storePassword=pw\nkeyPassword=pw\nkeyAlias=upload\nstoreFile=upload.jks\n",
        );
        let err = check(&dir, BuildMode::Release).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("no file exists there"), "{message}");
        assert!(
            message.contains(&dir.join("android/upload.jks").display().to_string()),
            "{message}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tilde_path_is_flagged_as_unexpanded() {
        let dir = unique_temp_dir("tilde");
        write(
            dir.join("android/key.properties"),
            "storePassword=pw\nkeyPassword=pw\nkeyAlias=upload\nstoreFile=~/upload.jks\n",
        );
        let err = check(&dir, BuildMode::Release).unwrap_err();
        assert!(err.to_string().contains("`~` is not expanded"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_relocated_prefixed_rig_passes_without_a_legacy_key_properties() {
        // The false negative: Gradle reads `android/app/keystores/key.properties`
        // with `prod.`-prefixed keys, and `android/key.properties` does not
        // exist at all.
        let dir = unique_temp_dir("relocated");
        touch_keystore(&dir, "android/app/keystores/prod.jks");
        write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [signing]\nkey-properties = \"app/keystores/key.properties\"\nprefix = \"prod\"\n",
        );
        write(
            dir.join("android/app/keystores/key.properties"),
            "develop.storeFile=develop.jks\n\
             prod.storeFile=prod.jks\nprod.storePassword=pw\nprod.keyAlias=upload\n\
             prod.keyPassword=pw\n",
        );
        assert!(!dir.join("android/key.properties").exists());
        assert!(check(&dir, BuildMode::Release).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_named_by_the_manifest_fills_values_the_properties_file_omits() {
        let dir = unique_temp_dir("env-fallback");
        touch_keystore(&dir, "android/app/keystores/prod.jks");
        write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [signing]\nkey-properties = \"app/keystores/key.properties\"\nprefix = \"prod\"\n\n\
             [signing.env]\nstore-password = \"PROD_STORE_PASSWORD\"\n\
             key-password = \"PROD_KEY_PASSWORD\"\n",
        );
        write(
            dir.join("android/app/keystores/key.properties"),
            "prod.storeFile=prod.jks\nprod.keyAlias=upload\n",
        );
        let env = FakeEnv::new()
            .set("PROD_STORE_PASSWORD", "pw")
            .set("PROD_KEY_PASSWORD", "pw");
        assert!(check_env(&dir, BuildMode::Release, env).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unset_env_fallback_is_reported_with_its_variable_name() {
        let dir = unique_temp_dir("env-unset");
        touch_keystore(&dir, "android/app/keystores/prod.jks");
        write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [signing]\nkey-properties = \"app/keystores/key.properties\"\nprefix = \"prod\"\n\n\
             [signing.env]\nstore-password = \"PROD_STORE_PASSWORD\"\n\
             key-password = \"PROD_KEY_PASSWORD\"\n",
        );
        write(
            dir.join("android/app/keystores/key.properties"),
            "prod.storeFile=prod.jks\nprod.keyAlias=upload\n",
        );
        let err = check_env(&dir, BuildMode::Release, FakeEnv::new()).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("$PROD_STORE_PASSWORD"), "{message}");
        assert!(message.contains("prod.storePassword"), "{message}");
        assert!(message.contains("[signing.env]"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_env_only_rig_passes_with_no_properties_file_at_all() {
        let dir = unique_temp_dir("env-only");
        touch_keystore(&dir, "android/ci.jks");
        write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [signing.env]\nstore-file = \"CI_STORE_FILE\"\n\
             store-password = \"CI_STORE_PASSWORD\"\nkey-alias = \"CI_KEY_ALIAS\"\n\
             key-password = \"CI_KEY_PASSWORD\"\n",
        );
        let env = FakeEnv::new()
            .set("CI_STORE_FILE", "ci.jks")
            .set("CI_STORE_PASSWORD", "pw")
            .set("CI_KEY_ALIAS", "upload")
            .set("CI_KEY_PASSWORD", "pw");
        assert!(check_env(&dir, BuildMode::Release, env).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn external_waives_the_gate_and_says_so() {
        let dir = unique_temp_dir("external");
        write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[signing]\nexternal = true\n",
        );
        let lines = check(&dir, BuildMode::Release).unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("external = true"), "{lines:?}");
        assert!(lines[0].contains("cannot promise"), "{lines:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_signing_section_fails_the_release_build() {
        let dir = unique_temp_dir("malformed");
        write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[signing]\nkeyProperties = \"x\"\n",
        );
        let err = check(&dir, BuildMode::Release).unwrap_err();
        assert!(err.to_string().contains("parsing"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn absolute_store_file_is_used_verbatim() {
        let dir = unique_temp_dir("absolute");
        let keystore = dir.join("elsewhere/upload.jks");
        write(keystore.clone(), "not-a-real-jks");
        write(
            dir.join("android/key.properties"),
            &format!(
                "storePassword=pw\nkeyPassword=pw\nkeyAlias=upload\nstoreFile={}\n",
                keystore.display()
            ),
        );
        assert!(check(&dir, BuildMode::Release).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn properties_parsing_skips_comments_and_accepts_colon_separators() {
        let dir = unique_temp_dir("props-parse");
        touch_keystore(&dir, "android/upload.jks");
        write(
            dir.join("android/key.properties"),
            "# generated by CI\n! legacy comment\n\n\
             storePassword : pw\nkeyPassword=pw\nkeyAlias=upload\nstoreFile=upload.jks\n",
        );
        assert!(check(&dir, BuildMode::Release).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
