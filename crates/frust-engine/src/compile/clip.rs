//! The compiler's clip stack, and the two shapes a clip lowers to.
//!
//! `frust_scene` has one clip stack, not two: `PushClip` and `PushClipRounded`
//! both push onto it and one `PopClip` pops either. [`ClipStack`] is that stack
//! inside the compiler, and its whole point is that neither shape ever needs an
//! intermediate texture — a clip is either a device-space rectangle intersected
//! into the enclosing one, or a coverage mask the strip generator already knows
//! how to intersect a draw against.
//!
//! **Scissor.** A rectangular clip whose composed transform leaves it
//! axis-aligned and whose edges land on whole device pixels is kept as a plain
//! [`RectU16`], intersected with the enclosing scissor. Nothing is rasterized
//! for it at all: once a draw's strips are generated, [`ClipStack::clip_run`]
//! rewrites that run so no coverage outside the rectangle survives. The
//! admission rule is [`fast_rect`]'s, deliberately — the rectangle a clip can be
//! answered by scissoring is exactly the rectangle a fill can be answered by
//! writing strip coverage directly.
//!
//! **Mask.** Every other clip — rounded corners, a rotation or skew in the
//! transform, an edge on a half pixel — is rasterized once into a coverage mask
//! held by [`ClipContext`], and subsequent draws are generated with that mask as
//! their clip path. Nesting is that library's own: a mask pushed inside another
//! is generated against the enclosing one, so the top of the stack is always the
//! full intersection.
//!
//! The two compose in either order. A mask is applied by the strip generator
//! while a draw's coverage is produced; a scissor is applied to the result. A
//! draw under both is clipped by both, and neither lowering has to know the
//! other happened.
//!
//! # Rewriting a strip run
//!
//! Scissoring reaches into the sparse-strip encoding rather than into geometry,
//! so it has to respect that encoding's alignment rules. A strip's `x` and its
//! width are both whole tiles (`vello_common`'s own `visit_strip_fill_segments`
//! asserts as much), and one strip row covers `Tile::HEIGHT` scanlines at once.
//! A scissor edge is only pixel-aligned, so it generally falls *inside* a tile
//! and inside a row.
//!
//! The run is therefore rewritten by masking coverage rather than by moving
//! edges: a span crossing the scissor boundary keeps its tile-aligned extent and
//! gets a fresh run of coverage bytes with every pixel outside the rectangle set
//! to zero. The interior of a solid span stays solid — only the tile an edge
//! falls inside is turned into coverage — so a full-screen fill under a
//! scrolling clip does not become a full-screen alpha buffer.

use kurbo::{Affine, PathEl, Rect, RoundedRect, RoundedRectRadii, Shape};
use peniko::Fill;

use vello_common::clip::{ClipContext, PathDataRef};
use vello_common::geometry::RectU16;
use vello_common::strip::Strip;
use vello_common::strip_generator::{StripGenerator, StripStorage};
use vello_common::tile::Tile;

use super::cull::ViewportSplit;
use super::{FLATTEN_TOLERANCE, cull_viewport, fast_rect};

/// The scissor of a stack that clips nothing.
///
/// Deliberately the whole `u16` grid rather than the viewport: the strip
/// pipeline addresses a viewport snapped up to whole tiles, and a draw is
/// allowed to carry coverage into that snapped margin. Starting from the
/// viewport would quietly clip that margin away on any frame that pushed a clip,
/// making a clip's presence change what an unclipped draw looks like.
const UNCLIPPED: RectU16 = RectU16::new(0, 0, u16::MAX, u16::MAX);

/// Coverage of a fully covered pixel.
const FULL_COVERAGE: u8 = 255;

/// The largest alpha index a [`Strip`] can carry: its packed field reserves the
/// top bit for the fill-gap flag.
const MAX_ALPHA_INDEX: u32 = u32::MAX >> 1;

