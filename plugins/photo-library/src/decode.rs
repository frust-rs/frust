//! Native full-resolution decode: the options and result types, and the platform-independent
//! half of [`PhotoLibrary::decode_file`](crate::PhotoLibrary::decode_file) and
//! [`PhotoLibrary::decode_asset`](crate::PhotoLibrary::decode_asset).

use crate::PhotoError;
use crate::cancel::CancelToken;
use crate::types::ExifData;

/// The colour space the decoded pixels are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImageColorSpace {
    /// sRGB.
    Srgb,
    /// Display P3.
    DisplayP3,
    /// Any other space the platform reported.
    Other,
}

/// Options for [`PhotoLibrary::decode_file`](crate::PhotoLibrary::decode_file) and
/// [`PhotoLibrary::decode_asset`](crate::PhotoLibrary::decode_asset). There is no constructor
/// without a size cap: a full-resolution decode of an untrusted file is unbounded otherwise.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// The longest side the result may have, in pixels. Larger images are downscaled to fit,
    /// keeping their aspect ratio. Zero is rejected with [`PhotoError::InvalidRequest`].
    pub max_dimension: u32,
    /// Convert the pixels to sRGB instead of returning the file's own colour space.
    pub srgb: bool,
    #[allow(dead_code)] // Read by the platform arms.
    pub(crate) cancel: Option<CancelToken>,
}

impl DecodeOptions {
    /// Decode with the longest side capped at `max_dimension` pixels, in the file's own colour
    /// space.
    pub fn new(max_dimension: u32) -> Self {
        Self {
            max_dimension,
            srgb: false,
            cancel: None,
        }
    }

    /// Converts the result to sRGB.
    pub fn srgb(mut self) -> Self {
        self.srgb = true;
        self
    }

    /// Lets the caller cancel the decode through `token`.
    pub fn cancel(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }
}

/// A decoded image.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DecodedImage {
    /// Width of [`rgba`](Self::rgba) in pixels.
    pub width: u32,
    /// Height of [`rgba`](Self::rgba) in pixels.
    pub height: u32,
    /// Width of the source image before any downscale.
    pub source_width: u32,
    /// Height of the source image before any downscale.
    pub source_height: u32,
    /// Straight-alpha RGBA8, rows tightly packed (`width * height * 4` bytes).
    pub rgba: Vec<u8>,
    /// The colour space of [`rgba`](Self::rgba).
    pub color_space: ImageColorSpace,
    /// Capture metadata when the file has any.
    pub exif: Option<ExifData>,
}

/// Cancelled-before-start and size-cap validation, shared by both public calls.
#[cfg(feature = "decode")]
pub(crate) fn precheck(options: &DecodeOptions) -> Result<(), PhotoError> {
    if options
        .cancel
        .as_ref()
        .is_some_and(CancelToken::is_cancelled)
    {
        return Err(PhotoError::Cancelled);
    }
    if options.max_dimension == 0 {
        return Err(PhotoError::InvalidRequest {
            reason: "max_dimension must be greater than zero",
        });
    }
    Ok(())
}

#[cfg_attr(feature = "decode", allow(dead_code))]
pub(crate) fn decode_disabled() -> PhotoError {
    PhotoError::FeatureDisabled { feature: "decode" }
}

