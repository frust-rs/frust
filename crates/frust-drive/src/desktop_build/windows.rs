//! Windows output-directory assembly.
//!
//! Layout (the shared desktop-bundle contract):
//!
//! ```text
//! build/desktop/windows/<binary>.exe
//! ```
//!
//! Flat by design: Windows has no bundle format, so the "bundle" is the
//! release `.exe` in a directory an installer step (or a zip) can pick up.
//!
//! **The icon is not embedded here.** The project's own `windows/build.rs`
//! embeds `build/desktop/windows/icon.ico` into the `.exe` through
//! `winresource` at compile time, so this module's icon job is to *generate
//! that file first* ([`generate_exe_icon`], called before the compile) and
//! then get out of the way. A project with no usable icon source still
//! builds — `build.rs` degrades to a cargo warning when the file isn't there.
//!
//! `[windows] file-version`/`product-version` are likewise compiled in by that
//! same `build.rs`; nothing this pipeline writes can change what the `.exe`
//! carries, so a configured override is reported
//! ([`BundleNote::WindowsVersionOverridesNotApplied`]) rather than silently
//! dropped.

use std::fs;
use std::path::{Path, PathBuf};

use crate::build_dirs::BuildLayout;
use crate::icons;

use super::bundle::{copy_file, icon_source, record_icons};
use super::cargo::binary_file_name;
use super::config::DesktopConfig;
use super::{BundleNote, BundleReport, DesktopBuildError, DesktopBundleTarget};

/// The path literal the *pre-`build/`-layout* `windows/build.rs` template
/// embedded (`git show 12f9b29e:templates/app/windows.tmpl/build.rs.tmpl`),
/// relative to the project root rather than under `build/desktop/windows`.
const LEGACY_ICON_LITERAL: &str = "windows/icon.ico";

/// The path literal the current template embeds — checked first so a
/// project already on the new layout is never mistaken for a legacy one
/// (`LEGACY_ICON_LITERAL` is a suffix of this one).
const CURRENT_ICON_LITERAL: &str = "build/desktop/windows/icon.ico";

/// Generates `<project>/build/desktop/windows/icon.ico` from `[desktop]
/// icon` so the project's `windows/build.rs` finds it during the compile
/// that follows. Returns the generated path, or `None` when there is no
/// usable icon source — with the reason recorded as a [`BundleNote`].
/// Infallible by design, like every other icon step here: a source the
/// pipeline can't use downgrades the `.exe` to no embedded icon, it does not
/// fail the build.
///
/// Runs before the compile (see `super::build_with_contributions`), writing
/// straight into `build/desktop/windows` without clearing it first — that
/// directory may still hold a previous, still-good bundle, and it must
/// survive a compile that goes on to fail. The directory is only cleared,
/// via `bundle::prepare_dir_keeping`, once the compile that reads this icon
/// has actually succeeded — that call runs beside `assemble`, not here.
/// `icons::generate_ico` creates its own parent directory regardless, so
/// nothing extra is needed in this function either way.
///
/// A project whose `windows/build.rs` predates the `build/` layout (still
/// embeds the legacy `windows/icon.ico` path rather than
/// `build/desktop/windows/icon.ico` — there is no `frust upgrade` to
/// regenerate it) also gets this icon mirrored to that legacy path; see
/// [`mirror_legacy_icon`].
pub(super) fn generate_exe_icon(
    project_dir: &Path,
    config: &DesktopConfig,
    notes: &mut Vec<BundleNote>,
) -> Option<PathBuf> {
    let source = icon_source(config, notes)?;
    let ico = project_dir.join(BuildLayout::windows_icon());
    let generated = icons::generate_ico(&source, &ico);
    let ico = record_icons(&source, generated, notes).into_iter().next()?;
    mirror_legacy_icon(project_dir, &ico, notes);
    Some(ico)
}

/// One-release compatibility for a project scaffolded before the `build/`
/// layout: its `windows/build.rs` was rendered from the old template and
/// still reads the icon from `windows/icon.ico` at the project root, not
/// `build/desktop/windows/icon.ico`. The project's own files are never
/// rewritten by this pipeline (this module's contract, and the wider
/// pipeline's — see `super`'s module doc's "The project's own files win"),
/// so instead of patching `build.rs` this copies the freshly generated icon
/// to the legacy path the stale `build.rs` actually reads — mirroring
/// `android_build`'s read-fallback pattern for a project scaffolded before a
/// template change — and records [`BundleNote::LegacyWindowsIconAlsoWritten`].
///
/// A no-op when `windows/build.rs` doesn't exist, already names the current
/// path, or never mentioned an icon at all (a hand-written `build.rs`).
/// Infallible like the rest of this module's icon handling: a copy that
/// fails leaves the primary icon (already written) untouched and simply
/// skips the note, rather than failing the build.
fn mirror_legacy_icon(project_dir: &Path, generated_ico: &Path, notes: &mut Vec<BundleNote>) {
    let build_rs = project_dir.join("windows").join("build.rs");
    let Ok(contents) = fs::read_to_string(&build_rs) else {
        return;
    };
    if contents.contains(CURRENT_ICON_LITERAL) || !contents.contains(LEGACY_ICON_LITERAL) {
        return;
    }
    let legacy_ico = project_dir.join(LEGACY_ICON_LITERAL);
    if fs::copy(generated_ico, &legacy_ico).is_ok() {
        notes.push(BundleNote::LegacyWindowsIconAlsoWritten);
    }
}

/// Assembles `build/desktop/windows/` around the compiled `binary`. `icon` is
/// whatever [`generate_exe_icon`] produced before the compile, recorded as an
/// artifact.
///
/// **Does not `prepare_dir` its own root.** By the time this runs, the
/// caller (`super::build_with_contributions`) has already cleared this
/// directory of any previous run's stale content — via
/// `bundle::prepare_dir_keeping`, called right before this function, once the
/// compile has succeeded — while preserving the icon that compile just
/// embedded. This function only adds the executable (the icon is already
/// there to report as an artifact); `copy_file` still creates the directory
/// if it somehow isn't there.
pub(super) fn assemble(
    project_dir: &Path,
    config: &DesktopConfig,
    binary: &Path,
    icon: Option<PathBuf>,
    notes: &mut Vec<BundleNote>,
) -> Result<BundleReport, DesktopBuildError> {
    let root = DesktopBundleTarget::Windows.output_dir(project_dir);

    let executable = root.join(binary_file_name(
        DesktopBundleTarget::Windows,
        &config.binary_name,
    ));
    copy_file(binary, &executable)?;

    let mut artifacts = vec![executable.clone()];
    artifacts.extend(icon);

    if config.windows_version_overrides {
        notes.push(BundleNote::WindowsVersionOverridesNotApplied);
    }

    Ok(BundleReport {
        target: DesktopBundleTarget::Windows,
        root,
        executable,
        artifacts,
        // Filled once by the caller: entitlements are a macOS-only concept
        // (always `None` here), notes keep accruing after this assembly.
        entitlements: None,
        notes: Vec::new(),
    })
}
