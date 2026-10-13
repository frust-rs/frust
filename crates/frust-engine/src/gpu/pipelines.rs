//! The engine's render pipelines, as plain values.
//!
//! Nine render states ([`EnginePipeline::ALL`]): six of them rasterize sparse
//! strips out of the one `strip.wgsl` source, differing only in their render
//! state; two more clear and copy intermediate textures, and the last runs one
//! pass of a layer [filter](crate::filters). Each strip state is further
//! specialised along a second axis, the [`PaintClass`] — which program the
//! strip source is compiled to (see *The paint-class axis* below). Every
//! variant is described by a [`frust_gpu::RenderPipelineDesc`], a value whose
//! only GPU handle is the shared strip layout, so the set the first frames are
//! known to need can be listed at start-up and handed to
//! [`frust_gpu::PipelineCache::warm_up`] before the first frame asks for one.
//!
//! ## The paint-class axis
//!
//! The strip fragment stage's colour-source dispatch costs every fragment the
//! branches it never takes (the S1 ablation on an Adreno 660: a program
//! keeping only coverage, solid and gradients shaded S1 in under half the time
//! of the whole one). So each strip state is compiled once per class it
//! [serves](EnginePipeline::serves), from the class's specialisation of the one
//! source ([`shader_src::strip_class_source`]), and a pass sets the pipeline of
//! each run's class in turn (`renderer`'s class runs). [`PaintClass::Uber`] is
//! the whole program, and the only one `FRUST_ENGINE_UBER_PROGRAM` uses.
//!
//! The axis is folded rather than multiplied out: the destination-out pair
//! only ever draws the punch's solid source, and the opaque state never draws
//! a composite or a blurred rectangle (neither is ever opaque), so
//!
//! | State | Classes served | Variants |
//! |---|---|---|
//! | [`EnginePipeline::StripIntermediate`] | all seven | 7 |
//! | [`EnginePipeline::StripAlpha`] | all seven | 7 |
//! | [`EnginePipeline::StripDepthAlpha`] | all seven | 7 |
//! | [`EnginePipeline::StripOpaque`] | solid, gradient, image, image-filtered, Uber | 5 |
//! | [`EnginePipeline::StripDestOut`] | solid, Uber | 2 |
//! | [`EnginePipeline::StripDepthDestOut`] | solid, Uber | 2 |
//! | clear, copy, filter | their own program | 3 |
//!
//! — [`VARIANT_COUNT`], 33 at most. A run of a class its state does not serve
//! is drawn with Uber ([`EnginePipeline::fold_class`]), which draws everything.
//! Variants are created lazily, on the first frame that draws a run of that
//! class through that state; [`warm_up_descs`] lists the nine Uber states plus
//! [`WARM_CLASSES`] across [`WARM_CLASS_STATES`] — the solid, gradient and image
//! programs of the two depth-tested surface states, which is what an ordinary
//! frame draws through — so steady state compiles nothing.
//!
//! Every strip variant, Uber included, is built against one explicit pipeline
//! layout ([`crate::gpu::bindings::strip_pipeline_layout`]) instead of the
//! layout wgpu derives per module: a class that stops reading a binding would
//! otherwise derive a different layout, and the renderer's bind groups would
//! stop fitting it.
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
//! ## The atlas-layer target
//!
//! A pass that rasterizes glyph outlines into one layer of the atlas array
//! draws through [`ATLAS_STRIP_PIPELINE`], which is
//! [`EnginePipeline::StripIntermediate`] itself rather than a variant of its
//! own. An atlas layer is an [`crate::gpu::atlas::ATLAS_FORMAT`] colour target
//! with no depth attachment, composited source-over so a COLR glyph's layers
//! and a re-used slot's clear stack in the order they were recorded — which is
//! the render state `StripIntermediate` already *is*, field for field. A second
//! enum arm carrying identical state would compile a second identical pipeline
//! object, and would break
//! `the_warm_up_list_covers_every_pipeline_exactly_once`, whose whole point is
//! that no two arms describe the same pipeline. What the atlas target gets
//! instead is a name and [`the_atlas_target_reuses_the_intermediate_variant`],
//! which pins the format equality the reuse rests on: if either format is ever
//! moved, that test fails rather than glyphs quietly rendering through a
//! pipeline whose colour target no longer matches its attachment.
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
//! ## The filter pass
//!
//! [`EnginePipeline::Filter`] is a genuinely new pipeline rather than a second
//! name for an existing one: a different program (`filter.wgsl`), a different
//! instance layout ([`filter_vertex_layout`], eight words per instance against
//! the strip's six), and a different bind-group shape. It writes
//! [`INTERMEDIATE_FORMAT`] unblended and with no depth, because a filter pass
//! *replaces* the region it writes rather than compositing onto it — the
//! scheduler clears the destination page ahead of every filter round, and the
//! quad overdraws a transparent padding border around the region for the same
//! reason (see [`crate::schedule`]'s *Filter rounds*).
//!
//! It is also where the engine's **first sampler** appears. Every other
//! pipeline reads its textures with `textureLoad` at integer coordinates; the
//! blur kernels sample bilinearly at fractional offsets, which is what lets a
//! decimation cost four samples instead of sixteen and a convolution tap one
//! instead of two. The sampler itself is [`crate::gpu::targets::filter_sampler`]
//! — the layout here only has to know that group 1 carries a texture *and* a
//! sampler.
//!
//! ## Bind groups
//!
//! The clear, copy and filter pipelines use wgpu's default layout, derived
//! from their shader module; the strip pipelines use the explicit layout above,
//! which is exactly the one the whole strip program derives. The group
//! structure is therefore whatever the WGSL declares, and it matches the
//! reference renderer's explicit layouts group for group — strip programs bind
//! groups 0..3 (alphas + config + layer input; atlas array + external texture;
//! encoded paints; gradient LUT), which is the WebGL2 ceiling of four exactly,
//! with no headroom. The filter program binds two (the filter-data texture;
//! the source page plus its sampler), well inside it.
//! [`EnginePipeline::layout_desc`] restates each pipeline's shape as the value
//! `frust_gpu::lint::lint_pipeline_layout` checks, so that ceiling is a test
//! rather than a comment.

