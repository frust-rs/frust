//! The perspective quad renderer: the one piece of GPU work this substrate
//! actually does.
//!
//! A 3D component describes its frame as a [`Quad3dScene`] — a handful of
//! [`Quad3d`]s, each a rectangle in the target's own texel space plus a
//! rotation and a face — and this module turns that into one render pass into
//! an offscreen target the engine then composites. Everything above it
//! (pooling, scheduling, acquisition) exists to get a device, a target and a
//! frame to this function.
//!
//! # The camera, and why an unrotated quad is pixel-exact
//!
//! World space here is chosen so the substrate degrades cleanly: the target's
//! height is `1.0` world unit, its width is `aspect` units, and the camera
//! sits at [`CAMERA_DISTANCE`] on `+Z` looking down `-Z` with a vertical
//! field of view of [`FOV_Y_RADIANS`]. [`CAMERA_DISTANCE`] is exactly the
//! distance at which one world unit of height fills the frustum vertically,
//! so **a quad covering the whole target with no rotation projects onto
//! exactly the whole target** — the 3D path at zero tilt is the 2D rectangle,
//! not an approximation of it. That property is what lets a component tilt
//! from rest without a visible jump, and it is pinned by a test rather than
//! left as an intention.
//!
//! A narrow field of view (30 degrees) is the card-tilt tuning: wide-angle
//! perspective exaggerates foreshortening into a fish-eye at the small tilt
//! angles a card, a wallet stack or a wheel actually uses. [`NEAR`]/[`FAR`]
//! bracket that shallow scene with room to spare rather than being fitted to
//! it, so a component pushing a face along `Z` does not fall out of the
//! frustum.
//!
//! # Alpha
//!
//! The target is cleared transparent and every face is blended into it with
//! premultiplied factors (`One`/`OneMinusSrcAlpha`), so what the target holds
//! is premultiplied colour — the convention every paint in an engine frame
//! travels in, and what an externally bound texture is unconditionally
//! composited as (`docs/LIMITATIONS.md`'s
//! `engine-scene-texture-always-blended`). Every [`QuadFace`] colour is
//! premultiplied on the way into the uniform buffer, in one place, so a face
//! author never has to think about it; a [`QuadFace::Texture`] view is
//! assumed already premultiplied, which is what an engine-produced texture
//! is.
//!
//! # Depth
//!
//! [`Quad3dScene::depth`] turns on a [`DEPTH_FORMAT`] attachment and
//! `LessEqual` depth testing with depth writes — what a cylinder or a stacked
//! wallet needs so a face behind another is hidden by it rather than by
//! painter order. It is opt-in because depth and blending interact badly for
//! *translucent* faces: a translucent face writes depth like an opaque one,
//! so a face behind it is rejected outright instead of showing through. A
//! scene of opaque faces wants depth on; a scene of translucent ones wants it
//! off and wants its quads submitted back-to-front.
//!
//! # No panics
//!
//! [`Quad3dRenderer::record`] runs inside an `ExternalPass`, where a panic is
//! only caught in a build that unwinds (`docs/LIMITATIONS.md`'s
//! `external-pass-panic-isolation-dev-only`). Every path here is total: a
//! non-finite transform is dropped, an empty scene records nothing, and no
//! indexing is unchecked.

use std::sync::Arc;

use peniko::Color;

use frust::authoring::Rect;

/// The offscreen colour format every 3D target is allocated and rendered in.
///
/// `Rgba8Unorm` reads back R, G, B, A in that order and takes the shader's
/// return value through unchanged, which is what makes a premultiplied
/// contract checkable by reading a texel rather than by trusting a pipeline.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The depth format a [`Quad3dScene::depth`] target allocates.
///
/// `Depth24Plus` is the universally supported depth-only format; nothing here
/// samples depth or needs stencil, so the cheapest portable choice is the
/// right one.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

/// Vertical field of view, in radians — 30 degrees. See the module docs for
/// why it is narrow.
pub const FOV_Y_RADIANS: f32 = std::f32::consts::FRAC_PI_6;

/// Near clip plane, in world units (one unit is the target's height).
pub const NEAR: f32 = 0.1;

/// Far clip plane, in world units.
pub const FAR: f32 = 100.0;

/// Camera distance from the world origin along `+Z`, in world units.
///
/// `0.5 / tan(FOV_Y_RADIANS / 2)` — the distance at which one world unit of
/// height exactly fills the frustum, which is what makes an unrotated
/// full-target quad project onto exactly the target. Written as a literal
/// because `tan` is not a `const fn`; the arithmetic is pinned by
/// `camera_distance_fills_the_frustum_vertically`.
pub const CAMERA_DISTANCE: f32 = 1.866_025_4;

/// Bytes one quad's uniform record occupies in the shader.
const UNIFORM_SIZE: u64 = 128;

/// Stride between two quads' uniform records, satisfying wgpu's largest
/// guaranteed `min_uniform_buffer_offset_alignment` (256) so a dynamic offset
/// is always legal without probing the adapter for a smaller one.
const UNIFORM_STRIDE: u64 = 256;

