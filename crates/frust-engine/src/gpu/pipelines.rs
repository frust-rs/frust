//! The eight render pipelines the engine draws with, as plain values.
//!
//! Six of them rasterize sparse strips out of the one `strip.wgsl` program,
//! differing only in their render state; the other two clear and copy
//! intermediate textures. Each is described by a
//! [`frust_gpu::RenderPipelineDesc`] — a value holding no GPU handle — so the
//! whole set can be listed at start-up and handed to
//! [`frust_gpu::PipelineCache::warm_up`] before the first frame asks for one.
//! That is the substrate's stated contract: a frame is never the first place a
//! pipeline gets compiled.
//!
//! ## The six strip variants
//!
//! | Variant | Target | Blend | Depth |
//! |---|---|---|---|
//! | [`EnginePipeline::StripIntermediate`] | [`INTERMEDIATE_FORMAT`] | premultiplied | none |
//! | [`EnginePipeline::StripAlpha`] | the frame's target format | premultiplied | none |
//! | [`EnginePipeline::StripDepthAlpha`] | the frame's target format | premultiplied | test, no write |
//! | [`EnginePipeline::StripOpaque`] | the frame's target format | none | test and write |
//! | [`EnginePipeline::StripDestOut`] | the frame's target format | [`DEST_OUT_BLEND`] | none |
//! | [`EnginePipeline::StripDepthDestOut`] | the frame's target format | [`DEST_OUT_BLEND`] | test, no write |
//!
//! The depth comparison is [`wgpu::CompareFunction::LessEqual`] against a
//! [`DEPTH_FORMAT`] attachment, which is what the vertex stage's own z
//! encoding expects: `strip.wgsl` maps painter's-order index 0 (the backmost
//! draw) to z = 1.0 and each draw in front of it to a smaller z, so drawing
//! front-to-back lets the opaque pass reject everything already covered.
//!
//! ## The destination-out pair
//!
//! The hole punch [`crate::compile::clear`] lowers is a strip run like any
//! other — same program, same instance layout — and differs only in its blend
//! state, which is why it is a pipeline variant rather than a shader of its
//! own. [`DEST_OUT_BLEND`] computes `dst · (1 − src.a)` in fixed function, for
//! the colour *and* the alpha component, which is what erases a rectangle to
//! `(0, 0, 0, 0)` without a shader-side composite reading a target it is also
//! writing.
//!
//! There are two of them for the same reason there are two alpha variants: a
//! pipeline's depth state has to agree with whether the pass it runs in
//! attaches a depth buffer at all, and the engine's depth attachment is
//! optional (no attachment, or `FRUST_ENGINE_NO_DEPTH`). The depth-testing one
//! is what restores the paint order the display list recorded; the other is
//! the same trade the two draw passes already make when they collapse into one.
//!
//! ## Bind groups
//!
//! [`frust_gpu::RenderPipelineDesc`] has no explicit-layout axis: every
//! pipeline uses wgpu's default layout, derived from the shader module. The
//! group structure is therefore whatever the WGSL declares, and it matches the
//! reference renderer's explicit layouts group for group — strip programs bind
//! groups 0..3 (alphas + config + layer input; atlas array + external texture;
//! encoded paints; gradient LUT), which is the WebGL2 ceiling of four exactly,
//! with no headroom. [`EnginePipeline::layout_desc`] restates each pipeline's
//! shape as the value `frust_gpu::lint::lint_pipeline_layout` checks, so that
//! ceiling is a test rather than a comment.

use frust_gpu::lint::PipelineLayoutDesc;
use frust_gpu::{PipelineCache, RenderPipelineDesc, ShaderId, ShaderLibrary, VertexLayout};

use super::GpuStrip;
use super::config::GpuConfig;
use super::shader_src;