/// One entry of the stack, naming what its matching pop has to undo.
#[derive(Debug)]
enum Entry {
    /// A rectangle intersected into the scissor, carrying the scissor to put
    /// back. Intersection is not invertible, so the previous value is stored
    /// rather than recomputed.
    Scissor { restore: RectU16 },
    /// A coverage mask, popped from the mask context.
    Mask,
}

/// One span of a draw's strip run, in the form the renderer reads back out: an
/// alpha-sampled span, or a solid one filling the gap to the next strip.
#[derive(Debug, Clone, Copy)]
struct Span {
    /// Top scanline of the strip row, a multiple of `Tile::HEIGHT`.
    y: u16,
    /// Left edge in pixels, a multiple of `Tile::WIDTH`.
    x: u16,
    /// Width in pixels, a multiple of `Tile::WIDTH`.
    width: u16,
    /// First coverage byte of the span, or `None` when it is solid.
    alpha_idx: Option<u32>,
}

impl Span {
    /// Right edge in pixels, exclusive.
    fn x1(&self) -> u32 {
        u32::from(self.x) + u32::from(self.width)
    }
}

/// The compiler's clip stack: scissor rectangles and coverage masks, one stack.
///
/// Retained across frames like the strip generator it works beside — the mask
/// context and the rewrite scratch are exactly the buffers a steady-state frame
/// should be reusing. [`ClipStack::reset`] is what keeps it from carrying state
/// between frames.
#[derive(Debug)]
pub struct ClipStack {
    /// One entry per unpopped push, in push order.
    entries: Vec<Entry>,
    /// The coverage masks, nested by `vello_common`'s own clip context.
    masks: ClipContext,
    /// The intersection of every scissor currently on the stack.
    scissor: RectU16,
    /// Rewritten strips, staged here before replacing a draw's own run.
    strips: Vec<Strip>,
    /// Coverage for `strips`, offset from the run's own alpha start.
    alphas: Vec<u8>,
    /// Clips lowered to a scissor this frame.
    scissor_clips: u32,
    /// Clips lowered to a coverage mask this frame.
    mask_clips: u32,
    /// Strips this frame's coverage masks cost.
    mask_strips: usize,
}