use frust_gpu::lint::PipelineLayoutDesc;
use frust_gpu::{PipelineCache, RenderPipelineDesc, ShaderId, ShaderLibrary, VertexLayout};

use super::GpuStrip;
use super::bindings::{STRIP_BIND_GROUP_COUNT, strip_pipeline_layout};
use super::config::GpuConfig;
use super::shader_src;
use super::strips::PaintClass;

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
const STRIP_BIND_GROUPS: usize = STRIP_BIND_GROUP_COUNT;

/// The classes whose programs are warmed up ahead of the first frame, beside
/// the nine Uber states: the solid, gradient and image programs — what every
/// ordinary frame's surface passes draw.
pub const WARM_CLASSES: [PaintClass; 3] = [
    PaintClass::SolidCoverage,
    PaintClass::Gradient,
    PaintClass::Image,
];

/// The strip states [`WARM_CLASSES`] are warmed up across: the two
/// depth-tested surface states every frame with a depth attachment draws
/// through.
pub const WARM_CLASS_STATES: [EnginePipeline; 2] =
    [EnginePipeline::StripOpaque, EnginePipeline::StripDepthAlpha];

/// Every distinct pipeline the engine can create for one target format: each
/// strip state times the classes it serves, plus the clear, copy and filter
/// pipelines. See the module doc's table.
pub const VARIANT_COUNT: usize = {
    let mut count = 0;
    let mut state = 0;
    while state < EnginePipeline::ALL.len() {
        let pipeline = EnginePipeline::ALL[state];
        if pipeline.is_strip() {
            let mut class = 0;
            while class < PaintClass::ALL.len() {
                if pipeline.serves(PaintClass::ALL[class]) {
                    count += 1;
                }
                class += 1;
            }
        } else {
            count += 1;
        }
        state += 1;
    }
    count
};

