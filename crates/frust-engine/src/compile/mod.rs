//! Scene compilation: a `frust_scene::Scene` becomes sparse strips plus draws.
//!
//! [`SceneCompiler`] is a stateless walk over an already-recorded display list.
//! Unlike an immediate-mode scene recorder, there is no render state to save
//! and restore and no transform stack to unwind — `frust_scene::SceneBuilder`
//! has already composed every command's transform, so each command carries the
//! only transform it needs and the walk composes it with the frame's root once.
//!
//! The compiler owns the retained scratch a
//! [`StripGenerator`] needs (line buffer, tiles, flatten/stroke context) so a
//! steady-state frame reuses those allocations; it holds no per-frame state
//! between calls, and [`compile`](SceneCompiler::compile) returns everything a
//! frame produced in one [`CompiledFrame`].
//!
//! Only the geometry primitives are compiled here: axis-aligned and rounded
//! rectangles, lines, and arbitrary filled/stroked paths. Clips, layers,
//! glyphs, images, shader quads and snapshot brackets are recognised and
//! skipped — the engine grows them in later passes, and skipping is the
//! conservative behaviour (a frame draws less, never wrong).

pub mod paint;

pub mod draw;

pub use draw::{DepthCounter, EngineDraw};

use kurbo::{
    Affine, BezPath, Cap, Join, Line, PathEl, Rect, RoundedRect, RoundedRectRadii, Shape, Stroke,
};
use peniko::{Brush, Fill};

use frust_scene::{Command, CornerRadii, DashPattern, PathStyle, Scene};

use vello_common::encode::EncodedPaint;
use vello_common::fearless_simd::Level;
use vello_common::record::CommandRecorder;
use vello_common::strip_generator::{GenerationMode, StripGenerator, StripStorage};
use vello_common::tile::Tile;
use vello_common::util::is_axis_aligned;

use crate::compile::paint::{LutRequest, encode_brush};
use crate::error::EngineError;

/// Curve-flattening tolerance, in device pixels.
///
/// The value `vello_hybrid`'s own scene recorder flattens at; keeping it
/// identical is what lets the two rasterizers be compared strip-for-strip.
const FLATTEN_TOLERANCE: f64 = 0.1;

/// Everything one compiled frame produced.
///
/// The strips and their alpha coverage share one
/// [`StripStorage`]: `strips.strips` is the frame's whole strip buffer (each
/// [`EngineDraw::strip_range`] indexes into it) and `strips.alphas` the alpha
/// runs those strips reference. They are kept together because a strip's
/// packed alpha index is only meaningful against the alpha buffer generated
/// alongside it.
///
/// The draws themselves live in `recorder.draws` rather than in a field of
/// their own: [`CommandRecorder`] already owns that vector, and its node
/// ranges index into it, so a second parallel copy could only drift out of
/// agreement with the recording. Read them through [`CompiledFrame::draws`].
#[derive(Debug)]
pub struct CompiledFrame {
    /// The frame's strips and the alpha coverage they index.
    pub strips: StripStorage,
    /// The recorded render graph, owning the frame's draws.
    pub recorder: CommandRecorder<EngineDraw>,
    /// Paints too complex to inline into a draw, indexed by
    /// [`Paint::Indexed`](vello_common::paint::Paint::Indexed).
    pub encoded_paints: Vec<EncodedPaint>,
    /// The colour ramps `encoded_paints` needs made resident before the frame
    /// is drawn, one per gradient entry.
    ///
    /// Deliberately not serviced here: the compiler holds no gradient cache,
    /// so ramp residency is decided once per frame by the renderer rather than
    /// per draw by the walk (see [`paint`]).
    pub lut_requests: Vec<LutRequest>,
    /// How many of this frame's draws wrote their strip coverage directly as a
    /// rectangle, bypassing flattening and tiling (see [`fast_rect`]).
    ///
    /// Purely observational — nothing downstream branches on it. It exists so
    /// the fast path's admission rule is measurable from outside the compiler
    /// rather than inferred from a strip count that both paths can produce.
    pub fast_rect_draws: u32,
}

impl CompiledFrame {
    /// The frame's draws, in paint order (back-most first).
    pub fn draws(&self) -> &[EngineDraw] {
        &self.recorder.draws
    }

