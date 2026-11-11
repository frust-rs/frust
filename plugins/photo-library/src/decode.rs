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
///
/// # What the cap bounds
///
/// [`max_dimension`](Self::max_dimension) bounds the output: the size of the returned image and
/// of its [`rgba`](DecodedImage::rgba) buffer (at most `max_dimension * max_dimension * 4`
/// bytes). It does not bound the platform's own decode work, which reads the whole file and may
/// hold its own full-size bitmap while it scales. Every returned buffer is also held under a
/// byte ceiling of `i32::MAX` bytes (just under 2 GiB) on every platform: a request whose capped
/// image would need more is [`PhotoError::InvalidRequest`] before the buffer is allocated, and an
/// allocation the allocator refuses is a [`PhotoError::Platform`] error. That does not stop the
/// system from ending the app when memory runs out while a very large buffer is filled, so for
/// an untrusted file, cap near the size you display rather than near the ceiling.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// The longest side the result may have, in pixels. Larger images are downscaled to fit,
    /// keeping their aspect ratio. Zero is rejected with [`PhotoError::InvalidRequest`]. See
    /// [What the cap bounds](Self#what-the-cap-bounds).
    pub max_dimension: u32,
    /// Convert the pixels to sRGB instead of returning the file's own colour space.
    pub srgb: bool,
    // Read only by the decode paths, which the `decode` feature compiles.
    #[cfg_attr(not(feature = "decode"), allow(dead_code))]
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

    /// Whether the caller's token, if any, has been cancelled.
    #[cfg(feature = "decode")]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(CancelToken::is_cancelled)
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

/// Cancelled-before-start and size-cap validation. Both public calls run it exactly once, first,
/// through [`decode_file_with`] and [`decode_asset_with`]; the platform arms rely on it and never
/// repeat it.
#[cfg(feature = "decode")]
pub(crate) fn precheck(options: &DecodeOptions) -> Result<(), PhotoError> {
    if options.is_cancelled() {
        return Err(PhotoError::Cancelled);
    }
    if options.max_dimension == 0 {
        return Err(PhotoError::InvalidRequest {
            reason: "max_dimension must be greater than zero",
        });
    }
    Ok(())
}

// Referenced only where some feature of the pair is off.
#[cfg(not(all(feature = "decode", feature = "library")))]
pub(crate) fn decode_disabled() -> PhotoError {
    PhotoError::FeatureDisabled { feature: "decode" }
}

/// [`precheck`], then `decode`: the whole of
/// [`PhotoLibrary::decode_file`](crate::PhotoLibrary::decode_file). `decode` is the platform
/// arm; tests pass fakes.
#[cfg(feature = "decode")]
pub(crate) fn decode_file_with<D>(
    path: &std::path::Path,
    options: &DecodeOptions,
    decode: D,
) -> Result<DecodedImage, PhotoError>
where
    D: FnOnce(&std::path::Path, &DecodeOptions) -> Result<DecodedImage, PhotoError>,
{
    precheck(options)?;
    decode(path, options)
}

/// [`precheck`], export, decode, then remove the exported file whatever the outcome, a panic in
/// `decode` included. `export` and `decode` are the platform calls; tests pass fakes.
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
    let _remove = RemoveOnDrop(&exported.path);
    if options.is_cancelled() {
        return Err(PhotoError::Cancelled);
    }
    decode(&exported.path, options)
}

/// Removes the exported file when dropped, so success, failure, cancel and a panic during the
/// decode all leave nothing behind.
#[cfg(all(feature = "decode", feature = "library"))]
struct RemoveOnDrop<'a>(&'a std::path::Path);

#[cfg(all(feature = "decode", feature = "library"))]
impl Drop for RemoveOnDrop<'_> {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(self.0) {
            log::warn!(
                "could not remove the exported file after decoding: {:?}",
                e.kind()
            );
        }
    }
}

/// The largest RGBA8 buffer any decode arm returns: `i32::MAX` bytes, the most a
/// `java.nio.ByteBuffer` (Android's transfer buffer) can address, applied on every platform so
/// a too-large request fails the same way everywhere.
#[cfg(all(
    feature = "decode",
    any(target_os = "android", target_os = "ios", target_os = "macos")
))]
pub(crate) const MAX_BUFFER_BYTES: usize = i32::MAX as usize;

/// The byte length of a tightly packed `width x height` RGBA8 buffer, or
/// [`PhotoError::InvalidRequest`] when it would exceed [`MAX_BUFFER_BYTES`].
#[cfg(all(
    feature = "decode",
    any(target_os = "android", target_os = "ios", target_os = "macos")
))]
pub(crate) fn rgba_len(width: u32, height: u32) -> Result<usize, PhotoError> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .filter(|n| *n <= MAX_BUFFER_BYTES)
        .ok_or(PhotoError::InvalidRequest {
            reason: "the capped image is too large for one pixel buffer; lower max_dimension",
        })
}

