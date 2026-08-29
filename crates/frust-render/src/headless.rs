//! Offscreen vello-classic rendering: the oracle harness golden tests and
//! pixel-level regression tests render a [`frust_scene::Scene`] through when
//! there is no window and no surface.
//!
//! Vello renders **by compute** into an `Rgba8Unorm` storage texture, so an
//! offscreen render needs no swapchain, no display server and no
//! `wgpu::Surface` — only a GPU, a driver and device-node permission (see
//! `docs/TESTING.md` § Native Headless GPU Rendering).
//!
//! Three properties make this the harness a *golden* comparison can be trusted
//! against, as opposed to a hand-rolled render in each test:
//!
//! 1. **The adapter is the one the environment asked for.** Selection goes
//!    through [`wgpu::util::initialize_adapter_from_env_or_default`], the same
//!    call the live device path uses, so `WGPU_ADAPTER_NAME` is honoured on a
//!    multi-GPU host rather than silently ignored by a bare `request_adapter`.
//! 2. **A wrong adapter fails before it can produce pixels.**
//!    [`HeadlessOptions::expect_adapter`]/[`HeadlessOptions::expect_backend`]
//!    (or [`GOLDEN_EXPECT_ADAPTER_ENV_VAR`]/[`GOLDEN_EXPECT_BACKEND_ENV_VAR`])
//!    are checked against the *resolved* adapter in [`HeadlessRenderer::new`],
//!    so a baseline can never be promoted from the wrong GPU with the right
//!    file name.
//! 3. **The device is the app's device.** Limits, optional features and the
//!    render-tier probe are the ones `RenderContext` resolves, so a headless
//!    run cannot pass on capabilities a real surface would never get.
//!
//! Every render is bracketed by a `Validation` error scope and fails on any
//! captured error, and the readback strips wgpu's 256-byte row padding, so an
//! arbitrary width (not just a multiple of 64 px) reads back exactly.
//!
//! The API is `async` because wgpu's adapter/device requests and its error
//! scope are futures and this crate has no executor dependency; callers drive
//! it with whatever they already have (`pollster::block_on` in tests).
//!
//! No `vello`/`wgpu` type crosses this module's public surface — the
//! confinement rule in `docs/RENDER_ARCHITECTURE.md`. Pixels come back as
//! plain bytes and the one handle that leaves ([`ImageData`]) is `peniko`'s,
//! the same vocabulary `frust-scene` already speaks.

use anyhow::{Result, anyhow};
use frust_scene::Scene;
use kurbo::Affine;
use peniko::{Color, ImageData};

use crate::context::{AaMode, effective_limits, is_ios_simulator, vello_optional_features};
use crate::convert::encode_scene;
use crate::tier::{
    RenderTier, TierCaps, TierOutcome, render_tier_override_from_env, select_render_tier,
};

/// Environment variable naming the adapter a golden/oracle run must have
/// resolved. Case-insensitive substring match on the adapter name, the same
/// shape `WGPU_ADAPTER_NAME` itself matches by — `T400` accepts
/// `NVIDIA T400 4GB`.
///
/// This is a *check*, not a selector: `WGPU_ADAPTER_NAME` chooses the adapter,
/// this refuses the run when the choice did not land where the operator
/// believed it would.
pub const GOLDEN_EXPECT_ADAPTER_ENV_VAR: &str = "FRUST_GOLDEN_EXPECT_ADAPTER";

/// Environment variable naming the wgpu backend a golden/oracle run must have
/// resolved (`vulkan`, `metal`, `dx12`, `gl`, …; case-insensitive, matched
/// whole). The backend counterpart of [`GOLDEN_EXPECT_ADAPTER_ENV_VAR`].
pub const GOLDEN_EXPECT_BACKEND_ENV_VAR: &str = "FRUST_GOLDEN_EXPECT_BACKEND";

/// wgpu's mandatory row alignment for `copy_texture_to_buffer`: every row of
/// the readback buffer starts on a multiple of this, so a width whose
/// `width * 4` is not a multiple of it reads back with trailing padding bytes
/// per row that [`strip_row_padding`] removes.
const COPY_ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// The anti-aliasing method a headless render asks vello for. Mirrors the
/// `FRUST_AA_MODE` knob's three modes without exposing `vello::AaConfig`
/// (this crate's confinement rule).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeadlessAa {
    /// vello's default: area anti-aliasing.
    #[default]
    Area,
    /// 8x multisampling.
    Msaa8,
    /// 16x multisampling.
    Msaa16,
}

