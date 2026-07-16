//! The [`Scene`] display list and the [`Command`]s it holds.

use kurbo::{Affine, Point, Rect};
use peniko::{Brush, Color, ImageData};

use crate::glyph::GlyphRun;

/// A single paint operation recorded into a [`Scene`].
///
/// This is the renderer-agnostic vocabulary the render crate (layer 4)
/// translates into backend draw calls. No `vello`/`wgpu` types appear here.
#[derive(Clone, Debug)]
pub enum Command {
    /// Fill an axis-aligned rectangle with a brush, under a transform.
    FillRect {
        rect: Rect,
        brush: Brush,
        transform: Affine,
    },
    /// Fill an axis-aligned rectangle with uniformly rounded corners.
    RoundedRect {
        rect: Rect,
        /// Corner radius, in the pre-transform coordinate space.
        radius: f64,
        brush: Brush,
        transform: Affine,
    },
    /// Stroke a straight line segment from `p0` to `p1`.
    Line {
        p0: Point,
        p1: Point,
        /// Stroke width, in the pre-transform coordinate space.
        width: f64,
        brush: Brush,
        transform: Affine,
    },
    /// Draw a positioned run of glyphs.
    GlyphRun(GlyphRun),
    /// Push a rectangular clip onto the render backend's clip stack, under a
    /// transform. Subsequent draws are clipped to it until the matching
    /// [`Command::PopClip`].
    PushClip { rect: Rect, transform: Affine },
    /// Pop the most recently pushed clip.
    PopClip,
    /// Draw a decoded image (natural pixel size `data.width`x`data.height`),
    /// scaled to fill `dest`, under a transform.
    ///
    /// `data` is cloned from the widget's cached `ImageSource` each frame;
    /// `peniko::ImageData`'s `Blob<u8>` is reference-counted internally, so
    /// this is a cheap handle clone, never a pixel copy or re-decode.
    Image {
        data: ImageData,
        dest: Rect,
        transform: Affine,
    },
    /// Draw a rounded rectangle with a gaussian-blurred elevation shadow (an
    /// approximation of a CSS `box-shadow`), under a transform.
    ///
    /// `radius` is the rectangle's corner radius, `std_dev` the blur's
    /// standard deviation, both in the pre-transform coordinate space. Maps
    /// directly onto vello 0.9's `Scene::draw_blurred_rounded_rect`, whose
    /// `brush` parameter is a concrete `peniko::Color` (not a `Brush`) — a
    /// blurred shadow has no gradient support in this vello version.
    BlurredRoundedRect {
        rect: Rect,
        radius: f64,
        std_dev: f64,
        color: Color,
        transform: Affine,
    },
    /// Push a translucent layer onto the render backend's layer stack, under
    /// a transform. Subsequent draws are composited at `alpha` until the
    /// matching [`Command::PopLayer`].
    ///
    /// Semantically a generalization of [`Command::PushClip`] (which is
    /// `PushLayer` with `alpha: 1.0`) — kept as a distinct variant rather than
    /// folded into it so existing `PushClip`/`PopClip` consumers are
    /// unaffected (see `forgekit-render::convert`).
    PushLayer {
        rect: Rect,
        alpha: f32,
        transform: Affine,
    },
    /// Pop the most recently pushed layer.
    PopLayer,
}

/// Renderer-agnostic, immediate-mode display list.
///
/// Widgets paint into a `Scene` (via [`crate::SceneBuilder`]) each frame; the
/// render crate consumes [`Scene::commands`] to draw. Rebuilt per frame — call
/// [`Scene::reset`] before re-recording rather than allocating a new `Scene`.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    commands: Vec<Command>,
}

impl Scene {
    /// Creates an empty scene.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears all recorded commands so the scene can be reused for the next frame.
    pub fn reset(&mut self) {
        self.commands.clear();
    }

    /// The recorded commands for this frame, in paint order.
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// Records a command. Used by [`crate::SceneBuilder`]; not part of the
    /// stable widget-facing API.
    pub(crate) fn push(&mut self, command: Command) {
        self.commands.push(command);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_scene_has_no_commands() {
        let scene = Scene::new();
        assert!(scene.commands().is_empty());
    }

    #[test]
    fn reset_clears_commands() {
        let mut scene = Scene::new();
        scene.push(Command::PushClip {
            rect: Rect::new(0.0, 0.0, 1.0, 1.0),
            transform: Affine::IDENTITY,
        });
        assert_eq!(scene.commands().len(), 1);

        scene.reset();
        assert!(scene.commands().is_empty());
    }
}
