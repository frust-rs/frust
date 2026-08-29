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

pub mod draw;

pub use draw::{DepthCounter, EngineDraw};

use kurbo::{Affine, BezPath, Cap, Join, Line, Rect, RoundedRect, RoundedRectRadii, Shape, Stroke};
use peniko::{Brush, Color, Fill};

use frust_scene::{Command, CornerRadii, DashPattern, PathStyle, Scene};

use vello_common::encode::EncodedPaint;
use vello_common::fearless_simd::Level;
use vello_common::paint::Paint;
use vello_common::record::CommandRecorder;
use vello_common::strip_generator::{GenerationMode, StripGenerator, StripStorage};
use vello_common::tile::Tile;
use vello_common::util::is_axis_aligned;

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
    /// [`Paint::Indexed`].
    pub encoded_paints: Vec<EncodedPaint>,
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
    /// [`EngineError::TargetTooLarge`] when the frame cannot be addressed on
    /// the u16 device grid: either `size` cannot be rounded up to whole tiles
    /// inside `u16`, or a composed transform is non-finite and so maps
    /// geometry to coordinates no `u16` pixel can hold. Both are refused
    /// before any strip is generated — the frame path returns errors and never
    /// panics.
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
        }

        self.generator.reset(width, height);

        let mut frame = CompiledFrame {
            strips: StripStorage::new(GenerationMode::Append),
            recorder: CommandRecorder::new(width, height),
            encoded_paints: Vec::new(),
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
                let paint = solid_paint(brush);

                if let Some(device_rect) = fast_rect(*rect, transform) {
                    let recorded = self.record(frame, depth, paint, |generator, storage| {
                        generator.generate_filled_rect_fast(&device_rect, storage, None);
                    });
                    if recorded {
                        frame.fast_rect_draws = frame.fast_rect_draws.saturating_add(1);
                    }
                } else {
                    self.record(frame, depth, paint, |generator, storage| {
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
                let paint = solid_paint(brush);
                let shape = RoundedRect::from_rect(*rect, rounded_rect_radii(*radii));

                self.record(frame, depth, paint, |generator, storage| {
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
                let paint = solid_paint(brush);
                let line = Line::new(*p0, *p1);
                let stroke = round_stroke(*width);

                self.record(frame, depth, paint, |generator, storage| {
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
                let paint = solid_paint(brush);

                match style {
                    PathStyle::Fill => {
                        self.record(frame, depth, paint, |generator, storage| {
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
                                self.record(frame, depth, paint, |generator, storage| {
                                    generator.generate_stroked_path(
                                        dashed.iter(),
                                        &stroke,
                                        transform,
                                        None,
                                        storage,
                                        None,
                                    );
                                });
                            }
                            None => {
                                self.record(frame, depth, paint, |generator, storage| {
                                    generator.generate_stroked_path(
                                        path.iter(),
                                        &stroke,
                                        transform,
                                        None,
                                        storage,
                                        None,
                                    );
                                });
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

    /// Run `generate`, then record whatever strips it appended as one draw.
    ///
    /// Returns whether a draw was recorded. A generator call that produced no
    /// strips (fully culled, degenerate, or empty geometry) records nothing and
    /// consumes no depth, so a frame's depths stay dense over the draws that
    /// actually exist.
    fn record<F>(
        &mut self,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
        paint: Paint,
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

        let draw = EngineDraw::new(paint, depth.advance(), strip_range.clone());
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
        Err(EngineError::TargetTooLarge)
    }
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
fn dash_path(path: &BezPath, dash: DashPattern) -> BezPath {
    kurbo::dash(path.iter(), dash.phase, &[dash.on, dash.off]).collect()
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

/// A brush as a solid paint.
///
/// The solid-only seam the paint-encoding pass replaces: a gradient resolves to
/// its first stop and an image brush to opaque black, both stand-ins that keep
/// a draw recorded (and therefore its geometry exercised) rather than dropping
/// it. Neither approximation is rendered anywhere yet.
fn solid_paint(brush: &Brush) -> Paint {
    let color = match brush {
        Brush::Solid(color) => *color,
        Brush::Gradient(gradient) => gradient
            .stops
            .first()
            .map_or(Color::BLACK, |stop| stop.color.to_alpha_color()),
        Brush::Image(_) => Color::BLACK,
    };

    Paint::from(color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use peniko::color::palette::css::RED;

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

    #[test]
    fn a_gradient_brush_falls_back_to_its_first_stop() {
        let gradient = peniko::Gradient::new_linear((0.0, 0.0), (10.0, 0.0))
            .with_stops([RED, peniko::color::palette::css::BLUE]);
        assert_eq!(solid_paint(&Brush::Gradient(gradient)), Paint::from(RED));
    }
}
