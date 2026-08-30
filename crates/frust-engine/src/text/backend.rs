//! The `glifo` backend the engine draws a glyph run through, and the draw sink
//! it drives.
//!
//! Two types, because `glifo` splits the work in two. [`EngineTextBackend`] is
//! the [`GlyphRunBackend`] a [`GlyphRunBuilder`](glifo::GlyphRunBuilder) is
//! parameterised on: it is consumed once per run and its only job is to hand
//! `glifo` the caches it prepares glyphs against and the sink it draws them
//! into. [`EngineGlyphSink`] is that sink — the [`DrawSink`]/[`GlyphRenderer`]
//! pair `glifo` issues per-glyph drawing commands to, which is where a glyph
//! actually becomes engine strips.
//!
//! The split is forced by borrowing, not chosen: `glifo` borrows the
//! preparation caches for as long as the run's renderer lives and borrows the
//! sink mutably *inside* that lifetime, so the two cannot be fields of one
//! struct the backend method owns. `vello_hybrid` and `vello_cpu` divide
//! themselves the same way.
//!
//! # What a glyph becomes
//!
//! With the atlas cacher disabled — the only mode this backend offers — an
//! outline glyph reaches the sink as a `save_state` / `set_transform` /
//! `set_paint_transform` / `fill_path` / `restore_state` sequence, and the sink
//! answers it by generating that path's coverage through the compiler's own
//! [`StripGenerator`] under the run's active clip and recording one
//! [`EngineDraw`]. Every glyph in a run is one draw at its own painter-order
//! depth, exactly as one small filled path per glyph would be, so nothing
//! downstream — scheduling, clipping, depth testing — has to learn that text
//! exists.
//!
//! # The run's paint, encoded once
//!
//! A run carries one brush, and `glifo` hands the sink a per-glyph *relative*
//! paint transform whose product with the glyph's own draw transform is, by
//! construction, the run's scene paint transform again — the same value for
//! every glyph in the run. The engine therefore encodes the brush once, before
//! the first glyph, and paints every glyph of the run with that one
//! [`Paint`]. A gradient-brushed run costs one encoded-paint entry and one
//! colour ramp rather than one per glyph, and the round trip through a
//! per-glyph inverse is skipped rather than paid and then undone.
//!
//! # A colour glyph's own paint
//!
//! A COLR glyph is the one case where the run's paint is *not* what a draw
//! carries: its layers bring their own palette colours and gradients, and the
//! run's brush reaches it only as the context colour a layer may ask for.
//! Those layers do not arrive as glyphs either — they arrive as a bracketed
//! stream of clips and rectangle fills, which [`color`](crate::text::color)
//! recombines into one shape and one brush per layer. This module encodes each
//! of those brushes into the frame's own paint table, exactly as the compiler
//! encodes a shape's, and records one draw per layer.
//!
//! # What is not drawn
//!
//! A bitmap-strike glyph reaches the sink as an image paint the engine has no
//! encoding for on this path. It is not drawn: the sink records nothing for it
//! and counts it, which keeps a bitmap-emoji run to missing glyphs rather than
//! wrongly-coloured ones. `glifo`'s `png` feature is deliberately left off —
//! the workspace decodes PNG in one place already — so a PNG strike is refused
//! by `glifo` before it ever reaches here, and the other two strike encodings
//! it never decodes at all.
//!
//! Stroked glyphs are refused on the same terms: the display list has no
//! stroked-glyph command, so the sink carries no stroke state to widen an
//! outline with and will not invent one.

use core::ops::RangeInclusive;

use glifo::{
    AtlasCacher, AtlasPaint, AtlasSlot, DrawSink, Glyph, GlyphPrepCache, GlyphRenderer, GlyphRun,
    GlyphRunBackend,
};
use kurbo::{Affine, BezPath, Rect, Shape};
use peniko::BlendMode;
use peniko::color::palette::css::BLACK;
use peniko::color::{AlphaColor, Srgb};
use peniko::{Brush, Fill};
use vello_common::clip::PathDataRef;
use vello_common::paint::{Image, ImageSource, Paint, PaintType};
use vello_common::strip_generator::{StripGenerator, StripStorage};

use crate::compile::clip::ClipStack;
use crate::compile::paint::encode_brush;
use crate::compile::{CompiledFrame, DepthCounter, EngineDraw, FLATTEN_TOLERANCE};
use crate::text::color::{ColorGlyph, ColorLayer, LayerShape};