impl HeadlessAa {
    /// The crate-internal mode this maps to, which owns the `vello` mapping.
    fn to_mode(self) -> AaMode {
        match self {
            HeadlessAa::Area => AaMode::Area,
            HeadlessAa::Msaa8 => AaMode::Msaa8,
            HeadlessAa::Msaa16 => AaMode::Msaa16,
        }
    }
}

/// How a [`HeadlessRenderer`] should pick, and then verify, its GPU.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeadlessOptions {
    /// Backend(s) to restrict adapter enumeration to, as a comma-separated
    /// list (`vulkan`, `metal`, `dx12`, `gl`, `vulkan,metal`, …).
    ///
    /// A *hint*: `WGPU_BACKEND` wins when it is set, so an operator can pin the
    /// backend of a run without editing the caller.
    pub backend_hint: Option<String>,
    /// Adapter name the resolved adapter must match (case-insensitive
    /// substring). Overrides [`GOLDEN_EXPECT_ADAPTER_ENV_VAR`] when set.
    pub expect_adapter: Option<String>,
    /// Backend the resolved adapter must be on (case-insensitive, whole word).
    /// Overrides [`GOLDEN_EXPECT_BACKEND_ENV_VAR`] when set.
    pub expect_backend: Option<String>,
}

/// One offscreen render's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadlessSpec {
    /// Target width in device pixels. Any width is valid — the readback pads
    /// and strips rows as wgpu requires.
    pub width: u32,
    /// Target height in device pixels.
    pub height: u32,
    /// The colour vello clears the target to before drawing the scene.
    pub base_color: Color,
    /// A transform applied on top of the whole encoded scene, the headless
    /// analogue of the blit arm's render-scale root.
    pub root: Affine,
    /// The anti-aliasing method to render with.
    pub aa: HeadlessAa,
}

impl HeadlessSpec {
    /// A `width` x `height` render over an opaque black base, unrooted, area
    /// anti-aliased — the defaults a case overrides field by field with
    /// struct-update syntax.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            base_color: peniko::color::palette::css::BLACK,
            root: Affine::IDENTITY,
            aa: HeadlessAa::Area,
        }
    }
}

/// Which GPU actually produced an image — the provenance every promoted
/// baseline and every failing artifact has to record (`docs/TESTING.md`
/// § GPU Run Metadata).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadlessMeta {
    /// The wgpu backend, lowercase (`vulkan`, `metal`, `dx12`, `gl`).
    pub backend: String,
    /// The adapter name as the driver reports it (e.g. `NVIDIA T400 4GB`).
    pub adapter: String,
    /// The driver name and, when the backend reports one, its version detail.
    pub driver: String,
}

impl HeadlessMeta {
    fn from_info(info: &wgpu::AdapterInfo) -> Self {
        let driver = if info.driver_info.is_empty() {
            info.driver.clone()
        } else if info.driver.is_empty() {
            info.driver_info.clone()
        } else {
            format!("{} ({})", info.driver, info.driver_info)
        };
        Self {
            backend: info.backend.to_str().to_string(),
            adapter: info.name.clone(),
            driver,
        }
    }
}

impl std::fmt::Display for HeadlessMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "backend={} adapter={:?} driver={:?}",
            self.backend, self.adapter, self.driver
        )
    }
}

/// The pixels of one headless render, plus the provenance of the GPU that
/// produced them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadlessImage {
    /// Width in pixels, matching the [`HeadlessSpec`] that produced it.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Tightly packed RGBA8 rows — `width * height * 4` bytes, **no** row
    /// padding, in whatever alpha representation vello writes (straight, not
    /// premultiplied: an erased pixel reads `[0, 0, 0, 0]`).
    pub rgba8: Vec<u8>,
    /// The GPU this image came from.
    pub meta: HeadlessMeta,
}

impl HeadlessImage {
    /// The RGBA bytes of pixel `(x, y)`.
    ///
    /// # Panics
    ///
    /// Panics when `(x, y)` is outside the image — an out-of-bounds probe in a
    /// pixel assertion is a broken test, not a runtime condition to handle.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(
            x < self.width && y < self.height,
            "pixel ({x}, {y}) is outside a {}x{} image",
            self.width,
            self.height
        );
        let at = ((y * self.width + x) * 4) as usize;
        [
            self.rgba8[at],
            self.rgba8[at + 1],
            self.rgba8[at + 2],
            self.rgba8[at + 3],
        ]
    }
}

