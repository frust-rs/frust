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
//! steady-state frame reuses those allocations, and the one piece of state that
//! genuinely spans frames — the [`ImageResidency`] that keeps an image's atlas
//! rectangle alive for as long as the scene keeps drawing it. Everything else
//! is per-frame: [`compile`](SceneCompiler::compile) returns what a frame
//! produced in one [`CompiledFrame`] and keeps nothing of it.
//!
//! Compiled here: the geometry primitives — axis-aligned and rounded
//! rectangles, lines, arbitrary filled/stroked paths — the clip bracket around
//! them, which lowers to a scissor rectangle or a coverage mask and never to an
//! intermediate texture (see [`clip`]), the opacity-layer and snapshot brackets
//! that group them (see [`layers`]), the hole punch that erases what they
//! painted (see [`clear`]), images, whose destination rectangle is
//! rasterized like any other fill and painted by an atlas-backed image paint
//! (see [`paint`] and [`crate::cache::images`]), and blurred rounded
//! rectangles, whose padded bounding rectangle is rasterized the same way and
//! painted by a gaussian-falloff paint the fragment shader evaluates per pixel
//! (see [`blur_rrect`]). Glyphs and shader quads are recognised and skipped —
//! the engine grows them in later passes, and skipping is the conservative
//! behaviour (a frame draws less, never wrong).

pub mod blur_rrect;

pub mod clear;

pub mod clip;

pub mod layers;

pub mod paint;

pub mod draw;

pub use clear::ClearPunch;
pub use clip::ClipStack;
pub use draw::{DepthCounter, EngineDraw};
pub use layers::{GroupStack, LayerLowering, SnapshotStack};

use std::sync::Once;

use kurbo::{
    Affine, BezPath, Cap, Join, Line, PathEl, Rect, RoundedRect, RoundedRectRadii, Shape, Stroke,
};
use peniko::{Brush, Color, Fill, ImageData};

use frust_gpu::TierCaps;
use frust_scene::{Command, CornerRadii, DashPattern, PathStyle, Scene};

use vello_common::clip::PathDataRef;
use vello_common::encode::EncodedPaint;
use vello_common::fearless_simd::Level;
use vello_common::record::CommandRecorder;
use vello_common::strip_generator::{GenerationMode, StripGenerator, StripStorage};
use vello_common::tile::Tile;
use vello_common::util::is_axis_aligned;

use crate::cache::images::{AtlasBudget, AtlasRegion, ImageResidency, ImageSkip, ImageUpload};
use crate::compile::blur_rrect::{encode_blurred_rounded_rect, inflated_bounds};
use crate::compile::clear::StagedPunch;
use crate::compile::paint::{LutRequest, encode_brush, encode_image_brush, encode_image_command};
use crate::error::EngineError;

/// Curve-flattening tolerance, in device pixels.
///
/// The value `vello_hybrid`'s own scene recorder flattens at; keeping it
/// identical is what lets the two rasterizers be compared strip-for-strip.
const FLATTEN_TOLERANCE: f64 = 0.1;

