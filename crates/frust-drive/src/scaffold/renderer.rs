//! Content rendering and path-segment placeholder substitution.
//! Flutter's Mustache-style engine is content-rendering only; path
//! templating (`androidIdentifier` → `com/example/app`) is bespoke to
//! Frust, implemented here as a generic segment walker.

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
    env.render_str(template, vars)
        .context("rendering template content")
}

/// Expands placeholder path segments.
///
/// A path segment that exactly matches a key in `vars` is replaced by that
/// key's value; a dotted value (`com.example.app`) expands into nested
/// directory segments (`com/example/app`) — this is the mechanism the
/// Android/iOS templates need for `androidIdentifier` / `iosIdentifier`.
/// Segments that don't match any key pass through unchanged.
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

/// Strips a trailing `.tmpl` from every *directory* path component (the
/// terminal file component's own `.tmpl`/`.copy.tmpl` suffix is already
/// handled by [`crate::scaffold::classify`] before this runs).
///
/// This lets an entire template source tree carry a `.tmpl` suffix on its
/// root directory as a purely organizational marker in
/// `crates/frust-drive/templates/app/`
/// (`android.tmpl/`, `ios.tmpl/`, `macos.tmpl/`, `windows.tmpl/`,
/// `linux.tmpl/`, `web.tmpl/` — mirrored by `crates/frust-drive/templates/app/` itself not
/// needing the suffix since it's the manifest root) without that suffix
/// leaking into the generated project's directory name (`android.tmpl/` →
/// `android/`). Those marker directories are also exactly what
/// [`crate::scaffold::ScaffoldPlatform::template_dir`] names, so the
/// platform-inclusion axis and this stripping stay one convention rather
/// than two.
pub fn strip_tmpl_dir_suffixes(relative: &Path) -> PathBuf {
    let mut components: Vec<Component> = relative.components().collect();
    let Some(file_component) = components.pop() else {
        return PathBuf::new();
    };
    let mut out = PathBuf::new();
    for component in components {
        match component {
            Component::Normal(segment) => {
                let segment = segment.to_string_lossy();
                match segment.strip_suffix(".tmpl") {
                    Some(stripped) => out.push(stripped),
                    None => out.push(segment.as_ref()),
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.push(file_component.as_os_str());
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
        let out = render(
            "name = \"{{ project_name }}\"",
            &vars(&[("project_name", "my_app")]),
        )
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

    #[test]
    fn strip_tmpl_dir_suffixes_strips_root_marker_directory() {
        let out = strip_tmpl_dir_suffixes(Path::new("android.tmpl/build.gradle.kts"));
        assert_eq!(out, Path::new("android/build.gradle.kts"));
    }

    #[test]
    fn strip_tmpl_dir_suffixes_strips_every_matching_intermediate_segment() {
        let out = strip_tmpl_dir_suffixes(Path::new(
            "android.tmpl/gradle/wrapper/gradle-wrapper.properties",
        ));
        assert_eq!(
            out,
            Path::new("android/gradle/wrapper/gradle-wrapper.properties")
        );
    }

    #[test]
    fn strip_tmpl_dir_suffixes_strips_every_platform_marker_directory() {
        // Every `ScaffoldPlatform::template_dir()` value, including the
        // browser target's `web.tmpl/` — the marker convention is uniform
        // across the platform axis, not special-cased per platform.
        for (marker, rendered) in [
            ("android.tmpl/build.gradle.kts", "android/build.gradle.kts"),
            ("ios.tmpl/Runner/Info.plist", "ios/Runner/Info.plist"),
            ("macos.tmpl/Info.plist", "macos/Info.plist"),
            ("windows.tmpl/build.rs", "windows/build.rs"),
            ("linux.tmpl/app.desktop", "linux/app.desktop"),
            ("web.tmpl/index.html", "web/index.html"),
            ("web.tmpl/frust_web.js", "web/frust_web.js"),
        ] {
            assert_eq!(
                strip_tmpl_dir_suffixes(Path::new(marker)),
                Path::new(rendered)
            );
        }
    }

    #[test]
    fn strip_tmpl_dir_suffixes_leaves_the_file_component_alone() {
        // The terminal component's own `.tmpl` suffix is handled upstream
        // by `classify`, not here — a literal `.tmpl`-suffixed file name
        // reaching this function is left untouched.
        let out = strip_tmpl_dir_suffixes(Path::new("android.tmpl/foo.tmpl"));
        assert_eq!(out, Path::new("android/foo.tmpl"));
    }

    #[test]
    fn strip_tmpl_dir_suffixes_no_op_without_tmpl_directories() {
        let out = strip_tmpl_dir_suffixes(Path::new("src/lib.rs"));
        assert_eq!(out, Path::new("src/lib.rs"));
    }
}