    /// The frame's whole strip buffer; a draw's `strip_range` indexes into it.
    pub fn strip_buf(&self) -> &[vello_common::strip::Strip] {
        &self.strips.strips
    }

    /// The alpha coverage the frame's strips reference.
    pub fn alphas(&self) -> &[u8] {
        &self.strips.alphas
    }
}

/// Compiles a `frust_scene::Scene` into strips and draws.
///
/// Create one per surface and reuse it across frames — the retained
/// [`StripGenerator`] is the point.
#[derive(Debug)]
pub struct SceneCompiler {
    generator: StripGenerator,
}

impl SceneCompiler {
    /// A compiler sized for a `width` x `height` viewport.
    ///
    /// The size is re-asserted on every [`compile`](Self::compile) call, so
    /// this is only the initial allocation hint; pass the surface's current
    /// size to avoid an immediate resize.
    pub fn new(width: u16, height: u16) -> Self {
        let level = Level::try_detect().unwrap_or(Level::baseline());
        Self {
            generator: StripGenerator::new(width, height, level),
        }
    }

    /// Compile `scene` for a `size` viewport, with `root` applied ahead of
    /// every command's own transform.
    ///
    /// # Errors
    ///
    /// [`EngineError::TargetTooLarge`] when `size` cannot be rounded up to
    /// whole tiles inside `u16`; [`EngineError::InvalidTransform`] when a
    /// composed transform is non-finite and so maps geometry to coordinates no
    /// `u16` pixel can hold; and [`EngineError::InvalidGeometry`] when a
    /// command the compiler lowers carries non-finite geometry of its own (see
    /// [`check_geometry`]). All three are refused before any strip is
    /// generated — the frame path returns errors and never panics.
    pub fn compile(
        &mut self,
        scene: &Scene,
        root: Affine,
        size: (u16, u16),
    ) -> Result<CompiledFrame, EngineError> {
        let (width, height) = size;
        check_tile_addressable(width, height)?;
        check_finite(root)?;

        // The whole scene is refused up front rather than mid-walk, so a
        // rejected frame never leaves half its draws recorded.
        for command in scene.commands() {
            if let Some(transform) = command_transform(command) {
                check_finite(root * transform)?;
            }
            check_geometry(command)?;
        }

        self.generator.reset(width, height);

        let mut frame = CompiledFrame {
            strips: StripStorage::new(GenerationMode::Append),
            recorder: CommandRecorder::new(width, height),
            encoded_paints: Vec::new(),
            lut_requests: Vec::new(),
            fast_rect_draws: 0,
        };
        let mut depth = DepthCounter::new();

        for command in scene.commands() {
            self.compile_command(command, root, &mut frame, &mut depth);
        }

        Ok(frame)
    }

