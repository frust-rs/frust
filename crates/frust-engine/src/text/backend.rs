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
//! With the atlas cacher disabled, an outline glyph reaches the sink as a
//! `save_state` / `set_transform` / `set_paint_transform` / `fill_path` /
//! `restore_state` sequence, and the sink answers it by generating that path's
//! coverage through the compiler's own [`StripGenerator`] under the run's
//! active clip and recording one [`EngineDraw`]. Every glyph in a run is one
//! draw at its own painter-order depth, exactly as one small filled path per
//! glyph would be, so nothing downstream — scheduling, clipping, depth testing
//! — has to learn that text exists.
//!
//! # A cached glyph is an image draw
//!
//! With the cacher *enabled* — which is what [`crate::text::atlas_policy`]
//! routes a settled run to — the same glyph arrives as a `set_tint` /
//! `set_transform` / `set_paint_image` / `set_paint_transform` / `fill_rect`
//! sequence instead: `glifo` has resolved (or just allocated) an atlas slot for
//! it, and asks the sink to draw that slot's rectangle. The sink answers it the
//! way the compiler answers a `Command::Image` — one [`EncodedPaint::Image`]
//! naming the slot's [`ImageId`](vello_common::paint::ImageId), the same
//! rectangle coverage every other draw produces, and one [`EngineDraw`]. The
//! slot travels alongside in [`CompiledFrame::glyph_slots`] because only the
//! sink is ever handed it, and the renderer needs the atlas rectangle it names
//! to lower the paint.
//!
//! The **tint** is the whole of the colour-glyph split, and it is `glifo`'s to
//! decide rather than this sink's: an outline glyph is a coverage mask, so it
//! arrives with [`TintMode::AlphaMask`](vello_common::paint::TintMode) and the
//! run's own colour, and the shader fills the covered texels with that colour;
//! a COLR or bitmap glyph carries its own pixels and arrives with *no* tint,
//! which the paint records as the identity multiply, so nothing multiplies a
//! colour emoji by the text colour. Passing the tint through unexamined is what
//! keeps those two cases one code path here.
//!
//! Only `fill_rect` draws from the atlas. `glifo` never issues an atlas paint
//! under `fill_path`, and a sink that honoured one there would be inventing a
//! mapping from an arbitrary path into a slot rectangle.
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
//! An *uncached* bitmap-strike glyph reaches the sink as a pixmap image paint,
//! which is pixels travelling with the draw rather than a handle into the
//! atlas, and the engine has no encoding for one: the sink records nothing for
//! it and counts it, which keeps a bitmap-emoji run to missing glyphs rather
//! than wrongly-coloured ones. `glifo`'s `png` feature is deliberately left off
//! — the workspace decodes PNG in one place already — so a PNG strike is
//! refused by `glifo` before it ever reaches here, and the other two strike
//! encodings it never decodes at all.
//!
//! Stroked glyphs are refused on the same terms: the display list has no
//! stroked-glyph command, so the sink carries no stroke state to widen an
//! outline with and will not invent one.

use core::cell::Cell;
use core::ops::RangeInclusive;

use glifo::{
    AtlasCacher, AtlasPaint, AtlasSlot, DrawSink, GLYPH_PADDING, Glyph, GlyphPrepCache,
    GlyphRenderer, GlyphRun, GlyphRunBackend,
};
use kurbo::{Affine, BezPath, Rect, Shape};
use peniko::BlendMode;
use peniko::color::palette::css::BLACK;
use peniko::color::{AlphaColor, Srgb};
use peniko::{Brush, Fill};
use vello_common::clip::PathDataRef;
use vello_common::encode::{EncodedImage, EncodedPaint};
use vello_common::paint::{Image, ImageSource, IndexedPaint, Paint, PaintType, Tint};
use vello_common::strip_generator::{StripGenerator, StripStorage};

