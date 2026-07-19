//! GPU smoke test: render a red `FillRect` scene headlessly and read the pixels
//! back. This exercises the real vello 0.9 / wgpu 29 pipeline end to end (the
//! version-pin de-risking called out in task 06), so it is `#[ignore]`d and run
//! manually on hardware with a GPU:
//!
//! ```text
//! cargo test -p frust-render -- --ignored
//! ```
//!
//! CI sandboxes without a GPU are expected to skip it; a local pass on Metal is
//! the gate. It renders through [`frust_render::encode_scene`] so the actual
//! scene-to-vello mapping is what gets validated, not a hand-written vello scene.

use frust_render::encode_scene;
use frust_scene::{Scene, SceneBuilder};
use peniko::Brush;
use peniko::color::palette::css::{BLACK, RED};

const SIZE: u32 = 64; // 64 * 4 bytes = 256, the wgpu row-copy alignment — no padding math needed.

#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn red_fill_rect_produces_non_zero_pixels() {
    pollster::block_on(run());
}

async fn run() {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("no compatible GPU adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("frust gpu_smoke"),
            required_features: adapter.features() & wgpu::Features::CLEAR_TEXTURE,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        })
        .await
        .expect("failed to create device");

    // Vello renders by compute into a STORAGE texture; add COPY_SRC for readback.
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust gpu_smoke target"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
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

    // Build a Frust scene with a full-surface red rect and encode it to vello.
    let mut fk_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut fk_scene);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Solid(RED),
        );
    }
    let mut vello_scene = vello::Scene::new();
    encode_scene(&fk_scene, &mut vello_scene);

    let mut renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
        .expect("failed to create vello renderer");
    renderer
        .render_to_texture(
            &device,
            &queue,
            &vello_scene,
            &view,
            &vello::RenderParams {
                base_color: BLACK,
                width: SIZE,
                height: SIZE,
                antialiasing_method: vello::AaConfig::Area,
            },
        )
        .expect("render_to_texture failed");

    // Copy the target texture into a mappable buffer and read it back.
    let bytes_per_row = SIZE * 4;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frust gpu_smoke readback"),
        size: (bytes_per_row * SIZE) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
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
    slice.map_async(wgpu::MapMode::Read, move |res| {
        let _ = tx.send(res);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll failed");
    rx.recv()
        .expect("map channel closed")
        .expect("buffer map failed");

    let data = slice.get_mapped_range();
    let non_zero = data.iter().any(|&b| b != 0);
    assert!(non_zero, "expected non-zero pixels from a red FillRect");

    // Spot-check the centre pixel is dominated by red.
    let centre = ((SIZE / 2) * bytes_per_row + (SIZE / 2) * 4) as usize;
    let (r, g, b) = (data[centre], data[centre + 1], data[centre + 2]);
    assert!(
        r > g && r > b,
        "centre pixel should be red-dominant, got rgb=({r},{g},{b})"
    );
}
