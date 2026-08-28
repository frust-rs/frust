//! The wgpu quad pass that draws [`crate::snapshot`]'s cached page textures
//! onto a frame — the consumer half of a [`FramePlan`](crate::snapshot::FramePlan).
//!
//! A cached page is a full-screen-ish image. Handing one to vello as an image
//! quad costs ~100 ms/frame in its fine stage on a low-end mobile GPU (Adreno
//! 620), which is the whole reason this module exists: the same pixels drawn
//! as one alpha-blended triangle strip through an ordinary render pipeline is
//! a single bilinear sample per pixel, and it runs OUTSIDE vello entirely.
//!
//! ## Where the pass lands
//!
//! One frame is composed as
//!
//! ```text
//! vello(pre) -> composite(layers)
//!            -> [vello(trailing, holes skipped, transparent scratch)
//!                -> composite(full quad)]
//! ```
//!
//! and both halves are recorded by ONE [`Compositor::composite`] call, whose
//! render pass loads (`LoadOp::Load`) what vello already wrote and draws the
//! layers in scene order, then the trailing scratch last.
//!
//! When the pre segment paints nothing at all
//! (`crate::snapshot::FramePlan::pre_draws`) the render path skips `vello(pre)`
//! outright — a pass that would draw no pixel still costs a full fine-stage
//! sweep of the target — and hands the frame's base colour over as
//! [`CompositeTarget::clear`]; the load op becomes `LoadOp::Clear` and this
//! pass alone composes the frame. The caller supplies
//! the attachment, which differs per render path
//! ([`crate::context::RenderPath`]): the acquired swapchain texture after
//! vello on the direct arm, the intermediate before the blit on the blit arm,
//! and the acquired swapchain texture AFTER the premultiply pass on the
//! direct-premultiplied arm — see [`OutputAlpha`] for why the last one needs
//! its own pipeline.
//!
//! Ordering is the same argument the two pre-passes already rely on: vello's
//! `render_to_texture` submits its own work, this pass submits its own
//! encoder afterwards, and wgpu serializes queue submissions — so every
//! sampled texture (the cached pages, the trailing scratch) is complete
//! before the quads read it. Nothing here is ever recorded across a vello
//! call: the pass is begun and ended inside one function.
//!
//! ## Placement
//!
//! Each layer's texture pixels `(0, 0)`..`(width, height)` cover its
//! [`CompositeLayer::rect`] under the bracket's own transform and this
//! frame's presentation scale — [`quad_transform`], which is
//! [`crate::snapshot`]'s coordinate mapping read forwards and the exact
//! composition `convert.rs` applied when the bracket lowered to an in-vello
//! image quad. The clip-space fold ([`ndc_transform`], including the y flip a
//! device-pixel space needs against NDC's y-up) happens on the CPU rather
//! than in the vertex stage: it is one `Affine` multiply per quad per frame,
//! it keeps the vertex stage to a single 2x3 transform, and — the reason that
//! matters here — it makes the whole mapping assertable without a GPU.
//!
//! ## Alpha
//!
//! vello writes STRAIGHT (un-premultiplied) alpha into a snapshot texture, so
//! the fragment stage premultiplies as it samples and the bracket's `alpha`
//! rides on the blend rather than on any vello layer. The two [`OutputAlpha`]
//! variants differ only in that final write and its blend state; they share
//! one WGSL source and one bind-group layout.

use kurbo::{Affine, Rect};

use crate::snapshot::CompositeLayer;

/// How a composited quad must be written for the target it lands on.
///
/// The distinction is the destination's alpha convention, not the source's:
/// a cached page is always sampled as straight alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputAlpha {
    /// The target holds straight (or opaque) pixels — vello's own output on
    /// the direct and blit arms. The fragment emits `(rgb, a * alpha)` and the
    /// blend is ordinary source-alpha compositing.
    ///
    /// Exact for an opaque destination, which is what both arms present:
    /// source-alpha blending computes `dst * (1 - a) + rgb * a`, which is the
    /// straight-alpha `over` only when the destination alpha is 1. A
    /// translucent surface takes the [`Premultiplied`](Self::Premultiplied)
    /// arm instead, where the algebra closes for every destination alpha.
    Straight,
    /// The target holds PREMULTIPLIED pixels — the swapchain of a
    /// premultiplied-expecting translucent surface, after
    /// [`crate::context::PremultiplyPass`] has converted vello's output. The
    /// fragment emits `(rgb * a * alpha, a * alpha)` — the same `(rgb*a, a)`
    /// that pass writes, with the bracket's presentation alpha folded in —
    /// and the blend is premultiplied `over`.
    Premultiplied,
}

/// Bytes of one quad's uniform record: three `vec4<f32>`s (see [`QUAD_WGSL`]'s
/// `Quad`).
const UNIFORM_SIZE: u64 = 48;

/// The compositor's whole shader: one vertex stage shared by both output
/// variants and one fragment entry point per variant.
///
/// The vertex stage takes no vertex buffer. It expands `@builtin(vertex_index)`
/// 0..4 into the unit-quad corners `(0,0)`, `(1,0)`, `(0,1)`, `(1,1)` — a
/// triangle strip — scales each corner onto the page's own pixel grid, and
/// applies the per-quad affine, which the CPU has already composed all the way
/// through to clip space (see [`quad_transform`]/[`ndc_transform`]). `uv` is
/// the corner itself, so the sampler reads the whole texture regardless of its
/// size.
const QUAD_WGSL: &str = r#"
struct Quad {
    // The clip-space affine's linear part, column-major: (xx, yx, xy, yy).
    linear: vec4<f32>,
    // .xy the clip-space affine's translation, .zw the page's pixel size.
    offset_size: vec4<f32>,
    // .x the bracket's presentation alpha this frame; .yzw pad the record to
    // the 16-byte uniform layout.
    alpha: vec4<f32>,
}

@group(0) @binding(0) var<uniform> quad: Quad;
@group(0) @binding(1) var page: texture_2d<f32>;
@group(0) @binding(2) var page_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    let texel = corner * quad.offset_size.zw;
    var out: VsOut;
    out.position = vec4<f32>(
        quad.linear.x * texel.x + quad.linear.z * texel.y + quad.offset_size.x,
        quad.linear.y * texel.x + quad.linear.w * texel.y + quad.offset_size.y,
        0.0,
        1.0,
    );
    out.uv = corner;
    return out;
}

// Straight-alpha target: the presentation alpha scales the sampled alpha and
// source-alpha blending does the rest.
@fragment
fn fs_straight(in: VsOut) -> @location(0) vec4<f32> {
    let page_color = textureSample(page, page_sampler, in.uv);
    return vec4<f32>(page_color.rgb, page_color.a * quad.alpha.x);
}

// Premultiplied target: emit exactly what `PremultiplyPass` would have
// written for this pixel, with the presentation alpha folded in.
@fragment
fn fs_premultiplied(in: VsOut) -> @location(0) vec4<f32> {
    let page_color = textureSample(page, page_sampler, in.uv);
    let alpha = page_color.a * quad.alpha.x;
    return vec4<f32>(page_color.rgb * alpha, alpha);
}
"#;

/// Whether this crate can composite onto a target of `format`.
///
/// Both 8-bit unorm surface formats are accepted — the blit arm's swapchain is
/// whichever of the two the platform advertises, even though today's composite
/// attachment is always the `Rgba8Unorm` intermediate. Anything else (an sRGB
/// view, a float target) would need its own pipeline and its own arithmetic
/// review, so it is refused rather than approximated: [`Compositor::new`]
/// answers `None` and the surface falls back to inline bracket lowering.
fn is_supported_target_format(format: wgpu::TextureFormat) -> bool {
    matches!(
        format,
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
    )
}

/// The blend state each [`OutputAlpha`] composites with.
///
/// Straight is wgpu's `ALPHA_BLENDING` (color `SrcAlpha`/`OneMinusSrcAlpha`,
/// alpha `One`/`OneMinusSrcAlpha`); premultiplied is
/// `PREMULTIPLIED_ALPHA_BLENDING` (`One`/`OneMinusSrcAlpha` on both). Named
/// here so the choice has one home and one test.
fn blend_state(output: OutputAlpha) -> wgpu::BlendState {
    match output {
        OutputAlpha::Straight => wgpu::BlendState::ALPHA_BLENDING,
        OutputAlpha::Premultiplied => wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
    }
}

/// The [`QUAD_WGSL`] fragment entry point each [`OutputAlpha`] draws with.
fn fragment_entry(output: OutputAlpha) -> &'static str {
    match output {
        OutputAlpha::Straight => "fs_straight",
        OutputAlpha::Premultiplied => "fs_premultiplied",
    }
}

/// Stride between two quads' uniform records in the shared buffer: the record
/// size rounded up to the device's `min_uniform_buffer_offset_alignment` (256
/// on every backend frust ships on, and forced to 256 on the iOS Simulator by
/// [`crate::context`]'s limits mitigation), since that is the granularity a
/// dynamic offset may address. Never smaller than one record.
fn uniform_stride(min_alignment: u32) -> u64 {
    let alignment = u64::from(min_alignment).max(1);
    UNIFORM_SIZE.div_ceil(alignment) * alignment
}

