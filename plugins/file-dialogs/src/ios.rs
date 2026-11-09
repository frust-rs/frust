//! iOS backend. Every dialog call answers [`FileDialogError::Unsupported`]: the
//! document picker is not wired in. Listing is the shared `walk`.

use std::path::{Path, PathBuf};

use crate::{ExportOutcome, FileDialogError, FolderEntry, FolderGrant, ListOptions, Request};

pub(crate) enum GrantState {}

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
    grant: &FolderGrant,
    options: &ListOptions,
) -> Result<Vec<FolderEntry>, FileDialogError> {
    let root = grant.local_path().ok_or(FileDialogError::AccessLost)?;
    crate::walk::list(root, options)
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
