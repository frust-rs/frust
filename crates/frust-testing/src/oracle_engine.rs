//! The GPU arm of the oracle pair: [`EngineOracle`], a [`SceneRenderer`] over
//! `frust-engine`'s [`EngineRenderer`] rendering into a `frust-gpu`
//! [`HeadlessTarget`].
//!
//! # Why it lives here, beside the other arm
//!
//! [`CpuOracle`](crate::oracle_cpu::CpuOracle) and this type are a *pair*:
//! the same [`crate::corpus`] case is handed to both behind one trait object,
//! and the whole point of the comparison is that one trait object can stand
//! in for either. An adapter that only existed inside `frust-engine`'s own
//! test binary could not be driven from a corpus that lives in this crate, so
//! the adapter lives beside the corpus (see the manifest's comment for why
//! the edge is sound for a `publish = false`, dev-dependency-only crate).
//!
//! # The encode contract, from the caller's side
//!
//! [`EngineRenderer::encode`] records into a `wgpu::CommandEncoder` the caller
//! owns and submits nothing. This oracle is that caller: it creates one
//! encoder per frame, hands it to `encode`, submits it once, calls
//! [`EngineRenderer::end_frame`], and only then reads the target back. The
//! whole render runs inside a wgpu validation error scope, drained *before*
//! the pixels are read — an oracle that rendered through a validation error
//! would be producing a reference nobody should trust, and a named error is
//! far more diagnostic than a plausible-looking image.
//!
//! # Which GPU, and which golden class
//!
//! Adapter selection is wgpu's own environment-aware initializer, so
//! `WGPU_BACKEND`/`WGPU_ADAPTER_NAME` pick the GPU on a multi-adapter host
//! rather than the run silently landing on whichever adapter enumerates first.
//! [`FRUST_GOLDEN_EXPECT_ADAPTER`](frust_render::GOLDEN_EXPECT_ADAPTER_ENV_VAR)
//! / [`FRUST_GOLDEN_EXPECT_BACKEND`](frust_render::GOLDEN_EXPECT_BACKEND_ENV_VAR)
//! then turn a wrong choice into a refusal *before any pixel is produced* —
//! two variables, each matched the way the thing it names is selected by
//! (adapter: case-insensitive substring, exactly as `WGPU_ADAPTER_NAME`
//! selects; backend: whole name, case-insensitive).
//!
//! [`engine_golden_class`] is the step after that: mapping the *resolved*
//! adapter onto a golden CLASS name, because `docs/TESTING.md`'s Golden
//! Classes are per-adapter directories and a baseline captured on one GPU
//! must never be compared against another's. [`ENGINE_CLASSES`] is the one
//! table that mapping reads — one reviewed-runner list, one place to add a
//! runner — and every class name in it is `engine-` prefixed because those
//! Golden Classes are per-*pipeline* as well as per-adapter: an engine
//! baseline must never be read as another rasterizer's capture of the same
//! machine. An adapter with no reviewed class resolves to
//! [`ENGINE_UNCLASSIFIED_CLASS`], which is deliberately not a directory
//! anyone promotes into: a run on an unknown GPU records artifacts and
//! compares nothing, rather than inventing a class name and inviting a
//! baseline to be promoted from an unreviewed machine (`docs/TESTING.md`'s
//! Baseline Updates: "never promote output from a machine that failed the
//! adapter/font/environment preflight").
//!
//! # Alpha
//!
//! The engine renders with premultiplied-blended pipelines into a target
//! declared [`OutputAlpha::Premultiplied`], so [`RenderedImage::rgba8`] is
//! PREMULTIPLIED and tagged as such — set from the pipeline's stated
//! convention, not assumed. That is the same shape
//! [`CpuOracle`](crate::oracle_cpu::CpuOracle) reports, so the two arms are
//! compared in the space both natively produce;
//! [`crate::corpus::straighten_alpha`] is the one conversion site every image
//! bound for a stored PNG — which is straight alpha — passes through.