impl Default for ClipStack {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipStack {
    /// An empty stack, clipping nothing.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            masks: ClipContext::new(),
            scissor: UNCLIPPED,
            strips: Vec::new(),
            alphas: Vec::new(),
            scissor_clips: 0,
            mask_clips: 0,
            mask_strips: 0,
        }
    }

    /// Drop every clip and counter, keeping the buffers.
    pub fn reset(&mut self) {
        self.entries.clear();
        self.masks.reset();
        self.scissor = UNCLIPPED;
        self.scissor_clips = 0;
        self.mask_clips = 0;
        self.mask_strips = 0;
    }

    /// Clips lowered to a scissor this frame.
    pub fn scissor_clips(&self) -> u32 {
        self.scissor_clips
    }

    /// Clips lowered to a coverage mask this frame.
    pub fn mask_clips(&self) -> u32 {
        self.mask_clips
    }

    /// Strips this frame's coverage masks cost — zero for a frame whose clips
    /// all scissored.
    pub fn mask_strips(&self) -> usize {
        self.mask_strips
    }

    /// The mask a draw generated now has to be clipped against, or `None` when
    /// no mask is on the stack.
    pub fn mask(&self) -> Option<PathDataRef<'_>> {
        self.masks.get()
    }

    /// Whether the current scissor admits nothing, so a draw need not be
    /// generated at all.
    pub fn blocks_everything(&self) -> bool {
        self.scissor.is_empty()
    }

    /// Push a rectangular clip: a scissor when the composed transform admits
    /// one, a coverage mask otherwise.
    pub fn push_rect(&mut self, rect: Rect, transform: Affine, generator: &mut StripGenerator) {
        match fast_rect(rect, transform) {
            Some(device) => self.push_scissor(device),
            None => self.push_mask(rect.path_elements(FLATTEN_TOLERANCE), transform, generator),
        }
    }

    /// Push a clip with rounded corners.
    ///
    /// Radii that are all square describe a plain rectangle, so such a clip
    /// takes the rectangular path and can still scissor: a caller spelling an
    /// unrounded clip through the rounded command pays nothing for the spelling.
    pub fn push_rounded(
        &mut self,
        rect: Rect,
        radii: RoundedRectRadii,
        transform: Affine,
        generator: &mut StripGenerator,
    ) {
        if radii_are_square(radii) {
            self.push_rect(rect, transform, generator);
            return;
        }
        let shape = RoundedRect::from_rect(rect, radii);
        self.push_mask(shape.path_elements(FLATTEN_TOLERANCE), transform, generator);
    }

    /// Pop the most recent clip.
    ///
    /// A pop with nothing to pop is ignored. A display list is recorded by a
    /// widget tree that can be unbalanced, and an unbalanced pop must not be
    /// able to lift a clip a sibling still relies on — nor to underflow the mask
    /// context, which would be a panic on the frame path.
    pub fn pop(&mut self) {
        match self.entries.pop() {
            Some(Entry::Scissor { restore }) => self.scissor = restore,
            Some(Entry::Mask) => self.masks.pop_clip(),
            None => {}
        }
    }

    fn push_scissor(&mut self, device: Rect) {
        let restore = self.scissor;
        self.scissor = self.scissor.intersect(device_rect(device));
        self.entries.push(Entry::Scissor { restore });
        self.scissor_clips = self.scissor_clips.saturating_add(1);
    }

    /// Rasterize `path` under `transform` into a mask nested in the current
    /// one.
    ///
    /// The mask is generated against the enclosing mask, so it is culled
    /// against that mask's bounds — or the generator's viewport when there is
    /// none — and the path goes through the [`ViewportSplit`] pre-pass against
    /// that same rectangle before it reaches the flattener.
    fn push_mask(
        &mut self,
        path: impl IntoIterator<Item = PathEl>,
        transform: Affine,
        generator: &mut StripGenerator,
    ) {
        let viewport = cull_viewport(generator, self.masks.get().as_ref());
        self.masks.push_clip(
            ViewportSplit::new(path, transform, viewport),
            generator,
            Fill::NonZero,
            Affine::IDENTITY,
            None,
        );
        self.entries.push(Entry::Mask);
        self.mask_clips = self.mask_clips.saturating_add(1);
        self.mask_strips = self
            .mask_strips
            .saturating_add(self.masks.get().map_or(0, |mask| mask.strips.len()));
    }

    /// Rewrite the strip run a draw just generated so nothing outside the
    /// scissor survives.
    ///
    /// `strip_start` and `alpha_start` are `storage`'s two lengths from
    /// immediately before the draw generated, so everything past them belongs to
    /// this draw alone and can be replaced wholesale. The rewrite reclaims the
    /// coverage it drops — the original bytes are truncated away and only the
    /// kept ones re-appended — so a clipped frame's alpha buffer stays
    /// proportional to what it actually paints.
    ///
    /// A run that needs no change is left exactly as it was, which is the whole
    /// unclipped case and most of the clipped one.
    pub fn clip_run(&mut self, storage: &mut StripStorage, strip_start: usize, alpha_start: usize) {
        if self.scissor == UNCLIPPED {
            return;
        }
        let Some(run) = storage.strips.get(strip_start..) else {
            return;
        };
        if !spans(run).any(|span| self.clips(&span)) {
            return;
        }

        self.strips.clear();
        self.alphas.clear();

        let base = alpha_start.min(storage.alphas.len());
        let scissor = self.scissor;
        for span in spans(run) {
            if let Some(clipped) = Clipped::of(&span, scissor) {
                self.emit(&span, &clipped, &storage.alphas, base);
            }
        }
        self.close_run(base);

        storage.strips.truncate(strip_start);
        storage.alphas.truncate(base);
        storage.strips.extend_from_slice(&self.strips);
        storage.alphas.extend_from_slice(&self.alphas);
    }

    /// Whether `span` would come out of the scissor changed — dropped, trimmed,
    /// or masked.
    fn clips(&self, span: &Span) -> bool {
        Clipped::of(span, self.scissor).is_none_or(|clipped| !clipped.covers_all_of(span))
    }

    /// Terminate the staged run with the sentinel strip the renderer's pairwise
    /// walk reads the last span's extent off.
    ///
    /// A run that staged nothing gets no sentinel: a lone sentinel is not a run,
    /// and the caller drops an empty range rather than recording a draw for it.
    fn close_run(&mut self, base: usize) {
        let Some(last) = self.strips.last() else {
            return;
        };
        let y = last.y;
        let end = alpha_index(base.saturating_add(self.alphas.len()));
        self.strips.push(Strip::sentinel(y, end));
    }

    /// Append `span`, clipped, to the staged run.
    fn emit(&mut self, span: &Span, clipped: &Clipped, alphas: &[u8], base: usize) {
        // A solid span keeps its whole-tile interior solid; only a tile the
        // scissor edge falls inside has to start carrying coverage. An alpha
        // span already pays for coverage across its width, so splitting it would
        // buy nothing.
        if span.alpha_idx.is_none()
            && clipped.rows_are_whole()
            && let Some((x0, x1)) = clipped.interior_tiles()
        {
            self.emit_split_solid(span, clipped, (x0, x1), alphas, base);
            return;
        }

        let (x0, x1) = clipped.tiles();
        self.emit_masked(span, clipped, (x0, x1), alphas, base);
    }

    /// Emit a solid span as a left boundary tile, a solid interior, and a right
    /// boundary tile — either boundary tile may be absent, and both are when the
    /// scissor did not cut this span at all.
    fn emit_split_solid(
        &mut self,
        span: &Span,
        clipped: &Clipped,
        interior: (u32, u32),
        alphas: &[u8],
        base: usize,
    ) {
        let (tile_x0, tile_x1) = clipped.tiles();
        let (interior_x0, interior_x1) = interior;
        if tile_x0 < interior_x0 {
            self.emit_masked(span, clipped, (tile_x0, interior_x0), alphas, base);
        }
        self.emit_solid(
            pixel(interior_x0),
            span.y,
            pixel(interior_x1.saturating_sub(interior_x0)),
            base,
        );
        if interior_x1 < tile_x1 {
            self.emit_masked(span, clipped, (interior_x1, tile_x1), alphas, base);
        }
    }

    /// Emit `[x0, x1)` of `span` as coverage, with every pixel the scissor
    /// excludes zeroed.
    ///
    /// `[x0, x1)` is always whole tiles inside the span's own extent, which is
    /// what keeps the emitted strip tile-aligned in both position and width
    /// however the scissor edge falls.
    fn emit_masked(
        &mut self,
        span: &Span,
        clipped: &Clipped,
        extent: (u32, u32),
        alphas: &[u8],
        base: usize,
    ) {
        let (x0, x1) = extent;
        let width = x1.saturating_sub(x0);
        if width == 0 {
            return;
        }

        let origin = u32::from(span.x);
        let start = alpha_index(base.saturating_add(self.alphas.len()));
        for column in 0..width {
            let x = x0.saturating_add(column);
            for row in 0..u32::from(Tile::HEIGHT) {
                let value = match span.alpha_idx {
                    _ if !clipped.contains(x, row) => 0,
                    None => FULL_COVERAGE,
                    Some(idx) => coverage_at(alphas, idx, x.saturating_sub(origin), row),
                };
                self.alphas.push(value);
            }
        }
        self.strips
            .push(Strip::new(pixel(x0), span.y, start, false));
    }

    /// Stage a solid span of `width` pixels at `(x, y)`.
    ///
    /// A solid span is two strips: one of zero width opening it, and one
    /// carrying the fill-gap flag closing it. That is how the encoding says
    /// "fill from here to there, with no coverage to sample".
    fn emit_solid(&mut self, x: u16, y: u16, width: u16, base: usize) {
        if width == 0 {
            return;
        }
        let index = alpha_index(base.saturating_add(self.alphas.len()));
        self.strips.push(Strip::new(x, y, index, false));
        self.strips
            .push(Strip::new(x.saturating_add(width), y, index, true));
    }
}