    fn compile_command(
        &mut self,
        command: &Command,
        root: Affine,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
    ) {
        match command {
            Command::FillRect {
                rect,
                brush,
                transform,
            } => {
                let transform = root * *transform;

                if let Some(device_rect) = fast_rect(*rect, transform) {
                    let recorded =
                        self.record(frame, depth, brush, transform, |generator, storage| {
                            generator.generate_filled_rect_fast(&device_rect, storage, None);
                        });
                    if recorded {
                        frame.fast_rect_draws = frame.fast_rect_draws.saturating_add(1);
                    }
                } else {
                    self.record(frame, depth, brush, transform, |generator, storage| {
                        generator.generate_filled_path(
                            rect.path_elements(FLATTEN_TOLERANCE),
                            Fill::NonZero,
                            transform,
                            None,
                            storage,
                            None,
                        );
                    });
                }
            }
            Command::RoundedRect {
                rect,
                radii,
                brush,
                transform,
            } => {
                let transform = root * *transform;
                let shape = RoundedRect::from_rect(*rect, rounded_rect_radii(*radii));

                self.record(frame, depth, brush, transform, |generator, storage| {
                    generator.generate_filled_path(
                        shape.path_elements(FLATTEN_TOLERANCE),
                        Fill::NonZero,
                        transform,
                        None,
                        storage,
                        None,
                    );
                });
            }
            Command::Line {
                p0,
                p1,
                width,
                brush,
                transform,
            } => {
                let transform = root * *transform;
                let line = Line::new(*p0, *p1);
                let stroke = round_stroke(*width);

                self.record(frame, depth, brush, transform, |generator, storage| {
                    generator.generate_stroked_path(
                        line.path_elements(FLATTEN_TOLERANCE),
                        &stroke,
                        transform,
                        None,
                        storage,
                        None,
                    );
                });
            }
            Command::Path {
                path,
                style,
                brush,
                transform,
            } => {
                let transform = root * *transform;

                match style {
                    PathStyle::Fill => {
                        self.record(frame, depth, brush, transform, |generator, storage| {
                            generator.generate_filled_path(
                                path.iter(),
                                Fill::NonZero,
                                transform,
                                None,
                                storage,
                                None,
                            );
                        });
                    }
                    PathStyle::Stroke { width, dash } => {
                        let stroke = round_stroke(*width);
                        // A dash pattern is expanded into its own sub-paths
                        // before the stroker runs, the same lowering the
                        // display list's other consumers apply: the pattern
                        // never reaches a backend's own dash support, so every
                        // rasterizer sees the identical geometry.
                        let dashed = match dash {
                            Some(dash) if dash.is_effective() => Some(dash_path(path, *dash)),
                            _ => None,
                        };

                        match &dashed {
                            Some(dashed) => {
                                self.record(
                                    frame,
                                    depth,
                                    brush,
                                    transform,
                                    |generator, storage| {
                                        generator.generate_stroked_path(
                                            dashed.iter(),
                                            &stroke,
                                            transform,
                                            None,
                                            storage,
                                            None,
                                        );
                                    },
                                );
                            }
                            None => {
                                self.record(
                                    frame,
                                    depth,
                                    brush,
                                    transform,
                                    |generator, storage| {
                                        generator.generate_stroked_path(
                                            path.iter(),
                                            &stroke,
                                            transform,
                                            None,
                                            storage,
                                            None,
                                        );
                                    },
                                );
                            }
                        }
                    }
                }
            }
            // Recognised but not yet compiled. Listed one by one rather than
            // caught by a wildcard so a command added to the display list
            // fails to compile here instead of silently vanishing from every
            // frame.
            Command::GlyphRun(_)
            | Command::PushClip { .. }
            | Command::PushClipRounded { .. }
            | Command::PopClip
            | Command::Image { .. }
            | Command::BlurredRoundedRect { .. }
            | Command::PushLayer { .. }
            | Command::PopLayer
            | Command::ClearRect { .. }
            | Command::ShaderQuad { .. }
            | Command::PushSnapshot { .. }
            | Command::PopSnapshot => {}
        }
    }

    /// Run `generate`, then record whatever strips it appended as one draw
    /// painted with `brush` under `transform`.
    ///
    /// Returns whether a draw was recorded. A generator call that produced no
    /// strips (fully culled, degenerate, or empty geometry) records nothing and
    /// consumes no depth, so a frame's depths stay dense over the draws that
    /// actually exist.
    ///
    /// The brush is encoded only once the strips are known to be non-empty, so
    /// a culled draw leaves no orphan entry in the frame's encoded-paint table
    /// and no ramp request for a gradient nothing paints with.
    fn record<F>(
        &mut self,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
        brush: &Brush,
        transform: Affine,
        generate: F,
    ) -> bool
    where
        F: FnOnce(&mut StripGenerator, &mut StripStorage),
    {
        let start = frame.strips.strips.len();
        generate(&mut self.generator, &mut frame.strips);
        let strip_range = start..frame.strips.strips.len();

        if strip_range.is_empty() {
            return false;
        }

        let encoding = encode_brush(brush, transform, &mut frame.encoded_paints);
        frame.lut_requests.extend(encoding.lut_request);

        let draw = EngineDraw::new(encoding.paint, depth.advance(), strip_range.clone());
        frame
            .recorder
            .push_draw(draw, &frame.strips.strips[strip_range]);
        true
    }
}

