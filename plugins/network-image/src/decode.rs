//! Decode, downscale and wrap: bytes in, an [`ImageSource`] out.

use std::io::Cursor;
use std::sync::Arc;

use frust::{ImageSource, spawn_blocking};
use image::imageops::FilterType;

/// A pixel box a decoded image must fit inside (aspect ratio preserved).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TargetPx {
    /// Maximum width in pixels.
    pub width: u32,
    /// Maximum height in pixels.
    pub height: u32,
}

/// Why decoding failed. Clone so it can ride a shared single-flight future.
#[non_exhaustive]
#[derive(thiserror::Error, Debug, Clone)]
pub enum DecodeError {
    /// The bytes are not a supported image.
    #[error("image decode failed: {0}")]
    Decode(String),
    /// The bytes are a recognised format this build cannot decode.
    #[error("unsupported image format: {0} (enable the matching crate feature)")]
    UnsupportedFormat(String),
    /// The image exceeds the configured [`DecodeLimits`]; nothing was decoded
    /// (or decoding was stopped by the allocation cap).
    #[error("image {width}x{height} exceeds the decode limits ({max_width}x{max_height} max)")]
    TooLarge {
        /// Declared image width in pixels.
        width: u32,
        /// Declared image height in pixels.
        height: u32,
        /// Configured maximum width.
        max_width: u32,
        /// Configured maximum height.
        max_height: u32,
    },
    /// The blocking decode task did not complete.
    #[error("decode task failed: {0}")]
    Task(String),
}

/// Bounds on what one decode may allocate.
///
/// The declared dimensions are read from the header and checked before any
/// pixel is decoded, and `image`'s own [`image::Limits`] are set from the same
/// values. The post-decode RGBA conversion and resize can transiently need up
/// to about twice the decoded RGBA size, so the defaults (8192 px per side,
/// 256 MiB) keep the peak near 512 MiB for a single worst-case decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeLimits {
    /// Maximum declared width in pixels (default 8192).
    pub max_width: u32,
    /// Maximum declared height in pixels (default 8192).
    pub max_height: u32,
    /// Maximum decoded RGBA bytes (default 256 MiB).
    pub max_alloc_bytes: u64,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_width: 8192,
            max_height: 8192,
            max_alloc_bytes: 256 * 1024 * 1024,
        }
    }
}

/// Decoded straight-alpha RGBA8 pixels.
pub(crate) struct DecodedImage {
    pub(crate) pixels: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Dimensions fitting `(w, h)` inside `target`, or `None` when it already fits.
fn fit_dims(w: u32, h: u32, target: TargetPx) -> Option<(u32, u32, f64)> {
    if w == 0 || h == 0 || target.width == 0 || target.height == 0 {
        return None;
    }
    let scale = (target.width as f64 / w as f64).min(target.height as f64 / h as f64);
    if scale >= 1.0 {
        return None;
    }
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    Some((nw, nh, scale))
}

/// Decode `bytes` (format guessed) and shrink to fit `target`, if any.
pub(crate) fn decode_and_fit(
    bytes: &[u8],
    target: Option<TargetPx>,
    limits: DecodeLimits,
) -> Result<DecodedImage, DecodeError> {
    let open = || {
        image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| DecodeError::Decode(e.to_string()))
    };
    let reader = open()?;
    #[cfg(not(feature = "webp"))]
    if reader.format() == Some(image::ImageFormat::WebP) {
        return Err(DecodeError::UnsupportedFormat("WebP".into()));
    }
    // Header only: reject before any pixel allocation.
    let (dw, dh) = reader
        .into_dimensions()
        .map_err(|e| DecodeError::Decode(e.to_string()))?;
    let too_large = || DecodeError::TooLarge {
        width: dw,
        height: dh,
        max_width: limits.max_width,
        max_height: limits.max_height,
    };
    let rgba_bytes = u64::from(dw) * u64::from(dh) * 4;
    if dw > limits.max_width || dh > limits.max_height || rgba_bytes > limits.max_alloc_bytes {
        return Err(too_large());
    }
    let mut reader = open()?;
    let mut lim = image::Limits::no_limits();
    lim.max_image_width = Some(limits.max_width);
    lim.max_image_height = Some(limits.max_height);
    lim.max_alloc = Some(limits.max_alloc_bytes);
    reader.limits(lim);
    let mut img = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => too_large(),
        e => DecodeError::Decode(e.to_string()),
    })?;
    if let Some((nw, nh, scale)) = target.and_then(|t| fit_dims(img.width(), img.height(), t)) {
        if scale < 0.5 {
            // Cheap Triangle pass to twice the final size, then Lanczos3 for quality.
            let iw = nw.saturating_mul(2).max(1);
            let ih = nh.saturating_mul(2).max(1);
            img = img.resize_exact(iw, ih, FilterType::Triangle);
        }
        img = img.resize_exact(nw, nh, FilterType::Lanczos3);
    }
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    log::debug!("network-image: decoded {width}x{height}");
    Ok(DecodedImage {
        pixels: rgba.into_raw(),
        width,
        height,
    })
}

/// Decode on the blocking pool, shared by [`decode_source`] and the loader.
pub(crate) async fn decode_shared(
    bytes: Arc<Vec<u8>>,
    target: Option<TargetPx>,
    limits: DecodeLimits,
) -> Result<ImageSource, DecodeError> {
    let joined = spawn_blocking(move || decode_and_fit(&bytes, target, limits)).await;
    match joined {
        Ok(Ok(d)) => Ok(ImageSource::from_rgba8(d.pixels, d.width, d.height)),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(DecodeError::Task(e.to_string())),
    }
}

/// Decode `bytes` on the blocking pool and fit them inside `target`, under
/// the default [`DecodeLimits`].
pub async fn decode_source(
    bytes: Vec<u8>,
    target: Option<TargetPx>,
) -> Result<ImageSource, DecodeError> {
    decode_shared(Arc::new(bytes), target, DecodeLimits::default()).await
}