/// A span's surviving extent under a scissor.
#[derive(Debug, Clone, Copy)]
struct Clipped {
    /// Left edge in pixels, inclusive.
    x0: u32,
    /// Right edge in pixels, exclusive.
    x1: u32,
    /// First surviving scanline of the strip row, `0..Tile::HEIGHT`.
    row0: u32,
    /// One past the last surviving scanline.
    row1: u32,
}

impl Clipped {
    /// `span` under `scissor`, or `None` when nothing of it survives.
    fn of(span: &Span, scissor: RectU16) -> Option<Self> {
        let top = u32::from(span.y);
        let bottom = top.saturating_add(u32::from(Tile::HEIGHT));
        let row0 = u32::from(scissor.y0).clamp(top, bottom) - top;
        let row1 = u32::from(scissor.y1).clamp(top, bottom) - top;
        if row0 >= row1 {
            return None;
        }

        let x0 = u32::from(span.x).max(u32::from(scissor.x0));
        let x1 = span.x1().min(u32::from(scissor.x1));
        if x0 >= x1 {
            return None;
        }

        Some(Self { x0, x1, row0, row1 })
    }

    /// Whether the whole span survived untouched.
    fn covers_all_of(&self, span: &Span) -> bool {
        self.rows_are_whole() && self.x0 == u32::from(span.x) && self.x1 == span.x1()
    }

