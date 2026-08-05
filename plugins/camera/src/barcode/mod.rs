//! Platform-independent QR/barcode decoding — luma buffers and
//! [`crate::ImageFrame`]s in, [`Barcode`]s out, over a private engine seam
//! ([`engine`], `rqrr` the sole v1 implementation). No camera involved: a
//! desktop app can synthesize its own [`LumaView`] and call [`decode_luma`]
//! directly, which is exactly how this module is unit-tested (see
//! [`conformance`] — `#[cfg(test)]`-only).
//!
//! # Formats
//!
//! [`BarcodeFormat`] is deliberately general-shaped even though the v1
//! engine only ever produces [`BarcodeFormat::QrCode`] — it is
//! `#[non_exhaustive]` so a later format lands as an additive variant, not a
//! breaking one. This does **not** speculatively enumerate a wider format
//! vocabulary (mobile_scanner's ~23) ahead of a real second engine.
//!
//! # Close-deadline interaction
//!
//! [`decode_frame`] is synchronous and does no I/O — safe to call from
//! inside [`crate::CameraSession::start_image_stream`]'s callback despite
//! that callback's own close-deadline contract ([`crate::ImageFrame`]'s
//! doc): [`decode_luma`]'s zero-copy path (`Yuv420`) never outlives the
//! callback, and the `Bgra` path's one scratch conversion buffer is
//! allocated and dropped entirely within the call.
//!
//! # Streaming composition
//!
//! [`crate::CameraSession::start_barcode_stream`] is the one entry point
//! that *does* touch the camera: it composes [`decode_frame`] with a live
//! image stream and a [`DetectionPolicy`] (empty/duplicate-detection
//! policy) — see [`stream`] for the composition itself, and
//! [`crate::CameraError::StreamBusy`] for how it shares the session's
//! single image stream with [`crate::CameraSession::start_image_stream`].

mod engine;
mod luma;
pub mod stream;

#[cfg(test)]
mod conformance;

pub use luma::{LumaView, LumaViewError};
pub use stream::DetectionPolicy;

use crate::{ImageFormat, ImageFrame};
use engine::{BarcodeEngine, RqrrEngine};

/// A decodable barcode symbology.
///
/// `#[non_exhaustive]`: v1 decodes QR only, but a later engine (PDF417,
/// Code128, …) adds a variant here rather than widening some other type.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BarcodeFormat {
    /// QR Code (Model 2). The only format the v1 engine decodes.
    QrCode,
}

/// One point of a [`Barcode::corners`] detection quad, in frame pixel
/// coordinates (pre-rotation — the same coordinate space
/// [`crate::ImageFrame::rotation_degrees`] describes, not yet corrected for
/// it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarcodePoint {
    /// X coordinate, in pixels.
    pub x: i32,
    /// Y coordinate, in pixels.
    pub y: i32,
}

/// A single decoded barcode.
#[derive(Clone, Debug)]
pub struct Barcode {
    /// Decoded textual payload (lossy UTF-8 where the payload is binary —
    /// v1's QR-only engine always decodes a valid UTF-8 string, so this is
    /// never actually lossy today; the field is typed for a future
    /// binary-payload format).
    pub raw_value: String,
    /// Raw decoded bytes when they differ from `raw_value`'s UTF-8 (else
    /// `None`). Always `None` in v1 — `rqrr` only ever hands back a
    /// UTF-8-validated `String`.
    pub raw_bytes: Option<Vec<u8>>,
    /// Which [`BarcodeFormat`] this decode matched.
    pub format: BarcodeFormat,
    /// Detection quad, clockwise from top-left, in frame coordinates
    /// (pre-rotation). `None` when the engine can't report bounds.
    pub corners: Option<[BarcodePoint; 4]>,
}

/// Options for [`crate::CameraSession::start_barcode_stream`].
///
/// `Default`: `formats` empty ("all supported" — matching [`decode_luma`]'s
/// own format-filter convention) and `detection` at [`DetectionPolicy`]'s
/// own default (`Throttled` at its default interval).
#[derive(Debug, Clone, Default)]
pub struct BarcodeStreamOptions {
    /// Which [`BarcodeFormat`]s to decode — an empty `Vec` means "all
    /// supported" (mobile_scanner semantics, the same convention
    /// [`decode_luma`]/[`decode_frame`] use).
    pub formats: Vec<BarcodeFormat>,
    /// When `on_detect` fires, relative to the underlying decode rate — see
    /// [`DetectionPolicy`].
    pub detection: DetectionPolicy,
}

/// Scan `luma` for every barcode matching `formats` (an empty slice means
/// "all supported" — mobile_scanner semantics; a non-empty slice that
/// doesn't include a format this engine supports returns `vec![]` without
/// scanning). Never errors: no detection is an empty `Vec`, not a `Result`.
///
/// Works standalone on any host, including this desktop's `cargo test` — no
/// camera, no platform backend.
pub fn decode_luma(luma: &LumaView<'_>, formats: &[BarcodeFormat]) -> Vec<Barcode> {
    RqrrEngine.decode(luma, formats)
}

/// Scan one [`crate::ImageFrame`] for every barcode matching `formats` (see
/// [`decode_luma`]'s format-filter semantics).
///
/// [`ImageFormat::Yuv420`] decodes zero-copy via [`LumaView::from_frame`].
/// [`ImageFormat::Bgra`] has no borrowed luma view (BGRA has no luma plane
/// to borrow), so this computes one scratch luma buffer first (integer
/// approximation `(77*R + 150*G + 29*B) >> 8`; BGRA's byte order puts `B` at
/// offset `0` within each pixel) and decodes that. Both paths are
/// synchronous and allocate nothing that outlives this call — safe to call
/// from inside [`crate::CameraSession::start_image_stream`]'s callback (see
/// this module's doc, *Close-deadline interaction*).
pub fn decode_frame(frame: &ImageFrame<'_>, formats: &[BarcodeFormat]) -> Vec<Barcode> {
    match frame.format {
        ImageFormat::Yuv420 => match LumaView::from_frame(frame) {
            Some(luma) => decode_luma(&luma, formats),
            None => Vec::new(),
        },
        ImageFormat::Bgra => {
            let Some(plane) = frame.planes.first() else {
                return Vec::new();
            };
            let width = frame.width as usize;
            let height = frame.height as usize;
            if width == 0 || height == 0 || plane.pixel_stride == 0 {
                return Vec::new();
            }

            let mut luma_buf = vec![0u8; width * height];
            for y in 0..height {
                let row_start = y * plane.row_stride;
                for x in 0..width {
                    let pixel_start = row_start + x * plane.pixel_stride;
                    // `pixel_start + 2` is the red byte (B, G, R, ..): bail
                    // out on a short/malformed row rather than panicking.
                    let Some(&b) = plane.data.get(pixel_start) else {
                        continue;
                    };
                    let Some(&g) = plane.data.get(pixel_start + 1) else {
                        continue;
                    };
                    let Some(&r) = plane.data.get(pixel_start + 2) else {
                        continue;
                    };
                    let luma = (77 * r as u32 + 150 * g as u32 + 29 * b as u32) >> 8;
                    luma_buf[y * width + x] = luma as u8;
                }
            }

            match LumaView::new(&luma_buf, width, height, width, 1) {
                Ok(luma) => decode_luma(&luma, formats),
                Err(_) => Vec::new(),
            }
        }
    }
}
