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
pub(crate) fn assets_after(
    _query: &crate::AssetQuery,
    _cursor: Option<&crate::AssetCursor>,
) -> Result<crate::AssetCursorPage, PhotoError> {
    Err(PhotoError::Unsupported)
}

#[cfg(feature = "library")]
pub(crate) fn thumbnail(
    _id: &crate::AssetId,
    _request: &crate::ThumbnailRequest,
    _token: &crate::CancelToken,
) -> Result<crate::Thumbnail, PhotoError> {
    Err(PhotoError::Unsupported)
}

#[cfg(feature = "library")]
pub(crate) fn manage_limited_selection() -> Result<(), PhotoError> {
    Err(PhotoError::Unsupported)
}

// Change feed: these targets have no photo library to watch.
#[cfg(feature = "library")]
pub(crate) fn current_change_token() -> Result<crate::ChangeToken, PhotoError> {
    Err(PhotoError::Unsupported)
}

#[cfg(feature = "library")]
pub(crate) fn changes_since(
    _token: &crate::ChangeToken,
) -> Result<crate::LibraryChanges, PhotoError> {
    Err(PhotoError::Unsupported)
}

#[cfg(feature = "library")]
pub(crate) fn start_change_observer() -> Result<(), PhotoError> {
    Err(PhotoError::Unsupported)
}

#[cfg(feature = "library")]
pub(crate) fn stop_change_observer() {}

// Originals and saving: replaced by the platform implementations.
#[cfg(feature = "library")]
pub(crate) fn export_original(
    _id: &crate::AssetId,
    _options: &crate::ExportOptions,
) -> Result<crate::ExportedOriginal, PhotoError> {
    Err(PhotoError::Unsupported)
}

// Metadata: these targets have no photo library to read from.
#[cfg(feature = "library")]
pub(crate) fn asset_metadata(
    _id: &crate::AssetId,
    _options: &crate::MetadataOptions,
) -> Result<crate::ExifData, PhotoError> {
    Err(PhotoError::Unsupported)
}

#[cfg(feature = "save")]
pub(crate) fn save(
    _request: &crate::SaveRequest,
    _kind: crate::MediaKind,
) -> Result<crate::SavedAsset, PhotoError> {
    Err(PhotoError::Unsupported)
}

// Decode: these targets have no native decoder.
#[cfg(feature = "decode")]
pub(crate) fn decode_file(
    _path: &std::path::Path,
    _options: &crate::DecodeOptions,
) -> Result<crate::DecodedImage, PhotoError> {
    Err(PhotoError::Unsupported)
}
