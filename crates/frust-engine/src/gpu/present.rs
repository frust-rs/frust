//! The present pass: the engine's premultiplied frame, converted for a
//! swapchain that stores STRAIGHT alpha.
//!
//! Every engine pipeline blends and writes premultiplied alpha (see
//! [`super::pipelines`]), which is exactly what an opaque swapchain — or a
//! premultiplied-expecting translucent one, Android's `Inherit` — wants. The
//! one composite alpha mode that disagrees is iOS's `PostMultiplied`: it reads
//! the frame's `(C·a, a)` as `(C, a)`, so every partial-alpha pixel presents
//! too dark. [`UnpremultiplyPass`] is what serves that surface — the frame is
//! rendered into an intermediate of the host's own and one full-screen
//! triangle writes `(rgb / max(a, ALPHA_FLOOR), a)` into the swapchain, in
//! `unpremultiply.wgsl`.
//!
//! **A render pass, never a compute one.** The conversion is a fragment write
//! into an ordinary `RENDER_ATTACHMENT` colour target: neither a compute stage
//! nor a storage texture (E1/E2), both of which the engine refuses by design
//! and neither of which a downlevel target offers. A swapchain configured for
//! this tier carries `RENDER_ATTACHMENT` and nothing else, so a storage write
//! into it is not merely disallowed by the rules — it is unavailable.
//!
//! **The host decides, the pass states the rule.** Which surfaces need it is
//! [`UnpremultiplyPass::selected_by`]: an [`OutputAlpha::Straight`] *swapchain*
//! does, an [`OutputAlpha::Premultiplied`] one does not. Note the split that
//! implies for a host on this arm — the intermediate the engine renders into
//! really does hold premultiplied pixels, so it is described to
//! [`crate::EngineRenderer::encode`] as [`OutputAlpha::Premultiplied`] (which
//! is what makes the frame's base colour clear in the same convention as
//! everything drawn over it); `Straight` describes the *swapchain* this pass
//! then writes, not the buffer it reads.
//!
//! **A plain copy, for a frame that has to read itself back.** A frame carrying
//! a backdrop blur reads a region of its own target back into a page
//! ([`crate::renderer::RegionCopyMode`]), which a swapchain — `RENDER_ATTACHMENT`
//! only, never widened — cannot be read back from. [`CopyPresentPass`] serves
//! that frame on an opaque or premultiplied surface: the host renders it into
//! an intermediate of the swapchain's own format and this pass copies it
//! across, one full-screen quad through the engine's own region-copy program
//! (`copy.wgsl`), sharing the conversion pass's plumbing — a pass the caller
//! records into its own encoder, clearing a fresh swapchain image, reading one
//! sampled texture. It costs one full-screen fragment pass on exactly the
//! frames that carry a backdrop; a straight-alpha surface already renders
//! every frame into an intermediate and needs no second pass.
//!
//! **The caller owns the encoder.** [`UnpremultiplyPass::record`] records into
//! a `wgpu::CommandEncoder` it is handed and never submits, the same
//! single-submit contract `frust_gpu::encoder` states and
//! [`crate::EngineRenderer::encode`] keeps: the frame's own passes and this one
//! land in one command buffer, in order, against the texture about to be
//! presented.

use std::sync::Arc;

use frust_gpu::lint::PipelineLayoutDesc;
use frust_gpu::{PipelineCache, RenderPipelineDesc, ShaderId, ShaderLibrary};

use super::pipelines::{FS_MAIN, TOPOLOGY, VS_MAIN, copy_vertex_layout};
use super::shader_src::{COPY, COPY_NAME};
use crate::OutputAlpha;

/// The conversion program, loaded from the shader directory like every other
/// engine module so `frust_gpu::lint::lint_wgsl_dir` sees every line the GPU
/// compiles.
///
/// Not assembled in [`super::shader_src`] with the pipeline modules: this is
/// the host's *present* pass rather than one of the frame's own, it needs no
/// helper prelude, and it is compiled per surface by [`UnpremultiplyPass::new`]
/// rather than registered in the renderer's shared library.
pub const UNPREMULTIPLY: &str = include_str!("../../shaders/unpremultiply.wgsl");

/// The name [`UNPREMULTIPLY`] is registered under in this pass's own shader
/// library.
pub const UNPREMULTIPLY_NAME: &str = "frust-engine unpremultiply";

/// The floor the shader divides by, mirroring its own `ALPHA_FLOOR` constant.
///
/// Stated here so a host can document the guard it is buying without reading
/// WGSL. A test below pins the two against each other, since a divisor guard
/// that drifted would only show up as NaN pixels on a device.
pub const ALPHA_FLOOR: f32 = 1e-4;

/// Vertices the full-screen triangle draws. One primitive, no vertex buffer.
const FULLSCREEN_VERTICES: u32 = 3;