use anyhow::{Result, anyhow};
use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
use frust_gpu::{HeadlessTarget, TierCaps};
use frust_render::{GOLDEN_EXPECT_ADAPTER_ENV_VAR, GOLDEN_EXPECT_BACKEND_ENV_VAR};
use kurbo::Affine;

use crate::render::{AlphaKind, BackendMeta, RenderSpec, RenderedImage, SceneRenderer};

/// The golden class an adapter with no reviewed engine class resolves to.
///
/// Not a directory under `testing/goldens/`: a corpus harness treats this
/// class as "render, assert what is backend-independent, record artifacts,
/// promote nothing" (`docs/TESTING.md`'s Baseline Updates — "never promote
/// output from a machine that failed the adapter/font/environment
/// preflight").
pub const ENGINE_UNCLASSIFIED_CLASS: &str = "engine-unclassified";

/// The reviewed GPU runners this repository keeps an engine golden class
/// for, as `(backend, adapter-name substring, class)`.
///
/// Both the backend (matched whole and case-insensitively —
/// [`AdapterMeta::backend`] already reports it lowercase) and the adapter
/// substring (matched case-insensitively, the same shape
/// `WGPU_ADAPTER_NAME` matches by) must hit, so
/// `engine-vulkan-nvidia-t400` cannot be claimed by the same card running
/// through a different backend.
///
/// Adding a runner is a deliberate act: it declares that the machine's
/// baselines are reviewed and owned (`docs/TESTING.md`'s Golden Classes —
/// "never pretend one device represents every GPU").
const ENGINE_CLASSES: &[(&str, &str, &str)] = &[
    ("vulkan", "t400", "engine-vulkan-nvidia-t400"),
    ("metal", "apple", "engine-metal-macos"),
];

/// The target format every engine frame is rendered into.
///
/// `Rgba8Unorm` reads back R, G, B, A in that order, so a comparison against
/// another arm's bytes needs no swizzle, and it is not the `*Srgb` variant:
/// the strip shader emits 8-bit sRGB-encoded premultiplied colour directly,
/// exactly as `vello_cpu`'s pixmap holds it, so an automatic gamma conversion
/// in the target would make the two arms disagree for a reason that is not
/// the renderer's.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Which GPU actually produced an image — the provenance every promoted
/// baseline and every failing artifact has to record (`docs/TESTING.md`'s
/// GPU Run Metadata).
///
/// This crate's own type rather than a renderer crate's: these three strings
/// are what golden-class routing and the `FRUST_GOLDEN_EXPECT_*` refusals
/// read, and nothing about them belongs to one render tier. No `wgpu` type
/// appears in it, so it crosses this crate's public API the same way
/// [`BackendMeta`] does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterMeta {
    /// The wgpu backend, lowercase (`vulkan`, `metal`, `dx12`, `gl`).
    pub backend: String,
    /// The adapter name as the driver reports it (e.g. `NVIDIA T400 4GB`).
    pub adapter: String,
    /// The driver name and, when the backend reports one, its version detail.
    pub driver: String,
}

impl std::fmt::Display for AdapterMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "backend={} adapter={:?} driver={:?}",
            self.backend, self.adapter, self.driver
        )
    }
}

/// The engine golden class for a resolved adapter, or
/// [`ENGINE_UNCLASSIFIED_CLASS`].
#[must_use]
pub fn engine_golden_class(backend: &str, adapter: &str) -> &'static str {
    let adapter = adapter.to_ascii_lowercase();
    ENGINE_CLASSES
        .iter()
        .find(|(known_backend, needle, _)| {
            backend.eq_ignore_ascii_case(known_backend) && adapter.contains(needle)
        })
        .map_or(ENGINE_UNCLASSIFIED_CLASS, |(_, _, class)| *class)
}

