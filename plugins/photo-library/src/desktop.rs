//! Desktop: no photo library, so the picker is the native file dialog.

use frust_file_dialogs::{FileDialog, FileDialogError, FileFilter};

use crate::cancel::ProgressThrottle;
use crate::media_types::{classify_path, extensions_for};
use crate::{
    CancelToken, MediaFilter, MediaKind, PhotoError, PickOptions, PickProgress, PickedItem,
};

pub(crate) fn pick(options: &PickOptions) -> Result<Vec<PickedItem>, PhotoError> {
    if is_cancelled(options) {
        return Err(PhotoError::Cancelled);
    }
    let mut dialog = FileDialog::new().title(match options.filter {
        MediaFilter::Images => "Choose photos",
        MediaFilter::Videos => "Choose videos",
        MediaFilter::ImagesAndVideos => "Choose photos or videos",
    });
    let all = FileFilter::new("Photos and videos", &extensions_for(options.filter));
    dialog = dialog.filter(all);
    if matches!(options.filter, MediaFilter::ImagesAndVideos) {
        dialog = dialog
            .filter(FileFilter::new(
                "Images",
                &extensions_for(MediaFilter::Images),
            ))
            .filter(FileFilter::new(
                "Videos",
                &extensions_for(MediaFilter::Videos),
            ));
    }

    let paths = if options.limit == 1 {
        dialog.open_file().map_err(map_err)?.into_iter().collect()
    } else {
        dialog.open_files().map_err(map_err)?
    };

    let limit = if options.limit == 0 {
        usize::MAX
    } else {
        options.limit as usize
    };
    // A cancel during the dialog wins over its result.
    if is_cancelled(options) {
        return Err(PhotoError::Cancelled);
    }
    let filter = options.filter;
    let items: Vec<PickedItem> = paths
        .into_iter()
        .take(limit)
        .map(|path| {
            let (kind, mime) = match classify_path(&path) {
                Some((k, m)) => (k, Some(m.to_owned())),
                None => (fallback_kind(filter), None),
            };
            let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            PickedItem {
                path,
                kind,
                file_name,
                mime,
                asset_id: None,
                is_copy: false,
            }
        })
        .collect();
    // Desktop returns the originals, so nothing loads: one completed report.
    if let (Some(cb), false) = (&options.progress, items.is_empty()) {
        ProgressThrottle::new(cb.clone()).report(PickProgress {
            items: items.len(),
            done: items.len(),
            fraction: 1.0,
        });
    }
    Ok(items)
}

fn is_cancelled(options: &PickOptions) -> bool {
    options
        .cancel
        .as_ref()
        .is_some_and(CancelToken::is_cancelled)
}

/// The kind of a file whose extension is unknown: a single-kind filter can only return that
/// kind; `ImagesAndVideos` falls back to [`MediaKind::Image`].
fn fallback_kind(filter: MediaFilter) -> MediaKind {
    match filter {
        MediaFilter::Videos => MediaKind::Video,
        MediaFilter::Images | MediaFilter::ImagesAndVideos => MediaKind::Image,
    }
}

fn map_err(e: FileDialogError) -> PhotoError {
    match e {
        FileDialogError::Unsupported => PhotoError::Unsupported,
        other => PhotoError::Platform(other.to_string()),
    }
}

// Library browsing: desktop has no photo library to browse.
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

// Originals and saving: desktop has no photo library.
#[cfg(feature = "library")]
pub(crate) fn export_original(
    _id: &crate::AssetId,
    _options: &crate::ExportOptions,
) -> Result<crate::ExportedOriginal, PhotoError> {
    Err(PhotoError::Unsupported)
}

#[cfg(feature = "save")]
pub(crate) fn save(
    _request: &crate::SaveRequest,
    _kind: crate::MediaKind,
) -> Result<crate::SavedAsset, PhotoError> {
    Err(PhotoError::Unsupported)
}

// Decode: desktop has no native decoder arm yet.
#[cfg(feature = "decode")]
pub(crate) fn decode_file(
    _path: &std::path::Path,
    _options: &crate::DecodeOptions,
) -> Result<crate::DecodedImage, PhotoError> {
    Err(PhotoError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_token_skips_the_dialog() {
        let t = CancelToken::new();
        t.cancel();
        let o = PickOptions::new().cancel(t);
        assert!(is_cancelled(&o));
        assert!(matches!(pick(&o), Err(PhotoError::Cancelled)));
        assert!(!is_cancelled(&PickOptions::new()));
    }

    #[test]
    fn unknown_extensions_follow_the_filter() {
        assert_eq!(fallback_kind(MediaFilter::Videos), MediaKind::Video);
        assert_eq!(fallback_kind(MediaFilter::Images), MediaKind::Image);
        assert_eq!(
            fallback_kind(MediaFilter::ImagesAndVideos),
            MediaKind::Image
        );
    }
}
