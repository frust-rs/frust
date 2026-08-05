//! Fixture-based decode conformance for [`super::decode_luma`]/
//! [`super::decode_frame`].
//!
//! `#[cfg(test)]`-only, mirroring `frust-shared-preferences`/
//! `frust-secure-storage`'s `conformance.rs` shape (reusable fixture +
//! assertion helpers) — adapted for the one real difference here: there is
//! no per-platform backend that can't run on this host, so the suite lives
//! entirely as `#[test]` functions in this one module rather than being
//! invoked from a separate backend test file.
//!
//! Every fixture is synthesized in-process via the `qrcode` dev-dependency
//! (module matrix in, scaled/rotated/strided luma buffer out) — no binary
//! image assets, no `image` crate (version-pinned exactly elsewhere and
//! unneeded here).
//!
//! **Dev-dependency visibility note**: `qrcode` only resolves for a `cargo
//! test`/`cargo bench` build, so this module is `#[cfg(test)]`-gated and
//! compiled out of a plain `cargo build -p frust-camera` entirely. That also
//! means `plugins/camera/tests/barcode_conformance.rs` — an external
//! integration test, linking against a normally-built copy of this library
//! with `#[cfg(test)]` code compiled out — cannot reach into this module by
//! path; it instead builds its own small fixture directly against the
//! crate's public API, confirming the same `decode_frame`/`decode_luma`
//! behavior the way an external app would consume it.

use qrcode::Color;

use super::{Barcode, BarcodeFormat, LumaView, decode_frame, decode_luma};
use crate::{ImageFormat, ImageFrame, ImagePlane};

/// `muxr`'s pairing-QR payload shape (the dense first-consumer case) — a
/// 43-char base64url token and 64-char hex fingerprint, fixed-format fields
/// per the pairing protocol, giving a 176-char total.
const PAYLOAD_DENSE: &str = "muxr://pair?v=2&h=192.168.1.100&p=50051&t=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ&fp=\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef&tm=pin&n=Test%20Server";
const PAYLOAD_SHORT: &str = "HELLO";

/// White quiet zone width, in modules — the QR spec's minimum is 4.
const QUIET_MODULES: usize = 4;

/// `colors[y*modules+x]`'s module color at `(x, y)`, rotated by
/// `quarter_turns` quarter turns (0..=3, clockwise) — rotating the *module*
/// grid before scaling to pixels keeps the quiet zone symmetric either way.
fn dark_at_rotated(
    colors: &[Color],
    modules: usize,
    quarter_turns: u8,
    x: usize,
    y: usize,
) -> bool {
    let (rx, ry) = match quarter_turns % 4 {
        0 => (x, y),
        1 => (y, modules - 1 - x),
        2 => (modules - 1 - x, modules - 1 - y),
        3 => (modules - 1 - y, x),
        _ => unreachable!("quarter_turns % 4 is always 0..=3"),
    };
    colors[ry * modules + rx] == Color::Dark
}

