//! Extension -> (kind, MIME) table shared by the desktop picker and any folder scan.

use std::path::Path;

use crate::{MediaFilter, MediaKind};

const IMAGES: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("heic", "image/heic"),
    ("heif", "image/heif"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("bmp", "image/bmp"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("dng", "image/x-adobe-dng"),
    ("cr2", "image/x-canon-cr2"),
    ("cr3", "image/x-canon-cr3"),
    ("nef", "image/x-nikon-nef"),
    ("arw", "image/x-sony-arw"),
    ("raf", "image/x-fuji-raf"),
    ("orf", "image/x-olympus-orf"),
    ("rw2", "image/x-panasonic-rw2"),
];

const VIDEOS: &[(&str, &str)] = &[
    ("mp4", "video/mp4"),
    ("mov", "video/quicktime"),
    ("m4v", "video/x-m4v"),
    ("3gp", "video/3gpp"),
    ("mkv", "video/x-matroska"),
    ("webm", "video/webm"),
];

/// Kind and MIME type for a file extension (case-insensitive, no dot).
pub(crate) fn lookup(ext: &str) -> Option<(MediaKind, &'static str)> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let find = |table: &'static [(&'static str, &'static str)]| {
        table.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
    };
    if let Some(m) = find(IMAGES) {
        return Some((MediaKind::Image, m));
    }
    find(VIDEOS).map(|m| (MediaKind::Video, m))
}

/// [`lookup`] by the extension of `path`.
#[cfg_attr(target_os = "ios", allow(dead_code))] // Android and desktop only.
pub(crate) fn classify_path(path: &Path) -> Option<(MediaKind, &'static str)> {
    lookup(path.extension()?.to_str()?)
}

/// The first extension the table maps to `mime` (case-insensitive).
#[cfg_attr(target_os = "ios", allow(dead_code))] // Android only.
pub(crate) fn extension_for_mime(mime: &str) -> Option<&'static str> {
    let mime = mime.to_ascii_lowercase();
    IMAGES
        .iter()
        .chain(VIDEOS)
        .find(|(_, m)| *m == mime)
        .map(|(e, _)| *e)
}

/// The extensions a picker should offer for `filter`.
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))] // Desktop only.
pub(crate) fn extensions_for(filter: MediaFilter) -> Vec<&'static str> {
    let images = IMAGES.iter().map(|(e, _)| *e);
    let videos = VIDEOS.iter().map(|(e, _)| *e);
    match filter {
        MediaFilter::Images => images.collect(),
        MediaFilter::Videos => videos.collect(),
        MediaFilter::ImagesAndVideos => images.chain(videos).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_extension_round_trips() {
        for (e, m) in IMAGES {
            assert_eq!(lookup(e), Some((MediaKind::Image, *m)));
        }
        for (e, m) in VIDEOS {
            assert_eq!(lookup(e), Some((MediaKind::Video, *m)));
        }
    }

    #[test]
    fn lookup_is_case_insensitive_and_dot_tolerant() {
        assert_eq!(lookup("JPG"), Some((MediaKind::Image, "image/jpeg")));
        assert_eq!(lookup(".MoV"), Some((MediaKind::Video, "video/quicktime")));
        assert_eq!(lookup("txt"), None);
        assert_eq!(lookup(""), None);
    }

    #[test]
    fn classify_path_uses_the_extension() {
        assert_eq!(
            classify_path(Path::new("/a/b/IMG_1.HEIC")),
            Some((MediaKind::Image, "image/heic"))
        );
        assert_eq!(classify_path(Path::new("/a/noext")), None);
    }

    #[test]
    fn filter_lists() {
        let images = extensions_for(MediaFilter::Images);
        let videos = extensions_for(MediaFilter::Videos);
        let both = extensions_for(MediaFilter::ImagesAndVideos);
        assert!(images.contains(&"png") && !images.contains(&"mp4"));
        assert!(videos.contains(&"mp4") && !videos.contains(&"png"));
        assert_eq!(both.len(), images.len() + videos.len());
        assert!(images.iter().chain(&videos).all(|e| lookup(e).is_some()));
    }

    #[test]
    fn mime_maps_back_to_an_extension() {
        assert_eq!(extension_for_mime("image/jpeg"), Some("jpg"));
        assert_eq!(extension_for_mime("VIDEO/MP4"), Some("mp4"));
        assert_eq!(extension_for_mime("text/plain"), None);
    }

    #[test]
    fn extensions_are_unique() {
        let both = extensions_for(MediaFilter::ImagesAndVideos);
        let mut sorted = both.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), both.len());
    }
}