/// What one quad's face is filled with.
///
/// The three sources a component can actually reach: a flat colour, a linear
/// ramp between two, and a caller-owned texture. Colours are authored
/// straight (non-premultiplied) — the renderer premultiplies on the way to
/// the GPU, once, in [`premultiplied`].
#[derive(Clone, Debug)]
pub enum QuadFace {
    /// A flat fill.
    Solid(Color),
    /// A linear ramp from `from` at one edge to `to` at the other, along
    /// `angle_radians` measured clockwise from the face's `+X` axis (`0` runs
    /// left to right, `PI / 2` runs top to bottom).
    Gradient {
        /// Colour at the start of the ramp.
        from: Color,
        /// Colour at the end of the ramp.
        to: Color,
        /// Ramp direction, radians clockwise from the face's `+X` axis.
        angle_radians: f32,
    },
    /// A texture the caller owns and keeps alive, sampled over the face with
    /// `opacity` applied on top.
    ///
    /// The view must be a non-array 2D view of a float-sampleable texture
    /// carrying `wgpu::TextureUsages::TEXTURE_BINDING`, and its texels are
    /// assumed **premultiplied** — the same assumption the engine makes of
    /// every externally bound texture.
    Texture {
        /// The texture to sample.
        view: Arc<wgpu::TextureView>,
        /// Uniform opacity applied to the sampled texel, `0.0..=1.0`.
        opacity: f32,
    },
}

/// One textured, rotated rectangle.
///
/// `dest` is in the target's own texel space (origin top-left, `y` down),
/// exactly like a 2D destination rectangle, and the rotations are applied
/// about the rectangle's own centre in `Z`, then `Y`, then `X` order —
/// roll, yaw, pitch as a designer names them.
#[derive(Clone, Debug)]
pub struct Quad3d {
    /// Where the face sits when it is not rotated, in target texels.
    pub dest: Rect,
    /// What fills it.
    pub face: QuadFace,
    /// Rotation about the vertical axis, radians. Positive brings the face's
    /// left edge toward the camera.
    pub yaw: f32,
    /// Rotation about the horizontal axis, radians. Positive brings the
    /// face's bottom edge toward the camera.
    pub pitch: f32,
    /// Rotation in the face's own plane, radians, clockwise on screen.
    pub roll: f32,
    /// Displacement along `+Z` (toward the camera) in world units, one unit
    /// being the target's height. The knob a stacked or cylindrical
    /// arrangement separates its faces with.
    pub depth_offset: f32,
}

impl Quad3d {
    /// An unrotated face at `dest`.
    #[must_use]
    pub const fn new(dest: Rect, face: QuadFace) -> Self {
        Self {
            dest,
            face,
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            depth_offset: 0.0,
        }
    }

    /// Sets [`Self::yaw`].
    #[must_use]
    pub const fn yaw(mut self, radians: f32) -> Self {
        self.yaw = radians;
        self
    }

    /// Sets [`Self::pitch`].
    #[must_use]
    pub const fn pitch(mut self, radians: f32) -> Self {
        self.pitch = radians;
        self
    }

    /// Sets [`Self::roll`].
    #[must_use]
    pub const fn roll(mut self, radians: f32) -> Self {
        self.roll = radians;
        self
    }

    /// Sets [`Self::depth_offset`].
    #[must_use]
    pub const fn depth_offset(mut self, offset: f32) -> Self {
        self.depth_offset = offset;
        self
    }

    /// Whether every number describing this quad is finite and its rectangle
    /// has area — a quad that fails this is dropped rather than handed to the
    /// GPU as a `NaN` transform.
    #[must_use]
    pub fn is_renderable(&self) -> bool {
        self.dest.width() > 0.0
            && self.dest.height() > 0.0
            && [
                self.dest.x0,
                self.dest.y0,
                self.dest.x1,
                self.dest.y1,
                f64::from(self.yaw),
                f64::from(self.pitch),
                f64::from(self.roll),
                f64::from(self.depth_offset),
            ]
            .iter()
            .all(|value| value.is_finite())
    }
}

/// One frame's worth of 3D content for one component.
#[derive(Clone, Debug, Default)]
pub struct Quad3dScene {
    /// The faces, in submission order. With [`Self::depth`] off that order is
    /// painter order; with it on, order only decides ties.
    pub quads: Vec<Quad3d>,
    /// Whether the target carries a depth attachment and faces occlude each
    /// other by distance. See the module docs' *Depth*.
    pub depth: bool,
}

impl Quad3dScene {
    /// An empty scene with depth off.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Turns depth testing on or off for this scene.
    #[must_use]
    pub fn with_depth(mut self, depth: bool) -> Self {
        self.depth = depth;
        self
    }

    /// Appends a face.
    pub fn push(&mut self, quad: Quad3d) {
        self.quads.push(quad);
    }

    /// Appends a face, builder style.
    #[must_use]
    pub fn with(mut self, quad: Quad3d) -> Self {
        self.quads.push(quad);
        self
    }

    /// How many faces this scene holds, renderable or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.quads.len()
    }

    /// Whether the scene holds no face at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.quads.is_empty()
    }
}

