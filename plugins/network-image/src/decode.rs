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
    /// The blocking decode task did not complete.
    #[error("decode task failed: {0}")]
    Task(String),
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
) -> Result<DecodedImage, DecodeError> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| DecodeError::Decode(e.to_string()))?;
    #[cfg(not(feature = "webp"))]
    if reader.format() == Some(image::ImageFormat::WebP) {
        return Err(DecodeError::UnsupportedFormat("WebP".into()));
    }
    let mut img = reader
        .decode()
        .map_err(|e| DecodeError::Decode(e.to_string()))?;
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
) -> Result<ImageSource, DecodeError> {
    let joined = spawn_blocking(move || decode_and_fit(&bytes, target)).await;
    match joined {
        Ok(Ok(d)) => Ok(ImageSource::from_rgba8(d.pixels, d.width, d.height)),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(DecodeError::Task(e.to_string())),
    }
}

/// Decode `bytes` on the blocking pool and fit them inside `target`.
pub async fn decode_source(
    bytes: Vec<u8>,
    target: Option<TargetPx>,
) -> Result<ImageSource, DecodeError> {
    decode_shared(Arc::new(bytes), target).await
}
