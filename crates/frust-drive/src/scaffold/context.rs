//! Template context construction and project-name validation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Why a `--frust-path` value was rejected by [`resolve_frust_crate_path`]:
/// neither accepted shape names the `frust` package.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrustPathError {
    #[error(
        "--frust-path `{given}` doesn't point at a Frust checkout: expected either the `frust` \
         facade crate itself (a directory whose Cargo.toml has `[package] name = \"frust\"`) or \
         its repo root (a directory containing `crates/frust` with that Cargo.toml) — found \
         neither"
    )]
    NotFacadeCrate { given: String },
}

/// Canonicalises a `--frust-path` argument to the `frust` facade crate
/// directory, accepting either shape:
///
/// 1. `path` itself is the facade crate (`<path>/Cargo.toml` names package
///    `frust`) -> returned unchanged.
/// 2. `path` is the repo root containing it (`<path>/crates/frust/Cargo.toml`
///    names package `frust`) -> returns the nested `<path>/crates/frust`.
/// 3. Neither -> [`FrustPathError::NotFacadeCrate`], naming both accepted
///    shapes.
///
/// Deliberately file-probed rather than string-matched on a trailing
/// `crates/frust` path segment, so a vendored or renamed checkout still
/// resolves. This closes a fourth broken path surface a repo-root value
/// otherwise produces silently: `frust = { path = "<repo-root>" }` points at
/// the root `Cargo.toml`, a virtual workspace manifest with no `[package]`
/// table — a hard Cargo error, not a merely-wrong-but-working path — so a
/// repo-root value cannot be made to work by adjusting the other
/// `frust_path`-derived joins ([`frust_path_from_project_subdir`],
/// [`TemplateContext::frust_embedding_android_dir`],
/// [`TemplateContext::frust_embedding_ios_dir`]); it must be normalised
/// before it ever reaches them.
pub fn resolve_frust_crate_path(path: &Path) -> Result<PathBuf, FrustPathError> {
    if manifest_names_package(path, "frust") {
        return Ok(path.to_path_buf());
    }
    let nested = path.join("crates").join("frust");
    if manifest_names_package(&nested, "frust") {
        return Ok(nested);
    }
    Err(FrustPathError::NotFacadeCrate {
        given: path.display().to_string(),
    })
}

/// A minimal `Cargo.toml` parse: only `[package] name`, serde-ignoring
/// everything else (mirrors `ios_build::team`'s partial-`frust.toml` parse
/// precedent).
#[derive(Debug, Deserialize)]
struct CargoManifestName {
    package: Option<CargoPackageName>,
}

#[derive(Debug, Deserialize)]
struct CargoPackageName {
    name: Option<String>,
}

/// Whether `<dir>/Cargo.toml` exists, parses, and names package `expected`.
/// Any failure along the way (missing file, a virtual-workspace manifest
/// with no `[package]` table, malformed toml) is treated as "no match", not
/// propagated — the caller falls through to the next accepted shape.
fn manifest_names_package(dir: &Path, expected: &str) -> bool {
    let Ok(contents) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
        return false;
    };
    let Ok(manifest) = toml::from_str::<CargoManifestName>(&contents) else {
        return false;
    };
    manifest.package.and_then(|p| p.name).as_deref() == Some(expected)
}

/// Re-base a **project-root-relative** `frust_path` for a consumer that
/// resolves it from a project *subdirectory* one level down (`android/`,
/// `ios/`) instead of from the project root.
///
/// `frust_path` is written into `Cargo.toml`'s `frust = { path = ... }`, and
/// `Cargo.toml` sits at the project root — so that is the base every relative
/// `frust_path` is expressed against (`plugin::apply::resolve_sibling` joins it
/// onto `project_root` for exactly that reason). But Gradle resolves
/// `frust.embedding.dir` / a plugin module's `projectDir` against
/// `<project>/android/`, and Xcode resolves an
/// `XCLocalSwiftPackageReference`'s `relativePath` against `<project>/ios/`
/// (the directory *containing* `Runner.xcodeproj`, not the bundle). Both are
/// one level down, so a relative path
/// needs one extra `../` to climb back out; an **absolute** path is base-
/// independent and is returned byte-identical.
///
/// The adjustment is deliberately **lexical**: the target need not exist at
/// scaffold time (`frust create` runs before any checkout is guaranteed in
/// place), so canonicalizing is not an option.
///
/// The single helper behind all three emitters
/// ([`TemplateContext::frust_embedding_android_dir`],
/// [`TemplateContext::frust_embedding_ios_dir`] and
/// `plugin::apply::repo_relative_path`) — one path convention, not two.
pub fn frust_path_from_project_subdir(frust_path: &str) -> String {
    if Path::new(frust_path).is_absolute() {
        frust_path.to_string()
    } else {
        format!("../{frust_path}")
    }
}