/// Bind groups the copy program declares: its source texture.
const COPY_BIND_GROUPS: usize = 1;

/// Bind groups the filter program declares: the filter-data texture, then the
/// source page together with the sampler its kernels read it through.
const FILTER_BIND_GROUPS: usize = 2;

/// The engine's compiled shader modules, by id, and the one explicit layout
/// every strip pipeline is built against.
///
/// Registered once at start-up into the [`ShaderLibrary`] a
/// [`PipelineCache`] is built over; a [`RenderPipelineDesc`] then names its
/// program by id rather than by source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineShaders {
    /// The whole sparse-strip rasterizer — the [`PaintClass::Uber`] program,
    /// shared by all six strip states.
    pub strip: ShaderId,
    /// The strip program's specialisations, one per
    /// [`PaintClass::SPECIALISED`] entry, in that order.
    pub strip_classes: [ShaderId; PaintClass::SPECIALISED.len()],
    /// The explicit layout every strip pipeline, of every class, is built
    /// against ([`crate::gpu::bindings::strip_pipeline_layout`]).
    pub strip_layout: wgpu::PipelineLayout,
    /// Region and fullscreen clears.
    pub clear: ShaderId,
    /// Rectangular region copies.
    pub copy: ShaderId,
    /// One pass of a layer filter's sequence.
    pub filter: ShaderId,
}

impl EngineShaders {
    /// Compiles every module in [`shader_src::MODULES`], and the strip
    /// program's class specialisations, into `library`, returning their ids
    /// beside a freshly created strip layout.
    ///
    /// Insertion is idempotent per name, so calling this twice against the
    /// same library returns the same ids without recompiling (the layout is
    /// a new object each call — a desc compares layouts by identity, so keep
    /// one `EngineShaders` per cache).
    #[must_use]
    pub fn register(library: &mut ShaderLibrary, device: &wgpu::Device) -> Self {
        Self {
            strip: library.insert_wgsl(device, shader_src::STRIP_NAME, shader_src::STRIP),
            strip_classes: PaintClass::SPECIALISED.map(|class| {
                library.insert_wgsl(
                    device,
                    shader_src::strip_class_name(class),
                    &shader_src::strip_class_source(class),
                )
            }),
            strip_layout: strip_pipeline_layout(device),
            clear: library.insert_wgsl(device, shader_src::CLEAR_NAME, shader_src::CLEAR),
            copy: library.insert_wgsl(device, shader_src::COPY_NAME, shader_src::COPY),
            filter: library.insert_wgsl(device, shader_src::FILTER_NAME, shader_src::FILTER),
        }
    }

    /// The registered id of the strip program specialised for `class`.
    #[must_use]
    pub fn strip_class(&self, class: PaintClass) -> ShaderId {
        PaintClass::SPECIALISED
            .iter()
            .position(|specialised| *specialised == class)
            .and_then(|index| self.strip_classes.get(index))
            .copied()
            .unwrap_or(self.strip)
    }
}

/// Which of the four registered modules a pipeline draws with.
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
    /// `filter.wgsl` — one pass of a layer filter's sequence.
    Filter,
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
    /// Runs one pass of a layer filter's sequence into an intermediate target,
    /// reading the other page of the pair through a bilinear sampler.
    Filter,
}

impl EnginePipeline {
    /// Every pipeline the engine warms up, in warm-up order.
    pub const ALL: [Self; 9] = [
        Self::StripIntermediate,
        Self::StripAlpha,
        Self::StripDepthAlpha,
        Self::StripOpaque,
        Self::StripDestOut,
        Self::StripDepthDestOut,
        Self::Clear,
        Self::Copy,
        Self::Filter,
    ];

