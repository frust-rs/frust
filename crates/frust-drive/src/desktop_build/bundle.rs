//! Assembly primitives shared by the three per-OS layouts: the filesystem
//! helpers that turn an I/O failure into a typed [`DesktopBuildError::Io`]
//! naming what was being attempted, and the icon-source resolution every
//! layout starts from.
//!
//! The icon rules live here because all three targets share them: an icon that
//! is unconfigured, absent, or rejected by the icon pipeline is recorded as a
//! [`BundleNote`] and skipped — never a failed build. A bundle without an icon
//! still launches, and the placeholder logo `frust create` ships is smaller
//! than the icon pipeline's minimum source size, so the strict reading would
//! fail every fresh project's first desktop build.

use std::fs;
use std::path::{Path, PathBuf};

use crate::icons::{IconReport, SourceWarning};

use super::config::DesktopConfig;
use super::{BundleNote, DesktopBuildError};

/// Creates `dir` fresh: an existing directory is removed first, so a rebuild
/// can never leave a previous run's file (a renamed icon, a dropped launcher
/// entry) inside the bundle it hands back.
pub(super) fn prepare_dir(dir: &Path) -> Result<(), DesktopBuildError> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|source| DesktopBuildError::Io {
            action: "removing the previous bundle at",
            path: dir.to_path_buf(),
            source,
        })?;
    }
    create_dir(dir)
}

/// `mkdir -p`.
pub(super) fn create_dir(dir: &Path) -> Result<(), DesktopBuildError> {
    fs::create_dir_all(dir).map_err(|source| DesktopBuildError::Io {
        action: "creating directory",
        path: dir.to_path_buf(),
        source,
    })
}

/// Copies `from` to `to`, creating `to`'s parent directories. On Unix this
/// carries the source's permission bits over, which is what keeps a copied
/// binary executable.
pub(super) fn copy_file(from: &Path, to: &Path) -> Result<(), DesktopBuildError> {
    if let Some(parent) = to.parent() {
        create_dir(parent)?;
    }
    fs::copy(from, to).map_err(|source| DesktopBuildError::Io {
        action: "copying into the bundle",
        path: from.to_path_buf(),
        source,
    })?;
    Ok(())
}

/// Writes `contents` to `path`, creating its parent directories.
pub(super) fn write_file(path: &Path, contents: &str) -> Result<(), DesktopBuildError> {
    if let Some(parent) = path.parent() {
        create_dir(parent)?;
    }
    fs::write(path, contents).map_err(|source| DesktopBuildError::Io {
        action: "writing",
        path: path.to_path_buf(),
        source,
    })
}

/// The usable icon source for this project, or `None` with a [`BundleNote`]
/// recorded explaining why there is none (unconfigured, or configured at a
/// path that doesn't exist).
pub(super) fn icon_source(config: &DesktopConfig, notes: &mut Vec<BundleNote>) -> Option<PathBuf> {
    let Some(path) = config.icon_source.clone() else {
        notes.push(BundleNote::IconNotConfigured);
        return None;
    };
    if !path.is_file() {
        notes.push(BundleNote::IconSourceMissing { path });
        return None;
    }
    Some(path)
}

/// Folds an icon-generation result into the report: the generated paths on
/// success (plus any source-quality note), or a [`BundleNote::IconUnusable`]
/// and no paths on failure.
///
/// Deliberately infallible — see this module's doc: an icon the pipeline
/// rejects downgrades the bundle, it does not fail the build.
pub(super) fn record_icons(
    source: &Path,
    result: Result<IconReport, crate::icons::IconError>,
    notes: &mut Vec<BundleNote>,
) -> Vec<PathBuf> {
    match result {
        Ok(report) => {
            if let Some(SourceWarning::BelowRecommendedSize { size, recommended }) = report.warning
            {
                notes.push(BundleNote::IconBelowRecommendedSize { size, recommended });
            }
            report.paths
        }
        Err(err) => {
            notes.push(BundleNote::IconUnusable {
                path: source.to_path_buf(),
                reason: err.to_string(),
            });
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons::IconError;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-desktop-bundle-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prepare_dir_clears_previous_contents() {
        let dir = temp_dir("prepare");
        let bundle = dir.join("bundle");
        fs::create_dir_all(bundle.join("nested")).unwrap();
        fs::write(bundle.join("nested/stale"), b"x").unwrap();

        prepare_dir(&bundle).unwrap();
        assert!(bundle.is_dir());
        assert!(!bundle.join("nested").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn copying_a_missing_source_names_it_in_the_error() {
        let dir = temp_dir("copy-missing");
        let err = copy_file(&dir.join("absent"), &dir.join("out/absent")).unwrap_err();
        assert!(err.to_string().contains("absent"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rejected_icon_source_becomes_a_note_and_no_paths() {
        let mut notes = Vec::new();
        let paths = record_icons(
            Path::new("/tmp/icon.png"),
            Err(IconError::TooSmall {
                path: PathBuf::from("/tmp/icon.png"),
                size: 128,
                min: 512,
            }),
            &mut notes,
        );
        assert!(paths.is_empty());
        assert!(matches!(
            notes.as_slice(),
            [BundleNote::IconUnusable { .. }]
        ));
    }

    #[test]
    fn a_soft_icon_source_still_yields_paths_plus_a_note() {
        let mut notes = Vec::new();
        let paths = record_icons(
            Path::new("/tmp/icon.png"),
            Ok(IconReport {
                paths: vec![PathBuf::from("/tmp/out.icns")],
                warning: Some(SourceWarning::BelowRecommendedSize {
                    size: 512,
                    recommended: 1024,
                }),
            }),
            &mut notes,
        );
        assert_eq!(paths, vec![PathBuf::from("/tmp/out.icns")]);
        assert_eq!(
            notes,
            vec![BundleNote::IconBelowRecommendedSize {
                size: 512,
                recommended: 1024
            }]
        );
    }
}