/// Whether a `composite` call has anything to do. The empty case is the
/// steady state of every frame that composites nothing (no cache hits, or the
/// kill switch): no encoder, no pass, no submit — the arm behaves exactly as
/// it did before this module existed.
///
/// A pending `clear` is work in its own right even with no quad to draw: on
/// that frame this pass is the ONLY thing writing the target (the render path
/// skipped vello's), so skipping it would present an undefined frame.
fn has_work(layers: usize, trailing: bool, clear: bool) -> bool {
    layers > 0 || trailing || clear
}

/// The clear value for a target holding `output`-convention pixels.
///
/// `wgpu` writes a clear value into a non-sRGB unorm attachment verbatim (and
/// this pass refuses every other format, [`is_supported_target_format`]), so
/// the straight variant hands over `peniko`'s own encoded components — the
/// same bytes vello's fine stage stores for a pixel the scene never covers,
/// which is what makes a skipped main pass indistinguishable from one that
/// only cleared. The premultiplied variant multiplies the colour through by
/// its own alpha, exactly as [`crate::context::PremultiplyPass`]'s
/// `(rgb * a, a)` would have.
fn clear_color(color: peniko::Color, output: OutputAlpha) -> wgpu::Color {
    let [red, green, blue, alpha] = color.components.map(f64::from);
    let scale = match output {
        OutputAlpha::Straight => 1.0,
        OutputAlpha::Premultiplied => alpha,
    };
    wgpu::Color {
        r: red * scale,
        g: green * scale,
        b: blue * scale,
        a: alpha,
    }
}

/// A layer's enclosing clip as a scissor rect on a `target`-sized attachment:
/// `(x, y, width, height)` in pixels, rounded OUTWARD so a clip edge on a
/// fractional pixel keeps that pixel rather than shaving a column off the
/// page, and clamped into the attachment (`wgpu` rejects a scissor that leaves
/// it).
///
/// `None` when the clip covers no pixel at all — an empty intersection of
/// nested clips, or a clip entirely off-surface. The caller then draws no
/// quad, which is what the clip means.
fn scissor_rect(clip: Rect, target: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let (width, height) = (f64::from(target.0), f64::from(target.1));
    let x0 = clip.x0.floor().clamp(0.0, width);
    let y0 = clip.y0.floor().clamp(0.0, height);
    let x1 = clip.x1.ceil().clamp(0.0, width);
    let y1 = clip.y1.ceil().clamp(0.0, height);
    // Also the NaN guard: every comparison against a NaN edge is false.
    (x1 > x0 && y1 > y0).then_some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Map a page's own pixel grid onto the surface: texture pixels
/// `(0, 0)`..`(width, height)` cover `rect` under `transform *
/// scale_about(scale, rect.center())`.
///
/// Read right to left: the page's pixels are scaled down onto `rect`'s local
/// extent, translated to `rect`'s origin, scaled about `rect`'s center by this
/// frame's presentation scale, and placed by the bracket's own transform. This
/// is [`crate::snapshot`]'s rasterization mapping inverted, and the exact
/// composition the encode walk applied while a cache hit still lowered to an
/// in-vello image quad — which is what makes a pixel-aligned page at scale 1.0
/// land identically either way.
///
/// A zero extent cannot occur (`snapshot_size` floors both axes to 1) but is
/// floored again rather than dividing by zero.
fn quad_transform(rect: Rect, scale: f64, transform: Affine, width: u32, height: u32) -> Affine {
    transform
        * Affine::scale_about(scale, rect.center())
        * Affine::translate((rect.x0, rect.y0))
        * Affine::scale_non_uniform(
            rect.width() / f64::from(width.max(1)),
            rect.height() / f64::from(height.max(1)),
        )
}

/// Device pixels to clip space for a `target`-sized attachment: `x_ndc =
/// 2x/W - 1` and `y_ndc = 1 - 2y/H`. The y flip is the whole content — device
/// pixel space runs downward from the top-left, NDC upward from the center.
fn ndc_transform(target: (u32, u32)) -> Affine {
    let width = f64::from(target.0.max(1));
    let height = f64::from(target.1.max(1));
    Affine::new([2.0 / width, 0.0, 0.0, -2.0 / height, -1.0, 1.0])
}

/// Pack one quad's uniform record: the clip-space affine's coefficients, the
/// page's pixel size, and the presentation alpha, as little-endian `f32`s in
/// [`QUAD_WGSL`]'s `Quad` layout. Pure — the byte layout is unit-testable.
fn uniform_bytes(clip: Affine, width: u32, height: u32, alpha: f32) -> [u8; UNIFORM_SIZE as usize] {
    fn put(bytes: &mut [u8], offset: usize, value: f64) {
        bytes[offset..offset + 4].copy_from_slice(&(value as f32).to_le_bytes());
    }
    let coefficients = clip.as_coeffs();
    let mut bytes = [0u8; UNIFORM_SIZE as usize];
    // linear: (xx, yx, xy, yy) at 0..16, then the translation at 16..24.
    for (index, coefficient) in coefficients.iter().enumerate() {
        put(&mut bytes, index * 4, *coefficient);
    }
    put(&mut bytes, 24, f64::from(width));
    put(&mut bytes, 28, f64::from(height));
    put(&mut bytes, 32, f64::from(alpha));
    // 36..48 stays zero — the `alpha` vec4's padding lanes.
    bytes
}

/// The attachment one [`Compositor::composite`] call draws onto: the view, its
/// pixel size, and the alpha convention its pixels are stored in.
///
/// A struct rather than three parameters because the three must agree — the
/// view's own arm decides all of them (see [`crate::context::RenderPath`]),
/// and passing the wrong `output` for a view is the one mistake here that
/// produces plausible-looking wrong pixels rather than an error.
pub(crate) struct CompositeTarget<'a> {
    pub view: &'a wgpu::TextureView,
    pub size: (u32, u32),
    pub output: OutputAlpha,
    /// The frame's base colour, when this pass must CLEAR the target to it
    /// instead of loading what vello wrote — the render path skipped the main
    /// vello pass because its segment drew nothing
    /// (`crate::snapshot::FramePlan::pre_draws`), so nothing wrote the target
    /// before this pass and the backdrop is this pass's job.
    ///
    /// `None` on every ordinary frame: load what is already there.
    pub clear: Option<peniko::Color>,
}

/// How many frames the trailing scratch may go unused before it is released:
/// a scratch last used before `frame - MAX_UNUSED_FRAMES` is dropped.
///
/// The same two frames of slack [`crate::snapshot::SnapshotCache`]'s entries
/// get, for the same reason — a scene that skips a trailing segment for one
/// frame (a diff hiccup, an every-other-frame repaint) should not pay a
/// re-allocation — while a scene that has simply stopped drawing after its
/// first composited bracket releases a full-surface texture promptly instead
/// of at surface teardown.
const MAX_UNUSED_FRAMES: u64 = 2;

/// The transparent intermediate the trailing segment's vello pass renders
/// into, sized to the surface and recreated when that size changes.
struct Scratch {
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    /// [`Compositor::frame`] when a frame last rendered a trailing segment
    /// into this texture — the clock [`Compositor::age_scratch`] ages against.
    last_used: u64,
}

/// Whether a scratch last used at `last_used` is stale at `frame`. The
/// boundary is inclusive of `frame - max_unused` (that scratch survives),
/// exactly like `snapshot::evictable_keys`.
fn scratch_expired(last_used: u64, frame: u64, max_unused: u64) -> bool {
    last_used + max_unused < frame
}

/// The per-surface quad pass: two pipelines over one shader, the sampler and
/// bind-group layout they share, a growable uniform buffer holding one record
/// per quad, and the trailing segment's scratch target.
///
/// Crate-private, like every other holder of a `wgpu` type here
/// (`docs/CODE_STANDARDS.md`). One instance lives per ready GPU-tier surface,
/// beside the [`crate::snapshot::SnapshotCache`] whose plans it draws, and
/// dies with it.
pub(crate) struct Compositor {
    straight: wgpu::RenderPipeline,
    premultiplied: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// One [`UNIFORM_SIZE`] record per quad at [`Self::stride`] spacing, or
    /// `None` until the first composited frame. Grown, never shrunk: a page
    /// transition's quad count is tiny and stable.
    uniforms: Option<wgpu::Buffer>,
    /// How many quads [`Self::uniforms`] currently has room for.
    capacity: u32,
    stride: u64,
    scratch: Option<Scratch>,
    /// Monotonic frame counter — the clock [`Scratch::last_used`] ages
    /// against. Advanced once per [`Self::age_scratch`], which the render path
    /// calls on every frame it encodes, trailing segment or not.
    frame: u64,
}

