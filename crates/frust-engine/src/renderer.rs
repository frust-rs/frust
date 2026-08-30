//! [`EngineRenderer`]: the public seam a host drives one surface's 2D frames
//! through.
//!
//! # The encode contract
//!
//! [`EngineRenderer::encode`] **records into a `wgpu::CommandEncoder` the
//! caller owns and never submits it.** That is the whole point of the seam: a
//! host compositing 2D over its own 3D content records its passes before and
//! after the engine's into one encoder and submits once, and a submit hidden
//! inside the engine would split that into two command buffers with a pipeline
//! flush between them. Two obligations follow, and both are contract rather
//! than preference:
//!
//! - every pass the engine begins is ended before `encode` returns, so the
//!   caller's next `begin_render_pass` on the same encoder is legal; and
//! - the engine issues no `queue.submit` for scene work. It does issue
//!   `queue.write_texture`/`write_buffer` uploads, which are ordered ahead of
//!   the command buffers submitted after them and so land before the passes
//!   that read them.
//!
//! # The three passes
//!
//! A frame records up to three passes into that encoder, in this order.
//!
//! 1. **Clear.** Clears the colour target to the frame's base colour, and the
//!    depth attachment to the far plane unless the caller stated it is already
//!    populated (see [`crate::gpu::depth`]). It draws nothing; separating it
//!    from the strip passes is what lets a frame with no draws at all still
//!    resolve to a clean surface.
//! 2. **Opaque strips**, depth-tested and depth-writing, unblended. Only the
//!    fully-covered interior spans of opaque draws reach it — an anti-aliased
//!    edge is by definition not opaque. The depth it establishes is what lets
//!    the alpha pass reject fragments an opaque draw in front of them already
//!    covered.
//! 3. **Alpha strips**, premultiplied-blended, in painter order. Depth-tested
//!    but not depth-writing when a depth attachment is in play; a plain
//!    painter's-algorithm pass when it is not.
//!
//! With depth unavailable — no attachment, or `FRUST_ENGINE_NO_DEPTH` set —
//! passes 2 and 3 collapse into one blended pass carrying *every* instance in
//! painter order. That is a correctness requirement rather than a fallback
//! detail: routing the opaque spans into a separate, earlier pass is only sound
//! because the depth buffer re-establishes their ordering against the blended
//! ones.
//!
//! # Paint resolution
//!
//! A solid colour travels inside the strip instance itself. Anything else the
//! compiler encoded — today, a gradient — is resolved once per frame before a
//! single instance is built, in three steps that have to happen in this order:
//!
//! 1. the frame's LUT requests are serviced through the [`GradientCache`], so
//!    every gradient's colour ramp is resident and has an offset into the
//!    packed LUT buffer;
//! 2. each encoded paint is lowered into the [`GpuEncodedPaint`] record the
//!    fragment shader samples, carrying that offset; and
//! 3. the records are serialized back to back, which fixes the texel each one
//!    starts at — the index a strip instance names its paint by.
//!
//! Ramp offsets are only valid within the frame that took them: the cache
//! compacts and rewrites them in [`EngineRenderer::end_frame`], which is why
//! residency is decided here rather than at compile time.
//!
//! A paint that still cannot be resolved — an image, whose pixels need an
//! atlas the engine does not own yet — leaves its draw skipped rather than
//! stamped in a wrong colour: the same "a frame draws less, never wrong" rule
//! the compiler follows for the commands it does not lower.

use std::collections::HashMap;
use std::sync::{Arc, Once};

use frust_gpu::{PipelineCache, ShaderLibrary, TextureId, TierCaps};
use frust_scene::Scene;
use kurbo::Affine;
use peniko::Color;
use vello_common::fearless_simd::Level;
use vello_common::paint::Paint;
use vello_common::strip::Strip;

use crate::cache::{BYTES_PER_TEXEL, CachedRamp, GradientCache, GradientTextureLayout};
use crate::compile::paint::resolve_lut_request;
use crate::compile::{CompiledFrame, SceneCompiler};
use crate::config;
use crate::error::EngineError;
use crate::gpu::depth::DepthAttachment;
use crate::gpu::paint_texture::lower_encoded_paint;
use crate::gpu::pipelines::{EnginePipeline, EngineShaders, warm_up_descs};
use crate::gpu::strips::{PaintType, pack_paint_descriptor};
use crate::gpu::targets::IntermediateTargets;
use crate::gpu::{self, GpuConfig, GpuEncodedPaint, GpuStrip, StripDraw};
use crate::{EngineTarget, OutputAlpha};

/// The packed paint descriptor of an inline premultiplied solid colour.
///
/// A solid paint indexes no encoded-paint record, so its descriptor carries
/// only the colour source and paint type — both of which are zero, which is
/// why this is named rather than written as a bare `0` at its call site: the
/// zero is a coincidence of the layout, not an absence of information.
const SOLID_PAINT: u32 = pack_paint_descriptor(PaintType::Solid, 0);

/// The smallest resource-texture dimension the engine can address.
///
/// A resource texture's row stride is its width times its texel size, and
/// `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` is 256; the narrowest of the three
/// resources is the gradient LUT at 4 bytes per texel, so 64 texels is the
/// point below which an upload's `bytes_per_row` stops being legal.
const MIN_RESOURCE_TEXTURE_DIM: u32 = 64;

/// The smallest strip instance buffer the engine allocates, in instances.
///
/// A first frame with a handful of strips should not force a second allocation
/// on the second frame.
const MIN_INSTANCE_CAPACITY: u64 = 4096;

static INDEXED_PAINT_WARNING: Once = Once::new();