/// Default `[macos] minimum-system-version` a freshly scaffolded project's
/// `macos/Info.plist.tmpl` renders (`LSMinimumSystemVersion`). `frust.toml`'s
/// `[macos]` stub ships commented out (see [`TemplateContext::render_vars`]'s
/// doc), so there is no manifest value to read back at scaffold time; this is
/// the same default the commented stub documents, kept as one constant so the
/// two never drift.
pub const DEFAULT_MACOS_MINIMUM_SYSTEM_VERSION: &str = "11.0";

/// Values substituted into `.tmpl` file contents, and (a subset of) values
/// usable as literal path-segment placeholders.
#[derive(Debug, Clone)]
pub struct TemplateContext {
    pub project_name: String,
    pub title_case_name: String,
    pub org: String,
    pub description: String,
    pub frust_version: String,
    pub frust_path: String,
    /// `--deeplink-scheme`: the URL scheme (e.g. `myapp`, no
    /// `://`) the generated Android manifest/iOS Info.plist register for
    /// deep links, and the value written into the generated `frust.toml`
    /// `[deeplink]` section. `None` renders byte-identical
    /// manifest/Info.plist output to a project with no deep-link config (no
    /// intent-filter, no `CFBundleURLTypes`) — see
    /// [`validate_deeplink_scheme`] for the accepted grammar.
    pub deeplink_scheme: Option<String>,
    /// `--deeplink-host`: an optional host restricting the
    /// Android intent-filter's `<data>` element (`android:host`); iOS's
    /// `CFBundleURLTypes` has no host concept, so this is Android-only and
    /// silently unused by the iOS template. Meaningless without
    /// `deeplink_scheme` also being set.
    pub deeplink_host: Option<String>,
}