    /// Whether every scanline of the strip row survived.
    fn rows_are_whole(&self) -> bool {
        self.row0 == 0 && self.row1 == u32::from(Tile::HEIGHT)
    }

    /// Whether device column `x`, scanline `row` of the strip row, is inside the
    /// scissor.
    fn contains(&self, x: u32, row: u32) -> bool {
        x >= self.x0 && x < self.x1 && row >= self.row0 && row < self.row1
    }

    /// The whole tiles the surviving extent touches, in pixels.
    fn tiles(&self) -> (u32, u32) {
        (tile_floor(self.x0), tile_ceil(self.x1))
    }

    /// The whole tiles lying entirely inside the surviving extent, in pixels, or
    /// `None` when the extent covers no whole tile.
    fn interior_tiles(&self) -> Option<(u32, u32)> {
        let x0 = tile_ceil(self.x0);
        let x1 = tile_floor(self.x1);
        (x0 < x1).then_some((x0, x1))
    }
}

/// The spans a strip run describes, read exactly as the renderer reads them:
/// each strip's own alpha-sampled extent, plus the solid extent filling the gap
/// to the next strip when the winding between them says there is one.
///
/// Decoding through the same rule the renderer applies is what makes the rewrite
/// faithful: a span this iterator does not report is a span nothing downstream
/// would have drawn either.
fn spans(run: &[Strip]) -> impl Iterator<Item = Span> + '_ {
    run.windows(2)
        .flat_map(|pair| {
            let [strip, next] = pair else {
                return [None, None];
            };
            if strip.is_sentinel() {
                return [None, None];
            }

            let width = strip.width_to(next);
            let alpha = (width > 0).then(|| Span {
                y: strip.y,
                x: strip.x,
                width,
                alpha_idx: Some(strip.alpha_idx()),
            });

            let gap = if next.fill_gap() && next.y == strip.y {
                let x = strip.x.saturating_add(width);
                let gap_width = next.x.saturating_sub(x);
                (gap_width > 0).then_some(Span {
                    y: strip.y,
                    x,
                    width: gap_width,
                    alpha_idx: None,
                })
            } else {
                None
            };

            [alpha, gap]
        })
        .flatten()
}