/// A zeroed buffer of `len` bytes, or an error rather than an abort when memory is short.
#[cfg(all(
    feature = "decode",
    any(target_os = "android", target_os = "ios", target_os = "macos")
))]
pub(crate) fn allocate_rgba(len: usize) -> Result<Vec<u8>, PhotoError> {
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(len)
        .map_err(|_| PhotoError::Platform("not enough memory for the decoded image".to_string()))?;
    rgba.resize(len, 0);
    Ok(rgba)
}

/// `Ok` when `path` exists; a missing file is [`PhotoError::NotFound`] on every arm, and any
/// other failure is [`PhotoError::Io`].
#[cfg(all(
    feature = "decode",
    any(target_os = "android", target_os = "ios", target_os = "macos")
))]
pub(crate) fn require_file(path: &std::path::Path) -> Result<(), PhotoError> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(PhotoError::NotFound),
        Err(e) => Err(e.into()),
    }
}

/// An EXIF measurement (ISO, exposure time, aperture, focal length) that means something: a
/// zero, negative or non-finite value is absent on every arm.
#[cfg(all(
    any(feature = "decode", feature = "library"),
    any(target_os = "android", target_os = "ios", target_os = "macos")
))]
pub(crate) fn positive_measure(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then_some(value)
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

    #[cfg(feature = "decode")]
    #[test]
    fn the_file_path_prechecks_before_the_platform() {
        let t = CancelToken::new();
        t.cancel();
        let refused = |opts: &DecodeOptions| {
            decode_file_with(std::path::Path::new("a.jpg"), opts, |_, _| {
                panic!("the platform must not run after a failed precheck")
            })
            .map(|_| ())
        };
        assert_eq!(
            refused(&DecodeOptions::new(8).cancel(t.clone())),
            Err(PhotoError::Cancelled)
        );
        assert!(matches!(
            refused(&DecodeOptions::new(0)),
            Err(PhotoError::InvalidRequest { .. })
        ));
        assert!(DecodeOptions::new(8).cancel(t).is_cancelled());
        assert!(!DecodeOptions::new(8).is_cancelled());
    }

    #[cfg(all(feature = "decode", any(target_os = "ios", target_os = "macos")))]
    #[test]
    fn a_missing_file_is_not_found() {
        let path = std::env::temp_dir().join(format!(
            "frust-photo-decode-missing-{}.jpg",
            std::process::id()
        ));
        assert_eq!(
            crate::PhotoLibrary::decode_file(&path, &DecodeOptions::new(8)).map(|_| ()),
            Err(PhotoError::NotFound)
        );
    }

    #[cfg(all(
        feature = "decode",
        any(target_os = "android", target_os = "ios", target_os = "macos")
    ))]
    mod shared {
        use super::*;

        #[test]
        fn buffers_over_the_ceiling_are_refused_before_allocating() {
            assert_eq!(rgba_len(400, 300), Ok(400 * 300 * 4));
            // The largest square under the ceiling fits; one pixel more on each side does not.
            assert!(rgba_len(23_170, 23_170).is_ok());
            assert!(matches!(
                rgba_len(23_171, 23_171),
                Err(PhotoError::InvalidRequest { .. })
            ));
            assert!(matches!(
                rgba_len(u32::MAX, u32::MAX),
                Err(PhotoError::InvalidRequest { .. })
            ));
        }

        #[test]
        fn an_allocation_that_cannot_succeed_is_an_error() {
            assert_eq!(allocate_rgba(8), Ok(vec![0; 8]));
            assert!(matches!(
                allocate_rgba(usize::MAX),
                Err(PhotoError::Platform(_))
            ));
        }

        #[test]
        fn require_file_maps_a_missing_path_to_not_found() {
            let path = std::env::temp_dir()
                .join(format!("frust-photo-decode-require-{}", std::process::id()));
            assert_eq!(require_file(&path), Err(PhotoError::NotFound));
            std::fs::write(&path, b"x").unwrap();
            assert_eq!(require_file(&path), Ok(()));
            std::fs::remove_file(&path).unwrap();
        }

        #[test]
        fn non_positive_measures_are_absent() {
            assert_eq!(positive_measure(1.8), Some(1.8));
            for bad in [0.0, -0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                assert_eq!(positive_measure(bad), None, "{bad}");
            }
        }
    }

    #[cfg(all(
        feature = "decode",
        any(target_os = "linux", target_os = "windows", target_os = "freebsd")
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
        fn exported_file_is_removed_when_the_decode_panics() {
            let path = temp_file("panic");
            let p = path.clone();
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                decode_asset_with(
                    &AssetId::from_raw("a"),
                    &DecodeOptions::new(8),
                    &ExportOptions::new(),
                    |_, _| Ok(exported(&p)),
                    |_, _| panic!("decode panicked"),
                )
            }));
            assert!(caught.is_err());
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
