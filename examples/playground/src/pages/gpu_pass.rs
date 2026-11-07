//! The "GPU pass" section: an animated [`ExternalPass`] drawn through the
//! public facade only (`frust` with its `gpu` feature).
//!
//! [`SpinningQuad`] renders a procedural checker texture onto a rotating quad,
//! over a background colour driven by the frame counter, into a target of its
//! own, and binds that target under a scene-texture id. The page draws the id
//! with [`texture_view`], and reports what the seam exposes: the published
//! device generation, whether the optional `SHADER_F16` feature the app asked
//! for was granted, and the pass's own drain / frame counters.
//!
//! # Mount lifetime and frame requests
//!
//! The pass is registered in [`Component::init`] and unregistered by an
//! [`on_cleanup`] registered beside it, so it lives exactly as long as the
//! page's reactive owner. `pattern_switcher` tears the outgoing section down
//! on the very rebuild that changes its key (the section stays *painted* for
//! the length of the exit animation, but is unmounted), so `on_cleanup` runs
//! when the user leaves the section, not when the slide ends. A pass can still
//! be called once more by a drain already in flight, so the pass also carries a
//! `live` flag the same cleanup clears, and [`SpinningQuad::record`] only asks
//! for another frame while it is set: leaving the section stops continuous
//! frames.
//!
//! # Frame driving
//!
//! A registered pass does not keep frames coming by itself. `record` calls
//! [`frust::texture::request_frame`] because it runs on the render thread (or
//! wherever the draining surface renders), where there is no `PaintCtx`;
//! `request_frame` is callable from any thread and coalesces repeated calls
//! into one owed frame.
//!
//! # Per-surface state
//!
//! The pass keeps one [`Target`] per [`ExternalFrame::drain_id`] (one per
//! engine-tier surface). A target is rebuilt when the frame's
//! [`ExternalFrame::device_generation`] or the wanted extent differs from the
//! one it was built under, and no device-owned object outlives a generation.
//!
//! The readout text is built from the counters when the page is built; the
//! **Refresh readout** button rebuilds it.
//!
//! # Memory pressure
//!
//! The page keeps an [`AppLifecycleListener`] with an `on_memory_pressure` hook.
//! Each event bumps a count and records the last level, which the readout
//! shows as `memory pressure: N (last: …)`, and logs one `memory pressure:
//! <level>` line. On [`MemoryPressure::Critical`] the hook also sets the pass's
//! drop flag; [`SpinningQuad::record`] takes it at its top and clears the
//! per-drain target map, so every target is rebuilt on the next record.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use frust::authoring::Size;
use frust::gpu::wgpu;
use frust::gpu::{
    ExternalFrame, ExternalPass, SceneTextureId, register_external_pass, unregister_external_pass,
    with_context,
};
use frust::{
    AppLifecycleListener, Axis, ButtonStyle, Color, Component, EdgeInsets, FlexChild, FlexView,
    Get, MemoryPressure, Padding, RwSignal, SizedBox, Theme, Update, View, button, inflexible,
    on_cleanup, text, texture_view, use_context,
};

/// The target's extent in texels (4:3, matching the on-screen box).
const EXTENT: (u32, u32) = (640, 480);
/// The on-screen box, in logical px.
const BOX_W: f64 = 320.0;
const BOX_H: f64 = 240.0;
/// Side of the procedural checker texture, in texels.
const CHECKER: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One surface's device-owned objects.
struct Target {
    /// Device generation this target was built under.
    generation: u64,
    extent: (u32, u32),
    /// Kept alive beside its view.
    _color: wgpu::Texture,
    view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    params: wgpu::Buffer,
    /// Kept alive beside the bind group that samples it.
    _checker: wgpu::Texture,
    /// This pass's record counter value when this target last drew.
    last_record: u64,
    /// Frames this target has drawn: the animation clock (see `record`).
    frames: u64,
}