/// A fullscreen-triangle WGSL program to render into an offscreen texture and
/// register with vello as an image override — the shape of the renderer's
/// shader pre-pass (`shader_effects.rs`), reproduced here so the
/// override → atlas copy is reachable from a test without the crate handing
/// out a `wgpu::Device`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShaderOverrideSpec<'a> {
    /// Width of the offscreen texture the program renders into.
    pub width: u32,
    /// Height of the offscreen texture.
    pub height: u32,
    /// WGSL source exposing a `vs_main` vertex entry point (expected to emit a
    /// fullscreen triangle from `@builtin(vertex_index)`, drawn as `0..3`) and
    /// an `fs_main` fragment entry point writing `Rgba8Unorm`.
    pub wgsl: &'a str,
}

/// A reusable offscreen vello-classic renderer: one adapter, one device, one
/// `vello::Renderer` and one size-matched target texture, across any number of
/// renders.
///
/// See the module docs for what makes it an oracle rather than a convenience
/// wrapper.
pub struct HeadlessRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: vello::Renderer,
    meta: HeadlessMeta,
    /// The current render target, rebuilt only when a render asks for a
    /// different size.
    target: Option<HeadlessTarget>,
}

/// The `Rgba8Unorm` storage texture vello renders into, kept across renders of
/// the same size.
struct HeadlessTarget {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl HeadlessRenderer {
    /// Resolves an adapter, verifies it against the expectations, probes the
    /// render tier, and creates the device and vello renderer every later
    /// [`render`](Self::render) reuses.
    ///
    /// # Errors
    ///
    /// Fails when no adapter is available, when the resolved adapter does not
    /// meet an `expect_adapter`/`expect_backend` expectation (**before** any
    /// rendering), when the tier probe refuses the adapter or resolves
    /// anything but the classic GPU tier, or when device/renderer creation
    /// fails.
    pub async fn new(options: HeadlessOptions) -> Result<Self> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        descriptor.backends = resolve_backends(
            wgpu::Backends::from_env(),
            options.backend_hint.as_deref(),
            descriptor.backends,
        );
        let instance = wgpu::Instance::new(descriptor);

        // The environment-aware initializer, exactly as `RenderContext`'s
        // device creation calls it: a bare `request_adapter` ignores
        // `WGPU_ADAPTER_NAME`, which on a dual-GPU host silently decides which
        // GPU a baseline was captured on.
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .map_err(|e| anyhow!("frust-render headless: no compatible GPU adapter: {e}"))?;
        let meta = HeadlessMeta::from_info(&adapter.get_info());

        let expect_adapter = options
            .expect_adapter
            .or_else(|| golden_env(GOLDEN_EXPECT_ADAPTER_ENV_VAR));
        let expect_backend = options
            .expect_backend
            .or_else(|| golden_env(GOLDEN_EXPECT_BACKEND_ENV_VAR));
        check_expectations(&meta, expect_adapter.as_deref(), expect_backend.as_deref())?;

        // The same tier probe device creation runs, so a headless render can
        // never pass on an adapter the app itself would refuse — and so a
        // capture is never labelled `classic` while an override asked for
        // another renderer.
        let caps = TierCaps {
            downlevel_flags: adapter.get_downlevel_capabilities().flags,
            adapter_name: meta.adapter.clone(),
        };
        let selection = select_render_tier(&caps, render_tier_override_from_env());
        match selection.outcome {
            TierOutcome::Available(RenderTier::Gpu) => {}
            TierOutcome::Available(other) => {
                return Err(anyhow!(
                    "frust-render headless: this harness renders vello classic on the Gpu tier, \
                     but tier selection resolved `{other:?}` ({}) — unset the render-tier \
                     override for an oracle run",
                    selection.diagnosis
                ));
            }
            TierOutcome::Unavailable { .. } => return Err(anyhow!(selection.diagnosis)),
        }

        let required_features = adapter.features() & vello_optional_features();
        let required_limits = effective_limits(adapter.limits(), is_ios_simulator());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-render headless device"),
                required_features,
                required_limits,
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow!("frust-render headless: failed to create GPU device: {e}"))?;