/// The coverage byte for `column`, `row` of the span starting at `alpha_idx`.
///
/// Coverage is stored column-major, `Tile::HEIGHT` bytes per pixel column — the
/// same unit the strip shader reads it back in. A byte past the end of the
/// buffer reads as uncovered rather than panicking: the frame path returns
/// errors, and a short coverage buffer is not one of them.
fn coverage_at(alphas: &[u8], alpha_idx: u32, column: u32, row: u32) -> u8 {
    let offset = column
        .saturating_mul(u32::from(Tile::HEIGHT))
        .saturating_add(row);
    let index = alpha_idx.saturating_add(offset) as usize;
    alphas.get(index).copied().unwrap_or(0)
}

/// `pixels` as a strip coordinate, saturating at the grid strips address.
fn pixel(pixels: u32) -> u16 {
    u16::try_from(pixels).unwrap_or(u16::MAX)
}

/// `index` as a strip's alpha index, saturating below the flag bit the packed
/// field reserves.
fn alpha_index(index: usize) -> u32 {
    u32::try_from(index)
        .unwrap_or(MAX_ALPHA_INDEX)
        .min(MAX_ALPHA_INDEX)
}

/// `x` rounded down to a tile boundary.
fn tile_floor(x: u32) -> u32 {
    x - x % u32::from(Tile::WIDTH)
}

/// `x` rounded up to a tile boundary.
fn tile_ceil(x: u32) -> u32 {
    tile_floor(x.saturating_add(u32::from(Tile::WIDTH) - 1))
}

/// Whether every corner radius describes a square corner.
fn radii_are_square(radii: RoundedRectRadii) -> bool {
    radii.top_left <= 0.0
        && radii.top_right <= 0.0
        && radii.bottom_right <= 0.0
        && radii.bottom_left <= 0.0
}

