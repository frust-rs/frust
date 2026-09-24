//! Release signing: one source of truth for `frust build --release` and the
//! Gradle build it invokes.
//!
//! **The problem this module solves.** `frust.toml`'s `[signing]` section has
//! three independent axes — the properties file **path**, a key **prefix**,
//! and environment-variable **names** — while the generated
//! `build.gradle.kts` honours one hardcoded path, no prefix concept, and four
//! fixed `ANDROID_*` names. Every axis where the CLI's resolution and Gradle's
//! disagree is a way for `frust build --release` to pass its gate while Gradle
//! silently *debug-signs* the artifact. Two rounds of teaching the gate about
//! Gradle's limitations (an advisory here, an env-name parity fix there) each
//! closed one axis and left another.
//!
//! **The shape that fixes it.** Resolution happens exactly once, here.
//! [`check_release_signing`] resolves the four values a signing config needs —
//! `storeFile`, `storePassword`, `keyAlias`, `keyPassword` — from the
//! project's Java properties file (optionally key-prefixed) with
//! per-value environment-variable fallbacks, checks the keystore really exists
//! on disk, and **returns the resolved material**. The pipeline then writes
//! that material to `android/.frust-signing.properties` via [`write_resolved`]
//! — unprefixed keys, absolute `storeFile` — and the generated Gradle reads
//! that file *first*. Gradle therefore sees precisely what the gate verified,
//! by construction: all three axes are resolved away before Gradle starts, and
//! there is no predicate left to drift.
//!
//! **Lifecycle of the generated file.** It exists only for the duration of one
//! Gradle invocation: created owner-only (0600 on Unix), gitignored by the
//! scaffold, and removed by [`GeneratedProperties`]'s `Drop` — which runs on
//! the error path too, not just a successful build. It materialises passwords
//! that an env-only CI rig would otherwise keep in memory alone; that tradeoff
//! buys the guarantee above, and the short, owner-only, always-deleted window
//! is what keeps it acceptable.
//!
//! **"Always deleted" includes an interrupted build.** `Drop` alone covers a
//! normal return or unwind, and the file lives across the whole multi-minute
//! Gradle invocation: Ctrl-C used to take the default disposition and leave the
//! passwords in the project tree, and so did a panic (a release `frust` is
//! built `panic = "abort"`, which runs no destructors). [`write_resolved`]
//! therefore arms [`crate::interrupt`] with the path **before** creating the
//! file and holds that registration inside the guard, so SIGINT, SIGTERM,
//! SIGHUP, and an abort all delete it too. Only SIGKILL or a machine crash can
//! still skip cleanup — the file's own header says so, and the next release
//! build warns about the leftover before replacing it.
//!
//! **Why a file at all**, given it is the passwords that make this delicate:
//! Gradle reads it at *configuration* time, and the read is written into the
//! project's own `build.gradle.kts` (`frustSigning(...)`), which Frust cannot
//! re-render in the installed base — there is no `frust upgrade`. The two
//! file-less transports are both worse. `-P` project properties land on the
//! `./gradlew` command line, where every user on the machine can read them out
//! of `ps`/`/proc` — strictly weaker than a 0600 file. Environment variables
//! are read by the build script through `System.getenv`, which inside a reused
//! Gradle **daemon** reports the environment that daemon *started* with, not
//! this invocation's — so what Gradle signed with would once again not be what
//! the gate resolved, which is the entire defect class this module exists to
//! close (and the template's `ANDROID_*` env lookups are the documented
//! no-promises fallback for exactly that reason).
//!
//! **The backstop.** That guarantee is by *construction*, and the construction
//! has a precondition nothing can check up front: that this project's
//! `build.gradle.kts` actually contains the `frustSigning(...)` read. A project
//! generated before that read existed (there is no `frust upgrade`), an edited
//! signing block, Gradle's configuration cache, or a concurrent build deleting
//! the fixed-name file all break it — and the template's fallthrough *succeeds*,
//! debug-signing with a warning, so `out.success` alone would report a green
//! release build over a debug-signed APK. So the pipelines also key off what
//! Gradle **did**: [`reported_debug_signing`] greps the captured output of a
//! successful release build for the template's fallback marker and
//! [`debug_signed_error`] turns it into a hard failure. That needs no model of
//! Gradle at all — every residual divergence route collapses into one refusal,
//! and no `build.gradle.kts` gets parsed.
//!
//! **Honest limits.** Nothing here parses `build.gradle.kts`. A project that
//! edits the generated template's *fallback* lookups is free to; the
//! `.frust-signing.properties` read sits ahead of them and is not something a
//! signing-rig edit touches. A hand-run `./gradlew assembleRelease` (Android
//! Studio) never invokes Frust, so the file does not exist and Gradle falls
//! back to `key.properties` + the four `ANDROID_*` variables — that path
//! carries **no Frust promise**, and the template says so. This module also
//! cannot tell a real JKS from an empty file of the same name; verifying
//! keystore contents would mean shelling out to `keytool` with the store
//! password on the command line. `[signing] external = true` waives the gate
//! outright for a project that signs through machinery Frust cannot see, writes
//! no generated file, and says so on every release build.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::build_info::BuildMode;
use crate::doctor::{EnvLookup, RealEnv};
use crate::manifest::{SigningEnv, SigningSection};

