//! Content rendering and path-segment placeholder substitution (spec
//! §12.3). Flutter's Mustache-style engine is content-rendering only;
//! path templating (`androidIdentifier` → `com/example/app`) is bespoke
//! to ForgeKit, implemented here as a generic segment walker.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use minijinja::{Environment, UndefinedBehavior};

/// Renders `template` against `vars`. Undefined behavior is strict: an
/// unknown `{{ placeholder }}` is a hard error, so template/context drift
/// is caught in CI rather than silently emitting `undefined` into
/// generated files.
pub fn render(template: &str, vars: &BTreeMap<&str, String>) -> Result<String> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.render_str(template, vars).context("rendering template content")
}

/// Expands placeholder path segments.
///
/// A path segment that exactly matches a key in `vars` is replaced by that
/// key's value; a dotted value (`com.example.app`) expands into nested
/// directory segments (`com/example/app`) — this is the mechanism the
/// spec Phase 2/3 Android/iOS templates need for `androidIdentifier` /
/// `iosIdentifier`. Segments that don't match any key pass through
/// unchanged.
pub fn expand_path(relative: &Path, vars: &BTreeMap<&str, String>) -> PathBuf {
    let mut out = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(segment) => {
                let segment = segment.to_string_lossy();
                if let Some(value) = vars.get(segment.as_ref()) {
                    for part in value.split('.') {
                        out.push(part);
                    }
                } else {
                    out.push(segment.as_ref());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&'static str, &str)]) -> BTreeMap<&'static str, String> {
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    #[test]
    fn render_substitutes_known_vars() {
        let out = render("name = \"{{ project_name }}\"", &vars(&[("project_name", "my_app")]))
            .unwrap();
        assert_eq!(out, "name = \"my_app\"");
    }

    #[test]
    fn render_rejects_unknown_placeholder() {
        assert!(render("{{ nope }}", &BTreeMap::new()).is_err());
    }

    #[test]
    fn expand_path_passes_through_unmatched_segments() {
        let out = expand_path(Path::new("src/lib.rs"), &BTreeMap::new());
        assert_eq!(out, Path::new("src/lib.rs"));
    }

    #[test]
    fn expand_path_substitutes_literal_segment() {
        let out = expand_path(
            Path::new("src/projectName/main.rs"),
            &vars(&[("projectName", "myapp")]),
        );
        assert_eq!(out, Path::new("src/myapp/main.rs"));
    }

    #[test]
    fn expand_path_expands_dotted_android_identifier() {
        let out = expand_path(
            Path::new("android/app/src/main/kotlin/androidIdentifier/MainActivity.kt"),
            &vars(&[("androidIdentifier", "com.example.app")]),
        );
        assert_eq!(
            out,
            Path::new("android/app/src/main/kotlin/com/example/app/MainActivity.kt")
        );
    }

    #[test]
    fn expand_path_leaves_non_placeholder_dotted_segments_alone() {
        // A segment that isn't a known placeholder key is copied verbatim,
        // even if it happens to contain dots.
        let out = expand_path(Path::new("src/lib.rs"), &vars(&[("org", "dev.f0x")]));
        assert_eq!(out, Path::new("src/lib.rs"));
    }
}
