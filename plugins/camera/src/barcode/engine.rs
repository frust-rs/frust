//! The private decode-engine seam — `rqrr` is the sole v1 implementation.
//!
//! Kept `pub(crate)` and un-re-exported on purpose: this is the pivot point
//! if a later device gate finds `rqrr` wanting (accuracy, speed, format
//! coverage) — swapping the engine never touches [`super::decode_luma`]/
//! [`super::decode_frame`]'s public signatures.

use super::{Barcode, BarcodeFormat, BarcodePoint, LumaView};

/// One QR/barcode decode engine. `decode` never errors — a failed
/// detection/decode is reported as an empty `Vec`, not a `Result`, matching
/// [`super::decode_luma`]'s "best-effort scan" contract.
pub(crate) trait BarcodeEngine {
    /// Scan `luma` for every barcode in `formats` (an empty slice means
    /// "all supported").
    fn decode(&mut self, luma: &LumaView<'_>, formats: &[BarcodeFormat]) -> Vec<Barcode>;
}

/// The sole v1 engine: pure-Rust QR detection/decoding via `rqrr`.
pub(crate) struct RqrrEngine;

impl BarcodeEngine for RqrrEngine {
    fn decode(&mut self, luma: &LumaView<'_>, formats: &[BarcodeFormat]) -> Vec<Barcode> {
        // An empty `formats` means "all supported" (mobile_scanner
        // semantics); a non-empty list that doesn't ask for `QrCode` — the
        // only format this engine has — short-circuits to no results
        // without ever touching `rqrr`.
        if !formats.is_empty() && !formats.contains(&BarcodeFormat::QrCode) {
            return Vec::new();
        }

        // The closure form consumes any stride: `rqrr` calls it once per
        // pixel while building its own internal buffer, so `luma`'s
        // `pixel_stride`/`row_stride` never has to be packed down first.
        let mut prepared =
            rqrr::PreparedImage::prepare_from_greyscale(luma.width(), luma.height(), |x, y| {
                luma.get(x, y)
            });

        prepared
            .detect_grids()
            .iter()
            .filter_map(|grid| {
                // A failed grid is "no detection", not an API error — the
                // caller never sees the decode failure, only that this grid
                // produced nothing. `decode_to` rather than `decode`: the
                // latter fails the whole grid on a payload that isn't UTF-8,
                // which would drop a valid byte-mode code.
                let mut bytes = Vec::new();
                grid.decode_to(&mut bytes).ok()?;
                let (raw_value, raw_bytes) = split_payload(bytes);
                Some(Barcode {
                    raw_value,
                    raw_bytes,
                    format: BarcodeFormat::QrCode,
                    // `grid.bounds` is documented `[top-left, top-right,
                    // bottom-right, bottom-left]` — already clockwise from
                    // top-left, matching our contract with no reordering.
                    corners: Some(grid.bounds.map(to_point)),
                })
            })
            .collect()
    }
}

/// A decoded payload as `(raw_value, raw_bytes)`: a UTF-8 payload is the
/// string alone; anything else keeps its exact bytes in `raw_bytes` next to
/// a lossy `raw_value`.
fn split_payload(bytes: Vec<u8>) -> (String, Option<Vec<u8>>) {
    match String::from_utf8(bytes) {
        Ok(text) => (text, None),
        Err(err) => {
            let bytes = err.into_bytes();
            (String::from_utf8_lossy(&bytes).into_owned(), Some(bytes))
        }
    }
}

fn to_point(p: rqrr::Point) -> BarcodePoint {
    BarcodePoint { x: p.x, y: p.y }
}