/// One surface's 2D render engine: a scene in, recorded passes out.
///
/// Create one per surface and keep it across frames — the retained scene
/// compiler, gradient cache, pipeline cache, resource textures and intermediate
/// pool are the reason a steady-state frame allocates nothing.
#[derive(Debug)]
pub struct EngineRenderer {
    caps: TierCaps,
    format: wgpu::TextureFormat,
    shaders: EngineShaders,
    pipelines: PipelineCache,
    compiler: SceneCompiler,
    gradients: GradientCache,
    depth: DepthAttachment,
    targets: IntermediateTargets,
    textures: HashMap<TextureId, wgpu::TextureView>,
    resources: FrameResources,
    scratch: Scratch,
}

impl EngineRenderer {
    /// Builds a renderer for `format` targets on `caps`' adapter, warming
    /// every engine pipeline in the background.
    ///
    /// `pipeline_cache` is the host's persisted driver cache when it has one —
    /// `None` on every backend but Vulkan.
    ///
    /// # Errors
    ///
    /// [`EngineError::AtlasError`] when the adapter's `resource_texture_dim`
    /// is not a power of two of at least [`MIN_RESOURCE_TEXTURE_DIM`]. The
    /// strip shader reconstructs a resource texture's width as `1 << bits` and
    /// an upload's row stride must satisfy wgpu's copy alignment, so neither
    /// the shader's addressing nor the uploads would be valid otherwise.
    pub fn new(
        device: &wgpu::Device,
        caps: &TierCaps,
        format: wgpu::TextureFormat,
        pipeline_cache: Option<&wgpu::PipelineCache>,
    ) -> Result<Self, EngineError> {
        let dim = caps.resource_texture_dim;
        if !dim.is_power_of_two() || dim < MIN_RESOURCE_TEXTURE_DIM {
            return Err(EngineError::AtlasError);
        }

        let mut library = ShaderLibrary::new();
        let shaders = EngineShaders::register(&mut library, device);
        let mut pipelines = PipelineCache::new(Arc::new(library), pipeline_cache.cloned());
        pipelines.warm_up(device, &warm_up_descs(&shaders, format));

        let level = Level::try_detect().unwrap_or(Level::baseline());
        let gradients = GradientCache::for_texture(GradientTextureLayout::square(dim), level);

        Ok(Self {
            caps: caps.clone(),
            format,
            shaders,
            pipelines,
            // The viewport is re-asserted on every compile, so this is only an
            // initial allocation hint.
            compiler: SceneCompiler::new(1, 1),
            gradients,
            depth: DepthAttachment::new(),
            targets: IntermediateTargets::new(caps),
            textures: HashMap::new(),
            resources: FrameResources::new(device, dim),
            scratch: Scratch::default(),
        })
    }

    /// The target format this renderer warmed its pipelines for.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// The adapter capabilities every sizing decision is made against.
    #[must_use]
    pub fn caps(&self) -> &TierCaps {
        &self.caps
    }

    /// The pool the engine's off-screen intermediates come from.
    #[must_use]
    pub fn targets(&self) -> &IntermediateTargets {
        &self.targets
    }

    /// Finishes pipeline warm-up on the calling thread, returning only once
    /// every engine pipeline exists.
    ///
    /// Warm-up is started in the background by [`Self::new`] and normally
    /// needs no attention. Two callers want it forced: a host that must not
    /// let the *first* frame pay for a compile, and anything about to drop the
    /// `wgpu::Device` shortly after building a renderer — the warm-up worker
    /// holds its own handle on that device, and tearing it down while the
    /// worker is mid-compile is a driver-level hazard rather than a clean
    /// cancellation.
    ///
    /// Each variant is requested through the cache, which builds a queued one
    /// inline and waits for one the worker has already started, so nothing is
    /// compiled twice and nothing is left for the worker to claim afterwards.
    pub fn finish_warm_up(&mut self, device: &wgpu::Device) {
        for pipeline in EnginePipeline::ALL {
            let _ = self
                .pipelines
                .get_or_create(device, &pipeline.desc(&self.shaders, self.format));
        }
    }

    /// How many render pipelines this renderer has compiled so far.
    ///
    /// Counts *distinct* pipelines, which is why it is a diagnostic rather
    /// than something to wait on: two entries of [`EnginePipeline::ALL`] that
    /// differ only in their colour format describe the same pipeline whenever
    /// the frame's target format happens to equal
    /// [`crate::gpu::pipelines::INTERMEDIATE_FORMAT`], and the cache compiles
    /// that one variant once. Use [`Self::finish_warm_up`] to wait.
    #[must_use]
    pub fn compiled_pipelines(&self) -> u64 {
        self.pipelines.compiled_variants()
    }

    /// States whether a caller-supplied depth attachment already holds the
    /// depth this frame should test against — see
    /// [`DepthAttachment::set_pre_cleared`].
    pub fn set_depth_pre_cleared(&mut self, pre_cleared: bool) {
        self.depth.set_pre_cleared(pre_cleared);
    }

    /// Registers `view` under `id` so a scene referencing that texture can
    /// resolve it, returning whatever was registered under `id` before.
    pub fn bind_texture(
        &mut self,
        id: TextureId,
        view: wgpu::TextureView,
    ) -> Option<wgpu::TextureView> {
        self.textures.insert(id, view)
    }

    /// Removes the view registered under `id`, returning it.
    pub fn unbind_texture(&mut self, id: TextureId) -> Option<wgpu::TextureView> {
        self.textures.remove(&id)
    }

    /// The view registered under `id`, if any.
    #[must_use]
    pub fn bound_texture(&self, id: TextureId) -> Option<&wgpu::TextureView> {
        self.textures.get(&id)
    }

