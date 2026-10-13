//! Every payload x ECC level x module size must decode back to itself with rqrr.

use frust_qr::{EccLevel, encode};

const QUIET: usize = 4;

fn payloads() -> Vec<String> {
    let mut out: Vec<String> = [
        "https://frust.dev",
        "https://example.com/booking/BK-2026-000142?lang=th&ref=app",
        "https://example.com/a/very/long/path/with/many/segments?query=value&other=another#fragment",
        "frust",
        "0",
        "สวัสดีครับ ยินดีต้อนรับ",
        "การจองของคุณได้รับการยืนยันแล้ว",
        "こんにちは世界",
        "予約が確定しました",
        "日本語とEnglishとไทยのmixed text",
        "BEGIN:VCARD\nVERSION:3.0\nFN:Frust\nEND:VCARD",
        "WIFI:T:WPA;S:guest;P:correct horse;;",
        "mailto:hello@example.com?subject=Hi",
        "emoji \u{1F3AB} ticket",
        "line one\nline two\r\nline three",
        "  leading and trailing spaces  ",
        "{\"id\":42,\"seat\":\"12A\",\"ok\":true}",
        "1234567890123456789012345678901234567890",
        "HELLO WORLD UPPERCASE ALPHANUMERIC 123",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // 200 ASCII bytes.
    out.push((0..200).map(|i| (b'a' + (i % 26) as u8) as char).collect());
    out
}

fn decode(payload: &str, ecc: EccLevel, module_px: usize) -> String {
    let m = encode(payload.as_bytes(), ecc).expect("encodes");
    let side = (m.width() + 2 * QUIET) * module_px;
    let img = rqrr_image(&m, side, module_px);
    let mut prepared =
        rqrr::PreparedImage::prepare_from_greyscale(side, side, |x, y| img[y * side + x]);
    let grids = prepared.detect_grids();
    assert_eq!(
        grids.len(),
        1,
        "one grid for {payload:?} {ecc:?} {module_px}px"
    );
    let (_, content) = grids[0].decode().expect("decodes");
    content
}

fn rqrr_image(m: &frust_qr::QrMatrix, side: usize, module_px: usize) -> Vec<u8> {
    let mut buf = vec![255u8; side * side];
    for y in 0..side {
        for x in 0..side {
            let (mx, my) = (x / module_px, y / module_px);
            if mx >= QUIET && my >= QUIET && m.is_dark(mx - QUIET, my - QUIET) {
                buf[y * side + x] = 0;
            }
        }
    }
    buf
}

#[test]
fn round_trip_matrix_decodes_every_case() {
    let payloads = payloads();
    assert_eq!(payloads.len(), 20);
    let mut cases = 0;
    for payload in &payloads {
        for ecc in [EccLevel::L, EccLevel::M, EccLevel::Q, EccLevel::H] {
            for module_px in [4, 8, 12] {
                assert_eq!(&decode(payload, ecc, module_px), payload);
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 240);
}
