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
//!
//! A copied entry is then [`reconcile`]d against what this run actually laid
//! out, because the two are written from different sources: the entry says
//! whatever a hand edit left in it, while the executable and the icon leaves
//! are named from the resolved manifest. Both findings are
//! [`BundleNote`]s — never a refusal, unlike the macOS `CFBundleExecutable`
//! case, because a `.desktop` `Exec=` is a *command line*, not a file
//! reference: a bare name resolved through `PATH`, an installed absolute path
//! and trailing `%f`/`%u` placeholders are all legitimate, so a difference is
//! reported for a human to judge rather than acted on.

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
    prepare_dir(&root, project_dir)?;

    let mut artifacts = Vec::new();

    let executable = root.join(&config.binary_name);
    copy_file(binary, &executable)?;
    artifacts.push(executable.clone());

    // False unless icon leaves were really written this run: `record_icons`
    // yields no paths for a source the icon pipeline rejected, and there is
    // already a note saying why.
    let mut icon_generated = false;
    if let Some(source) = icon_source(config, notes) {
        let theme_root = root.join("share").join("icons").join("hicolor");
        let generated = icons::generate_hicolor_set(&source, &theme_root);
        for path in record_icons(&source, generated, notes) {
            artifacts.push(rename_icon_leaf(&path, &config.identifier)?);
            icon_generated = true;
        }
    }

    let entry = root.join(format!("{}.desktop", config.identifier));
    let project_entry = project_dir.join("linux").join("app.desktop");
    if project_entry.is_file() {
        copy_file(&project_entry, &entry)?;
        // A generated entry is built from `config` by construction and has
        // nothing to reconcile; only the copied one can disagree. An entry
        // that isn't UTF-8 isn't a valid desktop entry either — nothing to
        // compare, and not this pipeline's refusal to make.
        if let Ok(text) = fs::read_to_string(&entry) {
            reconcile(&text, config, icon_generated, notes);
        }
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

/// The group a desktop entry's own keys live in; a `[Desktop Action …]` group
/// carries an `Exec=` of its own that describes a secondary action, not the
/// application's own launch command.
const DESKTOP_ENTRY_GROUP: &str = "Desktop Entry";

/// Where an icon-name mismatch was declared, for
/// [`BundleNote::IconIdentityMismatch`]'s message.
const ICON_KEY_SOURCE: &str = "`linux/app.desktop`'s Icon=";

/// The extension an `Icon=` key may or may not carry — freedesktop resolves an
/// extensionless icon *name* through the theme, and the leaves this module
/// writes are `.png`, so both spellings compare equal.
const PNG_EXTENSION: &str = ".png";

/// Compares the copied entry's `Exec=`/`Icon=` with what this run laid out,
/// recording a [`BundleNote`] per disagreement (see this module's header for
/// why neither is a refusal).
///
/// `icon_generated` gates the `Icon=` comparison: with the icon steps skipped
/// there is no file to be named wrongly, and a note about it would only repeat
/// the icon note already recorded.
fn reconcile(
    text: &str,
    config: &DesktopConfig,
    icon_generated: bool,
    notes: &mut Vec<BundleNote>,
) {
    if let Some(exec) = entry_value(text, "Exec")
        && let Some(program) = exec_program(exec)
        && program != config.binary_name
    {
        notes.push(BundleNote::ExecIdentityMismatch {
            exec: exec.to_string(),
            binary: config.binary_name.clone(),
        });
    }

    if !icon_generated {
        return;
    }
    // An `Icon=` naming an absolute path points at something installed
    // elsewhere on the system rather than at this bundle's theme tree; there
    // is nothing this run wrote to compare it against.
    if let Some(icon) = entry_value(text, "Icon")
        && !icon.contains('/')
        && icon.strip_suffix(PNG_EXTENSION).unwrap_or(icon) != config.identifier
    {
        notes.push(BundleNote::IconIdentityMismatch {
            declared_in: ICON_KEY_SOURCE,
            declared: icon.to_string(),
            generated: format!("{}{PNG_EXTENSION}", config.identifier),
        });
    }
}

/// Reads one `[Desktop Entry]` key out of a desktop file, line-wise: blank
/// lines and `#` comments are skipped, a `[…]` line switches groups, and the
/// first `key=value` match inside the entry group wins.
///
/// Deliberately a handful of lines rather than a dependency — this reads two
/// keys for a warning, and the format's own spec is `key=value` per line.
/// Localized spellings (`Name[de]=`) are not matched, which is correct for the
/// two keys read here: neither `Exec` nor `Icon` is localizable in practice,
/// and a localized variant is not the value a launcher would run.
fn entry_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(group) = line.strip_prefix('[') {
            in_entry = group.trim_end_matches(']').trim() == DESKTOP_ENTRY_GROUP;
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some((name, value)) = line.split_once('=')
            && name.trim() == key
        {
            return Some(value.trim());
        }
    }
    None
}