/// Properties file the project's Gradle reads its *fallback* signing material
/// from, relative to `android/`. `[signing] key-properties` overrides it.
const DEFAULT_KEY_PROPERTIES: &str = "key.properties";

/// The generated, single-source-of-truth properties file, relative to
/// `android/` (the Gradle root project). Written before a release Gradle
/// invocation and deleted the moment it returns — see [`GeneratedProperties`].
pub(crate) const GENERATED_PROPERTIES: &str = ".frust-signing.properties";

/// One value a release signing config needs. The property name is the
/// Gradle-template key; each field also has a default environment-variable
/// name, which `[signing.env]` may override.
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

    /// The key read from the properties file (after any `[signing] prefix`),
    /// and the key written *unprefixed* into [`GENERATED_PROPERTIES`].
    fn property(self) -> &'static str {
        match self {
            Field::StoreFile => "storeFile",
            Field::StorePassword => "storePassword",
            Field::KeyAlias => "keyAlias",
            Field::KeyPassword => "keyPassword",
        }
    }

    /// The environment variable this value falls back to when `[signing.env]`
    /// names none. These are the same four names the generated
    /// `build.gradle.kts` reads in its fallback path, so a CI rig that exports
    /// them needs no `frust.toml` edit at all.
    fn default_env_var(self) -> &'static str {
        match self {
            Field::StoreFile => "ANDROID_STORE_FILE",
            Field::StorePassword => "ANDROID_STORE_PASSWORD",
            Field::KeyAlias => "ANDROID_KEY_ALIAS",
            Field::KeyPassword => "ANDROID_KEY_PASSWORD",
        }
    }

    fn configured_env_var(self, env: &SigningEnv) -> Option<&str> {
        match self {
            Field::StoreFile => env.store_file.as_deref(),
            Field::StorePassword => env.store_password.as_deref(),
            Field::KeyAlias => env.key_alias.as_deref(),
            Field::KeyPassword => env.key_password.as_deref(),
        }
    }

    /// The environment variable actually consulted: `[signing.env]`'s name if
    /// the manifest gives one, else [`Field::default_env_var`].
    fn env_var(self, env: Option<&SigningEnv>) -> &str {
        env.and_then(|env| self.configured_env_var(env))
            .unwrap_or_else(|| self.default_env_var())
    }
}

/// The four release-signing values, fully resolved: prefix applied and
/// stripped, environment fallbacks folded in, `store_file` pinned to the
/// absolute path of a keystore that exists. This is what the gate verified and
/// — byte for byte — what [`write_resolved`] hands Gradle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedSigning {
    store_file: PathBuf,
    store_password: String,
    key_alias: String,
    key_password: String,
}

/// Gates [`BuildMode::Release`] on release signing material that actually
/// resolves, returning that material so the caller can hand it to Gradle
/// through [`write_resolved`]; profile/debug builds are never gated and
/// resolve nothing. Reads `[signing]` from `<project_root>/frust.toml` (absent
/// manifest or section == the scaffolded default, `android/key.properties`).
///
/// `Ok(None)` means "no generated file for this build": a non-release mode, or
/// `[signing] external = true`, whose waiver is announced on `on_line` — this
/// is a print-free core like the pipelines that call it.
pub fn check_release_signing(
    project_root: &Path,
    mode: BuildMode,
    on_line: &mut dyn FnMut(&str),
) -> Result<Option<ResolvedSigning>> {
    check_with_env(project_root, mode, &RealEnv, on_line)
}