impl TemplateContext {
    /// The minijinja rendering context (`{{ project_name }}`, etc.) used
    /// for every `.tmpl` file's content.
    pub fn render_vars(&self) -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            ("project_name", self.project_name.clone()),
            ("title_case_name", self.title_case_name.clone()),
            ("org", self.org.clone()),
            ("description", self.description.clone()),
            ("frust_version", self.frust_version.clone()),
            ("frust_path", self.frust_path.clone()),
            ("android_identifier", self.android_identifier()),
            ("iosIdentifier", self.ios_identifier()),
            ("desktop_identifier", self.desktop_identifier()),
            (
                "macos_minimum_system_version",
                DEFAULT_MACOS_MINIMUM_SYSTEM_VERSION.to_string(),
            ),
            (
                "deeplink_scheme",
                self.deeplink_scheme.clone().unwrap_or_default(),
            ),
            (
                "deeplink_host",
                self.deeplink_host.clone().unwrap_or_default(),
            ),
            (
                "frust_embedding_android_dir",
                self.frust_embedding_android_dir(),
            ),
            ("frust_embedding_ios_dir", self.frust_embedding_ios_dir()),
            ("web_module_name", self.web_module_name().to_string()),
        ])
    }

    /// The `wasm-bindgen --out-name` a browser build of this project
    /// produces, and therefore the module the generated host page's
    /// `?module=` default points at (`./pkg/<this>.js`).
    ///
    /// The crate name, because that is what `wasm-bindgen`'s own default
    /// `--out-name` would be and what `manifest::WebSection::out_name_or`
    /// falls back to (`[app] name`, which the scaffolded `frust.toml`
    /// renders from this same `project_name`). One derivation, named once:
    /// the generated page and the generated build config agree on the module
    /// name without either reading the other.
    pub fn web_module_name(&self) -> &str {
        &self.project_name
    }

    /// Placeholder values usable as *literal path segments* (the
    /// `androidIdentifier` model: a directory literally named after a key
    /// renders to that key's value, with dotted values expanding into
    /// nested directories — see [`crate::scaffold::renderer::expand_path`]).
    /// The Android template is the first consumer:
    /// `android/app/src/main/kotlin/androidIdentifier/` expands to the
    /// nested package directories for the generated `MainActivity.kt`.
    pub fn path_vars(&self) -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            ("project_name", self.project_name.clone()),
            ("org", self.org.clone()),
            ("androidIdentifier", self.android_identifier()),
        ])
    }

    /// Derives the Android application id / Kotlin package name from
    /// `org` + `project_name` via `crate::android_id::derive` — the single
    /// source of truth shared with
    /// `android_run::project::derive_app_id`, which re-derives the same id
    /// as `frust run`'s fallback when a generated project's
    /// `frust.toml` has no explicit `[android] identifier`; both must
    /// produce the same string for a given `org`/`project_name` pair.
    pub fn android_identifier(&self) -> String {
        crate::android_id::derive(&self.org, &self.project_name)
    }

    /// Derives the iOS bundle identifier from `org` + `project_name` via
    /// `crate::ios_id::derive` — the single source of truth shared with
    /// `ios_run`, which re-derives the same id as `frust run`'s fallback
    /// when a generated project's
    /// `frust.toml` has no explicit `[ios] identifier`. Exposed to
    /// templates as the `iosIdentifier` render var; unlike
    /// `androidIdentifier` it is *not* a path-segment placeholder because
    /// the iOS template has no identifier-named directories.
    pub fn ios_identifier(&self) -> String {
        crate::ios_id::derive(&self.org, &self.project_name)
    }

    /// Derives the desktop bundle identifier (macOS `CFBundleIdentifier`,
    /// the Windows product identifier `windows/build.rs`'s resource block
    /// documents, and the Linux desktop-entry `Icon`/id) shared by all three
    /// desktop templates — see the `[desktop] identifier` key in
    /// `frust.toml.tmpl`. Deliberately reuses [`crate::android_id::derive`]
    /// rather than [`crate::ios_id::derive`]'s camelCase transform: unlike an
    /// iOS bundle id, a desktop one is not App-Store-grammar-gated, so there
    /// is no reason to mangle the project name away from its `snake_case`
    /// form — this is the "matches the Android precedent" derivation the
    /// desktop-templates task called for.
    pub fn desktop_identifier(&self) -> String {
        crate::android_id::derive(&self.org, &self.project_name)
    }

    /// Validates the derived iOS bundle identifier (a scaffold-time
    /// fail-fast). Called by [`crate::scaffold::generate`]
    /// before any file is written so a `frust create` whose
    /// `org`/`project_name` produce a grammatically invalid bundle id errors
    /// out with an actionable message instead of emitting a project Xcode
    /// would reject.
    pub(crate) fn validate_ios_identifier(&self) -> Result<(), crate::ios_id::IdError> {
        crate::ios_id::validate(&self.ios_identifier())
    }

    /// The Android embedding Gradle library module's location, derived from
    /// `frust_path` the same way `plugin::apply::plugin_dep_path` derives a
    /// plugin crate directory: `frust` resolves to the facade crate dir
    /// (`crates/frust`), two levels below the repo root, so
    /// `{frust_path}/../../platform/android/frust-embedding` reaches the
    /// module directory.
    ///
    /// The emitted value lands in `android/gradle.properties`'
    /// `frust.embedding.dir` and is resolved by `file(...)` in
    /// `android/settings.gradle.kts` — i.e. from `<project>/android/`, **not**
    /// from the project root a relative `frust_path` is expressed against. So
    /// the path is re-based by [`frust_path_from_project_subdir`] first; an
    /// absolute `frust_path` (the `frust create` default) is unaffected.
    ///
    /// This **widens** the blast radius of the machine-specific
    /// developer-checkout path `frust_path` already carries rather than merely
    /// inheriting it: before the embedding extraction only `cargo build`
    /// needed the frust checkout and a scaffolded app's Gradle build was
    /// self-contained, whereas now a missing or moved checkout fails Gradle
    /// *sync* — the project cannot be opened or configured at all, not just
    /// linked. That is a deliberate trade-off, taken because the value is a
    /// placeholder for a published Maven coordinate once the embedding module
    /// ships to a registry post-crates.io, and because `gradle.properties`
    /// keeps it to one line to edit when a project moves machines.
    pub fn frust_embedding_android_dir(&self) -> String {
        format!(
            "{}/../../platform/android/frust-embedding",
            frust_path_from_project_subdir(&self.frust_path)
        )
    }

    /// The iOS embedding Swift package's location, derived from `frust_path`
    /// the same way as [`Self::frust_embedding_android_dir`] above, and
    /// re-based by the same [`frust_path_from_project_subdir`]: the value
    /// becomes an `XCLocalSwiftPackageReference`'s `relativePath` in
    /// `ios/Runner.xcodeproj`, which Xcode resolves against `<project>/ios/`
    /// — the directory containing the `.xcodeproj`, not the bundle itself.
    ///
    /// It widens the developer-checkout blast radius exactly as the Android
    /// accessor above describes — a missing checkout fails Xcode's *package
    /// resolution*, so the project won't open, not just link — under the same
    /// deliberate trade-off, and is a placeholder for a published Swift
    /// package reference once the embedding package ships to a registry
    /// post-crates.io.
    pub fn frust_embedding_ios_dir(&self) -> String {
        format!(
            "{}/../../platform/ios/FrustEmbedding",
            frust_path_from_project_subdir(&self.frust_path)
        )
    }
}