impl Target {
    fn new(frame: &ExternalFrame<'_>, extent: (u32, u32)) -> Self {
        let device = frame.device();
        let queue = frame.queue();

        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gpu pass target"),
            size: wgpu::Extent3d {
                width: extent.0,
                height: extent.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = color.create_view(&wgpu::TextureViewDescriptor::default());

        let checker = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gpu pass checker"),
            size: wgpu::Extent3d {
                width: CHECKER,
                height: CHECKER,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &checker,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &checker_texels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(CHECKER * 4),
                rows_per_image: Some(CHECKER),
            },
            wgpu::Extent3d {
                width: CHECKER,
                height: CHECKER,
                depth_or_array_layers: 1,
            },
        );
        let checker_view = checker.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gpu pass sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu pass params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpu pass shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu_pass.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gpu pass pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    // Opaque output: replace, nothing to blend inside the target.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu pass bind group"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&checker_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        Self {
            generation: frame.device_generation(),
            extent,
            _color: color,
            view,
            pipeline,
            bind_group,
            params,
            _checker: checker,
            last_record: 0,
            frames: 0,
        }
    }
}

/// A 64x64 two-tone checker, premultiplied (fully opaque).
fn checker_texels() -> Vec<u8> {
    let mut texels = Vec::with_capacity((CHECKER * CHECKER * 4) as usize);
    for y in 0..CHECKER {
        for x in 0..CHECKER {
            let on = ((x / 8) + (y / 8)) % 2 == 0;
            let rgba: [u8; 4] = if on {
                [250, 190, 60, 255]
            } else {
                [40, 60, 150, 255]
            };
            texels.extend_from_slice(&rgba);
        }
    }
    texels
}

/// A target not drawn within this many records of the pass is dropped: its
/// drain (an engine-tier surface) stopped drawing.
const STALE_AFTER: u64 = 240;

/// The animated external pass: a rotating checker quad over a cycling
/// background colour. See the [module docs](self).
pub struct SpinningQuad {
    /// Per-surface targets, keyed by [`ExternalFrame::drain_id`]. Entries of a
    /// device generation that is no longer current are dropped whenever a
    /// target is rebuilt.
    targets: Mutex<HashMap<u64, Target>>,
    /// Cleared on teardown; while clear, `record` stops asking for frames.
    live: AtomicBool,
    /// Set by the page on a `Critical` memory-pressure event; `record` takes it
    /// and clears `targets`, so every target is rebuilt on the next record.
    drop_targets: Arc<AtomicBool>,
    /// The last `drain_id` / `frame_index` this pass recorded, and how many
    /// times it has recorded; `0` drain id means "never recorded".
    last_drain: AtomicU64,
    last_frame: AtomicU64,
    records: AtomicU64,
}

impl Default for SpinningQuad {
    fn default() -> Self {
        Self::new()
    }
}

impl SpinningQuad {
    /// A live pass that has recorded nothing yet.
    pub fn new() -> Self {
        Self {
            targets: Mutex::new(HashMap::new()),
            live: AtomicBool::new(true),
            drop_targets: Arc::new(AtomicBool::new(false)),
            last_drain: AtomicU64::new(0),
            last_frame: AtomicU64::new(0),
            records: AtomicU64::new(0),
        }
    }

    /// Stops the pass asking for further frames.
    pub fn retire(&self) {
        self.live.store(false, Ordering::Release);
    }

    /// The flag a memory-pressure hook sets to ask the pass to drop its targets.
    pub fn drop_targets_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.drop_targets)
    }

    /// `(last drain id, last frame index, record count)`.
    pub fn counters(&self) -> (u64, u64, u64) {
        (
            self.last_drain.load(Ordering::Relaxed),
            self.last_frame.load(Ordering::Relaxed),
            self.records.load(Ordering::Relaxed),
        )
    }
}

