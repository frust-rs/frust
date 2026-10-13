//! The QR matrix: bytes in, a square grid of dark/light modules out.

use qrcode::{Color, EcLevel, QrCode, types::QrError as EncodeError};

use crate::EccLevel;

/// Why a payload could not be encoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QrError {
    /// The payload was empty; there is nothing to scan.
    #[error("QR payload is empty")]
    Empty,
    /// The payload exceeds the capacity of the largest QR version at this ECC level.
    #[error("QR payload is too long for the chosen error-correction level")]
    TooLong,
    /// Any other encoder failure.
    #[error("QR encoding failed: {0}")]
    Encode(String),
}

/// A square QR symbol without its quiet zone; `true` is a dark module.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QrMatrix {
    modules: Vec<bool>,
    width: usize,
}

impl QrMatrix {
    /// Modules per side.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Whether the module at column `x`, row `y` is dark; out of range is light.
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        x < self.width && y < self.width && self.modules[y * self.width + x]
    }

    /// Row `y` as a slice of modules (`true` = dark).
    ///
    /// # Panics
    /// If `y >= width()`.
    pub fn row(&self, y: usize) -> &[bool] {
        &self.modules[y * self.width..(y + 1) * self.width]
    }
}

fn level(ecc: EccLevel) -> EcLevel {
    match ecc {
        EccLevel::L => EcLevel::L,
        EccLevel::M => EcLevel::M,
        EccLevel::Q => EcLevel::Q,
        EccLevel::H => EcLevel::H,
    }
}

/// Encode `payload` at the given error-correction level (smallest fitting version).
pub fn encode(payload: &[u8], ecc: EccLevel) -> Result<QrMatrix, QrError> {
    if payload.is_empty() {
        return Err(QrError::Empty);
    }
    let code = QrCode::with_error_correction_level(payload, level(ecc)).map_err(|e| match e {
        EncodeError::DataTooLong => QrError::TooLong,
        other => QrError::Encode(other.to_string()),
    })?;
    let width = code.width();
    let modules = code
        .to_colors()
        .into_iter()
        .map(|c| c == Color::Dark)
        .collect();
    Ok(QrMatrix { modules, width })
}

/// Coalesce consecutive dark modules of one row into `(start, len)` runs.
pub fn dark_runs(row: &[bool]) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut x = 0;
    std::iter::from_fn(move || {
        while x < row.len() && !row[x] {
            x += 1;
        }
        if x >= row.len() {
            return None;
        }
        let start = x;
        while x < row.len() && row[x] {
            x += 1;
        }
        Some((start, x - start))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_coalesce() {
        let row = [true, true, false, true, false, false, true, true, true];
        let runs: Vec<_> = dark_runs(&row).collect();
        assert_eq!(runs, vec![(0, 2), (3, 1), (6, 3)]);
        assert_eq!(dark_runs(&[false, false]).count(), 0);
    }

    #[test]
    fn empty_and_oversized_payloads_error() {
        assert_eq!(encode(b"", EccLevel::M), Err(QrError::Empty));
        assert_eq!(
            encode(&vec![b'a'; 4000], EccLevel::H),
            Err(QrError::TooLong)
        );
    }

    #[test]
    fn matrix_is_square_with_finder_corner() {
        let m = encode(b"frust", EccLevel::M).unwrap();
        assert_eq!(m.width(), 21);
        assert!(m.is_dark(0, 0));
        assert!(!m.is_dark(99, 0));
    }
}