/// The format every intermediate (off-screen) strip target uses.
///
/// Fixed rather than negotiated: an intermediate exists only to be sampled or
/// copied by a later pass in the same frame, so it never has to agree with a
/// surface's own format.
pub const INTERMEDIATE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The fixed-function destination-out blend the punch pass draws with.
///
/// `src · 0 + dst · (1 − src.a)`, applied to the colour *and* the alpha
/// component — the `COMPOSE_DEST_OUT` arm of `shaders/blend.wgsl` evaluated for
/// a premultiplied source, without the shader-side composite that arm would
/// need a readable backdrop for. See [`crate::compile::clear`]'s module doc for
/// why the erase is a destination-out composite rather than a clear op.
pub const DEST_OUT_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: DEST_OUT_COMPONENT,
    alpha: DEST_OUT_COMPONENT,
};

/// The one blend component [`DEST_OUT_BLEND`] applies to both channels.
const DEST_OUT_COMPONENT: wgpu::BlendComponent = wgpu::BlendComponent {
    src_factor: wgpu::BlendFactor::Zero,
    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
    operation: wgpu::BlendOperation::Add,
};

/// The depth attachment format the depth-testing strip variants use.
///
/// 24 bits is what the vertex stage's z encoding is quantized to (it divides
/// the painter's-order index by `1 << 24`), so a wider format would buy no
/// extra ordering resolution.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

/// Every strip instance expands to one quad, so every pipeline here draws a
/// four-vertex strip with no index buffer.
pub const TOPOLOGY: wgpu::PrimitiveTopology = wgpu::PrimitiveTopology::TriangleStrip;

/// Vertex entry point of the strip, clear (region) and copy programs alike.
pub const VS_MAIN: &str = "vs_main";

/// Vertex entry point of the scissor-driven atlas clear.
pub const VS_MAIN_FULLSCREEN: &str = "vs_main_fullscreen";

/// Fragment entry point of every program here.
pub const FS_MAIN: &str = "fs_main";

/// Bind groups the strip programs declare: alphas/config/layer input, atlas
/// array/external texture, encoded paints, gradient LUT.
const STRIP_BIND_GROUPS: usize = 4;

/// Bind groups the copy program declares: its source texture.
const COPY_BIND_GROUPS: usize = 1;

/// The engine's compiled shader modules, by id.
///
/// Registered once at start-up into the [`ShaderLibrary`] a
/// [`PipelineCache`] is built over; a [`RenderPipelineDesc`] then names its
/// program by id rather than by source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineShaders {
    /// The sparse-strip rasterizer, shared by all four strip variants.
    pub strip: ShaderId,
    /// Region and fullscreen clears.
    pub clear: ShaderId,
    /// Rectangular region copies.
    pub copy: ShaderId,
}

impl EngineShaders {
    /// Compiles every module in [`shader_src::MODULES`] into `library` and
    /// returns their ids.
    ///
    /// Insertion is idempotent per name, so calling this twice against the
    /// same library returns the same ids without recompiling.
    #[must_use]
    pub fn register(library: &mut ShaderLibrary, device: &wgpu::Device) -> Self {
        Self {
            strip: library.insert_wgsl(device, shader_src::STRIP_NAME, shader_src::STRIP),
            clear: library.insert_wgsl(device, shader_src::CLEAR_NAME, shader_src::CLEAR),
            copy: library.insert_wgsl(device, shader_src::COPY_NAME, shader_src::COPY),
        }
    }
}

/// Which of the three registered modules a pipeline draws with.
///
/// The module identity a [`RenderPipelineDesc`] carries is a [`ShaderId`],
/// which only a [`ShaderLibrary`] can mint. This names the same distinction
/// without a device in the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EngineShaderModule {
    /// `strip.wgsl` — the sparse-strip rasterizer.
    Strip,
    /// `clear.wgsl` — region and fullscreen clears.
    Clear,
    /// `copy.wgsl` — rectangular region copies.
    Copy,
}

