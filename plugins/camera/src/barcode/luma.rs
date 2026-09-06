//! [`LumaView`]: a borrowed, stride-aware grayscale pixel view.

use crate::{ImageFormat, ImageFrame};

/// A borrowed, stride-aware grayscale (luma) pixel view over someone else's
/// buffer — the shape [`super::decode_luma`] and every decode engine behind
/// it consume, so a caller (a synthesized fixture, a [`crate::ImageFrame`]
/// plane) never has to repack pixels into a tightly-packed buffer first.
///
/// # Stride law
///
/// Every accessor indexes by `pixel_stride` unconditionally — never assumes
/// tightly-packed pixels. iOS's Y-plane `pixel_stride` is always `1`
/// (published), but Android forwards CameraX's `Plane.pixelStride` value
/// unverified, so `1` is a fast path ([`Self::row`]), never an assumption
/// baked into [`Self::get`].
#[derive(Debug, Clone, Copy)]
pub struct LumaView<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
    row_stride: usize,
    pixel_stride: usize,
}

/// [`LumaView::new`] rejects a buffer too small for the declared geometry —
/// a typed error (over an `Option`) so a caller can report exactly which
/// dimension/stride was inconsistent with the buffer it was handed, rather
/// than a bare `None`.
#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LumaViewError {
    /// `width` or `height` was `0` — no pixel could ever be addressed.
    #[error("luma view must be at least 1x1, got {width}x{height}")]
    EmptyDimensions {
        /// The requested width, in pixels.
        width: usize,
        /// The requested height, in pixels.
        height: usize,
    },
    /// The last addressable pixel — `(height-1)*row_stride +
    /// (width-1)*pixel_stride` — falls outside `data`, or the geometry
    /// overflows `usize` computing it.
    #[error(
        "luma buffer too small: {width}x{height} at row_stride={row_stride}/pixel_stride={pixel_stride} \
         needs at least {needed} bytes, got {got}"
    )]
    BufferTooSmall {
        /// The requested width, in pixels.
        width: usize,
        /// The requested height, in pixels.
        height: usize,
        /// Bytes between the start of consecutive rows.
        row_stride: usize,
        /// Bytes between consecutive pixels within a row.
        pixel_stride: usize,
        /// The minimum buffer length the geometry needs.
        needed: usize,
        /// The buffer length actually supplied.
        got: usize,
    },
}

impl<'a> LumaView<'a> {
    /// Build a view over `data`, validating that every pixel `(x, y)` for
    /// `x in 0..width` / `y in 0..height` addresses a byte actually inside
    /// `data` — i.e. that `(height-1)*row_stride + (width-1)*pixel_stride <
    /// data.len()`.
    ///
    /// # Errors
    /// [`LumaViewError::EmptyDimensions`] if `width` or `height` is `0`;
    /// [`LumaViewError::BufferTooSmall`] if `data` is too short for the
    /// declared geometry (including a `usize` overflow computing it).
    pub fn new(
        data: &'a [u8],
        width: usize,
        height: usize,
        row_stride: usize,
        pixel_stride: usize,
    ) -> Result<Self, LumaViewError> {
        if width == 0 || height == 0 {
            return Err(LumaViewError::EmptyDimensions { width, height });
        }

        let last_row_offset = (height - 1).checked_mul(row_stride);
        let last_col_offset = (width - 1).checked_mul(pixel_stride);
        let last_pixel_offset = last_row_offset
            .zip(last_col_offset)
            .and_then(|(r, c)| r.checked_add(c));

        // `last_offset` is the last valid pixel's *index*; `None` means the
        // geometry itself overflowed `usize` computing it — either way,
        // report the byte count (`index + 1`) a caller would actually need
        // to allocate, or `usize::MAX` when even that overflows.
        let too_small = |last_offset: Option<usize>| LumaViewError::BufferTooSmall {
            width,
            height,
            row_stride,
            pixel_stride,
            needed: last_offset
                .and_then(|n| n.checked_add(1))
                .unwrap_or(usize::MAX),
            got: data.len(),
        };

        match last_pixel_offset {
            Some(offset) if offset < data.len() => {}
            other => return Err(too_small(other)),
        }

        Ok(Self {
            data,
            width,
            height,
            row_stride,
            pixel_stride,
        })
    }

    /// The view's width, in pixels.
    #[inline]
    pub fn width(&self) -> usize {
        self.width
    }

    /// The view's height, in pixels.
    #[inline]
    pub fn height(&self) -> usize {
        self.height
    }

    /// The luma sample at `(x, y)`.
    ///
    /// # Panics
    /// If `x >= width()` or `y >= height()` — every caller in this crate
    /// only ever indexes within the bounds [`Self::new`] validated.
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> u8 {
        self.data[y * self.row_stride + x * self.pixel_stride]
    }

