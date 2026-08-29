//! The GPU arm of the oracle pair: [`ClassicOracle`], a [`SceneRenderer`]
//! over `frust-render`'s offscreen vello-classic renderer.
//!
//! # Why it lives here and not in `frust-render`'s tests
//!
//! [`crate::oracle_cpu::CpuOracle`] and this type are a *pair*: the same
//! [`crate::corpus`] case is handed to both, and the whole point of the
//! comparison is that one trait object can stand in for either. A GPU adapter
//! that only existed inside `frust-render`'s own test binary could not be
//! driven from a corpus that lives in this crate, so the adapter lives beside
//! the corpus and this crate takes a `frust-render` dependency (see the
//! manifest's comment for why that is sound for a `publish = false`,
//! dev-dependency-only crate, and why it is pinned to `frust-render`'s
//! DEFAULT feature set).
//!
//! No `vello`/`wgpu` type appears in this module: `frust-render` confines
//! them behind [`HeadlessRenderer`]'s plain-bytes surface
//! (`docs/RENDER_ARCHITECTURE.md`'s confinement rule), and this file only
//! speaks that surface.
//!
//! # Which GPU, and which golden class
//!
//! Adapter selection is entirely `frust-render`'s: `WGPU_BACKEND` /
//! `WGPU_ADAPTER_NAME` choose, and
//! [`FRUST_GOLDEN_EXPECT_ADAPTER`](frust_render::GOLDEN_EXPECT_ADAPTER_ENV_VAR)
//! / [`FRUST_GOLDEN_EXPECT_BACKEND`](frust_render::GOLDEN_EXPECT_BACKEND_ENV_VAR)
//! turn a wrong choice into a refusal before any pixel is produced. This
//! module's own job is the step after that: mapping the *resolved* adapter
//! onto a golden CLASS name ([`golden_class`]), because
//! `docs/TESTING.md`'s Golden Classes are per-adapter directories and a
//! baseline captured on one GPU must never be compared against another's.
//!
//! An adapter this module has no class for resolves to
//! [`UNCLASSIFIED_CLASS`], which is deliberately not a directory anyone
//! promotes into: a corpus run on an unknown GPU records artifacts and
//! compares nothing, rather than inventing a class name and inviting a
//! baseline to be promoted from an unreviewed machine (`docs/TESTING.md`'s
//! Baseline Updates: "never promote output from a machine that failed the
//! adapter/font/environment preflight").
//!
//! # Alpha
//!
//! [`HeadlessImage::rgba8`](frust_render::HeadlessImage::rgba8) is STRAIGHT
//! alpha, as vello writes it (an erased pixel reads `[0, 0, 0, 0]`), so this
//! arm needs no conversion before golden storage — unlike the CPU arm, whose
//! premultiplied pixmap [`crate::corpus::straighten_alpha`] converts. The
//! tag is set from that documented fact, not assumed.

use anyhow::Result;
use frust_render::{HeadlessAa, HeadlessMeta, HeadlessOptions, HeadlessRenderer, HeadlessSpec};
use kurbo::Affine;

use crate::render::{AlphaKind, BackendMeta, RenderSpec, RenderedImage, SceneRenderer};

/// The golden class an adapter with no entry in [`KNOWN_CLASSES`] resolves
/// to.
///
/// Not a directory under `testing/goldens/`: a corpus harness treats this
/// class as "render, assert what is backend-independent, record artifacts,
/// promote nothing".
pub const UNCLASSIFIED_CLASS: &str = "classic-unclassified";

/// The reviewed GPU runners this repository keeps a golden class for, as
/// `(backend, adapter-name substring, class)`.
///
/// Both the backend (matched whole, lowercase — the form
/// [`HeadlessMeta::backend`] already reports) and the adapter substring
/// (matched case-insensitively, the same shape `WGPU_ADAPTER_NAME` matches
/// by) must hit, so `vulkan-nvidia-t400` cannot be claimed by the same card
/// running through a different backend.
///
/// Adding a runner is a deliberate act: it declares that the machine's
/// baselines are reviewed and owned (`docs/TESTING.md`'s Golden Classes —
/// "never pretend one device represents every GPU").
const KNOWN_CLASSES: &[(&str, &str, &str)] = &[("vulkan", "t400", "vulkan-nvidia-t400")];

/// The golden class for a resolved adapter, or [`UNCLASSIFIED_CLASS`].
#[must_use]
pub fn golden_class(meta: &HeadlessMeta) -> &'static str {
    let adapter = meta.adapter.to_ascii_lowercase();
    KNOWN_CLASSES
        .iter()
        .find(|(backend, needle, _)| {
            meta.backend.eq_ignore_ascii_case(backend) && adapter.contains(needle)
        })
        .map_or(UNCLASSIFIED_CLASS, |(_, _, class)| *class)
}

/// A [`SceneRenderer`] backed by `frust-render`'s offscreen vello-classic
/// pipeline — the real GPU output a golden case is measured against.
///
/// Holds one adapter, device and `vello::Renderer` across every render, so a
/// whole corpus costs one device creation rather than one per case.
pub struct ClassicOracle {
    renderer: HeadlessRenderer,
    /// The class [`golden_class`] resolved for this adapter, doubling as
    /// [`SceneRenderer::id`] (see that method's contract: the id IS the
    /// golden-class routing key).
    class: &'static str,
    /// Captured once at construction — [`SceneRenderer::meta`]'s contract is
    /// static backend identity, not per-render state.
    meta: BackendMeta,
}

