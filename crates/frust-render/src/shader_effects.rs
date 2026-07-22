//! Offscreen WGSL fragment-shader effects (shader-showcase feature).
//!
//! A self-contained, crate-private engine that renders user-supplied WGSL
//! fragment shaders into offscreen `Rgba8Unorm` textures. It owns everything
//! that job needs and nothing else:
//!
//! - **Lazy per-program pipeline compilation** ([`ShaderEffects::ensure_pipeline`]),
//!   seeded from the surface's [`wgpu::PipelineCache`] so a persisted cache
//!   speeds first-frame compilation exactly like vello's own pipelines.
//! - **Per-`(program, size)` target state** ([`ShaderEffects::ensure_target`]):
//!   an offscreen texture + view, a 16-byte uniform buffer, and the bind group
//!   binding them, keyed by `(id, w, h)` so a resized quad transparently
//!   replaces its target.
//! - **Fullscreen-triangle pass encoding** ([`ShaderEffects::encode_pass`]).
//! - **Size-clamp policy** ([`clamp_size`]) against vello's 8192² atlas cap
//!   (RESEARCH.md §Q1: an over-cap image is a *silent* non-render, so we must
//!   clamp proactively rather than rely on any error surfacing).
//!
//! It is deliberately decoupled from `frust-scene`: the API speaks
//! `(id: u64, wgsl: &str, size, time)` primitives, never a `Scene` or a
//! `Command`, so it parallelizes with the scene-side plumbing task. The
//! per-program registered `peniko::ImageData` handle (filled by the
//! encode-prepass task that owns `&mut vello::Renderer`) is the one payload
//! that crosses back out, via [`ShaderEffects::take_dropped_images`].
//!
//! ## Shader contract
//!
//! The caller supplies only the **fragment** source. It is compiled after a
//! fixed prelude ([`VERTEX_PRELUDE`]) that declares the uniform block and the
//! fullscreen-triangle vertex stage, so a fragment shader:
//!
//! - defines the fragment entry point **`fs_main`**
//!   (`@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32>`), and
//! - reads the uniforms as `frust_u.resolution` / `frust_u.time`.
//!
//! ## No panics
//!
//! Every path here upholds the FFI no-panic invariant
//! (`docs/CODE_STANDARDS.md`): a shader that fails to compile is recorded in
//! [`ShaderEffects::failed`] and skipped (with a rate-limited `log::warn!`)
//! rather than panicking; the underlying validation error also surfaces
//! through the device's latched uncaptured-error handler
//! (`context::RenderContext::create_device`).

use peniko::ImageData;
use std::collections::{HashMap, HashSet};

/// vello's image atlas grows to a hard 8192×8192 ceiling (RESEARCH.md §Q1,
/// `vello_encoding/src/image_cache.rs`). A target larger than this cannot be
/// packed and would *silently* fail to render, so every requested size is
/// clamped to this cap (and further to the adapter's own `max_texture_dimension_2d`).
const MAX_TEXTURE_DIM: u32 = 8192;

/// Byte size of the uniform buffer: `vec2<f32> resolution` (8) + `f32 time`
/// (4) + `f32 _pad` (4). The WGSL struct rounds its 12-byte payload up to 16
/// (RESEARCH.md §Q5), so the buffer and the Rust-side packing both use 16.
const UNIFORM_SIZE: u64 = 16;

/// How many distinct shader-compile failures are logged at warn level before
/// the warnings are suppressed — the rate limit that keeps a batch of broken
/// shaders from flooding the log. A failure past this is still recorded in
/// [`ShaderEffects::failed`] (so it is skipped), just not re-logged.
const MAX_FAILED_WARNINGS: u32 = 8;

/// The fixed WGSL prelude prepended to every fragment source: the 16-byte
/// uniform block at `@group(0) @binding(0)` and the vertex-buffer-free
/// fullscreen-triangle vertex stage (RESEARCH.md §Q5).
///
/// The fragment source appended after this must define `fs_main` and read
/// `frust_u.resolution` / `frust_u.time` (see the module-level shader contract).
const VERTEX_PRELUDE: &str = r#"// --- frust shader-effect prelude (generated) ---
struct Uniforms {
    resolution: vec2<f32>,
    time: f32,
    _pad: f32,
}
@group(0) @binding(0) var<uniform> frust_u: Uniforms;

