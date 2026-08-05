//! External-consumer conformance for `frust_camera::barcode`.
//!
//! The exhaustive fixture matrix (dense/short payloads, module scale,
//! rotation, stride, contrast, negatives, format filter, and the
//! `decode_frame` `Yuv420`/`Bgra` paths) lives in the crate's own
//! `src/barcode/conformance.rs` (`#[cfg(test)]`-only, run as part of this
//! same `cargo test -p frust-camera`) — a `#[cfg(test)]` module compiles out
//! of the normally-built library this integration test links against, so it
//! can't be reached from here by path. This file instead builds its own
//! small fixture directly against the crate's genuinely public,
//! externally-constructible API (`barcode::decode_luma`/`LumaView`), the way
//! an external app actually consumes it.
//!
//! **`decode_frame` is not exercised here on purpose**: `ImageFrame`/
//! `ImagePlane` are `#[non_exhaustive]` structs with no public constructor —
//! only the crate's own Android/Apple platform backends build one, so
//! external code (including this file, compiled as a separate crate) cannot
//! construct one via struct-literal syntax (`E0639`). Its conformance
//! coverage lives in `src/barcode/conformance.rs` instead, which *is* inside
//! the crate.

use frust_camera::barcode::{BarcodeFormat, LumaView, decode_luma};
use qrcode::Color;

const PAYLOAD: &str = "https://frust.dev/camera";
const QUIET_MODULES: usize = 4;

/// Render `payload` to a tightly-packed luma buffer (`row_stride == width`,
/// `pixel_stride == 1`) at `module_px` pixels per module with a
/// [`QUIET_MODULES`]-module white quiet zone. Returns `(buffer, side_px)`.
fn render_qr_luma(payload: &str, module_px: usize) -> (Vec<u8>, usize) {
    let code = qrcode::QrCode::new(payload.as_bytes()).expect("payload encodes to a valid QR code");
    let modules = code.width();
    let colors = code.to_colors();

    let side_modules = modules + 2 * QUIET_MODULES;
    let side_px = side_modules * module_px;
    let mut buf = vec![255u8; side_px * side_px];

    for my in 0..modules {
        for mx in 0..modules {
            if colors[my * modules + mx] == Color::Dark {
                let px0 = (mx + QUIET_MODULES) * module_px;
                let py0 = (my + QUIET_MODULES) * module_px;
                for dy in 0..module_px {
                    for dx in 0..module_px {
                        let idx = (py0 + dy) * side_px + (px0 + dx);
                        buf[idx] = 0;
                    }
                }
            }
        }
    }
    (buf, side_px)
}

#[test]
fn decode_luma_finds_the_payload() {
    let (buf, side) = render_qr_luma(PAYLOAD, 4);
    let view = LumaView::new(&buf, side, side, side, 1).unwrap();

    let found = decode_luma(&view, &[]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].raw_value, PAYLOAD);
    assert_eq!(found[0].format, BarcodeFormat::QrCode);
    assert!(found[0].corners.is_some());
}

#[test]
fn decode_luma_format_filter_semantics() {
    let (buf, side) = render_qr_luma(PAYLOAD, 4);
    let view = LumaView::new(&buf, side, side, side, 1).unwrap();

    // Empty slice: "all supported" (mobile_scanner semantics).
    assert_eq!(decode_luma(&view, &[]).len(), 1);
    // Explicit, matching format list: still decodes.
    assert_eq!(decode_luma(&view, &[BarcodeFormat::QrCode]).len(), 1);
}

#[test]
fn decode_luma_returns_empty_for_a_blank_buffer() {
    let (_buf, side) = render_qr_luma(PAYLOAD, 4);
    let blank = vec![255u8; side * side];
    let view = LumaView::new(&blank, side, side, side, 1).unwrap();
    assert!(decode_luma(&view, &[]).is_empty());
}

#[test]
fn luma_view_rejects_an_undersized_buffer() {
    let too_small = vec![0u8; 3];
    assert!(LumaView::new(&too_small, 4, 4, 4, 1).is_err());
}