/// Which adapter an [`EngineOracle`] insists on resolving.
///
/// Both fields default to their `FRUST_GOLDEN_EXPECT_*` environment variable
/// when left `None`, so a run pinned from the shell needs no code change and
/// a caller that wants to pin from code is not fighting the environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngineOracleOptions {
    /// Adapter name the resolved GPU must contain, case-insensitively —
    /// the same substring shape `WGPU_ADAPTER_NAME` selects by. Falls back to
    /// [`GOLDEN_EXPECT_ADAPTER_ENV_VAR`].
    pub expect_adapter: Option<String>,
    /// Backend name the resolved GPU must equal, case-insensitively. Falls
    /// back to [`GOLDEN_EXPECT_BACKEND_ENV_VAR`].
    pub expect_backend: Option<String>,
}

/// A [`SceneRenderer`] backed by `frust-engine`'s sparse-strip pipeline — the
/// engine output a golden case is measured against.
///
/// Holds one adapter, device, queue and [`EngineRenderer`] across every
/// render, so a whole corpus costs one device creation rather than one per
/// case; the [`HeadlessTarget`] is rebuilt only when a case asks for a
/// different extent.
pub struct EngineOracle {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: EngineRenderer,
    /// The current render target, rebuilt only when a render asks for a
    /// different size — the same reuse `frust-render`'s headless renderer
    /// applies to its own target.
    target: Option<HeadlessTarget>,
    /// The class [`engine_golden_class`] resolved for this adapter, doubling
    /// as [`SceneRenderer::id`] (see that method's contract: the id IS the
    /// golden-class routing key).
    class: &'static str,
    /// The adapter this oracle resolved — the provenance to print when a run
    /// reports which machine it ran on.
    adapter: AdapterMeta,
    /// Captured once at construction — [`SceneRenderer::meta`]'s contract is
    /// static backend identity, not per-render state.
    meta: BackendMeta,
}

impl EngineOracle {
    /// Resolves an adapter under `options`, verifies it against whatever
    /// expectation the options/environment carry, and creates the device and
    /// renderer every later [`render`](SceneRenderer::render) reuses.
    ///
    /// Blocking: wgpu's adapter and device requests are futures while
    /// [`SceneRenderer`] is sync, so they are driven to completion here with
    /// `pollster`, the same minimal executor every GPU test in this
    /// workspace uses.
    ///
    /// Pipeline warm-up is forced to completion before returning. Warm-up
    /// normally runs on a background worker holding its own handle on the
    /// `wgpu::Device`, and a caller that drops this oracle while that worker
    /// is mid-compile tears the device out from under it — a driver-level
    /// hazard that surfaces at process exit, long after every assertion has
    /// passed. Paying for it once at construction keeps a teardown race from
    /// being read as a rendering fault.
    ///
    /// # Errors
    ///
    /// Fails when no adapter is available (the usual "no GPU on this machine"
    /// case a caller turns into a skip), when the resolved adapter does not
    /// meet an `expect_adapter`/`expect_backend` expectation, and when device
    /// or renderer creation fails. Every one of those is a refusal BEFORE any
    /// pixel is produced.
    pub fn new(options: &EngineOracleOptions) -> Result<Self> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        // The environment-aware initializer, exactly as `frust-render`'s
        // headless renderer and `frust-engine`'s own encode-contract test call
        // it: a bare `request_adapter` ignores `WGPU_ADAPTER_NAME`, which on a
        // dual-GPU host silently decides which GPU a baseline was captured on.
        let adapter = pollster::block_on(wgpu::util::initialize_adapter_from_env_or_default(
            &instance, None,
        ))
        .map_err(|err| anyhow!("frust-testing engine oracle: no compatible GPU adapter: {err}"))?;