struct FrustVsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Fullscreen triangle from the vertex index alone — no vertex buffer.
// Vertices 0,1,2 produce uv (0,0),(2,0),(0,2), covering the [-1,1] clip square
// with a single oversized triangle.
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> FrustVsOut {
    var out: FrustVsOut;
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    out.uv = uv;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}
// --- end prelude ---
"#;

/// A compiled render pipeline for one shader program plus the bind-group layout
/// its targets bind against.
struct PipelineEntry {
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
}

/// The per-`(program, size)` GPU state a shader effect renders into: an
/// offscreen `Rgba8Unorm` texture (`RENDER_ATTACHMENT | COPY_SRC` — the copy
/// source vello's override atlas reads, RESEARCH.md §Q1), its view, the 16-byte
/// uniform buffer, and the bind group wiring the buffer to `@binding(0)`.
struct TargetEntry {
    #[allow(unused)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// The `peniko::ImageData` handle registered with vello's renderer for this
    /// target, filled by the encode-prepass task that owns `&mut vello::Renderer`.
    /// Carried here so a resize/teardown can hand it back for `unregister_texture`
    /// (see [`ShaderEffects::take_dropped_images`]).
    image: Option<ImageData>,
}

/// Owns every GPU resource the shader-showcase feature needs: lazily-compiled
/// per-program pipelines, per-`(program, size)` targets, and the dropped-image
/// handoff queue. Crate-private — no `wgpu`/`vello` type escapes `frust-render`.
pub(crate) struct ShaderEffects {
    /// Compiled pipelines, keyed by program id.
    pipelines: HashMap<u64, PipelineEntry>,
    /// Offscreen targets, keyed by `(program id, width, height)`.
    targets: HashMap<(u64, u32, u32), TargetEntry>,
    /// A clone of the surface's pipeline cache, seeding pipeline compilation.
    pipeline_cache: Option<wgpu::PipelineCache>,
    /// Program ids whose shader failed to compile — skipped on every later
    /// `ensure_pipeline` so a broken shader is compiled at most once.
    failed: HashSet<u64>,
    /// Images belonging to evicted targets, awaiting `unregister_texture` by the
    /// caller (which owns the vello renderer). Drained by [`take_dropped_images`].
    ///
    /// [`take_dropped_images`]: ShaderEffects::take_dropped_images
    dropped_images: Vec<ImageData>,
    /// How many compile failures have been logged so far (the [`MAX_FAILED_WARNINGS`]
    /// rate limit).
    warned_failures: u32,
}

/// Clamp a requested target size to the renderable range: at most
/// [`MAX_TEXTURE_DIM`] (vello's atlas cap) and the adapter's own
/// `max_texture_dimension_2d`, and at least 1 per axis (a zero-sized texture is
/// invalid). Pure — no GPU state touched, so the policy is unit-testable.
///
/// vello's atlas overflow is a *silent* non-render (RESEARCH.md §Q1), so this
/// clamp is the sole guard against an over-large quad simply vanishing; the
/// caller warns once when a clamp actually changes the size.
pub(crate) fn clamp_size(requested: (u32, u32), adapter_max: u32) -> (u32, u32) {
    let cap = MAX_TEXTURE_DIM.min(adapter_max);
    let clamp = |v: u32| v.clamp(1, cap.max(1));
    (clamp(requested.0), clamp(requested.1))
}

/// Pack the 16-byte uniform buffer contents: `resolution.x`, `resolution.y`,
/// `time`, `_pad` as little-endian `f32`s. Pure — unit-testable byte layout.
fn uniform_bytes(width: u32, height: u32, time: f32) -> [u8; UNIFORM_SIZE as usize] {
    let mut bytes = [0u8; UNIFORM_SIZE as usize];
    bytes[0..4].copy_from_slice(&(width as f32).to_le_bytes());
    bytes[4..8].copy_from_slice(&(height as f32).to_le_bytes());
    bytes[8..12].copy_from_slice(&time.to_le_bytes());
    // bytes[12..16] stays zero — the explicit `_pad` field.
    bytes
}

