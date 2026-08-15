//! Linux bundle-directory assembly.
//!
//! Layout (the shared desktop-bundle contract):
//!
//! ```text
//! dist/linux/<binary>/
//!   <binary>
//!   <identifier>.desktop
//!   share/icons/hicolor/<N>x<N>/apps/<identifier>.png
//! ```
//!
//! Everything is named the way a freedesktop install expects it, so the
//! directory can be copied into a prefix (or handed to a packaging tool)
//! without renaming anything: the desktop entry's file name is the
//! application id, and each icon file is named after the entry's `Icon=` key —
//! which is that same id, both in the scaffolded `linux/app.desktop` and in
//! the generated fallback below. The icon pipeline emits a fixed `icon.png`
//! leaf name (it takes no app name), so this module renames each leaf into
//! place as it assembles.
//!
//! The project's own `linux/app.desktop` is copied verbatim when present; a
//! project scaffolded before that template existed gets a minimal entry
//! generated from `frust.toml` (`[desktop] name`/`identifier`, `[linux]
//! categories`) instead of a failed build.

use std::fs;
use std::path::{Path, PathBuf};

use crate::icons;

use super::bundle::{copy_file, icon_source, prepare_dir, record_icons, write_file};
use super::config::DesktopConfig;
use super::{BundleNote, BundleReport, DesktopBuildError, DesktopBundleTarget};

/// Assembles `dist/linux/<binary>/` around the compiled `binary`.
pub(super) fn assemble(
    project_dir: &Path,
    config: &DesktopConfig,
    binary: &Path,
    notes: &mut Vec<BundleNote>,
) -> Result<BundleReport, DesktopBuildError> {
    let root = DesktopBundleTarget::Linux
        .dist_dir(project_dir)
        .join(&config.binary_name);
    prepare_dir(&root)?;

    let mut artifacts = Vec::new();

    let executable = root.join(&config.binary_name);
    copy_file(binary, &executable)?;
    artifacts.push(executable.clone());

    if let Some(source) = icon_source(config, notes) {
        let theme_root = root.join("share").join("icons").join("hicolor");
        let generated = icons::generate_hicolor_set(&source, &theme_root);
        for path in record_icons(&source, generated, notes) {
            artifacts.push(rename_icon_leaf(&path, &config.identifier)?);
        }
    }

    let entry = root.join(format!("{}.desktop", config.identifier));
    let project_entry = project_dir.join("linux").join("app.desktop");
    if project_entry.is_file() {
        copy_file(&project_entry, &entry)?;
    } else {
        notes.push(BundleNote::GeneratedDesktopEntry);
        write_file(&entry, &desktop_entry(config))?;
    }
    artifacts.push(entry);

    Ok(BundleReport {
        target: DesktopBundleTarget::Linux,
        root,
        executable,
        artifacts,
        notes: Vec::new(),
    })
}

/// Renames a generated `<size>/apps/icon.png` leaf to `<identifier>.png`, so
/// the tree matches the desktop entry's `Icon=` key.
fn rename_icon_leaf(path: &Path, identifier: &str) -> Result<PathBuf, DesktopBuildError> {
    let dest = path.with_file_name(format!("{identifier}.png"));
    fs::rename(path, &dest).map_err(|source| DesktopBuildError::Io {
        action: "naming the icon",
        path: dest.clone(),
        source,
    })?;
    Ok(dest)
}

/// The minimal desktop entry generated for a project with no
/// `linux/app.desktop` of its own — the same keys the scaffolded template
/// carries, with `Categories=` taken from `[linux] categories`.
fn desktop_entry(config: &DesktopConfig) -> String {
    let categories: String = config
        .linux_categories
        .iter()
        .map(|category| format!("{};", sanitize(category)))
        .collect();
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name={name}\n\
         Exec={binary}\n\
         Icon={identifier}\n\
         Categories={categories}\n\
         Terminal=false\n\
         StartupWMClass={binary}\n",
        name = sanitize(&config.display_name),
        binary = sanitize(&config.binary_name),
        identifier = sanitize(&config.identifier),
    )
}

/// A desktop-entry value is a single line: strip anything that would end it
/// early (or inject a second key), since the display name is free-form user
/// text from `frust.toml`.
fn sanitize(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;

    fn config(toml: &str) -> DesktopConfig {
        DesktopConfig::resolve(
            Path::new("/projects/my_app"),
            &manifest::parse(toml).unwrap(),
        )
    }

    #[test]
    fn the_generated_entry_uses_the_manifests_name_identifier_and_categories() {
        let entry = desktop_entry(&config(
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
             [desktop]\nname = \"My App\"\nidentifier = \"com.example.app\"\n\n\
             [linux]\ncategories = [\"Graphics\", \"Utility\"]\n",
        ));
        assert!(entry.starts_with("[Desktop Entry]\n"), "{entry}");
        assert!(entry.contains("\nName=My App\n"), "{entry}");
        assert!(entry.contains("\nExec=my_app\n"), "{entry}");
        assert!(entry.contains("\nIcon=com.example.app\n"), "{entry}");
        assert!(
            entry.contains("\nCategories=Graphics;Utility;\n"),
            "{entry}"
        );
        assert!(entry.contains("\nStartupWMClass=my_app\n"), "{entry}");
    }

    #[test]
    fn the_default_category_matches_the_scaffolded_entry() {
        let entry = desktop_entry(&config("[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n"));
        assert!(entry.contains("\nCategories=Utility;\n"), "{entry}");
    }

    /// A display name carrying a newline must not be able to append a second
    /// key to the entry.
    #[test]
    fn a_multiline_display_name_cannot_inject_a_second_key() {
        let entry = desktop_entry(&config(
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
             [desktop]\nname = \"Evil\\nExec=/bin/sh\"\n",
        ));
        assert!(entry.contains("\nName=EvilExec=/bin/sh\n"), "{entry}");
        assert_eq!(
            entry.lines().filter(|l| l.starts_with("Exec=")).count(),
            1,
            "{entry}"
        );
    }
}