/// The render-context keys the platform-inclusion axis exposes to every app
/// `.tmpl` file — `{{ platform_windows }}`, `{{ platform_android }}`, etc. —
/// one entry per [`crate::scaffold::ScaffoldPlatform`] variant, merged into
/// [`TemplateContext::render_vars`]'s map by
/// [`crate::scaffold::generate_with_platforms`] before rendering.
///
/// [`crate::scaffold::ScaffoldPlatform`] gates whether a whole
/// `<platform>.tmpl/` subtree is emitted at all, but a platform-agnostic
/// file that is ALWAYS emitted (`Cargo.toml`) can still carry content that
/// only makes sense when a particular platform subtree exists (the
/// `windows/build.rs` wiring) — these keys are what let such a file branch
/// on the selection without the manifest-loop's coarser whole-file
/// include/exclude.
///
/// Each value is `""` (falsy — minijinja treats an empty string as falsy,
/// same as Jinja2) when the platform is not selected, and a non-empty
/// marker string (truthy) when it is, so a template writes the natural
/// `{% if platform_windows %} ... {% endif %}` rather than a string
/// comparison.
pub fn platform_render_vars(
    platforms: &[super::ScaffoldPlatform],
) -> BTreeMap<&'static str, String> {
    use super::ScaffoldPlatform;

    fn flag(platforms: &[ScaffoldPlatform], platform: ScaffoldPlatform) -> String {
        if platforms.contains(&platform) {
            "true".to_string()
        } else {
            String::new()
        }
    }

    BTreeMap::from([
        (
            "platform_android",
            flag(platforms, ScaffoldPlatform::Android),
        ),
        ("platform_ios", flag(platforms, ScaffoldPlatform::Ios)),
        ("platform_macos", flag(platforms, ScaffoldPlatform::Macos)),
        (
            "platform_windows",
            flag(platforms, ScaffoldPlatform::Windows),
        ),
        ("platform_linux", flag(platforms, ScaffoldPlatform::Linux)),
        ("platform_web", flag(platforms, ScaffoldPlatform::Web)),
    ])
}

/// Values substituted into a **design-system** template's `.tmpl` file
/// contents (`templates/design-system/`) — deliberately a strict subset of
/// [`TemplateContext`]'s vars, not that struct reused with dummy values. A
/// design-system crate is a plain library with no platform project, so it
/// carries no `org`, no android/ios identifiers, and no deeplink config;
/// [`crate::scaffold::generate_design_system`] is the counterpart of
/// [`crate::scaffold::generate`] that renders against this context instead.
#[derive(Debug, Clone)]
pub struct DesignSystemContext {
    /// The crate name (also its `DesignLanguage::Custom` identity tag) —
    /// validated with [`validate_project_name`], the same grammar a
    /// `frust create` app scaffold's `project_name` uses.
    pub name: String,
    pub frust_version: String,
    pub frust_path: String,
}

impl DesignSystemContext {
    /// The minijinja rendering context (`{{ name }}`, `{{ title_case_name }}`,
    /// etc.) used for every design-system `.tmpl` file's content.
    pub fn render_vars(&self) -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            ("name", self.name.clone()),
            ("title_case_name", title_case(&self.name)),
            ("frust_version", self.frust_version.clone()),
            ("frust_path", self.frust_path.clone()),
        ])
    }

    /// Placeholder values usable as literal path segments — see
    /// [`TemplateContext::path_vars`]'s doc for the convention. The
    /// design-system template ships no dotted-identifier directory (no
    /// android/ios tree), so this is empty today; kept as a real method
    /// rather than omitted so [`crate::scaffold::generate_design_system`]'s
    /// call shape matches [`crate::scaffold::generate`]'s.
    pub fn path_vars(&self) -> BTreeMap<&'static str, String> {
        BTreeMap::new()
    }
}

/// Why a `--deeplink-scheme` value was rejected. Follows the
/// `[ios] team`-style frust.toml precedent (`ios_build::team`) for what
/// gets validated here versus left to the platform build tools: this is a
/// scaffold-time, actionable check, not a full RFC 3986 scheme grammar
/// validator.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeepLinkError {
    #[error("--deeplink-scheme cannot be empty")]
    Empty,
    #[error(
        "--deeplink-scheme `{0}` must be a bare scheme (e.g. `myapp`), not a full URL — omit \
         the `://`"
    )]
    ContainsSchemeSeparator(String),
}

/// Validates a `--deeplink-scheme` value: non-empty, and not a
/// full URL (no `://` — a common mistake, e.g. passing `myapp://` instead of
/// `myapp`). Deliberately narrow: RFC 3986 scheme-grammar policing (alnum +
/// `+`/`-`/`.`) is left to the platform build tools (Gradle/`xcodebuild`),
/// which already reject a malformed scheme in the generated manifest/plist —
/// this only catches the "pasted a whole URL" mistake before it silently
/// bakes an invalid intent-filter/`CFBundleURLTypes` entry.
pub fn validate_deeplink_scheme(scheme: &str) -> Result<(), DeepLinkError> {
    if scheme.is_empty() {
        return Err(DeepLinkError::Empty);
    }
    if scheme.contains("://") {
        return Err(DeepLinkError::ContainsSchemeSeparator(scheme.to_string()));
    }
    Ok(())
}

