//! Desktop: no photo library, so the picker is the native file dialog.

use frust_file_dialogs::{FileDialog, FileDialogError, FileFilter};

use crate::media_types::{classify_path, extensions_for};
use crate::{MediaFilter, MediaKind, PhotoError, PickOptions, PickedItem};

pub(crate) fn pick(options: &PickOptions) -> Result<Vec<PickedItem>, PhotoError> {
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
    let filter = options.filter;
    Ok(paths
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
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;

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