/// What one glyph run cost the frame.
///
/// Purely observational, in the same spirit as [`CompiledFrame`]'s own
/// counters: nothing downstream branches on either number, and they exist so
/// "a glyph the engine cannot paint is a missing glyph, not a wrong one" is
/// measurable from outside the sink rather than asserted about it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GlyphRunOutcome {
    /// Glyphs that became a recorded draw.
    ///
    /// One per *glyph*, not per draw: a colour glyph costs one draw per layer
    /// and still counts once here, which is what keeps this bounded by the
    /// run's own glyph count.
    pub(crate) drawn: u32,
    /// Glyphs the sink refused to draw — a bitmap glyph, a stroked one, or a
    /// colour glyph whose layers have no exact engine spelling.
    pub(crate) skipped: u32,
}

/// The paint a drawing command arriving at the sink is painted with.
#[derive(Debug, Clone, PartialEq)]
enum SinkPaint {
    /// The run's own brush, as encoded once by the compiler.
    Run,
    /// A COLR layer's own palette colour or gradient, which the layer it
    /// paints is encoded from when the glyph closes.
    Layer(Brush),
    /// A paint set by `glifo` that this backend has no encoding for — a
    /// bitmap glyph's image.
    Unsupported,
}

/// The sink state `glifo` saves and restores around each glyph.
///
/// `glifo` brackets every glyph it draws in a `save_state`/`restore_state`
/// pair, so the transform, paint transform and paint it sets inside the
/// bracket must not leak into the next glyph. Restoring is a whole-value
/// assignment rather than a field-by-field undo, which is what makes that
/// impossible to get half-right.
#[derive(Debug, Clone)]
pub(crate) struct GlyphSinkState {
    /// Maps the drawing command's geometry into device space.
    transform: Affine,
    /// The paint transform relative to [`Self::transform`], as `glifo`
    /// computes it per glyph.
    paint_transform: Affine,
    /// Which paint the next drawing command is painted with.
    paint: SinkPaint,
}

/// The [`DrawSink`]/[`GlyphRenderer`] `glifo` draws a run's glyphs into.
///
/// Holds the pieces of a compile in progress a glyph draw needs, borrowed
/// disjointly from the compiler and the frame it is building: the retained
/// [`StripGenerator`], the active [`ClipStack`], the [`CompiledFrame`] being
/// filled, and the frame's [`DepthCounter`]. Everything else is per-run state
/// set up once by [`crate::text::lower_glyph_run`].
#[derive(Debug)]
pub(crate) struct EngineGlyphSink<'a> {
    /// The compiler's retained strip generator.
    generator: &'a mut StripGenerator,
    /// The clip stack the run is drawn under.
    clips: &'a mut ClipStack,
    /// The frame being compiled.
    frame: &'a mut CompiledFrame,
    /// The frame's painter-order depth counter.
    depth: &'a mut DepthCounter,
    /// The run's brush, encoded once (see this module's doc).
    run_paint: Paint,
    /// The run's brush in `glifo`'s own paint vocabulary, which it reads to
    /// resolve a COLR glyph's context colour and to decide whether a glyph is
    /// cacheable.
    context_paint: PaintType,
    /// The current, save/restore-scoped drawing state.
    state: GlyphSinkState,
    /// The colour glyph currently being replayed into layers, if any.
    color: ColorGlyph,
    /// What the run cost so far.
    outcome: GlyphRunOutcome,
}

impl<'a> EngineGlyphSink<'a> {
    /// A sink drawing into `frame` with `run_paint`, under `clips`.
    ///
    /// `context_paint` is the same brush `run_paint` was encoded from, in the
    /// vocabulary `glifo` reads it back through.
    pub(crate) fn new(
        generator: &'a mut StripGenerator,
        clips: &'a mut ClipStack,
        frame: &'a mut CompiledFrame,
        depth: &'a mut DepthCounter,
        run_paint: Paint,
        context_paint: PaintType,
    ) -> Self {
        Self {
            generator,
            clips,
            frame,
            depth,
            run_paint,
            context_paint,
            // A decoration's rectangles arrive in scene space with no
            // `set_transform` ahead of them, and the run transform is already
            // baked into every transform `glifo` derives, so the identity is
            // the right starting state rather than merely a neutral one.
            state: GlyphSinkState {
                transform: Affine::IDENTITY,
                paint_transform: Affine::IDENTITY,
                paint: SinkPaint::Run,
            },
            color: ColorGlyph::default(),
            outcome: GlyphRunOutcome::default(),
        }
    }