/// The transform a command carries, or `None` for one that carries none.
fn command_transform(command: &Command) -> Option<Affine> {
    match command {
        Command::FillRect { transform, .. }
        | Command::RoundedRect { transform, .. }
        | Command::Line { transform, .. }
        | Command::PushClip { transform, .. }
        | Command::PushClipRounded { transform, .. }
        | Command::Image { transform, .. }
        | Command::BlurredRoundedRect { transform, .. }
        | Command::PushLayer { transform, .. }
        | Command::ClearRect { transform, .. }
        | Command::Path { transform, .. }
        | Command::ShaderQuad { transform, .. }
        | Command::PushSnapshot { transform, .. } => Some(*transform),
        Command::GlyphRun(run) => Some(run.transform),
        Command::PopClip | Command::PopLayer | Command::PopSnapshot => None,
    }
}

/// Refuse a viewport whose tile-snapped extent would not fit in `u16`.
///
/// The recorder snaps the scene size up to whole tiles, and that rounding is
/// checked arithmetic upstream — an extent within three pixels of `u16::MAX`
/// has no representable tile-aligned bound. Refusing it here is what keeps the
/// frame path free of that panic.
fn check_tile_addressable(width: u16, height: u16) -> Result<(), EngineError> {
    let addressable = width.checked_next_multiple_of(Tile::WIDTH).is_some()
        && height.checked_next_multiple_of(Tile::HEIGHT).is_some();

    if addressable {
        Ok(())
    } else {
        Err(EngineError::TargetTooLarge)
    }
}

/// Refuse a transform that maps geometry off the finite device grid.
///
/// A non-finite coefficient (`NaN` from a degenerate inverse, an infinity from
/// an overflowed scale) sends every coordinate it touches outside the `u16`
/// pixel range the strip pipeline addresses, so the frame is refused rather
/// than rasterized into whatever the downstream float-to-integer conversions
/// happen to saturate to.
fn check_finite(transform: Affine) -> Result<(), EngineError> {
    if transform.as_coeffs().iter().all(|c| c.is_finite()) {
        Ok(())
    } else {
        Err(EngineError::InvalidTransform)
    }
}

/// Refuse a command whose own geometry is non-finite.
///
/// A finite transform is not enough on its own: a `NaN` corner radius, an
/// infinite rectangle extent, a `NaN` control point or stroke width all reach
/// the flattener and the stroker as they were recorded, and neither of those
/// bails on a non-finite number. They subdivide against it — a rounded rect of
/// unbounded extent with a `NaN` radius never finishes at all, and a `NaN`
/// stroke width buys hundreds of milliseconds and megabytes of scratch to emit
/// no coverage whatsoever. Refusing here, in the same up-front walk the
/// transforms are checked in, is what bounds the frame path's work by the
/// scene rather than by the arithmetic.
///
/// Only the commands the compiler actually lowers are checked. A command it
/// recognises and skips contributes no geometry to the frame, so refusing the
/// whole frame over one would draw *nothing* where skipping draws less — the
/// weaker outcome. The match is exhaustive over every [`Command`] variant, the
/// same as [`SceneCompiler::compile_command`]'s: a variant added to the enum
/// fails to compile here until it is placed in the lowered group or the
/// skipped one. Moving a variant *between* those two groups is not itself
/// compiler-enforced — the match stays exhaustive either way — so that half of
/// the discipline still has to be kept by hand alongside `compile_command`.
fn check_geometry(command: &Command) -> Result<(), EngineError> {
    let finite = match command {
        Command::FillRect { rect, .. } => rect.is_finite(),
        Command::RoundedRect { rect, radii, .. } => rect.is_finite() && radii_are_finite(*radii),
        Command::Line { p0, p1, width, .. } => {
            p0.is_finite() && p1.is_finite() && width.is_finite()
        }
        Command::Path { path, style, .. } => path.is_finite() && style_is_finite(style),
        // Recognised but not lowered — see above.
        Command::GlyphRun(_)
        | Command::PushClip { .. }
        | Command::PushClipRounded { .. }
        | Command::PopClip
        | Command::Image { .. }
        | Command::BlurredRoundedRect { .. }
        | Command::PushLayer { .. }
        | Command::PopLayer
        | Command::ClearRect { .. }
        | Command::ShaderQuad { .. }
        | Command::PushSnapshot { .. }
        | Command::PopSnapshot => true,
    };

    if finite {
        Ok(())
    } else {
        Err(EngineError::InvalidGeometry)
    }
}

