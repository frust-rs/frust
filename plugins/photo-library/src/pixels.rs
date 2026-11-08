//! Pixel-buffer helpers shared by the platform arms: alpha and byte-order fixes that turn a
//! decoded platform bitmap into the straight-alpha RGBA8 the widgets expect.

/// Converts premultiplied RGBA8 to straight alpha, in place.
///
/// `a == 0` zeroes the colour channels; `a == 255` is unchanged; other values round to nearest
/// and saturate at 255. A trailing partial pixel is left untouched.
pub(crate) fn unpremultiply_rgba8_in_place(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        match a {
            255 => {}
            0 => px[..3].fill(0),
            _ => {
                for c in &mut px[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
}

/// Swaps the red and blue channels of 4-byte pixels, in place (BGRA <-> RGBA).
pub(crate) fn bgra_to_rgba_in_place(pixels: &mut [u8]) {
    for px in pixels.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
}

/// Copies a strided bitmap into a tightly packed `width * height * 4` buffer.
///
/// `stride` is the byte distance between row starts. Bytes missing from a short `src` (or a
/// `stride` narrower than a row) read as zero, so the result always has the exact length.
pub(crate) fn repack_rows(src: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
    let row = width * 4;
    let mut out = vec![0u8; row * height];
    for y in 0..height {
        let start = y * stride;
        let take = row.min(stride);
        let Some(available) = src.get(start..) else {
            break;
        };
        let n = take.min(available.len());
        out[y * row..y * row + n].copy_from_slice(&available[..n]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpremultiply_edges() {
        let mut px = [10, 20, 30, 0, 1, 2, 3, 255];
        unpremultiply_rgba8_in_place(&mut px);
        assert_eq!(px, [0, 0, 0, 0, 1, 2, 3, 255]);
    }

    #[test]
    fn unpremultiply_rounds_to_nearest() {
        // 100 * 255 / 128 = 199.2 -> 199; 1 * 255 / 2 = 127.5 -> 128.
        let mut px = [100, 1, 0, 128];
        unpremultiply_rgba8_in_place(&mut px);
        assert_eq!(px, [199, 2, 0, 128]);
        // An out-of-range premultiplied channel saturates.
        let mut px = [200, 0, 0, 100];
        unpremultiply_rgba8_in_place(&mut px);
        assert_eq!(px[0], 255);
    }

    #[test]
    fn unpremultiply_ignores_partial_pixel() {
        let mut px = [1, 2, 3, 4, 9, 9];
        unpremultiply_rgba8_in_place(&mut px);
        assert_eq!(&px[4..], &[9, 9]);
    }

    #[test]
    fn bgra_swaps_red_and_blue() {
        let mut px = [1, 2, 3, 4, 5, 6, 7, 8];
        bgra_to_rgba_in_place(&mut px);
        assert_eq!(px, [3, 2, 1, 4, 7, 6, 5, 8]);
    }

    #[test]
    fn repack_drops_row_padding() {
        // 2x2 pixels, stride 12 (4 bytes of padding per row).
        let src: Vec<u8> = (0..24).collect();
        let out = repack_rows(&src, 2, 2, 12);
        assert_eq!(out.len(), 16);
        assert_eq!(&out[..8], &src[..8]);
        assert_eq!(&out[8..], &src[12..20]);
    }

    #[test]
    fn repack_tolerates_a_short_source() {
        let src = [7u8; 10];
        let out = repack_rows(&src, 2, 2, 8);
        assert_eq!(out.len(), 16);
        assert_eq!(&out[..8], &[7; 8]);
        assert_eq!(&out[8..10], &[7, 7]);
        assert_eq!(&out[10..], &[0; 6]);
        assert!(repack_rows(&[], 2, 0, 8).is_empty());
    }
}
