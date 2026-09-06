//! The desktop identity one assembly runs against: every value the three
//! bundle layouts need, resolved once from `frust.toml` (plus the app's
//! `Cargo.toml` for the binary name) with each fallback applied here rather
//! than at three call sites.
//!
//! The fallbacks matter because every desktop section is optional, and a
//! project scaffolded before the desktop templates existed has none of them:
//! the display name falls back to `[app] name`, and the identifier to the same
//! `org` + `name` derivation `frust create` bakes into `[desktop] identifier`
//! (`crate::android_id::derive`), so a manifest-less project and a scaffolded
//! one land on the same identifier.
//!
//! **This is also the path-safety gate.** Every value resolved here goes on to
//! *become* a path: the display name is the `.app` directory's name, the
//! binary name is the Linux bundle directory's, the identifier is the
//! `.desktop` file's stem and each hicolor icon leaf's name. Those directories
//! are then created — and, for a rebuild, recursively **deleted** — so a value
//! carrying `../..` or a leading `/` would reach outside `dist/` with the full
//! force of `remove_dir_all`. The three identity values are therefore checked
//! to be single file-name segments, and `[desktop] icon` to be a project-relative
//! path that stays inside the project, *before* the resolved config exists at
//! all: [`DesktopConfig::resolve`] is the type's only constructor, so no caller
//! in this module can hold an unchecked one.
//!
//! **Reject, never sanitize** — the same contract as
//! `crate::plugin::apply::safe_scaffold_rel_path`. A value that would escape is
//! a typed refusal naming the manifest key ([`DesktopBuildError::UnsafeDesktopIdentity`],
//! [`DesktopBuildError::UnsafeIconPath`]), not a quietly rewritten value: silently
//! turning `../../etc` into `etc` would build a bundle nobody asked for under a
//! name nobody wrote.
//!
//! This is deliberately *not* the same rule as `linux::sanitize`, which strips
//! control characters from a value being written *into* a `.desktop` file to
//! stop key injection. That one guards a file's syntax; this one guards the
//! filesystem.

use std::path::{Component, Path, PathBuf};

use crate::manifest::Manifest;

use super::DesktopBuildError;

/// Everything the per-OS assemblers read, resolved from the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DesktopConfig {
    /// Human-facing name: the `.app` bundle name, `CFBundleName`, the
    /// `.desktop` `Name=`. `[desktop] name`, else `[app] name`.
    pub display_name: String,
    /// Bundle id / `.desktop` file stem / hicolor icon name. `[desktop]
    /// identifier`, else the scaffold's own `org` + `name` derivation.
    pub identifier: String,
    /// The compiled binary's file stem (no `.exe`): the `Cargo.toml`
    /// `[package] name` when readable, else `[app] name`.
    pub binary_name: String,
    /// The `[desktop] icon` source resolved against the project directory, if
    /// configured — always inside `project_dir` (see [`icon_path`]).
    ///
    /// Existence is NOT checked here: a missing or unusable file is a
    /// [`super::BundleNote`], not an error. A path that *tries to leave the
    /// project* is the opposite — a hard refusal, since that is a security
    /// question rather than an icon-quality one.
    pub icon_source: Option<PathBuf>,
    /// `LSMinimumSystemVersion` for a generated `Info.plist`.
    pub macos_minimum_system_version: String,
    /// `[macos] signing-identity`; `Some` enables the codesign step.
    pub macos_signing_identity: Option<String>,
    /// `[macos] notarize`, defaulting to `false` — whether an installer build
    /// may hand `cargo-packager` the Apple credentials that let it notarize
    /// (and upload) the `.app` it signs. Read by
    /// [`super::installer`], which scrubs those credentials from the packaging
    /// tool's environment when this is `false`; no bundle-assembly step looks
    /// at it.
    pub macos_notarize: bool,
    /// `[linux] categories` for a generated desktop entry, defaulting to the
    /// same single `Utility` category the scaffolded entry carries.
    pub linux_categories: Vec<String>,
    /// Whether `[windows]` carries any version override — reported, since the
    /// values a `.exe` ends up with are compiled in by the project's own
    /// `windows/build.rs`.
    pub windows_version_overrides: bool,
}

/// The `Categories=` value a generated desktop entry uses when `[linux]
/// categories` is absent — the same single category `linux/app.desktop.tmpl`
/// renders.
const DEFAULT_LINUX_CATEGORY: &str = "Utility";

