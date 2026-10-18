//! Fallback backend for targets without a native dialog (Android, iOS, web).

use std::path::PathBuf;

use crate::{FileDialogError, Request};

/// Always reports [`FileDialogError::Unsupported`].
pub(crate) fn show(_request: Request) -> Result<Vec<PathBuf>, FileDialogError> {
    Err(FileDialogError::Unsupported)
}

#[cfg(test)]
mod tests {
    use crate::{FileDialog, FileDialogError};

    #[test]
    fn non_desktop_targets_are_unsupported() {
        assert_eq!(
            FileDialog::new().open_file(),
            Err(FileDialogError::Unsupported)
        );
    }
}