/// Raised the first time an image is refused residency, so a scene that draws
/// an unsupported image says so at least once at warning level without the
/// per-frame repetition a per-skip warning would produce.
static IMAGE_SKIP_WARNING: Once = Once::new();

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
    /// The frame's hole punches, hoisted to the root and issued as one
    /// destination-out pass after every draw (see [`clear`]).
    ///
    /// Deliberately not draws: a punch erases rather than paints, and keeping
    /// it out of the recording is what lets a target that disregards alpha
    /// drop the whole pass and read the frame unchanged.
    pub clears: Vec<ClearPunch>,
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
    /// How many of this frame's clips lowered to a scissor rectangle, costing
    /// no rasterization at all (see [`clip`]).
    pub scissor_clips: u32,
    /// How many of this frame's clips lowered to a coverage mask.
    pub mask_clips: u32,
    /// Atlas regions whose texels must be cleared before this frame draws,
    /// freed by the residency reap at the head of the frame.
    ///
    /// Serviced **before** [`image_uploads`](Self::image_uploads): a rectangle
    /// freed this frame can be re-allocated in the same frame, so clearing
    /// after writing would erase the image that just moved in.
    pub image_evictions: Vec<AtlasRegion>,
    /// Atlas regions whose texels must be written before this frame draws, one
    /// per image that became resident during it.
    ///
    /// Empty in the steady state: an image drawn on a thousand consecutive
    /// frames appears here exactly once, on the first.
    pub image_uploads: Vec<ImageUpload>,
    /// The atlas array depth this frame's paints address — the layer count the
    /// array texture must have grown to before the uploads are written.
    pub atlas_layers: u32,
    /// How many of this frame's draws painted with an atlas-backed image.
    pub image_draws: u32,
    /// How many image draws were dropped because the image could not be made
    /// resident (unsupported format, oversized, malformed, atlas full, or the
    /// atlas disabled outright).
    ///
    /// Observational, and the counter that makes "an image the engine cannot
    /// hold is a skipped draw, not a panicked frame" measurable rather than
    /// asserted.
    pub skipped_images: u32,
    /// How many strips this frame's coverage masks cost.
    ///
    /// Observational, and the counter the clip lowering's whole claim rests on:
    /// a frame whose clips all scissored reports zero here, which is what
    /// "a rectangular clip is free" means measured rather than asserted.
    pub clip_mask_strips: usize,
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
    clips: ClipStack,
    groups: GroupStack,
    snapshots: SnapshotStack,
    punches: Vec<StagedPunch>,
    images: ImageResidency,
}

impl SceneCompiler {
    /// A compiler sized for a `width` x `height` viewport.
    ///
    /// The size is re-asserted on every [`compile`](Self::compile) call, so
    /// this is only the initial allocation hint; pass the surface's current
    /// size to avoid an immediate resize.
    ///
    /// Image residency starts on [`AtlasBudget::MOBILE`], the smaller of the
    /// two tiers. A compiler built without an adapter in hand knows nothing
    /// about the device it will end up on, and over-budgeting a phone costs
    /// real memory while under-budgeting a desktop costs only an extra atlas
    /// layer — call [`for_caps`](Self::for_caps) or
    /// [`set_atlas_budget`](Self::set_atlas_budget) once the adapter is known.
    pub fn new(width: u16, height: u16) -> Self {
        Self::with_atlas_budget(width, height, AtlasBudget::MOBILE)
    }

    /// A compiler sized for a `width` x `height` viewport, with image
    /// residency budgeted for `caps`' adapter.
    pub fn for_caps(width: u16, height: u16, caps: &TierCaps) -> Self {
        Self::with_atlas_budget(width, height, AtlasBudget::for_caps(caps))
    }

    /// A compiler sized for a `width` x `height` viewport, with image
    /// residency budgeted explicitly.
    pub fn with_atlas_budget(width: u16, height: u16, budget: AtlasBudget) -> Self {
        let level = Level::try_detect().unwrap_or(Level::baseline());
        Self {
            generator: StripGenerator::new(width, height, level),
            clips: ClipStack::new(),
            groups: GroupStack::new(),
            snapshots: SnapshotStack::new(),
            punches: Vec::new(),
            images: ImageResidency::new(budget),
        }
    }

    /// The images this compiler currently holds resident.
    pub fn images(&self) -> &ImageResidency {
        &self.images
    }

    /// Re-budget image residency, dropping every image currently resident.
    ///
    /// The atlas geometry is what an allocation's coordinates mean, so a change
    /// to it invalidates every rectangle already handed out: residency starts
    /// over and each image re-uploads on the next frame that draws it. A caller
    /// that owns the atlas texture must recreate it at the new extent in the
    /// same step — this is an adapter-change or start-up operation, never a
    /// per-frame one.
    pub fn set_atlas_budget(&mut self, budget: AtlasBudget) {
        self.set_image_residency(ImageResidency::new(budget));
    }