/// Rust keywords (2015/2018/2021/2024 strict + reserved), used to reject
/// project names that wouldn't compile as a crate name.
const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

/// Why a project name was rejected. Every variant carries an actionable
/// message (reject `1app`, `my-app` suggesting `my_app`, and keywords).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum NameError {
    #[error("project name cannot be empty")]
    Empty,
    #[error("project name `{name}` must start with a lowercase letter (a-z), not `{first}`")]
    InvalidStart { name: String, first: char },
    #[error(
        "project name `{name}` may only contain lowercase letters, digits, and underscores{}",
        suggestion.as_ref().map(|s| format!(" (try `{s}`)")).unwrap_or_default()
    )]
    InvalidChars {
        name: String,
        suggestion: Option<String>,
    },
    #[error("project name `{name}` is a Rust keyword and cannot be used as a crate name")]
    Keyword { name: String },
}

/// Validates `name` as a Rust crate name: `[a-z][a-z0-9_]*`, non-keyword.
pub fn validate_project_name(name: &str) -> Result<(), NameError> {
    let mut chars = name.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return Err(NameError::Empty),
    };
    if !first.is_ascii_lowercase() {
        return Err(NameError::InvalidStart {
            name: name.to_string(),
            first,
        });
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        let candidate: String = name
            .chars()
            .map(|c| if c == '-' { '_' } else { c })
            .collect();
        let suggestion = (candidate != name
            && candidate
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
        .then_some(candidate);
        return Err(NameError::InvalidChars {
            name: name.to_string(),
            suggestion,
        });
    }
    if RUST_KEYWORDS.contains(&name) {
        return Err(NameError::Keyword {
            name: name.to_string(),
        });
    }
    Ok(())
}