/// The manifest keys an identity value can be resolved from. A refusal quotes
/// the key the offending value actually came from, so the message names the
/// line to edit rather than only the value — the fallbacks make that
/// non-obvious (a bad `[app] name` refuses as `[app] name`, not as
/// `[desktop] name`).
const DESKTOP_NAME_KEY: &str = "[desktop] name";
const APP_NAME_KEY: &str = "[app] name";
const DESKTOP_IDENTIFIER_KEY: &str = "[desktop] identifier";
const DERIVED_IDENTIFIER_KEY: &str = "the identifier derived from [app] org/name";
const CARGO_PACKAGE_NAME_KEY: &str = "[package] name in Cargo.toml";

impl DesktopConfig {
    /// Resolves — and path-checks — the desktop identity. The only constructor
    /// this type has, so every `DesktopConfig` in existence has passed the
    /// checks in this module's header.
    pub(super) fn resolve(
        project_dir: &Path,
        manifest: &Manifest,
    ) -> Result<DesktopConfig, DesktopBuildError> {
        let desktop = manifest.desktop.as_ref();
        let (display_name, display_name_key) = match desktop.and_then(|d| d.name.clone()) {
            Some(name) => (name, DESKTOP_NAME_KEY),
            None => (manifest.app.name.clone(), APP_NAME_KEY),
        };
        let (identifier, identifier_key) = match desktop.and_then(|d| d.identifier.clone()) {
            Some(identifier) => (identifier, DESKTOP_IDENTIFIER_KEY),
            None => (
                crate::android_id::derive(&manifest.app.org, &manifest.app.name),
                DERIVED_IDENTIFIER_KEY,
            ),
        };
        let (binary_name, binary_name_key) = binary_name(project_dir, &manifest.app.name);

        // Before anything below turns one of these into a path. Ordered the
        // way a reader meets them in `frust.toml`, so a manifest with two bad
        // values reports the first one.
        check_segment(display_name_key, &display_name)?;
        check_segment(identifier_key, &identifier)?;
        check_segment(binary_name_key, &binary_name)?;

        let icon_source = desktop
            .and_then(|d| d.icon.clone())
            .map(|icon| icon_path(project_dir, &icon))
            .transpose()?;

        let macos = manifest.macos.as_ref();
        let macos_minimum_system_version = macos
            .map(|m| m.minimum_system_version_or_default().to_string())
            .unwrap_or_else(|| {
                crate::manifest::MacosSection::default()
                    .minimum_system_version_or_default()
                    .to_string()
            });

        let linux_categories = manifest
            .linux
            .as_ref()
            .and_then(|l| l.categories.clone())
            .filter(|categories| !categories.is_empty())
            .unwrap_or_else(|| vec![DEFAULT_LINUX_CATEGORY.to_string()]);

        let windows_version_overrides = manifest
            .windows
            .as_ref()
            .is_some_and(|w| w.file_version.is_some() || w.product_version.is_some());

        Ok(DesktopConfig {
            display_name,
            identifier,
            binary_name,
            icon_source,
            macos_minimum_system_version,
            macos_signing_identity: macos.and_then(|m| m.signing_identity.clone()),
            macos_notarize: macos.is_some_and(|m| m.notarize_enabled()),
            linux_categories,
            windows_version_overrides,
        })
    }
}

/// Refuses `value` unless it is a single, self-contained file-name segment.
///
/// The rule, in full: not empty (nor blank), no control character, no `/` or
/// `\` anywhere, and exactly one `Component::Normal` when parsed as a path —
/// which is what rejects `..`, `.`, a leading `/`, and a Windows drive prefix.
///
/// Both separators are refused on every host, not just the native one: a
/// `frust.toml` is committed and built on all three, and `..\..\evil` parses
/// as one harmless-looking `Normal` component on Unix while traversing for
/// real on Windows. Checking the *string* rather than only the parse closes
/// that gap.
fn check_segment(field: &'static str, value: &str) -> Result<(), DesktopBuildError> {
    let refuse = || DesktopBuildError::UnsafeDesktopIdentity {
        field,
        value: value.to_string(),
    };
    if value.trim().is_empty()
        || value.chars().any(char::is_control)
        || value.contains('/')
        || value.contains('\\')
    {
        return Err(refuse());
    }
    let mut components = Path::new(value).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(refuse());
    }
    Ok(())
}