/// Export, decode, then remove the exported file whatever the outcome. `export` and `decode`
/// are the platform calls; tests pass fakes.
#[cfg(all(feature = "decode", feature = "library"))]
pub(crate) fn decode_asset_with<E, D>(
    id: &crate::AssetId,
    options: &DecodeOptions,
    export_options: &crate::ExportOptions,
    export: E,
    decode: D,
) -> Result<DecodedImage, PhotoError>
where
    E: FnOnce(
        &crate::AssetId,
        &crate::ExportOptions,
    ) -> Result<crate::ExportedOriginal, PhotoError>,
    D: FnOnce(&std::path::Path, &DecodeOptions) -> Result<DecodedImage, PhotoError>,
{
    precheck(options)?;
    let exported = export(id, export_options)?;
    let result = if options
        .cancel
        .as_ref()
        .is_some_and(CancelToken::is_cancelled)
    {
        Err(PhotoError::Cancelled)
    } else {
        decode(&exported.path, options)
    };
    if let Err(e) = std::fs::remove_file(&exported.path) {
        log::warn!(
            "could not remove the exported file after decoding: {:?}",
            e.kind()
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_data_needs_no_feature() {
        let e = ExifData::default();
        assert!(e.iso.is_none() && e.gps_latitude.is_none());
    }

    #[test]
    fn options_builders() {
        let o = DecodeOptions::new(512);
        assert_eq!(o.max_dimension, 512);
        assert!(!o.srgb);
        assert!(o.srgb().cancel(CancelToken::new()).cancel.is_some());
    }

    #[cfg(not(feature = "decode"))]
    #[test]
    fn calls_are_gated_without_decode() {
        let want = Err(PhotoError::FeatureDisabled { feature: "decode" });
        assert_eq!(
            crate::PhotoLibrary::decode_file(std::path::Path::new("a.jpg"), &DecodeOptions::new(8))
                .map(|_| ()),
            want
        );
        assert_eq!(
            crate::PhotoLibrary::decode_asset(
                &crate::AssetId::from_raw("a"),
                &DecodeOptions::new(8),
                &crate::ExportOptions::new()
            )
            .map(|_| ()),
            want
        );
    }

    #[cfg(all(feature = "decode", not(feature = "library")))]
    #[test]
    fn decode_asset_needs_library() {
        assert_eq!(
            crate::PhotoLibrary::decode_asset(
                &crate::AssetId::from_raw("a"),
                &DecodeOptions::new(8),
                &crate::ExportOptions::new()
            )
            .map(|_| ()),
            Err(PhotoError::FeatureDisabled { feature: "library" })
        );
    }

    #[cfg(feature = "decode")]
    #[test]
    fn precheck_rejects_cancelled_and_zero() {
        let t = CancelToken::new();
        t.cancel();
        assert_eq!(
            crate::PhotoLibrary::decode_file(
                std::path::Path::new("a.jpg"),
                &DecodeOptions::new(8).cancel(t)
            )
            .map(|_| ()),
            Err(PhotoError::Cancelled)
        );
        assert!(matches!(
            crate::PhotoLibrary::decode_file(std::path::Path::new("a.jpg"), &DecodeOptions::new(0)),
            Err(PhotoError::InvalidRequest { .. })
        ));
    }

    #[cfg(all(
        feature = "decode",
        any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows",
            target_os = "freebsd"
        )
    ))]
    #[test]
    fn desktop_decode_file_is_unsupported() {
        assert_eq!(
            crate::PhotoLibrary::decode_file(std::path::Path::new("a.jpg"), &DecodeOptions::new(8))
                .map(|_| ()),
            Err(PhotoError::Unsupported)
        );
    }

    #[cfg(all(feature = "decode", feature = "library"))]
    mod seam {
        use super::*;
        use crate::{AssetId, ExportOptions, ExportedOriginal, MediaKind};
        use std::cell::Cell;
        use std::path::PathBuf;

        fn temp_file(tag: &str) -> PathBuf {
            let p = std::env::temp_dir()
                .join(format!("frust-photo-decode-{tag}-{}", std::process::id()));
            std::fs::write(&p, b"x").unwrap();
            p
        }

        fn exported(path: &std::path::Path) -> ExportedOriginal {
            ExportedOriginal {
                path: path.to_path_buf(),
                file_name: None,
                mime: None,
                kind: MediaKind::Image,
                byte_len: 1,
            }
        }

        fn image() -> DecodedImage {
            DecodedImage {
                width: 1,
                height: 1,
                source_width: 1,
                source_height: 1,
                rgba: vec![0; 4],
                color_space: ImageColorSpace::Srgb,
                exif: None,
            }
        }

        fn run(
            tag: &str,
            opts: &DecodeOptions,
            decode: impl FnOnce(&std::path::Path, &DecodeOptions) -> Result<DecodedImage, PhotoError>,
        ) -> (Result<DecodedImage, PhotoError>, PathBuf) {
            let path = temp_file(tag);
            let p = path.clone();
            let r = decode_asset_with(
                &AssetId::from_raw("a"),
                opts,
                &ExportOptions::new(),
                |_, _| Ok(exported(&p)),
                decode,
            );
            (r, path)
        }

        #[test]
        fn exported_file_is_removed_on_success() {
            let (r, path) = run("ok", &DecodeOptions::new(8), |p, _| {
                assert!(p.exists());
                Ok(image())
            });
            assert_eq!(r, Ok(image()));
            assert!(!path.exists());
        }

        #[test]
        fn exported_file_is_removed_on_failure() {
            let (r, path) = run("fail", &DecodeOptions::new(8), |_, _| {
                Err(PhotoError::Platform("bad".into()))
            });
            assert_eq!(r, Err(PhotoError::Platform("bad".into())));
            assert!(!path.exists());
        }

        #[test]
        fn exported_file_is_removed_on_cancel() {
            let t = CancelToken::new();
            let t2 = t.clone();
            // Cancelled between export and decode: decode never runs.
            let path = temp_file("cancel");
            let p = path.clone();
            let r = decode_asset_with(
                &AssetId::from_raw("a"),
                &DecodeOptions::new(8).cancel(t),
                &ExportOptions::new(),
                |_, _| {
                    t2.cancel();
                    Ok(exported(&p))
                },
                |_, _| panic!("decode must not run after a cancel"),
            );
            assert_eq!(r.map(|_| ()), Err(PhotoError::Cancelled));
            assert!(!path.exists());

            // Cancelled during decode: the platform reports it.
            let (r, path) = run("cancel2", &DecodeOptions::new(8), |_, _| {
                Err(PhotoError::Cancelled)
            });
            assert_eq!(r.map(|_| ()), Err(PhotoError::Cancelled));
            assert!(!path.exists());
        }

        #[test]
        fn precheck_runs_before_export() {
            let exports = Cell::new(0);
            let r = decode_asset_with(
                &AssetId::from_raw("a"),
                &DecodeOptions::new(0),
                &ExportOptions::new(),
                |_, _| {
                    exports.set(exports.get() + 1);
                    Err(PhotoError::NotFound)
                },
                |_, _| Ok(image()),
            );
            assert!(matches!(r, Err(PhotoError::InvalidRequest { .. })));
            assert_eq!(exports.get(), 0);
        }

        #[test]
        fn export_failure_is_returned() {
            let r = decode_asset_with(
                &AssetId::from_raw("a"),
                &DecodeOptions::new(8),
                &ExportOptions::new(),
                |_, _| Err(PhotoError::NotFound),
                |_, _| Ok(image()),
            );
            assert_eq!(r.map(|_| ()), Err(PhotoError::NotFound));
        }
    }
}