/// The WGSL both pipelines are built from: a vertex-buffer-free quad whose
/// four corners come from `vertex_index`, and a fragment stage branching on
/// the face kind packed into the per-quad uniform.
const SHADER: &str = r#"
struct Quad {
    mvp: mat4x4<f32>,
    color_a: vec4<f32>,
    color_b: vec4<f32>,
    params: vec4<f32>,
    kind: vec4<u32>,
};

@group(0) @binding(0) var<uniform> quad: Quad;
@group(1) @binding(0) var face_texture: texture_2d<f32>;
@group(1) @binding(1) var face_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(-0.5, -0.5),
        vec2<f32>( 0.5, -0.5),
        vec2<f32>(-0.5,  0.5),
        vec2<f32>( 0.5,  0.5),
    );
    var order = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u);
    let corner = corners[order[index]];
    var out: VsOut;
    out.position = quad.mvp * vec4<f32>(corner.x, corner.y, 0.0, 1.0);
    out.uv = vec2<f32>(corner.x + 0.5, 0.5 - corner.y);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    switch quad.kind.x {
        case 1u: {
            let axis = vec2<f32>(quad.params.x, quad.params.y);
            let t = clamp(dot(in.uv - vec2<f32>(0.5, 0.5), axis) + 0.5, 0.0, 1.0);
            return mix(quad.color_a, quad.color_b, t);
        }
        case 2u: {
            return textureSample(face_texture, face_sampler, in.uv) * quad.params.z;
        }
        default: {
            return quad.color_a;
        }
    }
}
"#;

/// Where one [`Quad3dRenderer::record`] call draws: the attachments it owns,
/// and the sub-rect of them the component actually asked for.
///
/// The colour view may belong to a texture larger than `requested` — a pooled
/// target is allocated at a quantized extent — so `requested` is what the
/// viewport confines the draw to and what the engine maps the destination
/// rectangle onto. `depth` is `Some` only for a target allocated with a depth
/// attachment; it is never the *frame's* depth, which an `ExternalPass` is
/// never handed.
#[derive(Clone, Copy)]
pub struct Quad3dTarget<'a> {
    /// The colour attachment the faces are drawn into.
    pub color: &'a wgpu::TextureView,
    /// The depth attachment, when this target has one.
    pub depth: Option<&'a wgpu::TextureView>,
    /// The extent, in texels, the draw is confined to and the engine maps.
    pub requested: (u32, u32),
}

/// The compiled pipelines, the uniform buffer every quad's transform is
/// staged through, and the placeholder texture a non-textured face binds.
///
/// Built once per component, on the first frame that has a device, and reused
/// for the component's whole life. Both depth variants are built up front:
/// a scene can turn depth on and off between frames, and compiling a pipeline
/// mid-frame is exactly the stall this substrate exists to avoid.
pub struct Quad3dRenderer {
    no_depth: wgpu::RenderPipeline,
    with_depth: wgpu::RenderPipeline,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    placeholder: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    uniform_group: wgpu::BindGroup,
    capacity: u64,
    label: String,
}