impl Compositor {
    /// Build the pass for a surface whose composite attachment is `format`,
    /// seeding both pipelines from the surface's persisted pipeline cache (as
    /// vello's and the shader effects' pipelines are).
    ///
    /// `None` when the target format is one this pass has no arithmetic for
    /// ([`is_supported_target_format`]) — the caller then disables the
    /// snapshot cache for that surface, so every bracket lowers inline and the
    /// frame is exactly what it was before snapshot layers existed. Never
    /// panics and never leaves a surface half-composited.
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        pipeline_cache: Option<&wgpu::PipelineCache>,
    ) -> Option<Self> {
        if !is_supported_target_format(format) {
            log::warn!(
                "frust-render: no snapshot compositor for target format {format:?} — \
                 brackets lower inline on this surface"
            );
            return None;
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frust-render compositor module"),
            source: wgpu::ShaderSource::Wgsl(QUAD_WGSL.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frust-render compositor binds"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    // The vertex stage reads the affine, the fragment stage the
                    // alpha — one record, both stages.
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        // One buffer, one bind group per quad, each quad's
                        // record addressed by a dynamic offset. Immediate
                        // (push) constants are deliberately not used — they are
                        // not universally available on the targets frust ships.
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("frust-render compositor layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = |output: OutputAlpha| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("frust-render compositor pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    // Four corners, one strip; no culling, so a mirroring
                    // bracket transform still draws.
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fragment_entry(output)),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend_state(output)),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: pipeline_cache,
            })
        };
        Some(Self {
            straight: pipeline(OutputAlpha::Straight),
            premultiplied: pipeline(OutputAlpha::Premultiplied),
            bind_layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("frust-render compositor sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            uniforms: None,
            capacity: 0,
            stride: uniform_stride(device.limits().min_uniform_buffer_offset_alignment),
            scratch: None,
            frame: 0,
        })
    }

    /// The transparent `Rgba8Unorm` target the trailing segment's vello pass
    /// renders into, (re)created when the surface size changes and marked as
    /// used this frame (see [`Self::age_scratch`]).
    ///
    /// `STORAGE_BINDING` is vello's write path (it renders by compute),
    /// `TEXTURE_BINDING` is this pass's read path — the same pair a cached
    /// page carries, and for the same reason: the pixels are sampled where
    /// they were written, never staged through a copy.
    pub(crate) fn ensure_scratch(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> &wgpu::TextureView {
        let stale = self
            .scratch
            .as_ref()
            .is_none_or(|scratch| scratch.width != width || scratch.height != height);
        if stale {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frust-render compositor trailing scratch"),
                size: wgpu::Extent3d {
                    width: width.max(1),
                    height: height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            self.scratch = Some(Scratch {
                view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                width,
                height,
                last_used: self.frame,
            });
        }
        // Set immediately above when it was missing or stale.
        let scratch = self
            .scratch
            .as_mut()
            .expect("scratch is created when stale");
        scratch.last_used = self.frame;
        &scratch.view
    }

    /// Advance the frame clock and release the trailing scratch once no frame
    /// has needed one for [`MAX_UNUSED_FRAMES`] frames — the mirror of
    /// [`crate::snapshot::SnapshotCache`]'s own eviction, for the one texture
    /// this module owns.
    ///
    /// Called once per encoded frame by the render path, INCLUDING frames with
    /// no trailing segment (which is the whole point: those are the frames the
    /// scratch ages on). A frame that never allocated one does a counter
    /// increment and nothing else. Dropping the [`Scratch`] releases this
    /// module's handle; a `PendingComposite` still holding a clone of the view
    /// keeps the texture alive until that frame is submitted, since a wgpu
    /// view refcounts its texture.
    pub(crate) fn age_scratch(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        if self
            .scratch
            .as_ref()
            .is_some_and(|scratch| scratch_expired(scratch.last_used, frame, MAX_UNUSED_FRAMES))
        {
            self.scratch = None;
        }
    }

    /// Whether a trailing scratch is currently allocated, for aging
    /// assertions.
    #[cfg(test)]
    fn has_scratch(&self) -> bool {
        self.scratch.is_some()
    }

    /// Draw this frame's cached pages onto `target`, in scene order, then the
    /// trailing segment's scratch (when there is one) as a full-target quad at
    /// alpha 1.
    ///
    /// Records ONE render pass over `target` and submits its own encoder. The
    /// pass LOADS by default — everything vello (and, on the premultiplied
    /// arm, [`crate::context::PremultiplyPass`]) already wrote survives
    /// underneath — or CLEARS to [`CompositeTarget::clear`] when the render
    /// path skipped the main vello pass and this is the frame's only write.
    /// A no-op with nothing to draw and nothing to clear: no encoder, no
    /// submit.
    ///
    /// Each layer is drawn under its own [`CompositeLayer::clip`] as a scissor
    /// rect (the quad left vello, so nothing else still applies the clips the
    /// bracket was recorded under); a layer whose clip covers no pixel is
    /// skipped, and the trailing scratch always draws over the whole target.
    ///
    /// `target` carries the attachment, its pixel size (the surface size on
    /// every arm), the alpha convention its pixels are stored in, and that
    /// clear.
    pub(crate) fn composite(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: CompositeTarget<'_>,
        layers: &[CompositeLayer],
        trailing: Option<&wgpu::TextureView>,
    ) {
        if !has_work(layers.len(), trailing.is_some(), target.clear.is_some()) {
            return;
        }
        let quads = layers.len() + usize::from(trailing.is_some());
        // Clamped, never returned. Unreachable in practice — a frame's
        // brackets are counted in single digits and each layer holds a live
        // page texture, so 2^32 of them cannot coexist — but from here to
        // `queue.submit` below this function must have no exit: on a frame
        // whose main vello pass was skipped, this pass's `LoadOp::Clear` is
        // the ONLY write to the swapchain, and leaving early would present an
        // undefined frame.
        let quads = u32::try_from(quads).unwrap_or(u32::MAX);
        // One bind group per quad, and the scissor each is drawn under —
        // `None` for a quad nothing clips, which draws over the whole target.
        // Both stay empty on a clear-only pass (no layers, no trailing), whose
        // whole job is the load op below.
        let mut binds: Vec<wgpu::BindGroup> = Vec::new();
        let mut scissors: Vec<Option<Rect>> = Vec::new();
        if quads > 0 {
            // A refcounted handle, not a copy of the buffer: cloning ends
            // `ensure_capacity`'s borrow of `self`, leaving the bind-group
            // build below free to read `bind_layout`/`sampler`. There is
            // deliberately no `Option` to unwrap here, and so no shape that
            // invites an early return between the `has_work` check above and
            // the `queue.submit` below.
            let uniforms = self.ensure_capacity(device, quads).clone();

            // One write for every quad's record, then one bind group per quad
            // (each names its own page texture; the record is addressed by the
            // dynamic offset). The trailing scratch is the last quad: the
            // whole target, at alpha 1, sampled 1:1 and unclipped.
            let ndc = ndc_transform(target.size);
            let mut records = vec![0u8; self.stride as usize * quads as usize];
            let mut views: Vec<&wgpu::TextureView> = Vec::with_capacity(quads as usize);
            for (index, layer) in layers.iter().enumerate() {
                let clip = ndc
                    * quad_transform(
                        layer.rect,
                        layer.scale,
                        layer.transform,
                        layer.width,
                        layer.height,
                    );
                let bytes = uniform_bytes(clip, layer.width, layer.height, layer.alpha);
                let at = self.stride as usize * index;
                records[at..at + bytes.len()].copy_from_slice(&bytes);
                views.push(&layer.view);
                scissors.push(layer.clip);
            }
            if let Some(scratch) = trailing {
                let (width, height) = target.size;
                let full = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
                let clip = ndc * quad_transform(full, 1.0, Affine::IDENTITY, width, height);
                let bytes = uniform_bytes(clip, width, height, 1.0);
                let at = self.stride as usize * layers.len();
                records[at..at + bytes.len()].copy_from_slice(&bytes);
                views.push(scratch);
                scissors.push(None);
            }
            queue.write_buffer(&uniforms, 0, &records);

            // Every bind group must outlive the pass that binds it.
            binds = views
                .iter()
                .map(|view| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("frust-render compositor bind group"),
                        layout: &self.bind_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                    buffer: &uniforms,
                                    offset: 0,
                                    size: wgpu::BufferSize::new(UNIFORM_SIZE),
                                }),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                        ],
                    })
                })
                .collect();
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-render compositor"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frust-render compositor pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // The frame vello (or the premultiply pass) just wrote
                        // is the backdrop these quads blend onto — unless the
                        // render path skipped that pass because its segment
                        // painted nothing, in which case this pass lays the
                        // base colour down itself.
                        load: match target.clear {
                            Some(color) => wgpu::LoadOp::Clear(clear_color(color, target.output)),
                            None => wgpu::LoadOp::Load,
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(match target.output {
                OutputAlpha::Straight => &self.straight,
                OutputAlpha::Premultiplied => &self.premultiplied,
            });
            for (index, bind) in binds.iter().enumerate() {
                // The clips the bracket was recorded under, which the quad has
                // left vello and so left behind: re-applied here, or the page
                // paints over chrome the scene had clipped it away from. A
                // clip covering nothing draws nothing; an unclipped quad
                // resets the scissor to the whole target, since the previous
                // quad may have narrowed it.
                let scissor = match scissors.get(index).copied().flatten() {
                    Some(clip) => match scissor_rect(clip, target.size) {
                        Some(scissor) => scissor,
                        None => continue,
                    },
                    None => (0, 0, target.size.0, target.size.1),
                };
                pass.set_scissor_rect(scissor.0, scissor.1, scissor.2, scissor.3);
                let offset = self.stride * index as u64;
                // A dynamic offset is a u32 by API; the buffer is quads *
                // stride bytes, far inside that.
                pass.set_bind_group(0, bind, &[offset as u32]);
                pass.draw(0..4, 0..1);
            }
        }
        queue.submit([encoder.finish()]);
    }

    /// Ensure the shared uniform buffer holds `quads` records and RETURN it,
    /// reallocating (never shrinking) when it does not. A steady page
    /// transition reaches its capacity on the first composited frame and never
    /// allocates again.
    ///
    /// Returning the buffer is the point: it leaves [`Self::composite`] with
    /// no `Option` to unwrap after its `has_work` check, and therefore no
    /// shape that invites an early return out of a pass whose `LoadOp::Clear`
    /// may be the frame's only write to the swapchain.
    ///
    /// `quads` is floored at 1 — a zero-length uniform buffer is not a valid
    /// binding, and the caller only asks for one when it has a quad to draw.
    fn ensure_capacity(&mut self, device: &wgpu::Device, quads: u32) -> &wgpu::Buffer {
        let quads = quads.max(1);
        // Grow by dropping the undersized buffer, so the insert below rebuilds
        // it at the new capacity. Never shrinks: a smaller frame re-uses what
        // a larger one allocated.
        if self.capacity < quads {
            self.capacity = quads;
            self.uniforms = None;
        }
        let (stride, capacity) = (self.stride, self.capacity);
        self.uniforms.get_or_insert_with(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("frust-render compositor quads"),
                size: stride * u64::from(capacity),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Point;

    /// D2's worked example: a 100x200 page over the local rect
    /// `(10, 20)..(110, 220)` at scale 1.0 under no transform maps its own
    /// pixel corners onto the rect's corners exactly.
    #[test]
    fn quad_transform_maps_the_page_grid_onto_its_rect() {
        let rect = Rect::new(10.0, 20.0, 110.0, 220.0);
        let quad = quad_transform(rect, 1.0, Affine::IDENTITY, 100, 200);
        assert_eq!(quad * Point::new(0.0, 0.0), Point::new(10.0, 20.0));
        assert_eq!(quad * Point::new(100.0, 200.0), Point::new(110.0, 220.0));
        // A page whose pixel grid is not 1:1 with its rect still covers the
        // rect exactly — the grid resolution never moves the quad.
        let dense = quad_transform(rect, 1.0, Affine::IDENTITY, 200, 400);
        assert_eq!(dense * Point::new(200.0, 400.0), Point::new(110.0, 220.0));
    }

    /// The presentation scale contracts about the rect's CENTER, not its
    /// origin — the fade-through's shrink.
    #[test]
    fn quad_transform_contracts_about_the_rect_center() {
        let rect = Rect::new(10.0, 20.0, 110.0, 220.0);
        let quad = quad_transform(rect, 0.9, Affine::IDENTITY, 100, 200);
        assert_eq!(rect.center(), Point::new(60.0, 120.0));
        assert_eq!(quad * Point::new(0.0, 0.0), Point::new(15.0, 30.0));
        assert_eq!(quad * Point::new(100.0, 200.0), Point::new(105.0, 210.0));
        // The center is the fixed point.
        assert_eq!(quad * Point::new(50.0, 100.0), Point::new(60.0, 120.0));
    }

    /// The bracket's own transform places the whole quad, after the
    /// presentation scale — a page that merely slides keeps its texture and
    /// moves only here.
    #[test]
    fn quad_transform_applies_the_bracket_transform_last() {
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        let bracket = Affine::translate((8.0, 8.0)) * Affine::scale(2.0);
        let quad = quad_transform(rect, 1.0, bracket, 40, 40);
        assert_eq!(quad * Point::new(0.0, 0.0), Point::new(8.0, 8.0));
        assert_eq!(quad * Point::new(40.0, 40.0), Point::new(48.0, 48.0));
    }

    /// Device pixels to clip space, y flipped: the top-left device corner is
    /// NDC `(-1, 1)` and the bottom-right is `(1, -1)`.
    #[test]
    fn ndc_transform_normalizes_and_flips_y() {
        let ndc = ndc_transform((200, 100));
        assert_eq!(ndc * Point::new(0.0, 0.0), Point::new(-1.0, 1.0));
        assert_eq!(ndc * Point::new(200.0, 100.0), Point::new(1.0, -1.0));
        assert_eq!(ndc * Point::new(100.0, 50.0), Point::new(0.0, 0.0));
        // A degenerate target never divides by zero.
        assert!((ndc_transform((0, 0)) * Point::new(0.0, 0.0)).is_finite());
    }

    /// The composition the vertex stage actually consumes: page pixels through
    /// the placement and on into clip space, in one affine.
    #[test]
    fn clip_transform_carries_a_page_to_its_ndc_corners() {
        let rect = Rect::new(0.0, 0.0, 32.0, 16.0);
        let clip = ndc_transform((64, 64)) * quad_transform(rect, 1.0, Affine::IDENTITY, 32, 16);
        assert_eq!(clip * Point::new(0.0, 0.0), Point::new(-1.0, 1.0));
        // Device (32, 16) of a 64-square target: half across, a quarter down.
        assert_eq!(clip * Point::new(32.0, 16.0), Point::new(0.0, 0.5));
    }

    /// The uniform record's byte layout, which the WGSL `Quad` struct mirrors:
    /// the affine's six coefficients, the page size, then the alpha.
    #[test]
    fn uniform_bytes_pack_the_clip_affine_page_size_and_alpha() {
        let clip = Affine::new([1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let bytes = uniform_bytes(clip, 40, 20, 0.75);
        assert_eq!(bytes.len(), UNIFORM_SIZE as usize);
        let at = |offset: usize| {
            f32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("4 bytes"))
        };
        assert_eq!([at(0), at(4), at(8), at(12)], [1.0, 2.0, 3.0, 4.0]);
        assert_eq!([at(16), at(20)], [5.0, 6.0]);
        assert_eq!([at(24), at(28)], [40.0, 20.0]);
        assert_eq!(at(32), 0.75);
        // The `alpha` vec4's three padding lanes stay zero.
        assert_eq!(&bytes[36..48], &[0u8; 12]);
    }

    /// A dynamic offset must be a multiple of the device's uniform alignment,
    /// so the stride rounds the record up to it — and never below one record.
    #[test]
    fn uniform_stride_rounds_a_record_up_to_the_device_alignment() {
        assert_eq!(uniform_stride(256), 256);
        assert_eq!(uniform_stride(64), 64);
        assert_eq!(uniform_stride(16), UNIFORM_SIZE);
        assert_eq!(uniform_stride(32), 64);
        assert_eq!(uniform_stride(0), UNIFORM_SIZE);
    }

    /// Both 8-bit unorm surface formats are composited; the blit arm's
    /// swapchain is whichever of the two the platform advertises.
    #[test]
    fn only_the_eight_bit_unorm_target_formats_are_composited() {
        assert!(is_supported_target_format(wgpu::TextureFormat::Rgba8Unorm));
        assert!(is_supported_target_format(wgpu::TextureFormat::Bgra8Unorm));
        for refused in [
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba16Float,
        ] {
            assert!(
                !is_supported_target_format(refused),
                "{refused:?} has no compositor arithmetic and must be refused"
            );
        }
    }

    /// R5: the premultiplied variant's blend must be `One`/`OneMinusSrcAlpha`
    /// on BOTH channels (its fragment already premultiplied), the straight
    /// variant's colour `SrcAlpha`/`OneMinusSrcAlpha`.
    #[test]
    fn each_output_alpha_carries_its_own_blend_and_entry_point() {
        let straight = blend_state(OutputAlpha::Straight);
        assert_eq!(straight.color.src_factor, wgpu::BlendFactor::SrcAlpha);
        assert_eq!(
            straight.color.dst_factor,
            wgpu::BlendFactor::OneMinusSrcAlpha
        );
        assert_eq!(straight.alpha.src_factor, wgpu::BlendFactor::One);
        assert_eq!(
            straight.alpha.dst_factor,
            wgpu::BlendFactor::OneMinusSrcAlpha
        );

        let premultiplied = blend_state(OutputAlpha::Premultiplied);
        for component in [premultiplied.color, premultiplied.alpha] {
            assert_eq!(component.src_factor, wgpu::BlendFactor::One);
            assert_eq!(component.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
            assert_eq!(component.operation, wgpu::BlendOperation::Add);
        }

        assert_ne!(
            fragment_entry(OutputAlpha::Straight),
            fragment_entry(OutputAlpha::Premultiplied)
        );
        for output in [OutputAlpha::Straight, OutputAlpha::Premultiplied] {
            assert!(
                QUAD_WGSL.contains(&format!("fn {}(", fragment_entry(output))),
                "{output:?}'s entry point must exist in the shader"
            );
        }
    }

    /// The empty-plan fast path: a frame that composites nothing records no
    /// pass and submits no encoder, which is what makes a disabled cache (and
    /// every ordinary frame) byte-identical to the pre-compositor arms. A
    /// pending clear is the exception — nothing else would write the target.
    #[test]
    fn a_plan_with_no_layers_and_no_trailing_segment_is_a_no_op() {
        assert!(!has_work(0, false, false));
        assert!(has_work(1, false, false));
        assert!(has_work(0, true, false));
        assert!(has_work(2, true, false));
        assert!(has_work(0, false, true));
    }

    /// The straight variant hands `peniko`'s components to the attachment
    /// as-is (the bytes vello's own base-colour clear would have stored); the
    /// premultiplied one multiplies the colour through by its alpha, matching
    /// `PremultiplyPass`'s `(rgb * a, a)` exactly.
    #[test]
    fn clear_color_follows_the_targets_alpha_convention() {
        let opaque = Color::from_rgba8(255, 128, 0, 255);
        let straight = clear_color(opaque, OutputAlpha::Straight);
        assert!((straight.r - 1.0).abs() < 1e-6);
        assert!((straight.g - 128.0 / 255.0).abs() < 1e-6);
        assert!((straight.b).abs() < 1e-6);
        assert!((straight.a - 1.0).abs() < 1e-6);

        // An opaque colour is its own premultiplication, so the two variants
        // agree on every colour a surface actually clears to.
        let premultiplied = clear_color(opaque, OutputAlpha::Premultiplied);
        assert!((premultiplied.r - straight.r).abs() < 1e-6);
        assert!((premultiplied.g - straight.g).abs() < 1e-6);

        let translucent = Color::from_rgba8(255, 255, 255, 128);
        let alpha = 128.0 / 255.0;
        let folded = clear_color(translucent, OutputAlpha::Premultiplied);
        assert!((folded.r - alpha).abs() < 1e-6, "{folded:?}");
        assert!((folded.a - alpha).abs() < 1e-6, "{folded:?}");
        // The straight variant leaves the same colour un-premultiplied.
        let kept = clear_color(translucent, OutputAlpha::Straight);
        assert!((kept.r - 1.0).abs() < 1e-6, "{kept:?}");
        assert!((kept.a - alpha).abs() < 1e-6, "{kept:?}");
    }

    /// A clip becomes a scissor rounded OUTWARD (a fractional edge keeps its
    /// pixel) and clamped into the attachment, or `None` when it covers
    /// nothing at all.
    #[test]
    fn scissor_rect_rounds_outward_and_clamps_into_the_target() {
        assert_eq!(
            scissor_rect(Rect::new(10.0, 20.0, 30.0, 40.0), (64, 64)),
            Some((10, 20, 20, 20))
        );
        // Fractional edges grow the rect rather than shaving the page.
        assert_eq!(
            scissor_rect(Rect::new(10.4, 20.6, 29.1, 39.2), (64, 64)),
            Some((10, 20, 20, 20))
        );
        // A clip reaching past the surface is clamped, not refused.
        assert_eq!(
            scissor_rect(Rect::new(-40.0, -10.0, 100.0, 100.0), (64, 32)),
            Some((0, 0, 64, 32))
        );
        // Empty (the intersection of disjoint clips), inverted, entirely
        // off-surface, and non-finite all mean "draw no quad".
        assert_eq!(
            scissor_rect(Rect::new(20.0, 20.0, 10.0, 40.0), (64, 64)),
            None
        );
        assert_eq!(
            scissor_rect(Rect::new(80.0, 0.0, 90.0, 10.0), (64, 64)),
            None
        );
        assert_eq!(
            scissor_rect(Rect::new(f64::NAN, 0.0, 10.0, 10.0), (64, 64)),
            None
        );
    }

    /// Coupling: every swapchain format `context::create_render_surface` can
    /// hand out must be one this pass has arithmetic for. The composite pass
    /// draws onto the swapchain itself on both direct arms, so a format added
    /// there without a matching blend/clear story here would silently disable
    /// snapshot layers on that surface (`Compositor::new` returns `None`, and
    /// the renderer then builds the cache disabled).
    #[test]
    fn every_configurable_surface_format_is_a_supported_composite_target() {
        for format in crate::context::SURFACE_FORMATS {
            assert!(
                is_supported_target_format(format),
                "{format:?} can be configured on a surface but has no composite pipeline"
            );
        }
        // The blit arm's attachment is the intermediate, always `Rgba8Unorm`.
        assert!(is_supported_target_format(wgpu::TextureFormat::Rgba8Unorm));
        // Not every format is waved through: the pass writes non-sRGB unorm
        // clear values verbatim and has no conversion for anything else.
        assert!(!is_supported_target_format(
            wgpu::TextureFormat::Bgra8UnormSrgb
        ));
    }

    /// The scratch's aging arithmetic, without a GPU: a scratch used this
    /// frame survives, and one unused for more than [`MAX_UNUSED_FRAMES`]
    /// frames does not. Same boundary as `snapshot::evictable_keys`.
    #[test]
    fn scratch_ages_out_only_after_the_unused_window() {
        // Used this frame.
        assert!(!scratch_expired(7, 7, MAX_UNUSED_FRAMES));
        // Inside the slack window: a frame that skips a trailing segment (or
        // two) must not cost a re-allocation.
        assert!(!scratch_expired(7, 8, MAX_UNUSED_FRAMES));
        assert!(!scratch_expired(7, 9, MAX_UNUSED_FRAMES));
        // Past it.
        assert!(scratch_expired(7, 10, MAX_UNUSED_FRAMES));
        assert!(scratch_expired(0, u64::from(u32::MAX), MAX_UNUSED_FRAMES));
    }

    // --- GPU smokes --------------------------------------------------------
    // The compositor is crate-private (no `wgpu` type leaves this crate), so
    // these live here rather than in `tests/gpu_smoke.rs`: an integration test
    // could only re-implement the pass, which would prove nothing about the
    // one that ships. Same shape as `snapshot.rs`'s steady-state test —
    // `#[ignore]`d, run with `cargo test -p frust-render -- --ignored`.

    use frust_scene::{Scene, SceneBuilder};
    use peniko::Brush;
    use peniko::Color;
    use peniko::ImageData;
    use peniko::color::palette::css::{BLACK, BLUE, GREEN, RED, TRANSPARENT};
    use std::collections::{HashMap, HashSet};

    use crate::convert::{Segment, encode_range_with_overrides};
    use crate::snapshot::SnapshotCache;

    /// Square target edge for every smoke: 64 * 4 bytes = 256, wgpu's
    /// row-copy alignment, so readback needs no padding math.
    const SIZE: u32 = 64;

    /// Per-pixel tolerance between the composited and the inline arm: an
    /// 8-bit rounding step, no more. Widening this would let a mis-scaled or
    /// misplaced quad pass, since a placement error shows at edges first.
    const TOLERANCE: u8 = 3;

    async fn gpu() -> (wgpu::Device, wgpu::Queue) {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect("no compatible GPU adapter");
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust compositor test"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("failed to create device")
    }

    /// A `SIZE`-square surface target with every usage the three arms need:
    /// vello's storage write, the compositor's colour attachment, this
    /// module's own sampling, and readback.
    fn surface_target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frust compositor test target"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    /// A page texture filled with one straight-alpha colour — the compositor's
    /// input without vello in the way, so a formula check reads exact texels.
    fn page_of(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: u32,
        rgba: [u8; 4],
    ) -> wgpu::TextureView {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frust compositor test page"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let pixels: Vec<u8> = rgba
            .iter()
            .copied()
            .cycle()
            .take((size * size * 4) as usize)
            .collect();
        queue.write_texture(
            texture.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Clear `view` to `color`, so a composite's backdrop is a known state
    /// (the compositor itself only ever loads).
    fn clear(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        color: wgpu::Color,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust compositor test clear"),
        });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frust compositor test clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        queue.submit([encoder.finish()]);
    }

    async fn read_back(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
    ) -> Vec<u8> {
        let bytes_per_row = SIZE * 4;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frust compositor test readback"),
            size: u64::from(bytes_per_row * SIZE),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(SIZE),
                },
            },
            wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll failed");
        rx.recv()
            .expect("map channel closed")
            .expect("buffer map failed");
        slice.get_mapped_range().to_vec()
    }

    fn pixel(data: &[u8], x: u32, y: u32) -> [u8; 4] {
        let at = ((y * SIZE + x) * 4) as usize;
        [data[at], data[at + 1], data[at + 2], data[at + 3]]
    }

    /// The scene both arms of the parity smoke render.
    ///
    /// Deliberate shape: two commands BEFORE the bracket (the pre segment),
    /// one composited bracket, and one command AFTER it (the trailing
    /// segment), so a frame exercises every part of a [`FramePlan`] —
    /// `vello(pre)` → quad → `vello(trailing)` → quad. Every edge lands on a
    /// whole device pixel under the 2x bracket transform, so the comparison
    /// measures placement and alpha, not rasterizer subpixel coverage.
    fn parity_scene(alpha: f32, overlay: Color) -> Scene {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            // Pre segment: an opaque black backdrop (which makes the composite
            // arithmetic below exact) plus a marker outside the bracket, so a
            // dropped pre segment is visible.
            builder.fill_rect(
                Rect::new(0.0, 0.0, f64::from(SIZE), f64::from(SIZE)),
                Brush::Solid(BLACK),
            );
            builder.fill_rect(Rect::new(52.0, 52.0, 60.0, 60.0), Brush::Solid(BLUE));
            // The bracket: a 20x20 body under a 2x device scale offset by
            // (8, 8), so its rect covers device (8, 8)..(48, 48).
            bracket_body(&mut builder, alpha);
            // Trailing segment: an opaque overlay ON TOP of the page.
            builder.fill_rect(Rect::new(20.0, 20.0, 30.0, 30.0), Brush::Solid(overlay));
        }
        scene
    }

    /// The bracket body [`parity_scene`] draws, recorded under a 2x device
    /// scale offset by (8, 8) so it covers device (8, 8)..(48, 48).
    fn bracket_body(builder: &mut SceneBuilder<'_>, alpha: f32) {
        builder.push_transform(Affine::translate((8.0, 8.0)) * Affine::scale(2.0));
        builder.push_snapshot(1, Rect::new(0.0, 0.0, 20.0, 20.0), alpha, 1.0);
        builder.fill_rect(Rect::new(0.0, 0.0, 20.0, 10.0), Brush::Solid(RED));
        builder.fill_rect(
            Rect::new(0.0, 10.0, 20.0, 15.0),
            Brush::Solid(GREEN.with_alpha(0.5)),
        );
        builder.fill_rect(Rect::new(5.0, 15.0, 15.0, 20.0), Brush::Solid(BLACK));
        builder.pop_snapshot();
        builder.pop_transform();
    }

    /// The MEASURED page-transition shape: a clip, one composited bracket, the
    /// matching pop — and nothing anywhere that paints outside the bracket.
    /// Both segments are then pure structure, so the composited arm skips the
    /// main vello pass and the compositor's own pass clears the frame to the
    /// base colour and draws the page onto it.
    ///
    /// `clip` is the app-root clip's rect in device pixels; a clip NARROWER
    /// than the page is what proves the scissor.
    fn clipped_bracket_scene(clip: Rect, alpha: f32) -> Scene {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_clip(clip);
            bracket_body(&mut builder, alpha);
            builder.pop_clip();
        }
        scene
    }

    /// The navigator's TRAILING shape: an app-root clip, one composited
    /// bracket, an overlay recorded after it (the trailing segment), and the
    /// matching pop — so the trailing pass starts inside a clip it did not
    /// open. `clip` is in device pixels; a clip shorter than `overlay` is what
    /// proves the segment re-establishes it.
    fn clipped_trailing_scene(clip: Rect, overlay: Rect, color: Color) -> Scene {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_clip(clip);
            bracket_body(&mut builder, 1.0);
            builder.fill_rect(overlay, Brush::Solid(color));
            builder.pop_clip();
        }
        scene
    }

    /// Render `scene` through `cache`'s plan, compositing with `compositor`
    /// when the plan has layers — the blit arm's frame, minus the blit.
    async fn render_frame(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut vello::Renderer,
        cache: &mut SnapshotCache,
        compositor: &mut Compositor,
        scene: &Scene,
        base_color: Color,
    ) -> Vec<u8> {
        let adapter_max = device.limits().max_texture_dimension_2d;
        let (texture, view) = surface_target(device);
        let plan = cache.prepare(device, queue, renderer, scene, adapter_max, (SIZE, SIZE));
        let shader_images: HashMap<(u64, u32, u32), ImageData> = HashMap::new();
        let params = |base_color| vello::RenderParams {
            base_color,
            width: SIZE,
            height: SIZE,
            antialiasing_method: vello::AaConfig::Area,
        };

        // The renderer's own rule (`renderer::skips_main_pass`), mirrored: a
        // pre segment that paints nothing gets no vello pass, and the
        // composite pass clears the target to the base colour instead.
        let clear = (!plan.pre_draws && !plan.layers.is_empty()).then_some(base_color);
        let mut vello_scene = vello::Scene::new();
        encode_range_with_overrides(
            scene,
            &plan.pre,
            &mut vello_scene,
            &shader_images,
            adapter_max,
            &plan.holes,
        );
        if clear.is_none() {
            renderer
                .render_to_texture(device, queue, &vello_scene, &view, &params(base_color))
                .expect("pre-segment render failed");
        }

        // The segment carries its own prefix, so this helper cannot forward a
        // range without the groups that pass must re-open — the render path
        // hands `convert` exactly this value too.
        let trailing = plan.trailing.as_ref().map(|segment| {
            let scratch = compositor.ensure_scratch(device, SIZE, SIZE).clone();
            let mut trailing_scene = vello::Scene::new();
            encode_range_with_overrides(
                scene,
                segment,
                &mut trailing_scene,
                &shader_images,
                adapter_max,
                &plan.holes,
            );
            renderer
                .render_to_texture(
                    device,
                    queue,
                    &trailing_scene,
                    &scratch,
                    &params(TRANSPARENT),
                )
                .expect("trailing-segment render failed");
            scratch
        });

        compositor.composite(
            device,
            queue,
            CompositeTarget {
                view: &view,
                size: (SIZE, SIZE),
                output: OutputAlpha::Straight,
                clear,
            },
            &plan.layers,
            trailing.as_ref(),
        );
        // The render path ages the scratch once per encoded frame, trailing
        // segment or not; a helper that skipped it would never exercise the
        // aging these smokes run through.
        compositor.age_scratch();
        read_back(device, queue, &texture).await
    }

    /// The whole seam, pixel for pixel: a bracket rasterized into its own
    /// texture and drawn back as one composited quad must land the same pixels
    /// as the same bracket lowered inline (the kill switch's path).
    ///
    /// This is the test that pins the alpha decision. vello writes STRAIGHT
    /// (un-premultiplied) alpha into a target, so the fragment stage
    /// premultiplies as it samples; running the crate's premultiply compute
    /// pass over a page first would premultiply twice and halve the body's
    /// 50%-alpha band. It equally pins the placement: the page's pixel grid
    /// must cover the bracket's local rect exactly, or the arms disagree
    /// everywhere rather than at edges. And it pins the z-order of the
    /// trailing segment: the overlay recorded after the bracket must survive
    /// on top of the composited page.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn snapshot_layer_composites_pixel_identically() {
        pollster::block_on(run());

        async fn run() {
            let (device, queue) = gpu().await;
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
                .expect("failed to create vello renderer");
            let mut compositor = Compositor::new(&device, wgpu::TextureFormat::Rgba8Unorm, None)
                .expect("compositor");

            let overlay = Color::from_rgba8(255, 255, 0, 255);
            let scene = parity_scene(0.75, overlay);

            let mut cached = SnapshotCache::new(true);
            let composited = render_frame(
                &device,
                &queue,
                &mut renderer,
                &mut cached,
                &mut compositor,
                &scene,
                BLACK,
            )
            .await;
            // The kill switch: one whole-scene vello pass, no layers, no
            // compositor work — the arm exactly as it was before this module.
            let mut disabled = SnapshotCache::new(false);
            let inline = render_frame(
                &device,
                &queue,
                &mut renderer,
                &mut disabled,
                &mut compositor,
                &scene,
                BLACK,
            )
            .await;

            let (mut worst, mut worst_at) = (0u8, (0, 0));
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let delta = (0..4)
                        .map(|c| pixel(&inline, x, y)[c].abs_diff(pixel(&composited, x, y)[c]))
                        .max()
                        .unwrap_or(0);
                    if delta > worst {
                        worst = delta;
                        worst_at = (x, y);
                    }
                }
            }
            println!("compositor parity: max per-pixel delta {worst} at {worst_at:?}");
            assert!(
                worst <= TOLERANCE,
                "composited and inline arms disagree by {worst} at {worst_at:?} (tolerance \
                 {TOLERANCE}): the alpha handling or the quad placement is wrong. inline={:?} \
                 composited={:?}",
                pixel(&inline, worst_at.0, worst_at.1),
                pixel(&composited, worst_at.0, worst_at.1),
            );

            // Absolute checks, so a change that broke BOTH arms identically
            // still fails. The values are the composite arithmetic over the
            // black backdrop, not observations:
            //  - the opaque red block at the bracket's 0.75 -> 191,
            //  - the half-alpha green band (CSS green is 0x008000) at
            //    128 * 0.5 * 0.75 -> 48, the number a page premultiplied a
            //    second time before compositing would halve again.
            for (arm, data) in [("inline", &inline), ("composited", &composited)] {
                let red = pixel(data, 40, 12);
                assert!(
                    red[0].abs_diff(191) <= TOLERANCE,
                    "{arm} arm must draw the opaque body block at the bracket's alpha, got {red:?}"
                );
                let green = pixel(data, 40, 32);
                assert!(
                    green[1].abs_diff(48) <= TOLERANCE,
                    "{arm} arm must draw the body's half-alpha band at 128 * 0.5 * 0.75, got \
                     {green:?} (a doubly premultiplied page halves this)"
                );
                // The trailing segment sits ABOVE the composited page.
                let over = pixel(data, 24, 24);
                assert_eq!(
                    over,
                    [255, 255, 0, 255],
                    "{arm} arm must keep the overlay recorded after the bracket on top"
                );
                // The pre segment survives the split.
                let marker = pixel(data, 56, 56);
                assert!(
                    marker[2] > 200,
                    "{arm} arm must keep the pre-segment marker, got {marker:?}"
                );
            }

            let error = scope.pop().await;
            assert!(
                error.is_none(),
                "the compositor produced an uncaptured validation error: {error:?}"
            );
        }
    }

    /// The same parity, for the frame shape the device gate actually measured:
    /// a clip, the composited bracket, the matching pop, and nothing that
    /// paints outside the bracket anywhere. The composited arm therefore runs
    /// NO vello pass at all — the compositor clears the target to the base
    /// colour and draws the page — and must still land the inline arm's
    /// pixels, base colour included.
    ///
    /// The base colour is deliberately not black: the clear is the only thing
    /// writing every pixel outside the page here, so a wrong colour conversion
    /// (or a wrong premultiplication) shows up as a whole-frame delta rather
    /// than hiding in zeroes.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn snapshot_layer_composites_pixel_identically_without_a_main_pass() {
        pollster::block_on(run());

        async fn run() {
            let (device, queue) = gpu().await;
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
                .expect("failed to create vello renderer");
            let mut compositor = Compositor::new(&device, wgpu::TextureFormat::Rgba8Unorm, None)
                .expect("compositor");

            let base = Color::from_rgba8(32, 96, 160, 255);
            let full = Rect::new(0.0, 0.0, f64::from(SIZE), f64::from(SIZE));
            let scene = clipped_bracket_scene(full, 0.75);

            let mut cached = SnapshotCache::new(true);
            let plan = cached.prepare(
                &device,
                &queue,
                &mut renderer,
                &scene,
                device.limits().max_texture_dimension_2d,
                (SIZE, SIZE),
            );
            assert_eq!(
                plan.pre.range,
                0..1,
                "the pre segment is the PushClip alone"
            );
            assert!(
                !plan.pre_draws && plan.trailing.is_none(),
                "neither segment paints, so neither is worth a vello pass"
            );
            assert_eq!(plan.layers.len(), 1);
            assert_eq!(
                plan.layers[0].clip,
                Some(full),
                "the page carries the clip enclosing it"
            );

            let composited = render_frame(
                &device,
                &queue,
                &mut renderer,
                &mut cached,
                &mut compositor,
                &scene,
                base,
            )
            .await;
            let mut disabled = SnapshotCache::new(false);
            let inline = render_frame(
                &device,
                &queue,
                &mut renderer,
                &mut disabled,
                &mut compositor,
                &scene,
                base,
            )
            .await;

            let (mut worst, mut worst_at) = (0u8, (0, 0));
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let delta = (0..4)
                        .map(|c| pixel(&inline, x, y)[c].abs_diff(pixel(&composited, x, y)[c]))
                        .max()
                        .unwrap_or(0);
                    if delta > worst {
                        worst = delta;
                        worst_at = (x, y);
                    }
                }
            }
            println!("skipped-main-pass parity: max per-pixel delta {worst} at {worst_at:?}");
            assert!(
                worst <= TOLERANCE,
                "the skipped-main-pass arm disagrees by {worst} at {worst_at:?} (tolerance \
                 {TOLERANCE}): inline={:?} composited={:?}",
                pixel(&inline, worst_at.0, worst_at.1),
                pixel(&composited, worst_at.0, worst_at.1),
            );
            // The base colour outside the page comes from the compositor's own
            // clear on one arm and vello's on the other.
            let want = [32, 96, 160, 255];
            for (arm, data) in [("inline", &inline), ("composited", &composited)] {
                let outside = pixel(data, 56, 56);
                for channel in 0..4 {
                    assert!(
                        outside[channel].abs_diff(want[channel]) <= TOLERANCE,
                        "{arm} arm must leave the base colour outside the page: got {outside:?}, \
                         want {want:?}"
                    );
                }
                let red = pixel(data, 40, 12);
                assert!(
                    red[0].abs_diff(191) <= TOLERANCE + 40,
                    "{arm} arm must draw the body block at the bracket's alpha over the base, \
                     got {red:?}"
                );
            }

            let error = scope.pop().await;
            assert!(
                error.is_none(),
                "the compositor produced an uncaptured validation error: {error:?}"
            );
        }
    }

    /// The scissor, end to end: a page whose quad is wider than the clip it
    /// was recorded under must leave every pixel outside that clip at the base
    /// colour — the quad has left vello, so this pass's scissor is the only
    /// thing still applying the clip.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn a_clipped_page_paints_nothing_outside_its_clip() {
        pollster::block_on(run());

        async fn run() {
            let (device, queue) = gpu().await;
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
                .expect("failed to create vello renderer");
            let mut compositor = Compositor::new(&device, wgpu::TextureFormat::Rgba8Unorm, None)
                .expect("compositor");

            // The page covers device (8, 8)..(48, 48); the clip cuts it at
            // x = 28, so the right half of the page must never be drawn.
            let base = Color::from_rgba8(32, 96, 160, 255);
            let narrow = Rect::new(0.0, 0.0, 28.0, f64::from(SIZE));
            let scene = clipped_bracket_scene(narrow, 1.0);

            let mut cached = SnapshotCache::new(true);
            let composited = render_frame(
                &device,
                &queue,
                &mut renderer,
                &mut cached,
                &mut compositor,
                &scene,
                base,
            )
            .await;

            let want = [32, 96, 160, 255];
            // Just outside the clip but well inside the page: base colour.
            for (x, y) in [(29, 20), (40, 12), (47, 40), (30, 30)] {
                let got = pixel(&composited, x, y);
                for channel in 0..4 {
                    assert!(
                        got[channel].abs_diff(want[channel]) <= TOLERANCE,
                        "({x}, {y}) is outside the clip and must stay at the base colour: got \
                         {got:?}, want {want:?}"
                    );
                }
            }
            // Inside the clip the page is drawn: its opaque red band.
            let inside = pixel(&composited, 20, 12);
            assert!(
                inside[0] > 200 && inside[1] < 80,
                "the clipped page must still draw inside its clip, got {inside:?}"
            );

            let error = scope.pop().await;
            assert!(
                error.is_none(),
                "the compositor produced an uncaptured validation error: {error:?}"
            );
        }
    }

    /// The trailing segment's own clip, end to end. The scene is the
    /// navigator's shape: an app-root clip, a composited page, an overlay
    /// recorded AFTER the page (so it lands in the trailing segment), and the
    /// matching pop.
    ///
    /// The clip is deliberately SHORTER than the overlay. Without the trailing
    /// [`Segment`]'s own prefix the trailing pass encodes the overlay alone,
    /// under no clip at all, and paints it over the whole overlay rect — including the
    /// part the scene had clipped away — while its in-range `PopClip` pops a
    /// group that pass never pushed. Both arms must agree, and outside the
    /// clip both must be the untouched base colour.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn a_trailing_segment_keeps_the_clip_it_was_recorded_under() {
        pollster::block_on(run());

        async fn run() {
            let (device, queue) = gpu().await;
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
                .expect("failed to create vello renderer");
            let mut compositor = Compositor::new(&device, wgpu::TextureFormat::Rgba8Unorm, None)
                .expect("compositor");

            // The viewport stops at y = 28; the overlay would reach y = 40.
            let base = Color::from_rgba8(32, 96, 160, 255);
            let overlay_color = Color::from_rgba8(255, 255, 0, 255);
            let clip = Rect::new(0.0, 0.0, f64::from(SIZE), 28.0);
            let overlay = Rect::new(20.0, 20.0, 40.0, 40.0);
            let scene = clipped_trailing_scene(clip, overlay, overlay_color);

            let mut cached = SnapshotCache::new(true);
            let plan = cached.prepare(
                &device,
                &queue,
                &mut renderer,
                &scene,
                device.limits().max_texture_dimension_2d,
                (SIZE, SIZE),
            );
            assert_eq!(plan.layers.len(), 1, "the page composites");
            assert_eq!(
                plan.trailing,
                Some(Segment {
                    range: 6..8,
                    // The app-root clip is open where that pass starts, and
                    // travels WITH the range rather than beside it.
                    prefix: vec![0],
                }),
                "the overlay after the page needs a trailing pass, re-opening \
                 the clip it was recorded under"
            );

            let composited = render_frame(
                &device,
                &queue,
                &mut renderer,
                &mut cached,
                &mut compositor,
                &scene,
                base,
            )
            .await;
            let mut disabled = SnapshotCache::new(false);
            let inline = render_frame(
                &device,
                &queue,
                &mut renderer,
                &mut disabled,
                &mut compositor,
                &scene,
                base,
            )
            .await;

            let (mut worst, mut worst_at) = (0u8, (0, 0));
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let delta = (0..4)
                        .map(|c| pixel(&inline, x, y)[c].abs_diff(pixel(&composited, x, y)[c]))
                        .max()
                        .unwrap_or(0);
                    if delta > worst {
                        worst = delta;
                        worst_at = (x, y);
                    }
                }
            }
            println!("trailing-clip parity: max per-pixel delta {worst} at {worst_at:?}");
            assert!(
                worst <= TOLERANCE,
                "the trailing segment's arms disagree by {worst} at {worst_at:?} (tolerance \
                 {TOLERANCE}): inline={:?} composited={:?}",
                pixel(&inline, worst_at.0, worst_at.1),
                pixel(&composited, worst_at.0, worst_at.1),
            );

            let want_base = [32, 96, 160, 255];
            for (arm, data) in [("inline", &inline), ("composited", &composited)] {
                // The overlay's own pixels, below the clip: nothing may paint
                // there — this is the assertion an unclipped trailing pass
                // fails.
                for (x, y) in [(30, 30), (30, 39), (24, 34)] {
                    let got = pixel(data, x, y);
                    for channel in 0..4 {
                        assert!(
                            got[channel].abs_diff(want_base[channel]) <= TOLERANCE,
                            "{arm} arm painted ({x}, {y}) = {got:?} outside the clip the \
                             overlay was recorded under; want the base {want_base:?}"
                        );
                    }
                }
                // Inside the clip the overlay is drawn, above the page.
                let over = pixel(data, 30, 24);
                assert_eq!(
                    over,
                    [255, 255, 0, 255],
                    "{arm} arm must draw the overlay inside the clip"
                );
                // ...and the page under it survives where the overlay is not.
                let page = pixel(data, 12, 12);
                assert!(
                    page[0] > 200 && page[1] < 80,
                    "{arm} arm must keep the composited page inside the clip, got {page:?}"
                );
            }

            let error = scope.pop().await;
            assert!(
                error.is_none(),
                "the compositor produced an uncaptured validation error: {error:?}"
            );
        }
    }

    /// R4's structural check for a scaled page: at scale 0.9 the quad must
    /// contract about its rect's center and paint NOTHING outside the
    /// contracted rect. Bilinear resampling makes a per-pixel comparison
    /// against the inline arm meaningless here, so coverage and bounding box
    /// are what is asserted.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn snapshot_layer_scaled_quad_stays_inside_its_rect() {
        pollster::block_on(run());

        async fn run() {
            let (device, queue) = gpu().await;
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut compositor = Compositor::new(&device, wgpu::TextureFormat::Rgba8Unorm, None)
                .expect("compositor");
            let (texture, view) = surface_target(&device);
            clear(&device, &queue, &view, wgpu::Color::BLACK);

            // A page covering device (8, 8)..(48, 48), contracted about its
            // center (28, 28) by 0.9 -> (10, 10)..(46, 46) — whole pixels, so
            // the quad's edges need no tolerance of their own.
            let layer = CompositeLayer {
                key: 1,
                rect: Rect::new(0.0, 0.0, 20.0, 20.0),
                alpha: 1.0,
                scale: 0.9,
                transform: Affine::translate((8.0, 8.0)) * Affine::scale(2.0),
                width: 40,
                height: 40,
                clip: None,
                view: page_of(&device, &queue, 40, [255, 0, 0, 255]),
            };
            compositor.composite(
                &device,
                &queue,
                CompositeTarget {
                    view: &view,
                    size: (SIZE, SIZE),
                    output: OutputAlpha::Straight,
                    clear: None,
                },
                std::slice::from_ref(&layer),
                None,
            );
            let data = read_back(&device, &queue, &texture).await;

            let contracted = Rect::new(10.0, 10.0, 46.0, 46.0);
            let mut bounds: Option<(u32, u32, u32, u32)> = None;
            for y in 0..SIZE {
                for x in 0..SIZE {
                    if pixel(&data, x, y) == [0, 0, 0, 255] {
                        continue;
                    }
                    let inside = f64::from(x) + 1.0 > contracted.x0
                        && f64::from(x) < contracted.x1
                        && f64::from(y) + 1.0 > contracted.y0
                        && f64::from(y) < contracted.y1;
                    assert!(
                        inside,
                        "a scaled quad painted ({x}, {y}) = {:?} outside {contracted:?}",
                        pixel(&data, x, y)
                    );
                    bounds = Some(match bounds {
                        None => (x, y, x + 1, y + 1),
                        Some((x0, y0, x1, y1)) => {
                            (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1))
                        }
                    });
                }
            }
            let (x0, y0, x1, y1) = bounds.expect("the scaled quad painted nothing at all");
            for (label, got, want) in [
                ("x0", x0, 10),
                ("y0", y0, 10),
                ("x1", x1, 46),
                ("y1", y1, 46),
            ] {
                assert!(
                    got.abs_diff(want) <= 1,
                    "scaled quad {label} = {got}, expected {want} +-1 (bbox \
                     {x0},{y0}..{x1},{y1})"
                );
            }

            let error = scope.pop().await;
            assert!(
                error.is_none(),
                "the compositor produced an uncaptured validation error: {error:?}"
            );
        }
    }

    /// R5: onto a premultiplied (translucent) swapchain the quad must write
    /// exactly what `PremultiplyPass` would have — `(rgb * a * alpha, a *
    /// alpha)` — and leave a fully transparent page texel untouched.
    ///
    /// The page here is written texel-by-texel rather than rasterized, so the
    /// comparison is against the formula itself with no vello rounding in
    /// between.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn snapshot_layer_premultiplied_output_matches_formula() {
        pollster::block_on(run());

        async fn run() {
            let (device, queue) = gpu().await;
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut compositor = Compositor::new(&device, wgpu::TextureFormat::Rgba8Unorm, None)
                .expect("compositor");
            let (texture, view) = surface_target(&device);
            clear(&device, &queue, &view, wgpu::Color::TRANSPARENT);

            // Straight (0, 128, 0) at a = 128/255, composited at 0.75.
            let alpha = 0.75_f32;
            let layer = CompositeLayer {
                key: 1,
                rect: Rect::new(8.0, 8.0, 48.0, 48.0),
                alpha,
                scale: 1.0,
                transform: Affine::IDENTITY,
                width: 40,
                height: 40,
                clip: None,
                view: page_of(&device, &queue, 40, [0, 128, 0, 128]),
            };
            compositor.composite(
                &device,
                &queue,
                CompositeTarget {
                    view: &view,
                    size: (SIZE, SIZE),
                    output: OutputAlpha::Premultiplied,
                    clear: None,
                },
                std::slice::from_ref(&layer),
                None,
            );
            let data = read_back(&device, &queue, &texture).await;

            let out_alpha = (128.0 / 255.0) * f64::from(alpha);
            let want = [
                0,
                (128.0 * out_alpha).round() as u8,
                0,
                (out_alpha * 255.0).round() as u8,
            ];
            let got = pixel(&data, 28, 28);
            for channel in 0..4 {
                assert!(
                    got[channel].abs_diff(want[channel]) <= 2,
                    "premultiplied output {got:?} must equal (rgb * a * alpha, a * alpha) = \
                     {want:?}"
                );
            }
            assert_eq!(
                pixel(&data, 2, 2),
                [0, 0, 0, 0],
                "a pixel outside the quad must stay untouched on a transparent target"
            );

            let error = scope.pop().await;
            assert!(
                error.is_none(),
                "the compositor produced an uncaptured validation error: {error:?}"
            );
        }
    }

    /// Two identical frames composite the same page twice — the steady state
    /// of an animating bracket, seen from the compositor's side: the plan
    /// yields a layer, a hole and a trailing pass every frame, and the scratch
    /// is allocated once. That the SECOND frame performs no rasterization is
    /// `snapshot.rs`'s own steady-state test (the render counter it asserts is
    /// private to that module).
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn steady_state_frames_composite_the_page_every_frame() {
        pollster::block_on(run());

        async fn run() {
            let (device, queue) = gpu().await;
            let mut renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
                .expect("failed to create vello renderer");
            let mut compositor = Compositor::new(&device, wgpu::TextureFormat::Rgba8Unorm, None)
                .expect("compositor");
            let mut cache = SnapshotCache::new(true);
            let overlay = Color::from_rgba8(255, 255, 0, 255);

            let mut composites = 0;
            for _ in 0..2 {
                let scene = parity_scene(0.75, overlay);
                let plan = cache.prepare(
                    &device,
                    &queue,
                    &mut renderer,
                    &scene,
                    device.limits().max_texture_dimension_2d,
                    (SIZE, SIZE),
                );
                assert_eq!(
                    plan.layers.len(),
                    1,
                    "the bracket must composite every frame"
                );
                assert_eq!(plan.holes, HashSet::from([1]));
                assert!(
                    plan.trailing.is_some(),
                    "the overlay after the bracket needs a trailing pass"
                );
                composites += plan.layers.len();
            }
            assert_eq!(composites, 2, "one composited quad per frame");
            // The scratch is allocated once and re-used at a stable size, so a
            // steady frame allocates no texture at all — even though every
            // frame ages it.
            let first = compositor.ensure_scratch(&device, SIZE, SIZE).clone();
            compositor.age_scratch();
            let second = compositor.ensure_scratch(&device, SIZE, SIZE).clone();
            compositor.age_scratch();
            assert_eq!(
                first, second,
                "a same-size frame must not re-allocate the scratch"
            );
            // Frames that need no trailing segment age it out instead of
            // holding a full-surface texture until the surface dies.
            for _ in 0..=MAX_UNUSED_FRAMES {
                compositor.age_scratch();
            }
            assert!(
                !compositor.has_scratch(),
                "an unused scratch must not outlive the frames that need it"
            );
            let resized = compositor.ensure_scratch(&device, SIZE / 2, SIZE).clone();
            assert_ne!(resized, second, "a resize must re-create the scratch");
        }
    }
}
