//! `frust-file-dialogs`: native file open/save dialogs on Linux, macOS and
//! Windows, backed by the `rfd` crate.
//!
//! This is a **platform plugin** (`docs/ARCHITECTURE.md`): it depends on
//! platform crates only (no `frust-*` framework crate), and the `frust`
//! facade does not re-export it. Android, iOS and web report
//! [`FileDialogError::Unsupported`].
//!
//! # Blocking contract
//!
//! Every terminal call ([`FileDialog::open_file`], [`FileDialog::open_files`],
//! [`FileDialog::save_file`]) blocks the calling thread until the user
//! dismisses the dialog. Run them inside `frust::spawn_blocking` and deliver
//! the result through `use_task` / `AsyncValue`; never call them from a paint
//! or event handler, and never write a signal from the background thread.
//!
//! Threading follows `rfd`. On macOS it marshals the panel to the main thread
//! (`dispatch2::run_on_main`) when `NSApplication` is running, so calling from
//! `frust::spawn_blocking` under the frust desktop shell is correct and the
//! main thread blocks while the modal panel runs. On Windows it initialises
//! COM per calling thread, so a worker thread is fine. On Linux the portal
//! call blocks only the calling thread. The Linux leg was exercised on
//! 2026-10-18; the macOS and Windows legs have not been run on this plugin yet.
//!
//! # Cancellation and failure
//!
//! `Ok(None)` / an empty `Vec` means the user cancelled. `rfd`'s synchronous
//! API cannot report backend failures: on Linux, when neither an
//! xdg-desktop-portal backend nor `zenity` is available it logs an error and
//! returns no selection, which is indistinguishable from a cancel. On a
//! zenity-only desktop `rfd` splits a multi-selection on `'|'`, so
//! [`FileDialog::open_files`] splits a file name containing `|`.

mod filter;

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
#[path = "desktop.rs"]
mod backend;
#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
#[path = "fallback.rs"]
mod backend;

use backend::show;

use std::path::{Path, PathBuf};

pub use filter::FileFilter;

/// Why a dialog call failed. Never carries paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FileDialogError {
    /// This target has no native file dialog backend.
    #[error("file dialogs are not supported on this platform")]
    Unsupported,
}

/// The dialog flavour a [`Request`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    OpenFile,
    OpenFiles,
    SaveFile,
}

/// Everything a backend needs; built by [`FileDialog`], asserted in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub kind: Kind,
    pub title: Option<String>,
    pub directory: Option<PathBuf>,
    pub file_name: Option<String>,
    pub filters: Vec<FileFilter>,
}

/// Builder for a native file dialog.
///
/// ```no_run
/// use frust_file_dialogs::{FileDialog, FileFilter};
///
/// let picked = FileDialog::new()
///     .title("Choose an image")
///     .filter(FileFilter::new("Images", &["png", "jpg"]))
///     .open_file(); // blocks: call inside `frust::spawn_blocking`
/// ```
#[derive(Debug, Clone, Default)]
pub struct FileDialog {
    title: Option<String>,
    directory: Option<PathBuf>,
    filters: Vec<FileFilter>,
}

impl FileDialog {
    /// A dialog with no title, directory or filters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the dialog title.
    pub fn title(mut self, title: &str) -> Self {
        self.title = Some(title.to_owned());
        self
    }

    /// Sets the directory the dialog starts in.
    pub fn directory(mut self, directory: impl AsRef<Path>) -> Self {
        self.directory = Some(directory.as_ref().to_path_buf());
        self
    }

    /// Adds a type filter. Filters without extensions are ignored.
    pub fn filter(mut self, filter: FileFilter) -> Self {
        if !filter.extensions().is_empty() {
            self.filters.push(filter);
        }
        self
    }

    /// Picks one file. Blocks; see the crate-level blocking contract.
    /// `Ok(None)` means the user cancelled.
    pub fn open_file(self) -> Result<Option<PathBuf>, FileDialogError> {
        let mut picked = show(self.request(Kind::OpenFile, None))?;
        picked.truncate(1);
        Ok(picked.pop())
    }

    /// Picks any number of files. Blocks; an empty `Vec` means cancelled.
    pub fn open_files(self) -> Result<Vec<PathBuf>, FileDialogError> {
        show(self.request(Kind::OpenFiles, None))
    }

    /// Chooses a save destination, pre-filling `file_name` when given.
    /// Blocks; `Ok(None)` means cancelled. The file is not created.
    pub fn save_file(self, file_name: Option<&str>) -> Result<Option<PathBuf>, FileDialogError> {
        let mut picked = show(self.request(Kind::SaveFile, file_name))?;
        picked.truncate(1);
        Ok(picked.pop())
    }

    fn request(self, kind: Kind, file_name: Option<&str>) -> Request {
        Request {
            kind,
            title: self.title,
            directory: self.directory,
            file_name: file_name.map(str::to_owned),
            filters: self.filters,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_plumbs_title_directory_file_name_and_filters() {
        let req = FileDialog::new()
            .title("Pick")
            .directory("/tmp")
            .filter(FileFilter::new("Images", &["PNG"]))
            .filter(FileFilter::new("Empty", &[]))
            .request(Kind::SaveFile, Some("a.png"));
        assert_eq!(req.kind, Kind::SaveFile);
        assert_eq!(req.title.as_deref(), Some("Pick"));
        assert_eq!(req.directory.as_deref(), Some(Path::new("/tmp")));
        assert_eq!(req.file_name.as_deref(), Some("a.png"));
        assert_eq!(req.filters.len(), 1);
        assert_eq!(req.filters[0].extensions(), ["png"]);
    }

    #[test]
    fn default_request_is_empty() {
        let req = FileDialog::new().request(Kind::OpenFile, None);
        assert!(req.title.is_none() && req.directory.is_none() && req.file_name.is_none());
        assert!(req.filters.is_empty());
    }

    #[test]
    fn error_display_has_no_path() {
        assert_eq!(
            FileDialogError::Unsupported.to_string(),
            "file dialogs are not supported on this platform"
        );
    }
}