/// Resolves `[desktop] icon` against `project_dir`, refusing anything that
/// could name a file outside it.
///
/// Unlike an identity value this legitimately *is* a path — `assets/icon.png`
/// is the scaffolded value — so subdirectories are fine and only escape is
/// refused: absolute paths, any `..` component, a `\` separator (see
/// [`check_segment`] on why the foreign separator is refused too), control
/// characters, and blank.
///
/// The containment assertion at the end is lexical, on purpose: the icon file
/// need not exist yet (a missing one is a note, not an error), so there is
/// nothing to canonicalize. A symlink *inside* the project pointing out of it
/// is therefore not caught — accepted, because this path is only ever **read**
/// (by the icon pipeline), never written to or deleted, and a symlink in the
/// project tree is content the project's own author placed there.
fn icon_path(project_dir: &Path, icon: &str) -> Result<PathBuf, DesktopBuildError> {
    let refuse = |reason: &'static str| DesktopBuildError::UnsafeIconPath {
        icon: icon.to_string(),
        reason,
    };
    if icon.trim().is_empty() {
        return Err(refuse("is empty"));
    }
    if icon.chars().any(char::is_control) {
        return Err(refuse("contains a control character"));
    }
    if icon.contains('\\') {
        return Err(refuse("contains a `\\` separator (write it with `/`)"));
    }
    let relative = Path::new(icon);
    for component in relative.components() {
        match component {
            Component::ParentDir => return Err(refuse("contains a `..` component")),
            Component::RootDir | Component::Prefix(_) => {
                return Err(refuse("is an absolute path, not a project-relative one"));
            }
            Component::Normal(_) | Component::CurDir => {}
        }
    }
    let resolved = project_dir.join(relative);
    // Belt and braces: the loop above already guarantees this, so a failure
    // here means the rules and the join have drifted apart.
    if !resolved.starts_with(project_dir) {
        return Err(refuse("resolves outside the project directory"));
    }
    Ok(resolved)
}