        // `RendererOptions::default()` compiles every anti-aliasing
        // permutation, unlike the surface path's single selected mode: the
        // spec picks the mode per render here, so all three must exist.
        let renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
            .map_err(|e| anyhow!("frust-render headless: failed to create vello renderer: {e}"))?;

        log::info!("frust-render headless: {meta}");
        Ok(Self {
            device,
            queue,
            renderer,
            meta,
            target: None,
        })
    }

    /// The GPU this renderer resolved — the provenance to record with any
    /// image it produces.
    #[must_use]
    pub fn meta(&self) -> &HeadlessMeta {
        &self.meta
    }

    /// Renders `scene` offscreen and reads the pixels back, unpadded.
    ///
    /// # Errors
    ///
    /// Fails on a zero-sized spec, on a vello render error, on a failed buffer
    /// map, and on **any** wgpu validation error captured during the render —
    /// an oracle that renders through a validation error is producing a
    /// baseline nobody should trust.
    pub async fn render(&mut self, scene: &Scene, spec: &HeadlessSpec) -> Result<HeadlessImage> {
        if spec.width == 0 || spec.height == 0 {
            return Err(anyhow!(
                "frust-render headless: a render target must have a non-zero size, got {}x{}",
                spec.width,
                spec.height
            ));
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let rendered = self.render_inner(scene, spec);
        let validation = scope.pop().await;
        let rgba8 = finish_scoped(rendered, validation, "render")?;
        Ok(HeadlessImage {
            width: spec.width,
            height: spec.height,
            rgba8,
            meta: self.meta.clone(),
        })
    }

    /// Renders a fullscreen WGSL program into an offscreen texture and hands
    /// it to vello as an image override, returning the handle a scene draws it
    /// through (`SceneBuilder::draw_image`) — the exact lowering a
    /// `Command::ShaderQuad` gets from the renderer's shader pre-pass, with
    /// the pre-pass's ordering: the program is submitted before any later
    /// [`render`](Self::render) encodes.
    ///
    /// # Errors
    ///
    /// Fails on a zero-sized spec and on any wgpu validation error captured
    /// while compiling or running the program (a WGSL parse failure included).
    pub async fn register_shader_override(
        &mut self,
        spec: &ShaderOverrideSpec<'_>,
    ) -> Result<ImageData> {
        if spec.width == 0 || spec.height == 0 {
            return Err(anyhow!(
                "frust-render headless: a shader-override texture must have a non-zero size, \
                 got {}x{}",
                spec.width,
                spec.height
            ));
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let image = self.shader_override_inner(spec);
        let validation = scope.pop().await;
        finish_scoped(image, validation, "shader override")
    }

    /// The synchronous body of [`render`](Self::render): everything between
    /// pushing the validation scope and popping it.
    fn render_inner(&mut self, scene: &Scene, spec: &HeadlessSpec) -> Result<Vec<u8>> {
        let mut encoded = vello::Scene::new();
        encode_scene(scene, &mut encoded);
        // A root transform is applied by appending rather than by re-encoding,
        // because `encode_scene` is the one sanctioned scene→vello mapping and
        // an oracle must exercise it unmodified.
        let vello_scene = if spec.root == Affine::IDENTITY {
            encoded
        } else {
            let mut rooted = vello::Scene::new();
            rooted.append(&encoded, Some(spec.root));
            rooted
        };

        self.ensure_target(spec.width, spec.height);
        let target = self
            .target
            .as_ref()
            .expect("ensure_target always leaves a target in place");
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                &vello_scene,
                &target.view,
                &vello::RenderParams {
                    base_color: spec.base_color,
                    width: spec.width,
                    height: spec.height,
                    antialiasing_method: spec.aa.to_mode().to_vello(),
                },
            )
            .map_err(|e| anyhow!("frust-render headless: render_to_texture failed: {e}"))?;

        read_back(
            &self.device,
            &self.queue,
            &target.texture,
            spec.width,
            spec.height,
        )
    }

    /// The synchronous body of
    /// [`register_shader_override`](Self::register_shader_override).
    fn shader_override_inner(&mut self, spec: &ShaderOverrideSpec<'_>) -> Result<ImageData> {
        // RENDER_ATTACHMENT for our own pass, COPY_SRC because vello's
        // override copy reads the texture into its image atlas.
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frust-render headless shader override"),
            size: wgpu::Extent3d {
                width: spec.width,
                height: spec.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("frust-render headless shader override module"),
                source: wgpu::ShaderSource::Wgsl(spec.wgsl.into()),
            });
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("frust-render headless shader override layout"),
                bind_group_layouts: &[],
                immediate_size: 0,
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("frust-render headless shader override pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-render headless shader override pass"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frust-render headless shader override pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.draw(0..3, 0..1);
        }
        // Submitted before the scene render that draws it, which is the
        // pre-pass ordering the renderer itself uses.
        self.queue.submit([encoder.finish()]);

        let image = self.renderer.register_texture(texture);
        self.renderer.mark_override_image_dirty(&image);
        Ok(image)
    }

    /// Ensures [`Self::target`] is a `width` x `height` vello-compatible
    /// storage texture, rebuilding it only on a size change.
    fn ensure_target(&mut self, width: u32, height: u32) {
        if matches!(&self.target, Some(t) if t.width == width && t.height == height) {
            return;
        }
        // Vello renders by compute, hence STORAGE_BINDING; TEXTURE_BINDING
        // matches the live intermediate target's usage and COPY_SRC is what
        // makes the readback possible.
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frust-render headless target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            format: wgpu::TextureFormat::Rgba8Unorm,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.target = Some(HeadlessTarget {
            width,
            height,
            texture,
            view,
        });
    }
}

