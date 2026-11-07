//! Linux, Windows and web: no runtime permission system behind the API.

use crate::{Permission, PermissionError, PermissionStatus};

pub(crate) fn status(_p: Permission) -> Result<PermissionStatus, PermissionError> {
    Err(PermissionError::Unsupported)
}

pub(crate) fn request(_p: Permission) -> Result<PermissionStatus, PermissionError> {
    Err(PermissionError::Unsupported)
}

pub(crate) fn open_settings() -> Result<(), PermissionError> {
    Err(PermissionError::Unsupported)
}