    /// How many external textures are currently bound.
    #[must_use]
    pub fn bound_texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Releases everything sized against the old surface extent and
    /// re-establishes what the next frame needs at the new one.
    ///
    /// Pipelines, shader modules, the gradient cache and the resource textures
    /// are all extent-independent and deliberately survive: a resize must not
    /// cost a pipeline rebuild or a ramp re-bake. What goes is the intermediate
    /// pool's parked entries — every one keyed on an extent nothing will ask
    /// for again — and the engine-owned depth attachment, which has to match
    /// its colour attachment exactly. Reallocating the depth buffer here rather
    /// than on the next frame keeps it off the frame path.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.targets.drop_parked();
        self.depth.resize(device, width, height);
    }

    /// Closes the frame out: ages the intermediate pool by one frame and
    /// evicts the gradient cache down to its capacity.
    ///
    /// Call once per frame, after the frame's commands have been submitted and
    /// including frames that drew nothing — those are the frames a parked
    /// intermediate ages on. Gradient eviction compacts the packed LUT buffer
    /// and rewrites the offsets of the survivors, which is why it belongs at
    /// the frame boundary rather than mid-frame, where it would invalidate
    /// offsets the frame's own encoded paints already carry.
    ///
    /// `_queue` is part of the signature because the end-of-frame maintenance
    /// this method owns grows queue writes as the engine does (an atlas region
    /// cleared after the frame that consumed it, in the reference renderer);
    /// it has none of them yet.
    pub fn end_frame(&mut self, _queue: &wgpu::Queue) {
        self.targets.end_frame();
        self.gradients.maintain();
    }

    /// Compiles `scene` and records the frame's passes into `encoder`.
    ///
    /// `root` is applied ahead of every command's own transform and
    /// `base_color` is what the target is cleared to before anything is drawn.
    /// Neither the encoder nor the queue is submitted — see the module header.
    ///
    /// # Errors
    ///
    /// [`EngineError::TargetTooLarge`] for a target outside the `u16` device
    /// grid the strip pipeline addresses, [`EngineError::InvalidTransform`] for
    /// a non-finite transform, [`EngineError::AlphaCapacity`] when a frame's
    /// coverage outgrows the alpha texture, and [`EngineError::PaintCapacity`]
    /// when its encoded paints or colour ramps outgrow theirs. Every one of
    /// them is returned before anything is recorded, so a refused frame leaves
    /// `encoder` exactly as it was found.
    #[expect(
        clippy::too_many_arguments,
        reason = "the seam frust-render drives: device, queue, encoder, scene, \
                  target, base colour and root transform are each supplied by a \
                  different owner, so bundling them would only move the \
                  assembly to every call site"
    )]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene,
        target: EngineTarget<'_>,
        base_color: Color,
        root: Affine,
    ) -> Result<(), EngineError> {
        let size = grid_size(target.width, target.height)?;
        let mut frame = self.compiler.compile(scene, root, size)?;

        // Depth is available only when there is an attachment to use and the
        // kill switch is off. Settling that before a single instance is built
        // is what keeps the opaque/alpha split and the pass shape in agreement.
        let depth_enabled = !config::depth_disabled();
        if depth_enabled && target.depth.is_none() {
            self.depth.ensure(device, target.width, target.height);
        }
        let depth_view = depth_enabled
            .then(|| target.depth.or_else(|| self.depth.owned_view()))
            .flatten();

        // Paints are resolved before instances are built: an instance names
        // its paint by the texel its record starts at, which only exists once
        // the frame's ramps are resident and its records are laid out.
        self.resources.resolve_paints(&frame, &mut self.gradients);
        self.scratch
            .build_instances(&frame, depth_view.is_some(), &self.resources.paint_slots);

        // Everything that can fail does so here, ahead of the first
        // `begin_render_pass`.
        let dim = self.caps.resource_texture_dim;
        let alphas_grown = gpu::grow_alpha_texture_height(
            self.resources.alphas.height,
            frame.alphas().len(),
            dim,
        )?;
        let paints_grown = gpu::paint_texture::grow_encoded_paints_texture_height(
            self.resources.paints.height,
            self.resources.paint_texels(),
            dim,
        )?;
        let gradients_grown = self.resources.grown_gradient_height(&self.gradients)?;

        self.resources.resize_alphas(device, alphas_grown);
        self.resources.resize_paints(device, paints_grown);
        self.resources.resize_gradients(device, gradients_grown);
        self.resources
            .upload(queue, &mut frame, &mut self.gradients, size, dim);
        self.resources
            .upload_instances(device, queue, &self.scratch);

        let format = target.format;
        let alpha_variant = if depth_view.is_some() {
            EnginePipeline::StripDepthAlpha
        } else {
            EnginePipeline::StripAlpha
        };
        let alpha_pipeline = self
            .pipelines
            .get_or_create(device, &alpha_variant.desc(&self.shaders, format))
            .clone();
        let opaque_pipeline = depth_view.is_some().then(|| {
            self.pipelines
                .get_or_create(
                    device,
                    &EnginePipeline::StripOpaque.desc(&self.shaders, format),
                )
                .clone()
        });

        self.resources
            .ensure_bind_groups(device, alpha_variant, &alpha_pipeline, format);
        if let Some(pipeline) = opaque_pipeline.as_ref() {
            self.resources.ensure_bind_groups(
                device,
                EnginePipeline::StripOpaque,
                pipeline,
                format,
            );
        }

        self.record_passes(
            encoder,
            &target,
            depth_view,
            base_color,
            opaque_pipeline
                .as_ref()
                .map(|p| (EnginePipeline::StripOpaque, p)),
            (alpha_variant, &alpha_pipeline),
        );

        Ok(())
    }

    /// Records the clear pass and whichever strip passes have instances.
    ///
    /// Every pass opened here is ended before the method returns, which is the
    /// half of the encode contract a caller cannot check for itself.
    fn record_passes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &EngineTarget<'_>,
        depth_view: Option<&wgpu::TextureView>,
        base_color: Color,
        opaque: Option<(EnginePipeline, &wgpu::RenderPipeline)>,
        alpha: (EnginePipeline, &wgpu::RenderPipeline),
    ) {
        let depth_load = self.depth.load_op(target.depth.is_some());

        // Drawing nothing is the point: this pass exists so a frame with no
        // instances at all still resolves to a clean surface.
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frust-engine clear"),
            color_attachments: &[Some(color_attachment(
                target.view,
                wgpu::LoadOp::Clear(clear_color(base_color, target.output)),
            ))],
            depth_stencil_attachment: depth_view.map(|view| depth_attachment(view, depth_load)),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }));

        let Some(instances) = self.resources.instances.as_ref() else {
            return;
        };
        let opaque_count = self.scratch.opaque.len() as u32;
        let alpha_count = self.scratch.alpha.len() as u32;

        if opaque_count > 0
            && let Some((variant, pipeline)) = opaque
        {
            self.strip_pass(
                encoder,
                "frust-engine opaque strips",
                target.view,
                depth_view,
                variant,
                pipeline,
                instances,
                GpuStrip::instance_range(0, opaque_count),
            );
        }

        if alpha_count > 0 {
            self.strip_pass(
                encoder,
                "frust-engine alpha strips",
                target.view,
                depth_view,
                alpha.0,
                alpha.1,
                instances,
                GpuStrip::instance_range(opaque_count, alpha_count),
            );
        }
    }

    /// Records one strip pass: load the colour target, bind `variant`'s own
    /// bind groups, and draw `range` out of the shared instance buffer.
    ///
    /// The bind groups are looked up per pipeline variant rather than shared,
    /// because every engine pipeline uses wgpu's *derived* layout — a layout
    /// derived from a shader module is exclusive to the pipeline that derived
    /// it, so a bind group built against one variant's layout is rejected by
    /// another's even when the two layouts are structurally identical.
    #[expect(
        clippy::too_many_arguments,
        reason = "one pass's full recording state; a struct would name the \
                  same borrows without shortening their list"
    )]
    fn strip_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
        view: &wgpu::TextureView,
        depth_view: Option<&wgpu::TextureView>,
        variant: EnginePipeline,
        pipeline: &wgpu::RenderPipeline,
        instances: &wgpu::Buffer,
        range: core::ops::Range<u32>,
    ) {
        let Some(bind_groups) = self.resources.bind_groups.get(&variant) else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(color_attachment(view, wgpu::LoadOp::Load))],
            depth_stencil_attachment: depth_view
                .map(|view| depth_attachment(view, wgpu::LoadOp::Load)),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        bind_groups.bind(&mut pass);
        pass.set_vertex_buffer(0, instances.slice(..));
        pass.draw(GpuStrip::vertex_range(), range);
    }
}

