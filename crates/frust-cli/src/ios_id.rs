//! Single source of truth for deriving an iOS bundle identifier from
//! `org` + `project_name`, and for validating that any bundle identifier
//! (explicit `[ios] identifier` or derived fallback) is grammatically safe
//! before it is interpolated into an Xcode project or an `xcrun simctl`
//! command line.
//!
//! This mirrors [`crate::android_id`]'s derive/validate/`IdError` structure,
//! but for Apple's bundle-identifier grammar rather than Android's package
//! grammar. The two differ in one load-bearing way: an underscore is a valid
//! Android package character but is *not* valid in a bundle-id segment, so
//! the (snake_case) project name is camelCased the way Flutter's iOS template
//! does (`my_app` → `myApp`) rather than passed through verbatim.
//!
//! [`validate`] is called at two points: at scaffold time (fail-fast — a
//! `frust create` whose `org`/`project_name` derive an invalid bundle id
//! errors out before writing any files) and later at run time (task 35's
//! `ios_run`, where the resolved id flows into `simctl launch <id>` and must
//! already be a safe, grammatically valid identifier).

use thiserror::Error;

/// Derives the iOS bundle identifier from `org` + `project_name`:
/// `org.camelCase(project_name)`. Underscores are not valid in a bundle-id
/// segment, so the (snake_case) project name is camelCased the way Flutter's
/// template does (`my_app` → `myApp`).
///
/// This alone does not guarantee a grammatically valid bundle identifier —
/// e.g. an `org` carrying whitespace or metacharacters survives untouched,
/// and a `project_name` that camelCases to empty leaves a trailing empty
/// segment. Callers must gate the result through [`validate`].
pub(crate) fn derive(org: &str, project_name: &str) -> String {
    format!("{org}.{}", camel_case(project_name))
}

/// Converts a `snake_case` project name to `camelCase` (`my_app` → `myApp`):
/// the first `_`-delimited word is kept as-is and each subsequent non-empty
/// word has its first character upper-cased. Empty words (from leading,
/// trailing, or doubled underscores) are skipped.
fn camel_case(name: &str) -> String {
    let mut words = name.split('_').filter(|word| !word.is_empty());
    let first = words.next().unwrap_or_default().to_string();
    words.fold(first, |mut acc, word| {
        let mut chars = word.chars();
        if let Some(c) = chars.next() {
            acc.extend(c.to_uppercase());
            acc.push_str(chars.as_str());
        }
        acc
    })
}

/// Why an iOS bundle identifier failed grammar validation. Every variant
/// carries the offending value so the caller can build an actionable error.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum IdError {
    #[error("iOS bundle identifier cannot be empty")]
    Empty,
    #[error(
        "iOS bundle identifier `{id}` must be at least two dot-separated segments (e.g. `dev.example.app`)"
    )]
    TooFewSegments { id: String },
    #[error(
        "iOS bundle identifier `{id}` has an invalid segment `{segment}` — each segment must match `[A-Za-z0-9][A-Za-z0-9-]*`"
    )]
    InvalidSegment { id: String, segment: String },
}

/// Validates `id` against Apple's bundle-identifier grammar: dot-separated
/// segments, each starting with an alphanumeric and otherwise matching
/// `[A-Za-z0-9-]`, with at least two segments. Rejecting empty segments and
/// non-`[A-Za-z0-9-]` characters is what closes the injection path for a
/// value that later reaches an `xcrun simctl launch <id>` command line.
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
        let valid_first = matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric());
        let valid_rest = chars.all(|c| c.is_ascii_alphanumeric() || c == '-');
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
    fn derive_joins_org_and_camel_cased_project_name() {
        assert_eq!(derive("dev.f0x", "myapp"), "dev.f0x.myapp");
    }

    #[test]
    fn derive_camel_cases_underscored_project_name() {
        assert_eq!(derive("dev.f0x", "my_app"), "dev.f0x.myApp");
        assert_eq!(
            derive("com.example", "my_cool_app"),
            "com.example.myCoolApp"
        );
    }

    #[test]
    fn camel_case_keeps_first_word_lowercase() {
        assert_eq!(camel_case("app"), "app");
        assert_eq!(camel_case("my_app"), "myApp");
        assert_eq!(camel_case("a_b_c"), "aBC");
    }

    #[test]
    fn camel_case_skips_empty_words() {
        // Doubled/leading/trailing underscores can't occur in a validated
        // project name, but the helper must not panic or emit empty segments
        // if they do.
        assert_eq!(camel_case("my__app"), "myApp");
        assert_eq!(camel_case("_my_app_"), "myApp");
    }

    #[test]
    fn validate_accepts_grammatically_valid_ids() {
        assert!(validate("dev.frust.app").is_ok());
        assert!(validate("com.example.myApp").is_ok());
        // Hyphens and leading digits are legal in a bundle-id segment.
        assert!(validate("com.example.my-app").is_ok());
        assert!(validate("com.example.app2").is_ok());
    }

    #[test]
    fn validate_rejects_empty_string() {
        assert_eq!(validate(""), Err(IdError::Empty));
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
    fn validate_rejects_underscore_segment() {
        // The whole point of camelCasing: an underscore is invalid in a
        // bundle-id segment, so a raw snake_case name must be rejected.
        let err = validate("dev.f0x.my_app").unwrap_err();
        assert_eq!(
            err,
            IdError::InvalidSegment {
                id: "dev.f0x.my_app".to_string(),
                segment: "my_app".to_string()
            }
        );
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
    fn validate_rejects_whitespace_segment() {
        assert!(validate("com example.app").is_err());
    }

    #[test]
    fn validate_rejects_empty_segment() {
        let err = validate("a..b").unwrap_err();
        assert_eq!(
            err,
            IdError::InvalidSegment {
                id: "a..b".to_string(),
                segment: String::new()
            }
        );
    }
}
