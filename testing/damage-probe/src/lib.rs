//! Damage-aware presentation probe for gfx-rs/wgpu#10152.
//!
//! The whole frame is always rendered (the PR's contract: "the texture must
//! still contain the whole frame"). What varies is the *hint* handed to
//! `Queue::present_with_damage`:
//!
//! - `none`  — plain `Queue::present`, the control;
//! - `exact` — the square's old and new bounds (what a correct client sends);
//! - `under` — only the NEW bounds, the vacated old position deliberately
//!   unreported. A backend + compositor that honours the hint never repaints the
//!   old position on screen, so a trail of stale squares stays visible; a
//!   backend that ignores the hint shows a clean frame in every mode.
//!
//! `full_every` reports the whole surface every nth frame so a trail is wiped
//! periodically — proof that the trail is the compositor's, not the probe's.

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// Which damage hint the probe hands to the present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageMode {
    /// `Queue::present` — no hint at all (control).
    None,
    /// Old and new bounds of the moving square — a correct hint.
    Exact,
    /// New bounds only — the vacated area is deliberately unreported.
    Under,
}

impl DamageMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" => Some(Self::None),
            "exact" | "on" => Some(Self::Exact),
            "under" => Some(Self::Under),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Exact => "exact",
            Self::Under => "under",
        }
    }
}

/// Probe configuration, from the CLI (desktop) or system properties (Android).
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub mode: DamageMode,
    /// Report the whole surface every nth frame (0 = never).
    pub full_every: u32,
    /// Pixels the square advances per frame.
    pub step: u32,
    /// Square edge in pixels.
    pub size: u32,
    /// Frame pacing cap; `None` = as fast as the swapchain allows (vsync).
    pub fps: Option<u32>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: DamageMode::Exact,
            full_every: 0,
            step: 4,
            size: 48,
            fps: None,
        }
    }
}

const BACKGROUND: wgpu::Color = wgpu::Color {
    r: 0.08,
    g: 0.09,
    b: 0.11,
    a: 1.0,
};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    surface: [f32; 2],
    _pad: [f32; 2],
    rects: [[f32; 4]; 2],
    colors: [[f32; 4]; 2],
}

struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    backend: wgpu::Backend,
    adapter_name: String,
}