/// Bind groups the program declares: its source texture, and nothing else.
const UNPREMULTIPLY_BIND_GROUPS: usize = 1;

/// The straight-alpha present pass for one surface: a pipeline built for that
/// surface's own format plus the bind-group layout its program derived.
///
/// Built once per surface configure (a rare event) — a pipeline object, a
/// bind-group layout and nothing per frame but one bind group.
#[derive(Debug)]
pub struct UnpremultiplyPass {
    pipeline: wgpu::RenderPipeline,
    /// wgpu's default layout, derived from the program — the only layout
    /// `frust_gpu::RenderPipelineDesc` builds with, so it is read back off the
    /// pipeline rather than declared twice.
    bind_group_layout: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
}

impl UnpremultiplyPass {
    /// Whether a swapchain interpreting its alpha as `output` needs this pass.
    ///
    /// The whole routing rule, in one place: `Straight` does, `Premultiplied`
    /// does not — the engine already writes the latter's convention, and a
    /// conversion there would divide every partial-alpha pixel by its own alpha
    /// for nothing.
    #[must_use]
    pub fn selected_by(output: OutputAlpha) -> bool {
        matches!(output, OutputAlpha::Straight)
    }

    /// Builds the pass for a `format` target.
    ///
    /// `driver_cache` is the host's persisted `wgpu::PipelineCache` when it has
    /// one — `None` on every backend but Vulkan, which is every backend this
    /// arm actually runs on today (it exists for iOS/Metal), so the parameter
    /// is about keeping the seam honest rather than about a cache hit.
    ///
    /// The pipeline is built through a `frust_gpu::PipelineCache` of this
    /// pass's own, over a one-module library: the substrate's rule is that a
    /// frame is never the first place a pipeline is compiled, and a surface
    /// configure is not a frame. The cache is not kept — the compiled pipeline
    /// is a reference-counted handle that outlives it, and there is exactly one
    /// variant to ask for.
    #[must_use]
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        driver_cache: Option<&wgpu::PipelineCache>,
    ) -> Self {
        let mut library = ShaderLibrary::new();
        let shader = library.insert_wgsl(device, UNPREMULTIPLY_NAME, UNPREMULTIPLY);
        let mut pipelines = PipelineCache::new(Arc::new(library), driver_cache.cloned());
        let pipeline = pipelines
            .get_or_create(device, &Self::desc(shader, format))
            .clone();
        let bind_group_layout = pipeline.get_bind_group_layout(0);
        Self {
            pipeline,
            bind_group_layout,
            format,
        }
    }

    /// The target format this pass was built for — the surface's own, since it
    /// writes the swapchain directly.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Records the conversion into `encoder`: reads `source_view` (the
    /// premultiplied intermediate the frame was rendered into) and writes
    /// straight-alpha pixels into `target_view` (the acquired swapchain
    /// texture). The caller submits `encoder`.
    ///
    /// No extent is taken: the triangle covers the whole destination whatever
    /// its size, and the fragment stage reads the source at its own position,
    /// so the two views only have to agree with each other — which the surface
    /// that owns both guarantees by recreating them together.
    ///
    /// The attachment is CLEARED rather than loaded even though every pixel of
    /// it is then written: the destination is a fresh swapchain image whose
    /// prior contents mean nothing, and a load would cost a tile read per tile
    /// on exactly the mobile GPUs this tier targets.
    ///
    /// The bind group is built per call rather than cached, mirroring
    /// `frust-render`'s own premultiply pass: it names the source view, which a
    /// resize replaces, so caching it would need an invalidation seam to buy one
    /// allocation a frame.
    ///
    /// `timestamp_writes` is the pass's own `timestamp_writes`, verbatim — the
    /// host asks its `frust_gpu::diag::TimestampRing` (via
    /// `frust_engine::FrameTimestamps`) for one fresh pair charged to
    /// [`crate::diag::EngineSpan::Blit`] and hands it straight through, `None`
    /// on every arm that does not time this pass (an inert ring, a tier build
    /// with no `perf-trace`, or the `EngineDirect` arm this pass never runs on
    /// at all).
    pub fn record(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        target_view: &wgpu::TextureView,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let bind_group = source_bind_group(
            device,
            &self.bind_group_layout,
            source_view,
            "frust-engine unpremultiply bind group",
        );
        let mut pass = begin_present(
            encoder,
            target_view,
            "frust-engine unpremultiply pass",
            timestamp_writes,
        );
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..FULLSCREEN_VERTICES, 0..1);
    }

    /// This pass's shape as the downlevel lint reads it, so the bind-group,
    /// vertex-topology and sample-count ceilings
    /// (`frust_gpu::lint::lint_pipeline_layout`) are checked against the real
    /// description rather than restated in prose.
    #[must_use]
    pub fn layout_desc() -> PipelineLayoutDesc {
        PipelineLayoutDesc {
            bind_group_count: UNPREMULTIPLY_BIND_GROUPS,
            // The program generates its own geometry, so there is no vertex
            // buffer to declare and nothing to step over.
            max_vertex_buffers: 0,
            total_vertex_attributes: 0,
            max_vertex_buffer_stride: 0,
            sample_count: 1,
            uniform_buffer_sizes: Vec::new(),
        }
    }

    /// The pipeline description, which is `RenderPipelineDesc::new`'s minimal
    /// variant unchanged — every one of its defaults is what this pass wants:
    /// no vertex layouts (the program generates its own geometry), no blend
    /// (the pass REPLACES every pixel of the destination — a blend would
    /// composite the frame onto whatever the swapchain image last held), no
    /// depth, one sample, and a triangle list, whose single primitive is the
    /// full-screen triangle itself.
    fn desc(shader: ShaderId, format: wgpu::TextureFormat) -> RenderPipelineDesc {
        RenderPipelineDesc::new(shader, VS_MAIN, FS_MAIN, format)
    }
}