    /// What this sink drew and refused.
    ///
    /// Takes `&mut self` because it also closes the one state a run can end
    /// in that is not a finished glyph: a colour glyph whose brackets `glifo`
    /// never balanced (a cyclic paint graph in a malformed face is the way
    /// that happens). Its layers are dropped rather than drawn under a clip
    /// nothing closed, and it is counted as the missing glyph it is.
    pub(crate) fn outcome(&mut self) -> GlyphRunOutcome {
        if self.color.is_open() {
            self.color.reset();
            self.outcome.skipped = self.outcome.skipped.saturating_add(1);
        }

        self.outcome
    }

    /// Run `generate` under the active clip and record whatever strips
    /// survived as one draw painted with the run's paint, counting the glyph
    /// it drew.
    ///
    /// The whole of an outline glyph's path through this sink. A colour
    /// glyph's layers go through [`record_draw`](Self::record_draw) instead:
    /// they carry their own paints and are counted once for the glyph rather
    /// than once per draw.
    fn record<F>(&mut self, generate: F)
    where
        F: FnOnce(&mut StripGenerator, &mut StripStorage, Option<PathDataRef<'_>>),
    {
        if self.state.paint != SinkPaint::Run {
            self.outcome.skipped = self.outcome.skipped.saturating_add(1);
            return;
        }

        let paint = self.run_paint.clone();
        if self.record_draw(paint, generate) {
            self.outcome.drawn = self.outcome.drawn.saturating_add(1);
        }
    }

    /// Run `generate` under the active clip and record whatever strips
    /// survived as one draw painted with `paint`, answering whether a draw was
    /// recorded at all.
    ///
    /// The same shape [`crate::compile::SceneCompiler`]'s own draw recording
    /// takes, minus the paint encoding: the paint is resolved by the caller,
    /// so there is nothing to roll back and a culled glyph simply records
    /// nothing and consumes no depth.
    fn record_draw<F>(&mut self, paint: Paint, generate: F) -> bool
    where
        F: FnOnce(&mut StripGenerator, &mut StripStorage, Option<PathDataRef<'_>>),
    {
        if self.clips.blocks_everything() {
            return false;
        }

        let start = self.frame.strips.strips.len();
        let alpha_start = self.frame.strips.alphas.len();
        generate(self.generator, &mut self.frame.strips, self.clips.mask());
        self.clips
            .clip_run(&mut self.frame.strips, start, alpha_start);
        let strip_range = start..self.frame.strips.strips.len();

        if strip_range.is_empty() {
            return false;
        }

        let draw = EngineDraw::new(paint, self.depth.advance(), strip_range.clone());
        self.frame
            .recorder
            .push_draw(draw, &self.frame.strips.strips[strip_range]);
        true
    }

    /// Draw the colour glyph whose last bracket just closed, or count it
    /// skipped when it turned out to be inexpressible.
    ///
    /// Every layer is drawn under the glyph's own transform with its own
    /// brush, encoded into the frame's paint table the same way a shape's is.
    /// A glyph whose layers all culled away counts as neither drawn nor
    /// skipped, exactly as an outline glyph with no ink does.
    fn flush_color_glyph(&mut self) {
        let transform = self.color.transform();
        let Some(layers) = self.color.take() else {
            self.outcome.skipped = self.outcome.skipped.saturating_add(1);
            return;
        };

        let mut drew = false;
        for layer in layers {
            drew |= self.draw_color_layer(&layer, transform);
        }
        if drew {
            self.outcome.drawn = self.outcome.drawn.saturating_add(1);
        }
    }

    /// Draw one colour layer, answering whether it recorded a draw.
    fn draw_color_layer(&mut self, layer: &ColorLayer, transform: Affine) -> bool {
        let encoding = encode_brush(
            &layer.brush,
            transform * layer.paint_transform,
            &mut self.frame.encoded_paints,
        );
        self.frame.lut_requests.extend(encoding.lut_request);

        // Pushed only for a layer whose outline the rectangle would actually
        // cut, which a well-formed face never has (see [`crate::text::color`]).
        let clipped = layer.clip.is_some();
        if let Some(clip) = layer.clip {
            self.clips.push_rect(clip, transform, self.generator);
        }

        let drawn = match &layer.shape {
            LayerShape::Rect(rect) => {
                let rect = *rect;
                self.record_draw(encoding.paint, |generator, storage, clip| {
                    generator.generate_filled_path(
                        rect.path_elements(FLATTEN_TOLERANCE),
                        Fill::NonZero,
                        transform,
                        None,
                        storage,
                        clip,
                    );
                })
            }
            LayerShape::Path(path) => {
                self.record_draw(encoding.paint, |generator, storage, clip| {
                    generator.generate_filled_path(
                        path.iter(),
                        Fill::NonZero,
                        transform,
                        None,
                        storage,
                        clip,
                    );
                })
            }
        };

        if clipped {
            self.clips.pop();
        }

        drawn
    }
}

impl DrawSink for EngineGlyphSink<'_> {
    fn set_transform(&mut self, t: Affine) {
        self.state.transform = t;
    }

    fn set_paint(&mut self, paint: AtlasPaint) {
        // Only a COLR layer sets one, and it is the layer's own paint rather
        // than anything derived from the run's brush.
        self.state.paint = SinkPaint::Layer(match paint {
            AtlasPaint::Solid(color) => Brush::Solid(color),
            AtlasPaint::Gradient(gradient) => Brush::Gradient(gradient),
        });
    }

    fn set_paint_transform(&mut self, t: Affine) {
        self.state.paint_transform = t;
    }

    fn fill_path(&mut self, path: &BezPath) {
        if self.color.is_open() {
            // A colour glyph's layers arrive as rectangle fills through a
            // clip; a path fill inside one is a shape this sink has no place
            // to put, so the glyph goes missing rather than landing wrong.
            self.color.refuse();
            return;
        }

        let transform = self.state.transform;
        self.record(|generator, storage, clip| {
            generator.generate_filled_path(
                path.iter(),
                Fill::NonZero,
                transform,
                None,
                storage,
                clip,
            );
        });
    }

    fn fill_rect(&mut self, rect: &Rect) {
        let rect = *rect;
        if self.color.is_open() {
            match &self.state.paint {
                SinkPaint::Layer(brush) => {
                    let brush = brush.clone();
                    let paint_transform = self.state.paint_transform;
                    self.color.fill(rect, brush, paint_transform);
                }
                // A colour layer always sets its own paint before filling, so
                // a fill without one is a stream this sink does not recognise.
                SinkPaint::Run | SinkPaint::Unsupported => self.color.refuse(),
            }
            return;
        }

        let transform = self.state.transform;
        self.record(|generator, storage, clip| {
            generator.generate_filled_path(
                rect.path_elements(FLATTEN_TOLERANCE),
                Fill::NonZero,
                transform,
                None,
                storage,
                clip,
            );
        });
    }

    fn push_clip_layer(&mut self, _clip: &BezPath) {
        // An *isolated* clip layer, which `glifo` asks for only around a
        // colour glyph whose paint graph blends non-destructively — the one
        // thing the engine's own layer scheduling refuses too.
        self.color.push_refused(self.state.transform);
    }

    fn push_clip_path(&mut self, clip: &BezPath) {
        self.color.push_clip(clip, self.state.transform);
    }

    fn push_blend_layer(&mut self, _blend_mode: BlendMode) {
        self.color.push_refused(self.state.transform);
    }

    fn pop_layer(&mut self) {
        if self.color.pop() {
            self.flush_color_glyph();
        }
    }

    fn pop_clip_path(&mut self) {
        if self.color.pop() {
            self.flush_color_glyph();
        }
    }

    fn width(&self) -> u16 {
        self.generator.width()
    }

    fn height(&self) -> u16 {
        self.generator.height()
    }
}

impl GlyphRenderer for EngineGlyphSink<'_> {
    type SavedState = GlyphSinkState;