/// A colour attachment over `view` with `load`, keeping what it stores.
fn color_attachment(
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load,
            store: wgpu::StoreOp::Store,
        },
    }
}

/// A depth attachment over `view` with `load`, keeping what it stores so the
/// pass after it tests against the same buffer.
fn depth_attachment(
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<f32>,
) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    wgpu::RenderPassDepthStencilAttachment {
        view,
        depth_ops: Some(wgpu::Operations {
            load,
            store: wgpu::StoreOp::Store,
        }),
        stencil_ops: None,
    }
}

/// The frame's base colour as a clear value.
///
/// The strip pipelines blend premultiplied and the shader emits premultiplied
/// colour, so a [`OutputAlpha::Premultiplied`] target's clear value has to be
/// premultiplied too — otherwise the background sits in a different alpha
/// convention from everything drawn over it. `Straight` is honoured here and
/// only here: the pipelines themselves are fixed premultiplied, so a straight
/// target's *drawn* content is premultiplied regardless.
fn clear_color(base_color: Color, output: OutputAlpha) -> wgpu::Color {
    let components = match output {
        OutputAlpha::Premultiplied => base_color.premultiply().components,
        OutputAlpha::Straight => base_color.components,
    };
    wgpu::Color {
        r: f64::from(components[0]),
        g: f64::from(components[1]),
        b: f64::from(components[2]),
        a: f64::from(components[3]),
    }
}

/// The target extent on the `u16` device grid the strip pipeline addresses.
fn grid_size(width: u32, height: u32) -> Result<(u16, u16), EngineError> {
    let width = u16::try_from(width).map_err(|_| EngineError::TargetTooLarge)?;
    let height = u16::try_from(height).map_err(|_| EngineError::TargetTooLarge)?;
    Ok((width, height))
}

/// The frame's instance buffers, retained so a steady-state frame refills them
/// rather than reallocating them.
#[derive(Debug, Default)]
struct Scratch {
    /// Fully-covered spans of opaque draws, drawn unblended with depth write.
    opaque: Vec<GpuStrip>,
    /// Everything else, drawn premultiplied-blended in painter order.
    alpha: Vec<GpuStrip>,
}

impl Scratch {
    /// Turns a compiled frame's draws into instances, routing each to the pass
    /// that can draw it.
    ///
    /// A draw's anti-aliased spans always land in the blended buffer: partial
    /// coverage is not opaque however opaque the paint is. Its fully-covered
    /// spans land in the opaque buffer only when the paint is opaque *and*
    /// depth is available to re-establish their ordering against the blended
    /// ones — otherwise the frame is a plain painter's-algorithm walk and
    /// everything stays in the one buffer, in order.
    fn build_instances(
        &mut self,
        frame: &CompiledFrame,
        depth_active: bool,
        paint_slots: &[Option<ResolvedPaint>],
    ) {
        self.opaque.clear();
        self.alpha.clear();

        let strips = frame.strip_buf();
        for draw in frame.draws() {
            let Some(paint) = pack_paint(&draw.paint, draw.depth, paint_slots) else {
                continue;
            };
            let Some(range) = strips.get(draw.strip_range.clone()) else {
                continue;
            };
            let to_opaque = paint.opaque && depth_active;

            // A generation's last strip is its sentinel, which is what carries
            // the preceding strip's extent — so every instance comes from a
            // pair, and the sentinel itself never becomes one.
            for pair in range.windows(2) {
                self.push_span(&pair[0], &pair[1], paint, to_opaque);
            }
        }
    }

