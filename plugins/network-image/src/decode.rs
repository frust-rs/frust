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
    /// The declared dimensions exceed the configured [`DecodeLimits`]; nothing
    /// was decoded.
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
    /// The estimated decode-and-fit peak exceeds
    /// [`DecodeLimits::max_alloc_bytes`] (or the decoder hit its allocation cap).
    #[error(
        "image {width}x{height} would need about {estimated_peak_bytes} bytes to decode and fit, over the {max_alloc_bytes} byte budget"
    )]
    OverBudget {
        /// Declared image width in pixels.
        width: u32,
        /// Declared image height in pixels.
        height: u32,
        /// Peak transient bytes estimated from the header.
        estimated_peak_bytes: u64,
        /// Configured peak budget.
        max_alloc_bytes: u64,
    },
    /// The blocking decode task did not complete.
    #[error("decode task failed: {0}")]
    Task(String),
}

/// Bounds on what one decode may allocate.
///
/// The declared dimensions and bit depth are read from the header and checked
/// before any pixel is decoded, and `image`'s own [`image::Limits`] are set
/// from the same values as a safety net.
///
/// `max_alloc_bytes` is the budget for one decode-and-fit's peak transient
/// allocation, estimated from the header: the native decoded bytes plus either
/// the RGBA copy (no resize) or the resize working set (an `f32` RGBA
/// intermediate of source width x pass height x 16 bytes, plus the pass
/// output). The default (8192 px per side, 512 MiB) admits an 8192x8192 RGBA8
/// image with no resize, or a 48 MP photo fitted to a phone-sized target. The
/// bound is per decode; concurrent decodes add up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeLimits {
    /// Maximum declared width in pixels (default 8192).
    pub max_width: u32,
    /// Maximum declared height in pixels (default 8192).
    pub max_height: u32,
    /// Peak decode-and-fit allocation budget in bytes (default 512 MiB).
    pub max_alloc_bytes: u64,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_width: 8192,
            max_height: 8192,
            max_alloc_bytes: 512 * 1024 * 1024,
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

/// Dimensions of the first resize pass for a shrink to `(nw, nh)` at `scale`:
/// twice the final size below half scale (a cheap Triangle pass precedes the
/// Lanczos3 pass), else the final size.
fn first_pass_dims(nw: u32, nh: u32, scale: f64) -> (u32, u32) {
    if scale < 0.5 {
        (nw.saturating_mul(2).max(1), nh.saturating_mul(2).max(1))
    } else {
        (nw, nh)
    }
}

/// Peak transient bytes of decoding a `dw` x `dh` image of `native_bpp` bytes
/// per pixel and fitting it to `target`, estimated from the header.
pub(crate) fn peak_estimate(dw: u32, dh: u32, native_bpp: u64, target: Option<TargetPx>) -> u64 {
    let (w, h) = (u64::from(dw), u64::from(dh));
    let native = w.saturating_mul(h).saturating_mul(native_bpp);
    match target.and_then(|t| fit_dims(dw, dh, t)) {
        None => native.saturating_add(w.saturating_mul(h).saturating_mul(4)),
        Some((nw, nh, scale)) => {
            let (pw, ph) = first_pass_dims(nw, nh, scale);
            let (pw, ph) = (u64::from(pw), u64::from(ph));
            let vertical = w.saturating_mul(ph).saturating_mul(16);
            let output = pw.saturating_mul(ph).saturating_mul(native_bpp);
            native.saturating_add(vertical).saturating_add(output)
        }
    }
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
    let decoder = reader
        .into_decoder()
        .map_err(|e| DecodeError::Decode(e.to_string()))?;
    let (dw, dh) = image::ImageDecoder::dimensions(&decoder);
    let native_bpp = u64::from(image::ImageDecoder::color_type(&decoder).bytes_per_pixel());
    drop(decoder);
    if dw > limits.max_width || dh > limits.max_height {
        return Err(DecodeError::TooLarge {
            width: dw,
            height: dh,
            max_width: limits.max_width,
            max_height: limits.max_height,
        });
    }
    let over_budget = |estimated_peak_bytes| DecodeError::OverBudget {
        width: dw,
        height: dh,
        estimated_peak_bytes,
        max_alloc_bytes: limits.max_alloc_bytes,
    };
    let estimate = peak_estimate(dw, dh, native_bpp, target);
    if estimate > limits.max_alloc_bytes {
        return Err(over_budget(estimate));
    }
    let mut reader = open()?;
    let mut lim = image::Limits::no_limits();
    lim.max_image_width = Some(limits.max_width);
    lim.max_image_height = Some(limits.max_height);
    lim.max_alloc = Some(limits.max_alloc_bytes);
    reader.limits(lim);
    let mut img = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => over_budget(peak_estimate(dw, dh, native_bpp, None)),
        e => DecodeError::Decode(e.to_string()),
    })?;
    if let Some((nw, nh, scale)) = target.and_then(|t| fit_dims(img.width(), img.height(), t)) {
        let (pw, ph) = first_pass_dims(nw, nh, scale);
        if scale < 0.5 {
            // Cheap Triangle pass to twice the final size, then Lanczos3 for quality.
            img = img.resize_exact(pw, ph, FilterType::Triangle);
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

#[cfg(test)]
mod tests {
    use super::*;

    const NATIVE_8K: u64 = 8192 * 8192 * 4;

    #[test]
    fn estimate_counts_the_resize_working_set() {
        let t = Some(TargetPx {
            width: 2560,
            height: 2560,
        });
        assert_eq!(
            peak_estimate(8192, 8192, 4, t),
            NATIVE_8K + 8192 * 5120 * 16 + 5120 * 5120 * 4
        );
    }

    #[test]
    fn estimate_without_a_resize_adds_the_rgba_copy() {
        assert_eq!(peak_estimate(8192, 8192, 4, None), 2 * NATIVE_8K);
    }

    #[test]
    fn estimate_uses_the_native_depth() {
        let t = Some(TargetPx {
            width: 300,
            height: 300,
        });
        assert_eq!(
            peak_estimate(1200, 1200, 3, t),
            1200 * 1200 * 3 + 1200 * 600 * 16 + 600 * 600 * 3
        );
    }
}