/// Vertices the copy program's one quad draws (`copy.wgsl`'s `quad_corner`,
/// a triangle strip).
const COPY_QUAD_VERTICES: u32 = 4;

/// One copy instance: four `u16` pairs ([`copy_vertex_layout`]).
const COPY_INSTANCE_BYTES: u64 = 16;

/// The plain present pass for one surface: the frame, rendered into an
/// intermediate of the swapchain's own format, copied into the swapchain
/// unconverted (see the module doc's *A plain copy*).
///
/// Built once per surface configure, like [`UnpremultiplyPass`]: a pipeline
/// for the swapchain's format, the bind-group layout its program derived, and
/// the one 16-byte instance the quad is drawn from.
#[derive(Debug)]
pub struct CopyPresentPass {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    instance: wgpu::Buffer,
    format: wgpu::TextureFormat,
}

impl CopyPresentPass {
    /// Builds the pass for a `format` target, through a pipeline cache of its
    /// own exactly as [`UnpremultiplyPass::new`] does — a surface configure is
    /// not a frame, so the one pipeline is compiled here.
    #[must_use]
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        driver_cache: Option<&wgpu::PipelineCache>,
    ) -> Self {
        let mut library = ShaderLibrary::new();
        let shader = library.insert_wgsl(device, COPY_NAME, COPY);
        let mut pipelines = PipelineCache::new(Arc::new(library), driver_cache.cloned());
        let pipeline = pipelines
            .get_or_create(device, &Self::desc(shader, format))
            .clone();
        let bind_group_layout = pipeline.get_bind_group_layout(0);
        let instance = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frust-engine copy present instance"),
            size: COPY_INSTANCE_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            bind_group_layout,
            instance,
            format,
        }
    }

    /// The target format this pass was built for — the swapchain's own.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Records the copy into `encoder`: reads `source_view` (the intermediate
    /// the frame was rendered into, `width` x `height` like the swapchain) and
    /// writes it unconverted over the whole of `target_view` (the acquired
    /// swapchain texture). The caller submits `encoder`.
    ///
    /// The instance is written through `queue` and lands ahead of the command
    /// buffer the caller submits, so a resize between frames is honoured by
    /// the very next one. The attachment is cleared rather than loaded, for
    /// [`UnpremultiplyPass::record`]'s reason. `timestamp_writes` is that
    /// method's too.
    #[expect(
        clippy::too_many_arguments,
        reason = "the device, queue and encoder the pass records with, its two views,                   their shared extent and the timestamp sink, each supplied by a                   different owner"
    )]
    pub fn record(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        target_view: &wgpu::TextureView,
        size: (u32, u32),
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let width = u16::try_from(size.0).unwrap_or(u16::MAX);
        let height = u16::try_from(size.1).unwrap_or(u16::MAX);
        let extent = u32::from(width) | (u32::from(height) << 16);
        // Destination origin, source origin, region size, destination size.
        let instance: [u32; 4] = [0, 0, extent, extent];
        queue.write_buffer(&self.instance, 0, bytemuck::cast_slice(&instance));
        let bind_group = source_bind_group(
            device,
            &self.bind_group_layout,
            source_view,
            "frust-engine copy present bind group",
        );
        let mut pass = begin_present(
            encoder,
            target_view,
            "frust-engine copy present pass",
            timestamp_writes,
        );
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.instance.slice(..));
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..COPY_QUAD_VERTICES, 0..1);
    }

    /// This pass's shape as the downlevel lint reads it — one bind group, one
    /// instance buffer of four `u32` attributes, no uniform.
    #[must_use]
    pub fn layout_desc() -> PipelineLayoutDesc {
        let layout = copy_vertex_layout();
        PipelineLayoutDesc {
            bind_group_count: 1,
            max_vertex_buffers: 1,
            total_vertex_attributes: layout.attributes.len(),
            max_vertex_buffer_stride: layout.array_stride as usize,
            sample_count: 1,
            uniform_buffer_sizes: Vec::new(),
        }
    }

    /// The copy program drawn into the swapchain's format: its instance
    /// layout, the engine's strip topology, no blend (the pass replaces every
    /// pixel), no depth.
    fn desc(shader: ShaderId, format: wgpu::TextureFormat) -> RenderPipelineDesc {
        RenderPipelineDesc {
            vertex_layouts: vec![copy_vertex_layout()],
            topology: TOPOLOGY,
            ..RenderPipelineDesc::new(shader, VS_MAIN, FS_MAIN, format)
        }
    }
}

