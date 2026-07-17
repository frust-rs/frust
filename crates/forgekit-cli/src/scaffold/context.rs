//! Template context construction and project-name validation (spec §12.3).

use std::collections::BTreeMap;

use thiserror::Error;

/// Values substituted into `.tmpl` file contents, and (a subset of) values
/// usable as literal path-segment placeholders.
#[derive(Debug, Clone)]
pub struct TemplateContext {
    pub project_name: String,
    pub title_case_name: String,
    pub org: String,
    pub description: String,
    pub forgekit_version: String,
    pub forgekit_path: String,
    /// `--deeplink-scheme` (task 07): the URL scheme (e.g. `myapp`, no
    /// `://`) the generated Android manifest/iOS Info.plist register for
    /// deep links, and the value written into the generated `forgekit.toml`
    /// `[deeplink]` section. `None` renders byte-identical
    /// manifest/Info.plist output to a project with no deep-link config (no
    /// intent-filter, no `CFBundleURLTypes`) — see
    /// [`validate_deeplink_scheme`] for the accepted grammar.
    pub deeplink_scheme: Option<String>,
    /// `--deeplink-host` (task 07): an optional host restricting the
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
            ("forgekit_version", self.forgekit_version.clone()),
            ("forgekit_path", self.forgekit_path.clone()),
            ("android_identifier", self.android_identifier()),
            ("iosIdentifier", self.ios_identifier()),
            (
                "deeplink_scheme",
                self.deeplink_scheme.clone().unwrap_or_default(),
            ),
            (
                "deeplink_host",
                self.deeplink_host.clone().unwrap_or_default(),
            ),
        ])
    }

    /// Placeholder values usable as *literal path segments* (spec §12.3's
    /// `androidIdentifier` model: a directory literally named after a key
    /// renders to that key's value, with dotted values expanding into
    /// nested directories — see [`crate::scaffold::renderer::expand_path`]).
    /// The Android template (spec Phase 2 task 22) is the first consumer:
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
    /// `org` + `project_name` (spec Phase 2 task 22) via
    /// `crate::android_id::derive` — the single source of truth shared with
    /// `android_run::project::derive_app_id`, which re-derives the same id
    /// as `forgekit run`'s fallback when a generated project's
    /// `forgekit.toml` has no explicit `[android] identifier`; both must
    /// produce the same string for a given `org`/`project_name` pair.
    pub fn android_identifier(&self) -> String {
        crate::android_id::derive(&self.org, &self.project_name)
    }

    /// Derives the iOS bundle identifier from `org` + `project_name` (spec
    /// Phase 3 task 34) via `crate::ios_id::derive` — the single source of
    /// truth shared with `ios_run` (task 35), which re-derives the same id
    /// as `forgekit run`'s fallback when a generated project's
    /// `forgekit.toml` has no explicit `[ios] identifier`. Exposed to
    /// templates as the `iosIdentifier` render var; unlike
    /// `androidIdentifier` it is *not* a path-segment placeholder because
    /// the iOS template has no identifier-named directories.
    pub fn ios_identifier(&self) -> String {
        crate::ios_id::derive(&self.org, &self.project_name)
    }

    /// Validates the derived iOS bundle identifier (spec Phase 3 task 34's
    /// scaffold-time fail-fast). Called by [`crate::scaffold::generate`]
    /// before any file is written so a `forgekit create` whose
    /// `org`/`project_name` produce a grammatically invalid bundle id errors
    /// out with an actionable message instead of emitting a project Xcode
    /// would reject.
    pub(crate) fn validate_ios_identifier(&self) -> Result<(), crate::ios_id::IdError> {
        crate::ios_id::validate(&self.ios_identifier())
    }
}

/// Why a `--deeplink-scheme` value was rejected (task 07). Follows the
/// `[ios] team`-style forgekit.toml precedent (`ios_build::team`) for what
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

/// Validates a `--deeplink-scheme` value (task 07): non-empty, and not a
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
/// message (spec §12.3 acceptance criteria: reject `1app`, `my-app`
/// suggesting `my_app`, and keywords).
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
            description: "A new ForgeKit application.".into(),
            forgekit_version: "0.1.0".into(),
            forgekit_path: "/path/to/forgekit".into(),
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
}
