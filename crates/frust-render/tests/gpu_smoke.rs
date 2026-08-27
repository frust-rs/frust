//! GPU smoke test: render a red `FillRect` scene headlessly and read the pixels
//! back. This exercises the real vello 0.9 / wgpu 29 pipeline end to end (the
//! version-pin de-risking this workspace's rendering stack needs), so it is
//! `#[ignore]`d and run manually on hardware with a GPU:
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
use peniko::color::palette::css::{BLACK, GREEN, RED, TRANSPARENT};

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
/// override→atlas copy, with no uncaptured validation error.
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
    // COPY_SRC (vello's override copy reads it).
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

/// Pixel-level regression test for the Mode B hole punch: a `clear_rect`
/// recorded INSIDE a
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

/// The snapshot-layer round trip, pixel for pixel: a bracket rasterized into
/// its own texture, registered as a vello image override and composited back
/// as one image quad must land the same pixels as the same bracket painted
/// inline — which is what the encode walk does when there is no cached image.
///
/// This is the test that pins the alpha decision. vello renders STRAIGHT
/// (un-premultiplied) alpha into a target texture, and `register_texture`
/// declares the override `ImageAlphaType::Alpha`, so vello premultiplies each
/// sampled texel itself at composite time: the snapshot texture is registered
/// exactly as rendered, with no premultiply pass in between. Running one would
/// premultiply twice — the body's half-alpha green band would composite at
/// roughly half its correct intensity, tens of levels off, on interior pixels
/// this test compares strictly. It equally pins the image mapping: the cached
/// texture's pixel grid must cover the bracket's local `rect` exactly, or the
/// two arms disagree everywhere rather than at edges.
///
/// The comparison is strict on every pixel. The inline arm rasterizes vector
/// edges at the composited scale while the cached arm resamples a texture
/// rasterized at the bracket's own device scale, which in general leaves a
/// one-pixel seam at content edges; the geometry below removes that variable
/// deliberately, keeping every content edge on a whole device pixel in both
/// arms (measured: the two agree exactly, delta 0, on Metal). A tolerance is
/// kept only for 8-bit rounding — widening it to accommodate a seam would let
/// a mis-scaled or misplaced image pass, since a placement error shows up at
/// edges first.
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
fn snapshot_layer_round_trips_through_atlas() {
    pollster::block_on(run_snapshot());
}

/// Per-pixel tolerance for everything but the antialiasing seam: the two arms
/// must agree to within a rounding step.
const SNAPSHOT_TOLERANCE: u8 = 3;