impl Gpu {
    fn new(window: Arc<Window>) -> Result<Self, String> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("create_surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| format!("request_adapter: {e}"))?;
        let info = adapter.get_info();
        log::info!(
            "adapter: {} | backend {:?} | driver {} {} | type {:?}",
            info.name,
            info.backend,
            info.driver,
            info.driver_info,
            info.device_type
        );
        log_incremental_present_support(&adapter);

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("damage-probe"),
            required_limits:
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| format!("request_device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.remove_srgb_suffix() == *f)
            .unwrap_or(caps.formats[0]);
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Fifo) {
            wgpu::PresentMode::Fifo
        } else {
            caps.present_modes[0]
        };
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes[0]
        };
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        log::info!(
            "surface: {:?} {}x{} present {:?} alpha {:?} (caps: formats {:?}, present {:?}, alpha {:?})",
            format,
            config.width,
            config.height,
            present_mode,
            alpha_mode,
            caps.formats,
            caps.present_modes,
            caps.alpha_modes
        );

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("probe.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("probe.wgsl").into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniforms"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("probe"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("probe"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            uniforms,
            bind_group,
            backend: info.backend,
            adapter_name: info.name,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }
}

/// On Vulkan, whether the adapter advertises `VK_KHR_incremental_present` — the
/// PR enables it on the device whenever the adapter has it, so this is the
/// "will the hint reach the driver" question. Metal and DX12 have no equivalent.
fn log_incremental_present_support(adapter: &wgpu::Adapter) {
    #[cfg(not(target_vendor = "apple"))]
    {
        // SAFETY: `as_hal` only asks that the hal adapter not outlive the wgpu
        // adapter and that no hal state is mutated; the guard is dropped inside
        // this expression after a read-only capability query.
        let supported = unsafe { adapter.as_hal::<wgpu::hal::api::Vulkan>() }.map(|hal_adapter| {
            hal_adapter
                .physical_device_capabilities()
                .supports_extension(c"VK_KHR_incremental_present")
        });
        match supported {
            Some(true) => log::info!(
                "VK_KHR_incremental_present: supported by the adapter (the PR enables it)"
            ),
            Some(false) => log::info!(
                "VK_KHR_incremental_present: NOT supported by the adapter — rects will be dropped"
            ),
            None => log::info!("VK_KHR_incremental_present: n/a (not a Vulkan adapter)"),
        }
    }
    #[cfg(target_vendor = "apple")]
    {
        let _ = adapter;
        log::info!("VK_KHR_incremental_present: n/a on Metal — the PR ignores the rects here");
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl Rect {
    fn clipped(self, width: u32, height: u32) -> Option<wgpu::Rect<u32>> {
        let x1 = self.x.min(width);
        let y1 = self.y.min(height);
        let x2 = self.x.saturating_add(self.w).min(width);
        let y2 = self.y.saturating_add(self.h).min(height);
        if x2 > x1 && y2 > y1 {
            Some(wgpu::Rect {
                x: x1,
                y: y1,
                w: x2 - x1,
                h: y2 - y1,
            })
        } else {
            None
        }
    }
}

/// Move the square one step along its track, remembering where it was. A free
/// function over the two rects so it can run while the GPU side is borrowed —
/// it must run only once a frame has actually been acquired, or a skipped
/// frame would move the square without presenting it and the next `exact`
/// hint would name a position that was never on screen.
fn step_square(square: &mut Rect, prev: &mut Rect, cfg: &Config, width: u32, height: u32) {
    *prev = *square;
    let row_pitch = cfg.size + 8;
    let mut x = square.x + cfg.step;
    let mut y = square.y;
    if x + cfg.size > width {
        x = 0;
        y += row_pitch;
        if y + cfg.size > height {
            y = 0;
        }
    }
    square.x = x;
    square.y = y;
}

struct App {
    cfg: Config,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    frame: u64,
    square: Rect,
    prev_square: Rect,
    /// The next present reports the whole surface (first frame, resize, recreate).
    force_full: bool,
    next_deadline: Instant,
    last_log: Instant,
    frames_since_log: u32,
}

impl App {
    fn new(cfg: Config) -> Self {
        Self {
            cfg,
            window: None,
            gpu: None,
            frame: 0,
            square: Rect {
                x: 0,
                y: 0,
                w: cfg.size,
                h: cfg.size,
            },
            prev_square: Rect::default(),
            force_full: true,
            next_deadline: Instant::now(),
            last_log: Instant::now(),
            frames_since_log: 0,
        }
    }

    fn render(&mut self) {
        let Some(gpu) = self.gpu.as_mut() else { return };
        let (width, height) = (gpu.config.width, gpu.config.height);

        if let Some(fps) = self.cfg.fps {
            let now = Instant::now();
            if now < self.next_deadline {
                std::thread::sleep(self.next_deadline - now);
            }
            self.next_deadline = Instant::now() + Duration::from_secs_f64(1.0 / fps.max(1) as f64);
        }

        use wgpu::CurrentSurfaceTexture as Cst;
        let texture = match gpu.surface.get_current_texture() {
            Cst::Success(t) => t,
            Cst::Suboptimal(t) => {
                self.force_full = true;
                t
            }
            Cst::Outdated | Cst::Lost => {
                // Reconfigure at the window's current size, not the stale config.
                let size = self.window.as_ref().map(|w| w.inner_size());
                let (w, h) = size.map_or((width, height), |s| (s.width, s.height));
                gpu.resize(w, h);
                self.force_full = true;
                return;
            }
            Cst::Timeout | Cst::Occluded => return,
            other => {
                log::error!("get_current_texture: {other:?}");
                return;
            }
        };

        // Only now — a frame is acquired and will be presented — does the square move.
        step_square(
            &mut self.square,
            &mut self.prev_square,
            &self.cfg,
            width,
            height,
        );
        let marker = Rect {
            x: width.saturating_sub(32),
            y: 8,
            w: 24,
            h: 24,
        };
        let uniforms = Uniforms {
            surface: [width as f32, height as f32],
            _pad: [0.0; 2],
            rects: [
                [
                    self.square.x as f32,
                    self.square.y as f32,
                    self.square.w as f32,
                    self.square.h as f32,
                ],
                [
                    marker.x as f32,
                    marker.y as f32,
                    marker.w as f32,
                    marker.h as f32,
                ],
            ],
            colors: [[0.95, 0.55, 0.10, 1.0], [0.20, 0.75, 0.35, 1.0]],
        };
        gpu.queue
            .write_buffer(&gpu.uniforms, 0, bytemuck::bytes_of(&uniforms));

        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("probe"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("probe"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(BACKGROUND),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &gpu.bind_group, &[]);
            pass.draw(0..6, 0..2);
        }
        gpu.queue.submit([encoder.finish()]);

        // The hint. The whole frame above is always valid; only the rects differ.
        self.frame += 1;
        let full = self.force_full
            || (self.cfg.full_every > 0 && self.frame.is_multiple_of(self.cfg.full_every as u64));
        self.force_full = false;
        let mut rects: Vec<wgpu::Rect<u32>> = Vec::with_capacity(2);
        match self.cfg.mode {
            DamageMode::None => {}
            DamageMode::Exact => {
                rects.extend(self.prev_square.clipped(width, height));
                rects.extend(self.square.clipped(width, height));
            }
            DamageMode::Under => {
                rects.extend(self.square.clipped(width, height));
            }
        }
        if full && self.cfg.mode != DamageMode::None {
            rects.clear();
            rects.push(wgpu::Rect {
                x: 0,
                y: 0,
                w: width,
                h: height,
            });
        }
        if self.cfg.mode == DamageMode::None {
            gpu.queue.present(texture);
        } else {
            gpu.queue.present_with_damage(texture, &rects);
        }

        self.frames_since_log += 1;
        if self.last_log.elapsed() >= Duration::from_secs(5) {
            let secs = self.last_log.elapsed().as_secs_f64();
            log::info!(
                "{:.1} fps | mode {} | frame {} | last rects {:?}",
                self.frames_since_log as f64 / secs,
                self.cfg.mode.as_str(),
                self.frame,
                rects
            );
            self.last_log = Instant::now();
            self.frames_since_log = 0;
        }
    }

    fn title(&self) -> String {
        match &self.gpu {
            Some(gpu) => format!(
                "damage-probe — {} — {:?} — {}",
                self.cfg.mode.as_str(),
                gpu.backend,
                gpu.adapter_name
            ),
            None => format!("damage-probe — {}", self.cfg.mode.as_str()),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let attrs = Window::default_attributes()
                .with_title(self.title())
                .with_inner_size(winit::dpi::LogicalSize::new(800.0, 600.0));
            match event_loop.create_window(attrs) {
                Ok(w) => self.window = Some(Arc::new(w)),
                Err(e) => {
                    log::error!("create_window: {e}");
                    event_loop.exit();
                    return;
                }
            }
        }
        // (Re)create the GPU side on every resume: on Android the native window
        // is new each time the activity comes back.
        if self.gpu.is_none() {
            let window = self.window.clone().expect("window exists");
            match Gpu::new(window.clone()) {
                Ok(gpu) => {
                    self.gpu = Some(gpu);
                    self.force_full = true;
                    window.set_title(&self.title());
                    log::info!(
                        "probe running: mode={} full_every={} step={} size={} fps={:?}",
                        self.cfg.mode.as_str(),
                        self.cfg.full_every,
                        self.cfg.step,
                        self.cfg.size,
                        self.cfg.fps
                    );
                    window.request_redraw();
                }
                Err(e) => {
                    log::error!("gpu init failed: {e}");
                    event_loop.exit();
                }
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // Android: the surface is gone until the next `resumed`.
        self.gpu = None;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(size.width, size.height);
                    self.force_full = true;
                }
            }
            WindowEvent::RedrawRequested => {
                self.render();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

/// Run the probe on an already-built event loop.
pub fn run(event_loop: EventLoop<()>, cfg: Config) {
    let mut app = App::new(cfg);
    if let Err(e) = event_loop.run_app(&mut app) {
        log::error!("event loop: {e}");
    }
}

#[cfg(target_os = "android")]
mod android {
    use super::{Config, DamageMode};

    fn props() -> android_system_properties::AndroidSystemProperties {
        android_system_properties::AndroidSystemProperties::new()
    }

    /// `adb shell setprop debug.damage_probe.<key> <value>` before launch:
    /// `mode` (none|exact|under), `full_every`, `step`, `size`, `fps`.
    pub fn config_from_properties() -> Config {
        let p = props();
        // `debug.damage_probe.backend vulkan|gl` reaches wgpu through its own
        // `WGPU_BACKEND` knob — set before the instance exists, single-threaded.
        if let Some(b) = p.get("debug.damage_probe.backend") {
            let b = b.trim().to_string();
            if !b.is_empty() {
                // SAFETY: called from `android_main` before any other thread
                // of this process reads the environment.
                unsafe { std::env::set_var("WGPU_BACKEND", &b) };
            }
        }
        let mut cfg = Config::default();
        if let Some(m) = p
            .get("debug.damage_probe.mode")
            .and_then(|s| DamageMode::parse(&s))
        {
            cfg.mode = m;
        }
        if let Some(v) = p
            .get("debug.damage_probe.full_every")
            .and_then(|s| s.trim().parse().ok())
        {
            cfg.full_every = v;
        }
        if let Some(v) = p
            .get("debug.damage_probe.step")
            .and_then(|s| s.trim().parse().ok())
        {
            cfg.step = v;
        }
        if let Some(v) = p
            .get("debug.damage_probe.size")
            .and_then(|s| s.trim().parse().ok())
        {
            cfg.size = v;
        }
        if let Some(v) = p
            .get("debug.damage_probe.fps")
            .and_then(|s| s.trim().parse().ok())
        {
            cfg.fps = Some(v);
        }
        cfg
    }

    #[unsafe(no_mangle)]
    pub fn android_main(app: winit::platform::android::activity::AndroidApp) {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("damage-probe"),
        );
        use winit::platform::android::EventLoopBuilderExtAndroid;
        let cfg = config_from_properties();
        log::info!("android config: {cfg:?}");
        let event_loop = match winit::event_loop::EventLoop::builder()
            .with_android_app(app)
            .build()
        {
            Ok(el) => el,
            Err(e) => {
                log::error!("event loop build: {e}");
                return;
            }
        };
        super::run(event_loop, cfg);
    }
}
