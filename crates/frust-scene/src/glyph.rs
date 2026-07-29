//! Glyph and font primitives used by [`crate::Command::GlyphRun`].

use kurbo::Affine;
use peniko::{Brush, FontData};

/// Opaque wrapper around a loaded font resource (blob + collection index).
///
/// `frust-text` constructs `FontHandle`s from `peniko::FontData`;
/// `frust-render` (layer 4) unwraps them when building glyph draw calls.
/// `peniko::FontData` is the shared Linebender vocabulary re-exported by vello — it
/// does not count as a vello type for the purposes of this crate's "no vello in
/// the public API" rule. Note: peniko 0.6.1 names
/// this type `FontData`, not `Font`; the shape
/// (blob + collection index) is unchanged.
#[derive(Clone, Debug)]
pub struct FontHandle(FontData);

impl FontHandle {
    /// Wraps a `peniko::FontData` for use in a [`GlyphRun`].
    pub fn new(font: FontData) -> Self {
        Self(font)
    }

    /// Borrows the underlying `peniko::FontData`.
    pub fn font(&self) -> &FontData {
        &self.0
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