async fn run_snapshot() {
    // The bracket: a 20x20 body in local space, recorded under a 2x device
    // scale offset by (4, 6), composited at 75% opacity and 90% scale. Every
    // body coordinate is a multiple of 5, which — with this transform — puts
    // every inline edge on a whole device pixel (device = 6 + 1.8 * local in x,
    // 8 + 1.8 * local in y), so the comparison is not measuring rasterizer
    // subpixel coverage.
    const KEY: u64 = 1;
    const ALPHA: f32 = 0.75;
    const SCALE: f64 = 0.9;
    let bracket = kurbo::Affine::translate((4.0, 6.0)) * kurbo::Affine::scale(2.0);
    let rect = kurbo::Rect::new(0.0, 0.0, 20.0, 20.0);

    // The body: an opaque red block, a half-alpha green band over NOTHING (so
    // the rasterized texture really carries partial alpha — the double
    // premultiply this test rules out is invisible where alpha is 1), and a
    // black bar with transparent margins standing in for a line of text.
    let body = |builder: &mut SceneBuilder| {
        builder.fill_rect(kurbo::Rect::new(0.0, 0.0, 20.0, 10.0), Brush::Solid(RED));
        builder.fill_rect(
            kurbo::Rect::new(0.0, 10.0, 20.0, 15.0),
            Brush::Solid(GREEN.with_alpha(0.5)),
        );
        builder.fill_rect(kurbo::Rect::new(5.0, 15.0, 15.0, 20.0), Brush::Solid(BLACK));
    };

    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("no compatible GPU adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("frust gpu_smoke snapshot"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        })
        .await
        .expect("failed to create device");
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
        .expect("failed to create vello renderer");

    // --- the cached arm's pre-pass -----------------------------------------
    // Texture size and rasterization root, derived exactly as the cache does:
    // `raster` is the bracket transform's linear part (translation zeroed), the
    // size is `rect` under `raster` rounded outward, and the root maps `rect`'s
    // local origin to texture (0, 0) with `rect` spread over the whole grid.
    let raster = {
        let c = bracket.as_coeffs();
        kurbo::Affine::new([c[0], c[1], c[2], c[3], 0.0, 0.0])
    };
    let bbox = raster.transform_rect_bbox(rect);
    let (tex_w, tex_h) = (bbox.width().ceil() as u32, bbox.height().ceil() as u32);
    assert_eq!(
        (tex_w, tex_h),
        (40, 40),
        "2x device scale over a 20x20 body"
    );
    let root = kurbo::Affine::scale_non_uniform(
        f64::from(tex_w) / rect.width(),
        f64::from(tex_h) / rect.height(),
    ) * kurbo::Affine::translate((-rect.x0, -rect.y0))
        * bracket.inverse();

    // Rasterize the body under `root`, over transparency. The body's commands
    // carry the bracket's own transform, so the scene is recorded under
    // `root * bracket` — what the cache's command-slice encode produces.
    let mut body_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut body_scene);
        builder.push_transform(root * bracket);
        body(&mut builder);
        builder.pop_transform();
    }
    let mut vello_scene = vello::Scene::new();
    encode_scene(&body_scene, &mut vello_scene);
    let snapshot_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust gpu_smoke snapshot layer"),
        size: wgpu::Extent3d {
            width: tex_w,
            height: tex_h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let snapshot_view = snapshot_tex.create_view(&wgpu::TextureViewDescriptor::default());
    renderer
        .render_to_texture(
            &device,
            &queue,
            &vello_scene,
            &snapshot_view,
            &vello::RenderParams {
                base_color: TRANSPARENT,
                width: tex_w,
                height: tex_h,
                antialiasing_method: vello::AaConfig::Area,
            },
        )
        .expect("snapshot rasterization failed");
    // Registered as-is: no premultiply pass between vello's straight-alpha
    // output and the override vello premultiplies per texel at composite.
    let image = renderer.register_texture(snapshot_tex);
    renderer.mark_override_image_dirty(&image);

    // --- the two arms ------------------------------------------------------
    // Cached: one image quad under the bracket's alpha layer and presentation
    // scale — the shape the encode walk's hit path emits.
    let mut cached_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut cached_scene);
        builder.push_transform(bracket);
        builder.push_layer(rect, ALPHA);
        builder.push_transform(kurbo::Affine::scale_about(SCALE, rect.center()));
        builder.draw_image(&image, rect);
        builder.pop_transform();
        builder.pop_layer();
        builder.pop_transform();
    }
    // Inline: the bracket itself, which without a snapshot-image map lowers to
    // the encode walk's inline emulation.
    let mut inline_scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut inline_scene);
        builder.push_transform(bracket);
        builder.push_snapshot(KEY, rect, ALPHA, SCALE);
        body(&mut builder);
        builder.pop_snapshot();
        builder.pop_transform();
    }

    let cached = render_and_read(&device, &queue, &mut renderer, &cached_scene).await;
    let inline = render_and_read(&device, &queue, &mut renderer, &inline_scene).await;

    // --- comparison --------------------------------------------------------
    // Every pixel, no exclusions: the geometry above puts every content edge
    // on a whole device pixel in BOTH arms, so there is no antialiasing seam
    // to forgive. A disagreement anywhere means the cached texture is
    // misplaced, mis-scaled, or composited with the wrong alpha type.
    let channel = |data: &[u8], x: u32, y: u32, c: usize| data[((y * SIZE + x) * 4) as usize + c];
    let pixel =
        |data: &[u8], x: u32, y: u32| (0..4).map(|c| channel(data, x, y, c)).collect::<Vec<_>>();

    let (mut worst, mut worst_at) = (0u8, (0, 0));
    for y in 0..SIZE {
        for x in 0..SIZE {
            let delta = (0..4)
                .map(|c| channel(&inline, x, y, c).abs_diff(channel(&cached, x, y, c)))
                .max()
                .unwrap_or(0);
            if delta > worst {
                worst = delta;
                worst_at = (x, y);
            }
        }
    }
    println!("snapshot round trip: max per-pixel delta {worst} at {worst_at:?}");
    assert!(
        worst <= SNAPSHOT_TOLERANCE,
        "cached and inline arms disagree by {worst} at {worst_at:?} (tolerance \
         {SNAPSHOT_TOLERANCE}): the alpha handling or the image mapping is wrong. \
         inline={:?} cached={:?}",
        pixel(&inline, worst_at.0, worst_at.1),
        pixel(&cached, worst_at.0, worst_at.1),
    );

    // Absolute checks, so a change that broke BOTH arms identically still
    // fails. Device coordinates follow the mapping noted above; the expected
    // values are the composite arithmetic, not observations:
    //  - the opaque red block composites at the bracket's 0.75 -> 191,
    //  - the half-alpha green band (CSS green is 0x008000) composites at
    //    128 * 0.5 * 0.75 -> 48, which is the number a texture premultiplied
    //    a second time before compositing would halve again.
    for (label, data) in [("inline", &inline), ("cached", &cached)] {
        let red = pixel(data, 24, 12);
        let green = pixel(data, 24, 30);
        assert!(
            red[0].abs_diff(191) <= SNAPSHOT_TOLERANCE,
            "{label} arm must composite the opaque body block at the bracket's alpha, got {red:?}"
        );
        assert!(
            green[1].abs_diff(48) <= SNAPSHOT_TOLERANCE,
            "{label} arm must composite the body's half-alpha band at 128 * 0.5 * 0.75, \
             got {green:?} (a doubly premultiplied snapshot texture halves this)"
        );
    }

    let scope_err = error_scope.pop().await;
    assert!(
        scope_err.is_none(),
        "snapshot pre-pass produced an uncaptured validation error: {scope_err:?}"
    );
}

/// Render `scene` into a fresh `SIZE`-square vello target over an opaque black
/// base and read the pixels back.
async fn render_and_read(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut vello::Renderer,
    scene: &Scene,
) -> Vec<u8> {
    let mut vello_scene = vello::Scene::new();
    encode_scene(scene, &mut vello_scene);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust gpu_smoke snapshot target"),
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
    renderer
        .render_to_texture(
            device,
            queue,
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

    let bytes_per_row = SIZE * 4;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frust gpu_smoke snapshot readback"),
        size: u64::from(bytes_per_row * SIZE),
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
    slice.get_mapped_range().to_vec()
}