    /// Emits the instances the `strip`/`next` pair describes: the strip's own
    /// alpha-sampled span, plus the solid span filling the gap to `next` when
    /// the winding between them says there is one.
    fn push_span(&mut self, strip: &Strip, next: &Strip, paint: PackedPaint, to_opaque: bool) {
        let values = paint.values_at(strip.x, strip.y);
        let span = GpuStrip::from_strip_pair(strip, next, values);
        if span.width > 0 {
            self.alpha.push(span);
        }
        // A gap starts where the strip ends rather than where it begins, so a
        // position-sampled paint is re-evaluated at the gap's own origin — the
        // gap is a different piece of the scene, not a continuation of the
        // span's sampling.
        if let Some(mut gap) = GpuStrip::gap_fill(strip, next, values) {
            gap.payload = paint.payload_at(gap.x, gap.y);
            if to_opaque {
                self.opaque.push(gap);
            } else {
                self.alpha.push(gap);
            }
        }
    }

    /// The instance bytes, opaque buffer first, so one vertex buffer serves
    /// both passes and each is a contiguous instance range into it.
    fn instance_bytes(&self) -> (&[u8], &[u8]) {
        (
            bytemuck::cast_slice(&self.opaque),
            bytemuck::cast_slice(&self.alpha),
        )
    }
}

/// Where a strip instance's payload comes from.
///
/// A solid paint carries its colour there; every other paint reads its colour
/// from a record instead, and spends the payload on the scene position the
/// paint is sampled at — which is why the payload is per instance rather than
/// per draw.
#[derive(Debug, Clone, Copy)]
enum PaintPayload {
    /// A premultiplied RGBA8 colour, the same for every instance of the draw.
    Solid(u32),
    /// The instance's own scene-space origin, packed as a `u16` pair.
    Position,
}

/// One draw's paint, resolved to the values its instances repeat.
#[derive(Debug, Clone, Copy)]
struct PackedPaint {
    payload: PaintPayload,
    /// The packed paint descriptor, without [`RECT_STRIP_FLAG`](crate::gpu::strips::RECT_STRIP_FLAG).
    paint: u32,
    depth_index: u32,
    /// Whether every pixel this paint produces is opaque, and so whether its
    /// fully-covered spans may take the depth-writing pass.
    opaque: bool,
}

impl PackedPaint {
    /// The payload an instance whose scene origin is `(x, y)` carries.
    fn payload_at(self, x: u16, y: u16) -> u32 {
        match self.payload {
            PaintPayload::Solid(rgba) => rgba,
            PaintPayload::Position => pack_u16_pair(x, y),
        }
    }

    /// The per-instance values for an instance whose scene origin is `(x, y)`.
    fn values_at(self, x: u16, y: u16) -> StripDraw {
        StripDraw {
            payload: self.payload_at(x, y),
            paint: self.paint,
            depth_index: self.depth_index,
        }
    }
}

/// Two `u16`s in one word, low half first — the packing the shader's
/// `unpack_u16_pair` reverses.
fn pack_u16_pair(x: u16, y: u16) -> u32 {
    u32::from(x) | (u32::from(y) << 16)
}

/// The shader values for `paint`, resolved against the frame's paint slots.
///
/// Answers `None` for an indexed paint the frame could not resolve — one whose
/// entry is missing from `paint_slots` entirely, or one the lowering refused
/// because the engine has nowhere to hold its pixels yet (see the module
/// header). The draw is then skipped rather than stamped in a wrong colour.
fn pack_paint(
    paint: &Paint,
    depth: u32,
    paint_slots: &[Option<ResolvedPaint>],
) -> Option<PackedPaint> {
    match paint {
        Paint::Solid(color) => Some(PackedPaint {
            payload: PaintPayload::Solid(color.as_premul_rgba8().to_u32()),
            paint: SOLID_PAINT,
            depth_index: depth,
            opaque: color.is_opaque(),
        }),
        Paint::Indexed(indexed) => {
            let Some(Some(resolved)) = paint_slots.get(indexed.index()) else {
                INDEXED_PAINT_WARNING.call_once(|| {
                    log::warn!(
                        "an indexed paint could not be lowered to a GPU record; those draws are \
                         skipped (logged once)"
                    );
                });
                return None;
            };
            Some(PackedPaint {
                payload: PaintPayload::Position,
                paint: pack_paint_descriptor(resolved.paint_type, resolved.texel_offset),
                depth_index: depth,
                opaque: resolved.opaque,
            })
        }
    }
}

/// One encoded paint after lowering: where its record landed and how a strip
/// instance names it.
#[derive(Debug, Clone, Copy)]
struct ResolvedPaint {
    /// How the fragment shader reads the record.
    paint_type: PaintType,
    /// The texel the record starts at in the encoded-paint texture.
    texel_offset: u32,
    /// Whether every pixel the paint produces is opaque.
    opaque: bool,
}

/// One resource texture and the extent it currently holds.
#[derive(Debug)]
struct ResourceTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl ResourceTexture {
    fn new(device: &wgpu::Device, descriptor: &wgpu::TextureDescriptor<'_>) -> Self {
        let texture = device.create_texture(descriptor);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            width: descriptor.size.width,
            height: descriptor.size.height,
        }
    }

    /// The whole texture as a copy destination.
    fn copy_target(&self) -> wgpu::TexelCopyTextureInfo<'_> {
        wgpu::TexelCopyTextureInfo {
            texture: &self.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        }
    }

    /// The extent one full-texture upload covers.
    fn extent(&self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.width,
            height: self.height,
            depth_or_array_layers: 1,
        }
    }
}