/// Concatenate the fixed [`VERTEX_PRELUDE`] and a caller-supplied fragment
/// source into the full WGSL module compiled for a program. Pure.
fn compose_shader(fragment_src: &str) -> String {
    format!("{VERTEX_PRELUDE}\n{fragment_src}")
}

/// The keys of same-`id` targets whose size differs from `(w, h)` — the entries
/// a resize evicts (a resized quad replaces its target). Pure: operates on the
/// key set alone, so the eviction policy is GPU-free unit-testable.
fn stale_target_keys<I>(keys: I, id: u64, w: u32, h: u32) -> Vec<(u64, u32, u32)>
where
    I: IntoIterator<Item = (u64, u32, u32)>,
{
    keys.into_iter()
        .filter(|&(tid, tw, th)| tid == id && (tw, th) != (w, h))
        .collect()
}

/// Whether a compile failure at `prior_warn_count` prior warnings should still
/// be logged (the [`MAX_FAILED_WARNINGS`] rate limit). Pure.
fn should_warn_failure(prior_warn_count: u32) -> bool {
    prior_warn_count < MAX_FAILED_WARNINGS
}

impl ShaderEffects {
    /// Create an empty engine seeded with an optional clone of the surface's
    /// [`wgpu::PipelineCache`] (used as `RenderPipelineDescriptor.cache` so a
    /// persisted cache speeds first compilation). `None` on adapters without
    /// `PIPELINE_CACHE` support (Metal/desktop), which is a plain cold start.
    pub(crate) fn new(pipeline_cache: Option<wgpu::PipelineCache>) -> Self {
        Self {
            pipelines: HashMap::new(),
            targets: HashMap::new(),
            pipeline_cache,
            failed: HashSet::new(),
            dropped_images: Vec::new(),
            warned_failures: 0,
        }
    }

    /// Whether program `id` still needs its pipeline compiled — false once it is
    /// either compiled or known-failed. Pure predicate (the fast-path skip
    /// [`ensure_pipeline`] consults first), unit-testable without a device.
    pub(crate) fn needs_compile(&self, id: u64) -> bool {
        !self.pipelines.contains_key(&id) && !self.failed.contains(&id)
    }

    /// Lazily compile the render pipeline for program `id` from `wgsl` (the
    /// fragment source; see the module-level shader contract). A no-op if the
    /// program is already compiled or already known-failed.
    ///
    /// Compilation is wrapped in a `Validation` error scope: a shader that fails
    /// to validate records `id` in [`failed`](Self::failed) (skipped forever
    /// after) with a rate-limited `log::warn!`, and **never panics** — the FFI
    /// no-panic invariant. The validation error also surfaces through the
    /// device's latched uncaptured-error handler.
    pub(crate) fn ensure_pipeline(&mut self, device: &wgpu::Device, id: u64, wgsl: &str) {
        if !self.needs_compile(id) {
            return;
        }

        let source = compose_shader(wgsl);
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frust shader-effect module"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frust shader-effect binds"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("frust shader-effect layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("frust shader-effect pipeline"),
            layout: Some(&pipeline_layout),
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
                // Opaque v1 contract (RESEARCH.md §Q1): no blending, so a
                // shader's premultiplied output at alpha=1.0 is written straight.
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: self.pipeline_cache.as_ref(),
        });

        if let Some(error) = drain_error_scope(device, scope) {
            self.record_failed(id, &error.to_string());
            return;
        }