        let info = adapter.get_info();
        let meta = adapter_meta(&info);
        check_expectations(
            &meta,
            options
                .expect_adapter
                .clone()
                .or_else(|| golden_env(GOLDEN_EXPECT_ADAPTER_ENV_VAR))
                .as_deref(),
            options
                .expect_backend
                .clone()
                .or_else(|| golden_env(GOLDEN_EXPECT_BACKEND_ENV_VAR))
                .as_deref(),
        )?;

        let caps = TierCaps::probe(&adapter);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("frust-testing engine oracle device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .map_err(|err| {
            anyhow!("frust-testing engine oracle: failed to create GPU device: {err}")
        })?;

        let mut renderer =
            EngineRenderer::new(&device, &caps, TARGET_FORMAT, None).map_err(|err| {
                anyhow!("frust-testing engine oracle: the engine refused this device: {err}")
            })?;
        renderer.finish_warm_up(&device);

        let class = engine_golden_class(&meta.backend, &meta.adapter);
        log::info!("frust-testing engine oracle: {meta} class={class}");
        Ok(Self {
            device,
            queue,
            renderer,
            target: None,
            class,
            meta: BackendMeta {
                backend: class.to_string(),
                adapter: meta.adapter.clone(),
                driver: meta.driver.clone(),
                // The engine tier only runs on a real GPU adapter, so the
                // coarse classification is known without asking wgpu for a
                // `DeviceType` (which would be a wgpu type crossing this
                // crate's own confinement boundary).
                device_kind: "gpu".to_string(),
            },
            adapter: meta,
        })
    }

    /// The GPU this oracle resolved — the provenance to print when a run
    /// reports which machine it ran on.
    #[must_use]
    pub fn adapter_meta(&self) -> &AdapterMeta {
        &self.adapter
    }

    /// Whether this oracle resolved onto a reviewed engine golden class
    /// ([`ENGINE_CLASSES`]) rather than [`ENGINE_UNCLASSIFIED_CLASS`].
    ///
    /// A corpus harness gates baseline PROMOTION on this: an unknown GPU
    /// renders and asserts, but has nothing anyone should promote.
    #[must_use]
    pub fn is_classified(&self) -> bool {
        self.class != ENGINE_UNCLASSIFIED_CLASS
    }

    /// Rebuilds the render target when the requested extent differs from the
    /// live one, telling the renderer about the resize so nothing sized
    /// against the old extent survives into the next frame.
    fn ensure_target(&mut self, width: u32, height: u32) {
        let matches = self
            .target
            .as_ref()
            .is_some_and(|target| target.width() == width && target.height() == height);
        if matches {
            return;
        }
        if self.target.is_some() {
            self.renderer.resize(&self.device, width, height);
        }
        self.target = Some(HeadlessTarget::new(
            &self.device,
            width,
            height,
            TARGET_FORMAT,
        ));
    }
}