/// The GPU resources one renderer holds across frames.
#[derive(Debug)]
struct FrameResources {
    alphas: ResourceTexture,
    paints: ResourceTexture,
    gradients: ResourceTexture,
    /// Stand-ins for the three bindings the strip shader declares but nothing
    /// in a frame writes yet: the layer input a composited layer would be
    /// sampled from, the glyph/image atlas array, and an externally bound
    /// texture. Every declared binding has to be bound for a pass to validate,
    /// whether or not an instance samples it.
    placeholders: Placeholders,
    config: wgpu::Buffer,
    instances: Option<wgpu::Buffer>,
    instance_capacity: u64,
    /// The GPU records this frame's indexed paints resolve against, in
    /// serialization order — which is the order the texel offsets in
    /// `paint_slots` were taken from.
    paints_data: Vec<GpuEncodedPaint>,
    /// One slot per encoded paint of the frame, indexed by
    /// [`Paint::Indexed`](vello_common::paint::Paint::Indexed): `None` for one
    /// the engine could not lower. Kept parallel to the *encoded* paints
    /// rather than compacted to the lowered ones, because a draw names its
    /// paint by the compiler's index.
    paint_slots: Vec<Option<ResolvedPaint>>,
    /// Reusable per-paint ramp residency, filled from a frame's LUT requests
    /// before its paints are lowered.
    paint_ramps: Vec<Option<CachedRamp>>,
    /// Reusable staging for the encoded-paint upload, padded to the texture's
    /// footprint.
    paint_staging: Vec<u8>,
    /// One set of bind groups per strip pipeline variant. Not one shared set:
    /// every engine pipeline uses wgpu's derived layout, and a derived layout
    /// is exclusive to the pipeline that derived it.
    bind_groups: HashMap<EnginePipeline, StripBindGroups>,
    /// The target format the live bind groups were built against; a change
    /// means different pipelines, so the whole map is dropped rather than
    /// accumulating a set per format ever rendered to.
    bind_group_format: Option<wgpu::TextureFormat>,
}

impl FrameResources {
    fn new(device: &wgpu::Device, dim: u32) -> Self {
        let min = gpu::MIN_RESOURCE_TEXTURE_HEIGHT;
        Self {
            alphas: ResourceTexture::new(device, &gpu::alpha_texture_descriptor(dim, min)),
            paints: ResourceTexture::new(
                device,
                &gpu::paint_texture::encoded_paints_texture_descriptor(dim, min),
            ),
            gradients: ResourceTexture::new(device, &gradient_texture_descriptor(dim, min)),
            placeholders: Placeholders::new(device),
            config: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("frust-engine config uniform"),
                size: GpuConfig::SIZE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            instances: None,
            instance_capacity: 0,
            paints_data: Vec::new(),
            paint_slots: Vec::new(),
            paint_ramps: Vec::new(),
            paint_staging: Vec::new(),
            bind_groups: HashMap::new(),
            bind_group_format: None,
        }
    }

    /// Services `frame`'s LUT requests and lowers its encoded paints into the
    /// records the shader samples.
    ///
    /// Leaves `paints_data` holding the lowered records in serialization order
    /// and `paint_slots` naming, per *encoded* paint, the texel its record
    /// starts at — or `None` where the paint could not be lowered.
    ///
    /// A solid-only frame leaves both empty and touches neither the cache nor
    /// the paint texture, so it costs exactly what it did before paints were
    /// wired up.
    fn resolve_paints(&mut self, frame: &CompiledFrame, cache: &mut GradientCache) {
        self.paints_data.clear();
        self.paint_slots.clear();

        if frame.encoded_paints.is_empty() {
            return;
        }

        // Residency first, for the whole frame: a ramp's offset is only
        // meaningful once the cache has finished baking this frame's misses,
        // and a record built before that would name a ramp that had not been
        // packed yet.
        self.paint_ramps.clear();
        self.paint_ramps.resize(frame.encoded_paints.len(), None);
        for request in &frame.lut_requests {
            let ramp = resolve_lut_request(*request, &frame.encoded_paints, cache);
            if let Some(slot) = self.paint_ramps.get_mut(request.paint_index) {
                *slot = ramp;
            }
        }

        let mut texel_offset = 0;
        for (index, paint) in frame.encoded_paints.iter().enumerate() {
            let ramp = self.paint_ramps.get(index).copied().flatten();
            let slot = lower_encoded_paint(paint, ramp).map(|record| {
                let resolved = ResolvedPaint {
                    paint_type: record.paint_type(),
                    texel_offset,
                    opaque: !paint.may_have_transparency(),
                };
                texel_offset += record.texel_len();
                self.paints_data.push(record);
                resolved
            });
            self.paint_slots.push(slot);
        }
    }

    /// The texels this frame's encoded paints occupy.
    fn paint_texels(&self) -> u32 {
        self.paints_data
            .iter()
            .map(GpuEncodedPaint::texel_len)
            .sum()
    }

    /// The height the gradient LUT texture must grow to for every ramp the
    /// cache has packed, or `None` when it already fits.
    ///
    /// # Errors
    ///
    /// [`EngineError::PaintCapacity`]: a LUT set past the resource dimension
    /// squared has nowhere to live, and the frame path reports it rather than
    /// asserting.
    fn grown_gradient_height(&self, cache: &GradientCache) -> Result<Option<u32>, EngineError> {
        let width = self.gradients.width;
        let texels = u32::try_from(cache.luts_size() / BYTES_PER_TEXEL as usize)
            .map_err(|_| EngineError::PaintCapacity)?;
        let required = texels.div_ceil(width).max(gpu::MIN_RESOURCE_TEXTURE_HEIGHT);
        if required > width {
            return Err(EngineError::PaintCapacity);
        }
        Ok((required > self.gradients.height).then_some(required))
    }

    fn resize_alphas(&mut self, device: &wgpu::Device, height: Option<u32>) {
        if let Some(height) = height {
            self.alphas = ResourceTexture::new(
                device,
                &gpu::alpha_texture_descriptor(self.alphas.width, height),
            );
            self.bind_groups.clear();
        }
    }

    fn resize_paints(&mut self, device: &wgpu::Device, height: Option<u32>) {
        if let Some(height) = height {
            self.paints = ResourceTexture::new(
                device,
                &gpu::paint_texture::encoded_paints_texture_descriptor(self.paints.width, height),
            );
            self.bind_groups.clear();
        }
    }

