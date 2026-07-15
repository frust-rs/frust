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
    /// `org` + `project_name` (spec Phase 2 task 22): `org.project_name`,
    /// sanitized to `[a-zA-Z0-9_.]` per Flutter's `androidIdentifier` rule
    /// (any other character becomes `_`).
    ///
    /// Kept in sync with `android_run::project::derive_app_id`
    /// (`forgekit-cli`'s `crates/forgekit-cli/src/android_run/project.rs`),
    /// which re-derives the same id as `forgekit run`'s fallback when a
    /// generated project's `forgekit.toml` has no explicit
    /// `[android] identifier` — both must produce the same string for a
    /// given `org`/`project_name` pair.
    pub fn android_identifier(&self) -> String {
        sanitize_android_identifier(&format!("{}.{}", self.org, self.project_name))
    }
}

fn sanitize_android_identifier(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
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
    fn path_vars_include_camel_case_android_identifier_key() {
        let vars = test_context().path_vars();
        assert_eq!(vars.get("androidIdentifier").unwrap(), "dev.f0x.my_app");
    }

    #[test]
    fn title_case_converts_snake_case() {
        assert_eq!(title_case("my_app"), "My App");
        assert_eq!(title_case("app"), "App");
    }
}