impl SceneRenderer for EngineOracle {
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
        if spec.width == 0 || spec.height == 0 {
            return Err(anyhow!(
                "frust-testing engine oracle: a render target must have a non-zero size, got {}x{}",
                spec.width,
                spec.height
            ));
        }
        self.ensure_target(spec.width, spec.height);
        let target = self
            .target
            .as_ref()
            .expect("ensure_target leaves a target in place");

        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-testing engine oracle frame"),
            });
        let encoded = self.renderer.encode(
            &self.device,
            &self.queue,
            &mut encoder,
            scene,
            EngineTarget {
                view: target.view(),
                format: TARGET_FORMAT,
                width: spec.width,
                height: spec.height,
                // The engine owns its own depth attachment here: this oracle
                // composites no 3D pass of its own, so there is no populated
                // depth buffer for a frame to test against.
                depth: None,
                output: OutputAlpha::Premultiplied,
            },
            spec.base_color,
            // `scale` applies AHEAD of `root` (see `RenderSpec::scale`), the
            // same composition the other two arms build — the arms must fold
            // the spec identically or every scaled case would diverge for a
            // reason that is not the renderer's.
            Affine::scale(spec.scale) * spec.root,
        );
        // Submitted whether or not the frame encoded: a refused frame leaves
        // the encoder exactly as it was found, and finishing it keeps the
        // device's own bookkeeping in step before the error scope is drained.
        self.queue.submit([encoder.finish()]);
        self.renderer.end_frame(&self.queue);

        // Drained before the pixels are read: a wrong bind group, a mismatched
        // attachment or an out-of-range draw names itself here, where a pixel
        // mismatch would only say that something went wrong.
        let validation = drain_error_scope(&self.device, scope);
        encoded
            .map_err(|err| anyhow!("frust-testing engine oracle: the frame was refused: {err}"))?;
        if let Some(error) = validation {
            return Err(anyhow!(
                "frust-testing engine oracle: wgpu validation error during render: {error}"
            ));
        }

        let target = self
            .target
            .as_ref()
            .expect("ensure_target leaves a target in place");
        let rgba8 = target.read_back(&self.device, &self.queue);
        Ok(RenderedImage {
            width: spec.width,
            height: spec.height,
            rgba8,
            // Premultiplied, as the strip pipelines blend and the target was
            // declared — see the module docs' Alpha section.
            alpha: AlphaKind::Premultiplied,
            meta: self.meta.clone(),
        })
    }
}

/// The adapter identity recorded for a run, built from wgpu's own report.
///
/// The driver fold — name, version detail, or both — is the shape every GPU
/// run in this repository has printed since the first golden class was
/// promoted, so a re-run on the same machine prints (and routes on) the same
/// strings a stored baseline's provenance recorded.
fn adapter_meta(info: &wgpu::AdapterInfo) -> AdapterMeta {
    let driver = if info.driver_info.is_empty() {
        info.driver.clone()
    } else if info.driver.is_empty() {
        info.driver_info.clone()
    } else {
        format!("{} ({})", info.driver, info.driver_info)
    };
    AdapterMeta {
        backend: info.backend.to_str().to_string(),
        adapter: info.name.clone(),
        driver,
    }
}

/// The value of a `FRUST_GOLDEN_EXPECT_*` variable, treating an empty value as
/// unset — an operator knob for a host test run, never baked into a binary.
fn golden_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Refuses a run whose resolved GPU is not the expected one, **before** it can
/// render anything.
///
/// `expect_adapter` matches case-insensitively as a substring, the same way
/// `WGPU_ADAPTER_NAME` selects (so the selector and the check cannot disagree
/// about what `T400` means); `expect_backend` matches the whole backend name,
/// case-insensitively.
fn check_expectations(
    meta: &AdapterMeta,
    expect_adapter: Option<&str>,
    expect_backend: Option<&str>,
) -> Result<()> {
    if let Some(expected) = expect_adapter.map(str::trim).filter(|e| !e.is_empty())
        && !meta
            .adapter
            .to_lowercase()
            .contains(&expected.to_lowercase())
    {
        return Err(anyhow!(
            "frust-testing engine oracle: expected adapter matching `{expected}`, but the run \
             resolved `{}` ({meta}); set WGPU_ADAPTER_NAME (or isolate the driver ICD) so the \
             intended GPU is selected",
            meta.adapter
        ));
    }
    if let Some(expected) = expect_backend.map(str::trim).filter(|e| !e.is_empty())
        && !meta.backend.eq_ignore_ascii_case(expected)
    {
        return Err(anyhow!(
            "frust-testing engine oracle: expected backend `{expected}`, but the run resolved \
             `{}` ({meta}); set WGPU_BACKEND to pin it",
            meta.backend
        ));
    }
    Ok(())
}

