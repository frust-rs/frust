//! The [`Scene`] display list and the [`Command`]s it holds.

use kurbo::{Affine, BezPath, Point, Rect};
use peniko::{Brush, Color, ImageData};

use crate::glyph::GlyphRun;
use crate::shader::ShaderProgram;

/// Per-corner radii for a rounded rectangle, in the pre-transform coordinate
/// space.
///
/// Corners are named clockwise from the top-left, matching
/// `kurbo::RoundedRectRadii` — the concrete shape `frust-render` builds at
/// encode time (scene-layer purity keeps the shape type itself out of this
/// crate's commands).
///
/// `From<f64>` covers the uniform case, so the one-radius callers that predate
/// this type keep passing a bare `f64` and the builder converts:
/// `CornerRadii::from(8.0)` == `CornerRadii::new(8.0, 8.0, 8.0, 8.0)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CornerRadii {
    /// Top-left corner radius.
    pub top_left: f64,
    /// Top-right corner radius.
    pub top_right: f64,
    /// Bottom-right corner radius.
    pub bottom_right: f64,
    /// Bottom-left corner radius.
    pub bottom_left: f64,
}

impl CornerRadii {
    /// Radii for each corner, clockwise from the top-left (the
    /// `kurbo::RoundedRectRadii::new` argument order).
    pub const fn new(top_left: f64, top_right: f64, bottom_right: f64, bottom_left: f64) -> Self {
        Self {
            top_left,
            top_right,
            bottom_right,
            bottom_left,
        }
    }

    /// The same `radius` on all four corners — what [`From<f64>`] builds.
    pub const fn uniform(radius: f64) -> Self {
        Self::new(radius, radius, radius, radius)
    }

    /// The largest of the four corner radii.
    ///
    /// A backend that can only express a single radius (both blurred-shadow
    /// primitives take one) lowers through this rather than through the
    /// smallest: a shadow rounded *more* than its caster only lightens a square
    /// corner, while one rounded less pushes a hard shadow wedge out through a
    /// rounded corner's notch.
    pub fn largest(&self) -> f64 {
        self.top_left
            .max(self.top_right)
            .max(self.bottom_right)
            .max(self.bottom_left)
    }
}

impl From<f64> for CornerRadii {
    fn from(radius: f64) -> Self {
        Self::uniform(radius)
    }
}

/// A stroke's dash pattern: an `on` run, an `off` gap, and a `phase` offset
/// into that repeating cycle — all lengths in the pre-transform coordinate
/// space.
///
/// One on/off pair rather than an arbitrary-length array: it covers the dashed
/// dividers/outlines/focus rings widgets ask for, stays `Copy` (so
/// [`PathStyle`] and every command holding one stay `Copy`/allocation-free),
/// and the render crate expands it into a `[on, off]` slice at encode time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DashPattern {
    /// Length of each painted dash.
    pub on: f64,
    /// Length of each gap between dashes.
    pub off: f64,
    /// Distance into the on/off cycle the pattern starts at — animate this to
    /// march the dashes along the path.
    pub phase: f64,
}

/// Minimum total dash period (on + off) in logical pixels for a pattern to be
/// rendered as dashed. Below this, the dash segments would be too small to see
/// and the period becomes too dense to efficiently expand at encode time, so
/// the pattern falls back to a solid stroke. This is the threshold below which
/// a dash pattern is visually indistinguishable from a solid stroke anyway.
const DASH_PERIOD_EPSILON: f64 = 0.1;

impl DashPattern {
    /// An `on`/`off` cycle starting at phase 0.
    pub const fn new(on: f64, off: f64) -> Self {
        Self {
            on,
            off,
            phase: 0.0,
        }
    }

    /// The same pattern offset by `phase` into its cycle.
    pub const fn with_phase(mut self, phase: f64) -> Self {
        self.phase = phase;
        self
    }