/// One of the engine's render pipelines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EnginePipeline {
    /// Strips into an [`INTERMEDIATE_FORMAT`] off-screen target.
    StripIntermediate,
    /// Strips into the frame's own target, alpha-blended, no depth.
    StripAlpha,
    /// Strips into the frame's own target, alpha-blended, depth-tested but
    /// not depth-writing.
    StripDepthAlpha,
    /// Strips into the frame's own target, opaque, depth-tested and
    /// depth-writing.
    StripOpaque,
    /// Hole-punch coverage into the frame's own target, destination-out, no
    /// depth.
    StripDestOut,
    /// Hole-punch coverage into the frame's own target, destination-out,
    /// depth-tested but not depth-writing.
    StripDepthDestOut,
    /// Clears rectangular regions of an intermediate target.
    Clear,
    /// Copies rectangular regions between intermediate targets.
    Copy,
}

impl EnginePipeline {
    /// Every pipeline the engine warms up, in warm-up order.
    pub const ALL: [Self; 8] = [
        Self::StripIntermediate,
        Self::StripAlpha,
        Self::StripDepthAlpha,
        Self::StripOpaque,
        Self::StripDestOut,
        Self::StripDepthDestOut,
        Self::Clear,
        Self::Copy,
    ];

    /// A human-readable name for logs and captures.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::StripIntermediate => "strip-intermediate",
            Self::StripAlpha => "strip-alpha",
            Self::StripDepthAlpha => "strip-depth-alpha",
            Self::StripOpaque => "strip-opaque",
            Self::StripDestOut => "strip-dest-out",
            Self::StripDepthDestOut => "strip-depth-dest-out",
            Self::Clear => "clear",
            Self::Copy => "copy",
        }
    }

    /// Whether this pipeline rasterizes strips (as opposed to clearing or
    /// copying an intermediate).
    #[must_use]
    pub const fn is_strip(self) -> bool {
        matches!(
            self,
            Self::StripIntermediate
                | Self::StripAlpha
                | Self::StripDepthAlpha
                | Self::StripOpaque
                | Self::StripDestOut
                | Self::StripDepthDestOut
        )
    }

    /// How many bind groups the pipeline's program declares.
    ///
    /// Four is the WebGL2 ceiling, which the strip programs sit exactly on.
    #[must_use]
    pub const fn bind_group_count(self) -> usize {
        match self {
            Self::StripIntermediate
            | Self::StripAlpha
            | Self::StripDepthAlpha
            | Self::StripOpaque
            | Self::StripDestOut
            | Self::StripDepthDestOut => STRIP_BIND_GROUPS,
            Self::Clear => 0,
            Self::Copy => COPY_BIND_GROUPS,
        }
    }

    /// The color format this pipeline writes, given the frame's target
    /// format.
    ///
    /// Only the variants that draw into the frame's own target take it; the
    /// rest are pinned to [`INTERMEDIATE_FORMAT`].
    #[must_use]
    pub const fn format(self, target_format: wgpu::TextureFormat) -> wgpu::TextureFormat {
        match self {
            Self::StripAlpha
            | Self::StripDepthAlpha
            | Self::StripOpaque
            | Self::StripDestOut
            | Self::StripDepthDestOut => target_format,
            Self::StripIntermediate | Self::Clear | Self::Copy => INTERMEDIATE_FORMAT,
        }
    }

    /// The color blending this pipeline uses, or `None` for an opaque write.
    #[must_use]
    pub const fn blend(self) -> Option<wgpu::BlendState> {
        match self {
            Self::StripIntermediate | Self::StripAlpha | Self::StripDepthAlpha => {
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING)
            }
            Self::StripDestOut | Self::StripDepthDestOut => Some(DEST_OUT_BLEND),
            Self::StripOpaque | Self::Clear | Self::Copy => None,
        }
    }

    /// The depth state this pipeline uses, or `None` for a color-only pass.
    #[must_use]
    pub fn depth(self) -> Option<wgpu::DepthStencilState> {
        match self {
            Self::StripDepthAlpha | Self::StripDepthDestOut => Some(depth_state(false)),
            Self::StripOpaque => Some(depth_state(true)),
            Self::StripIntermediate
            | Self::StripAlpha
            | Self::StripDestOut
            | Self::Clear
            | Self::Copy => None,
        }
    }

    /// The vertex buffer layouts this pipeline steps over.
    #[must_use]
    pub fn vertex_layouts(self) -> Vec<VertexLayout> {
        match self {
            Self::StripIntermediate
            | Self::StripAlpha
            | Self::StripDepthAlpha
            | Self::StripOpaque
            | Self::StripDestOut
            | Self::StripDepthDestOut => vec![GpuStrip::vertex_layout()],
            Self::Clear => vec![clear_vertex_layout()],
            Self::Copy => vec![copy_vertex_layout()],
        }
    }

    /// Which module this pipeline draws with.
    #[must_use]
    pub const fn module(self) -> EngineShaderModule {
        match self {
            Self::StripIntermediate
            | Self::StripAlpha
            | Self::StripDepthAlpha
            | Self::StripOpaque
            | Self::StripDestOut
            | Self::StripDepthDestOut => EngineShaderModule::Strip,
            Self::Clear => EngineShaderModule::Clear,
            Self::Copy => EngineShaderModule::Copy,
        }
    }

    /// The registered id of the module this pipeline draws with.
    #[must_use]
    pub const fn shader(self, shaders: &EngineShaders) -> ShaderId {
        match self.module() {
            EngineShaderModule::Strip => shaders.strip,
            EngineShaderModule::Clear => shaders.clear,
            EngineShaderModule::Copy => shaders.copy,
        }
    }

    /// The full pipeline description, ready for
    /// [`PipelineCache::get_or_create`] or [`PipelineCache::warm_up`].
    ///
    /// `sample_count` is always 1: the engine never multisamples, since a
    /// downlevel target cannot.
    #[must_use]
    pub fn desc(
        self,
        shaders: &EngineShaders,
        target_format: wgpu::TextureFormat,
    ) -> RenderPipelineDesc {
        RenderPipelineDesc {
            shader: self.shader(shaders),
            vs: VS_MAIN.into(),
            fs: FS_MAIN.into(),
            vertex_layouts: self.vertex_layouts(),
            blend: self.blend(),
            format: self.format(target_format),
            sample_count: 1,
            depth: self.depth(),
            topology: TOPOLOGY,
        }
    }

    /// This pipeline's shape as the downlevel lint reads it, so the
    /// bind-group, vertex-topology and sample-count ceilings
    /// (`frust_gpu::lint::lint_pipeline_layout`) are checked against the real
    /// descriptions rather than restated by hand.
    #[must_use]
    pub fn layout_desc(self) -> PipelineLayoutDesc {
        let layouts = self.vertex_layouts();
        PipelineLayoutDesc {
            bind_group_count: self.bind_group_count(),
            max_vertex_buffers: layouts.len(),
            total_vertex_attributes: layouts.iter().map(|l| l.attributes.len()).sum(),
            max_vertex_buffer_stride: layouts
                .iter()
                .map(|l| l.array_stride as usize)
                .max()
                .unwrap_or(0),
            sample_count: 1,
            uniform_buffer_sizes: if self.is_strip() {
                vec![GpuConfig::SIZE as usize]
            } else {
                Vec::new()
            },
        }
    }
}