impl Quad3dRenderer {
    /// Compiles the shader and both pipelines on `device`.
    ///
    /// `label` names this component in adapter/validation diagnostics; it is
    /// the only place a component's identity reaches the GPU.
    #[must_use]
    pub fn new(device: &wgpu::Device, label: &str) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frust-beui gpu_fx quad3d"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frust-beui gpu_fx quad uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frust-beui gpu_fx quad face"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("frust-beui gpu_fx quad3d layout"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let no_depth = build_pipeline(device, &module, &pipeline_layout, None);
        let with_depth = build_pipeline(
            device,
            &module,
            &pipeline_layout,
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("frust-beui gpu_fx face sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let placeholder = placeholder_group(device, &texture_layout, &sampler);
        let capacity = 1;
        let uniforms = create_uniform_buffer(device, capacity);
        let uniform_group = create_uniform_group(device, &uniform_layout, &uniforms);
        Self {
            no_depth,
            with_depth,
            uniform_layout,
            texture_layout,
            sampler,
            placeholder,
            uniforms,
            uniform_group,
            capacity,
            label: label.to_owned(),
        }
    }

    /// Records `scene` into `target`, confined by a viewport to the target's
    /// requested sub-rect of a texture that may be larger.
    ///
    /// One render pass, one draw call per renderable face. The pass is opened
    /// and closed inside this call, so the caller's encoder is left finishable
    /// on every path — the shared-encoder rule an `ExternalPass` is under.
    /// A face whose transform is not finite is dropped; a scene with no
    /// renderable face still clears the target, which is what makes "the
    /// component went blank" look blank rather than frozen.
    pub fn record(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: Quad3dTarget<'_>,
        scene: &Quad3dScene,
    ) {
        let Quad3dTarget {
            color,
            depth,
            requested,
        } = target;
        let (width, height) = requested;
        if width == 0 || height == 0 {
            return;
        }
        let aspect = width as f32 / height as f32;
        let projection = perspective(aspect);
        let view = translation(0.0, 0.0, -CAMERA_DISTANCE);
        let view_projection = multiply(&projection, &view);

        let renderable: Vec<&Quad3d> = scene
            .quads
            .iter()
            .filter(|quad| quad.is_renderable())
            .collect();
        self.reserve(device, renderable.len() as u64);

        let mut bytes = Vec::with_capacity(renderable.len() * UNIFORM_STRIDE as usize);
        for quad in &renderable {
            let mvp = multiply(&view_projection, &model(quad, width, height));
            encode_uniform(&mut bytes, &mvp, &quad.face);
            bytes.resize(bytes.len().next_multiple_of(UNIFORM_STRIDE as usize), 0);
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.uniforms, 0, &bytes);
        }

        // Face bind groups are built before the pass so each one outlives the
        // borrow the pass takes of it.
        let faces: Vec<wgpu::BindGroup> = renderable
            .iter()
            .map(|quad| match &quad.face {
                QuadFace::Texture { view, .. } => {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("frust-beui gpu_fx face"),
                        layout: &self.texture_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                        ],
                    })
                }
                QuadFace::Solid(_) | QuadFace::Gradient { .. } => self.placeholder.clone(),
            })
            .collect();

        let depth_attachment = depth.map(|view| wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(1.0),
                store: wgpu::StoreOp::Discard,
            }),
            stencil_ops: None,
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(&format!("frust-beui gpu_fx pass: {}", self.label)),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: depth_attachment,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if renderable.is_empty() {
            return;
        }
        pass.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);
        pass.set_scissor_rect(0, 0, width, height);
        pass.set_pipeline(if depth.is_some() && scene.depth {
            &self.with_depth
        } else {
            &self.no_depth
        });
        for (index, face) in faces.iter().enumerate() {
            let offset = index as u64 * UNIFORM_STRIDE;
            pass.set_bind_group(0, &self.uniform_group, &[offset as u32]);
            pass.set_bind_group(1, face, &[]);
            pass.draw(0..6, 0..1);
        }
    }

    /// Grows the uniform buffer to hold `quads` records, rebuilding its bind
    /// group with it. Never shrinks: a component's face count oscillates far
    /// more often than it settles down for good.
    fn reserve(&mut self, device: &wgpu::Device, quads: u64) {
        let wanted = quads.max(1);
        if wanted <= self.capacity {
            return;
        }
        self.capacity = wanted;
        self.uniforms = create_uniform_buffer(device, wanted);
        self.uniform_group = create_uniform_group(device, &self.uniform_layout, &self.uniforms);
    }

    /// How many quads the uniform buffer can hold without regrowing.
    #[must_use]
    pub const fn capacity(&self) -> u64 {
        self.capacity
    }
}

/// Builds one of the two pipeline variants — they differ only in depth state.
fn build_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    depth: Option<wgpu::DepthStencilState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("frust-beui gpu_fx quad3d pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            // Both faces draw: a card tilted past 90 degrees shows its back,
            // and this substrate has no separate back-face material to swap
            // in, so culling would make it vanish instead.
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: depth,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                // Premultiplied source-over: the shader returns premultiplied
                // colour, so the source factor is One rather than SrcAlpha.
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// The uniform buffer for `quads` records at [`UNIFORM_STRIDE`] each.
fn create_uniform_buffer(device: &wgpu::Device, quads: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frust-beui gpu_fx quad uniforms"),
        size: quads.max(1) * UNIFORM_STRIDE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The dynamic-offset bind group over `buffer`.
fn create_uniform_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frust-beui gpu_fx quad uniforms"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: wgpu::BufferSize::new(UNIFORM_SIZE),
            }),
        }],
    })
}

/// A 1x1 opaque-white texture bound for every non-textured face.
///
/// The shader never samples it (a solid or gradient face returns before the
/// `textureSample`), but a pipeline layout is fixed and every draw has to
/// satisfy group 1 — binding one placeholder is cheaper than compiling a
/// second pipeline family without the texture binding.
fn placeholder_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust-beui gpu_fx placeholder face"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frust-beui gpu_fx placeholder face"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// A colour in the premultiplied RGBA the target's blend factors expect.
#[must_use]
pub fn premultiplied(color: Color) -> [f32; 4] {
    let [r, g, b, a] = color.components;
    let alpha = a.clamp(0.0, 1.0);
    [r * alpha, g * alpha, b * alpha, alpha]
}

