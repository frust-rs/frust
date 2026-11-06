//! External passes through [`frust_render::HeadlessRenderer`]: an explicit
//! pass list, and the opt-in drain of the process-wide registry.
//!
//! The cases need a real queue (a pass records GPU work and the engine
//! composites its target), so they are `#[ignore]`d:
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-render --test headless_external_pass -- --ignored --test-threads=1
//! ```

#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use frust_gpu::SceneTextureId;
use frust_render::{
    ExternalFrame, ExternalPass, HeadlessOptions, HeadlessRenderer, HeadlessSpec,
    register_external_pass, unregister_external_pass,
};
use frust_scene::{Scene, SceneBuilder};
use kurbo::Rect;
use peniko::color::palette::css::BLACK;

const SIZE: u32 = 64;
const TEXTURE: u32 = 32;
const DEST: Rect = Rect::new(16.0, 16.0, 48.0, 48.0);
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The registry is process-wide and each case builds its own device.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A pass that clears a target of its own to a known colour and binds it.
struct Fill(wgpu::Color);

impl ExternalPass for Fill {
    fn record(&self, frame: &mut ExternalFrame<'_>) {
        let texture = frame.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("headless external pass target"),
            size: wgpu::Extent3d {
                width: TEXTURE,
                height: TEXTURE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        frame
            .encoder()
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("headless external pass clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.0),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        frame.bind_texture((TEXTURE, TEXTURE), view);
    }
}

struct Panics;

impl ExternalPass for Panics {
    fn record(&self, _frame: &mut ExternalFrame<'_>) {
        panic!("this pass panics on purpose");
    }
}

const GREEN: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 1.0,
    b: 0.0,
    a: 1.0,
};
const BLUE: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 1.0,
    a: 1.0,
};

fn scene_drawing(id: SceneTextureId) -> Scene {
    let mut scene = Scene::new();
    SceneBuilder::new(&mut scene).scene_texture(id.get(), DEST);
    scene
}

fn base() -> [u8; 4] {
    BLACK.to_rgba8().to_u8_array()
}

fn assert_composites(image: &frust_render::HeadlessImage, colour: [u8; 4]) {
    assert_eq!(image.pixel(32, 32), colour, "the centre holds the pass");
    for (x, y) in [(1, 1), (SIZE - 2, 1), (1, SIZE - 2), (SIZE - 2, SIZE - 2)] {
        assert_eq!(image.pixel(x, y), base(), "corner ({x},{y}) is the base");
    }
}

fn assert_base_only(image: &frust_render::HeadlessImage) {
    assert_eq!(image.pixel(32, 32), base(), "no pass ran");
}

async fn renderer() -> HeadlessRenderer {
    HeadlessRenderer::new(HeadlessOptions::default())
        .await
        .expect("a GPU adapter")
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test headless_external_pass -- --ignored`"]
fn an_explicit_pass_composites_and_plain_render_draws_base_only() {
    let _serial = serial();
    pollster::block_on(async {
        let mut headless = renderer().await;
        let id = SceneTextureId::mint();
        let scene = scene_drawing(id);
        let spec = HeadlessSpec::new(SIZE, SIZE);
        let passes: Vec<(SceneTextureId, Arc<dyn ExternalPass>)> =
            vec![(id, Arc::new(Fill(GREEN)))];

        let with = headless
            .render_with_passes(&scene, &spec, &passes)
            .await
            .expect("render with passes");
        assert_composites(&with, [0, 255, 0, 255]);

        // A plain render drains nothing, and the earlier binding is cleared
        // rather than left to leak into it.
        let plain = headless.render(&scene, &spec).await.expect("plain render");
        assert_base_only(&plain);
    });
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test headless_external_pass -- --ignored`"]
fn an_explicit_render_ignores_the_registry_and_drops_ids_between_calls() {
    let _serial = serial();
    pollster::block_on(async {
        let mut headless = renderer().await;
        let id = SceneTextureId::mint();
        let other = SceneTextureId::mint();
        let scene = scene_drawing(id);
        let spec = HeadlessSpec::new(SIZE, SIZE);

        // Registered but not listed: an explicit render must not run it.
        assert!(register_external_pass(other, Arc::new(Fill(BLUE))));
        let first: Vec<(SceneTextureId, Arc<dyn ExternalPass>)> = vec![(id, Arc::new(Fill(GREEN)))];
        let image = headless
            .render_with_passes(&scene, &spec, &first)
            .await
            .expect("first render");
        assert_composites(&image, [0, 255, 0, 255]);

        // The next call lists a different set: `id` must not stay bound.
        let second: Vec<(SceneTextureId, Arc<dyn ExternalPass>)> =
            vec![(other, Arc::new(Fill(BLUE)))];
        let image = headless
            .render_with_passes(&scene, &spec, &second)
            .await
            .expect("second render");
        assert_base_only(&image);
        assert!(unregister_external_pass(other));
    });
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test headless_external_pass -- --ignored`"]
fn draining_the_registry_is_opt_in_and_unregistering_stops_it() {
    let _serial = serial();
    pollster::block_on(async {
        let mut headless = renderer().await;
        let id = SceneTextureId::mint();
        let scene = scene_drawing(id);
        let spec = HeadlessSpec::new(SIZE, SIZE);
        assert!(register_external_pass(id, Arc::new(Fill(GREEN))));

        assert_base_only(&headless.render(&scene, &spec).await.expect("plain"));
        let drained = headless
            .render_draining_registry(&scene, &spec)
            .await
            .expect("draining render");
        assert_composites(&drained, [0, 255, 0, 255]);

        assert!(unregister_external_pass(id));
        let after = headless
            .render_draining_registry(&scene, &spec)
            .await
            .expect("render after unregister");
        assert_base_only(&after);
    });
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test headless_external_pass -- --ignored`"]
fn a_panicking_explicit_pass_fails_the_render_and_a_later_render_recovers() {
    let _serial = serial();
    pollster::block_on(async {
        let mut headless = renderer().await;
        let id = SceneTextureId::mint();
        let scene = scene_drawing(id);
        let spec = HeadlessSpec::new(SIZE, SIZE);

        let bad: Vec<(SceneTextureId, Arc<dyn ExternalPass>)> = vec![(id, Arc::new(Panics))];
        let error = headless
            .render_with_passes(&scene, &spec, &bad)
            .await
            .expect_err("a panicking pass fails the render");
        assert!(error.to_string().contains("panicked"), "{error}");

        let good: Vec<(SceneTextureId, Arc<dyn ExternalPass>)> = vec![(id, Arc::new(Fill(GREEN)))];
        let image = headless
            .render_with_passes(&scene, &spec, &good)
            .await
            .expect("recovers");
        assert_composites(&image, [0, 255, 0, 255]);

        // Duplicate ids are refused up front.
        let twice: Vec<(SceneTextureId, Arc<dyn ExternalPass>)> =
            vec![(id, Arc::new(Fill(GREEN))), (id, Arc::new(Fill(BLUE)))];
        assert!(
            headless
                .render_with_passes(&scene, &spec, &twice)
                .await
                .is_err()
        );
    });
}