    fn save_state(&mut self) -> Self::SavedState {
        self.state.clone()
    }

    fn restore_state(&mut self, state: Self::SavedState) {
        self.state = state;
    }

    fn stroke_path(&mut self, _path: &BezPath) {
        // The display list carries no stroked-glyph command, so the sink holds
        // no stroke width to widen this outline by. Counted rather than
        // guessed at: a stroke drawn at an invented width would be wrong
        // pixels, and this is a glyph that goes missing instead — or, inside a
        // colour glyph, the whole glyph it is a layer of.
        if self.color.is_open() {
            self.color.refuse();
            return;
        }

        self.outcome.skipped = self.outcome.skipped.saturating_add(1);
    }

    fn set_paint_image(&mut self, _image: Image) {
        // A bitmap-strike glyph, or a glyph replayed out of the glyph atlas.
        // Neither is served on this path.
        self.state.paint = SinkPaint::Unsupported;
    }

    fn set_tint(&mut self, _tint: Option<vello_common::paint::Tint>) {
        // Tinting only ever applies to an atlas-sampled glyph image, which
        // this backend never draws.
    }

    fn get_context_color(&self) -> AlphaColor<Srgb> {
        match &self.context_paint {
            PaintType::Solid(color) => *color,
            _ => BLACK,
        }
    }

    fn current_paint(&self) -> &PaintType {
        &self.context_paint
    }