    /// Whether this pattern actually breaks a stroke into dashes.
    ///
    /// A non-positive or non-finite length has no dashed interpretation (a zero
    /// `off` is a solid line; a zero `on` paints nothing, which a caller never
    /// means by "dashed"), so `frust-render` strokes such a path solid rather
    /// than feeding a degenerate cycle to the dash iterator. Additionally, a
    /// period (on + off) smaller than [`DASH_PERIOD_EPSILON`] is too dense to
    /// efficiently expand at encode time and is visually indistinguishable from
    /// a solid stroke anyway, so the pattern falls back to solid.
    pub fn is_effective(&self) -> bool {
        self.on.is_finite()
            && self.off.is_finite()
            && self.phase.is_finite()
            && self.on > 0.0
            && self.off > 0.0
            && (self.on + self.off) >= DASH_PERIOD_EPSILON
    }
}

/// How a [`Command::Path`] is rendered: filled or stroked.
///
/// Kept intentionally minimal: the fill/stroke shape
/// widgets need for arcs (circular progress, activity indicators) — a
/// nonzero-fill, or a stroke with a fixed width, round caps/joins, and an
/// optional [`DashPattern`]. No miter limit or even-odd fill rule yet; extend
/// here (and in `frust-render::convert`) if a later widget needs one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathStyle {
    /// Fill using the nonzero winding rule.
    Fill,
    /// Stroke with the given width (pre-transform coordinate space) and
    /// round caps/joins.
    Stroke {
        /// Stroke width, in the pre-transform coordinate space.
        width: f64,
        /// Dash pattern, or `None` for a solid stroke — what every stroke that
        /// predates dashing records, and what the render crate falls back to
        /// for a degenerate pattern (see [`DashPattern::is_effective`]).
        dash: Option<DashPattern>,
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
    /// Fill an axis-aligned rectangle with rounded corners.
    RoundedRect {
        rect: Rect,
        /// Per-corner radii, in the pre-transform coordinate space; a uniform
        /// radius arrives here as [`CornerRadii::uniform`].
        radii: CornerRadii,
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
    /// Push a clip with rounded corners onto the render backend's clip stack,
    /// under a transform. Subsequent draws are clipped to the rounded shape
    /// until the matching [`Command::PopClip`].
    ///
    /// Popped by the *same* [`Command::PopClip`] a [`Command::PushClip`] uses —
    /// there is one clip stack, not a separate rounded one. The motivating
    /// consumer is a radiused mask over a bitmap (an avatar/thumbnail), which a
    /// rectangular clip cannot express.
    ///
    /// Carried as a `Rect` + [`CornerRadii`], mirroring
    /// [`Command::RoundedRect`] rather than naming a `kurbo::RoundedRect`; the
    /// render crate reconstitutes the concrete shape at encode time. An
    /// arbitrary-path clip is deliberately not modelled yet — extend here (and
    /// in `frust-render::convert`) if one is ever needed.
    PushClipRounded {
        rect: Rect,
        /// Per-corner radii, in the pre-transform coordinate space.
        radii: CornerRadii,
        transform: Affine,
    },
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
    /// `radii` are the rectangle's corner radii, `std_dev` the blur's standard
    /// deviation, both in the pre-transform coordinate space. Maps onto vello
    /// 0.9's `Scene::draw_blurred_rounded_rect`, whose `brush` parameter is a
    /// concrete `peniko::Color` (not a `Brush`) — a blurred shadow has no
    /// gradient support in this vello version — and which takes a *single*
    /// radius, as does the CPU tier's `fill_blurred_rounded_rect`. A per-corner
    /// shadow therefore lowers through [`CornerRadii::largest`] at encode time
    /// (see `frust-render::convert`); the field carries all four corners so
    /// this command shares one radii vocabulary with its rounded siblings and
    /// gains per-corner blur for free if a backend ever grows it.
    BlurredRoundedRect {
        rect: Rect,
        radii: CornerRadii,
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
    /// Marks the start of a cacheable "snapshot" bracket, under a transform.
    ///
    /// The commands between this and the matching [`Command::PopSnapshot`] are
    /// the BODY, recorded in the ordinary composed transform space (i.e. not
    /// pre-multiplied by `scale`). `rect` is the body's bounds in the local
    /// space of `transform` — the same convention [`Command::PushLayer`]'s
    /// `rect`/`alpha`/`transform` use.
    ///
    /// `alpha` (`0.0..=1.0`) and `scale` (uniform, about `rect`'s center) are
    /// PRESENTATION parameters applied to the body as a whole — deliberately
    /// NOT baked into the body's own commands, so a renderer can reuse a
    /// rasterized body while they animate.
    ///
    /// A renderer that implements snapshots MAY rasterize the body once and
    /// draw it as an image with `transform * scale_about(rect.center(),
    /// scale)` inside an alpha layer. A renderer that does not MUST paint the
    /// body inline wrapped exactly as if the recorder had emitted
    /// `push_transform(scale_about(rect.center(), scale))` (when `scale !=
    /// 1.0`) then `push_layer(rect, alpha)` (when `alpha < 1.0`) — pops
    /// reversed (see `frust-render::convert`'s miss path).
    ///
    /// Syntactic nesting is allowed, but only the OUTERMOST bracket needs
    /// honouring — an inner bracket's own `alpha`/`scale` may be ignored by a
    /// non-implementing renderer. An unbalanced [`Command::PopSnapshot`] is
    /// ignored, the same policy [`Command::PopLayer`] follows.
    PushSnapshot {
        /// Cache key identifying this bracket's body across frames.
        key: u64,
        rect: Rect,
        alpha: f32,
        /// Uniform scale, applied about `rect`'s center.
        scale: f64,
        transform: Affine,
    },
    /// Pop the most recently pushed snapshot bracket (see
    /// [`Command::PushSnapshot`]).
    PopSnapshot,
    /// Draw an externally owned GPU texture scaled to fill `dest` under
    /// `transform`.
    ///
    /// `id` is opaque scene-layer data (precedent: [`ShaderProgram`]'s opaque
    /// id, `shader.rs`) — only the render backend resolves it against
    /// textures registered with the GPU context; an unregistered id draws
    /// nothing.
    SceneTexture {
        id: u64,
        dest: Rect,
        transform: Affine,
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
    /// so its backing `Vec` allocation is
    /// reused across every `SceneBuilder::new` call instead of reallocating
    /// per frame — a `SceneBuilder` borrows it via `&mut Scene` and resets it
    /// to `[Affine::IDENTITY]` on construction, so behavior is unchanged.
    /// Not part of the stable widget-facing API.
    pub(crate) transform_stack: Vec<Affine>,
    /// Depth counter for open [`Command::PushSnapshot`] brackets, incremented
    /// by [`crate::SceneBuilder::push_snapshot`] and decremented — only when
    /// greater than zero — by [`crate::SceneBuilder::pop_snapshot`], the same
    /// unbalanced-pop policy [`Command::PopClip`]/[`Command::PopLayer`]
    /// follow. Reset alongside `commands` in [`Scene::reset`]; NOT reset by
    /// [`crate::SceneBuilder::new`] (unlike `transform_stack`), since it
    /// tracks bracket balance across the whole scene, not a builder session.
    /// Not part of the stable widget-facing API.
    pub(crate) snapshot_depth: usize,
}

impl Scene {
    /// Creates an empty scene.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears all recorded commands so the scene can be reused for the next frame.
    pub fn reset(&mut self) {
        self.commands.clear();
        self.snapshot_depth = 0;
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

    /// How many [`Command::PushSnapshot`] brackets are still open.
    ///
    /// `#[doc(hidden)]`, and deliberately not part of the stable widget-facing
    /// API: a widget has no business reading the bracket depth mid-recording,
    /// and nothing in the framework branches on it. It exists so a property
    /// test outside this crate can assert the balance contract the field's own
    /// docs state — that a balanced sequence leaves the depth at zero and an
    /// unmatched [`crate::SceneBuilder::pop_snapshot`] never drives it below
    /// zero — which is otherwise unobservable from the command stream alone,
    /// since an ignored pop records nothing to observe.
    #[doc(hidden)]
    pub fn snapshot_depth(&self) -> usize {
        self.snapshot_depth
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

    #[test]
    fn reset_clears_snapshot_depth() {
        let mut scene = Scene::new();
        scene.snapshot_depth = 2;
        scene.reset();
        assert_eq!(scene.snapshot_depth, 0);
    }

    #[test]
    fn corner_radii_from_f64_is_uniform() {
        assert_eq!(CornerRadii::from(6.0), CornerRadii::new(6.0, 6.0, 6.0, 6.0));
        assert_eq!(CornerRadii::from(6.0), CornerRadii::uniform(6.0));
    }

    #[test]
    fn corner_radii_new_orders_corners_clockwise_from_top_left() {
        // The argument order is load-bearing: it must match
        // `kurbo::RoundedRectRadii::new`, which `frust-render` converts into.
        let radii = CornerRadii::new(1.0, 2.0, 3.0, 4.0);
        assert_eq!(radii.top_left, 1.0);
        assert_eq!(radii.top_right, 2.0);
        assert_eq!(radii.bottom_right, 3.0);
        assert_eq!(radii.bottom_left, 4.0);
    }

    #[test]
    fn corner_radii_largest_picks_the_biggest_corner() {
        assert_eq!(CornerRadii::new(1.0, 9.0, 3.0, 4.0).largest(), 9.0);
        assert_eq!(CornerRadii::uniform(2.0).largest(), 2.0);
        assert_eq!(CornerRadii::default().largest(), 0.0);
    }

    #[test]
    fn dash_pattern_defaults_to_zero_phase_and_carries_with_phase() {
        let dash = DashPattern::new(4.0, 2.0);
        assert_eq!(dash.phase, 0.0);
        assert_eq!(dash.with_phase(1.5).phase, 1.5);
        // `with_phase` leaves the cycle itself alone.
        assert_eq!(dash.with_phase(1.5).on, 4.0);
        assert_eq!(dash.with_phase(1.5).off, 2.0);
    }

    #[test]
    fn degenerate_dash_patterns_are_not_effective() {
        assert!(DashPattern::new(4.0, 2.0).is_effective());
        assert!(!DashPattern::new(0.0, 2.0).is_effective());
        assert!(!DashPattern::new(4.0, 0.0).is_effective());
        assert!(!DashPattern::new(-4.0, 2.0).is_effective());
        assert!(!DashPattern::new(f64::NAN, 2.0).is_effective());
        assert!(!DashPattern::new(f64::INFINITY, 2.0).is_effective());
        assert!(
            !DashPattern::new(4.0, 2.0)
                .with_phase(f64::NAN)
                .is_effective()
        );
    }

    #[test]
    fn tiny_period_dash_patterns_are_not_effective() {
        // A pattern with a vanishingly small period (on + off) below the
        // epsilon is not effective, falling back to solid stroke. This prevents
        // the dash iterator from expanding into astronomically many segments.
        assert!(!DashPattern::new(1e-9, 1e-9).is_effective());
        assert!(!DashPattern::new(0.01, 0.01).is_effective()); // period 0.02 < 0.1
        assert!(!DashPattern::new(0.04, 0.05).is_effective()); // period 0.09 < 0.1
    }

    #[test]
    fn dash_pattern_at_epsilon_boundary_is_effective() {
        // At the epsilon boundary, the pattern is exactly effective (not strict <).
        assert!(DashPattern::new(0.05, 0.05).is_effective()); // period exactly 0.1
        assert!(DashPattern::new(0.03, 0.07).is_effective()); // period exactly 0.1
    }

    #[test]
    fn normal_dash_patterns_remain_effective() {
        // Normal-sized patterns well above epsilon remain effective.
        assert!(DashPattern::new(1.0, 0.5).is_effective());
        assert!(DashPattern::new(4.0, 2.0).is_effective());
        assert!(DashPattern::new(10.0, 10.0).is_effective());
    }
}