/// A pixel-aligned device rectangle on the `u16` grid strips address.
///
/// The rectangle's edges are already whole numbers — that is what admitted it to
/// the scissor path — but not necessarily small ones: a clip far outside the
/// viewport is clamped rather than refused, which turns an enormous clip into
/// "clips nothing" and an entirely negative one into "clips everything", both of
/// which are what the geometry says.
fn device_rect(rect: Rect) -> RectU16 {
    let coordinate = |value: f64| value.clamp(0.0, f64::from(u16::MAX)) as u16;
    RectU16::new(
        coordinate(rect.x0),
        coordinate(rect.y0),
        coordinate(rect.x1),
        coordinate(rect.y1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use vello_common::fearless_simd::Level;

    fn generator() -> StripGenerator {
        StripGenerator::new(64, 64, Level::baseline())
    }

    #[test]
    fn a_run_decodes_into_its_alpha_and_solid_spans() {
        // Four pixels of coverage at x = 0, a solid gap on to x = 12, then the
        // sentinel closing the row.
        let run = [
            Strip::new(0, 0, 0, false),
            Strip::new(12, 0, 16, true),
            Strip::sentinel(0, 16),
        ];
        let decoded: Vec<Span> = spans(&run).collect();

        assert_eq!(decoded.len(), 2);
        assert_eq!((decoded[0].x, decoded[0].width), (0, 4));
        assert_eq!(decoded[0].alpha_idx, Some(0));
        assert_eq!((decoded[1].x, decoded[1].width), (4, 8));
        assert_eq!(decoded[1].alpha_idx, None);
    }

    #[test]
    fn a_span_the_scissor_misses_does_not_survive() {
        let span = Span {
            y: 0,
            x: 0,
            width: 8,
            alpha_idx: None,
        };
        assert!(Clipped::of(&span, RectU16::new(16, 0, 32, 4)).is_none());
        assert!(Clipped::of(&span, RectU16::new(0, 8, 32, 12)).is_none());
        assert!(Clipped::of(&span, RectU16::new(0, 0, 8, 4)).is_some());
    }

    #[test]
    fn tile_rounding_brackets_a_pixel_extent() {
        assert_eq!(tile_floor(5), 4);
        assert_eq!(tile_ceil(5), 8);
        assert_eq!(tile_floor(8), 8);
        assert_eq!(tile_ceil(8), 8);
    }

    #[test]
    fn interior_tiles_are_only_the_whole_ones() {
        let span = Span {
            y: 0,
            x: 0,
            width: 32,
            alpha_idx: None,
        };
        let wide = Clipped::of(&span, RectU16::new(5, 0, 19, 4)).expect("overlaps");
        assert_eq!(wide.interior_tiles(), Some((8, 16)));

        let narrow = Clipped::of(&span, RectU16::new(5, 0, 7, 4)).expect("overlaps");
        assert_eq!(narrow.interior_tiles(), None);
    }

    #[test]
    fn an_enormous_clip_rectangle_clamps_rather_than_wrapping() {
        let huge = device_rect(Rect::new(-1e30, -1e30, 1e30, 1e30));
        assert_eq!(huge, UNCLIPPED);

        let behind = device_rect(Rect::new(-1e30, -1e30, -1e29, -1e29));
        assert!(behind.is_empty());
    }

    #[test]
    fn an_unbalanced_pop_leaves_the_stack_alone() {
        let mut stack = ClipStack::new();
        stack.pop();
        stack.pop();

        assert!(!stack.blocks_everything());
        assert_eq!(stack.scissor_clips(), 0);
        assert_eq!(stack.mask_clips(), 0);
    }

    #[test]
    fn nested_scissors_intersect_and_unwind() {
        let mut generator = generator();
        let mut stack = ClipStack::new();

        stack.push_rect(
            Rect::new(0.0, 0.0, 40.0, 40.0),
            Affine::IDENTITY,
            &mut generator,
        );
        stack.push_rect(
            Rect::new(20.0, 20.0, 60.0, 60.0),
            Affine::IDENTITY,
            &mut generator,
        );
        assert_eq!(stack.scissor, RectU16::new(20, 20, 40, 40));

        stack.pop();
        assert_eq!(stack.scissor, RectU16::new(0, 0, 40, 40));
        stack.pop();
        assert_eq!(stack.scissor, UNCLIPPED);

        assert_eq!(stack.scissor_clips(), 2);
        assert_eq!(stack.mask_strips(), 0);
    }

    #[test]
    fn a_square_cornered_rounded_clip_still_scissors() {
        let mut generator = generator();
        let mut stack = ClipStack::new();

        stack.push_rounded(
            Rect::new(4.0, 4.0, 20.0, 20.0),
            RoundedRectRadii::from_single_radius(0.0),
            Affine::IDENTITY,
            &mut generator,
        );

        assert_eq!(stack.scissor_clips(), 1);
        assert_eq!(stack.mask_clips(), 0);
        assert_eq!(stack.mask_strips(), 0);
    }

    #[test]
    fn a_rounded_clip_rasterizes_a_mask() {
        let mut generator = generator();
        let mut stack = ClipStack::new();

        stack.push_rounded(
            Rect::new(4.0, 4.0, 40.0, 40.0),
            RoundedRectRadii::from_single_radius(8.0),
            Affine::IDENTITY,
            &mut generator,
        );

        assert_eq!(stack.mask_clips(), 1);
        assert!(stack.mask_strips() > 0);
        assert!(stack.mask().is_some());

        stack.pop();
        assert!(stack.mask().is_none());
    }
}