/// The depth state shared by the two depth-testing strip variants.
///
/// `depth_write_enabled` is the only axis that differs: the opaque pass writes
/// the depth it establishes, the alpha pass only tests against it.
fn depth_state(depth_write_enabled: bool) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(depth_write_enabled),
        depth_compare: Some(wgpu::CompareFunction::LessEqual),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

/// The clear program's instance layout: origin, size and target size, each a
/// pair of `u32`s, 24 bytes per instance.
#[must_use]
pub fn clear_vertex_layout() -> VertexLayout {
    VertexLayout {
        array_stride: 24,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: wgpu::vertex_attr_array![
            0 => Uint32x2,
            1 => Uint32x2,
            2 => Uint32x2,
        ]
        .to_vec(),
    }
}

/// The copy program's instance layout: destination origin, source origin,
/// region size and destination size, each a `u16` pair packed into one `u32`,
/// 16 bytes per instance.
#[must_use]
pub fn copy_vertex_layout() -> VertexLayout {
    VertexLayout {
        array_stride: 16,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: wgpu::vertex_attr_array![
            0 => Uint32,
            1 => Uint32,
            2 => Uint32,
            3 => Uint32,
        ]
        .to_vec(),
    }
}

/// The full warm-up list: every pipeline in [`EnginePipeline::ALL`],
/// described against `target_format`.
#[must_use]
pub fn warm_up_descs(
    shaders: &EngineShaders,
    target_format: wgpu::TextureFormat,
) -> Vec<RenderPipelineDesc> {
    EnginePipeline::ALL
        .iter()
        .map(|pipeline| pipeline.desc(shaders, target_format))
        .collect()
}