/// Converts a validated `snake_case` project name into `Title Case` (used
/// for doc comments / the generated greeting).
pub fn title_case(project_name: &str) -> String {
    project_name
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_names() {
        assert!(validate_project_name("my_app").is_ok());
        assert!(validate_project_name("app").is_ok());
        assert!(validate_project_name("app2").is_ok());
    }

    #[test]
    fn rejects_leading_digit() {
        let err = validate_project_name("1app").unwrap_err();
        assert!(matches!(err, NameError::InvalidStart { .. }));
    }

    #[test]
    fn rejects_hyphen_with_suggestion() {
        let err = validate_project_name("my-app").unwrap_err();
        match err {
            NameError::InvalidChars { suggestion, .. } => {
                assert_eq!(suggestion.as_deref(), Some("my_app"));
            }
            other => panic!("expected InvalidChars, got {other:?}"),
        }
    }

    #[test]
    fn rejects_keyword() {
        let err = validate_project_name("fn").unwrap_err();
        assert!(matches!(err, NameError::Keyword { .. }));
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(validate_project_name(""), Err(NameError::Empty));
    }

    fn test_context() -> TemplateContext {
        TemplateContext {
            project_name: "my_app".into(),
            title_case_name: "My App".into(),
            org: "dev.f0x".into(),
            description: "A new Frust application.".into(),
            frust_version: "0.1.0".into(),
            frust_path: "/path/to/frust".into(),
            deeplink_scheme: None,
            deeplink_host: None,
        }
    }

    #[test]
    fn android_identifier_joins_org_and_project_name() {
        assert_eq!(test_context().android_identifier(), "dev.f0x.my_app");
    }

    #[test]
    fn android_identifier_sanitizes_invalid_characters() {
        let mut ctx = test_context();
        ctx.org = "dev f0x".into();
        ctx.project_name = "my-app".into();
        assert_eq!(ctx.android_identifier(), "dev_f0x.my_app");
    }

    #[test]
    fn render_vars_include_android_identifier() {
        let vars = test_context().render_vars();
        assert_eq!(vars.get("android_identifier").unwrap(), "dev.f0x.my_app");
    }

    #[test]
    fn ios_identifier_camel_cases_project_name() {
        // Underscores are invalid in a bundle-id segment, so `my_app`
        // becomes `myApp` (unlike the Android id, which keeps the underscore).
        assert_eq!(test_context().ios_identifier(), "dev.f0x.myApp");
    }

    #[test]
    fn render_vars_include_ios_identifier() {
        let vars = test_context().render_vars();
        assert_eq!(vars.get("iosIdentifier").unwrap(), "dev.f0x.myApp");
    }

    #[test]
    fn desktop_identifier_matches_android_derivation_not_ios_camel_case() {
        // The whole point of reusing `android_id::derive`: unlike the iOS
        // bundle id, the underscore in `my_app` survives verbatim.
        let mut ctx = test_context();
        ctx.project_name = "my_app".into();
        assert_eq!(ctx.desktop_identifier(), "dev.f0x.my_app");
        assert_eq!(ctx.desktop_identifier(), ctx.android_identifier());
        assert_ne!(ctx.desktop_identifier(), ctx.ios_identifier());
    }

    #[test]
    fn render_vars_include_desktop_identifier_and_macos_minimum_system_version() {
        let vars = test_context().render_vars();
        assert_eq!(vars.get("desktop_identifier").unwrap(), "dev.f0x.my_app");
        assert_eq!(
            vars.get("macos_minimum_system_version").unwrap(),
            DEFAULT_MACOS_MINIMUM_SYSTEM_VERSION,
        );
    }

    #[test]
    fn ios_identifier_is_not_a_path_var() {
        // The iOS template has no identifier-named directories, so
        // `iosIdentifier` must not leak into path-segment substitution.
        assert!(!test_context().path_vars().contains_key("iosIdentifier"));
    }

    #[test]
    fn validate_ios_identifier_accepts_derived_id() {
        assert!(test_context().validate_ios_identifier().is_ok());
    }

    #[test]
    fn validate_ios_identifier_rejects_invalid_org() {
        // An `org` carrying whitespace derives a bundle id with an invalid
        // segment; scaffold-time validation must reject it (fail-fast).
        let mut ctx = test_context();
        ctx.org = "dev f0x".into();
        assert!(ctx.validate_ios_identifier().is_err());
    }

    #[test]
    fn path_vars_include_camel_case_android_identifier_key() {
        let vars = test_context().path_vars();
        assert_eq!(vars.get("androidIdentifier").unwrap(), "dev.f0x.my_app");
    }

    #[test]
    fn title_case_converts_snake_case() {
        assert_eq!(title_case("my_app"), "My App");
        assert_eq!(title_case("app"), "App");
    }

    #[test]
    fn validate_deeplink_scheme_accepts_bare_scheme() {
        assert!(validate_deeplink_scheme("myapp").is_ok());
        assert!(validate_deeplink_scheme("my-app+1").is_ok());
    }

    #[test]
    fn validate_deeplink_scheme_rejects_empty() {
        assert_eq!(validate_deeplink_scheme(""), Err(DeepLinkError::Empty));
    }

    #[test]
    fn validate_deeplink_scheme_rejects_full_url() {
        let err = validate_deeplink_scheme("myapp://").unwrap_err();
        assert!(matches!(err, DeepLinkError::ContainsSchemeSeparator(_)));
        let err = validate_deeplink_scheme("https://example.com").unwrap_err();
        assert!(matches!(err, DeepLinkError::ContainsSchemeSeparator(_)));
    }

    #[test]
    fn render_vars_default_deeplink_keys_to_empty_string() {
        let vars = test_context().render_vars();
        assert_eq!(vars.get("deeplink_scheme").unwrap(), "");
        assert_eq!(vars.get("deeplink_host").unwrap(), "");
    }

    #[test]
    fn render_vars_include_configured_deeplink_scheme_and_host() {
        let mut ctx = test_context();
        ctx.deeplink_scheme = Some("myapp".into());
        ctx.deeplink_host = Some("open".into());
        let vars = ctx.render_vars();
        assert_eq!(vars.get("deeplink_scheme").unwrap(), "myapp");
        assert_eq!(vars.get("deeplink_host").unwrap(), "open");
    }

    #[test]
    fn embedding_dirs_derive_from_frust_path() {
        let ctx = test_context(); // frust_path = "/path/to/frust"
        assert_eq!(
            ctx.frust_embedding_android_dir(),
            "/path/to/frust/../../platform/android/frust-embedding"
        );
        assert_eq!(
            ctx.frust_embedding_ios_dir(),
            "/path/to/frust/../../platform/ios/FrustEmbedding"
        );
    }

    #[test]
    fn embedding_dirs_are_render_vars_not_path_vars() {
        let ctx = test_context();
        let render_vars = ctx.render_vars();
        assert_eq!(
            render_vars.get("frust_embedding_android_dir").unwrap(),
            &ctx.frust_embedding_android_dir()
        );
        assert_eq!(
            render_vars.get("frust_embedding_ios_dir").unwrap(),
            &ctx.frust_embedding_ios_dir()
        );

        let path_vars = ctx.path_vars();
        assert!(!path_vars.contains_key("frust_embedding_android_dir"));
        assert!(!path_vars.contains_key("frust_embedding_ios_dir"));
    }

    #[test]
    fn embedding_dirs_preserve_absolute_or_relative_form() {
        let mut ctx = test_context();
        ctx.frust_path = "/absolute/frust".into();
        assert!(ctx.frust_embedding_android_dir().starts_with('/'));
        assert!(ctx.frust_embedding_ios_dir().starts_with('/'));
        // An absolute path is base-independent: emitted byte-identical.
        assert_eq!(
            ctx.frust_embedding_android_dir(),
            "/absolute/frust/../../platform/android/frust-embedding"
        );
        assert_eq!(
            ctx.frust_embedding_ios_dir(),
            "/absolute/frust/../../platform/ios/FrustEmbedding"
        );

        // A relative path carries one extra `../`: both values are resolved
        // from a project *subdirectory* (`android/`, `ios/`), while
        // `frust_path` itself is expressed against the project root.
        ctx.frust_path = "../relative/frust".into();
        assert!(!ctx.frust_embedding_android_dir().starts_with('/'));
        assert!(!ctx.frust_embedding_ios_dir().starts_with('/'));
        assert_eq!(
            ctx.frust_embedding_android_dir(),
            "../../relative/frust/../../platform/android/frust-embedding"
        );
        assert_eq!(
            ctx.frust_embedding_ios_dir(),
            "../../relative/frust/../../platform/ios/FrustEmbedding"
        );
    }

    /// Lexical `..`/`.` collapse — the scaffold targets need not exist, so
    /// `fs::canonicalize` is unavailable (mirrors the accessors' own
    /// deliberately lexical derivation).
    fn normalize_lexically(path: &std::path::Path) -> std::path::PathBuf {
        use std::path::{Component, PathBuf};
        let mut out = PathBuf::new();
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        out
    }

    /// The *intent* behind [`embedding_dirs_preserve_absolute_or_relative_form`]'s
    /// literal strings: each emitted path, resolved from the subdirectory that
    /// actually consumes it, must land where the project-root-relative
    /// `frust_path` convention reaches from the project root. A string-only
    /// assertion is what let the one-level-short form ship in the first place.
    #[test]
    fn embedding_dirs_resolve_from_their_consumers_base_directory() {
        use std::path::Path;

        let project_root = Path::new("/projects/my_app");
        for frust_path in [
            "../checkouts/frust/crates/frust",
            "vendor/frust/crates/frust",
            "/absolute/checkout/crates/frust",
        ] {
            let mut ctx = test_context();
            ctx.frust_path = frust_path.into();

            // What `frust_path`'s own (project-root-relative) convention
            // reaches — the same walk `plugin::apply::resolve_sibling` does.
            let android_truth = normalize_lexically(
                &project_root
                    .join(frust_path)
                    .join("../../platform/android/frust-embedding"),
            );
            let ios_truth = normalize_lexically(
                &project_root
                    .join(frust_path)
                    .join("../../platform/ios/FrustEmbedding"),
            );

            // What the emitted values reach from the directories that
            // actually resolve them: `<proj>/android/` (Gradle) and
            // `<proj>/ios/` (Xcode, the dir containing `Runner.xcodeproj`).
            let android_actual = normalize_lexically(
                &project_root
                    .join("android")
                    .join(ctx.frust_embedding_android_dir()),
            );
            let ios_actual =
                normalize_lexically(&project_root.join("ios").join(ctx.frust_embedding_ios_dir()));

            assert_eq!(
                android_actual, android_truth,
                "android embedding dir for frust_path `{frust_path}`"
            );
            assert_eq!(
                ios_actual, ios_truth,
                "ios embedding dir for frust_path `{frust_path}`"
            );
        }
    }

    #[test]
    fn frust_path_from_project_subdir_climbs_only_for_relative_paths() {
        assert_eq!(
            frust_path_from_project_subdir("/abs/frust"),
            "/abs/frust",
            "an absolute path is base-independent"
        );
        assert_eq!(frust_path_from_project_subdir("../frust"), "../../frust");
        assert_eq!(
            frust_path_from_project_subdir("vendor/frust"),
            "../vendor/frust"
        );
    }

    fn test_design_system_context() -> DesignSystemContext {
        DesignSystemContext {
            name: "acme_design".into(),
            frust_version: "0.1.0".into(),
            frust_path: "/path/to/frust".into(),
        }
    }

    #[test]
    fn design_system_render_vars_include_name_and_derived_title_case() {
        let vars = test_design_system_context().render_vars();
        assert_eq!(vars.get("name").unwrap(), "acme_design");
        assert_eq!(vars.get("title_case_name").unwrap(), "Acme Design");
        assert_eq!(vars.get("frust_path").unwrap(), "/path/to/frust");
        assert_eq!(vars.get("frust_version").unwrap(), "0.1.0");
    }

    #[test]
    fn design_system_path_vars_are_empty() {
        assert!(test_design_system_context().path_vars().is_empty());
    }

    #[test]
    fn platform_render_vars_carries_one_falsy_or_truthy_entry_per_platform() {
        use super::super::ScaffoldPlatform;

        let vars = platform_render_vars(&[ScaffoldPlatform::Windows, ScaffoldPlatform::Web]);
        assert_eq!(vars.len(), ScaffoldPlatform::ALL.len());
        assert_eq!(vars.get("platform_windows").unwrap(), "true");
        assert_eq!(vars.get("platform_web").unwrap(), "true");
        for key in [
            "platform_android",
            "platform_ios",
            "platform_macos",
            "platform_linux",
        ] {
            assert_eq!(vars.get(key).unwrap(), "", "{key}");
        }
    }

    #[test]
    fn platform_render_vars_over_the_default_set_marks_every_platform_but_web() {
        use super::super::ScaffoldPlatform;

        let vars = platform_render_vars(ScaffoldPlatform::DEFAULT);
        assert_eq!(vars.get("platform_web").unwrap(), "");
        for key in [
            "platform_android",
            "platform_ios",
            "platform_macos",
            "platform_windows",
            "platform_linux",
        ] {
            assert_eq!(vars.get(key).unwrap(), "true", "{key}");
        }
    }

    #[test]
    fn platform_render_vars_over_an_empty_set_marks_nothing() {
        use super::super::ScaffoldPlatform;

        let vars = platform_render_vars(&[]);
        assert_eq!(vars.len(), ScaffoldPlatform::ALL.len());
        assert!(vars.values().all(|v| v.is_empty()));
    }

    mod resolve_frust_crate_path_tests {
        use super::*;
        use std::sync::atomic::{AtomicU32, Ordering};

        fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "frust-drive-frust-path-test-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            dir
        }

        fn write_manifest(dir: &std::path::Path, package_name: &str) {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(
                dir.join("Cargo.toml"),
                format!("[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\n"),
            )
            .unwrap();
        }

        /// Shape 1: `path` is the facade crate itself -> used as-is.
        #[test]
        fn accepts_facade_crate_directly() {
            let root = unique_temp_dir("facade-direct");
            write_manifest(&root, "frust");

            assert_eq!(resolve_frust_crate_path(&root).unwrap(), root);

            let _ = std::fs::remove_dir_all(&root);
        }

        /// Shape 2: `path` is the repo root containing `crates/frust` ->
        /// canonicalises to the nested facade crate directory. This is the
        /// previously-uncovered case: a repo-root value must resolve to a
        /// real, buildable path rather than silently baking `frust = {
        /// path = "<repo-root>" }` (a virtual-workspace manifest with no
        /// `[package]` table).
        #[test]
        fn canonicalises_repo_root_to_nested_facade_crate() {
            let root = unique_temp_dir("repo-root");
            // A virtual-workspace manifest at the root — no `[package]`
            // table, so it must NOT satisfy shape 1.
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
            write_manifest(&root.join("crates").join("frust"), "frust");

            assert_eq!(
                resolve_frust_crate_path(&root).unwrap(),
                root.join("crates").join("frust")
            );

            let _ = std::fs::remove_dir_all(&root);
        }

        /// Shape 3: neither the path itself nor `<path>/crates/frust` names
        /// package `frust` -> hard error naming both accepted shapes.
        #[test]
        fn rejects_unrelated_directory_naming_both_accepted_shapes() {
            let dir = unique_temp_dir("unrelated");
            std::fs::create_dir_all(&dir).unwrap();

            let err = resolve_frust_crate_path(&dir).unwrap_err();
            let message = err.to_string();
            assert!(message.contains(&dir.display().to_string()), "{message}");
            assert!(message.contains("facade crate"), "{message}");
            assert!(message.contains("crates/frust"), "{message}");

            let _ = std::fs::remove_dir_all(&dir);
        }

        /// A directory with no `Cargo.toml` at all (not just a wrong
        /// package name) hits the same rejection path.
        #[test]
        fn rejects_directory_with_no_cargo_toml() {
            let dir = unique_temp_dir("no-manifest");
            std::fs::create_dir_all(&dir).unwrap();

            assert!(resolve_frust_crate_path(&dir).is_err());

            let _ = std::fs::remove_dir_all(&dir);
        }

        /// A vendored/renamed checkout — the facade crate directory itself
        /// doesn't literally end in `crates/frust` — still resolves via
        /// shape 1, since resolution is file-probed, not string-matched.
        #[test]
        fn accepts_vendored_checkout_with_nonstandard_directory_name() {
            let root = unique_temp_dir("vendored");
            let vendored = root.join("third_party").join("frust-vendor");
            write_manifest(&vendored, "frust");

            assert_eq!(resolve_frust_crate_path(&vendored).unwrap(), vendored);

            let _ = std::fs::remove_dir_all(&root);
        }
    }
}