/// The program an `Exec=` value runs: its leading token, reduced to a file
/// name, or `None` when there is nothing to compare (empty, or a leading
/// `%f`-style field code).
///
/// The leading token ends at the first whitespace, or — when the value opens
/// with the quote the desktop-entry spec requires around an argument
/// containing spaces — at the closing quote. An escaped `\"` inside a quoted
/// program name is not handled: it would end the token early and at worst
/// costs a spurious note, which is the failure direction to prefer here.
///
/// Reducing to the file name is what makes `Exec=/usr/bin/my_app` agree with a
/// binary named `my_app` — an installed absolute path is the same program, not
/// a drift worth reporting.
fn exec_program(exec: &str) -> Option<&str> {
    let exec = exec.trim();
    let token = match exec.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => exec.split_whitespace().next()?,
    };
    if token.is_empty() || token.starts_with('%') {
        return None;
    }
    Path::new(token).file_name()?.to_str()
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
        .unwrap()
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

    /// A value carrying a newline must not be able to append a second key to
    /// the entry. `[linux] categories` is the live vector: unlike the identity
    /// values, a category is not a path, so `DesktopConfig::resolve` has no
    /// reason to refuse it and [`sanitize`] is the only thing standing between
    /// it and an injected `Exec=`.
    #[test]
    fn a_multiline_category_cannot_inject_a_second_key() {
        let entry = desktop_entry(&config(
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
             [linux]\ncategories = [\"Evil\\nExec=/bin/sh\"]\n",
        ));
        assert!(
            entry.contains("\nCategories=EvilExec=/bin/sh;\n"),
            "{entry}"
        );
        assert_eq!(
            entry.lines().filter(|l| l.starts_with("Exec=")).count(),
            1,
            "{entry}"
        );
    }

    /// The entry a scaffolded project ships, against the identity the default
    /// manifest resolves (binary `my_app`, identifier `dev.f0x.my_app`).
    fn scaffolded_entry(exec: &str, icon: &str) -> String {
        format!(
            "# A comment, and a blank line below.\n\n\
             [Desktop Entry]\nType=Application\nName=My App\n\
             Exec={exec}\nIcon={icon}\nCategories=Utility;\nTerminal=false\n"
        )
    }

    fn default_config() -> DesktopConfig {
        config("[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n[desktop]\nname = \"My App\"\n")
    }

    fn reconcile_notes(text: &str, icon_generated: bool) -> Vec<BundleNote> {
        let mut notes = Vec::new();
        reconcile(text, &default_config(), icon_generated, &mut notes);
        notes
    }

    /// Every `Exec=` shape that still names this bundle's own program: a bare
    /// name resolved through `PATH`, the same name with freedesktop's field
    /// codes, an installed absolute path, and a quoted one.
    #[test]
    fn an_exec_line_naming_the_bundled_binary_records_nothing() {
        for exec in [
            "my_app",
            "my_app %f",
            "my_app %U --flag",
            "/usr/bin/my_app",
            "\"/opt/My App/my_app\" %u",
        ] {
            let notes = reconcile_notes(&scaffolded_entry(exec, "dev.f0x.my_app"), true);
            assert!(notes.is_empty(), "{exec}: {notes:?}");
        }
    }

    /// The drift this check exists for: the binary was renamed (in
    /// `Cargo.toml`) and the hand-editable entry wasn't. A note, not a
    /// refusal — `renamed_app` may well be on the user's `PATH`.
    #[test]
    fn an_exec_line_naming_another_program_is_a_note() {
        let notes = reconcile_notes(&scaffolded_entry("renamed_app %f", "dev.f0x.my_app"), true);
        assert_eq!(
            notes,
            vec![BundleNote::ExecIdentityMismatch {
                exec: "renamed_app %f".to_string(),
                binary: "my_app".to_string(),
            }]
        );
        assert!(notes[0].to_string().contains("PATH"), "{:?}", notes[0]);
    }

    #[test]
    fn an_icon_line_naming_another_icon_is_a_note() {
        let notes = reconcile_notes(&scaffolded_entry("my_app", "com.example.other"), true);
        assert_eq!(
            notes,
            vec![BundleNote::IconIdentityMismatch {
                declared_in: ICON_KEY_SOURCE,
                declared: "com.example.other".to_string(),
                generated: "dev.f0x.my_app.png".to_string(),
            }]
        );
    }

    /// Freedesktop resolves an extensionless icon *name* through the theme,
    /// and an absolute path is something installed elsewhere entirely —
    /// neither is a mismatch with the tree this run wrote.
    #[test]
    fn an_icon_line_matches_with_or_without_the_png_extension_and_skips_paths() {
        for icon in [
            "dev.f0x.my_app",
            "dev.f0x.my_app.png",
            "/usr/share/pixmaps/whatever.png",
        ] {
            let notes = reconcile_notes(&scaffolded_entry("my_app", icon), true);
            assert!(notes.is_empty(), "{icon}: {notes:?}");
        }
    }

    /// Nothing is compared against a file this run didn't write: with the icon
    /// steps skipped (no source, or one the pipeline rejected — both already
    /// noted), a stale `Icon=` says nothing new.
    #[test]
    fn the_icon_line_is_not_compared_when_no_icon_was_generated() {
        let notes = reconcile_notes(&scaffolded_entry("my_app", "com.example.other"), false);
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// An entry carrying neither key is nothing to reconcile — a desktop entry
    /// without `Exec=` is another tool's problem, not a drift report.
    #[test]
    fn an_entry_missing_both_keys_records_nothing() {
        let notes = reconcile_notes(
            "[Desktop Entry]\nType=Application\nName=My App\nTerminal=false\n",
            true,
        );
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// A `[Desktop Action …]` group's `Exec=` describes a secondary action,
    /// not the application's launch command, so only the `[Desktop Entry]`
    /// group's keys are read.
    #[test]
    fn keys_outside_the_desktop_entry_group_are_ignored() {
        let text = "[Desktop Entry]\nExec=my_app\nIcon=dev.f0x.my_app\nActions=new;\n\n\
                    [Desktop Action new]\nName=New Window\nExec=some_other_thing --new\n\
                    Icon=some-other-icon\n";
        assert!(reconcile_notes(text, true).is_empty());
        assert_eq!(entry_value(text, "Exec"), Some("my_app"));
    }

    /// Comments, blank lines and surrounding whitespace are not values.
    #[test]
    fn commented_and_blank_lines_are_skipped_when_reading_a_key() {
        let text = "\n# Exec=commented_out\n[Desktop Entry]\n\n  Exec = my_app %f  \n";
        assert_eq!(entry_value(text, "Exec"), Some("my_app %f"));
        assert_eq!(entry_value(text, "Icon"), None);
    }

    /// A value that is only a field code names no program to compare.
    #[test]
    fn an_exec_value_with_nothing_to_compare_yields_no_program() {
        assert_eq!(exec_program(""), None);
        assert_eq!(exec_program("%f"), None);
        assert_eq!(exec_program("my_app"), Some("my_app"));
    }

    /// The same protection still covers the identity values, even though
    /// `DesktopConfig::resolve` now refuses a display name carrying a control
    /// character before one can reach here (hence the hand-built config): the
    /// two layers answer different questions — resolve asks whether a value is
    /// a safe *path*, [`sanitize`] whether it is a safe *desktop-entry value* —
    /// and neither is a substitute for the other.
    #[test]
    fn a_multiline_display_name_cannot_inject_a_second_key() {
        let mut config = config("[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n");
        config.display_name = "Evil\nExec=/bin/sh".to_string();
        let entry = desktop_entry(&config);
        assert!(entry.contains("\nName=EvilExec=/bin/sh\n"), "{entry}");
        assert_eq!(
            entry.lines().filter(|l| l.starts_with("Exec=")).count(),
            1,
            "{entry}"
        );
    }
}
