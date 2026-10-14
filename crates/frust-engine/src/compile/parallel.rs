//! What a shape command rasterizes, described once.
//!
//! Every strip-generated shape — a filled or stroked path, a rectangle, a
//! rounded rectangle, a line, an image's or a blurred shadow's destination
//! rectangle — reaches the strip generator as a [`Geometry`]: the command's
//! own shape resolved against its composed transform. The walk records a draw
//! from it without knowing which kind of shape it holds, and anything else
//! that generates a command's strips generates them from the same value, so no
//! two paths can ever lower one command differently.

use kurbo::{Affine, BezPath, PathEl, Rect, RoundedRect, Shape};
use peniko::Fill;

use frust_scene::{Command, DashPattern, PathStyle};

use vello_common::clip::PathDataRef;
use vello_common::strip_generator::{StripGenerator, StripStorage};

use super::blur_rrect::inflated_bounds;
use super::cull::{generate_fill, generate_stroke, rounded_rect_elements};
use super::{FLATTEN_TOLERANCE, dash_path, fast_rect, round_stroke, rounded_rect_radii};

/// What one drawing command rasterizes, resolved against its composed
/// transform.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Geometry<'a> {
    /// A pixel-aligned device rectangle, written directly as coverage.
    FastRect(Rect),
    /// A rectangle that needs full path processing.
    RectPath {
        /// The rectangle in the command's own space.
        rect: Rect,
        /// The composed transform.
        transform: Affine,
    },
    /// A filled rounded rectangle.
    Rounded {
        /// The shape in the command's own space.
        shape: RoundedRect,
        /// The composed transform.
        transform: Affine,
    },
    /// A stroked line segment, round-capped.
    Line {
        /// The segment's two elements.
        line: [PathEl; 2],
        /// The stroke width.
        width: f64,
        /// The composed transform.
        transform: Affine,
    },
    /// A filled path.
    Fill {
        /// The path in the command's own space.
        path: &'a BezPath,
        /// The composed transform.
        transform: Affine,
    },
    /// A stroked path, round-capped and round-joined, dashed when `dash` is
    /// an effective pattern.
    Stroke {
        /// The path in the command's own space.
        path: &'a BezPath,
        /// The stroke width.
        width: f64,
        /// The dash pattern, present only when it is effective.
        dash: Option<DashPattern>,
        /// The composed transform.
        transform: Affine,
    },
}

impl<'a> Geometry<'a> {
    /// The geometry of a rectangle drawn under `transform`: the fast
    /// rectangle path whenever [`fast_rect`] admits it.
    pub(crate) fn rect(rect: Rect, transform: Affine) -> Self {
        match fast_rect(rect, transform) {
            Some(device) => Self::FastRect(device),
            None => Self::RectPath { rect, transform },
        }
    }

    /// The geometry `command` rasterizes under `combined` (the frame root with
    /// any snapshot correction), or `None` for a command that is not a
    /// strip-generated shape — a bracket, a glyph run, an external texture.
    pub(crate) fn of(command: &'a Command, combined: Affine) -> Option<Self> {
        let geometry = match command {
            Command::FillRect {
                rect, transform, ..
            } => Self::rect(*rect, combined * *transform),
            Command::RoundedRect {
                rect,
                radii,
                transform,
                ..
            } => Self::Rounded {
                shape: RoundedRect::from_rect(*rect, rounded_rect_radii(*radii)),
                transform: combined * *transform,
            },
            Command::Line {
                p0,
                p1,
                width,
                transform,
                ..
            } => Self::Line {
                // The two elements `kurbo::Line::path_elements` yields, spelled
                // out so the stroke helper can walk them twice.
                line: [PathEl::MoveTo(*p0), PathEl::LineTo(*p1)],
                width: *width,
                transform: combined * *transform,
            },
            Command::Path {
                path,
                style,
                transform,
                ..
            } => match style {
                PathStyle::Fill => Self::Fill {
                    path,
                    transform: combined * *transform,
                },
                PathStyle::Stroke { width, dash } => Self::Stroke {
                    path,
                    width: *width,
                    dash: dash.filter(DashPattern::is_effective),
                    transform: combined * *transform,
                },
            },
            Command::Image {
                dest, transform, ..
            } => Self::rect(*dest, combined * *transform),
            // The strip generator rasterizes the padded bounding rectangle,
            // not the rounded shape (see [`super::blur_rrect`]).
            Command::BlurredRoundedRect {
                rect,
                std_dev,
                transform,
                ..
            } => Self::rect(inflated_bounds(*rect, *std_dev), combined * *transform),
            _ => return None,
        };
        Some(geometry)
    }

    /// Whether this is the fast rectangle path.
    pub(crate) fn is_fast_rect(&self) -> bool {
        matches!(self, Self::FastRect(_))
    }

    /// Generate this geometry's strips into `storage` under `clip`.
    ///
    /// The one place a shape command's generator call is spelled.
    pub(crate) fn generate(
        &self,
        generator: &mut StripGenerator,
        storage: &mut StripStorage,
        clip: Option<PathDataRef<'_>>,
    ) {
        match *self {
            Self::FastRect(device) => {
                generator.generate_filled_rect_fast(&device, storage, clip);
            }
            Self::RectPath { rect, transform } => {
                generator.generate_filled_path(
                    rect.path_elements(FLATTEN_TOLERANCE),
                    Fill::NonZero,
                    transform,
                    None,
                    storage,
                    clip,
                );
            }
            Self::Rounded { shape, transform } => {
                generate_fill(
                    generator,
                    rounded_rect_elements(&shape),
                    transform,
                    storage,
                    clip,
                );
            }
            Self::Line {
                line,
                width,
                transform,
            } => {
                generate_stroke(
                    generator,
                    line,
                    &round_stroke(width),
                    transform,
                    storage,
                    clip,
                );
            }
            Self::Fill { path, transform } => {
                generate_fill(generator, path.iter(), transform, storage, clip);
            }
            Self::Stroke {
                path,
                width,
                dash,
                transform,
            } => {
                let stroke = round_stroke(width);
                // A dash pattern is expanded into its own sub-paths before the
                // stroker runs, the same lowering the display list's other
                // consumers apply: the pattern never reaches a backend's own
                // dash support, so every rasterizer sees the identical
                // geometry.
                match dash {
                    Some(dash) => {
                        let dashed = dash_path(path, dash);
                        generate_stroke(
                            generator,
                            dashed.iter(),
                            &stroke,
                            transform,
                            storage,
                            clip,
                        );
                    }
                    None => {
                        generate_stroke(generator, path.iter(), &stroke, transform, storage, clip);
                    }
                }
            }
        }
    }
}
