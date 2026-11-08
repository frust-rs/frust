//! Targets with neither a photo picker nor a native file dialog (web and other).

use crate::{PhotoError, PickOptions, PickedItem};

pub(crate) fn pick(_options: &PickOptions) -> Result<Vec<PickedItem>, PhotoError> {
    Err(PhotoError::Unsupported)
}

// Library browsing: these targets have no photo library to browse.
#[cfg(feature = "library")]
pub(crate) fn albums() -> Result<Vec<crate::Album>, PhotoError> {
    Err(PhotoError::Unsupported)
}

#[cfg(feature = "library")]
pub(crate) fn assets(_query: &crate::AssetQuery) -> Result<crate::AssetPage, PhotoError> {
    Err(PhotoError::Unsupported)
}

#[cfg(feature = "library")]
pub(crate) fn thumbnail(
    _id: &crate::AssetId,
    _request: &crate::ThumbnailRequest,
) -> Result<crate::Thumbnail, PhotoError> {
    Err(PhotoError::Unsupported)
}

#[cfg(feature = "library")]
pub(crate) fn manage_limited_selection() -> Result<(), PhotoError> {
    Err(PhotoError::Unsupported)
}