/// Copies `texture` into a mappable buffer and returns its rows with wgpu's
/// 256-byte row padding removed.
fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let bytes_per_row = padded_bytes_per_row(width);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frust-render headless readback"),
        size: u64::from(bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frust-render headless readback copy"),
    });
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
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |res| {
        let _ = tx.send(res);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| anyhow!("frust-render headless: device poll failed: {e}"))?;
    rx.recv()
        .map_err(|e| anyhow!("frust-render headless: readback map channel closed: {e}"))?
        .map_err(|e| anyhow!("frust-render headless: readback buffer map failed: {e}"))?;

    let mapped = slice.get_mapped_range();
    let pixels = strip_row_padding(&mapped, width, height);
    drop(mapped);
    buffer.unmap();
    Ok(pixels)
}

/// The `bytes_per_row` a `copy_texture_to_buffer` of a `width`-pixel RGBA8
/// texture must use: the tight row length rounded up to wgpu's
/// [`COPY_ROW_ALIGNMENT`].
fn padded_bytes_per_row(width: u32) -> u32 {
    (width * 4).next_multiple_of(COPY_ROW_ALIGNMENT)
}

/// Copies the leading `width * 4` bytes out of each padded row of `padded`,
/// producing tightly packed RGBA8 rows.
fn strip_row_padding(padded: &[u8], width: u32, height: u32) -> Vec<u8> {
    let row = width as usize * 4;
    let stride = padded_bytes_per_row(width) as usize;
    let mut out = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        let start = y * stride;
        out.extend_from_slice(&padded[start..start + row]);
    }
    out
}

/// Combines a scoped operation's own result with whatever its `Validation`
/// error scope captured.
///
/// A captured validation error fails the operation even when it otherwise
/// "succeeded" — the case this exists for is a render that produced plausible
/// pixels through an error wgpu's default handler would only have logged.
fn finish_scoped<T>(result: Result<T>, validation: Option<wgpu::Error>, what: &str) -> Result<T> {
    match (result, validation) {
        (Ok(value), None) => Ok(value),
        (Ok(_), Some(error)) => Err(anyhow!(
            "frust-render headless: wgpu validation error during {what}: {error}"
        )),
        (Err(error), None) => Err(error),
        (Err(error), Some(validation)) => {
            Err(error.context(format!("wgpu validation error during {what}: {validation}")))
        }
    }
}

/// The backends adapter enumeration should be restricted to.
///
/// `WGPU_BACKEND` (already parsed into `env_backends`) wins over the caller's
/// `hint`, so an operator can pin a run's backend without editing the caller;
/// with neither set, `fallback` (the instance descriptor's own env-derived
/// default) applies.
fn resolve_backends(
    env_backends: Option<wgpu::Backends>,
    hint: Option<&str>,
    fallback: wgpu::Backends,
) -> wgpu::Backends {
    env_backends
        .or_else(|| hint.map(wgpu::Backends::from_comma_list))
        .unwrap_or(fallback)
}

