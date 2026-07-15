//! Single source of truth for deriving an Android application id from
//! `org` + `project_name`, and for validating that any Android application
//! id (explicit `[android] identifier` or derived fallback) is grammatically
//! safe before it is ever interpolated into an `adb shell` command line.
//!
//! Two call sites previously duplicated a byte-identical `sanitize` helper —
//! `scaffold::context::TemplateContext::android_identifier` (used when
//! rendering a new project's Android template) and
//! `android_run::project::detect` (used to resolve the id `forgekit run`
//! launches). Both now delegate here.
//!
//! [`validate`] closes an injection path: `android_run::project::detect`
//! resolves an `app_id` that flows verbatim into
//! `android_run::adb::launch`'s `adb shell am start -n <id>/…` and
//! `android_run::adb::resolve_pid`'s `adb shell pidof <id>` — the device
//! shell interprets metacharacters in that string, so an explicit
//! `[android] identifier` in `forgekit.toml` (or, less likely, a derived id
//! with unlucky input) could inject arbitrary shell commands on the
//! connected device. Validating the id at the single point it's resolved
//! (`android_run::project::detect`) means every downstream consumer can rely
//! on it already being a safe, grammatically valid Android application id.

use thiserror::Error;

/// Derives the Android application id / Kotlin package name from `org` +
/// `project_name`: `org.project_name`, sanitized to `[a-zA-Z0-9_.]` per
/// Flutter's `androidIdentifier` rule (any other character becomes `_`).
///
/// This alone does not guarantee a grammatically valid Android application
/// id (e.g. leading digits or empty segments can survive sanitization) — see
/// [`validate`].
pub(crate) fn derive(org: &str, project_name: &str) -> String {
    sanitize(&format!("{org}.{project_name}"))
}

fn sanitize(raw: &str) -> String {
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

/// Why an Android application id failed grammar validation. Every variant
/// carries the offending value so the caller can build an actionable error.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum IdError {
    #[error("Android application id cannot be empty")]
    Empty,
    #[error(
        "Android application id `{id}` must be at least two dot-separated segments (e.g. `dev.example.app`)"
    )]
    TooFewSegments { id: String },
    #[error(
        "Android application id `{id}` has an invalid segment `{segment}` — each segment must match `[A-Za-z_][A-Za-z0-9_]*`"
    )]
    InvalidSegment { id: String, segment: String },
}

/// Validates `id` against Android package grammar: dot-separated segments,
/// each matching `[A-Za-z_][A-Za-z0-9_]*`, at least two segments. This is
/// deliberately stricter than [`sanitize`]'s character allow-list — it also
/// rejects empty segments and segments starting with a digit, which
/// `sanitize` alone cannot catch (a leading digit or a `..` both survive
/// character sanitization untouched).
pub(crate) fn validate(id: &str) -> Result<(), IdError> {
    if id.is_empty() {
        return Err(IdError::Empty);
    }
    let segments: Vec<&str> = id.split('.').collect();
    if segments.len() < 2 {
        return Err(IdError::TooFewSegments { id: id.to_string() });
    }
    for segment in &segments {
        let mut chars = segment.chars();
        let valid_first = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_');
        let valid_rest = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid_first || !valid_rest {
            return Err(IdError::InvalidSegment {
                id: id.to_string(),
                segment: segment.to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_joins_org_and_project_name() {
        assert_eq!(derive("dev.f0x", "myapp"), "dev.f0x.myapp");
    }

    #[test]
    fn sanitize_keeps_alphanumeric_underscore_dot() {
        assert_eq!(sanitize("aZ09_."), "aZ09_.");
    }

    #[test]
    fn sanitize_replaces_every_other_char_class_with_underscore() {
        // Exhaustive-ish sweep over the disallowed-char classes: space,
        // hyphen, slash, semicolon, `$`, parens, backtick.
        assert_eq!(sanitize(" -/;$()`"), "________");
    }

    #[test]
    fn validate_accepts_grammatically_valid_ids() {
        assert!(validate("dev.forgekit.app").is_ok());
        assert!(validate("a_b.c1").is_ok());
    }

    #[test]
    fn validate_rejects_shell_injection_attempt_semicolon() {
        let err = validate("x; rm -rf /").unwrap_err();
        assert!(matches!(
            err,
            IdError::TooFewSegments { .. } | IdError::InvalidSegment { .. }
        ));
    }

    #[test]
    fn validate_rejects_command_substitution() {
        assert!(validate("$(cmd)").is_err());
    }

    #[test]
    fn validate_rejects_backticks() {
        assert!(validate("`cmd`").is_err());
    }

    #[test]
    fn validate_rejects_single_segment() {
        let err = validate("single").unwrap_err();
        assert_eq!(
            err,
            IdError::TooFewSegments {
                id: "single".to_string()
            }
        );
    }

    #[test]
    fn validate_rejects_leading_digit_segment() {
        let err = validate("1bad.seg").unwrap_err();
        assert_eq!(
            err,
            IdError::InvalidSegment {
                id: "1bad.seg".to_string(),
                segment: "1bad".to_string()
            }
        );
    }

    #[test]
    fn validate_rejects_empty_segment() {
        let err = validate("a..b").unwrap_err();
        assert_eq!(
            err,
            IdError::InvalidSegment {
                id: "a..b".to_string(),
                segment: "".to_string()
            }
        );
    }

    #[test]
    fn validate_rejects_empty_string() {
        assert_eq!(validate(""), Err(IdError::Empty));
    }
}