/// The testable core of [`check_release_signing`], taking an injected
/// [`EnvLookup`] so environment fallbacks can be exercised with a
/// `crate::doctor::FakeEnv` instead of the real process environment.
fn check_with_env(
    project_root: &Path,
    mode: BuildMode,
    env: &dyn EnvLookup,
    on_line: &mut dyn FnMut(&str),
) -> Result<Option<ResolvedSigning>> {
    if mode != BuildMode::Release {
        return Ok(None);
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
) -> Result<Option<ResolvedSigning>> {
    if signing.external {
        on_line(
            "warning: `[signing] external = true` in frust.toml — the release-signing gate is \
             waived, so Frust cannot promise this artifact is release-signed.",
        );
        return Ok(None);
    }

    let android_dir = project_root.join("android");
    let key_properties = resolve_path(
        &android_dir,
        signing
            .key_properties
            .as_deref()
            .unwrap_or(DEFAULT_KEY_PROPERTIES),
    );
    let properties = read_properties(&key_properties)?;

    let mut resolved: HashMap<Field, String> = HashMap::new();
    let mut missing: Vec<Field> = Vec::new();
    for field in Field::ALL {
        match resolve_field(field, signing, properties.as_ref(), env) {
            Some(found) => {
                resolved.insert(field, found);
            }
            None => missing.push(field),
        }
    }

    if !missing.is_empty() {
        bail!(unresolved_message(&key_properties, signing, &missing));
    }

    let store_file = &resolved[&Field::StoreFile];
    let candidates = store_file_candidates(project_root, &android_dir, &key_properties, store_file);
    let Some(found) = candidates.iter().find(|candidate| candidate.is_file()) else {
        bail!(missing_keystore_message(store_file, &candidates));
    };

    Ok(Some(ResolvedSigning {
        // Absolute, so whichever base the project's Gradle resolves against
        // (`rootProject.file(...)` in the shipped template, a module-relative
        // `file(...)` in a Flutter-style rig) can't disagree with whichever
        // of the four bases the gate searched actually landed.
        store_file: absolute(found),
        store_password: resolved[&Field::StorePassword].clone(),
        key_alias: resolved[&Field::KeyAlias].clone(),
        key_password: resolved[&Field::KeyPassword].clone(),
    }))
}

/// The machine-stable token the generated `build.gradle.kts` prints when its
/// release build falls through to the debug signing config. Prose gets
/// reworded; a token does not — the template carries a comment saying the CLI
/// greps for this exact string, and the two must move together.
const FALLBACK_TOKEN: &str = "FRUST-SIGNING-FALLBACK";

/// The prose fragment that same warning has carried in **every** template Frust
/// has ever shipped, back through the pre-rename ForgeKit one. It is matched
/// alongside [`FALLBACK_TOKEN`] because the installed base is precisely what
/// this check exists for: a project generated before the token existed cannot
/// print it, and there is no `frust upgrade` to re-render its Gradle. An old
/// template keeps the old prose, so the matcher accepts both.
const FALLBACK_PROSE: &str = "release build is debug-signed";

/// Whether Gradle reported that it debug-signed the release build it just
/// finished — i.e. whether the generated `build.gradle.kts` reached its
/// no-promises fallback despite the gate having resolved and written real
/// material.
///
/// Call this **only for a build that succeeded**. On a *failed* build Gradle
/// may echo lines of the build script itself (which quotes both markers in a
/// comment) while diagnosing an error, and a failed build is already reported
/// as a failure anyway.
pub(crate) fn reported_debug_signing(gradle_output: &str) -> bool {
    gradle_output.contains(FALLBACK_TOKEN) || gradle_output.contains(FALLBACK_PROSE)
}

/// The refusal [`reported_debug_signing`] earns: Gradle exited 0, but over a
/// debug-signed artifact, so the build is not the release build it claims to
/// be. Names the two routes that actually reach it and the two ways out.
pub(crate) fn debug_signed_error() -> anyhow::Error {
    anyhow::anyhow!(
        "Gradle debug-signed this release build — refusing to report it as a release build.\n\n\
         The signing gate resolved release material from frust.toml's `[signing]` section and \
         handed it to Gradle in `android/.frust-signing.properties`, but \
         `android/app/build.gradle.kts` did not use it: Gradle logged its debug-signing \
         fallback instead. The artifact carries the Android debug certificate, not your \
         keystore.\n\n\
         Usual causes:\n\n\
         \t- the project was generated before that read existed (there is no `frust upgrade` — \
         the signing block has to be brought forward by hand)\n\
         \t- the release `signingConfigs` block was edited so the `frustSigning(...)` lookups \
         no longer run\n\n\
         Fix `android/app/build.gradle.kts` so the release signing config reads \
         `.frust-signing.properties` first — `frust create` a throwaway project and copy its \
         `signingConfigs` block across. If this project signs through machinery Frust cannot \
         see (CI, a Gradle signing plugin), declare `[signing] external = true` in frust.toml \
         instead: that waives the gate and this check, and warns on every release build."
    )
}

/// A live `android/.frust-signing.properties`. Dropping it deletes the file —
/// which is the point: the drop runs whether the Gradle invocation succeeded,
/// failed, or bailed early, so plaintext keystore passwords never outlive the
/// build that needed them.
///
/// The `Drop` covers a return or unwind; the [`crate::interrupt`] registration
/// it carries covers the rest of the ways this process can die — SIGINT,
/// SIGTERM, SIGHUP, and an abort all delete the same path. Only SIGKILL or a
/// machine crash can skip it; the file's own header says so and the scaffold
/// gitignores it.
#[must_use = "dropping the guard immediately deletes the file Gradle is about to read"]
pub(crate) struct GeneratedProperties {
    path: PathBuf,
    /// Armed before the file was created and dropped only after this struct's
    /// `Drop` has removed it (fields drop after the enclosing `Drop::drop`
    /// body), so the signal-time cleanup is never disarmed while the file could
    /// still exist.
    _scrub: crate::interrupt::SecretScrub,
}

impl GeneratedProperties {
    /// Test-only: production code never needs the path back, it only needs the
    /// guard to stay alive across the Gradle invocation.
    #[cfg(test)]
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for GeneratedProperties {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// The header the generated file carries, so anyone who finds one left behind
/// by a hard kill knows what it is, that it holds secrets, and what to do about
/// it. This is the only thing that reaches a user whose project predates the
/// scaffold's `.gitignore` entry for it (there is no `frust upgrade`), so it
/// spells the ignore line out rather than assuming one exists.
const GENERATED_HEADER: &str = "\
# Generated by `frust build`/`frust run`. DO NOT COMMIT. DO NOT EDIT. DELETE IT.
#
# These four values are exactly what the Frust release-signing gate resolved
# from frust.toml's [signing] section: the key prefix has been applied and
# stripped, environment-variable fallbacks folded in, and storeFile pinned to
# an absolute path that was verified to exist. The generated build.gradle.kts
# reads this file before anything else, so the gate's verdict and the signature
# on the artifact are the same fact rather than two agreeing guesses.
#
# IT CONTAINS THE KEYSTORE PASSWORDS IN PLAINTEXT. It is created owner-only
# (0600 on Unix) and deleted as soon as the Gradle invocation returns --
# success, failure, Ctrl-C, SIGTERM/SIGHUP, or a crash of the CLI itself.
#
# So if you are reading this in an editor, either a build is running right now,
# or one was killed with SIGKILL / lost its machine. In the second case: delete
# this file and treat the keystore password as having been on disk unprotected.
# A project generated before Frust wrote this file has no .gitignore line for
# it -- add one so it can never be committed:
#
#     android/.frust-signing.properties
";

/// Writes `resolved` to `<android_dir>/.frust-signing.properties` with
/// unprefixed keys and an absolute `storeFile`, owner-only, returning the
/// guard that deletes it again. Call this immediately before invoking Gradle
/// and let the guard drop the moment Gradle returns.
///
/// **Ordering matters and is load-bearing:** the [`crate::interrupt`] scrub is
/// armed before this function makes any filesystem call at all, and the
/// returned guard owns that registration — so from the instant the
/// file can exist to the instant it is gone, an interrupted process still
/// deletes it. Every early return here disarms by dropping the local
/// registration, and by then no file exists to disarm for.
///
/// A file already sitting at the path is reported through `on_line`: nothing
/// but a SIGKILL/crash (or a concurrent build in the same project) can leave
/// one, and the user it happened to is the one who needs to hear that plaintext
/// passwords were on disk.
pub(crate) fn write_resolved(
    android_dir: &Path,
    resolved: &ResolvedSigning,
    on_line: &mut dyn FnMut(&str),
) -> Result<GeneratedProperties> {
    let path = android_dir.join(GENERATED_PROPERTIES);
    // Armed before anything can create the file — see this function's doc.
    let scrub = crate::interrupt::scrub_on_signal(&path)?;
    if path.exists() {
        on_line(&leftover_warning(&path));
    }
    let mut body = String::from(GENERATED_HEADER);
    for (key, value) in [
        (
            Field::StoreFile.property(),
            resolved.store_file.to_string_lossy().into_owned(),
        ),
        (
            Field::StorePassword.property(),
            resolved.store_password.clone(),
        ),
        (Field::KeyAlias.property(), resolved.key_alias.clone()),
        (Field::KeyPassword.property(), resolved.key_password.clone()),
    ] {
        body.push_str(&format!("{key}={}\n", escape_property_value(&value)));
    }

    // Remove any stale file first: `OpenOptions::mode` only applies at
    // creation, so reusing an existing (possibly world-readable) file would
    // silently skip the owner-only permission.
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(anyhow::Error::new(err))
                .with_context(|| format!("removing a stale `{}`", path.display()));
        }
    }

    let mut file =
        create_owner_only(&path).with_context(|| format!("creating `{}`", path.display()))?;
    // From here on the file exists, so hand ownership to the guard before the
    // write can fail — an aborted write must still be cleaned up.
    let guard = GeneratedProperties {
        path,
        _scrub: scrub,
    };
    file.write_all(body.as_bytes())
        .with_context(|| format!("writing `{}`", guard.path.display()))?;
    Ok(guard)
}

/// The leftover-file warning. Says what the file is, what it means that it
/// exists, and — for the installed base, whose `.gitignore` predates it — the
/// line to add so it can never be committed.
fn leftover_warning(path: &Path) -> String {
    format!(
        "warning: `{}` already existed and has been replaced. It holds this project's keystore \
         passwords in plaintext and is deleted whenever a build returns, is interrupted, or \
         crashes — so a leftover one means an earlier build was SIGKILLed or lost its machine \
         (or another build is running in this project right now). Treat the password as having \
         been exposed on disk, and make sure your .gitignore carries \
         `android/.frust-signing.properties` — a project generated before Frust wrote this file \
         has no line for it.",
        path.display()
    )
}

#[cfg(unix)]
fn create_owner_only(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_owner_only(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Escapes a value for `java.util.Properties.load`: backslash and the
/// whitespace escapes, plus `\uXXXX` for every non-ASCII character (that
/// loader decodes a byte stream as ISO-8859-1, so a UTF-8 password would
/// otherwise arrive mangled). Separator characters (`=`, `:`) and comment
/// markers (`#`, `!`) need no escaping *inside* a value.
fn escape_property_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut utf16 = [0u16; 2];
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_ascii() => out.push(ch),
            ch => {
                for unit in ch.encode_utf16(&mut utf16) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out
}

/// Resolves one value: the properties file first (the same precedence a
/// Gradle rig uses), then the environment variable — `[signing.env]`'s name if
/// the manifest gives one, else the `ANDROID_*` default. Blank values count as
/// absent at both steps: an empty `storePassword=` line is not signing
/// material, it falls through to the environment.
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
    env.get(field.env_var(signing.env.as_ref()))
        .as_deref()
        .and_then(non_empty)
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

/// Best-effort absolute form of an existing path: `canonicalize` (which also
/// resolves symlinks), falling back to a lexical absolute path, falling back
/// to the input. Only ever called on a path that just tested `is_file`, so the
/// fallbacks are defensive rather than expected.
fn absolute(path: &Path) -> PathBuf {
    fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Every base a relative `storeFile` plausibly resolves against, in the order
/// tried: the properties file's own directory (the relocated-rig convention,
/// `storeFile=prod.jks` beside `key.properties`), the Gradle root project
/// (`android/`, what the shipped template's `rootProject.file(…)` uses), the
/// Gradle app module (`android/app/`, what a bare `file(…)` inside
/// `app/build.gradle.kts` resolves against — the Flutter convention, kept
/// verbatim by a rig ported from a Flutter app), and the Frust project root.
/// The gate accepts the first that lands on a real file rather than guessing
/// which base the project's Gradle picked — and since the resolved path is
/// then handed to Gradle *absolute*, that guess is no longer something the
/// two sides can disagree about.
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
    // The source module `frust create` lays out — not
    // `BuildLayout::android_app()`, which is the build *output* tree.
    bases.push(android_dir.join("app"));
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
/// still come from the environment.
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

/// The unresolved-material error: names every missing value, the environment
/// variable it would also have accepted, where the properties file was looked
/// for, and — for a default-configured project — the exact keytool one-liner
/// and `key.properties` shape that fixes it.
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
            format!(
                "\t{dotted}{} (or ${})\n",
                f.property(),
                f.env_var(signing.env.as_ref())
            )
        })
        .collect::<String>();

    let mut message = format!(
        "release build has no usable signing material — Frust will not ship a debug-signed \
         release.\n\nUnresolved:\n\n{listed}\nLooked in `{}` and the environment.\n\n",
        key_properties.display()
    );

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
             `[signing]` in frust.toml (`key-properties`, `prefix`, `[signing.env]`) — Frust \
             resolves it and hands Gradle the result, so the two cannot disagree.",
        );
    } else {
        message.push_str(
            "`[signing]` in frust.toml points Frust at this rig — check `key-properties`, \
             `prefix`, and the `[signing.env]` variable names against where the material \
             actually lives.",
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

    type Checked = (Option<ResolvedSigning>, Vec<String>);

    fn check(project: &Path, mode: BuildMode) -> Result<Checked> {
        check_env(project, mode, FakeEnv::new())
    }

    fn check_env(project: &Path, mode: BuildMode, env: FakeEnv) -> Result<Checked> {
        let mut lines = Vec::new();
        let resolved = check_with_env(project, mode, &env, &mut |line| {
            lines.push(line.to_string())
        })?;
        Ok((resolved, lines))
    }

    /// Resolves a release build and writes the generated file, returning its
    /// contents plus the still-live guard (so the caller controls deletion).
    /// Asserts nothing was announced on either step — a leftover-file warning
    /// is the one thing `write_resolved` says, and only
    /// [`generate_reporting`]'s callers expect it.
    fn generate(project: &Path, env: FakeEnv) -> (String, GeneratedProperties) {
        let (contents, guard, lines) = generate_reporting(project, env);
        assert!(lines.is_empty(), "unexpected advisory lines: {lines:?}");
        (contents, guard)
    }

    /// [`generate`] plus every line the resolve/write pair emitted.
    fn generate_reporting(
        project: &Path,
        env: FakeEnv,
    ) -> (String, GeneratedProperties, Vec<String>) {
        let (resolved, mut lines) = check_env(project, BuildMode::Release, env).unwrap();
        let resolved = resolved.expect("release material must resolve");
        let guard = write_resolved(&project.join("android"), &resolved, &mut |line| {
            lines.push(line.to_string())
        })
        .unwrap();
        let contents = fs::read_to_string(guard.path()).unwrap();
        (contents, guard, lines)
    }

    #[test]
    fn debug_and_profile_never_gated_and_resolve_nothing() {
        let dir = unique_temp_dir("debug-profile-skip");
        assert!(check(&dir, BuildMode::Debug).unwrap().0.is_none());
        assert!(check(&dir, BuildMode::Profile).unwrap().0.is_none());
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
        // The default environment fallbacks are named, so a CI rig learns the
        // variables it can export without editing frust.toml.
        assert!(message.contains("$ANDROID_STORE_FILE"), "{message}");
        assert!(message.contains("$ANDROID_KEY_PASSWORD"), "{message}");
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
        let (resolved, lines) = check(&dir, BuildMode::Release).unwrap();
        assert!(lines.is_empty(), "{lines:?}");
        let resolved = resolved.expect("material resolves");
        assert!(resolved.store_file.is_absolute(), "{resolved:?}");
        assert!(resolved.store_file.ends_with("upload.jks"), "{resolved:?}");
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
        // Component-wise `Path::join`s, not a single `/`-embedded string:
        // the production candidates are built base-then-filename via two
        // separate `.join()` calls (see `store_file_candidates`), which on
        // Windows renders pure-`\` — `dir.join("android/upload.jks")` would
        // instead carry a literal `/` inside the pushed component, never
        // matching that `Display` output there.
        assert!(
            message.contains(&dir.join("android").join("upload.jks").display().to_string()),
            "{message}"
        );
        assert!(
            message.contains(
                &dir.join("android")
                    .join("app")
                    .join("upload.jks")
                    .display()
                    .to_string()
            ),
            "{message}"
        );
        assert!(
            message.contains(&dir.join("upload.jks").display().to_string()),
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

    /// The regression, in miniature: `prefix` with the **default**
    /// properties path. The gate resolves `prod.`-prefixed keys; the generated
    /// file it hands Gradle must carry them **unprefixed**, since the Gradle
    /// template has no prefix concept at all. Before the generated file
    /// existed this shape passed the gate and debug-signed the artifact.
    #[test]
    fn a_prefix_only_rig_resolves_and_hands_gradle_unprefixed_keys() {
        let dir = unique_temp_dir("prefix-only");
        touch_keystore(&dir, "android/prod.jks");
        write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[signing]\nprefix = \"prod\"\n",
        );
        write(
            dir.join("android/key.properties"),
            "develop.storeFile=develop.jks\n\
             prod.storeFile=prod.jks\nprod.storePassword=pw\nprod.keyAlias=upload\n\
             prod.keyPassword=pw\n",
        );

        let (contents, guard) = generate(&dir, FakeEnv::new());
        assert!(contents.contains("keyAlias=upload"), "{contents}");
        assert!(contents.contains("storePassword=pw"), "{contents}");
        assert!(contents.contains("keyPassword=pw"), "{contents}");
        assert!(
            !contents.lines().any(|line| line.starts_with("prod.")),
            "the prefix must be resolved away, not passed on: {contents}"
        );
        let store_file = store_file_line(&contents);
        assert!(
            Path::new(&store_file).is_absolute() && Path::new(&store_file).is_file(),
            "storeFile must be an absolute path to a real keystore: {store_file}"
        );

        drop(guard);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The other regression case: a CI rig exporting the four `ANDROID_*`
    /// variables against a **stock** frust.toml — no `[signing.env]` block at
    /// all. The gate must resolve them (they are the documented defaults, and
    /// the same names the generated Gradle falls back to) instead of hard
    /// failing a build Gradle would have signed.
    #[test]
    fn an_env_only_rig_resolves_with_no_signing_env_block_at_all() {
        let dir = unique_temp_dir("env-defaults");
        touch_keystore(&dir, "android/ci.jks");
        write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n",
        );
        let env = FakeEnv::new()
            .set("ANDROID_STORE_FILE", "ci.jks")
            .set("ANDROID_STORE_PASSWORD", "pw")
            .set("ANDROID_KEY_ALIAS", "upload")
            .set("ANDROID_KEY_PASSWORD", "pw");

        let (contents, guard) = generate(&dir, env);
        assert!(contents.contains("keyAlias=upload"), "{contents}");
        let store_file = store_file_line(&contents);
        assert!(
            Path::new(&store_file).is_file(),
            "storeFile must resolve to the real keystore: {store_file}"
        );

        drop(guard);
        let _ = fs::remove_dir_all(&dir);
    }

    /// `[signing.env]` still overrides the defaults per value, and the
    /// properties file still wins over both.
    #[test]
    fn named_env_vars_override_the_defaults_and_the_properties_file_wins() {
        let dir = unique_temp_dir("env-named");
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
            "prod.storeFile=prod.jks\nprod.keyAlias=from-properties\n",
        );
        let env = FakeEnv::new()
            .set("PROD_STORE_PASSWORD", "named-pw")
            .set("PROD_KEY_PASSWORD", "named-key-pw")
            // The default name must lose to the properties file entry.
            .set("ANDROID_KEY_ALIAS", "from-environment");

        let (contents, guard) = generate(&dir, env);
        assert!(contents.contains("storePassword=named-pw"), "{contents}");
        assert!(contents.contains("keyPassword=named-key-pw"), "{contents}");
        assert!(contents.contains("keyAlias=from-properties"), "{contents}");

        drop(guard);
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
    fn a_relocated_prefixed_rig_passes_without_a_legacy_key_properties() {
        // The false negative: Gradle reads `android/app/keystores/key.properties`
        // with `prod.`-prefixed keys, and `android/key.properties` does not
        // exist at all. Passing is silent now — the generated file removes the
        // divergence this used to advise about.
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
        let (contents, guard) = generate(&dir, FakeEnv::new());
        assert!(contents.contains("keyAlias=upload"), "{contents}");
        assert!(
            Path::new(&store_file_line(&contents)).is_file(),
            "{contents}"
        );
        drop(guard);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The Flutter convention, verbatim: `key.properties` lives in
    /// `android/app/keystores/`, its `storeFile` is `keystores/production.jks`,
    /// and the app's `build.gradle.kts` resolves it with a bare `file(…)` —
    /// relative to the *module*, `android/app/`. Before the module base
    /// existed the gate refused this rig over a keystore sitting exactly where
    /// Gradle would look, listing a doubled `keystores/keystores/` path among
    /// the bases it had tried.
    #[test]
    fn a_module_relative_store_file_resolves_against_android_app() {
        let dir = unique_temp_dir("module-relative");
        touch_keystore(&dir, "android/app/keystores/production.jks");
        write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [signing]\nkey-properties = \"app/keystores/key.properties\"\nprefix = \"prod\"\n",
        );
        write(
            dir.join("android/app/keystores/key.properties"),
            "prod.storeFile=keystores/production.jks\nprod.storePassword=pw\n\
             prod.keyAlias=upload\nprod.keyPassword=pw\n",
        );
        assert!(!dir.join("android/app/keystores/keystores").exists());
        let (contents, guard) = generate(&dir, FakeEnv::new());
        let generated = PathBuf::from(store_file_line(&contents));
        assert!(generated.is_file(), "{contents}");
        assert_eq!(
            fs::canonicalize(&generated).unwrap(),
            fs::canonicalize(dir.join("android/app/keystores/production.jks")).unwrap(),
            "{contents}"
        );
        drop(guard);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn external_waives_the_gate_says_so_and_generates_nothing() {
        let dir = unique_temp_dir("external");
        write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[signing]\nexternal = true\n",
        );
        let (resolved, lines) = check(&dir, BuildMode::Release).unwrap();
        assert!(resolved.is_none(), "external must generate no file");
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
        let (resolved, lines) = check(&dir, BuildMode::Release).unwrap();
        assert!(lines.is_empty(), "{lines:?}");
        assert!(resolved.unwrap().store_file.is_file());
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
        let (resolved, lines) = check(&dir, BuildMode::Release).unwrap();
        assert!(lines.is_empty(), "{lines:?}");
        assert!(resolved.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The generated file's whole lifecycle: owner-only while it lives,
    /// header warning present, and gone the moment the guard drops.
    #[test]
    fn the_generated_file_is_owner_only_and_deleted_when_the_guard_drops() {
        let dir = unique_temp_dir("generated-lifecycle");
        touch_keystore(&dir, "android/upload.jks");
        write(
            dir.join("android/key.properties"),
            "storePassword=pw\nkeyPassword=pw\nkeyAlias=upload\nstoreFile=upload.jks\n",
        );

        let (contents, guard) = generate(&dir, FakeEnv::new());
        let path = guard.path().to_path_buf();
        assert_eq!(path, dir.join("android").join(GENERATED_PROPERTIES));
        assert!(contents.contains("DO NOT COMMIT"), "{contents}");
        assert!(contents.contains("PLAINTEXT"), "{contents}");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "generated file must be owner-only");
        }

        drop(guard);
        assert!(
            !path.exists(),
            "the guard must delete the generated file on drop"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The generated file is armed with [`crate::interrupt`] for the whole
    /// time it exists — that registration, not `Drop`, is what deletes it when
    /// a Ctrl-C during the Gradle invocation stops `Drop` from ever running —
    /// and is disarmed once the guard has removed the file.
    #[test]
    fn the_generated_file_is_armed_for_signal_cleanup_while_it_lives() {
        let dir = unique_temp_dir("generated-armed");
        touch_keystore(&dir, "android/upload.jks");
        write(
            dir.join("android/key.properties"),
            "storePassword=pw\nkeyPassword=pw\nkeyAlias=upload\nstoreFile=upload.jks\n",
        );
        let path = dir.join("android").join(GENERATED_PROPERTIES);
        assert!(
            !crate::interrupt::armed_paths().contains(&path),
            "nothing is armed before the file is written"
        );

        let (_contents, guard) = generate(&dir, FakeEnv::new());
        assert!(
            crate::interrupt::armed_paths().contains(&path),
            "the live generated file must be armed for signal-time deletion"
        );

        drop(guard);
        assert!(!path.exists());
        assert!(
            !crate::interrupt::armed_paths().contains(&path),
            "the arm is released only once the file is gone"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A `write_resolved` that fails leaves nothing armed: the registration is
    /// a local until the guard takes it, so an early return disarms it — and by
    /// then no file exists to disarm for.
    #[test]
    fn a_failed_write_leaves_nothing_armed() {
        let dir = unique_temp_dir("generated-write-fails");
        touch_keystore(&dir, "android/upload.jks");
        write(
            dir.join("android/key.properties"),
            "storePassword=pw\nkeyPassword=pw\nkeyAlias=upload\nstoreFile=upload.jks\n",
        );
        let (resolved, _) = check(&dir, BuildMode::Release).unwrap();
        let resolved = resolved.unwrap();

        // A directory that does not exist: `create_owner_only` fails.
        let missing = dir.join("android/nowhere");
        let err = match write_resolved(&missing, &resolved, &mut |_| {}) {
            Err(err) => err,
            Ok(_guard) => panic!("writing into a non-existent directory must fail"),
        };
        assert!(err.to_string().contains("creating"), "{err}");
        assert!(
            !crate::interrupt::armed_paths().contains(&missing.join(GENERATED_PROPERTIES)),
            "a failed write must not leave a stale arm behind"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A pre-existing (possibly world-readable) file is replaced, not reused —
    /// `OpenOptions::mode` only applies at creation.
    #[test]
    fn a_stale_generated_file_is_replaced_rather_than_reused() {
        let dir = unique_temp_dir("generated-stale");
        touch_keystore(&dir, "android/upload.jks");
        write(
            dir.join("android/key.properties"),
            "storePassword=pw\nkeyPassword=pw\nkeyAlias=upload\nstoreFile=upload.jks\n",
        );
        let stale = dir.join("android").join(GENERATED_PROPERTIES);
        fs::write(&stale, "storePassword=leftover\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&stale, fs::Permissions::from_mode(0o644)).unwrap();
        }

        let (contents, guard, lines) = generate_reporting(&dir, FakeEnv::new());
        assert!(!contents.contains("leftover"), "{contents}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(guard.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // Only a SIGKILL/crash (or a concurrent build) can leave one behind now,
        // and the user it happened to is the one who needs to hear that
        // plaintext passwords sat on disk — including the .gitignore line an
        // older project has no entry for.
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("already existed"), "{lines:?}");
        assert!(lines[0].contains("plaintext"), "{lines:?}");
        assert!(
            lines[0].contains("android/.frust-signing.properties"),
            "{lines:?}"
        );

        drop(guard);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn property_values_are_escaped_for_java_properties_load() {
        assert_eq!(
            escape_property_value("C:\\keys\\up.jks"),
            "C:\\\\keys\\\\up.jks"
        );
        assert_eq!(escape_property_value("a\tb\nc"), "a\\tb\\nc");
        // Non-ASCII becomes \uXXXX: `Properties.load(InputStream)` decodes
        // ISO-8859-1, so a raw UTF-8 password would arrive mangled.
        assert_eq!(escape_property_value("pä"), "p\\u00e4");
        // Separators and comment markers are literal inside a value.
        assert_eq!(escape_property_value("a=b:c#d"), "a=b:c#d");
    }

    /// The `storeFile=` value out of a generated file, unescaping the only
    /// escape a filesystem path can realistically carry here: the backslash
    /// doubling `escape_property_value` applies to every `\` (Windows path
    /// separators, and the `\\?\` verbatim-path prefix `fs::canonicalize`
    /// returns there). Never surfaced on Unix, where a real path has no `\`
    /// to double in the first place — a bare "strip the prefix" used to be
    /// enough there, but reading a raw `\\`-doubled path back on Windows
    /// makes `Path::is_absolute`/`is_file` fail against a string that no
    /// longer names the real file.
    fn store_file_line(contents: &str) -> String {
        let raw = contents
            .lines()
            .find_map(|line| line.strip_prefix("storeFile="))
            .unwrap_or_else(|| panic!("no storeFile line in:\n{contents}"));
        unescape_backslashes(raw)
    }

    /// Reverses `escape_property_value`'s `\` → `\\` doubling. Deliberately
    /// narrow (backslash only) — this test helper only ever reads back a
    /// filesystem path, never the `\n`/`\r`/`\t`/`\uXXXX` escapes that
    /// function also emits for arbitrary values.
    fn unescape_backslashes(value: &str) -> String {
        let mut out = String::with_capacity(value.len());
        let mut chars = value.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' && chars.as_str().starts_with('\\') {
                chars.next();
            }
            out.push(ch);
        }
        out
    }
}
