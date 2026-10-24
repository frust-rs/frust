//! Glyph and font primitives used by [`crate::Command::GlyphRun`].

use std::sync::Arc;

use kurbo::Affine;
use peniko::{Brush, FontData};

/// Opaque wrapper around a loaded font resource (blob + collection index),
/// plus the variation instance a run was shaped at.
///
/// `frust-text` constructs `FontHandle`s from `peniko::FontData`;
/// `frust-render` (layer 4) unwraps them when building glyph draw calls.
/// `peniko::FontData` is the shared Linebender vocabulary `vello_common` re-exports — it
/// does not count as a vello type for the purposes of this crate's "no vello in
/// the public API" rule. Note: peniko 0.6.1 names
/// this type `FontData`, not `Font`; the shape
/// (blob + collection index) is unchanged.
///
/// A handle built with [`FontHandle::new`] names the face's default instance.
/// [`FontHandle::with_normalized_coords`] attaches the normalized variation
/// coordinates the shaper chose (a requested weight on a variable face), in
/// the face's `fvar` axis order; a renderer that honours them reads
/// [`FontHandle::normalized_coords`].
#[derive(Clone, Debug)]
pub struct FontHandle {
    font: FontData,
    /// Normalized (F2Dot14) variation coordinates, `fvar` axis order; `None`
    /// for the default instance.
    coords: Option<Arc<[i16]>>,
}

impl FontHandle {
    /// Wraps a `peniko::FontData` for use in a [`GlyphRun`], at the face's
    /// default variation instance.
    pub fn new(font: FontData) -> Self {
        Self { font, coords: None }
    }

    /// This handle at the variation instance `coords` names: normalized
    /// F2Dot14 coordinates, one per `fvar` axis in axis order, as a shaper
    /// such as parley reports them. Coordinates that are all zero (or empty)
    /// name the default instance and are not kept.
    #[must_use]
    pub fn with_normalized_coords(mut self, coords: &[i16]) -> Self {
        self.coords = coords
            .iter()
            .any(|coord| *coord != 0)
            .then(|| Arc::from(coords));
        self
    }

    /// Borrows the underlying `peniko::FontData`.
    pub fn font(&self) -> &FontData {
        &self.font
    }

    /// The normalized variation coordinates this run was shaped at, in `fvar`
    /// axis order; empty for the face's default instance.
    pub fn normalized_coords(&self) -> &[i16] {
        self.coords.as_deref().unwrap_or(&[])
    }
}

impl From<FontData> for FontHandle {
    fn from(font: FontData) -> Self {
        Self::new(font)
    }
}

/// A single positioned glyph within a [`GlyphRun`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    /// Glyph ID within the run's font.
    pub id: u32,
    /// X position, in the run's local coordinate space (pre-transform).
    pub x: f32,
    /// Y position, in the run's local coordinate space (pre-transform).
    pub y: f32,
}

/// A run of glyphs sharing a font, size, brush, and transform.
#[derive(Clone, Debug)]
pub struct GlyphRun {
    /// Font this run's glyph IDs are resolved against.
    pub font: FontHandle,
    /// Font size, in pixels.
    pub font_size: f32,
    /// Paint used to fill the glyphs.
    pub brush: Brush,
    /// Transform applied to the whole run.
    pub transform: Affine,
    /// The positioned glyphs.
    pub glyphs: Vec<Glyph>,
}

#[cfg(test)]
mod tests {
    use peniko::Blob;

    use super::*;

    fn font() -> FontData {
        FontData::new(Blob::new(Arc::new(vec![0_u8; 4])), 0)
    }

    #[test]
    fn a_new_handle_names_the_default_instance() {
        assert!(FontHandle::new(font()).normalized_coords().is_empty());
    }

    #[test]
    fn variation_coords_are_kept_unless_they_name_the_default_instance() {
        let varied = FontHandle::new(font()).with_normalized_coords(&[-4096, 0, 8192]);
        assert_eq!(varied.normalized_coords(), &[-4096, 0, 8192]);
        assert_eq!(varied.clone().normalized_coords(), &[-4096, 0, 8192]);
        for default in [&[][..], &[0, 0, 0][..]] {
            let handle = varied.clone().with_normalized_coords(default);
            assert!(handle.normalized_coords().is_empty(), "{default:?}");
        }
    }
}