/// Render `payload`'s QR code to a tightly-packed luma buffer (`row_stride
/// == width`, `pixel_stride == 1`, dark module `0`/light module `255`) at
/// `module_px` pixels per module, with a [`QUIET_MODULES`]-module white
/// quiet zone, optionally rotated by `quarter_turns` quarter turns.
/// Returns `(buffer, side_px)` — the buffer is always square.
///
/// `pub(super)`: `barcode::stream`'s own replay tests reuse this builder
/// rather than duplicating QR-fixture synthesis (`docs/DEVELOPMENT.md`'s
/// task notes) — visible to `barcode` and its descendant modules, which
/// covers the sibling `stream` module.
pub(super) fn render_qr_luma(
    payload: &str,
    module_px: usize,
    quarter_turns: u8,
) -> (Vec<u8>, usize) {
    let code = qrcode::QrCode::new(payload.as_bytes()).expect("payload encodes to a valid QR code");
    let modules = code.width();
    let colors = code.to_colors();

    let side_modules = modules + 2 * QUIET_MODULES;
    let side_px = side_modules * module_px;
    let mut buf = vec![255u8; side_px * side_px];

    for my in 0..modules {
        for mx in 0..modules {
            if dark_at_rotated(&colors, modules, quarter_turns, mx, my) {
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

/// Decode a tightly-packed square luma buffer built by [`render_qr_luma`]
/// (or a same-shaped negative fixture).
fn decode_tight(buf: &[u8], side: usize, formats: &[BarcodeFormat]) -> Vec<Barcode> {
    let view = LumaView::new(buf, side, side, side, 1)
        .expect("tight buffer matches its own declared geometry");
    decode_luma(&view, formats)
}

#[test]
fn decodes_dense_and_short_payloads_at_multiple_module_scales() {
    for payload in [PAYLOAD_DENSE, PAYLOAD_SHORT] {
        for module_px in [4usize, 8usize] {
            let (buf, side) = render_qr_luma(payload, module_px, 0);
            let found = decode_tight(&buf, side, &[]);
            assert_eq!(
                found.len(),
                1,
                "payload {payload:?} at {module_px}px/module"
            );
            assert_eq!(found[0].raw_value, payload);
            assert_eq!(found[0].format, BarcodeFormat::QrCode);
            assert!(found[0].raw_bytes.is_none());
            assert!(
                found[0].corners.is_some(),
                "a real detection reports its quad"
            );
        }
    }
}

#[test]
fn decodes_at_90_and_180_degree_rotation() {
    // Rotational symmetry is a QR design property (3 of 4 corners carry a
    // finder pattern), not something the decode path fakes — the *streamed*
    // frame is portrait-hardcoded, but the code itself must still decode
    // regardless of how the camera happened to be held.
    for turns in [1u8, 2] {
        let (buf, side) = render_qr_luma(PAYLOAD_SHORT, 4, turns);
        let found = decode_tight(&buf, side, &[]);
        assert_eq!(found.len(), 1, "{turns} quarter-turn rotation");
        assert_eq!(found[0].raw_value, PAYLOAD_SHORT);
    }
}

#[test]
fn decodes_with_a_wider_row_stride() {
    let (tight, side) = render_qr_luma(PAYLOAD_SHORT, 4, 0);
    let row_stride = side + 17; // arbitrary padding beyond the image width
    let mut wide = vec![255u8; row_stride * side];
    for y in 0..side {
        wide[y * row_stride..y * row_stride + side]
            .copy_from_slice(&tight[y * side..y * side + side]);
    }
    let view = LumaView::new(&wide, side, side, row_stride, 1).unwrap();
    let found = decode_luma(&view, &[]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].raw_value, PAYLOAD_SHORT);
}

#[test]
fn decodes_with_an_interleaved_pixel_stride_of_two() {
    let (tight, side) = render_qr_luma(PAYLOAD_SHORT, 4, 0);
    // Every luma byte followed by one filler byte from a second, unrelated
    // channel — pins the Android-stride defensiveness (`LumaView` indexes by
    // `pixel_stride` unconditionally, never assumes a tightly-packed row).
    let mut interleaved = vec![0u8; tight.len() * 2];
    for (i, &luma) in tight.iter().enumerate() {
        interleaved[i * 2] = luma;
        interleaved[i * 2 + 1] = 0xAA;
    }
    let view = LumaView::new(&interleaved, side, side, side * 2, 2).unwrap();
    let found = decode_luma(&view, &[]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].raw_value, PAYLOAD_SHORT);
}

#[test]
fn decodes_under_compressed_contrast() {
    let (tight, side) = render_qr_luma(PAYLOAD_SHORT, 4, 0);
    let (low, high) = (80u32, 160u32);
    let compressed: Vec<u8> = tight
        .iter()
        .map(|&v| (low + (v as u32 * (high - low)) / 255) as u8)
        .collect();
    let found = decode_tight(&compressed, side, &[]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].raw_value, PAYLOAD_SHORT);
}

#[test]
fn negatives_return_no_barcodes() {
    let (_tight, side) = render_qr_luma(PAYLOAD_SHORT, 4, 0);

    let uniform_gray = vec![128u8; side * side];
    assert!(
        decode_tight(&uniform_gray, side, &[]).is_empty(),
        "uniform gray"
    );

    let blank = vec![0u8; side * side];
    assert!(
        decode_tight(&blank, side, &[]).is_empty(),
        "blank (all-black) frame"
    );

    // A seeded LCG (Knuth/MMIX's 64-bit constants) rather than a `rand`
    // dependency — deterministic, reproducible pseudo-random noise.
    let mut state: u64 = 0xC0FFEE;
    let noise: Vec<u8> = (0..side * side)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 56) as u8
        })
        .collect();
    assert!(
        decode_tight(&noise, side, &[]).is_empty(),
        "deterministic noise"
    );
}

#[test]
fn format_filter_semantics() {
    let (tight, side) = render_qr_luma(PAYLOAD_SHORT, 4, 0);

    // `&[]` means "all supported" (mobile_scanner semantics).
    assert_eq!(decode_tight(&tight, side, &[]).len(), 1);
    // An explicit, matching format list still decodes.
    assert_eq!(
        decode_tight(&tight, side, &[BarcodeFormat::QrCode]).len(),
        1
    );

    // The spec also calls for a `formats` slice that *excludes* `QrCode`
    // returning `vec![]` on a decodable fixture. `BarcodeFormat` is
    // `#[non_exhaustive]` but v1 defines exactly one variant, so no such
    // slice can be constructed with a real value today — `RqrrEngine::decode`
    // implements the short-circuit (`!formats.is_empty() &&
    // !formats.contains(&QrCode)`) per spec, verifiable by inspection until
    // a second format ships and this test can build one.
}

#[test]
fn decode_frame_yuv420_matches_decode_luma() {
    let (tight, side) = render_qr_luma(PAYLOAD_SHORT, 4, 0);
    let plane = ImagePlane {
        data: &tight,
        row_stride: side,
        pixel_stride: 1,
    };
    let frame = ImageFrame {
        format: ImageFormat::Yuv420,
        width: side as u32,
        height: side as u32,
        rotation_degrees: 0,
        planes: std::slice::from_ref(&plane),
    };

    let found = decode_frame(&frame, &[]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].raw_value, PAYLOAD_SHORT);
}

#[test]
fn decode_frame_bgra_converts_and_decodes() {
    let (tight, side) = render_qr_luma(PAYLOAD_SHORT, 4, 0);
    // Gray BGRA (B == G == R == luma) so the integer luma approximation
    // round-trips back to the original value exactly.
    let mut bgra = vec![0u8; tight.len() * 4];
    for (i, &luma) in tight.iter().enumerate() {
        bgra[i * 4] = luma; // B
        bgra[i * 4 + 1] = luma; // G
        bgra[i * 4 + 2] = luma; // R
        bgra[i * 4 + 3] = 0xFF; // A
    }
    let plane = ImagePlane {
        data: &bgra,
        row_stride: side * 4,
        pixel_stride: 4,
    };
    let frame = ImageFrame {
        format: ImageFormat::Bgra,
        width: side as u32,
        height: side as u32,
        rotation_degrees: 0,
        planes: std::slice::from_ref(&plane),
    };

    let found = decode_frame(&frame, &[]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].raw_value, PAYLOAD_SHORT);
}