    /// Replace this compiler's image residency wholesale, dropping every image
    /// currently resident.
    ///
    /// The same invalidation [`set_atlas_budget`](Self::set_atlas_budget)
    /// carries, exposed for the residencies a budget alone cannot express — a
    /// deliberately [disabled](ImageResidency::disabled) one, or one a caller
    /// built against an adapter's own capabilities.
    pub fn set_image_residency(&mut self, images: ImageResidency) {
        self.images = images;
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
        self.clips.reset();
        self.groups.reset();
        self.snapshots.reset();
        self.punches.clear();
        // Ahead of the walk, so a rectangle this frame's reap frees is
        // available to this frame's own allocations and its clear is ordered
        // ahead of their uploads.
        self.images.begin_frame();

        let mut frame = CompiledFrame {
            strips: StripStorage::new(GenerationMode::Append),
            recorder: CommandRecorder::new(width, height),
            encoded_paints: Vec::new(),
            clears: Vec::new(),
            lut_requests: Vec::new(),
            fast_rect_draws: 0,
            scissor_clips: 0,
            mask_clips: 0,
            clip_mask_strips: 0,
            image_evictions: Vec::new(),
            image_uploads: Vec::new(),
            atlas_layers: 0,
            image_draws: 0,
            skipped_images: 0,
        };
        let mut depth = DepthCounter::new();

        for command in scene.commands() {
            self.compile_command(command, root, &mut frame, &mut depth);
        }

        self.close_open_groups(&mut frame);
        self.generate_punches(&mut frame);

        frame.scissor_clips = self.clips.scissor_clips();
        frame.mask_clips = self.clips.mask_clips();
        frame.clip_mask_strips = self.clips.mask_strips();
        let (evictions, uploads) = self.images.take_plan();
        frame.image_evictions = evictions;
        frame.image_uploads = uploads;
        frame.atlas_layers = self.images.layers();

        Ok(frame)
    }