    fn resize_gradients(&mut self, device: &wgpu::Device, height: Option<u32>) {
        if let Some(height) = height {
            self.gradients = ResourceTexture::new(
                device,
                &gradient_texture_descriptor(self.gradients.width, height),
            );
            self.bind_groups.clear();
        }
    }

    /// Uploads the frame's coverage, paint records, colour ramps and config.
    fn upload(
        &mut self,
        queue: &wgpu::Queue,
        frame: &mut CompiledFrame,
        cache: &mut GradientCache,
        size: (u16, u16),
        dim: u32,
    ) {
        let alphas = &self.alphas;
        gpu::with_padded_alphas(
            &mut frame.strips.alphas,
            alphas.width,
            alphas.height,
            |bytes| {
                queue.write_texture(
                    alphas.copy_target(),
                    bytes,
                    resource_layout(gpu::resource_bytes_per_row(alphas.width), alphas.height),
                    alphas.extent(),
                );
            },
        );

        if !self.paints_data.is_empty() {
            let footprint = gpu::resource_texture_bytes(self.paints.width, self.paints.height);
            self.paint_staging
                .resize(usize::try_from(footprint).unwrap_or(usize::MAX), 0);
            // The height was checked to fit before any pass was recorded, so a
            // buffer too short for the records is unreachable; skipping the
            // upload rather than unwrapping keeps the frame path total anyway.
            if GpuEncodedPaint::serialize_to_buffer(&self.paints_data, &mut self.paint_staging)
                .is_ok()
            {
                queue.write_texture(
                    self.paints.copy_target(),
                    &self.paint_staging,
                    resource_layout(
                        gpu::resource_bytes_per_row(self.paints.width),
                        self.paints.height,
                    ),
                    self.paints.extent(),
                );
            }
        }

        if cache.has_changed() {
            let layout = GradientTextureLayout {
                width: self.gradients.width,
                height: self.gradients.height,
            };
            if let Some(upload) = cache.begin_upload(layout) {
                queue.write_texture(
                    self.gradients.copy_target(),
                    &upload,
                    resource_layout(upload.bytes_per_row(), self.gradients.height),
                    self.gradients.extent(),
                );
            }
            cache.mark_synced();
        }

        let config = GpuConfig::new(u32::from(size.0), u32::from(size.1), dim, dim);
        queue.write_buffer(&self.config, 0, bytemuck::bytes_of(&config));
    }

    /// Grows the instance buffer if this frame outgrew it, then uploads the
    /// opaque and alpha instances back to back.
    fn upload_instances(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scratch: &Scratch) {
        let (opaque, alpha) = scratch.instance_bytes();
        let required = (opaque.len() + alpha.len()) as u64;
        if required == 0 {
            return;
        }

        let buffer = match &mut self.instances {
            Some(buffer) if self.instance_capacity >= required => buffer,
            slot => {
                let floor = MIN_INSTANCE_CAPACITY * size_of::<GpuStrip>() as u64;
                let capacity = required
                    .checked_next_power_of_two()
                    .unwrap_or(required)
                    .max(floor);
                self.instance_capacity = capacity;
                slot.insert(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("frust-engine strip instances"),
                    size: capacity,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }))
            }
        };
        if !opaque.is_empty() {
            queue.write_buffer(buffer, 0, opaque);
        }
        if !alpha.is_empty() {
            queue.write_buffer(buffer, opaque.len() as u64, alpha);
        }
    }

    /// Builds `variant`'s bind groups if it has none, first dropping every
    /// set when the target format changed under them.
    fn ensure_bind_groups(
        &mut self,
        device: &wgpu::Device,
        variant: EnginePipeline,
        pipeline: &wgpu::RenderPipeline,
        format: wgpu::TextureFormat,
    ) {
        if self.bind_group_format != Some(format) {
            self.bind_groups.clear();
            self.bind_group_format = Some(format);
        }
        if self.bind_groups.contains_key(&variant) {
            return;
        }
        let groups = StripBindGroups::new(
            device,
            pipeline,
            &self.alphas.view,
            &self.config,
            &self.placeholders,
            &self.paints.view,
            &self.gradients.view,
        );
        self.bind_groups.insert(variant, groups);
    }
}

/// The texel copy layout of a full resource-texture upload.
fn resource_layout(bytes_per_row: u32, rows: u32) -> wgpu::TexelCopyBufferLayout {
    wgpu::TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(bytes_per_row),
        rows_per_image: Some(rows),
    }
}

/// The descriptor for a gradient LUT texture of `width` x `height` texels.
fn gradient_texture_descriptor(width: u32, height: u32) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("frust-engine gradient texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: GradientTextureLayout::FORMAT,
        usage: gpu::RESOURCE_TEXTURE_USAGES,
        view_formats: &[],
    }
}

/// The 1x1 stand-ins for the strip shader's not-yet-written bindings.
#[derive(Debug)]
struct Placeholders {
    layer_input: wgpu::TextureView,
    atlas_array: wgpu::TextureView,
    external: wgpu::TextureView,
}

impl Placeholders {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            layer_input: placeholder_view(device, "frust-engine layer input placeholder", false),
            atlas_array: placeholder_view(device, "frust-engine atlas placeholder", true),
            external: placeholder_view(device, "frust-engine external placeholder", false),
        }
    }
}

/// A 1x1 transparent `Rgba8Unorm` texture's view, as a plain 2D texture or as
/// a single-layer 2D array.
fn placeholder_view(device: &wgpu::Device, label: &str, array: bool) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some(label),
        dimension: Some(if array {
            wgpu::TextureViewDimension::D2Array
        } else {
            wgpu::TextureViewDimension::D2
        }),
        ..Default::default()
    })
}

/// The four bind groups every strip pass sets.
///
/// The grouping is the shader's, not this module's: coverage plus config plus
/// layer input, atlas array plus external texture, encoded paints, gradient
/// ramps — four groups exactly, which is the downlevel ceiling with no
/// headroom left.
#[derive(Debug)]
struct StripBindGroups {
    resources: wgpu::BindGroup,
    images: wgpu::BindGroup,
    paints: wgpu::BindGroup,
    gradients: wgpu::BindGroup,
}