/// Whether every corner radius is finite.
fn radii_are_finite(radii: CornerRadii) -> bool {
    radii.top_left.is_finite()
        && radii.top_right.is_finite()
        && radii.bottom_right.is_finite()
        && radii.bottom_left.is_finite()
}

/// Whether a path style's own numbers are finite.
///
/// The dash lengths and phase are checked even though
/// [`DashPattern::is_effective`] would strike a non-finite pattern out and
/// stroke solid: a frame the compiler refuses for a `NaN` stroke width would
/// otherwise be accepted for a `NaN` dash phase, and one contract over every
/// number a *lowered* command carries is the one a caller can hold in their
/// head — not a claim about a command [`check_geometry`] skips rather than
/// lowers, whose numbers this function never sees.
///
/// An effective dash pattern is checked further, past its own fields: see
/// [`dash_cycle_is_normalizable`].
fn style_is_finite(style: &PathStyle) -> bool {
    match style {
        PathStyle::Fill => true,
        PathStyle::Stroke { width, dash } => {
            let dash_finite = match dash {
                Some(dash) => {
                    dash.on.is_finite()
                        && dash.off.is_finite()
                        && dash.phase.is_finite()
                        && dash_cycle_is_normalizable(dash)
                }
                None => true,
            };
            width.is_finite() && dash_finite
        }
    }
}

/// Whether a dash pattern's derived cycle survives kurbo's own normalization
/// arithmetic, so `kurbo::dash` terminates instead of spinning forever.
///
/// [`DashPattern::is_effective`] already screens out a non-positive or
/// sub-epsilon pattern in favour of a solid stroke, but its own period check —
/// `on + off >= DASH_PERIOD_EPSILON` — can itself be fooled: two individually
/// finite lengths can sum past `f64::MAX` into `+inf`, and `+inf >=
/// DASH_PERIOD_EPSILON` still reads as effective. kurbo doubles this crate's
/// on/off pair into its own length-2 dash array, so the period it derives is
/// always `on + off`; once that overflows, `phase.rem_euclid(period)`
/// overflows with it, and the catch-up loop `kurbo::dash` runs before it ever
/// pulls a `PathEl` adds an infinite step to a value that never converges —
/// `on = off = f64::MAX`, `phase = -1.0` hangs this way. Refusing here, where
/// `on`, `off` and `phase` already passed their own finiteness checks, is what
/// keeps that unbounded loop out of the frame path.
fn dash_cycle_is_normalizable(dash: &DashPattern) -> bool {
    if !dash.is_effective() {
        // A degenerate pattern never reaches `dash_path`: `is_effective` is
        // what routes it to a solid stroke instead, so its derived period is
        // moot here.
        return true;
    }
    let period = dash.on + dash.off;
    period.is_finite() && period > 0.0 && dash.phase.rem_euclid(period).is_finite()
}

/// The device-space rectangle to hand the fast rectangle path, or `None` when
/// this rectangle has to go through full path processing.
///
/// The fast path writes strip coverage for a rectangle directly, skipping
/// flattening and tiling entirely, and is taken only when the result is
/// indistinguishable from the general path: the composed transform must keep
/// the rectangle axis-aligned (no rotation or skew), and the transformed
/// rectangle must land on whole pixels, so no edge needs partial coverage.
fn fast_rect(rect: Rect, transform: Affine) -> Option<Rect> {
    if !is_axis_aligned(&transform) {
        return None;
    }

    let device = transform.transform_rect_bbox(rect);
    is_pixel_aligned(device).then_some(device)
}

/// Whether every edge of `rect` falls on a whole pixel.
fn is_pixel_aligned(rect: Rect) -> bool {
    [rect.x0, rect.y0, rect.x1, rect.y1]
        .iter()
        .all(|v| v.is_finite() && v.fract() == 0.0)
}

/// A stroke of `width` with round caps and joins — the only stroke style the
/// display list can express.
fn round_stroke(width: f64) -> Stroke {
    Stroke::new(width)
        .with_caps(Cap::Round)
        .with_join(Join::Round)
}