/// Appends one quad's 128-byte uniform record to `bytes`.
fn encode_uniform(bytes: &mut Vec<u8>, mvp: &Mat4, face: &QuadFace) {
    for value in mvp {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let (color_a, color_b, params, kind) = match face {
        QuadFace::Solid(color) => (premultiplied(*color), [0.0; 4], [0.0; 4], 0_u32),
        QuadFace::Gradient {
            from,
            to,
            angle_radians,
        } => {
            let angle = if angle_radians.is_finite() {
                *angle_radians
            } else {
                0.0
            };
            (
                premultiplied(*from),
                premultiplied(*to),
                [angle.cos(), angle.sin(), 0.0, 0.0],
                1_u32,
            )
        }
        QuadFace::Texture { opacity, .. } => {
            let opacity = if opacity.is_finite() {
                opacity.clamp(0.0, 1.0)
            } else {
                0.0
            };
            ([0.0; 4], [0.0; 4], [0.0, 0.0, opacity, 0.0], 2_u32)
        }
    };
    for value in color_a.iter().chain(&color_b).chain(&params) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [kind, 0, 0, 0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

/// A column-major 4x4 matrix, laid out exactly as WGSL reads a `mat4x4<f32>`
/// out of a uniform buffer: element `(row, col)` lives at `col * 4 + row`.
type Mat4 = [f32; 16];

/// The identity matrix.
#[must_use]
const fn identity() -> Mat4 {
    let mut m = [0.0; 16];
    m[0] = 1.0;
    m[5] = 1.0;
    m[10] = 1.0;
    m[15] = 1.0;
    m
}

/// `a * b`, column-major.
#[must_use]
fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

/// A pure translation.
#[must_use]
fn translation(x: f32, y: f32, z: f32) -> Mat4 {
    let mut m = identity();
    m[12] = x;
    m[13] = y;
    m[14] = z;
    m
}

/// A non-uniform scale.
#[must_use]
fn scale(x: f32, y: f32, z: f32) -> Mat4 {
    let mut m = identity();
    m[0] = x;
    m[5] = y;
    m[10] = z;
    m
}

/// Rotation about `+X` (pitch).
#[must_use]
fn rotation_x(radians: f32) -> Mat4 {
    let (sin, cos) = radians.sin_cos();
    let mut m = identity();
    m[5] = cos;
    m[6] = sin;
    m[9] = -sin;
    m[10] = cos;
    m
}

/// Rotation about `+Y` (yaw).
#[must_use]
fn rotation_y(radians: f32) -> Mat4 {
    let (sin, cos) = radians.sin_cos();
    let mut m = identity();
    m[0] = cos;
    m[2] = -sin;
    m[8] = sin;
    m[10] = cos;
    m
}

/// Rotation about `+Z` (roll).
#[must_use]
fn rotation_z(radians: f32) -> Mat4 {
    let (sin, cos) = radians.sin_cos();
    let mut m = identity();
    m[0] = cos;
    m[1] = sin;
    m[4] = -sin;
    m[5] = cos;
    m
}

/// The right-handed perspective projection this substrate uses, mapping the
/// frustum's depth onto wgpu's `0..1` clip range.
#[must_use]
pub fn perspective(aspect: f32) -> Mat4 {
    let focal = 1.0 / (FOV_Y_RADIANS / 2.0).tan();
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    let mut m = [0.0; 16];
    m[0] = focal / aspect;
    m[5] = focal;
    m[10] = FAR / (NEAR - FAR);
    m[11] = -1.0;
    m[14] = NEAR * FAR / (NEAR - FAR);
    m
}

/// One quad's model matrix: the unit square scaled to the quad's world size,
/// rotated in place, and translated to where its destination rectangle puts
/// its centre.
///
/// World units are the target's own height, and `y` is flipped, so a
/// destination in target texels lands where a 2D fill of the same rectangle
/// would — see the module docs.
#[must_use]
fn model(quad: &Quad3d, width: u32, height: u32) -> Mat4 {
    let unit = 1.0 / f64::from(height);
    let centre = quad.dest.center();
    let translate = translation(
        ((centre.x - f64::from(width) / 2.0) * unit) as f32,
        ((f64::from(height) / 2.0 - centre.y) * unit) as f32,
        quad.depth_offset,
    );
    let rotate = multiply(
        &rotation_z(quad.roll),
        &multiply(&rotation_y(quad.yaw), &rotation_x(quad.pitch)),
    );
    let resize = scale(
        (quad.dest.width() * unit) as f32,
        (quad.dest.height() * unit) as f32,
        1.0,
    );
    multiply(&translate, &multiply(&rotate, &resize))
}

#[cfg(test)]
mod tests {
    use super::{
        CAMERA_DISTANCE, FOV_Y_RADIANS, Mat4, Quad3d, Quad3dScene, QuadFace, UNIFORM_SIZE,
        UNIFORM_STRIDE, encode_uniform, identity, model, multiply, perspective, premultiplied,
        translation,
    };
    use frust::authoring::Rect;
    use peniko::Color;

    /// Projects a world point through the view-projection this substrate
    /// uses, answering its normalised device coordinates.
    fn project(point: [f32; 3], aspect: f32) -> [f32; 3] {
        let vp = multiply(
            &perspective(aspect),
            &translation(0.0, 0.0, -CAMERA_DISTANCE),
        );
        transform(&vp, point)
    }

    /// `m * (point, 1)`, perspective-divided.
    fn transform(m: &Mat4, point: [f32; 3]) -> [f32; 3] {
        let [x, y, z] = point;
        let mut out = [0.0_f32; 4];
        for (row, slot) in out.iter_mut().enumerate() {
            *slot = m[row] * x + m[4 + row] * y + m[8 + row] * z + m[12 + row];
        }
        [out[0] / out[3], out[1] / out[3], out[2] / out[3]]
    }

    #[test]
    fn the_camera_distance_is_the_one_that_fills_the_frustum_vertically() {
        let expected = 0.5 / (FOV_Y_RADIANS / 2.0).tan();
        assert!(
            (CAMERA_DISTANCE - expected).abs() < 1e-5,
            "CAMERA_DISTANCE {CAMERA_DISTANCE} should be {expected}"
        );
    }

    /// The property the whole camera choice exists for: with no rotation, the
    /// 3D path is the 2D rectangle.
    #[test]
    fn an_unrotated_full_target_quad_projects_onto_the_whole_target() {
        let aspect = 2.0;
        let top_left = project([-aspect / 2.0, 0.5, 0.0], aspect);
        let bottom_right = project([aspect / 2.0, -0.5, 0.0], aspect);
        assert!((top_left[0] + 1.0).abs() < 1e-4, "{top_left:?}");
        assert!((top_left[1] - 1.0).abs() < 1e-4, "{top_left:?}");
        assert!((bottom_right[0] - 1.0).abs() < 1e-4, "{bottom_right:?}");
        assert!((bottom_right[1] + 1.0).abs() < 1e-4, "{bottom_right:?}");
    }

    #[test]
    fn a_models_corners_land_where_a_two_dimensional_fill_would() {
        let quad = Quad3d::new(
            Rect::new(0.0, 0.0, 128.0, 64.0),
            QuadFace::Solid(Color::WHITE),
        );
        let mvp = multiply(
            &multiply(&perspective(2.0), &translation(0.0, 0.0, -CAMERA_DISTANCE)),
            &model(&quad, 128, 64),
        );
        let top_left = transform(&mvp, [-0.5, 0.5, 0.0]);
        let bottom_right = transform(&mvp, [0.5, -0.5, 0.0]);
        assert!((top_left[0] + 1.0).abs() < 1e-4, "{top_left:?}");
        assert!((top_left[1] - 1.0).abs() < 1e-4, "{top_left:?}");
        assert!((bottom_right[0] - 1.0).abs() < 1e-4, "{bottom_right:?}");
        assert!((bottom_right[1] + 1.0).abs() < 1e-4, "{bottom_right:?}");
    }

    /// Positive yaw brings the left edge forward, which is what the GPU
    /// foreshortening assertion reads as a taller left column.
    #[test]
    fn positive_yaw_brings_the_left_edge_toward_the_camera() {
        let quad = Quad3d::new(
            Rect::new(0.0, 0.0, 64.0, 64.0),
            QuadFace::Solid(Color::WHITE),
        )
        .yaw(std::f32::consts::FRAC_PI_4);
        let m = model(&quad, 64, 64);
        let left = transform(&m, [-0.5, 0.0, 0.0]);
        let right = transform(&m, [0.5, 0.0, 0.0]);
        assert!(
            left[2] > right[2],
            "left {left:?} should be nearer than right {right:?}"
        );
    }

    #[test]
    fn a_yawed_quad_is_foreshortened_in_projection() {
        let quad = Quad3d::new(
            Rect::new(8.0, 8.0, 56.0, 56.0),
            QuadFace::Solid(Color::WHITE),
        )
        .yaw(std::f32::consts::FRAC_PI_4);
        let mvp = multiply(
            &multiply(&perspective(1.0), &translation(0.0, 0.0, -CAMERA_DISTANCE)),
            &model(&quad, 64, 64),
        );
        let near_top = transform(&mvp, [-0.5, 0.5, 0.0]);
        let near_bottom = transform(&mvp, [-0.5, -0.5, 0.0]);
        let far_top = transform(&mvp, [0.5, 0.5, 0.0]);
        let far_bottom = transform(&mvp, [0.5, -0.5, 0.0]);
        let near_height = near_top[1] - near_bottom[1];
        let far_height = far_top[1] - far_bottom[1];
        assert!(
            near_height > far_height * 1.2,
            "near edge {near_height} should be markedly taller than far edge {far_height}"
        );
    }

    #[test]
    fn a_degenerate_aspect_never_produces_a_non_finite_projection() {
        for aspect in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(perspective(aspect).iter().all(|value| value.is_finite()));
        }
    }

    #[test]
    fn a_colour_is_premultiplied_once() {
        let half = Color::from_rgba8(255, 0, 0, 128);
        let [r, g, b, a] = premultiplied(half);
        assert!((a - 128.0 / 255.0).abs() < 1e-3);
        assert!((r - a).abs() < 1e-3, "red should be scaled to alpha");
        assert!(g.abs() < 1e-6);
        assert!(b.abs() < 1e-6);
    }

    #[test]
    fn an_opaque_colour_survives_premultiplication_unchanged() {
        let [r, g, b, a] = premultiplied(Color::from_rgba8(255, 128, 0, 255));
        assert!((r - 1.0).abs() < 1e-6);
        assert!((g - 128.0 / 255.0).abs() < 1e-3);
        assert!(b.abs() < 1e-6);
        assert!((a - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_uniform_record_is_exactly_the_size_the_layout_declares() {
        let mut bytes = Vec::new();
        encode_uniform(&mut bytes, &identity(), &QuadFace::Solid(Color::WHITE));
        assert_eq!(bytes.len() as u64, UNIFORM_SIZE);
        const { assert!(UNIFORM_SIZE <= UNIFORM_STRIDE) };
    }

    #[test]
    fn each_face_kind_encodes_its_own_discriminant() {
        for (face, expected) in [
            (QuadFace::Solid(Color::WHITE), 0_u32),
            (
                QuadFace::Gradient {
                    from: Color::WHITE,
                    to: Color::BLACK,
                    angle_radians: 0.0,
                },
                1,
            ),
        ] {
            let mut bytes = Vec::new();
            encode_uniform(&mut bytes, &identity(), &face);
            let kind = u32::from_le_bytes(
                bytes[112..116]
                    .try_into()
                    .expect("the kind word is four bytes"),
            );
            assert_eq!(kind, expected);
        }
    }

    #[test]
    fn a_non_finite_gradient_angle_encodes_as_a_finite_axis() {
        let mut bytes = Vec::new();
        encode_uniform(
            &mut bytes,
            &identity(),
            &QuadFace::Gradient {
                from: Color::WHITE,
                to: Color::BLACK,
                angle_radians: f32::NAN,
            },
        );
        let axis_x = f32::from_le_bytes(bytes[96..100].try_into().expect("four bytes"));
        assert!(axis_x.is_finite());
    }

    #[test]
    fn a_quad_with_non_finite_geometry_is_not_renderable() {
        let base = Quad3d::new(
            Rect::new(0.0, 0.0, 10.0, 10.0),
            QuadFace::Solid(Color::WHITE),
        );
        assert!(base.is_renderable());
        assert!(!base.clone().yaw(f32::NAN).is_renderable());
        assert!(
            !Quad3d::new(
                Rect::new(0.0, 0.0, 0.0, 10.0),
                QuadFace::Solid(Color::WHITE)
            )
            .is_renderable()
        );
    }

    #[test]
    fn a_scene_starts_empty_and_depth_free() {
        let scene = Quad3dScene::new();
        assert!(scene.is_empty());
        assert_eq!(scene.len(), 0);
        assert!(!scene.depth);
        let scene = scene.with_depth(true).with(Quad3d::new(
            Rect::new(0.0, 0.0, 4.0, 4.0),
            QuadFace::Solid(Color::WHITE),
        ));
        assert!(scene.depth);
        assert_eq!(scene.len(), 1);
        assert!(!scene.is_empty());
    }
}

/// The renderer against real hardware. Ignored by default; see
/// [`crate::gpu_fx::test_gpu`] for the invocation and why a read-back is the
/// only honest check here.
#[cfg(test)]
mod gpu_tests {
    use frust::authoring::Rect;
    use peniko::Color;

    use super::{
        COLOR_FORMAT, DEPTH_FORMAT, Quad3d, Quad3dRenderer, Quad3dScene, Quad3dTarget, QuadFace,
    };
    use crate::gpu_fx::pool::{FxComponentId, MAX_UNSEEN_FRAMES, TargetKey, TargetPool};
    use crate::gpu_fx::test_gpu::{read_back, with_device};

    /// Side of every target under test. 64 texels of RGBA is exactly wgpu's
    /// row-copy alignment, so the read-back needs no unpadding to be trusted.
    const SIDE: u32 = 64;

    /// Builds the colour target every case renders into.
    fn color_target(device: &wgpu::Device) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frust-beui gpu_fx case colour"),
            size: wgpu::Extent3d {
                width: SIDE,
                height: SIDE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    /// Builds the depth attachment the occlusion case needs.
    fn depth_target(device: &wgpu::Device) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("frust-beui gpu_fx case depth"),
                size: wgpu::Extent3d {
                    width: SIDE,
                    height: SIDE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Renders one scene and hands back the target's texels.
    fn render(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut Quad3dRenderer,
        depth: Option<&wgpu::TextureView>,
        scene: &Quad3dScene,
    ) -> Vec<u8> {
        let texture = color_target(device);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-beui gpu_fx case"),
        });
        renderer.record(
            device,
            queue,
            &mut encoder,
            Quad3dTarget {
                color: &view,
                depth,
                requested: (SIDE, SIDE),
            },
            scene,
        );
        queue.submit([encoder.finish()]);
        read_back(device, queue, &texture, SIDE, SIDE)
    }

    /// The RGBA at device pixel `(x, y)`.
    fn pixel(frame: &[u8], x: u32, y: u32) -> [u8; 4] {
        let start = ((y * SIDE + x) * 4) as usize;
        frame[start..start + 4]
            .try_into()
            .expect("a read-back pixel is four bytes")
    }

    /// How many texels of column `x` the render covered at all.
    fn column_coverage(frame: &[u8], x: u32) -> u32 {
        (0..SIDE).filter(|y| pixel(frame, x, *y)[3] > 0).count() as u32
    }

    /// A yawed quad is genuinely projected, not sheared: the edge rotated
    /// toward the camera covers markedly more of its column than the edge
    /// rotated away, and both stay inside the destination's own span.
    ///
    /// The unrotated control in the same case is the other half of the claim —
    /// at zero tilt the projection is the plain 2D rectangle, which is what
    /// lets a component tilt from rest without a jump.
    #[test]
    #[ignore = "needs a real GPU adapter"]
    fn a_yawed_quad_renders_foreshortened() {
        with_device(|device, queue| {
            let mut renderer = Quad3dRenderer::new(device, "foreshortening");
            let dest = Rect::new(8.0, 8.0, 56.0, 56.0);
            let face = QuadFace::Solid(Color::WHITE);

            let flat = render(
                device,
                queue,
                &mut renderer,
                None,
                &Quad3dScene::new().with(Quad3d::new(dest, face.clone())),
            );
            for x in [13, 31, 46] {
                let covered = column_coverage(&flat, x);
                assert!(
                    covered.abs_diff(48) <= 1,
                    "an unrotated quad covers its own 48 rows at x={x}, saw {covered}"
                );
            }
            assert_eq!(
                column_coverage(&flat, 2),
                0,
                "nothing is painted outside the destination"
            );

            let yawed = render(
                device,
                queue,
                &mut renderer,
                None,
                &Quad3dScene::new().with(Quad3d::new(dest, face).yaw(std::f32::consts::FRAC_PI_4)),
            );
            let near = (10..20)
                .map(|x| column_coverage(&yawed, x))
                .max()
                .unwrap_or(0);
            let far = (42..52)
                .map(|x| column_coverage(&yawed, x))
                .max()
                .unwrap_or(0);
            println!("gpu_fx foreshortening: near column {near}, far column {far}");
            assert!(near > 0 && far > 0, "the yawed quad rendered nothing");
            assert!(
                near > far + 6,
                "the near edge ({near}) should cover markedly more than the far edge ({far})"
            );
            assert!(
                (0..8).all(|x| column_coverage(&yawed, x) == 0),
                "the projection stays inside the target"
            );
        });
    }

    /// With depth on, a face behind another is hidden by distance rather than
    /// by paint order — the far face is drawn *second* and still loses. The
    /// depth-free control in the same case is the negative half: without the
    /// attachment the same order paints the far face over the near one.
    #[test]
    #[ignore = "needs a real GPU adapter"]
    fn two_quads_occlude_by_depth_not_paint_order() {
        with_device(|device, queue| {
            let mut renderer = Quad3dRenderer::new(device, "occlusion");
            let dest = Rect::new(12.0, 12.0, 52.0, 52.0);
            let near = Color::from_rgba8(255, 0, 0, 255);
            let far = Color::from_rgba8(0, 0, 255, 255);
            let stack = || {
                Quad3dScene::new()
                    .with(Quad3d::new(dest, QuadFace::Solid(near)).depth_offset(0.15))
                    .with(Quad3d::new(dest, QuadFace::Solid(far)).depth_offset(-0.15))
            };

            let depth = depth_target(device);
            let occluded = render(
                device,
                queue,
                &mut renderer,
                Some(&depth),
                &stack().with_depth(true),
            );
            assert_eq!(
                pixel(&occluded, SIDE / 2, SIDE / 2),
                [255, 0, 0, 255],
                "the nearer face survives the farther one drawn after it"
            );

            let painted = render(device, queue, &mut renderer, None, &stack());
            assert_eq!(
                pixel(&painted, SIDE / 2, SIDE / 2),
                [0, 0, 255, 255],
                "without depth the same order is plain painter order"
            );
        });
    }

    /// A pooled target survives the whole unseen window and is reclaimed one
    /// frame past it, and the replacement is recognisably a different texture
    /// — the generation change a stale binding is caught by.
    #[test]
    #[ignore = "needs a real GPU adapter"]
    fn a_pooled_target_is_reaped_after_the_unseen_window() {
        with_device(|device, _queue| {
            let component = FxComponentId::mint();
            let key = TargetKey::new(component, SIDE, SIDE);
            let mut pool = TargetPool::new();

            pool.begin_frame(1);
            let first = pool
                .acquire(device, key, false, "reap")
                .expect("the device allocates a target")
                .generation();
            assert_eq!(pool.len(), 1);

            // A nearby size quantizes to the same key, so nothing is
            // reallocated and the generation holds.
            let nearby = pool
                .acquire(
                    device,
                    TargetKey::new(component, SIDE + 1, SIDE),
                    false,
                    "reap",
                )
                .expect("the same key answers")
                .generation();
            assert_eq!(nearby, first, "a quantized-equal request reuses the target");
            assert_eq!(pool.len(), 1);

            pool.begin_frame(1 + MAX_UNSEEN_FRAMES);
            assert!(
                pool.reap().is_empty(),
                "a target inside the window is not reclaimed"
            );
            assert_eq!(pool.len(), 1);

            pool.begin_frame(2 + MAX_UNSEEN_FRAMES);
            assert_eq!(pool.reap(), vec![key], "the aged-out key is reclaimed");
            assert!(pool.is_empty());

            let reborn = pool
                .acquire(device, key, false, "reap")
                .expect("the key can be asked for again")
                .generation();
            assert_ne!(
                reborn, first,
                "a recreated target is a different generation, which is what \
                 makes a stale binding detectable at an unchanged extent"
            );
        });
    }
}
