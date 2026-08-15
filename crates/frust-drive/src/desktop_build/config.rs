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

use std::path::{Path, PathBuf};

use crate::manifest::Manifest;

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
    /// Absolute path to the `[desktop] icon` source, if configured. Existence
    /// is NOT checked here — a missing file is a note, not an error.
    pub icon_source: Option<PathBuf>,
    /// `LSMinimumSystemVersion` for a generated `Info.plist`.
    pub macos_minimum_system_version: String,
    /// `[macos] signing-identity`; `Some` enables the codesign step.
    pub macos_signing_identity: Option<String>,
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

impl DesktopConfig {
    pub(super) fn resolve(project_dir: &Path, manifest: &Manifest) -> DesktopConfig {
        let desktop = manifest.desktop.as_ref();
        let display_name = desktop
            .and_then(|d| d.name.clone())
            .unwrap_or_else(|| manifest.app.name.clone());
        let identifier = desktop
            .and_then(|d| d.identifier.clone())
            .unwrap_or_else(|| crate::android_id::derive(&manifest.app.org, &manifest.app.name));
        let icon_source = desktop
            .and_then(|d| d.icon.clone())
            .map(|icon| project_dir.join(icon));

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

        DesktopConfig {
            display_name,
            identifier,
            binary_name: binary_name(project_dir, &manifest.app.name),
            icon_source,
            macos_minimum_system_version,
            macos_signing_identity: macos.and_then(|m| m.signing_identity.clone()),
            linux_categories,
            windows_version_overrides,
        }
    }
}

/// The app crate's binary name: `Cargo.toml`'s `[package] name`, falling back
/// to `fallback` (`[app] name`) whenever the manifest is missing, unreadable,
/// unparseable, or nameless.
///
/// **Fails soft**, mirroring `crate::cargo_manifest`'s stance: a manifest this
/// can't read is not a reason to fail a build, and for a scaffolded project
/// both values are the same string anyway (`Cargo.toml.tmpl` renders
/// `[package] name` from the project name `[app] name` also carries).
fn binary_name(project_dir: &Path, fallback: &str) -> String {
    let Ok(text) = std::fs::read_to_string(project_dir.join("Cargo.toml")) else {
        return fallback.to_string();
    };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return fallback.to_string();
    };
    doc.get("package")
        .and_then(|package| package.as_table_like())
        .and_then(|package| package.get("name"))
        .and_then(|name| name.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
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

    fn resolve(dir: &Path, toml: &str) -> DesktopConfig {
        DesktopConfig::resolve(dir, &manifest::parse(toml).unwrap())
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
             signing-identity = \"Developer ID Application: Example\"\n\n\
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
