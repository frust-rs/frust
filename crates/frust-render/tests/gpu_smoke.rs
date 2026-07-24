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

/// A self-contained fullscreen-triangle WGSL module returning a solid green —
/// the shape of the shader pre-pass's offscreen program (`shader_effects.rs`'s
/// prelude + an `fs_main`), minus the uniform block a solid color needs.
const SOLID_GREEN_WGSL: &str = r#"
struct VsOut { @builtin(position) pos: vec4<f32> };
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    out.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
"#;

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

/// End-to-end smoke of the shader-showcase GPU mechanism the `encode` pre-pass
/// (`renderer.rs`'s `run_shader_prepass`) performs: render a solid-color WGSL
/// fragment shader into an offscreen `Rgba8Unorm` texture, register it with
/// vello as an image override (`register_texture`), and draw it through Frust's
/// scene encode — the exact `draw_image` a `Command::ShaderQuad` lowers to.
/// Reads the presented pixels back and asserts the shader's color survived the
/// override→atlas copy (RESEARCH.md §Q1), with no uncaptured validation error.
///
/// The crate-private `ShaderEffects` wrapper is unit-tested in-crate; this
/// covers the real-device GPU path it drives, which no headless surface can.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn shader_override_solid_color_survives_the_atlas_copy() {
    pollster::block_on(run_shader());
}

async fn run_shader() {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("no compatible GPU adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("frust gpu_smoke shader"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        })
        .await
        .expect("failed to create device");
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    // The shader pre-pass target: Rgba8Unorm + RENDER_ATTACHMENT (our pass) +
    // COPY_SRC (vello's override copy reads it — RESEARCH.md §Q1).
    let shader_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust gpu_smoke shader target"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let shader_view = shader_tex.create_view(&wgpu::TextureViewDescriptor::default());

    // Compile + run the fullscreen solid-green fragment shader into the target.
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("frust gpu_smoke shader module"),
        source: wgpu::ShaderSource::Wgsl(SOLID_GREEN_WGSL.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("frust gpu_smoke shader layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("frust gpu_smoke shader pipeline"),
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
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frust gpu_smoke shader pass"),
    });
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frust gpu_smoke shader pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &shader_view,
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
    // Submit the shader pass BEFORE vello renders (the pre-pass's ordering).
    queue.submit([enc.finish()]);

    // Register the shader texture as a vello override, then draw it through the
    // Frust scene encode — the `draw_image` a `ShaderQuad` lowers to.
    let mut renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
        .expect("failed to create vello renderer");
    let image = renderer.register_texture(shader_tex);
    renderer.mark_override_image_dirty(&image);

    let mut fk_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut fk_scene);
        builder.draw_image(&image, kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64));
    }
    let mut vello_scene = vello::Scene::new();
    encode_scene(&fk_scene, &mut vello_scene);

    // Present into a vello-compatible storage target and read it back.
    let out_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust gpu_smoke shader present target"),
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
    let out_view = out_tex.create_view(&wgpu::TextureViewDescriptor::default());
    renderer
        .render_to_texture(
            &device,
            &queue,
            &vello_scene,
            &out_view,
            &vello::RenderParams {
                base_color: BLACK,
                width: SIZE,
                height: SIZE,
                antialiasing_method: vello::AaConfig::Area,
            },
        )
        .expect("render_to_texture failed");

    let bytes_per_row = SIZE * 4;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frust gpu_smoke shader readback"),
        size: (bytes_per_row * SIZE) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &out_tex,
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
    // Centre pixel came from the solid-green shader via the override copy.
    let centre = ((SIZE / 2) * bytes_per_row + (SIZE / 2) * 4) as usize;
    let (r, g, b) = (data[centre], data[centre + 1], data[centre + 2]);
    assert!(
        g > r && g > b,
        "centre pixel should be green-dominant (the shader color), got rgb=({r},{g},{b})"
    );
    drop(data);

    let scope_err = error_scope.pop().await;
    assert!(
        scope_err.is_none(),
        "shader pre-pass produced an uncaptured validation error: {scope_err:?}"
    );
}

/// Pixel-level regression test for the Mode B hole punch (platform-views
/// t11-fix 01 + the t11-redo D4 follow-up): a `clear_rect` recorded INSIDE a
/// clip/opacity layer group must still erase an opaque backdrop painted
/// OUTSIDE the group (the encode walk hoists the punch to root — a
/// group-local erase would be sealed in by the group composite), and the
/// erase must be PIXEL-EXACT at a 16-px-tile-UNALIGNED edge (vello 0.9's
/// `Compose::Clear` bleeds to whole boundary tiles, which is why the punch
/// uses `Compose::DestOut`; both defects were first caught on-device).
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn clear_rect_punches_pixel_exact_through_a_layer_group() {
    pollster::block_on(run_clear_probe());
}

async fn run_clear_probe() {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("no compatible GPU adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("frust clear-rect probe"),
            required_features: adapter.features() & wgpu::Features::CLEAR_TEXTURE,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        })
        .await
        .expect("failed to create device");

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust clear-rect probe target"),
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

    // Opaque backdrop at ROOT; the punch inside a full-surface layer group,
    // with its right edge one px past a 16-px tile boundary (SIZE/2 + 1).
    let punch_edge = (SIZE / 2 + 1) as f64;
    let mut fk_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut fk_scene);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
            Brush::Solid(RED),
        );
        builder.push_layer(kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64), 1.0);
        builder.clear_rect(kurbo::Rect::new(0.0, 0.0, punch_edge, SIZE as f64));
        builder.pop_layer();
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
                base_color: peniko::color::palette::css::TRANSPARENT,
                width: SIZE,
                height: SIZE,
                antialiasing_method: vello::AaConfig::Area,
            },
        )
        .expect("render_to_texture failed");

    let bytes_per_row = SIZE * 4;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frust clear-rect probe readback"),
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
    let mid_row = ((SIZE / 2) * bytes_per_row) as usize;
    let px = |x: u32| {
        let i = mid_row + (x * 4) as usize;
        (data[i], data[i + 1], data[i + 2], data[i + 3])
    };
    // Inside the punch: fully erased, through the group, over the backdrop.
    assert_eq!(px(SIZE / 4), (0, 0, 0, 0), "punch centre must be erased");
    assert_eq!(
        px(SIZE / 2),
        (0, 0, 0, 0),
        "last px inside the unaligned edge must be erased"
    );
    // Just past the edge, INSIDE the same 16-px tile: the backdrop must
    // survive untouched (the Compose::Clear tile bleed this test pins down).
    for x in [SIZE / 2 + 1, SIZE / 2 + 4, SIZE / 2 + 14] {
        let (r, _, _, a) = px(x);
        assert!(
            r > 200 && a > 200,
            "backdrop at x={x} (same tile as the punch edge) must stay opaque red, got {:?}",
            px(x)
        );
    }
}
