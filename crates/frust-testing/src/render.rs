//! The renderer-agnostic contract every golden/oracle backend implements.
//!
//! `SceneRenderer` is the seam a golden test drives: it hands a
//! [`frust_scene::Scene`] plus a fixed [`RenderSpec`] to whichever backend is
//! under test (the `vello_cpu` reference oracle, the GPU engine arm, …) and
//! gets back a [`RenderedImage`] it can compare against a stored baseline.
//! Scene-layer purity binds here too — no `vello`/`wgpu` type appears in this
//! module (`docs/CODE_STANDARDS.md`'s "Leaking `vello`/`wgpu` types outside
//! `frust-render`" anti-pattern).

use kurbo::Affine;
use peniko::Color;

/// A backend under test hands back a rendered frame for comparison.
///
/// The trait itself names no concrete backend and pulls in no
/// `vello`/`wgpu`/GPU type: an implementation supplies those (this crate's
/// own [`CpuOracle`](crate::oracle_cpu::CpuOracle) and
/// [`EngineOracle`](crate::oracle_engine::EngineOracle) do, and a consumer
/// outside this crate can supply its own).
pub trait SceneRenderer {
    /// A stable, backend-identifying id (e.g. `"cpu"`,
    /// `"engine-metal-macos"`) — matched against [`crate::case::BackendSet`]
    /// to decide whether a case should be skipped for this backend, and
    /// recorded into [`RenderedImage::meta`]/`crate::meta::GoldenMeta` for
    /// golden-class routing (`docs/TESTING.md`'s Golden Classes).
    fn id(&self) -> &'static str;

    /// Static metadata about this backend, captured once (not per render) —
    /// the adapter/driver/device-kind identity a promoted baseline must
    /// record (`docs/TESTING.md`'s GPU Run Metadata).
    fn meta(&self) -> BackendMeta;

    /// Renders `scene` under `spec`, returning the resulting frame.
    ///
    /// Deterministic-input discipline (`docs/TESTING.md`'s Deterministic
    /// Inputs) is the caller's responsibility — this call takes a
    /// fully-specified `scene`/`spec` pair and must not consult wall-clock
    /// time, environment state, or any other unrecorded input.
    fn render(
        &mut self,
        scene: &frust_scene::Scene,
        spec: &RenderSpec,
    ) -> anyhow::Result<RenderedImage>;
}

/// The fixed, deterministic inputs a golden case renders under.
///
/// Every field here is part of the deterministic-input contract
/// (`docs/TESTING.md`'s Deterministic Inputs) — a golden case must pin all
/// of them rather than letting a backend default any one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderSpec {
    /// Output width in physical pixels.
    pub width: u32,
    /// Output height in physical pixels.
    pub height: u32,
    /// The color the surface is cleared to before the scene is drawn.
    pub base_color: Color,
    /// Uniform device-pixel scale factor applied ahead of `root`.
    pub scale: f64,
    /// The root transform composed under the scene's own transform stack —
    /// how a golden case pins viewport/orientation without re-recording the
    /// scene itself.
    pub root: Affine,
}

/// Whether [`RenderedImage::rgba8`] carries straight or premultiplied alpha.
///
/// A comparator must know which convention it is diffing — the two are not
/// interchangeable for any color channel with non-opaque alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaKind {
    /// Color channels are not scaled by alpha.
    Straight,
    /// Color channels are pre-scaled by alpha (vello's own internal
    /// convention).
    Premultiplied,
}

/// A single rendered frame, ready for comparison or PNG encode.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedImage {
    /// Width in physical pixels; matches `rgba8`'s row stride
    /// (`width * 4`, no row padding).
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Unpadded 8-bit RGBA pixel data, `width * height * 4` bytes, row-major
    /// top-to-bottom. Straight or premultiplied per `alpha`.
    pub rgba8: Vec<u8>,
    /// Which alpha convention `rgba8` uses.
    pub alpha: AlphaKind,
    /// The backend that produced this frame.
    pub meta: BackendMeta,
}

/// Backend identity recorded alongside a rendered frame — no `wgpu` type
/// appears here so this crate stays a leaf.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendMeta {
    /// The backend's own stable id (`SceneRenderer::id`).
    pub backend: String,
    /// The GPU/software adapter name (e.g. an adapter's reported name, or a
    /// software-oracle's own identifier).
    pub adapter: String,
    /// Driver/API version string (e.g. a Vulkan driver version, or a crate
    /// version for a software oracle).
    pub driver: String,
    /// Coarse device classification (e.g. `"discrete-gpu"`, `"cpu"`,
    /// `"software"`).
    pub device_kind: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_spec_is_copy_and_comparable() {
        let a = RenderSpec {
            width: 100,
            height: 100,
            base_color: Color::TRANSPARENT,
            scale: 1.0,
            root: Affine::IDENTITY,
        };
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn alpha_kind_variants_are_distinct() {
        assert_ne!(AlphaKind::Straight, AlphaKind::Premultiplied);
    }

    #[test]
    fn backend_meta_default_is_empty() {
        let meta = BackendMeta::default();
        assert!(meta.backend.is_empty());
        assert!(meta.adapter.is_empty());
        assert!(meta.driver.is_empty());
        assert!(meta.device_kind.is_empty());
    }

    #[test]
    fn rendered_image_carries_its_own_meta() {
        let meta = BackendMeta {
            backend: "cpu".into(),
            adapter: "vello_cpu".into(),
            driver: "0.2.0".into(),
            device_kind: "software".into(),
        };
        let image = RenderedImage {
            width: 2,
            height: 1,
            rgba8: vec![0, 0, 0, 255, 255, 255, 255, 255],
            alpha: AlphaKind::Straight,
            meta: meta.clone(),
        };
        assert_eq!(image.rgba8.len() as u32, image.width * image.height * 4);
        assert_eq!(image.meta, meta);
    }
}