    /// A tightly-packed row's bytes, when `pixel_stride == 1` — the fast
    /// path a decode engine can take instead of calling [`Self::get`] pixel
    /// by pixel. Returns `None` when `pixel_stride != 1` (the row isn't
    /// tightly packed) or `y` is out of bounds.
    #[inline]
    pub fn row(&self, y: usize) -> Option<&'a [u8]> {
        if self.pixel_stride != 1 || y >= self.height {
            return None;
        }
        let start = y * self.row_stride;
        Some(&self.data[start..start + self.width])
    }

    /// Build a view over `frame`'s Y-plane, zero-copy —
    /// [`crate::ImageFormat::Yuv420`] only. [`crate::ImageFormat::Bgra`] has
    /// no borrowed luma plane to view (see [`super::decode_frame`]'s doc for
    /// that path instead), so this always returns `None` for it. Also
    /// `None` when the frame reports no planes at all, or the plane's
    /// declared strides don't fit its own data ([`LumaViewError`] from
    /// [`Self::new`], discarded here since this is a best-effort
    /// constructor).
    pub fn from_frame(frame: &ImageFrame<'a>) -> Option<LumaView<'a>> {
        match frame.format {
            ImageFormat::Yuv420 => {
                let plane = frame.planes.first()?;
                LumaView::new(
                    plane.data,
                    frame.width as usize,
                    frame.height as usize,
                    plane.row_stride,
                    plane.pixel_stride,
                )
                .ok()
            }
            ImageFormat::Bgra => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_accepts_a_tightly_packed_buffer() {
        let data = vec![0u8; 4 * 3];
        let view = LumaView::new(&data, 4, 3, 4, 1).unwrap();
        assert_eq!(view.width(), 4);
        assert_eq!(view.height(), 3);
    }

    #[test]
    // The `* 1` below spells out the constructor's own `(height-1)*row_stride
    // + (width-1)*pixel_stride` formula verbatim (pixel_stride is 1 in this
    // fixture) rather than pre-folding it — clippy flags the redundant `* 1`.
    #[allow(clippy::identity_op)]
    fn constructor_rejects_a_buffer_one_byte_too_small() {
        // Exactly the last valid pixel's byte, minus one.
        let needed = (3 - 1) * 4 + (4 - 1) * 1 + 1; // == 15
        let data = vec![0u8; needed - 1];
        let err = LumaView::new(&data, 4, 3, 4, 1).unwrap_err();
        match err {
            LumaViewError::BufferTooSmall { needed: n, got, .. } => {
                assert_eq!(n, needed);
                assert_eq!(got, needed - 1);
            }
            other => panic!("expected BufferTooSmall, got {other:?}"),
        }
    }

    #[test]
    #[allow(clippy::identity_op)]
    fn constructor_accepts_the_exact_minimum_buffer() {
        let needed = (3 - 1) * 4 + (4 - 1) * 1 + 1; // == 15
        let data = vec![0u8; needed];
        assert!(LumaView::new(&data, 4, 3, 4, 1).is_ok());
    }

    #[test]
    fn constructor_rejects_zero_dimensions() {
        let data = vec![0u8; 4];
        assert_eq!(
            LumaView::new(&data, 0, 3, 4, 1).unwrap_err(),
            LumaViewError::EmptyDimensions {
                width: 0,
                height: 3
            }
        );
        assert_eq!(
            LumaView::new(&data, 4, 0, 4, 1).unwrap_err(),
            LumaViewError::EmptyDimensions {
                width: 4,
                height: 0
            }
        );
    }

    #[test]
    fn get_indexes_by_asymmetric_strides() {
        // A 2x2 view embedded in a wider, pixel_stride=2 buffer:
        // row 0: [A, _, B, _, pad, pad]
        // row 1: [C, _, D, _, pad, pad]
        // row_stride = 6, pixel_stride = 2.
        #[rustfmt::skip]
        let data: [u8; 12] = [
            b'A', 0, b'B', 0, 0, 0,
            b'C', 0, b'D', 0, 0, 0,
        ];
        let view = LumaView::new(&data, 2, 2, 6, 2).unwrap();
        assert_eq!(view.get(0, 0), b'A');
        assert_eq!(view.get(1, 0), b'B');
        assert_eq!(view.get(0, 1), b'C');
        assert_eq!(view.get(1, 1), b'D');
        // pixel_stride != 1: no tightly-packed row fast path.
        assert_eq!(view.row(0), None);
    }

    #[test]
    fn row_is_available_only_for_pixel_stride_one() {
        let data: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
        let view = LumaView::new(&data, 4, 2, 4, 1).unwrap();
        assert_eq!(view.row(0), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(view.row(1), Some(&[5u8, 6, 7, 8][..]));
        assert_eq!(view.row(2), None, "out of bounds");
    }
}