        self.pipelines.insert(
            id,
            PipelineEntry {
                pipeline,
                bind_layout,
            },
        );
    }

    /// Record a compile failure: mark `id` skipped and warn (rate-limited).
    fn record_failed(&mut self, id: u64, message: &str) {
        self.failed.insert(id);
        if should_warn_failure(self.warned_failures) {
            self.warned_failures += 1;
            log::warn!("frust-render: shader program {id} failed to compile, skipping: {message}");
        }
    }

    /// Ensure a target exists for `(id, w, h)`, creating it if absent. Any
    /// other-size target for the same `id` is evicted (a resized quad replaces
    /// its target), and its registered image (if any) is queued for
    /// `unregister_texture` — see [`take_dropped_images`](Self::take_dropped_images).
    ///
    /// A no-op if program `id` has no compiled pipeline (never compiled, or
    /// compile-failed): a target is useless without the pipeline that owns its
    /// bind-group layout, so the caller compiles first.
    pub(crate) fn ensure_target(&mut self, device: &wgpu::Device, id: u64, w: u32, h: u32) {
        let key = (id, w, h);
        if self.targets.contains_key(&key) {
            return;
        }

        // Evict other-size entries for this id, handing their images back.
        for stale in stale_target_keys(self.targets.keys().copied(), id, w, h) {
            if let Some(entry) = self.targets.remove(&stale)
                && let Some(image) = entry.image
            {
                self.dropped_images.push(image);
            }
        }

        let Some(pipeline_entry) = self.pipelines.get(&id) else {
            return;
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frust shader-effect target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
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
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frust shader-effect uniforms"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frust shader-effect bind group"),
            layout: &pipeline_entry.bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });

        self.targets.insert(
            key,
            TargetEntry {
                texture,
                view,
                uniforms,
                bind_group,
                image: None,
            },
        );
    }

    /// Encode one fullscreen-triangle pass for program `id` at `size` into its
    /// target, writing the uniform buffer (`resolution`, `time`) first. A no-op
    /// if the program's pipeline or the `(id, size)` target is missing. Adds no
    /// `queue.submit` — the caller owns encoder creation and submission ordering
    /// relative to vello.
    pub(crate) fn encode_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        id: u64,
        size: (u32, u32),
        time: f32,
    ) {
        let (w, h) = size;
        let (Some(pipeline_entry), Some(target)) =
            (self.pipelines.get(&id), self.targets.get(&(id, w, h)))
        else {
            return;
        };

        queue.write_buffer(&target.uniforms, 0, &uniform_bytes(w, h, time));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frust shader-effect pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
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
        pass.set_pipeline(&pipeline_entry.pipeline);
        pass.set_bind_group(0, &target.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Drain the images of evicted targets, transferring ownership to the caller
    /// so it can `unregister_texture` them against the vello renderer it owns.
    pub(crate) fn take_dropped_images(&mut self) -> Vec<ImageData> {
        std::mem::take(&mut self.dropped_images)
    }
}

/// Synchronously drain a wgpu error scope, returning the captured validation
/// error (if any). On native wgpu the [`pop`](wgpu::ErrorScopeGuard::pop) future
/// is ready as soon as the synchronously-captured creation error is recorded, so
/// this resolves without an async runtime; the `device.poll` fallback drives any
/// residual pending state to completion rather than spinning. Never panics.
fn drain_error_scope(device: &wgpu::Device, scope: wgpu::ErrorScopeGuard) -> Option<wgpu::Error> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_size_caps_at_8192() {
        assert_eq!(clamp_size((10_000, 10_000), u32::MAX), (8192, 8192));
    }

    #[test]
    fn clamp_size_respects_adapter_max_below_cap() {
        // A 4096-max adapter clamps below the 8192 vello cap.
        assert_eq!(clamp_size((6000, 6000), 4096), (4096, 4096));
    }

    #[test]
    fn clamp_size_floors_zero_to_one() {
        assert_eq!(clamp_size((0, 0), 8192), (1, 1));
        assert_eq!(clamp_size((0, 512), 8192), (1, 512));
    }

    #[test]
    fn clamp_size_passes_through_in_range() {
        assert_eq!(clamp_size((1290, 2796), 16384), (1290, 2796));
    }

    #[test]
    fn clamp_size_survives_zero_adapter_max() {
        // A degenerate adapter_max of 0 must never produce a 0 dimension.
        assert_eq!(clamp_size((100, 100), 0), (1, 1));
    }

    #[test]
    fn uniform_bytes_is_16_and_little_endian() {
        let bytes = uniform_bytes(1920, 1080, 2.5);
        assert_eq!(bytes.len(), 16);
        assert_eq!(f32::from_le_bytes(bytes[0..4].try_into().unwrap()), 1920.0);
        assert_eq!(f32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1080.0);
        assert_eq!(f32::from_le_bytes(bytes[8..12].try_into().unwrap()), 2.5);
        // The explicit `_pad` word is zero.
        assert_eq!(&bytes[12..16], &[0, 0, 0, 0]);
    }

    #[test]
    fn compose_shader_prepends_prelude_and_keeps_fragment() {
        let frag = "@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> \
                    { return vec4<f32>(frust_u.time, 0.0, 0.0, 1.0); }";
        let full = compose_shader(frag);
        assert!(full.starts_with(VERTEX_PRELUDE));
        assert!(full.contains("fn vs_main"));
        assert!(full.contains("var<uniform> frust_u: Uniforms"));
        assert!(full.contains(frag));
        // The fragment is appended after the prelude, never before it.
        assert!(full.find("fn vs_main").unwrap() < full.find(frag).unwrap());
    }

    #[test]
    fn needs_compile_tracks_pipeline_and_failed_sets() {
        let mut fx = ShaderEffects::new(None);
        assert!(fx.needs_compile(7));
        // A recorded failure skips further compilation of that id.
        fx.record_failed(7, "boom");
        assert!(!fx.needs_compile(7));
        assert!(fx.failed.contains(&7));
        // A different id is still eligible.
        assert!(fx.needs_compile(8));
    }

    #[test]
    fn record_failed_warns_only_up_to_the_rate_limit() {
        let mut fx = ShaderEffects::new(None);
        // Distinct failing ids past the cap: every id is recorded as failed, but
        // warnings stop at MAX_FAILED_WARNINGS.
        for id in 0..(MAX_FAILED_WARNINGS + 5) as u64 {
            fx.record_failed(id, "bad shader");
        }
        assert_eq!(fx.warned_failures, MAX_FAILED_WARNINGS);
        assert_eq!(fx.failed.len(), (MAX_FAILED_WARNINGS + 5) as usize);
    }

    #[test]
    fn should_warn_failure_stops_at_cap() {
        assert!(should_warn_failure(0));
        assert!(should_warn_failure(MAX_FAILED_WARNINGS - 1));
        assert!(!should_warn_failure(MAX_FAILED_WARNINGS));
        assert!(!should_warn_failure(MAX_FAILED_WARNINGS + 1));
    }

    #[test]
    fn stale_target_keys_selects_only_other_sizes_of_same_id() {
        let keys = [
            (1, 100, 100), // same id, same size — kept
            (1, 200, 200), // same id, other size — evicted
            (1, 100, 200), // same id, other size — evicted
            (2, 100, 100), // other id — kept
        ];
        let mut stale = stale_target_keys(keys.iter().copied(), 1, 100, 100);
        stale.sort();
        assert_eq!(stale, vec![(1, 100, 200), (1, 200, 200)]);
    }

    #[test]
    fn stale_target_keys_empty_when_no_prior_target() {
        assert!(stale_target_keys(std::iter::empty(), 1, 100, 100).is_empty());
    }

    fn dummy_image(marker: u8) -> ImageData {
        ImageData {
            data: peniko::Blob::from(vec![marker; 2 * 2 * 4]),
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::Alpha,
            width: 2,
            height: 2,
        }
    }

    #[test]
    fn take_dropped_images_drains_the_handoff_queue() {
        let mut fx = ShaderEffects::new(None);
        assert!(fx.take_dropped_images().is_empty());
        // Simulate an eviction having queued an image for unregister.
        fx.dropped_images.push(dummy_image(1));
        fx.dropped_images.push(dummy_image(2));
        let taken = fx.take_dropped_images();
        assert_eq!(taken.len(), 2);
        // Draining leaves the queue empty for the next frame.
        assert!(fx.take_dropped_images().is_empty());
    }
}
