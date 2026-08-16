//! Windows dist-directory assembly.
//!
//! Layout (the shared desktop-bundle contract):
//!
//! ```text
//! dist/windows/<binary>.exe
//! ```
//!
//! Flat by design: Windows has no bundle format, so the "bundle" is the
//! release `.exe` in a directory an installer step (or a zip) can pick up.
//!
//! **The icon is not embedded here.** The project's own `windows/build.rs`
//! embeds `windows/icon.ico` into the `.exe` through `winresource` at compile
//! time, so this module's icon job is to *generate that file first*
//! ([`generate_exe_icon`], called before the compile) and then get out of the
//! way. A project with no usable icon source still builds — `build.rs`
//! degrades to a cargo warning when the file isn't there.
//!
//! `[windows] file-version`/`product-version` are likewise compiled in by that
//! same `build.rs`; nothing this pipeline writes can change what the `.exe`
//! carries, so a configured override is reported
//! ([`BundleNote::WindowsVersionOverridesNotApplied`]) rather than silently
//! dropped.

use std::path::{Path, PathBuf};

use crate::icons;

use super::bundle::{copy_file, icon_source, prepare_dir, record_icons};
use super::cargo::binary_file_name;
use super::config::DesktopConfig;
use super::{BundleNote, BundleReport, DesktopBuildError, DesktopBundleTarget};

/// Generates `<project>/windows/icon.ico` from `[desktop] icon` so the
/// project's `windows/build.rs` finds it during the compile that follows.
/// Returns the generated path (an artifact of this build, even though it
/// lands in the project rather than in `dist/`), or `None` when there is no
/// usable icon source — with the reason recorded as a [`BundleNote`].
/// Infallible by design, like every other icon step here: a source the
/// pipeline can't use downgrades the `.exe` to no embedded icon, it does not
/// fail the build.
pub(super) fn generate_exe_icon(
    project_dir: &Path,
    config: &DesktopConfig,
    notes: &mut Vec<BundleNote>,
) -> Option<PathBuf> {
    let source = icon_source(config, notes)?;
    let ico = project_dir.join("windows").join("icon.ico");
    let generated = icons::generate_ico(&source, &ico);
    record_icons(&source, generated, notes).into_iter().next()
}

/// Assembles `dist/windows/` around the compiled `binary`. `icon` is whatever
/// [`generate_exe_icon`] produced before the compile, recorded as an artifact.
pub(super) fn assemble(
    project_dir: &Path,
    config: &DesktopConfig,
    binary: &Path,
    icon: Option<PathBuf>,
    notes: &mut Vec<BundleNote>,
) -> Result<BundleReport, DesktopBuildError> {
    let root = DesktopBundleTarget::Windows.dist_dir(project_dir);
    prepare_dir(&root, project_dir)?;

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