/// The value of a `FRUST_GOLDEN_EXPECT_*` variable, treating an empty value as
/// unset (`context::env_str`'s shared precedence, runtime half only — these
/// are operator knobs for a host test run, never baked into a binary).
fn golden_env(name: &str) -> Option<String> {
    crate::context::env_str(None, std::env::var(name).ok())
}

/// Refuses a run whose resolved GPU is not the expected one, **before** it can
/// render anything.
///
/// `expect_adapter` matches case-insensitively as a substring, the same way
/// `WGPU_ADAPTER_NAME` selects (so the selector and the check cannot disagree
/// about what `T400` means); `expect_backend` matches the whole backend name,
/// case-insensitively.
fn check_expectations(
    meta: &HeadlessMeta,
    expect_adapter: Option<&str>,
    expect_backend: Option<&str>,
) -> Result<()> {
    if let Some(expected) = expect_adapter.map(str::trim).filter(|e| !e.is_empty())
        && !meta
            .adapter
            .to_lowercase()
            .contains(&expected.to_lowercase())
    {
        return Err(anyhow!(
            "frust-render headless: expected adapter matching `{expected}`, but the run resolved \
             `{}` ({meta}); set WGPU_ADAPTER_NAME (or isolate the driver ICD) so the intended GPU \
             is selected",
            meta.adapter
        ));
    }
    if let Some(expected) = expect_backend.map(str::trim).filter(|e| !e.is_empty())
        && !meta.backend.eq_ignore_ascii_case(expected)
    {
        return Err(anyhow!(
            "frust-render headless: expected backend `{expected}`, but the run resolved `{}` \
             ({meta}); set WGPU_BACKEND to pin it",
            meta.backend
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(backend: &str, adapter: &str) -> HeadlessMeta {
        HeadlessMeta {
            backend: backend.to_string(),
            adapter: adapter.to_string(),
            driver: "test driver".to_string(),
        }
    }

    #[test]
    fn padded_row_is_the_tight_row_when_already_aligned() {
        // 64 px is the width the smoke tests used precisely to dodge padding.
        assert_eq!(padded_bytes_per_row(64), 256);
        assert_eq!(padded_bytes_per_row(128), 512);
        assert_eq!(padded_bytes_per_row(256), 1024);
    }

    #[test]
    fn padded_row_rounds_an_unaligned_width_up_to_the_alignment() {
        assert_eq!(padded_bytes_per_row(1), 256);
        assert_eq!(padded_bytes_per_row(63), 256);
        assert_eq!(padded_bytes_per_row(65), 512);
        assert_eq!(padded_bytes_per_row(97), 512);
        assert_eq!(padded_bytes_per_row(129), 768);
        for width in 1..600u32 {
            let padded = padded_bytes_per_row(width);
            assert!(padded >= width * 4, "padding must never truncate a row");
            assert_eq!(padded % COPY_ROW_ALIGNMENT, 0, "width {width}");
            assert!(
                padded - width * 4 < COPY_ROW_ALIGNMENT,
                "padding must be minimal, width {width}"
            );
        }
    }

    #[test]
    fn stripping_padding_keeps_every_rows_own_pixels() {
        // 3 px rows (12 bytes) padded to 256: each row is tagged with its own
        // index so a stride slip is visible rather than plausible.
        let width = 3;
        let height = 4;
        let stride = padded_bytes_per_row(width) as usize;
        let mut padded = vec![0xEE_u8; stride * height as usize];
        for y in 0..height as usize {
            for byte in 0..(width as usize * 4) {
                padded[y * stride + byte] = (y * 16 + byte) as u8;
            }
        }
        let stripped = strip_row_padding(&padded, width, height);
        assert_eq!(stripped.len(), (width * height * 4) as usize);
        for y in 0..height as usize {
            for byte in 0..(width as usize * 4) {
                assert_eq!(
                    stripped[y * width as usize * 4 + byte],
                    (y * 16 + byte) as u8,
                    "row {y} byte {byte}"
                );
            }
        }
        assert!(
            !stripped.contains(&0xEE),
            "no padding byte may survive the strip"
        );
    }

    #[test]
    fn stripping_an_already_aligned_width_is_a_plain_copy() {
        let width = 64;
        let height = 2;
        let padded: Vec<u8> = (0..(width * height * 4)).map(|i| (i % 251) as u8).collect();
        assert_eq!(strip_row_padding(&padded, width, height), padded);
    }

    #[test]
    fn no_expectation_accepts_any_adapter() {
        assert!(check_expectations(&meta("vulkan", "Intel UHD 770"), None, None).is_ok());
        assert!(check_expectations(&meta("vulkan", "Intel UHD 770"), Some(""), Some("  ")).is_ok());
    }

    #[test]
    fn an_expected_adapter_matches_case_insensitively_as_a_substring() {
        let resolved = meta("vulkan", "NVIDIA T400 4GB");
        assert!(check_expectations(&resolved, Some("T400"), None).is_ok());
        assert!(check_expectations(&resolved, Some("t400"), None).is_ok());
        assert!(check_expectations(&resolved, Some(" NVIDIA T400 "), None).is_ok());
    }

    #[test]
    fn the_wrong_adapter_is_refused_naming_both_names() {
        // The dual-GPU host this exists for: the run asked for the T400 and
        // enumeration handed back the integrated GPU.
        let error = check_expectations(
            &meta("vulkan", "Intel UHD Graphics 770"),
            Some("T400"),
            None,
        )
        .expect_err("a mismatched adapter must be refused");
        let message = error.to_string();
        assert!(message.contains("T400"), "{message}");
        assert!(message.contains("Intel UHD Graphics 770"), "{message}");
        assert!(message.contains("WGPU_ADAPTER_NAME"), "{message}");
    }

    #[test]
    fn the_wrong_backend_is_refused() {
        let resolved = meta("gl", "NVIDIA T400 4GB");
        assert!(check_expectations(&resolved, None, Some("vulkan")).is_err());
        assert!(check_expectations(&resolved, None, Some("GL")).is_ok());
        assert!(check_expectations(&resolved, Some("T400"), Some("vulkan")).is_err());
    }

    #[test]
    fn the_backend_env_knob_wins_over_the_caller_hint() {
        assert_eq!(
            resolve_backends(
                Some(wgpu::Backends::VULKAN),
                Some("metal"),
                wgpu::Backends::all()
            ),
            wgpu::Backends::VULKAN
        );
        assert_eq!(
            resolve_backends(None, Some("vulkan"), wgpu::Backends::all()),
            wgpu::Backends::VULKAN
        );
        assert_eq!(
            resolve_backends(None, None, wgpu::Backends::PRIMARY),
            wgpu::Backends::PRIMARY
        );
    }

    #[test]
    fn meta_renders_the_provenance_a_baseline_must_record() {
        let info = meta("vulkan", "NVIDIA T400 4GB");
        let line = info.to_string();
        assert!(line.contains("backend=vulkan"), "{line}");
        assert!(line.contains("NVIDIA T400 4GB"), "{line}");
        assert!(line.contains("test driver"), "{line}");
    }

    /// A stand-in for what a scope pops, since a real one needs a device.
    fn validation_error(description: &str) -> wgpu::Error {
        wgpu::Error::Validation {
            source: Box::new(std::io::Error::other(description.to_string())),
            description: description.to_string(),
        }
    }

    #[test]
    fn a_clean_scope_passes_the_result_through() {
        assert_eq!(finish_scoped(Ok(7_u8), None, "render").unwrap(), 7);
        let failed: Result<u8> = Err(anyhow!("render_to_texture failed"));
        let error = finish_scoped(failed, None, "render").expect_err("the failure must survive");
        assert!(error.to_string().contains("render_to_texture failed"));
    }

    #[test]
    fn a_captured_validation_error_fails_an_otherwise_successful_operation() {
        // The case this exists for: plausible pixels produced through an error
        // wgpu's default handler would only have logged.
        let error = finish_scoped(Ok(7_u8), Some(validation_error("bad bind group")), "render")
            .expect_err("a validation error must fail the operation");
        let message = error.to_string();
        assert!(
            message.contains("validation error during render"),
            "{message}"
        );
        assert!(message.contains("bad bind group"), "{message}");
    }

    #[test]
    fn a_captured_validation_error_annotates_a_failed_operation() {
        let failed: Result<u8> = Err(anyhow!("render_to_texture failed"));
        let error = finish_scoped(failed, Some(validation_error("bad bind group")), "render")
            .expect_err("the failure must survive");
        let chain = format!("{error:#}");
        assert!(chain.contains("render_to_texture failed"), "{chain}");
        assert!(chain.contains("bad bind group"), "{chain}");
    }
}
