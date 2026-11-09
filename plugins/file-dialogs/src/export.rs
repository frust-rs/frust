//! Exporting a file: the outcome type and the copy step shared with
//! [`FolderEntry::copy_to`](crate::FolderEntry::copy_to).

use std::path::Path;

use crate::FileDialogError;

/// What [`FileDialog::export_file`](crate::FileDialog::export_file) did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExportOutcome {
    /// The file was written to the place the user chose.
    Saved,
    /// The user dismissed the dialog; nothing was written.
    Cancelled,
}

/// Logs the detail of a failed file operation and reduces it to the payload-free
/// variant. `std::io::Error`'s text carries the OS reason, never a path.
pub(crate) fn io_error(err: std::io::Error) -> FileDialogError {
    log::warn!("frust-file-dialogs: file operation failed: {err}");
    FileDialogError::Io
}

/// Copies `source` over `destination`. A file this call created is removed again
/// if the copy fails part-way.
pub(crate) fn copy_file(source: &Path, destination: &Path) -> Result<(), FileDialogError> {
    // `fs::copy` onto its own source truncates the file.
    if is_same_file(source, destination) {
        return Ok(());
    }
    let existed = destination.exists();
    match std::fs::copy(source, destination) {
        Ok(_) => Ok(()),
        Err(err) => {
            if !existed {
                let _ = std::fs::remove_file(destination);
            }
            Err(io_error(err))
        }
    }
}

fn is_same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TestDir;

    #[test]
    fn copies_over_an_existing_destination() {
        let dir = TestDir::new();
        let source = dir.write("source.txt", b"new contents");
        let destination = dir.write("destination.txt", b"old");
        copy_file(&source, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"new contents");
        assert_eq!(std::fs::read(&source).unwrap(), b"new contents");
    }

    #[test]
    fn copying_a_file_onto_itself_keeps_its_contents() {
        let dir = TestDir::new();
        let source = dir.write("same.txt", b"keep me");
        copy_file(&source, &source).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), b"keep me");
    }

    #[test]
    fn a_failed_copy_reports_io_and_creates_nothing() {
        let dir = TestDir::new();
        let destination = dir.path().join("destination.txt");
        let result = copy_file(&dir.path().join("missing.txt"), &destination);
        assert_eq!(result, Err(FileDialogError::Io));
        assert!(!destination.exists());
    }
}