/// `path` expanded into the sub-paths `dash` breaks it into.
///
/// Both ends of the expansion go through [`well_formed`]. The input needs it
/// because `kurbo::dash` mishandles a subpath that closes without ever
/// producing a segment: it emits that subpath's closing element ahead of the
/// `MoveTo` meant to open the output, so a path whose *first* subpath is a
/// zero-length closed one (a dashed arc at zero sweep records exactly that)
/// dashes to a sequence beginning with `ClosePath`. Such a sequence is not a
/// path any consumer can read — `BezPath`'s own "begins with `MoveTo`"
/// invariant is asserted in a debug build and silently strokes malformed
/// geometry in a release one. Normalizing those subpaths away first removes
/// the input the iterator gets wrong; normalizing the result as well makes the
/// well-formedness of what this returns a property of this function rather
/// than of the dash iterator's internal states.
///
/// Callers inside this crate only ever reach `dash` here once
/// [`dash_cycle_is_normalizable`] has passed it, since `kurbo::dash` itself
/// does not bound its catch-up loop against a non-normalizable cycle; a caller
/// outside the up-front walk carries that same obligation. Made `pub` (rather
/// than `pub(crate)`) so `frust-testing`'s CPU oracle, which keeps its own
/// independent copy of this lowering (see that crate's `oracle_cpu` module
/// docs for why), can pin its output against this one directly rather than
/// only through a rendered image.
pub fn dash_path(path: &BezPath, dash: DashPattern) -> BezPath {
    let source = well_formed(path.iter());
    well_formed(kurbo::dash(source.iter(), dash.phase, &[dash.on, dash.off]))
}

/// `elements` as a path every consumer can read: opened by a `MoveTo`, and
/// carrying no `ClosePath` that closes a subpath with no segments in it.
///
/// Both rules drop elements that describe no geometry — an element before the
/// first `MoveTo` has no start point to be drawn from, and closing a subpath
/// that never left its start point adds no segment — so a well-formed path in
/// yields itself back unchanged.
///
/// `pub` for the same cross-crate-parity reason as [`dash_path`].
pub fn well_formed(elements: impl Iterator<Item = PathEl>) -> BezPath {
    let mut out = BezPath::new();
    // Tracked rather than read back off `out`: `BezPath::is_empty` asks whether
    // a path holds any SEGMENT, which a path holding only its opening `MoveTo`
    // does not.
    let mut opened = false;
    let mut segments_in_subpath = 0_usize;

    for element in elements {
        match element {
            PathEl::MoveTo(_) => {
                opened = true;
                segments_in_subpath = 0;
                out.push(element);
            }
            PathEl::ClosePath => {
                if segments_in_subpath > 0 {
                    segments_in_subpath = 0;
                    out.push(element);
                }
            }
            PathEl::LineTo(_) | PathEl::QuadTo(..) | PathEl::CurveTo(..) => {
                if opened {
                    segments_in_subpath += 1;
                    out.push(element);
                }
            }
        }
    }
    out
}

/// The display list's per-corner radii as kurbo's, in its clockwise-from-top-left
/// argument order.
fn rounded_rect_radii(radii: CornerRadii) -> RoundedRectRadii {
    RoundedRectRadii::new(
        radii.top_left,
        radii.top_right,
        radii.bottom_right,
        radii.bottom_left,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_viewport_within_three_pixels_of_the_u16_ceiling_is_refused() {
        assert!(check_tile_addressable(65532, 65532).is_ok());
        assert!(matches!(
            check_tile_addressable(65533, 16),
            Err(EngineError::TargetTooLarge)
        ));
        assert!(matches!(
            check_tile_addressable(16, u16::MAX),
            Err(EngineError::TargetTooLarge)
        ));
    }

    #[test]
    fn pixel_alignment_rejects_fractional_edges() {
        assert!(is_pixel_aligned(Rect::new(0.0, 0.0, 4.0, 4.0)));
        assert!(!is_pixel_aligned(Rect::new(0.0, 0.5, 4.0, 4.0)));
        assert!(!is_pixel_aligned(Rect::new(0.0, 0.0, 4.0, f64::INFINITY)));
    }
}
