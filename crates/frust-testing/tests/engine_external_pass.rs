//! External passes through [`frust_testing::EngineOracle`]: an explicit pass
//! list composites at its destination, a plain render draws the base only, and
//! the registry drain is opt-in.
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-testing --test engine_external_pass -- --ignored --test-threads=1
//! ```

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use frust_gpu::SceneTextureId;
use frust_render::{ExternalFrame, ExternalPass, register_external_pass, unregister_external_pass};
use frust_scene::{Scene, SceneBuilder};
use frust_testing::oracle_engine::{EngineOracle, EngineOracleOptions};
use frust_testing::render::{RenderSpec, SceneRenderer};
use kurbo::{Affine, Rect};
use peniko::color::palette::css::BLACK;

const SIZE: u32 = 64;
const TEXTURE: u32 = 32;
const DEST: Rect = Rect::new(16.0, 16.0, 48.0, 48.0);

/// A pass that clears a target of its own to opaque green and binds it.
struct Green;

impl ExternalPass for Green {
    fn record(&self, frame: &mut ExternalFrame<'_>) {
        let texture = frame.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("oracle external pass target"),
            size: wgpu::Extent3d {
                width: TEXTURE,
                height: TEXTURE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        frame
            .encoder()
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("oracle external pass clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
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

fn spec() -> RenderSpec {
    RenderSpec {
        width: SIZE,
        height: SIZE,
        base_color: BLACK,
        scale: 1.0,
        root: Affine::IDENTITY,
    }
}

fn pixel(image: &frust_testing::render::RenderedImage, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * image.width + x) * 4) as usize;
    image.rgba8[at..at + 4].try_into().expect("four bytes")
}

fn base() -> [u8; 4] {
    BLACK.to_rgba8().to_u8_array()
}

fn scene_drawing(id: SceneTextureId) -> Scene {
    let mut scene = Scene::new();
    SceneBuilder::new(&mut scene).scene_texture(id.get(), DEST);
    scene
}

fn assert_composites(image: &frust_testing::render::RenderedImage) {
    assert_eq!(pixel(image, 32, 32), [0, 255, 0, 255], "centre is the pass");
    for (x, y) in [(1, 1), (SIZE - 2, 1), (1, SIZE - 2), (SIZE - 2, SIZE - 2)] {
        assert_eq!(pixel(image, x, y), base(), "corner ({x},{y}) is the base");
    }
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-testing --test engine_external_pass -- --ignored`"]
fn an_explicit_pass_composites_and_plain_render_draws_base_only() {
    let mut oracle = EngineOracle::new(&EngineOracleOptions::default()).expect("a GPU adapter");
    let id = SceneTextureId::mint();
    let scene = scene_drawing(id);
    let passes: Vec<(SceneTextureId, Arc<dyn ExternalPass>)> = vec![(id, Arc::new(Green))];

    let with = oracle
        .render_with_passes(&scene, &spec(), &passes)
        .expect("render with passes");
    assert_composites(&with);

    let plain = oracle.render(&scene, &spec()).expect("plain render");
    assert_eq!(pixel(&plain, 32, 32), base(), "a plain render runs no pass");
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-testing --test engine_external_pass -- --ignored`"]
fn draining_the_registry_is_opt_in_and_unregistering_stops_it() {
    let mut oracle = EngineOracle::new(&EngineOracleOptions::default()).expect("a GPU adapter");
    let id = SceneTextureId::mint();
    let scene = scene_drawing(id);
    assert!(register_external_pass(id, Arc::new(Green)));

    let plain = oracle.render(&scene, &spec()).expect("plain render");
    assert_eq!(pixel(&plain, 32, 32), base());
    let drained = oracle
        .render_draining_registry(&scene, &spec())
        .expect("draining render");
    assert_composites(&drained);

    assert!(unregister_external_pass(id));
    let after = oracle
        .render_draining_registry(&scene, &spec())
        .expect("render after unregister");
    assert_eq!(pixel(&after, 32, 32), base());
}