    fn compile_command(
        &mut self,
        command: &Command,
        root: Affine,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
    ) {
        // The frame root with any open snapshot bracket's presentation scale
        // composed ahead of it (see [`layers`]). The identity outside a
        // bracket, so this is the plain frame root for every frame that
        // records none.
        let combined = root * self.snapshots.correction();

        // A correction composes a transform the up-front walk never saw, and
        // the product can leave the finite device grid even though both
        // factors are on it. Such a command draws nothing rather than refusing
        // the frame: the refusal is the up-front walk's to make over the
        // numbers a scene actually carries, and a bracket's presentation scale
        // is not one of them. Only a command *inside* a snapshot bracket can
        // land here at all — outside one the composition is the frame root's,
        // which that walk already checked.
        //
        // Drawing nothing is not the same as doing nothing: a command that
        // opens a bracket still has to open one, or its pop would close the
        // bracket around it instead. So a bracket lands blocked rather than
        // absent, which draws nothing inside it and balances its own pop.
        let on_grid = command_transform(command)
            .is_none_or(|transform| check_finite(combined * transform).is_ok());
        if !on_grid {
            match command {
                Command::PushClip { .. }
                | Command::PushClipRounded { .. }
                | Command::PushLayer { .. } => self.open_blocked_group(),
                // The correction is the outermost bracket's, so a bracket
                // reaching here is a nested one, whose presentation is ignored
                // anyway; only its depth has to be counted.
                Command::PushSnapshot { rect, .. } => {
                    self.snapshots.enter(*rect, 1.0, Affine::IDENTITY);
                }
                _ => {}
            }
            return;
        }

        match command {
            Command::FillRect {
                rect,
                brush,
                transform,
            } => {
                let transform = combined * *transform;

                if let Some(device_rect) = fast_rect(*rect, transform) {
                    let recorded = self.record(
                        frame,
                        depth,
                        PaintSource::Brush(brush),
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_rect_fast(&device_rect, storage, clip);
                        },
                    );
                    if recorded {
                        frame.fast_rect_draws = frame.fast_rect_draws.saturating_add(1);
                    }
                } else {
                    self.record(
                        frame,
                        depth,
                        PaintSource::Brush(brush),
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_path(
                                rect.path_elements(FLATTEN_TOLERANCE),
                                Fill::NonZero,
                                transform,
                                None,
                                storage,
                                clip,
                            );
                        },
                    );
                }
            }
            Command::RoundedRect {
                rect,
                radii,
                brush,
                transform,
            } => {
                let transform = combined * *transform;
                let shape = RoundedRect::from_rect(*rect, rounded_rect_radii(*radii));

                self.record(
                    frame,
                    depth,
                    PaintSource::Brush(brush),
                    transform,
                    |generator, storage, clip| {
                        generator.generate_filled_path(
                            shape.path_elements(FLATTEN_TOLERANCE),
                            Fill::NonZero,
                            transform,
                            None,
                            storage,
                            clip,
                        );
                    },
                );
            }
            Command::Line {
                p0,
                p1,
                width,
                brush,
                transform,
            } => {
                let transform = combined * *transform;
                let line = Line::new(*p0, *p1);
                let stroke = round_stroke(*width);

                self.record(
                    frame,
                    depth,
                    PaintSource::Brush(brush),
                    transform,
                    |generator, storage, clip| {
                        generator.generate_stroked_path(
                            line.path_elements(FLATTEN_TOLERANCE),
                            &stroke,
                            transform,
                            None,
                            storage,
                            clip,
                        );
                    },
                );
            }
            Command::Path {
                path,
                style,
                brush,
                transform,
            } => {
                let transform = combined * *transform;

                match style {
                    PathStyle::Fill => {
                        self.record(
                            frame,
                            depth,
                            PaintSource::Brush(brush),
                            transform,
                            |generator, storage, clip| {
                                generator.generate_filled_path(
                                    path.iter(),
                                    Fill::NonZero,
                                    transform,
                                    None,
                                    storage,
                                    clip,
                                );
                            },
                        );
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
                                    PaintSource::Brush(brush),
                                    transform,
                                    |generator, storage, clip| {
                                        generator.generate_stroked_path(
                                            dashed.iter(),
                                            &stroke,
                                            transform,
                                            None,
                                            storage,
                                            clip,
                                        );
                                    },
                                );
                            }
                            None => {
                                self.record(
                                    frame,
                                    depth,
                                    PaintSource::Brush(brush),
                                    transform,
                                    |generator, storage, clip| {
                                        generator.generate_stroked_path(
                                            path.iter(),
                                            &stroke,
                                            transform,
                                            None,
                                            storage,
                                            clip,
                                        );
                                    },
                                );
                            }
                        }
                    }
                }
            }
            Command::PushClip { rect, transform } => {
                let transform = combined * *transform;
                self.clips.push_rect(*rect, transform, &mut self.generator);
                self.groups.push_clip(transform.transform_rect_bbox(*rect));
            }
            Command::PushClipRounded {
                rect,
                radii,
                transform,
            } => {
                let transform = combined * *transform;
                self.clips.push_rounded(
                    *rect,
                    rounded_rect_radii(*radii),
                    transform,
                    &mut self.generator,
                );
                self.groups.push_clip(transform.transform_rect_bbox(*rect));
            }
            Command::PushLayer {
                rect,
                alpha,
                transform,
            } => {
                self.open_layer(frame, *rect, *alpha, combined * *transform);
            }
            // One bracket stack serves all three kinds, so whichever pop
            // arrives closes the innermost open bracket (see [`layers`]). A pop
            // with nothing open is ignored: an unbalanced widget tree must not
            // be able to lift a bracket a sibling still relies on.
            Command::PopClip | Command::PopLayer => self.close_group(frame),
            Command::ClearRect { rect, transform } => {
                let transform = combined * *transform;
                // Hoisted here rather than at the end of the frame because
                // this is the only point the brackets confining it are still
                // open; its coverage is generated once the frame's draws are
                // done (see [`clear`]).
                let punch = clear::punch_rect(*rect, transform, self.groups.bounds());
                if let Some(device) = punch {
                    self.punches.push(StagedPunch {
                        device,
                        depth: depth.advance(),
                    });
                }
            }
            Command::PushSnapshot {
                rect,
                alpha,
                scale,
                transform,
                ..
            } => {
                if self.snapshots.enter(*rect, *scale, *transform) {
                    // The bracket's own correction is the one that applies to
                    // the layer it opens, so the transform is recomposed here
                    // rather than reusing `combined` from before the entry.
                    let corrected = root * self.snapshots.correction() * *transform;
                    if *alpha < 1.0 && check_finite(corrected).is_ok() {
                        self.open_layer(frame, *rect, *alpha, corrected);
                        self.snapshots.record_layer();
                    }
                }
            }
            Command::PopSnapshot => {
                if self.snapshots.leave() {
                    self.close_group(frame);
                }
            }
            Command::Image {
                data,
                dest,
                transform,
            } => {
                let transform = combined * *transform;
                let source = PaintSource::Image { data, dest: *dest };

                // An image is its destination rectangle's coverage under an
                // image paint — the same two rectangle paths a solid fill
                // takes, so a pixel-aligned image costs no flattening either.
                if let Some(device_rect) = fast_rect(*dest, transform) {
                    let recorded = self.record(
                        frame,
                        depth,
                        source,
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_rect_fast(&device_rect, storage, clip);
                        },
                    );
                    if recorded {
                        frame.fast_rect_draws = frame.fast_rect_draws.saturating_add(1);
                    }
                } else {
                    self.record(
                        frame,
                        depth,
                        source,
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_path(
                                dest.path_elements(FLATTEN_TOLERANCE),
                                Fill::NonZero,
                                transform,
                                None,
                                storage,
                                clip,
                            );
                        },
                    );
                }
            }
            Command::BlurredRoundedRect {
                rect,
                radii,
                std_dev,
                color,
                transform,
            } => {
                let transform = combined * *transform;
                let source = PaintSource::BlurredRect {
                    rect: *rect,
                    radii: *radii,
                    std_dev: *std_dev,
                    color: *color,
                };
                // The strip generator rasterizes the padded bounding
                // rectangle, not `rect` itself and not a rounded shape — see
                // [`blur_rrect`]'s module doc for why. It takes the same fast
                // rectangle path a fill or an image does whenever that padded
                // rectangle lands pixel-aligned under `transform`.
                let bounds = inflated_bounds(*rect, *std_dev);

                if let Some(device_rect) = fast_rect(bounds, transform) {
                    let recorded = self.record(
                        frame,
                        depth,
                        source,
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_rect_fast(&device_rect, storage, clip);
                        },
                    );
                    if recorded {
                        frame.fast_rect_draws = frame.fast_rect_draws.saturating_add(1);
                    }
                } else {
                    self.record(
                        frame,
                        depth,
                        source,
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_path(
                                bounds.path_elements(FLATTEN_TOLERANCE),
                                Fill::NonZero,
                                transform,
                                None,
                                storage,
                                clip,
                            );
                        },
                    );
                }
            }
            // Recognised but not yet compiled. Listed one by one rather than
            // caught by a wildcard so a command added to the display list
            // fails to compile here instead of silently vanishing from every
            // frame.
            Command::GlyphRun(_) | Command::ShaderQuad { .. } => {}
        }
    }

    /// Open a layer bracket: its rectangle's clip, and — below full opacity —
    /// a recorded layer for the scheduler to give a page of its own.
    ///
    /// The clip and the bracket are pushed together and unconditionally, which
    /// is what keeps the two stacks in step for [`close_group`](Self::close_group).
    fn open_layer(&mut self, frame: &mut CompiledFrame, rect: Rect, alpha: f32, transform: Affine) {
        self.clips.push_rect(rect, transform, &mut self.generator);

        let isolated = layers::lower_layer(alpha) == LayerLowering::Isolated;
        if isolated {
            frame.recorder.push_layer(layers::layer_props(alpha), None);
        }
        self.groups
            .push_layer(transform.transform_rect_bbox(rect), isolated);
    }

    /// Open a bracket that admits nothing, for a push whose transform does not
    /// land on the device grid.
    ///
    /// A degenerate rectangle under the identity, rather than the push's own
    /// geometry under its own transform: the point is to reach an empty
    /// scissor without handing the flattener a transform it cannot subdivide
    /// against, which is the very thing that made this push unusable.
    fn open_blocked_group(&mut self) {
        self.clips
            .push_rect(Rect::ZERO, Affine::IDENTITY, &mut self.generator);
        self.groups.push_clip(Rect::ZERO);
    }

    /// Close the innermost open bracket, undoing exactly what opened it.
    ///
    /// The recording is only popped when this bracket is the one that pushed
    /// it *and* the recording agrees a layer is open — the recorder's own pop
    /// panics on an empty layer stack, and the frame path returns errors
    /// rather than panicking (E17).
    fn close_group(&mut self, frame: &mut CompiledFrame) {
        let Some(group) = self.groups.pop() else {
            return;
        };
        if group.closes_clip() {
            self.clips.pop();
        }
        if group.closes_layer() && frame.recorder.has_layers() {
            frame.recorder.pop_layer();
        }
    }

    /// Close every bracket the display list left open at the end of the frame.
    ///
    /// A recorded layer that is never popped has no bounds — the recorder
    /// computes them at the pop — so an unbalanced push would otherwise leave
    /// the scheduler a layer it cannot place. Closing here is the same policy
    /// an unbalanced pop gets, applied at the other end.
    fn close_open_groups(&mut self, frame: &mut CompiledFrame) {
        while !self.groups.is_empty() {
            self.close_group(frame);
        }
        self.snapshots.reset();
    }

    /// Generate the coverage for every punch the frame hoisted.
    ///
    /// Runs after the walk, so the strips land past every draw's own range and
    /// no draw references them. The punch is rasterized at the frame root
    /// under no clip at all — being hoisted out of its brackets is exactly
    /// what the confinement in [`clear::punch_rect`] already accounted for —
    /// and takes the fast rectangle path whenever its edges fall on whole
    /// pixels, which is what makes a pixel-aligned punch pixel-exact however
    /// its edges fall inside a tile.
    fn generate_punches(&mut self, frame: &mut CompiledFrame) {
        for index in 0..self.punches.len() {
            let Some(punch) = self.punches.get(index).copied() else {
                continue;
            };

            let start = frame.strips.strips.len();
            match fast_rect(punch.device, Affine::IDENTITY) {
                Some(device) => {
                    self.generator
                        .generate_filled_rect_fast(&device, &mut frame.strips, None);
                }
                None => {
                    self.generator.generate_filled_path(
                        punch.device.path_elements(FLATTEN_TOLERANCE),
                        Fill::NonZero,
                        Affine::IDENTITY,
                        None,
                        &mut frame.strips,
                        None,
                    );
                }
            }

            let strip_range = start..frame.strips.strips.len();
            if strip_range.is_empty() {
                continue;
            }

            frame.clears.push(ClearPunch {
                strip_range,
                bounds: clear::device_bounds(punch.device),
                depth: punch.depth,
            });
        }
    }

    /// Run `generate` under the active clip, then record whatever strips
    /// survived as one draw painted from `source` under `transform`.
    ///
    /// `generate` is handed the clip stack's coverage mask to pass on to the
    /// strip generator, which is what intersects a mask clip while the draw's
    /// own coverage is produced; the scissor is applied afterwards, to the run
    /// the generator appended. A scissor admitting nothing skips generation
    /// entirely rather than generating coverage to throw away.
    ///
    /// Returns whether a draw was recorded. A generator call that produced no
    /// strips (fully culled, clipped away, degenerate, or empty geometry)
    /// records nothing and consumes no depth, so a frame's depths stay dense
    /// over the draws that actually exist.
    ///
    /// The paint is encoded only once the strips are known to be non-empty, so
    /// a culled draw leaves no orphan entry in the frame's encoded-paint table
    /// and no ramp request for a gradient nothing paints with. An image the
    /// atlas refuses arrives *after* that point, so its coverage is rolled back
    /// to where the generator started rather than left behind as strips no draw
    /// references.
    fn record<F>(
        &mut self,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
        source: PaintSource<'_>,
        transform: Affine,
        generate: F,
    ) -> bool
    where
        F: FnOnce(&mut StripGenerator, &mut StripStorage, Option<PathDataRef<'_>>),
    {
        if self.clips.blocks_everything() {
            return false;
        }

        let start = frame.strips.strips.len();
        let alpha_start = frame.strips.alphas.len();
        generate(&mut self.generator, &mut frame.strips, self.clips.mask());
        self.clips.clip_run(&mut frame.strips, start, alpha_start);
        let strip_range = start..frame.strips.strips.len();

        if strip_range.is_empty() {
            return false;
        }

        let Some(paint) = self.encode_paint(source, transform, frame) else {
            frame.strips.strips.truncate(start);
            frame.strips.alphas.truncate(alpha_start);
            return false;
        };

        let draw = EngineDraw::new(paint, depth.advance(), strip_range.clone());
        frame
            .recorder
            .push_draw(draw, &frame.strips.strips[strip_range]);
        true
    }

    /// Encode `source` into the paint a draw carries, or `None` when the paint
    /// cannot be resolved and the draw is to be dropped.
    ///
    /// Only an image can answer `None`: a solid and a gradient are always
    /// encodable (a degenerate gradient falls back to a solid), while an image
    /// needs atlas space the residency may refuse.
    fn encode_paint(
        &mut self,
        source: PaintSource<'_>,
        transform: Affine,
        frame: &mut CompiledFrame,
    ) -> Option<vello_common::paint::Paint> {
        let encoded = match source {
            PaintSource::Brush(Brush::Image(brush)) => encode_image_brush(
                brush,
                transform,
                &mut frame.encoded_paints,
                &mut self.images,
            ),
            PaintSource::Brush(brush) => {
                let encoding = encode_brush(brush, transform, &mut frame.encoded_paints);
                frame.lut_requests.extend(encoding.lut_request);
                return Some(encoding.paint);
            }
            PaintSource::Image { data, dest } => encode_image_command(
                data,
                dest,
                transform,
                &mut frame.encoded_paints,
                &mut self.images,
            ),
            PaintSource::BlurredRect {
                rect,
                radii,
                std_dev,
                color,
            } => {
                // Unlike an image, this can never be refused (see
                // [`encode_blurred_rounded_rect`]'s doc), so it returns
                // straight away rather than joining the fallible match below.
                let paint = encode_blurred_rounded_rect(
                    rect,
                    radii,
                    std_dev,
                    color,
                    transform,
                    &mut frame.encoded_paints,
                );
                return Some(paint);
            }
        };

        match encoded {
            Ok(encoding) => {
                frame.image_draws = frame.image_draws.saturating_add(1);
                Some(encoding.paint)
            }
            Err(skip) => {
                frame.skipped_images = frame.skipped_images.saturating_add(1);
                note_image_skip(skip);
                None
            }
        }
    }
}