    fn atlas_image_source(&self, atlas_slot: &AtlasSlot) -> ImageSource {
        // Unreached while the atlas cacher is disabled, and answered honestly
        // rather than stubbed so the seam is already correct for the frust
        // atlas policy that turns caching on.
        ImageSource::opaque_id(atlas_slot.image_id)
    }

    fn atlas_paint_transform(&self, atlas_slot: &AtlasSlot) -> Affine {
        Affine::translate((-f64::from(atlas_slot.x), -f64::from(atlas_slot.y)))
    }
}

/// The [`GlyphRunBackend`] a run's [`GlyphRunBuilder`](glifo::GlyphRunBuilder)
/// is built on.
///
/// Consumed by whichever of its three drawing methods the builder ends in, so
/// it holds the two borrows a run needs and nothing else: the sink the glyphs
/// are drawn into, and the preparation caches `glifo` resolves outlines and
/// hinting instances against.
#[derive(Debug)]
pub(crate) struct EngineTextBackend<'a, 's> {
    /// Where the run's glyphs are drawn.
    sink: &'a mut EngineGlyphSink<'s>,
    /// The compiler's retained outline/hinting caches.
    prep: &'a mut GlyphPrepCache,
}

impl<'a, 's> EngineTextBackend<'a, 's> {
    /// A backend drawing into `sink`, preparing glyphs against `prep`.
    pub(crate) fn new(sink: &'a mut EngineGlyphSink<'s>, prep: &'a mut GlyphPrepCache) -> Self {
        Self { sink, prep }
    }
}

impl<'a> GlyphRunBackend<'a> for EngineTextBackend<'a, '_> {
    /// Atlas-backed glyph caching, which this backend does not offer.
    ///
    /// The engine owns no `GlyphAtlas` on this path: every glyph is drawn as
    /// strips, so there is nothing for a cache to hold and
    /// [`AtlasCacher::Disabled`] is what reaches `glifo` whichever way this is
    /// called. Accepting the request and ignoring it — rather than refusing it
    /// — is what keeps the trait's contract intact for a caller that asks; the
    /// engine's own lowering never does.
    fn atlas_cache(self, _enabled: bool) -> Self {
        self
    }

    fn fill_glyphs<Glyphs>(self, run: GlyphRun<'a>, glyphs: Glyphs)
    where
        Glyphs: Iterator<Item = Glyph> + Clone,
    {
        let Self { sink, prep } = self;
        let mut renderer = run.build(glyphs, prep.as_mut(), AtlasCacher::Disabled);
        renderer.fill_glyphs(sink);
    }

    fn stroke_glyphs<Glyphs>(self, run: GlyphRun<'a>, glyphs: Glyphs)
    where
        Glyphs: Iterator<Item = Glyph> + Clone,
    {
        let Self { sink, prep } = self;
        let mut renderer = run.build(glyphs, prep.as_mut(), AtlasCacher::Disabled);
        // No stroke width is scaled by `stroke_adjustment` on the way in, the
        // way a renderer holding stroke state would: the sink has none, and
        // refuses the stroked outline when it arrives (see
        // [`EngineGlyphSink::stroke_path`]).
        renderer.stroke_glyphs(sink);
    }

    fn render_decoration<Glyphs>(
        self,
        run: GlyphRun<'a>,
        glyphs: Glyphs,
        x_range: RangeInclusive<f32>,
        baseline_y: f32,
        offset: f32,
        size: f32,
        buffer: f32,
    ) where
        Glyphs: Iterator<Item = Glyph> + Clone,
    {
        let Self { sink, prep } = self;
        let mut renderer = run.build(glyphs, prep.as_mut(), AtlasCacher::Disabled);
        renderer.render_decoration(x_range, baseline_y, offset, size, buffer, sink);
    }
}

/// A display-list brush in the paint vocabulary `glifo` reads back through.
///
/// `glifo` asks the sink what it is painting with for two reasons — a COLR
/// glyph's context colour, and whether a glyph may be atlas-cached — and both
/// answers come from the run's own brush rather than from anything the sink
/// derives. An image brush has no counterpart in that vocabulary (its image
/// type is the renderer's, not `peniko`'s), and answers black, the same
/// fallback the CPU reference renderer gives for every non-solid paint.
pub(crate) fn context_paint(brush: &Brush) -> PaintType {
    match brush {
        Brush::Solid(color) => PaintType::Solid(*color),
        Brush::Gradient(gradient) => PaintType::Gradient(gradient.clone()),
        Brush::Image(_) => PaintType::Solid(BLACK),
    }
}
