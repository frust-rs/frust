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
///
/// The text tier also says which face this is: [`FontHandle::with_identity`]
/// attaches the PostScript name and hidden flag its font oracle resolved the
/// face to. A renderer that cannot read a face's outlines itself (CoreText for
/// an `hvgl`-only face) opens it by that name; it does not re-derive the name
/// from the blob, and draws only when the system honours exactly that name: a
/// substituted face or a handle with no name is a counted refusal, so a hidden
/// face draws only if the system resolves its hidden name.
#[derive(Clone, Debug, PartialEq)]
pub struct FontHandle {
    font: FontData,
    /// Normalized (F2Dot14) variation coordinates, `fvar` axis order; `None`
    /// for the default instance.
    coords: Option<Arc<[i16]>>,
    /// The face's PostScript name as the text tier's oracle resolved it.
    postscript_name: Option<Arc<str>>,
    /// The oracle flagged the face system-private (a dot-prefixed name).
    hidden: bool,
    /// Faux-bold stroke as a fraction of the em; `0.0` for none.
    embolden: f32,
    /// Faux-oblique slant in degrees; `0.0` for none.
    skew_degrees: f32,
}

impl FontHandle {
    /// Wraps a `peniko::FontData` for use in a [`GlyphRun`], at the face's
    /// default variation instance.
    pub fn new(font: FontData) -> Self {
        Self {
            font,
            coords: None,
            postscript_name: None,
            hidden: false,
            embolden: 0.0,
            skew_degrees: 0.0,
        }
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

    /// This handle with the face identity the text tier resolved: the face's
    /// PostScript name (`None` when the oracle reported none) and whether the
    /// oracle flagged it system-private.
    #[must_use]
    pub fn with_identity(mut self, postscript_name: Option<Arc<str>>, hidden: bool) -> Self {
        self.postscript_name = postscript_name;
        self.hidden = hidden;
        self
    }

    /// This handle with the synthesis the text tier decided for a face that
    /// lacks the requested weight or slant: `embolden` is the faux-bold stroke
    /// as a fraction of the em, `skew_degrees` the faux-oblique angle. Zero
    /// (or a non-finite or negative `embolden`) means none. Both take part in
    /// equality, so differently synthesized handles are different faces.
    #[must_use]
    pub fn with_synthesis(mut self, embolden: f32, skew_degrees: f32) -> Self {
        self.embolden = if embolden.is_finite() && embolden > 0.0 {
            embolden
        } else {
            0.0
        };
        self.skew_degrees = if skew_degrees.is_finite() {
            skew_degrees
        } else {
            0.0
        };
        self
    }

    /// Faux-bold stroke as a fraction of the em; `0.0` for none.
    pub fn embolden(&self) -> f32 {
        self.embolden
    }

    /// Faux-oblique slant in degrees; `0.0` for none.
    pub fn skew_degrees(&self) -> f32 {
        self.skew_degrees
    }

    /// The face's PostScript name as the text tier resolved it; `None` for a
    /// handle built without an identity.
    pub fn postscript_name(&self) -> Option<&str> {
        self.postscript_name.as_deref()
    }

    /// Whether the text tier flagged the face system-private (a dot-prefixed
    /// name that name-based lookup cannot be trusted to resolve).
    pub fn is_hidden(&self) -> bool {
        self.hidden
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
    fn synthesis_defaults_to_none_and_takes_part_in_equality() {
        let plain = FontHandle::new(font());
        assert_eq!((plain.embolden(), plain.skew_degrees()), (0.0, 0.0));
        let bold = plain.clone().with_synthesis(0.04, 0.0);
        let oblique = plain.clone().with_synthesis(0.0, 14.0);
        assert_eq!(bold.embolden(), 0.04);
        assert_eq!(oblique.skew_degrees(), 14.0);
        assert_eq!(plain, plain.clone());
        assert_ne!(plain, bold);
        assert_ne!(plain, oblique);
        assert_ne!(bold, oblique);
        assert_eq!(bold, bold.clone());
        let bad = plain.clone().with_synthesis(f32::NAN, f32::INFINITY);
        assert_eq!(bad, plain);
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

    #[test]
    fn a_new_handle_has_no_identity() {
        let handle = FontHandle::new(font());
        assert_eq!(handle.postscript_name(), None);
        assert!(!handle.is_hidden());
    }

    #[test]
    fn identity_is_kept_alongside_the_coordinates_and_survives_clones() {
        let name: Arc<str> = Arc::from(".PingFangUITextSC-Default");
        let handle = FontHandle::new(font())
            .with_normalized_coords(&[0, 8192])
            .with_identity(Some(name), true);
        let cloned = handle.clone();
        assert_eq!(cloned.postscript_name(), Some(".PingFangUITextSC-Default"));
        assert!(cloned.is_hidden());
        assert_eq!(cloned.normalized_coords(), &[0, 8192]);
        let renamed = handle.with_identity(None, false);
        assert_eq!(renamed.postscript_name(), None);
        assert!(!renamed.is_hidden());
        assert_eq!(renamed.normalized_coords(), &[0, 8192]);
    }
}