/// The present passes' one bind group: their source texture, at binding 0.
fn source_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source_view: &wgpu::TextureView,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(source_view),
        }],
    })
}

/// Begins a present pass over `target_view`, clearing it: the destination is a
/// fresh swapchain image whose prior contents mean nothing.
fn begin_present<'encoder>(
    encoder: &'encoder mut wgpu::CommandEncoder,
    target_view: &wgpu::TextureView,
    label: &str,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'encoder> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_present_layout_passes_the_downlevel_lint() {
        let violations = frust_gpu::lint_pipeline_layout(&CopyPresentPass::layout_desc());
        assert!(
            violations.is_empty(),
            "the copy present pass violates a downlevel design rule: {violations:?}"
        );
    }

    #[test]
    fn the_copy_present_program_is_the_engines_own_region_copy() {
        assert!(COPY.contains("fn vs_main"));
        assert!(COPY.contains("fn fs_main"));
        assert!(
            !COPY.contains("@compute"),
            "the copy is a render pass, never a compute one (E1)"
        );
        assert!(
            !COPY.contains("texture_storage_"),
            "a swapchain on this tier is RENDER_ATTACHMENT-only (E2)"
        );
    }

    /// The pass is selected by the swapchain's own alpha convention, and by
    /// nothing else.
    #[test]
    fn only_a_straight_alpha_target_selects_the_pass() {
        assert!(UnpremultiplyPass::selected_by(OutputAlpha::Straight));
        assert!(!UnpremultiplyPass::selected_by(OutputAlpha::Premultiplied));
    }

    /// The value the shader's own `ALPHA_FLOOR` declaration carries, read out
    /// of the source the GPU compiles rather than restated here.
    fn shader_alpha_floor() -> f32 {
        let declaration = UNPREMULTIPLY
            .lines()
            .find(|line| line.trim_start().starts_with("const ALPHA_FLOOR"))
            .expect("the shader declares an ALPHA_FLOOR constant");
        declaration
            .split('=')
            .nth(1)
            .expect("the declaration assigns a value")
            .trim()
            .trim_end_matches(';')
            .parse()
            .expect("the shader's alpha floor is a float literal")
    }

    /// The divisor guard is one decision spelled in two languages; a drift
    /// between them would surface only as NaN pixels on a device.
    #[test]
    fn the_shader_and_rust_alpha_floors_agree() {
        let floor = shader_alpha_floor();
        assert_eq!(
            floor, ALPHA_FLOOR,
            "the shader's floor must stay in step with `ALPHA_FLOOR`"
        );
        assert!(
            floor > 0.0,
            "a floor of zero would leave `0 / 0` NaNs in the swapchain"
        );
        assert!(
            floor < 1.0 / 255.0,
            "the floor must sit below the smallest representable 8-bit alpha, so it never clamps \
             a pixel that carries colour"
        );
    }

    /// The program the host compiles is the file the WGSL lint scans, not an
    /// inline string — the shader directory's own rule.
    #[test]
    fn the_program_is_loaded_from_the_shader_directory() {
        assert!(UNPREMULTIPLY.contains("fn vs_main"));
        assert!(UNPREMULTIPLY.contains("fn fs_main"));
        assert!(
            !UNPREMULTIPLY.contains("@compute"),
            "the conversion is a render pass, never a compute one (E1)"
        );
        assert!(
            !UNPREMULTIPLY.contains("texture_storage_"),
            "a swapchain on this tier is RENDER_ATTACHMENT-only (E2)"
        );
    }

    #[test]
    fn the_pass_layout_passes_the_downlevel_lint() {
        let violations = frust_gpu::lint_pipeline_layout(&UnpremultiplyPass::layout_desc());
        assert!(
            violations.is_empty(),
            "the present pass violates a downlevel design rule: {violations:?}"
        );
        assert_eq!(
            UnpremultiplyPass::layout_desc().bind_group_count,
            UNPREMULTIPLY_BIND_GROUPS,
            "one bind group: the source texture"
        );
    }
}
