//! Reading one library asset's EXIF: the options type and the platform-independent half of
//! [`PhotoLibrary::asset_metadata`](crate::PhotoLibrary::asset_metadata).

use std::fmt;

use crate::cancel::CancelToken;

/// Options for [`PhotoLibrary::asset_metadata`](crate::PhotoLibrary::asset_metadata).
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct MetadataOptions {
    /// Whether an original that is only in iCloud may be downloaded to read its metadata
    /// (default `false`). When `false` such an asset fails with
    /// [`PhotoError::NetworkRequired`](crate::PhotoError::NetworkRequired). Android makes no
    /// network distinction, so this has no effect there.
    pub allow_network: bool,
    // Read only by the metadata paths, which the `library` feature compiles.
    #[cfg_attr(not(feature = "library"), allow(dead_code))]
    pub(crate) cancel: Option<CancelToken>,
}

impl MetadataOptions {
    /// Local data only: no network download.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets whether an iCloud-only original may be downloaded.
    pub fn allow_network(mut self, allow: bool) -> Self {
        self.allow_network = allow;
        self
    }

    /// Lets the caller cancel the read through `token`.
    pub fn cancel(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Whether the caller's token, if any, has been cancelled.
    #[cfg(feature = "library")]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(CancelToken::is_cancelled)
    }
}

impl fmt::Debug for MetadataOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetadataOptions")
            .field("allow_network", &self.allow_network)
            .field("cancel", &self.cancel.is_some())
            .finish()
    }
}

/// A cancelled token first, then `read`: the whole of
/// [`PhotoLibrary::asset_metadata`](crate::PhotoLibrary::asset_metadata). `read` is the platform
/// arm, which refuses its UI thread itself; tests pass fakes.
#[cfg(feature = "library")]
pub(crate) fn asset_metadata_with<R>(
    id: &crate::AssetId,
    options: &MetadataOptions,
    read: R,
) -> Result<crate::ExifData, crate::PhotoError>
where
    R: FnOnce(&crate::AssetId, &MetadataOptions) -> Result<crate::ExifData, crate::PhotoError>,
{
    if options.is_cancelled() {
        return Err(crate::PhotoError::Cancelled);
    }
    read(id, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AssetId, PhotoError, PhotoLibrary};

    #[test]
    fn options_default_to_local_data_and_never_print_the_token() {
        let o = MetadataOptions::new();
        assert!(!o.allow_network);
        assert!(o.cancel.is_none());
        assert_eq!(
            format!("{o:?}"),
            "MetadataOptions { allow_network: false, cancel: false }"
        );
        let o = o.allow_network(true).cancel(CancelToken::new());
        assert!(o.allow_network);
        assert_eq!(
            format!("{o:?}"),
            "MetadataOptions { allow_network: true, cancel: true }"
        );
    }

    #[cfg(not(feature = "library"))]
    #[test]
    fn asset_metadata_is_feature_gated() {
        let token = CancelToken::new();
        token.cancel();
        for options in [MetadataOptions::new(), MetadataOptions::new().cancel(token)] {
            assert_eq!(
                PhotoLibrary::asset_metadata(&AssetId::from_raw("a"), &options),
                Err(PhotoError::FeatureDisabled { feature: "library" })
            );
        }
    }

    #[cfg(feature = "library")]
    #[test]
    fn a_cancelled_token_stops_the_read_before_the_platform() {
        let token = CancelToken::new();
        token.cancel();
        let options = MetadataOptions::new().cancel(token);
        assert_eq!(
            asset_metadata_with(&AssetId::from_raw("a"), &options, |_, _| {
                panic!("the platform must not run after a cancel")
            }),
            Err(PhotoError::Cancelled)
        );
        // Through the public call as well, on every platform.
        assert_eq!(
            PhotoLibrary::asset_metadata(&AssetId::from_raw("a"), &options),
            Err(PhotoError::Cancelled)
        );
    }

    #[cfg(feature = "library")]
    #[test]
    fn the_platform_answer_is_returned_unchanged() {
        use crate::ExifData;
        let exif = ExifData {
            camera_make: Some("Frust".to_owned()),
            ..ExifData::default()
        };
        let passed = exif.clone();
        let options = MetadataOptions::new().allow_network(true);
        assert_eq!(
            asset_metadata_with(&AssetId::from_raw("a"), &options, |id, o| {
                assert_eq!(id.as_str(), "a");
                assert!(o.allow_network);
                Ok(passed)
            }),
            Ok(exif)
        );
        // An asset without EXIF is an empty value, not an error.
        assert_eq!(
            asset_metadata_with(&AssetId::from_raw("a"), &options, |_, _| Ok(
                ExifData::default()
            )),
            Ok(ExifData::default())
        );
        assert_eq!(
            asset_metadata_with(&AssetId::from_raw("a"), &options, |_, _| Err(
                PhotoError::NetworkRequired
            )),
            Err(PhotoError::NetworkRequired)
        );
    }

    #[cfg(all(
        feature = "library",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    #[test]
    fn desktop_asset_metadata_is_unsupported() {
        assert_eq!(
            PhotoLibrary::asset_metadata(&AssetId::from_raw("a"), &MetadataOptions::new()),
            Err(PhotoError::Unsupported)
        );
    }
}