    /// This pipeline's position in [`Self::ALL`] — a dense index for tables
    /// keyed by render state.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::StripIntermediate => 0,
            Self::StripAlpha => 1,
            Self::StripDepthAlpha => 2,
            Self::StripOpaque => 3,
            Self::StripDestOut => 4,
            Self::StripDepthDestOut => 5,
            Self::Clear => 6,
            Self::Copy => 7,
            Self::Filter => 8,
        }
    }

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
            Self::Filter => "filter",
        }
    }

    /// Whether this strip state is ever compiled with `class`'s program.
    ///
    /// Every strip state serves [`PaintClass::Uber`] and
    /// [`PaintClass::SolidCoverage`]; the destination-out pair serves nothing
    /// else (a punch erases with a solid source), and the opaque state serves
    /// the classes a fully covered opaque span can have — solid, gradient,
    /// either image class — but never a composite or a blurred rectangle,
    /// neither of which is ever opaque. A non-strip pipeline serves only Uber, which for it
    /// means "its own program".
    #[must_use]
    pub const fn serves(self, class: PaintClass) -> bool {
        match class {
            PaintClass::Uber => true,
            PaintClass::SolidCoverage => self.is_strip(),
            PaintClass::Gradient | PaintClass::Image | PaintClass::ImageFiltered => matches!(
                self,
                Self::StripIntermediate
                    | Self::StripAlpha
                    | Self::StripDepthAlpha
                    | Self::StripOpaque
            ),
            PaintClass::BlurredRect | PaintClass::LayerComposite => matches!(
                self,
                Self::StripIntermediate | Self::StripAlpha | Self::StripDepthAlpha
            ),
        }
    }

    /// The class this state actually draws a run of `class` with: the class
    /// itself when served, [`PaintClass::Uber`] — which draws everything —
    /// when not.
    #[must_use]
    pub const fn fold_class(self, class: PaintClass) -> PaintClass {
        if self.serves(class) {
            class
        } else {
            PaintClass::Uber
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
            Self::Filter => FILTER_BIND_GROUPS,
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
            Self::StripIntermediate | Self::Clear | Self::Copy | Self::Filter => {
                INTERMEDIATE_FORMAT
            }
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
            // A filter pass replaces the region it writes rather than
            // compositing onto it: the round cleared the page first, and the
            // quad's padding border writes transparent black on purpose. A
            // blend would turn that deliberate erase into a no-op.
            Self::StripOpaque | Self::Clear | Self::Copy | Self::Filter => None,
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
            | Self::Copy
            | Self::Filter => None,
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
            Self::Filter => vec![filter_vertex_layout()],
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
            Self::Filter => EngineShaderModule::Filter,
        }
    }

    /// The registered id of the module this pipeline draws with — for a
    /// strip state, the whole ([`PaintClass::Uber`]) program.
    #[must_use]
    pub fn shader(self, shaders: &EngineShaders) -> ShaderId {
        self.class_shader(shaders, PaintClass::Uber)
    }

    /// The registered id of the module this pipeline draws `class` with: the
    /// class's specialisation for a strip state that serves it, the Uber
    /// program for one that does not, and the pipeline's own module for a
    /// non-strip pipeline (which has no class axis).
    #[must_use]
    pub fn class_shader(self, shaders: &EngineShaders, class: PaintClass) -> ShaderId {
        match self.module() {
            EngineShaderModule::Strip => shaders.strip_class(self.fold_class(class)),
            EngineShaderModule::Clear => shaders.clear,
            EngineShaderModule::Copy => shaders.copy,
            EngineShaderModule::Filter => shaders.filter,
        }
    }

    /// The full pipeline description of this state's whole
    /// ([`PaintClass::Uber`]) program, ready for
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
        self.class_desc(shaders, target_format, PaintClass::Uber)
    }

    /// The full pipeline description of this state drawing `class` — folded
    /// to Uber when the state does not serve it ([`Self::fold_class`]), and
    /// ignored by a non-strip pipeline.
    ///
    /// A strip description names the shared explicit strip layout, so every
    /// class of every strip state accepts the same bind groups; the clear,
    /// copy and filter pipelines keep wgpu's derived layout.
    #[must_use]
    pub fn class_desc(
        self,
        shaders: &EngineShaders,
        target_format: wgpu::TextureFormat,
        class: PaintClass,
    ) -> RenderPipelineDesc {
        RenderPipelineDesc {
            shader: self.class_shader(shaders, class),
            vs: VS_MAIN.into(),
            fs: FS_MAIN.into(),
            vertex_layouts: self.vertex_layouts(),
            blend: self.blend(),
            format: self.format(target_format),
            sample_count: 1,
            depth: self.depth(),
            topology: TOPOLOGY,
            layout: self.is_strip().then(|| shaders.strip_layout.clone()),
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

/// The filter program's instance layout: two `u16`-pair extents each for the
/// source region, the destination region and the destination page, plus the
/// filter's own texel offset, the layer's unscaled extent and the pass kind —
/// eight `u32`s, 32 bytes per instance.
///
/// Byte-identical to [`crate::filters::blur::FilterInstanceData`], which is
/// what the fragment stage unpacks; `shader_src`'s own naga test pins the two
/// against each other.
#[must_use]
pub fn filter_vertex_layout() -> VertexLayout {
    VertexLayout {
        array_stride: 32,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: wgpu::vertex_attr_array![
            0 => Uint32,
            1 => Uint32,
            2 => Uint32,
            3 => Uint32,
            4 => Uint32,
            5 => Uint32,
            6 => Uint32,
            7 => Uint32,
        ]
        .to_vec(),
    }
}

/// The strip pipeline a pass whose colour attachment is one atlas array layer
/// draws through.
///
/// See the module doc's *The atlas-layer target* for why this is a name for an
/// existing variant rather than a variant of its own.
pub const ATLAS_STRIP_PIPELINE: EnginePipeline = EnginePipeline::StripIntermediate;

/// The description [`ATLAS_STRIP_PIPELINE`] is built from.
///
/// Takes no target format: the atlas array's format is fixed by the array
/// itself, so unlike a pass over the frame's own target there is nothing here
/// to negotiate. Already covered by [`warm_up_descs`] — an atlas pass is
/// therefore never the first place a pipeline is compiled, on the frames that
/// have one.
#[must_use]
pub fn atlas_strip_desc(shaders: &EngineShaders) -> RenderPipelineDesc {
    ATLAS_STRIP_PIPELINE.desc(shaders, INTERMEDIATE_FORMAT)
}

/// The full warm-up list: every pipeline in [`EnginePipeline::ALL`] (each
/// strip state's Uber program), then [`WARM_CLASSES`] across
/// [`WARM_CLASS_STATES`], described against `target_format`.
///
/// The class entries are left out when `FRUST_ENGINE_UBER_PROGRAM` is set
/// ([`crate::config::uber_program_forced`]): nothing would draw with them.
#[must_use]
pub fn warm_up_descs(
    shaders: &EngineShaders,
    target_format: wgpu::TextureFormat,
) -> Vec<RenderPipelineDesc> {
    warm_up_descs_for(shaders, target_format, crate::config::uber_program_forced())
}

/// [`warm_up_descs`] with the kill switch stated rather than read: with
/// `uber_only` set the list is exactly the nine Uber states.
#[must_use]
pub fn warm_up_descs_for(
    shaders: &EngineShaders,
    target_format: wgpu::TextureFormat,
    uber_only: bool,
) -> Vec<RenderPipelineDesc> {
    let mut descs: Vec<_> = EnginePipeline::ALL
        .iter()
        .map(|pipeline| pipeline.desc(shaders, target_format))
        .collect();
    if !uber_only {
        for state in WARM_CLASS_STATES {
            for class in WARM_CLASSES {
                descs.push(state.class_desc(shaders, target_format, class));
            }
        }
    }
    descs
}

/// Queues the [`warm_up_descs`] list for background compilation.
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
        assert_eq!(EnginePipeline::ALL.len(), 9);
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
    fn the_class_axis_is_folded_to_thirty_three_variants() {
        assert_eq!(VARIANT_COUNT, 33);
        let served = |pipeline: EnginePipeline| {
            PaintClass::ALL
                .iter()
                .filter(|class| pipeline.serves(**class))
                .count()
        };
        assert_eq!(served(EnginePipeline::StripIntermediate), 7);
        assert_eq!(served(EnginePipeline::StripAlpha), 7);
        assert_eq!(served(EnginePipeline::StripDepthAlpha), 7);
        assert_eq!(served(EnginePipeline::StripOpaque), 5);
        assert_eq!(served(EnginePipeline::StripDestOut), 2);
        assert_eq!(served(EnginePipeline::StripDepthDestOut), 2);
        for pipeline in [
            EnginePipeline::Clear,
            EnginePipeline::Copy,
            EnginePipeline::Filter,
        ] {
            assert_eq!(served(pipeline), 1);
            assert!(pipeline.serves(PaintClass::Uber));
        }
        // An unserved class folds to the program that draws everything.
        assert_eq!(
            EnginePipeline::StripDestOut.fold_class(PaintClass::Gradient),
            PaintClass::Uber
        );
        assert_eq!(
            EnginePipeline::StripOpaque.fold_class(PaintClass::LayerComposite),
            PaintClass::Uber
        );
        assert_eq!(
            EnginePipeline::StripDestOut.fold_class(PaintClass::SolidCoverage),
            PaintClass::SolidCoverage
        );
        // The warm-up classes are served by the states they are warmed on.
        for state in WARM_CLASS_STATES {
            assert!(state.is_strip() && state.depth().is_some());
            for class in WARM_CLASSES {
                assert!(state.serves(class));
            }
        }
    }

    #[test]
    fn the_state_index_is_the_position_in_all() {
        for (index, pipeline) in EnginePipeline::ALL.iter().enumerate() {
            assert_eq!(pipeline.index(), index, "{}", pipeline.label());
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

    /// The atlas pass reuses the intermediate variant, and the equality that
    /// makes the reuse sound is checked rather than asserted in prose.
    ///
    /// A colour attachment whose format disagrees with its pipeline's is a
    /// validation error, not a mis-render — but the failure would surface on a
    /// device, on the first frame that missed a glyph, rather than here.
    #[test]
    fn the_atlas_target_reuses_the_intermediate_variant() {
        assert_eq!(
            crate::gpu::atlas::ATLAS_FORMAT,
            INTERMEDIATE_FORMAT,
            "an atlas layer is drawn through the intermediate variant, so the two formats are one \
             decision"
        );

        assert_eq!(ATLAS_STRIP_PIPELINE, EnginePipeline::StripIntermediate);
        assert_eq!(
            ATLAS_STRIP_PIPELINE.format(TARGET),
            crate::gpu::atlas::ATLAS_FORMAT,
            "the atlas pipeline writes the atlas format whatever the frame's own target is"
        );
        assert_eq!(
            ATLAS_STRIP_PIPELINE.blend(),
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            "a replayed glyph composites source-over onto whatever the slot already holds"
        );
        assert_eq!(
            ATLAS_STRIP_PIPELINE.depth(),
            None,
            "an atlas layer carries no depth attachment for a pipeline to test against"
        );
        assert!(ATLAS_STRIP_PIPELINE.is_strip());

        // Warmed up with the rest, so an atlas pass never compiles inline.
        let module = ATLAS_STRIP_PIPELINE.module();
        assert_eq!(module, EngineShaderModule::Strip);
        assert!(EnginePipeline::ALL.contains(&ATLAS_STRIP_PIPELINE));
        assert!(
            frust_gpu::lint_pipeline_layout(&ATLAS_STRIP_PIPELINE.layout_desc()).is_empty(),
            "the atlas pass must stay inside the downlevel design rules"
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

    /// The filter pass is a pipeline of its own, not a strip variant under
    /// another name: its own program, its own instance layout, and the only
    /// one whose bind groups carry a sampler.
    ///
    /// Its render state is stated here rather than left to the module doc
    /// because each field is load-bearing: an intermediate colour format (a
    /// filter round only ever writes a pooled page), no blend (the pass
    /// replaces the region and deliberately writes transparent black over the
    /// padding border), and no depth (a page carries no depth attachment).
    #[test]
    fn the_filter_pipeline_replaces_an_intermediate_region_unblended_and_undepthed() {
        let filter = EnginePipeline::Filter;

        assert_eq!(filter.module(), EngineShaderModule::Filter);
        assert_eq!(
            filter.format(TARGET),
            INTERMEDIATE_FORMAT,
            "a filter pass writes a pooled page, never the frame's own target"
        );
        assert_eq!(
            filter.blend(),
            None,
            "a blend would turn the padding border's deliberate erase into a no-op"
        );
        assert_eq!(filter.depth(), None);
        assert!(!filter.is_strip());
        assert_eq!(filter.bind_group_count(), FILTER_BIND_GROUPS);
        assert_eq!(filter.vertex_layouts(), vec![filter_vertex_layout()]);
        assert!(
            filter.layout_desc().uniform_buffer_sizes.is_empty(),
            "a filter pass takes its viewport from its own instance, not a uniform"
        );

        // Warmed up with the rest, so a frame that blurs never compiles a
        // pipeline inline.
        assert!(EnginePipeline::ALL.contains(&filter));
        assert!(
            frust_gpu::lint_pipeline_layout(&filter.layout_desc()).is_empty(),
            "the filter pass must stay inside the downlevel design rules"
        );
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

        let filter = filter_vertex_layout();
        assert_eq!(filter.step_mode, wgpu::VertexStepMode::Instance);
        assert_eq!(filter.attributes.len(), 8);
        assert_eq!(
            filter.array_stride as usize,
            size_of::<crate::filters::blur::FilterInstanceData>()
        );
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
        assert_eq!(
            library.len(),
            shader_src::MODULES.len() + PaintClass::SPECIALISED.len(),
            "one module per engine shader source, plus one per strip class"
        );
        assert_eq!(
            EnginePipeline::ALL
                .iter()
                .filter(|pipeline| pipeline.is_strip())
                .count(),
            6,
            "six render states over the one strip program"
        );

        let mut cache = PipelineCache::new(std::sync::Arc::new(library), None);
        let warm = warm_up_descs_for(&shaders, TARGET, false);
        assert_eq!(
            warm.len(),
            EnginePipeline::ALL.len() + WARM_CLASSES.len() * WARM_CLASS_STATES.len()
        );
        assert_eq!(
            warm_up_descs_for(&shaders, TARGET, true).len(),
            EnginePipeline::ALL.len(),
            "the kill switch warms the Uber states alone"
        );
        // Every variant the engine can create, not just the warmed ones: each
        // class program must validate against the shared explicit layout under
        // every state that serves it.
        let mut descs = Vec::new();
        for pipeline in EnginePipeline::ALL {
            for class in PaintClass::ALL {
                if pipeline.serves(class) {
                    descs.push(pipeline.class_desc(&shaders, TARGET, class));
                }
            }
        }
        assert_eq!(descs.len(), VARIANT_COUNT);
        for desc in &warm {
            assert!(
                descs.contains(desc),
                "a warmed variant the engine never builds"
            );
        }
        // The atlas pass draws through a description the warm-up list already
        // carries, so it never compiles a pipeline inline on a frame that
        // misses a glyph.
        assert!(
            warm.contains(&atlas_strip_desc(&shaders)),
            "the atlas description must be one the warm-up list already covers"
        );
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
            VARIANT_COUNT as u64,
            "each engine pipeline variant must compile exactly once"
        );
    }
}
