//! `frust-file-dialogs`: native file dialogs, persistable folder access and
//! file export.
//!
//! A **platform plugin** (`docs/ARCHITECTURE.md`) the `frust` facade does not
//! re-export. Linux, macOS and Windows go through `rfd`, Android through the
//! Storage Access Framework and iOS through `UIDocumentPickerViewController`;
//! any other target reports [`FileDialogError::Unsupported`]. On Android and iOS
//! opening a file returns an app-private copy and [`FileDialog::save_file`] is
//! unsupported: use [`FileDialog::export_file`].
//!
//! # Blocking contract
//!
//! Every [`FileDialog`] terminal call blocks until the dialog closes, and
//! [`FolderGrant`] and [`FolderEntry`] calls block on storage. Run them inside
//! `frust::spawn_blocking` and deliver the result through `use_task` /
//! `AsyncValue`; never call them from a paint or event handler or write a signal
//! from the background thread. Android and iOS refuse a call on the UI thread
//! with [`FileDialogError::UiThread`]. A cancel is `Ok(None)`, an empty `Vec` or
//! [`ExportOutcome::Cancelled`]; `rfd` cannot report a missing desktop backend,
//! which looks like a cancel (see the README).

mod export;
mod filter;
mod folder;
#[cfg(any(
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod walk;

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
#[cfg(target_os = "android")]
#[path = "android.rs"]
mod backend;
#[cfg(target_os = "ios")]
#[path = "ios.rs"]
mod backend;
#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
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

pub use export::ExportOutcome;
pub use filter::FileFilter;
pub use folder::{EntryLocation, FolderEntry, FolderGrant, ListOptions};

/// Why a dialog call failed. Never carries paths; the detail goes to the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FileDialogError {
    /// This target has no native file dialog backend.
    #[error("file dialogs are not supported on this platform")]
    Unsupported,
    /// A blocking call ran on the Android or iOS UI thread and was refused.
    #[error("file dialog calls block and cannot run on the UI thread")]
    UiThread,
    /// No foreground activity or view controller could present the dialog.
    #[error("no foreground window is available to present the file dialog")]
    NoPresenter,
    /// The platform side of the plugin is not initialised in this process.
    #[error("the file dialog platform integration is not initialized")]
    PlatformNotInitialized,
    /// A persisted folder grant is no longer honoured; ask the user again.
    #[error("access to the folder is no longer available")]
    AccessLost,
    /// Reading, writing or copying a file failed.
    #[error("a file operation failed")]
    Io,
    /// The platform dialog or storage API reported a failure.
    #[error("the platform file dialog failed")]
    Platform,
}

/// The dialog flavour a [`Request`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    OpenFile,
    OpenFiles,
    SaveFile,
    PickFolder,
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

    /// Picks a folder and holds access to it. Blocks; `Ok(None)` means
    /// cancelled. Filters are ignored.
    pub fn pick_folder(self) -> Result<Option<FolderGrant>, FileDialogError> {
        backend::pick_folder(self.folder_request())
    }

    /// Lets the user choose where a copy of `source` goes, pre-filling
    /// `suggested_name` or else `source`'s own name, and writes it. Blocks.
    pub fn export_file(
        self,
        source: &Path,
        suggested_name: Option<&str>,
    ) -> Result<ExportOutcome, FileDialogError> {
        backend::export_file(self.export_request(source, suggested_name), source)
    }

    fn folder_request(self) -> Request {
        Request {
            filters: Vec::new(),
            ..self.request(Kind::PickFolder, None)
        }
    }

    fn export_request(self, source: &Path, suggested_name: Option<&str>) -> Request {
        let source_name = source.file_name().and_then(|name| name.to_str());
        self.request(Kind::SaveFile, suggested_name.or(source_name))
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
mod test_dir {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A unique directory under the system temp dir, removed on drop.
    pub(crate) struct TestDir(PathBuf);

    impl TestDir {
        pub(crate) fn new() -> Self {
            let unique = format!(
                "frust-file-dialogs-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("create test dir");
            Self(path)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }

        /// Writes `bytes` at `relative`, creating parents, and returns the path.
        pub(crate) fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().expect("has a parent")).expect("create parent");
            std::fs::write(&path, bytes).expect("write test file");
            path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
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
    fn folder_request_drops_filters_and_keeps_title_and_directory() {
        let req = FileDialog::new()
            .title("Pick")
            .directory("/tmp")
            .filter(FileFilter::new("Images", &["png"]))
            .folder_request();
        assert_eq!(req.kind, Kind::PickFolder);
        assert_eq!(req.title.as_deref(), Some("Pick"));
        assert_eq!(req.directory.as_deref(), Some(Path::new("/tmp")));
        assert!(req.filters.is_empty() && req.file_name.is_none());
    }

    #[test]
    fn export_request_prefers_the_suggested_name_over_the_source_name() {
        let source = Path::new("/data/report.pdf");
        let named = FileDialog::new().export_request(source, Some("copy.pdf"));
        assert_eq!(named.kind, Kind::SaveFile);
        assert_eq!(named.file_name.as_deref(), Some("copy.pdf"));
        let unnamed = FileDialog::new().export_request(source, None);
        assert_eq!(unnamed.file_name.as_deref(), Some("report.pdf"));
    }

    #[test]
    fn no_error_display_carries_a_path() {
        for error in [
            FileDialogError::Unsupported,
            FileDialogError::UiThread,
            FileDialogError::NoPresenter,
            FileDialogError::PlatformNotInitialized,
            FileDialogError::AccessLost,
            FileDialogError::Io,
            FileDialogError::Platform,
        ] {
            let text = error.to_string();
            assert!(!text.is_empty() && !text.contains(['/', '\\']), "{text}");
        }
    }

    #[test]
    fn error_display_has_no_path() {
        assert_eq!(
            FileDialogError::Unsupported.to_string(),
            "file dialogs are not supported on this platform"
        );
    }
}