/// Pops a validation error scope, pumping the device until the pop resolves.
///
/// The scope's `pop` is a future that only completes once the device has
/// processed the work it covers, so it is driven with an explicit poll loop
/// rather than handed to a block-on executor that would have nothing to drive
/// it with — the same shape `frust-engine`'s own encode-contract test uses.
fn drain_error_scope(device: &wgpu::Device, scope: wgpu::ErrorScopeGuard) -> Option<wgpu::Error> {
    use std::task::{Context, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = std::pin::pin!(scope.pop());
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(error) => return error,
            Poll::Pending => {
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(backend: &str, adapter: &str) -> AdapterMeta {
        AdapterMeta {
            backend: backend.to_string(),
            adapter: adapter.to_string(),
            driver: "test".to_string(),
        }
    }

    #[test]
    fn the_t400_runner_resolves_to_its_own_engine_class() {
        assert_eq!(
            engine_golden_class("vulkan", "NVIDIA T400 4GB"),
            "engine-vulkan-nvidia-t400"
        );
        assert_eq!(
            engine_golden_class("Vulkan", "nvidia t400"),
            "engine-vulkan-nvidia-t400",
            "adapter matching is case-insensitive, exactly as WGPU_ADAPTER_NAME selects"
        );
    }

    #[test]
    fn the_mac_metal_runner_resolves_to_its_own_engine_class() {
        assert_eq!(
            engine_golden_class("metal", "Apple M4"),
            "engine-metal-macos"
        );
    }

    #[test]
    fn an_unreviewed_adapter_is_unclassified_rather_than_slugified() {
        // Deliberately NOT `engine-intel-uhd-770`: a class is a reviewed,
        // owned runner, not whatever name a driver happened to report.
        assert_eq!(
            engine_golden_class("vulkan", "Intel(R) UHD Graphics 770 (ADL-S GT1)"),
            ENGINE_UNCLASSIFIED_CLASS
        );
        // The class name states the backend, so a `gl`/`dx12` run on the very
        // same card must not claim the Vulkan baselines.
        assert_eq!(
            engine_golden_class("gl", "NVIDIA T400 4GB"),
            ENGINE_UNCLASSIFIED_CLASS
        );
    }

    #[test]
    fn the_unclassified_class_is_not_a_promotable_directory_name() {
        // A guard on the constant itself: every entry in `ENGINE_CLASSES` is
        // a directory under `testing/goldens/`, and the fallback must never
        // collide with one.
        for (_, _, class) in ENGINE_CLASSES {
            assert_ne!(*class, ENGINE_UNCLASSIFIED_CLASS);
        }
    }

    #[test]
    fn every_reviewed_class_is_engine_prefixed() {
        // The engine's baselines share `testing/goldens/` with every other
        // arm's, so the prefix is what keeps one machine's engine capture
        // from ever being read as another rasterizer's.
        for (_, _, class) in ENGINE_CLASSES {
            assert!(
                class.starts_with("engine-"),
                "`{class}` is not prefixed as an engine class"
            );
        }
        assert!(ENGINE_UNCLASSIFIED_CLASS.starts_with("engine-"));
    }

    #[test]
    fn an_expectation_refuses_the_wrong_adapter_and_accepts_the_right_one() {
        let resolved = meta("vulkan", "NVIDIA T400 4GB");
        assert!(check_expectations(&resolved, Some("t400"), Some("vulkan")).is_ok());
        assert!(check_expectations(&resolved, None, None).is_ok());
        // An empty expectation is "unset", not "match nothing".
        assert!(check_expectations(&resolved, Some("  "), None).is_ok());

        let refusal = check_expectations(&resolved, Some("UHD"), None)
            .expect_err("a foreign adapter must be refused");
        assert!(
            refusal.to_string().contains("NVIDIA T400 4GB"),
            "the refusal names what was actually resolved: {refusal}"
        );
        assert!(check_expectations(&resolved, None, Some("metal")).is_err());
    }

    #[test]
    fn the_target_format_needs_no_swizzle_and_no_gamma_conversion() {
        assert_eq!(TARGET_FORMAT, wgpu::TextureFormat::Rgba8Unorm);
    }
}