/// What a draw is painted with, as the walk hands it to
/// [`SceneCompiler::record`].
///
/// An image is not a [`Brush`] in the display list — [`Command::Image`] carries
/// its pixels and a destination rectangle directly — so the two arrive by
/// different routes and are distinguished here rather than by forcing one into
/// the shape of the other.
enum PaintSource<'a> {
    /// A solid, gradient or image brush recorded on a shape command.
    Brush(&'a Brush),
    /// A [`Command::Image`]'s pixels scaled to fill `dest`.
    Image {
        /// The decoded image to make resident.
        data: &'a ImageData,
        /// The destination rectangle, in the command's own coordinate space.
        dest: Rect,
    },
    /// A [`Command::BlurredRoundedRect`]'s shadow parameters, in the
    /// command's own (pre-transform) coordinate space.
    BlurredRect {
        /// The un-padded rectangle the shadow is cast from.
        rect: Rect,
        /// Per-corner radii, collapsed to their largest at encode time (see
        /// [`blur_rrect`]).
        radii: CornerRadii,
        /// The blur's standard deviation.
        std_dev: f64,
        /// The shadow's base colour.
        color: Color,
    },
}

/// Report an image the atlas refused.
///
/// The first refusal in a process is a warning, because a blank image where one
/// was expected is otherwise invisible; the rest are debug, because a scene
/// that keeps drawing a refused image would repeat the message every frame.
fn note_image_skip(skip: ImageSkip) {
    IMAGE_SKIP_WARNING.call_once(|| {
        log::warn!("image draw skipped: {skip} (further skips are logged at debug level)");
    });
    log::debug!("image draw skipped: {skip}");
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
/// weaker outcome. A clip is checked because it *is* lowered: its rectangle and
/// radii decide whether the clip scissors or masks and where its edges land, so
/// a non-finite one is refused on the same terms as a fill's, matching the
/// refusal its transform already drew. A layer and a snapshot bracket are
/// checked on those same terms, and for the same reason: each lowers its
/// rectangle through the clip stack, so a non-finite one reaches the flattener
/// exactly as a clip's would. Their `alpha` and `scale` are checked alongside
/// it because neither is decoration — an alpha decides whether the layer
/// isolates, and a scale composes a transform every command inside the bracket
/// is drawn under. A clear is checked because its rectangle *is* the coverage
/// it erases with. An image's destination rectangle is checked on those same
/// terms — it is both the coverage the image paints through and the scale its
/// natural-to-destination transform is derived from, so a non-finite one would
/// reach the flattener and the paint encoding alike. A blurred rounded
/// rectangle's rectangle, corner radii and standard deviation are checked on
/// those same terms — together they decide the padded rectangle the strip
/// generator rasterizes ([`blur_rrect::inflated_bounds`]) and the falloff the
/// fragment shader evaluates from the encoded paint
/// ([`blur_rrect::encode_blurred_rounded_rect`]), so a non-finite one would
/// reach the flattener and the paint encoding exactly as a non-finite rounded
/// rect's radii already do. A lowered command carrying no geometry at all
/// ([`Command::PopClip`] and its two siblings) has nothing to check and sits
/// with the skipped group.
///
/// The match is exhaustive over every [`Command`] variant, the same as
/// [`SceneCompiler::compile_command`]'s: a variant added to the enum fails to
/// compile here until it is placed in the checked group or the unchecked one.
/// Moving a variant *between* those two groups is not itself compiler-enforced
/// — the match stays exhaustive either way — so that half of the discipline
/// still has to be kept by hand alongside `compile_command`.
fn check_geometry(command: &Command) -> Result<(), EngineError> {
    let finite = match command {
        Command::FillRect { rect, .. } => rect.is_finite(),
        Command::RoundedRect { rect, radii, .. } => rect.is_finite() && radii_are_finite(*radii),
        Command::Line { p0, p1, width, .. } => {
            p0.is_finite() && p1.is_finite() && width.is_finite()
        }
        Command::Path { path, style, .. } => path.is_finite() && style_is_finite(style),
        Command::PushClip { rect, .. } => rect.is_finite(),
        Command::PushClipRounded { rect, radii, .. } => {
            rect.is_finite() && radii_are_finite(*radii)
        }
        Command::PushLayer { rect, alpha, .. } => rect.is_finite() && alpha.is_finite(),
        Command::ClearRect { rect, .. } => rect.is_finite(),
        Command::PushSnapshot {
            rect, alpha, scale, ..
        } => rect.is_finite() && alpha.is_finite() && scale.is_finite(),
        Command::Image { dest, .. } => dest.is_finite(),
        Command::BlurredRoundedRect {
            rect,
            radii,
            std_dev,
            ..
        } => rect.is_finite() && radii_are_finite(*radii) && std_dev.is_finite(),
        // Carrying no geometry of their own — see above.
        Command::GlyphRun(_)
        | Command::PopClip
        | Command::PopLayer
        | Command::ShaderQuad { .. }
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
///
/// Public for the same reason [`dash_path`] and [`well_formed`] are: the CPU
/// oracle carries an identical copy, and a cross-crate test pins the two
/// against each other so they cannot drift apart silently.
pub fn dash_cycle_is_normalizable(dash: &DashPattern) -> bool {
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
///
/// [`clip`] admits a rectangular clip to its scissor path by the same rule and
/// through this same function: a rectangle whose coverage can be written
/// exactly is a rectangle whose *clip* can be applied exactly, so the two share
/// one admission rule rather than two that could drift apart.
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