impl ClassicOracle {
    /// Resolves an adapter under `options`, verifies it against whatever
    /// expectation the options/environment carry, and creates the device and
    /// renderer every later [`render`](SceneRenderer::render) reuses.
    ///
    /// Blocking: `frust-render`'s headless API is `async` because wgpu's
    /// adapter/device requests are futures, while [`SceneRenderer`] is sync.
    /// The future is driven to completion here with `pollster`, the same
    /// minimal executor `frust-render`'s own GPU tests use.
    ///
    /// # Errors
    ///
    /// Fails when no adapter is available (the usual "no GPU on this
    /// machine" case a caller turns into a skip), when the resolved adapter
    /// does not meet an `expect_adapter`/`expect_backend` expectation, or
    /// when device/renderer creation fails. Every one of those is a refusal
    /// BEFORE any pixel is produced.
    pub fn new(options: HeadlessOptions) -> Result<Self> {
        let renderer = pollster::block_on(HeadlessRenderer::new(options))?;
        let headless_meta = renderer.meta().clone();
        let class = golden_class(&headless_meta);
        Ok(Self {
            renderer,
            class,
            meta: BackendMeta {
                backend: class.to_string(),
                adapter: headless_meta.adapter.clone(),
                driver: headless_meta.driver.clone(),
                // `HeadlessMeta` reports no `wgpu::DeviceType` (that would be
                // a wgpu type crossing `frust-render`'s confinement
                // boundary), and the tier probe has already refused anything
                // but the classic GPU tier, so the coarse classification is
                // known without one.
                device_kind: "gpu".to_string(),
            },
        })
    }

    /// The GPU this oracle resolved, in `frust-render`'s own vocabulary —
    /// the provenance to print when a corpus run reports which machine it
    /// ran on.
    #[must_use]
    pub fn headless_meta(&self) -> &HeadlessMeta {
        self.renderer.meta()
    }

    /// Whether this oracle resolved onto a reviewed golden class
    /// ([`KNOWN_CLASSES`]) rather than [`UNCLASSIFIED_CLASS`].
    ///
    /// A corpus harness gates baseline COMPARISON on this: an unknown GPU
    /// renders and asserts, but has nothing to diff against.
    #[must_use]
    pub fn is_classified(&self) -> bool {
        self.class != UNCLASSIFIED_CLASS
    }
}

impl SceneRenderer for ClassicOracle {
    fn id(&self) -> &'static str {
        self.class
    }

    fn meta(&self) -> BackendMeta {
        self.meta.clone()
    }

    fn render(
        &mut self,
        scene: &frust_scene::Scene,
        spec: &RenderSpec,
    ) -> anyhow::Result<RenderedImage> {
        let image = pollster::block_on(self.renderer.render(
            scene,
            &HeadlessSpec {
                width: spec.width,
                height: spec.height,
                base_color: spec.base_color,
                // `scale` applies AHEAD of `root` (see `RenderSpec::scale`),
                // the same composition `CpuOracle` builds — the two arms
                // must fold the spec identically or every scaled case would
                // diverge for a reason that is not the renderer's.
                root: Affine::scale(spec.scale) * spec.root,
                // Pinned rather than defaulted: the AA mode is a
                // deterministic input (`docs/TESTING.md`), so the corpus
                // states it instead of inheriting whatever `HeadlessSpec`'s
                // default happens to be.
                aa: HeadlessAa::Area,
            },
        ))?;
        Ok(RenderedImage {
            width: image.width,
            height: image.height,
            rgba8: image.rgba8,
            // Straight, as vello writes it — see the module docs' Alpha
            // section.
            alpha: AlphaKind::Straight,
            meta: self.meta.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headless_meta(backend: &str, adapter: &str) -> HeadlessMeta {
        HeadlessMeta {
            backend: backend.to_string(),
            adapter: adapter.to_string(),
            driver: "test".to_string(),
        }
    }

    #[test]
    fn the_t400_runner_resolves_to_its_own_golden_class() {
        assert_eq!(
            golden_class(&headless_meta("vulkan", "NVIDIA T400 4GB")),
            "vulkan-nvidia-t400"
        );
    }

    #[test]
    fn adapter_matching_is_case_insensitive() {
        assert_eq!(
            golden_class(&headless_meta("Vulkan", "nvidia t400")),
            "vulkan-nvidia-t400"
        );
    }

    #[test]
    fn the_same_card_on_another_backend_is_not_the_vulkan_class() {
        // The class name states the backend, so a `gl`/`dx12` run on the very
        // same card must not claim the Vulkan baselines.
        assert_eq!(
            golden_class(&headless_meta("gl", "NVIDIA T400 4GB")),
            UNCLASSIFIED_CLASS
        );
    }

    #[test]
    fn an_unknown_adapter_is_unclassified_rather_than_slugified() {
        // Deliberately NOT `intel-uhd-770`: a class is a reviewed, owned
        // runner, not whatever name a driver happened to report.
        assert_eq!(
            golden_class(&headless_meta(
                "vulkan",
                "Intel(R) UHD Graphics 770 (ADL-S GT1)"
            )),
            UNCLASSIFIED_CLASS
        );
    }

    #[test]
    fn the_unclassified_class_is_not_a_promotable_directory_name() {
        // A guard on the constant itself: every entry in `KNOWN_CLASSES` is a
        // directory under `testing/goldens/`, and the fallback must never
        // collide with one.
        for (_, _, class) in KNOWN_CLASSES {
            assert_ne!(*class, UNCLASSIFIED_CLASS);
        }
    }
}