impl ExternalPass for SpinningQuad {
    fn record(&self, frame: &mut ExternalFrame<'_>) {
        let drain = frame.drain_id();
        let generation = frame.device_generation();
        let index = frame.frame_index();
        let record_no = self.records.load(Ordering::Relaxed);

        let mut targets = self.targets.lock().unwrap_or_else(PoisonError::into_inner);
        // Memory pressure asked for the targets to go: rebuild them all below.
        take_drop_request(&self.drop_targets, &mut *targets);
        let rebuild = targets
            .get(&drain)
            .is_none_or(|t| t.generation != generation || t.extent != EXTENT);
        if rebuild {
            // Nothing built under another generation may be kept.
            targets.retain(|_, t| t.generation == generation);
        }
        // Drop targets of drains that stopped drawing.
        targets.retain(|&key, t| {
            key == drain || record_no.saturating_sub(t.last_record) <= STALE_AFTER
        });
        let target = match targets.entry(drain) {
            Entry::Occupied(entry) if !rebuild => entry.into_mut(),
            Entry::Occupied(mut entry) => {
                entry.insert(Target::new(frame, EXTENT));
                entry.into_mut()
            }
            Entry::Vacant(entry) => entry.insert(Target::new(frame, EXTENT)),
        };

        target.last_record = record_no;
        // Animated from a count kept on this drain's target, not from
        // `frame.frame_index()`: that index is process-wide on a surface, so
        // it advances for every other drain and pass too and would make this
        // quad spin faster the more surfaces are live.
        let t = target.frames as f64;
        target.frames += 1;
        let angle = (t * 0.03) as f32;
        let aspect = EXTENT.0 as f32 / EXTENT.1 as f32;
        let mut params = [0u8; 8];
        params[..4].copy_from_slice(&angle.to_le_bytes());
        params[4..].copy_from_slice(&aspect.to_le_bytes());
        frame.queue().write_buffer(&target.params, 0, &params);

        let background = wgpu::Color {
            r: 0.2 + 0.15 * (t * 0.02).sin(),
            g: 0.2 + 0.15 * (t * 0.02 + 2.1).sin(),
            b: 0.25 + 0.15 * (t * 0.02 + 4.2).sin(),
            a: 1.0,
        };
        {
            let mut pass = frame
                .encoder()
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("gpu pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(background),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            pass.set_pipeline(&target.pipeline);
            pass.set_bind_group(0, &target.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        // Bound only after the target holds content for this frame.
        let view = target.view.clone();
        drop(targets);
        frame.bind_texture(EXTENT, view);

        self.last_drain.store(drain, Ordering::Relaxed);
        self.last_frame.store(index, Ordering::Relaxed);
        self.records.fetch_add(1, Ordering::Relaxed);

        // `record` has no `PaintCtx`; `request_frame` is callable from any
        // thread and coalesces into one owed frame. Only while the page lives.
        if self.live.load(Ordering::Acquire) {
            frust::texture::request_frame();
        }
    }
}

/// The page's state: the pass, its id, a counter the readout button bumps, and
/// the memory-pressure readout (event count, last level) with its listener.
pub struct GpuPassState {
    id: SceneTextureId,
    pass: Arc<SpinningQuad>,
    refreshes: u32,
    pressure: RwSignal<(u32, Option<MemoryPressure>)>,
    _pressure: AppLifecycleListener,
}

/// Takes a pending drop request: when `flag` was set, clears the per-drain
/// target map and resets the flag, so a request is honoured exactly once.
/// Generic over the target type so the test can drive it without a GPU.
fn take_drop_request<T>(flag: &AtomicBool, targets: &mut HashMap<u64, T>) {
    if flag.swap(false, Ordering::AcqRel) {
        targets.clear();
    }
}

/// Whether a memory-pressure level asks the pass to drop its per-drain targets.
fn drops_targets(level: MemoryPressure) -> bool {
    level == MemoryPressure::Critical
}

/// The memory-pressure readout line: the event count and the last level.
fn pressure_readout((count, last): (u32, Option<MemoryPressure>)) -> String {
    let last = last.map_or_else(|| "none".to_owned(), |level| format!("{level:?}"));
    format!("memory pressure: {count} (last: {last})")
}

/// The GPU pass page's own [`Component`] (see the [module docs](self)).
pub struct GpuPassPage;

impl Component for GpuPassPage {
    type State = GpuPassState;

    fn init(&self) -> GpuPassState {
        let id = SceneTextureId::mint();
        let pass = Arc::new(SpinningQuad::new());
        register_external_pass(id, pass.clone());
        let owned = pass.clone();
        on_cleanup(move || {
            owned.retire();
            unregister_external_pass(id);
        });

        let pressure = RwSignal::new((0, None));
        let drop_targets = pass.drop_targets_flag();
        // Dropping the listener (with the state) unregisters the hook.
        let listener = AppLifecycleListener::new().on_memory_pressure(move |level| {
            log::info!("memory pressure: {level:?}");
            if drops_targets(level) {
                drop_targets.store(true, Ordering::Release);
            }
            pressure.update(|(count, last)| {
                *count += 1;
                *last = Some(level);
            });
        });

        GpuPassState {
            id,
            pass,
            refreshes: 0,
            pressure,
            _pressure: listener,
        }
    }

    fn build(&self, state: &mut GpuPassState) -> impl View<GpuPassState> {
        let (drain, frame, records) = state.pass.counters();
        let pressure = pressure_readout(state.pressure.get());
        let generation = frust::gpu::device_generation().map_or_else(
            || "none yet".to_owned(),
            |generation| generation.to_string(),
        );
        let f16 = with_context(|handle| {
            handle
                .device
                .features()
                .contains(wgpu::Features::SHADER_F16)
        });
        let f16 = match f16 {
            Some(true) => "granted",
            Some(false) => "not granted",
            None => "no device published",
        };

        let children: Vec<FlexChild<GpuPassState>> = vec![
            inflexible(text("GPU pass").size(13.0).color(accent())),
            gap(6.0),
            inflexible(
                text(
                    "An ExternalPass drawing a rotating textured quad into its own target, \
                     composited here as a scene texture.",
                )
                .size(11.0)
                .color(muted()),
            ),
            gap(12.0),
            inflexible(
                texture_view(state.id.get())
                    .size(Size::new(BOX_W, BOX_H))
                    .corner_radius(8.0)
                    .label("GPU pass picture"),
            ),
            gap(12.0),
            inflexible(text(format!("Device generation: {generation}")).size(12.0)),
            inflexible(text(format!("SHADER_F16: {f16}")).size(12.0)),
            inflexible(
                text(format!(
                    "Last drain: {drain}  frame: {frame}  records: {records}"
                ))
                .size(12.0),
            ),
            inflexible(text(pressure).size(13.0)),
            inflexible(text(format!("Readout refreshed {} times", state.refreshes)).size(11.0)),
            gap(6.0),
            inflexible(
                button("Refresh readout", |s: &mut GpuPassState| s.refreshes += 1)
                    .style(ButtonStyle::Secondary)
                    .small(),
            ),
        ];
        Padding(
            EdgeInsets::all(12.0),
            FlexView::new(Axis::Vertical, children),
        )
    }
}

fn gap(h: f64) -> FlexChild<GpuPassState> {
    inflexible(SizedBox(None, Some(h)))
}

fn accent() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .primary
}

fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .on_surface_variant
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_critical_drops_the_targets() {
        assert!(drops_targets(MemoryPressure::Critical));
        assert!(!drops_targets(MemoryPressure::Moderate));
    }

    #[test]
    fn readout_names_the_count_and_last_level() {
        assert_eq!(
            pressure_readout((0, None)),
            "memory pressure: 0 (last: none)"
        );
        assert_eq!(
            pressure_readout((3, Some(MemoryPressure::Moderate))),
            "memory pressure: 3 (last: Moderate)"
        );
        assert_eq!(
            pressure_readout((4, Some(MemoryPressure::Critical))),
            "memory pressure: 4 (last: Critical)"
        );
    }

    #[test]
    fn drop_flag_is_taken_once() {
        let pass = SpinningQuad::new();
        let flag = pass.drop_targets_flag();
        // Unit targets stand in for the GPU `Target`s the map holds in `record`.
        let mut targets: HashMap<u64, ()> = HashMap::from([(1, ()), (2, ())]);

        // An unset flag leaves a non-empty map alone.
        take_drop_request(&pass.drop_targets, &mut targets);
        assert_eq!(targets.len(), 2);

        // A set flag clears the map once, and is reset by taking it.
        flag.store(true, Ordering::Release);
        take_drop_request(&pass.drop_targets, &mut targets);
        assert!(targets.is_empty());
        assert!(!pass.drop_targets.load(Ordering::Acquire));

        // A second take with the flag now clear does not touch a refilled map.
        targets.insert(3, ());
        take_drop_request(&pass.drop_targets, &mut targets);
        assert_eq!(targets.len(), 1);
    }
}