/// Queues every engine pipeline for background compilation.
///
/// Call it once the device and the frame's target format are known. It
/// returns immediately; a variant the render thread needs before the worker
/// reaches it is stolen out of the queue and built inline rather than waited
/// on.
pub fn warm_up(
    cache: &mut PipelineCache,
    device: &wgpu::Device,
    shaders: &EngineShaders,
    target_format: wgpu::TextureFormat,
) {
    cache.warm_up(device, &warm_up_descs(shaders, target_format));
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

    /// Everything about a pipeline that makes it a distinct variant, minus
    /// the [`ShaderId`] — which only a [`ShaderLibrary`] can mint, and which
    /// therefore needs a device. Two pipelines that agree on all of this
    /// would compile to the same object.
    fn variant(
        pipeline: EnginePipeline,
    ) -> (
        EngineShaderModule,
        wgpu::TextureFormat,
        Option<wgpu::BlendState>,
        Option<wgpu::DepthStencilState>,
        Vec<VertexLayout>,
    ) {
        (
            pipeline.module(),
            pipeline.format(TARGET),
            pipeline.blend(),
            pipeline.depth(),
            pipeline.vertex_layouts(),
        )
    }

    #[test]
    fn the_warm_up_list_covers_every_pipeline_exactly_once() {
        assert_eq!(EnginePipeline::ALL.len(), 8);
        for (i, a) in EnginePipeline::ALL.iter().enumerate() {
            for b in EnginePipeline::ALL.iter().skip(i + 1) {
                assert_ne!(a, b, "the warm-up list repeats {}", a.label());
                assert_ne!(
                    variant(*a),
                    variant(*b),
                    "{} and {} describe the same pipeline, so one would never be reached",
                    a.label(),
                    b.label()
                );
            }
        }
    }

    #[test]
    fn every_pipeline_layout_passes_the_downlevel_lint() {
        for pipeline in EnginePipeline::ALL {
            let violations = frust_gpu::lint_pipeline_layout(&pipeline.layout_desc());
            assert!(
                violations.is_empty(),
                "{} violates a downlevel design rule: {violations:?}",
                pipeline.label()
            );
        }
    }

    #[test]
    fn the_strip_pipelines_sit_exactly_on_the_bind_group_ceiling() {
        let ceiling = wgpu::Limits::downlevel_webgl2_defaults().max_bind_groups as usize;
        for pipeline in EnginePipeline::ALL {
            assert!(
                pipeline.bind_group_count() <= ceiling,
                "{} declares {} bind groups, over the WebGL2 ceiling of {ceiling}",
                pipeline.label(),
                pipeline.bind_group_count()
            );
        }
        assert_eq!(
            EnginePipeline::StripAlpha.bind_group_count(),
            ceiling,
            "the strip programs bind the ceiling exactly, with no headroom left"
        );
    }

    #[test]
    fn no_pipeline_multisamples() {
        for pipeline in EnginePipeline::ALL {
            assert_eq!(
                pipeline.layout_desc().sample_count,
                1,
                "{} multisamples",
                pipeline.label()
            );
        }
    }

    #[test]
    fn the_strip_variants_differ_only_in_target_blend_and_depth() {
        let strips = [
            EnginePipeline::StripIntermediate,
            EnginePipeline::StripAlpha,
            EnginePipeline::StripDepthAlpha,
            EnginePipeline::StripOpaque,
            EnginePipeline::StripDestOut,
            EnginePipeline::StripDepthDestOut,
        ];
        for pipeline in strips {
            assert_eq!(pipeline.module(), EngineShaderModule::Strip);
            assert_eq!(pipeline.vertex_layouts(), vec![GpuStrip::vertex_layout()]);
            assert_eq!(pipeline.bind_group_count(), STRIP_BIND_GROUPS);
        }

        assert_eq!(
            EnginePipeline::StripIntermediate.format(TARGET),
            INTERMEDIATE_FORMAT
        );
        assert_eq!(EnginePipeline::StripAlpha.format(TARGET), TARGET);

        let premultiplied = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        assert_eq!(EnginePipeline::StripIntermediate.blend(), premultiplied);
        assert_eq!(EnginePipeline::StripAlpha.blend(), premultiplied);
        assert_eq!(EnginePipeline::StripDepthAlpha.blend(), premultiplied);
        assert_eq!(
            EnginePipeline::StripOpaque.blend(),
            None,
            "the opaque variant must not blend"
        );

        assert_eq!(EnginePipeline::StripIntermediate.depth(), None);
        assert_eq!(EnginePipeline::StripAlpha.depth(), None);
        assert_eq!(
            EnginePipeline::StripDepthAlpha
                .depth()
                .and_then(|d| d.depth_write_enabled),
            Some(false),
            "the depth-alpha variant tests depth without writing it"
        );
        assert_eq!(
            EnginePipeline::StripOpaque
                .depth()
                .and_then(|d| d.depth_write_enabled),
            Some(true),
            "the opaque variant establishes the depth later passes test against"
        );
        for pipeline in [
            EnginePipeline::StripDepthAlpha,
            EnginePipeline::StripOpaque,
            EnginePipeline::StripDepthDestOut,
        ] {
            let depth = pipeline.depth().expect("a depth-testing variant");
            assert_eq!(depth.format, DEPTH_FORMAT);
            assert_eq!(depth.depth_compare, Some(wgpu::CompareFunction::LessEqual));
        }
    }

    /// The punch pair erases `dst · (1 − src.a)` in fixed function, colour and
    /// alpha alike, and differs from the alpha pair in nothing but that blend
    /// and its depth state.
    ///
    /// Stated as arithmetic over the blend factors rather than as a comparison
    /// against a named `wgpu` preset: `wgpu` has no destination-out preset, and
    /// a punch that darkened colour without erasing alpha (or the reverse)
    /// would leave exactly the residue `compile::clear`'s contract forbids.
    #[test]
    fn the_dest_out_variants_erase_colour_and_alpha_alike() {
        for pipeline in [
            EnginePipeline::StripDestOut,
            EnginePipeline::StripDepthDestOut,
        ] {
            let blend = pipeline.blend().expect("a destination-out variant blends");
            assert_eq!(blend, DEST_OUT_BLEND);
            assert_eq!(
                blend.color,
                blend.alpha,
                "{} must erase alpha exactly as it erases colour",
                pipeline.label()
            );
            for component in [blend.color, blend.alpha] {
                assert_eq!(component.src_factor, wgpu::BlendFactor::Zero);
                assert_eq!(
                    component.dst_factor,
                    wgpu::BlendFactor::OneMinusSrcAlpha,
                    "{} must weight the destination by the source's own coverage",
                    pipeline.label()
                );
                assert_eq!(component.operation, wgpu::BlendOperation::Add);
            }
            assert_eq!(pipeline.format(TARGET), TARGET, "the punch is a frame pass");
            assert_eq!(pipeline.bind_group_count(), STRIP_BIND_GROUPS);
        }

        assert_eq!(
            EnginePipeline::StripDestOut.depth(),
            None,
            "the depth-free punch runs when the frame has no depth attachment"
        );
        assert_eq!(
            EnginePipeline::StripDepthDestOut
                .depth()
                .and_then(|d| d.depth_write_enabled),
            Some(false),
            "a punch tests the depth the opaque pass established without writing it"
        );
    }

    #[test]
    fn the_clear_and_copy_pipelines_write_the_intermediate_format_unblended() {
        for pipeline in [EnginePipeline::Clear, EnginePipeline::Copy] {
            assert_eq!(pipeline.format(TARGET), INTERMEDIATE_FORMAT);
            assert_eq!(pipeline.blend(), None);
            assert_eq!(pipeline.depth(), None);
            assert!(!pipeline.is_strip());
        }
        assert_eq!(EnginePipeline::Clear.bind_group_count(), 0);
        assert_eq!(EnginePipeline::Copy.bind_group_count(), COPY_BIND_GROUPS);
    }

    #[test]
    fn the_instance_layouts_match_their_shader_declarations() {
        let strip = GpuStrip::vertex_layout();
        assert_eq!(strip.step_mode, wgpu::VertexStepMode::Instance);
        assert_eq!(strip.attributes.len(), 6);

        let clear = clear_vertex_layout();
        assert_eq!(clear.step_mode, wgpu::VertexStepMode::Instance);
        assert_eq!(clear.attributes.len(), 3);
        assert_eq!(clear.array_stride, 24);

        let copy = copy_vertex_layout();
        assert_eq!(copy.step_mode, wgpu::VertexStepMode::Instance);
        assert_eq!(copy.attributes.len(), 4);
        assert_eq!(copy.array_stride, 16);
    }

    /// Blocks on `future` by polling it to completion.
    ///
    /// wgpu's native adapter and device requests resolve without an executor
    /// driving them, so a bare poll loop is enough here; this crate has no
    /// async runtime of its own and the only caller is the ignored
    /// hardware test below.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Waker};

        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    /// Pops a validation error scope, pumping the device until the pop
    /// resolves — the pop is a future a plain poll loop would park on.
    fn drain_error_scope(
        device: &wgpu::Device,
        scope: wgpu::ErrorScopeGuard,
    ) -> Option<wgpu::Error> {
        use std::task::{Context, Poll, Waker};

        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut future = std::pin::pin!(scope.pop());
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(error) => return error,
                Poll::Pending => {
                    let _ = device.poll(wgpu::PollType::wait_indefinitely());
                }
            }
        }
    }

    /// Every engine pipeline, built on real hardware.
    ///
    /// The tests above are all device-free, and `shader_src`'s naga tests
    /// prove the modules themselves validate. This is the only case that
    /// proves the *pipelines* are accepted — entry points, derived
    /// bind-group layouts, instance layouts, color targets and depth state
    /// included — and that warming the list up compiles each of them exactly
    /// once.
    #[test]
    #[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine -- --ignored`"]
    fn every_pipeline_builds_on_a_real_device() {
        let (device, _queue) = block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            // The environment-aware initializer, so `WGPU_ADAPTER_NAME` picks
            // the GPU on a multi-adapter host instead of the run silently
            // landing on whichever one enumerates first.
            let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
                .await
                .expect("no compatible GPU adapter");
            println!(
                "frust-engine pipeline test adapter: {:?}",
                adapter.get_info()
            );
            adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-engine pipeline test device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create the device")
        });

        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let mut library = ShaderLibrary::new();
        let shaders = EngineShaders::register(&mut library, &device);
        assert_eq!(library.len(), 3, "one module per engine shader source");
        assert_eq!(
            EnginePipeline::ALL
                .iter()
                .filter(|pipeline| pipeline.is_strip())
                .count(),
            6,
            "six render states over the one strip program"
        );

        let mut cache = PipelineCache::new(std::sync::Arc::new(library), None);
        let descs = warm_up_descs(&shaders, TARGET);
        assert_eq!(descs.len(), EnginePipeline::ALL.len());
        for desc in &descs {
            let _pipeline = cache.get_or_create(&device, desc);
        }
        // A second pass must be pure cache hits.
        for desc in &descs {
            let _pipeline = cache.get_or_create(&device, desc);
        }

        let error = drain_error_scope(&device, scope);
        assert!(error.is_none(), "pipeline creation raised {error:?}");
        assert_eq!(
            cache.compiled_variants(),
            EnginePipeline::ALL.len() as u64,
            "each engine pipeline must compile exactly once"
        );
    }
}