use crate::cache::images::AtlasRegion;
use crate::compile::clip::ClipStack;
use crate::compile::cull::generate_fill;
use crate::compile::paint::encode_brush;
use crate::compile::{CompiledFrame, DepthCounter, EngineDraw, FLATTEN_TOLERANCE, GlyphSlot};
use crate::gpu::atlas::x_y_advances;
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
#[derive(Debug, Clone)]
enum SinkPaint {
    /// The run's own brush, as encoded once by the compiler.
    Run,
    /// A COLR layer's own palette colour or gradient, which the layer it
    /// paints is encoded from when the glyph closes.
    Layer(Brush),
    /// A glyph resolved out of the atlas: the image `glifo` built from the
    /// slot, whose source is the handle [`EngineGlyphSink::atlas_image_source`]
    /// answered with.
    Atlas(Image),
    /// A paint set by `glifo` that this backend has no encoding for — an
    /// uncached bitmap glyph's pixmap.
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
    /// The tint an atlas-sampled glyph is drawn through, as `glifo` set it.
    ///
    /// Part of the saved state rather than a bare field because `glifo` sets it
    /// *inside* the save/restore bracket it draws a cached glyph in, so a
    /// restore has to take it back with everything else — the same rule the
    /// transform and the paint follow.
    tint: Option<Tint>,
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
    /// The slot the atlas paint about to be set names.
    ///
    /// `glifo` hands the slot to [`Self::atlas_image_source`] and then throws
    /// it away, keeping only the [`ImageSource`](vello_common::paint::ImageSource)
    /// that answer returned — but the engine's paint record needs the slot's
    /// *rectangle* as well, and nothing downstream carries it. So the one call
    /// that is given it stashes it here for the `fill_rect` two commands later.
    /// A [`Cell`] because that call takes `&self`: the trait asks a question,
    /// and the answer is what the drawing command that follows is built from.
    atlas_slot: Cell<Option<AtlasSlot>>,
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
                tint: None,
            },
            atlas_slot: Cell::new(None),
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
        if !matches!(self.state.paint, SinkPaint::Run) {
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

    /// Draw one glyph out of the atlas: `area`, the slot's own rectangle,
    /// painted by an image paint naming the slot.
    ///
    /// The two transforms are the ones every image draw in this crate is built
    /// from, and they are *not* the same value. The coverage is `area` under
    /// the sink's current transform — where the glyph lands on the surface.
    /// The paint is placed by that transform composed with `glifo`'s relative
    /// paint transform, which is the absolute mapping this backend is
    /// documented to reconstruct that way; the encoded entry stores its
    /// inverse, because device-to-image is the direction the shader applies.
    ///
    /// The slot is reported into [`CompiledFrame::glyph_slots`] on every draw
    /// rather than once per allocation. That is what makes a recycled handle
    /// safe: `glifo` frees a slot's [`ImageId`](vello_common::paint::ImageId)
    /// back to the shared allocator when it evicts, and the next occupant of
    /// that id — a glyph or an image — may hold a different rectangle, so the
    /// rectangle a draw *names* is the one that has to travel with it.
    fn draw_atlas_glyph(&mut self, image: &Image, area: Rect) {
        let Some(slot) = self.atlas_slot.get() else {
            // `glifo` sets an atlas image paint only after asking this sink for
            // the slot's source, so this is unreachable on its own path; a
            // stream that reached it would be one this backend cannot place.
            self.outcome.skipped = self.outcome.skipped.saturating_add(1);
            return;
        };
        if slot.width == 0 || slot.height == 0 {
            // A glyph with no ink — a space, whose outline bounds are empty and
            // whose slot is therefore a degenerate rectangle. Counted as
            // neither drawn nor skipped, exactly as the outline path counts one
            // whose fill produced no strips: it is not a glyph the engine
            // failed to paint, it is a glyph with nothing to paint.
            return;
        }

        let transform = self.state.transform;
        let paint_transform = transform * self.state.paint_transform;
        let inverse = paint_transform.inverse();
        if !inverse.as_coeffs().iter().all(|coeff| coeff.is_finite()) {
            self.outcome.skipped = self.outcome.skipped.saturating_add(1);
            return;
        }

        let (x_advance, y_advance) = x_y_advances(inverse);
        let paint_index = self.frame.encoded_paints.len();
        self.frame
            .encoded_paints
            .push(EncodedPaint::Image(EncodedImage {
                // A glyph slot is an anti-aliased coverage mask (an outline) or a
                // colour glyph with its own alpha; neither is opaque, and claiming
                // otherwise would route it into the depth-writing opaque pass.
                may_have_transparency: true,
                source: image.image.clone(),
                sampler: image.sampler,
                transform: inverse,
                x_advance,
                y_advance,
                tint: self.state.tint,
            }));

        let paint = Paint::Indexed(IndexedPaint::new(paint_index));
        let drawn = self.record_draw(paint, |generator, storage, clip| {
            generator.generate_filled_path(
                area.path_elements(FLATTEN_TOLERANCE),
                Fill::NonZero,
                transform,
                None,
                storage,
                clip,
            );
        });

        if drawn {
            self.frame.glyph_slots.push(GlyphSlot {
                id: slot.image_id,
                region: AtlasRegion {
                    layer: slot.page_index,
                    offset: [u32::from(slot.x), u32::from(slot.y)],
                    size: [u32::from(slot.width), u32::from(slot.height)],
                },
                padding: u32::from(GLYPH_PADDING),
            });
            self.frame.atlas_glyph_draws = self.frame.atlas_glyph_draws.saturating_add(1);
            self.outcome.drawn = self.outcome.drawn.saturating_add(1);
        } else {
            // The same roll-back the compiler's own `record` keeps: a glyph
            // clipped away leaves no orphan entry in the frame's paint table.
            // The entry just pushed is the last one, so this is exact.
            self.frame.encoded_paints.truncate(paint_index);
        }
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
                    generate_fill(generator, path.iter(), transform, storage, clip);
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
            generate_fill(generator, path.iter(), transform, storage, clip);
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
                SinkPaint::Run | SinkPaint::Atlas(_) | SinkPaint::Unsupported => {
                    self.color.refuse();
                }
            }
            return;
        }

        // A cached glyph is a rectangle of the atlas, and this is the only
        // command `glifo` draws one with.
        if let SinkPaint::Atlas(image) = &self.state.paint {
            let image = image.clone();
            self.draw_atlas_glyph(&image, rect);
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

    fn set_paint_image(&mut self, image: Image) {
        // The source is what tells the two apart, and it is this sink's own
        // answer coming back: a glyph resolved out of the atlas carries the
        // handle `atlas_image_source` returned, while an uncached bitmap strike
        // carries its pixmap inline — pixels the engine has nowhere to put on
        // this path.
        self.state.paint = match &image.image {
            ImageSource::OpaqueId { .. } => SinkPaint::Atlas(image),
            ImageSource::Pixmap(_) => SinkPaint::Unsupported,
        };
    }

    fn set_tint(&mut self, tint: Option<vello_common::paint::Tint>) {
        // Carried, not interpreted: `glifo` sets the alpha-mask tint that
        // colours an outline glyph and leaves it unset for a COLR or bitmap
        // one, which is exactly the split the encoded paint has to record (see
        // this module's doc).
        self.state.tint = tint;
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
        // The one call handed the slot, so it is where the slot is kept for
        // the draw that follows (see [`EngineGlyphSink::atlas_slot`]).
        self.atlas_slot.set(Some(*atlas_slot));
        ImageSource::opaque_id(atlas_slot.image_id)
    }

    fn atlas_paint_transform(&self, _atlas_slot: &AtlasSlot) -> Affine {
        // The identity, because this engine addresses an atlas paint by its
        // own *region* — the layer, offset and extent travel in the lowered
        // record — rather than by sampling the whole page and shifting into
        // the slot. A renderer that bound the page would need the slot's
        // negative offset here; one whose record already carries the offset
        // would be applying it twice.
        Affine::IDENTITY
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
    /// The glyph atlas this run's glyphs may be cached in, or
    /// [`AtlasCacher::Disabled`] for a run the policy routed to outlines.
    cacher: AtlasCacher<'a>,
}

impl<'a, 's> EngineTextBackend<'a, 's> {
    /// A backend drawing into `sink`, preparing glyphs against `prep`, caching
    /// through `cacher`.
    ///
    /// `cacher` is the policy's answer for this run already made: the caller
    /// classified the run before building this, so nothing here re-decides it.
    pub(crate) fn new(
        sink: &'a mut EngineGlyphSink<'s>,
        prep: &'a mut GlyphPrepCache,
        cacher: AtlasCacher<'a>,
    ) -> Self {
        Self { sink, prep, cacher }
    }
}

impl<'a> GlyphRunBackend<'a> for EngineTextBackend<'a, '_> {
    /// Turn atlas-backed glyph caching off for this run.
    ///
    /// One-way on purpose. The cacher this backend was built with is already
    /// the policy's answer for the run — the *only* place that decides whether
    /// a run may be cached (see [`crate::text::atlas_policy`]) — so a caller
    /// asking for caching cannot conjure a cache the policy withheld, while a
    /// caller asking to go without is always honoured. The engine's own
    /// lowering asks for neither.
    fn atlas_cache(mut self, enabled: bool) -> Self {
        if !enabled {
            self.cacher = AtlasCacher::Disabled;
        }
        self
    }

    fn fill_glyphs<Glyphs>(self, run: GlyphRun<'a>, glyphs: Glyphs)
    where
        Glyphs: Iterator<Item = Glyph> + Clone,
    {
        let Self { sink, prep, cacher } = self;
        let mut renderer = run.build(glyphs, prep.as_mut(), cacher);
        renderer.fill_glyphs(sink);
    }

    fn stroke_glyphs<Glyphs>(self, run: GlyphRun<'a>, glyphs: Glyphs)
    where
        Glyphs: Iterator<Item = Glyph> + Clone,
    {
        let Self { sink, prep, cacher } = self;
        let mut renderer = run.build(glyphs, prep.as_mut(), cacher);
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
        let Self { sink, prep, cacher } = self;
        let mut renderer = run.build(glyphs, prep.as_mut(), cacher);
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
