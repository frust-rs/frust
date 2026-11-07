//! Registration hints on an external pass's texture, proven against the CPU
//! oracle: the engine's sampling of a bound texture is the same arithmetic an
//! image takes, so a CPU render of equivalent pixels is the reference.
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-testing --test scene_texture_hints -- --ignored --test-threads=1
//! ```
//!
//! Each reference is built CPU-side from the very pixels the pass uploads:
//!
//! - **unhinted, 1:1** - the pixels as a `Command::Image` at the same
//!   whole-pixel destination;
//! - **nearest, 4x** - a 4x nearest-replicated copy of the pixels as a
//!   `Command::Image` at 1:1 (an image has no sampling quality of its own, so
//!   replication is what stands in for nearest magnification);
//! - **opaque** - the same pixels hinted opaque and unhinted; opacity changes
//!   routing, not colour.
//!
//! Every comparison is exact (`Tolerance::exact()`): all sources are opaque
//! and every placement lands texel centres on pixel centres, so no filtering
//! or coverage arithmetic differs between the two arms.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use frust_gpu::SceneTextureId;
use frust_render::{ExternalFrame, ExternalPass, SceneTextureFilter, SceneTextureHints};
use frust_scene::{Scene, SceneBuilder};
use frust_testing::case::Tolerance;
use frust_testing::diff::diff_images;
use frust_testing::oracle_cpu::CpuOracle;
use frust_testing::oracle_engine::{EngineOracle, EngineOracleOptions};
use frust_testing::render::{RenderSpec, RenderedImage, SceneRenderer};
use image::RgbaImage;
use kurbo::{Affine, Rect};
use peniko::color::palette::css::BLACK;
use peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

const SIZE: u32 = 64;

/// A pass that uploads fixed pixels into a texture of its own and binds it
/// with `hints`.
struct Upload {
    width: u32,
    height: u32,
    rgba8: Vec<u8>,
    hints: SceneTextureHints,
}

impl ExternalPass for Upload {
    fn record(&self, frame: &mut ExternalFrame<'_>) {
        let texture = frame.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("scene texture hints source"),
            size: wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        frame.queue().write_texture(
            texture.as_image_copy(),
            &self.rgba8,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width * 4),
                rows_per_image: Some(self.height),
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        frame.bind_texture_with((self.width, self.height), view, self.hints);
    }
}

/// An opaque, per-texel-distinct pattern (a neighbour never shares a colour).
fn pattern(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            out.extend_from_slice(&[
                (x * 255 / (width - 1).max(1)) as u8,
                (y * 255 / (height - 1).max(1)) as u8,
                ((x * 7 + y * 13) % 256) as u8,
                255,
            ]);
        }
    }
    out
}

/// `pixels` replicated `factor` times along each axis (nearest magnification).
fn replicate(pixels: &[u8], width: u32, height: u32, factor: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..height * factor {
        for x in 0..width * factor {
            let at = (((y / factor) * width + x / factor) * 4) as usize;
            out.extend_from_slice(&pixels[at..at + 4]);
        }
    }
    out
}

fn image_data(rgba8: Vec<u8>, width: u32, height: u32) -> ImageData {
    ImageData {
        data: Blob::from(rgba8),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::AlphaPremultiplied,
        width,
        height,
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

fn to_rgba_image(image: &RenderedImage) -> RgbaImage {
    RgbaImage::from_raw(image.width, image.height, image.rgba8.clone())
        .expect("a readback holds width*height*4 bytes")
}

fn assert_same(label: &str, expected: &RenderedImage, actual: &RenderedImage) {
    let outcome = diff_images(
        &to_rgba_image(expected),
        &to_rgba_image(actual),
        Tolerance::exact(),
        false,
    );
    assert!(
        outcome.passed,
        "{label}: engine differs from reference: {} pixels over tolerance",
        outcome.report.pixel_count
    );
}

/// Renders `pass` bound to a fresh id and drawn into `dest` through the engine.
fn engine_render(oracle: &mut EngineOracle, pass: Upload, dest: Rect) -> RenderedImage {
    let id = SceneTextureId::mint();
    let mut scene = Scene::new();
    SceneBuilder::new(&mut scene).scene_texture(id.get(), dest);
    let passes: Vec<(SceneTextureId, Arc<dyn ExternalPass>)> = vec![(id, Arc::new(pass))];
    oracle
        .render_with_passes(&scene, &spec(), &passes)
        .expect("engine render with passes")
}

/// Renders `data` as an image into `dest` through the CPU oracle.
fn cpu_render(data: &ImageData, dest: Rect) -> RenderedImage {
    let mut scene = Scene::new();
    SceneBuilder::new(&mut scene).draw_image(data, dest);
    CpuOracle::new()
        .render(&scene, &spec())
        .expect("cpu render")
}

fn engine() -> EngineOracle {
    EngineOracle::new(&EngineOracleOptions::default()).expect("a GPU adapter")
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-testing --test scene_texture_hints -- --ignored`"]
fn an_unhinted_texture_at_a_whole_pixel_offset_matches_an_image() {
    let pixels = pattern(16, 16);
    let dest = Rect::new(8.0, 8.0, 24.0, 24.0);
    let actual = engine_render(
        &mut engine(),
        Upload {
            width: 16,
            height: 16,
            rgba8: pixels.clone(),
            hints: SceneTextureHints::default(),
        },
        dest,
    );
    let expected = cpu_render(&image_data(pixels, 16, 16), dest);
    assert_same("unhinted 1:1", &expected, &actual);
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-testing --test scene_texture_hints -- --ignored`"]
fn a_nearest_texture_at_four_times_matches_a_replicated_image() {
    let pixels = pattern(8, 8);
    let dest = Rect::new(16.0, 16.0, 48.0, 48.0);
    let actual = engine_render(
        &mut engine(),
        Upload {
            width: 8,
            height: 8,
            rgba8: pixels.clone(),
            hints: SceneTextureHints::new().with_filter(SceneTextureFilter::Nearest),
        },
        dest,
    );
    let expected = cpu_render(&image_data(replicate(&pixels, 8, 8, 4), 32, 32), dest);
    assert_same("nearest 4x", &expected, &actual);
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-testing --test scene_texture_hints -- --ignored`"]
fn an_opaque_hint_on_an_opaque_texture_changes_routing_not_colour() {
    let pixels = pattern(16, 16);
    let dest = Rect::new(8.0, 8.0, 24.0, 24.0);
    let mut oracle = engine();
    let unhinted = engine_render(
        &mut oracle,
        Upload {
            width: 16,
            height: 16,
            rgba8: pixels.clone(),
            hints: SceneTextureHints::default(),
        },
        dest,
    );
    let opaque = engine_render(
        &mut oracle,
        Upload {
            width: 16,
            height: 16,
            rgba8: pixels,
            hints: SceneTextureHints::new().assume_opaque(true),
        },
        dest,
    );
    assert_same("opaque vs unhinted", &unhinted, &opaque);
}