/// The app crate's binary name: `Cargo.toml`'s `[package] name`, falling back
/// to `fallback` (`[app] name`) whenever the manifest is missing, unreadable,
/// unparseable, or nameless. Returns the key it came from alongside the value,
/// so a refusal can name the right file.
///
/// **Fails soft**, mirroring `crate::cargo_manifest`'s stance: a manifest this
/// can't read is not a reason to fail a build, and for a scaffolded project
/// both values are the same string anyway (`Cargo.toml.tmpl` renders
/// `[package] name` from the project name `[app] name` also carries). Failing
/// soft is about *unreadable* manifests only — a readable one naming an unsafe
/// binary is refused by the caller, never fallen back on.
fn binary_name(project_dir: &Path, fallback: &str) -> (String, &'static str) {
    let fallback = || (fallback.to_string(), APP_NAME_KEY);
    let Ok(text) = std::fs::read_to_string(project_dir.join("Cargo.toml")) else {
        return fallback();
    };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return fallback();
    };
    doc.get("package")
        .and_then(|package| package.as_table_like())
        .and_then(|package| package.get("name"))
        .and_then(|name| name.as_str())
        .map(|name| (name.to_string(), CARGO_PACKAGE_NAME_KEY))
        .unwrap_or_else(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-desktop-config-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn try_resolve(dir: &Path, toml: &str) -> Result<DesktopConfig, DesktopBuildError> {
        DesktopConfig::resolve(dir, &manifest::parse(toml).unwrap())
    }

    fn resolve(dir: &Path, toml: &str) -> DesktopConfig {
        try_resolve(dir, toml).unwrap()
    }

    /// Every string the path-safety tests drive through each identity field:
    /// the shapes a crafted — or merely typo'd — manifest can carry.
    const UNSAFE_VALUES: &[&str] = &[
        "../../etc",
        "..",
        ".",
        "/etc/passwd",
        "..\\..\\windows",
        "sub/dir",
        "a\\b",
        "with\nnewline",
        "with\0nul",
        "",
        "   ",
    ];

    /// Asserts a manifest is refused by [`DesktopConfig::resolve`], naming
    /// `field` and quoting `value` back verbatim (never a sanitized form).
    fn assert_identity_refused(dir: &Path, toml: &str, field: &str, value: &str) {
        match try_resolve(dir, toml) {
            Err(DesktopBuildError::UnsafeDesktopIdentity {
                field: got_field,
                value: got_value,
            }) => {
                assert_eq!(got_field, field, "for value {value:?}");
                assert_eq!(got_value, value, "for value {value:?}");
            }
            other => panic!("expected {value:?} to be refused, got {other:?}"),
        }
    }

    /// The same value, TOML-escaped for embedding in a manifest string.
    fn toml_escape(value: &str) -> String {
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\0', "\\u0000")
    }

    #[test]
    fn every_desktop_value_comes_from_the_manifest_when_present() {
        let dir = temp_dir("full");
        let config = resolve(
            &dir,
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
             [desktop]\nname = \"My App\"\nidentifier = \"com.example.app\"\n\
             icon = \"assets/icon-1024.png\"\n\n\
             [macos]\nminimum-system-version = \"12.0\"\n\
             signing-identity = \"Developer ID Application: Example\"\nnotarize = true\n\n\
             [windows]\nproduct-version = \"1.2.3\"\n\n\
             [linux]\ncategories = [\"Graphics\"]\n",
        );

        assert_eq!(config.display_name, "My App");
        assert_eq!(config.identifier, "com.example.app");
        assert_eq!(config.icon_source, Some(dir.join("assets/icon-1024.png")));
        assert_eq!(config.macos_minimum_system_version, "12.0");
        assert_eq!(
            config.macos_signing_identity.as_deref(),
            Some("Developer ID Application: Example")
        );
        assert!(config.macos_notarize);
        assert_eq!(config.linux_categories, vec!["Graphics".to_string()]);
        assert!(config.windows_version_overrides);

        let _ = fs::remove_dir_all(&dir);
    }

    /// A project predating the desktop templates: nothing but `[app]`.
    #[test]
    fn falls_back_to_the_app_section_and_the_scaffolds_own_derivation() {
        let dir = temp_dir("fallback");
        let config = resolve(&dir, "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n");

        assert_eq!(config.display_name, "my_app");
        assert_eq!(config.identifier, "dev.f0x.my_app");
        assert_eq!(config.binary_name, "my_app");
        assert_eq!(config.icon_source, None);
        assert_eq!(config.macos_minimum_system_version, "11.0");
        assert_eq!(config.macos_signing_identity, None);
        // Notarization is opt-in: a manifest with no `[macos]` section at all
        // resolves to the credential-scrubbing default.
        assert!(!config.macos_notarize);
        assert_eq!(config.linux_categories, vec!["Utility".to_string()]);
        assert!(!config.windows_version_overrides);

        let _ = fs::remove_dir_all(&dir);
    }

    /// The identifier fallback is the *same* derivation `frust create` bakes
    /// into `[desktop] identifier`, so a scaffolded project and a
    /// desktop-section-less one agree.
    #[test]
    fn the_identifier_fallback_matches_the_scaffolds_desktop_identifier() {
        let dir = temp_dir("identifier");
        let config = resolve(&dir, "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n");
        assert_eq!(
            config.identifier,
            crate::android_id::derive("dev.f0x", "my_app")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_binary_name_comes_from_cargo_toml_when_it_differs_from_the_app_name() {
        let dir = temp_dir("bin-name");
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"renamed_app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let config = resolve(&dir, "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n");
        assert_eq!(config.binary_name, "renamed_app");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unparseable_cargo_toml_falls_back_to_the_app_name() {
        let dir = temp_dir("bin-name-broken");
        fs::write(dir.join("Cargo.toml"), "this is not toml {{{").unwrap();
        let config = resolve(&dir, "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n");
        assert_eq!(config.binary_name, "my_app");
        let _ = fs::remove_dir_all(&dir);
    }

    /// `[desktop] name` becomes the `.app` directory's own name, and that
    /// directory is `remove_dir_all`'d on every rebuild — the whole reason
    /// these values are refused rather than sanitized.
    #[test]
    fn a_display_name_that_is_not_a_plain_file_name_is_refused() {
        let dir = temp_dir("unsafe-display-name");
        for value in UNSAFE_VALUES {
            let toml = format!(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nname = \"{}\"\n",
                toml_escape(value)
            );
            assert_identity_refused(&dir, &toml, "[desktop] name", value);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// The identifier names the `.desktop` file and every hicolor icon leaf.
    #[test]
    fn an_identifier_that_is_not_a_plain_file_name_is_refused() {
        let dir = temp_dir("unsafe-identifier");
        for value in UNSAFE_VALUES {
            let toml = format!(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nidentifier = \"{}\"\n",
                toml_escape(value)
            );
            assert_identity_refused(&dir, &toml, "[desktop] identifier", value);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// The binary name is the Linux bundle *directory*, so `Cargo.toml`'s
    /// `[package] name` is in the blast radius too — and the refusal names
    /// that file rather than `frust.toml`.
    #[test]
    fn a_cargo_package_name_that_is_not_a_plain_file_name_is_refused() {
        let dir = temp_dir("unsafe-binary-name");
        let toml = "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n";
        for value in UNSAFE_VALUES {
            fs::write(
                dir.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{}\"\nversion = \"0.1.0\"\n",
                    toml_escape(value)
                ),
            )
            .unwrap();
            // A nameless `[package]` falls back to `[app] name`; an empty
            // string is a *present* name, so it must still be refused.
            assert_identity_refused(&dir, toml, "[package] name in Cargo.toml", value);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// Both values a section-less project falls back to come from `[app]`, so
    /// a traversal there must be refused under the `[app]` key — the fallback
    /// is not an escape hatch around the check.
    #[test]
    fn an_unsafe_app_name_is_refused_through_both_of_its_fallbacks() {
        let dir = temp_dir("unsafe-app-name");
        for value in UNSAFE_VALUES {
            let toml = format!(
                "[app]\nname = \"{}\"\norg = \"dev.f0x\"\n",
                toml_escape(value)
            );
            assert_identity_refused(&dir, &toml, "[app] name", value);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// The derived identifier is checked like any other value, but normally
    /// passes: `android_id::derive` maps every character outside
    /// `[A-Za-z0-9_.]` to `_`, separators included, so a traversal in `[app]
    /// org` cannot survive into a path. (`.` does survive — harmlessly, since
    /// what lands on disk is one `.._.._etc.my_app.desktop` file name.) This
    /// pins the two together: a derivation that let a separator through would
    /// fail here.
    #[test]
    fn a_derived_identifier_survives_an_org_full_of_separators() {
        let dir = temp_dir("derived-identifier");
        let config = resolve(
            &dir,
            "[app]\nname = \"my_app\"\norg = \"../../etc\"\n\n[desktop]\nname = \"My App\"\n",
        );
        assert_eq!(config.identifier, ".._.._etc.my_app");
        let _ = fs::remove_dir_all(&dir);
    }

    /// …but the check is not decoration: a derivation that lands on a bare
    /// `.` is refused, under the key that produced it rather than under the
    /// `[desktop] identifier` nobody wrote.
    #[test]
    fn a_derived_identifier_that_lands_on_a_current_dir_is_refused() {
        let dir = temp_dir("derived-current-dir");
        // A binary name from `Cargo.toml` and an explicit `[desktop] name`, so
        // the empty `[app]` values reach nothing but the derivation.
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"my_app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert_identity_refused(
            &dir,
            "[app]\nname = \"\"\norg = \"\"\n\n[desktop]\nname = \"My App\"\n",
            "the identifier derived from [app] org/name",
            ".",
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// `[desktop] icon` is a real path (`assets/icon.png`), so subdirectories
    /// are fine — only leaving the project is refused.
    #[test]
    fn an_icon_path_that_escapes_the_project_is_refused() {
        let dir = temp_dir("unsafe-icon");
        for value in [
            "../../etc/shadow",
            "/etc/passwd",
            "..\\..\\windows\\system32",
            "assets/../../outside.png",
            "..",
            "icon\n.png",
            "",
            "   ",
        ] {
            let toml = format!(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nicon = \"{}\"\n",
                toml_escape(value)
            );
            match try_resolve(&dir, &toml) {
                Err(DesktopBuildError::UnsafeIconPath { icon, .. }) => {
                    assert_eq!(icon, value)
                }
                other => panic!("expected {value:?} to be refused, got {other:?}"),
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_relative_icon_path_resolves_inside_the_project() {
        let dir = temp_dir("safe-icon");
        for value in ["assets/icon.png", "./assets/icon.png", "icon.png"] {
            let config = resolve(
                &dir,
                &format!(
                    "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                     [desktop]\nicon = \"{value}\"\n"
                ),
            );
            let icon = config.icon_source.expect("a configured icon resolves");
            assert!(icon.starts_with(&dir), "{}", icon.display());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// The refusal has to be actionable: it names the manifest key and quotes
    /// the offending value back unchanged.
    #[test]
    fn a_refusal_names_the_manifest_key_and_the_value() {
        let dir = temp_dir("refusal-message");
        let err = try_resolve(
            &dir,
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
             [desktop]\nname = \"../../../home/x\"\n",
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("[desktop] name"), "{message}");
        assert!(message.contains("../../../home/x"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_categories_list_falls_back_to_the_default_category() {
        let dir = temp_dir("empty-categories");
        let config = resolve(
            &dir,
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n[linux]\ncategories = []\n",
        );
        assert_eq!(config.linux_categories, vec!["Utility".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }
}
