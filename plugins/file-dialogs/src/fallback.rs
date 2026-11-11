//! Fallback backend for targets without a native dialog (web and anything else
//! outside the desktop, Android and iOS arms).

use std::path::{Path, PathBuf};

use crate::{ExportOutcome, FileDialogError, FolderEntry, FolderGrant, ListOptions, Request};

/// Uninhabited: no grant is ever issued on these targets.
pub(crate) enum GrantState {}

/// Always reports [`FileDialogError::Unsupported`].
pub(crate) fn show(_request: Request) -> Result<Vec<PathBuf>, FileDialogError> {
    Err(FileDialogError::Unsupported)
}

pub(crate) fn pick_folder(_request: Request) -> Result<Option<FolderGrant>, FileDialogError> {
    Err(FileDialogError::Unsupported)
}

pub(crate) fn restore_folder(_token: &str) -> Result<FolderGrant, FileDialogError> {
    Err(FileDialogError::Unsupported)
}

pub(crate) fn list_folder(
    _grant: &FolderGrant,
    _options: &ListOptions,
) -> Result<Vec<FolderEntry>, FileDialogError> {
    Err(FileDialogError::Unsupported)
}

pub(crate) fn resolve_entry(
    _grant: &FolderGrant,
    _segments: &[&str],
) -> Result<Option<FolderEntry>, FileDialogError> {
    Err(FileDialogError::Unsupported)
}

pub(crate) fn release_folder(_state: GrantState, _token: &str) {}

pub(crate) fn export_file(
    _request: Request,
    _source: &Path,
) -> Result<ExportOutcome, FileDialogError> {
    Err(FileDialogError::Unsupported)
}

pub(crate) fn copy_content_uri(_uri: &str, _destination: &Path) -> Result<(), FileDialogError> {
    Err(FileDialogError::Unsupported)
}

pub(crate) fn read_content_uri(_uri: &str, _max_bytes: u64) -> Result<Vec<u8>, FileDialogError> {
    Err(FileDialogError::Unsupported)
}

#[cfg(test)]
mod tests {
    use crate::{ExportOutcome, FileDialog, FileDialogError, FolderGrant};
    use std::path::Path;

    #[test]
    fn non_desktop_targets_are_unsupported() {
        assert_eq!(
            FileDialog::new().open_file(),
            Err(FileDialogError::Unsupported)
        );
    }

    #[test]
    fn folder_and_export_calls_are_unsupported() {
        assert!(matches!(
            FileDialog::new().pick_folder(),
            Err(FileDialogError::Unsupported)
        ));
        assert!(matches!(
            FolderGrant::restore("token"),
            Err(FileDialogError::Unsupported)
        ));
        let export: Result<ExportOutcome, _> =
            FileDialog::new().export_file(Path::new("a.txt"), None);
        assert_eq!(export, Err(FileDialogError::Unsupported));
    }
}
