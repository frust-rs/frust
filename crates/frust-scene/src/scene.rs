//! The [`Scene`] display list and the [`Command`]s it holds.

use kurbo::{Affine, BezPath, Point, Rect};
use peniko::{Brush, Color, ImageData};

use crate::glyph::GlyphRun;
use crate::shader::ShaderProgram;

/// How a [`Command::Path`] is rendered: filled or stroked.
///
/// Kept intentionally minimal (task 05, PLAN.md D2b): the fill/stroke shape
/// widgets need for arcs (circular progress, activity indicators) — a
/// nonzero-fill, or a stroke with a fixed width and round caps/joins. No
/// dash pattern, miter limit, or even-odd fill rule yet; extend here (and in
/// `frust-render::convert`) if a later widget needs one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathStyle {
    /// Fill using the nonzero winding rule.
    Fill,
    /// Stroke with the given width (pre-transform coordinate space) and
    /// round caps/joins.
    Stroke {
        /// Stroke width, in the pre-transform coordinate space.
        width: f64,
    },
}

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
    /// unaffected (see `frust-render::convert`).
    PushLayer {
        rect: Rect,
        alpha: f32,
        transform: Affine,
    },
    /// Pop the most recently pushed layer.
    PopLayer,
    /// Clear an axis-aligned rectangle to full transparency (alpha 0) under a
    /// transform, erasing everything already drawn beneath it in this scene —
    /// a real destination-clearing composite, not a skipped paint.
    ///
    /// The platform-view hole-punch is the sole v1 producer (see
    /// `frust-core`'s `PaintScene::clear_rect`): a translucent-surface (Mode B)
    /// slot punches its rect so an opaque app backdrop painted below it (the
    /// catalog's `AppBackground`) doesn't seal the hole the hosted native view
    /// shows through. `frust-render::convert` lowers this to a destination-out
    /// composite (an opaque fill erasing color and alpha wherever it covers),
    /// hoisted to the scene root past any enclosing clip/opacity group so a
    /// nested slot's punch isn't confined to its own group's content — the
    /// exact composite mode and hoist mechanics are `frust-render`'s to name
    /// (scene-layer purity: no vello types here). The clear only becomes
    /// visible on a surface that actually carries an alpha channel; on an
    /// opaque surface the transparency is disregarded (vello's surface
    /// contract), which is why the producer gates it on the translucent flag
    /// rather than punching always.
    ClearRect { rect: Rect, transform: Affine },
    /// Fill or stroke an arbitrary vector path (e.g. an arc), under a
    /// transform.
    ///
    /// `path` is a `kurbo::BezPath` already positioned in the same
    /// coordinate space as every other command (the caller has translated it
    /// to the widget's origin before recording); `style` selects fill vs.
    /// stroke (see [`PathStyle`]).
    Path {
        path: BezPath,
        style: PathStyle,
        brush: Brush,
        transform: Affine,
    },
    /// Draw a fragment-shader-filled rectangle, scaled to fill `dest`, under
    /// a transform.
    ///
    /// `program` is compiled (and cache-keyed on [`ShaderProgram::id`]) by
    /// `frust-render` — see [`ShaderProgram`]'s v1 opaque-output contract.
    /// `time` is seconds, app-supplied (from `PaintCtx::frame_time` at the
    /// widget layer), threaded into the shader's uniform buffer.
    ShaderQuad {
        program: ShaderProgram,
        dest: Rect,
        transform: Affine,
        time: f32,
    },
}

/// Renderer-agnostic, immediate-mode display list.
///
/// Widgets paint into a `Scene` (via [`crate::SceneBuilder`]) each frame; the
/// render crate consumes [`Scene::commands`] to draw. Rebuilt per frame — call
/// [`Scene::reset`] before re-recording rather than allocating a new `Scene`.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    commands: Vec<Command>,
    /// [`crate::SceneBuilder`]'s transform stack, parked here between frames
    /// (task 10.E, PLAN.md Phase 10.E) so its backing `Vec` allocation is
    /// reused across every `SceneBuilder::new` call instead of reallocating
    /// per frame — a `SceneBuilder` borrows it via `&mut Scene` and resets it
    /// to `[Affine::IDENTITY]` on construction, so behavior is unchanged.
    /// Not part of the stable widget-facing API.
    pub(crate) transform_stack: Vec<Affine>,
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