impl StripBindGroups {
    /// Takes the seven resources one by one rather than a `&FrameResources`
    /// so the caller can build a set while holding the map it lands in
    /// mutably.
    fn new(
        device: &wgpu::Device,
        pipeline: &wgpu::RenderPipeline,
        alphas: &wgpu::TextureView,
        config: &wgpu::Buffer,
        placeholders: &Placeholders,
        paints: &wgpu::TextureView,
        gradients: &wgpu::TextureView,
    ) -> Self {
        let resources_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frust-engine strip resources"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(alphas),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: config.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&placeholders.layer_input),
                },
            ],
        });
        let images = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frust-engine strip images"),
            layout: &pipeline.get_bind_group_layout(1),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&placeholders.atlas_array),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&placeholders.external),
                },
            ],
        });
        let paints = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frust-engine strip paints"),
            layout: &pipeline.get_bind_group_layout(2),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(paints),
            }],
        });
        let gradients = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frust-engine strip gradients"),
            layout: &pipeline.get_bind_group_layout(3),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(gradients),
            }],
        });

        Self {
            resources: resources_group,
            images,
            paints,
            gradients,
        }
    }

    /// Sets all four groups on `pass`.
    fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(0, &self.resources, &[]);
        pass.set_bind_group(1, &self.images, &[]);
        pass.set_bind_group(2, &self.paints, &[]);
        pass.set_bind_group(3, &self.gradients, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peniko::color::palette::css::{BLUE, RED};
    use vello_common::paint::IndexedPaint;

    #[test]
    fn a_target_past_the_u16_grid_is_refused() {
        assert_eq!(
            grid_size(1920, 1080).expect("a normal surface"),
            (1920, 1080)
        );
        assert_eq!(
            grid_size(u32::from(u16::MAX), 16).expect("the grid ceiling itself"),
            (u16::MAX, 16)
        );
        assert!(matches!(
            grid_size(u32::from(u16::MAX) + 1, 16),
            Err(EngineError::TargetTooLarge)
        ));
        assert!(matches!(
            grid_size(16, u32::from(u16::MAX) + 1),
            Err(EngineError::TargetTooLarge)
        ));
    }

    #[test]
    fn a_premultiplied_clear_folds_alpha_into_the_colour() {
        let half = RED.with_alpha(0.5);
        let premultiplied = clear_color(half, OutputAlpha::Premultiplied);
        let straight = clear_color(half, OutputAlpha::Straight);

        assert!((premultiplied.r - 0.5).abs() < 1e-6);
        assert!((premultiplied.a - 0.5).abs() < 1e-6);
        assert!((straight.r - 1.0).abs() < 1e-6);
        assert!((straight.a - 0.5).abs() < 1e-6);

        // An opaque colour is identical under both conventions.
        assert_eq!(
            clear_color(BLUE, OutputAlpha::Premultiplied),
            clear_color(BLUE, OutputAlpha::Straight)
        );
    }

    #[test]
    fn a_solid_paint_travels_premultiplied_in_the_instance_payload() {
        let paint = pack_paint(&Paint::from(RED), 3, &[]).expect("a solid paint always resolves");
        let values = paint.values_at(40, 12);
        assert_eq!(values.paint, SOLID_PAINT);
        assert_eq!(values.depth_index, 3);
        assert_eq!(values.payload, RED.premultiply().to_rgba8().to_u32());
        assert!(paint.opaque);
        assert_eq!(
            paint.values_at(0, 0).payload,
            values.payload,
            "a solid colour is the same wherever it is stamped"
        );

        let paint = pack_paint(&Paint::from(RED.with_alpha(0.5)), 0, &[])
            .expect("a translucent solid paint still resolves");
        assert!(
            !paint.opaque,
            "a translucent paint never reaches the opaque pass"
        );
    }

    #[test]
    fn an_unresolvable_indexed_paint_skips_its_draw() {
        let indexed = Paint::Indexed(IndexedPaint::new(0));
        assert!(
            pack_paint(&indexed, 0, &[]).is_none(),
            "an index past the frame's slots resolves to nothing"
        );
        assert!(
            pack_paint(&indexed, 0, &[None]).is_none(),
            "so does a slot the lowering refused"
        );
    }

    #[test]
    fn a_resolved_indexed_paint_samples_at_each_instances_own_position() {
        let slots = [Some(ResolvedPaint {
            paint_type: PaintType::LinearGradient,
            texel_offset: 6,
            opaque: true,
        })];
        let paint = pack_paint(&Paint::Indexed(IndexedPaint::new(0)), 2, &slots)
            .expect("a lowered paint resolves");

        assert_eq!(
            paint.paint,
            pack_paint_descriptor(PaintType::LinearGradient, 6)
        );
        assert_eq!(paint.depth_index, 2);
        // The payload is the instance's scene origin, not a colour, so two
        // instances of the same draw carry different payloads.
        assert_eq!(paint.payload_at(8, 4), 8 | (4 << 16));
        assert_eq!(paint.payload_at(40, 4), 40 | (4 << 16));
    }

    #[test]
    fn a_u16_pair_packs_low_half_first() {
        assert_eq!(pack_u16_pair(0, 0), 0);
        assert_eq!(pack_u16_pair(1, 0), 1);
        assert_eq!(pack_u16_pair(0, 1), 1 << 16);
        assert_eq!(pack_u16_pair(u16::MAX, u16::MAX), u32::MAX);
    }

    #[test]
    fn the_minimum_resource_dimension_keeps_every_upload_row_legal() {
        // The gradient LUT is the narrowest resource at four bytes per texel,
        // so it is the one that fixes the floor.
        assert_eq!(
            MIN_RESOURCE_TEXTURE_DIM * BYTES_PER_TEXEL,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
        );
        assert!(MIN_RESOURCE_TEXTURE_DIM.is_power_of_two());
    }
}
