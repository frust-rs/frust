//! Offscreen engine-tier rendering: the harness the crate's own pixel-level
//! regression tests render a [`frust_scene::Scene`] through when there is no
//! window and no surface.
//!
//! `frust-engine` draws through ordinary render passes into a plain
//! `RENDER_ATTACHMENT` texture, so an offscreen render needs no swapchain, no
//! display server and no `wgpu::Surface` — only a GPU, a driver and device-node
//! permission (see `docs/TESTING.md` § Native Headless GPU Rendering). The
//! target itself is `frust_gpu::HeadlessTarget`, the substrate's own offscreen
//! target, rather than a second implementation here.
//!
//! Three properties make this the harness a pixel comparison can be trusted
//! against, as opposed to a hand-rolled render in each test:
//!
//! 1. **The adapter is the one the environment asked for.** Selection goes
//!    through [`wgpu::util::initialize_adapter_from_env_or_default`], the same
//!    call the live device path uses, so `WGPU_ADAPTER_NAME` is honoured on a
//!    multi-GPU host rather than silently ignored by a bare `request_adapter`.
//! 2. **A wrong adapter fails before it can produce pixels.**
//!    [`HeadlessOptions::expect_adapter`]/[`HeadlessOptions::expect_backend`]
//!    (or [`GOLDEN_EXPECT_ADAPTER_ENV_VAR`]/[`GOLDEN_EXPECT_BACKEND_ENV_VAR`])
//!    are checked against the *resolved* adapter in [`HeadlessRenderer::new`],
//!    so a baseline can never be promoted from the wrong GPU with the right
//!    file name.
//! 3. **The device is the app's device.** Limits, optional features and the
//!    render-tier probe are the ones `RenderContext` resolves, so a headless
//!    run cannot pass on capabilities a real surface would never get — and a
//!    tier override that asks for anything but the engine is refused rather
//!    than quietly honoured.
//!
//! Every render is bracketed by a `Validation` error scope and fails on any
//! captured error, and the readback strips wgpu's 256-byte row padding
//! (`frust_gpu::HeadlessTarget::read_back`), so an arbitrary width — not just
//! a multiple of 64 px — reads back exactly.
//!
//! **Alpha.** Pixels come back PREMULTIPLIED, which is what the engine's strip
//! pipelines write and what every frust GPU path uses; an erased pixel still
//! reads `[0, 0, 0, 0]`, and an opaque one is unaffected by the convention.
//!
//! The API is `async` because wgpu's adapter/device requests and its error
//! scope are futures and this crate has no executor dependency; callers drive
//! it with whatever they already have (`pollster::block_on` in tests).
//!
//! No `wgpu` type crosses this module's public surface — the confinement rule
//! in `docs/RENDER_ARCHITECTURE.md`. Pixels come back as plain bytes.

use anyhow::{Result, anyhow};

/// Environment variable naming the adapter a golden/oracle run must have
/// resolved. Case-insensitive substring match on the adapter name, the same
/// shape `WGPU_ADAPTER_NAME` itself matches by — `T400` accepts
/// `NVIDIA T400 4GB`.
///
/// This is a *check*, not a selector: `WGPU_ADAPTER_NAME` chooses the adapter,
/// this refuses the run when the choice did not land where the operator
/// believed it would. Also read by `frust-testing`'s own engine oracle, which
/// is why it lives outside this module's `engine-tier` gate.
pub const GOLDEN_EXPECT_ADAPTER_ENV_VAR: &str = "FRUST_GOLDEN_EXPECT_ADAPTER";

/// Environment variable naming the wgpu backend a golden/oracle run must have
/// resolved (`vulkan`, `metal`, `dx12`, `gl`, …; case-insensitive, matched
/// whole). The backend counterpart of [`GOLDEN_EXPECT_ADAPTER_ENV_VAR`].
pub const GOLDEN_EXPECT_BACKEND_ENV_VAR: &str = "FRUST_GOLDEN_EXPECT_BACKEND";

/// How a [`HeadlessRenderer`] should pick, and then verify, its GPU.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeadlessOptions {
    /// Backend(s) to restrict adapter enumeration to, as a comma-separated
    /// list (`vulkan`, `metal`, `dx12`, `gl`, `vulkan,metal`, …).
    ///
    /// A *hint*: `WGPU_BACKEND` wins when it is set, so an operator can pin the
    /// backend of a run without editing the caller.
    pub backend_hint: Option<String>,
    /// Adapter name the resolved adapter must match (case-insensitive
    /// substring). Overrides [`GOLDEN_EXPECT_ADAPTER_ENV_VAR`] when set.
    pub expect_adapter: Option<String>,
    /// Backend the resolved adapter must be on (case-insensitive, whole word).
    /// Overrides [`GOLDEN_EXPECT_BACKEND_ENV_VAR`] when set.
    pub expect_backend: Option<String>,
}

/// One offscreen render's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadlessSpec {
    /// Target width in device pixels. Any width is valid — the readback pads
    /// and strips rows as wgpu requires.
    pub width: u32,
    /// Target height in device pixels.
    pub height: u32,
    /// The colour the renderer clears the target to before drawing the scene.
    pub base_color: peniko::Color,
    /// A transform applied on top of the whole encoded scene — the headless
    /// analogue of the root a live surface encodes under.
    pub root: kurbo::Affine,
}

impl HeadlessSpec {
    /// A `width` x `height` render over an opaque black base, unrooted — the
    /// defaults a case overrides field by field with struct-update syntax.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            base_color: peniko::color::palette::css::BLACK,
            root: kurbo::Affine::IDENTITY,
        }
    }
}

/// Which GPU actually produced an image — the provenance every promoted
/// baseline and every failing artifact has to record (`docs/TESTING.md`
/// § GPU Run Metadata).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadlessMeta {
    /// The wgpu backend, lowercase (`vulkan`, `metal`, `dx12`, `gl`).
    pub backend: String,
    /// The adapter name as the driver reports it (e.g. `NVIDIA T400 4GB`).
    pub adapter: String,
    /// The driver name and, when the backend reports one, its version detail.
    pub driver: String,
}

impl HeadlessMeta {
    #[cfg(feature = "engine-tier")]
    fn from_info(info: &wgpu::AdapterInfo) -> Self {
        let driver = if info.driver_info.is_empty() {
            info.driver.clone()
        } else if info.driver.is_empty() {
            info.driver_info.clone()
        } else {
            format!("{} ({})", info.driver, info.driver_info)
        };
        Self {
            backend: info.backend.to_str().to_string(),
            adapter: info.name.clone(),
            driver,
        }
    }
}

impl std::fmt::Display for HeadlessMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "backend={} adapter={:?} driver={:?}",
            self.backend, self.adapter, self.driver
        )
    }
}

/// The pixels of one headless render, plus the provenance of the GPU that
/// produced them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadlessImage {
    /// Width in pixels, matching the [`HeadlessSpec`] that produced it.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Tightly packed RGBA8 rows — `width * height * 4` bytes, **no** row
    /// padding, in the engine's PREMULTIPLIED alpha convention (an erased
    /// pixel reads `[0, 0, 0, 0]`).
    pub rgba8: Vec<u8>,
    /// The GPU this image came from.
    pub meta: HeadlessMeta,
}

impl HeadlessImage {
    /// The RGBA bytes of pixel `(x, y)`.
    ///
    /// # Panics
    ///
    /// Panics when `(x, y)` is outside the image — an out-of-bounds probe in a
    /// pixel assertion is a broken test, not a runtime condition to handle.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(
            x < self.width && y < self.height,
            "pixel ({x}, {y}) is outside a {}x{} image",
            self.width,
            self.height
        );
        let at = ((y * self.width + x) * 4) as usize;
        [
            self.rgba8[at],
            self.rgba8[at + 1],
            self.rgba8[at + 2],
            self.rgba8[at + 3],
        ]
    }
}

/// The colour format every headless render targets.
///
/// `Rgba8Unorm` is `frust-engine`'s own off-screen format and is renderable on
/// every wgpu backend, so a readback's channel order needs no per-backend
/// correction — unlike a swapchain, whose reported format is the platform's
/// business.
#[cfg(feature = "engine-tier")]
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A reusable offscreen engine renderer: one adapter, one device, one
/// [`frust_engine::EngineRenderer`] and one size-matched
/// [`frust_gpu::HeadlessTarget`], across any number of renders.
///
/// See the module docs for what makes it a harness rather than a convenience
/// wrapper.
#[cfg(feature = "engine-tier")]
pub struct HeadlessRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    engine: frust_engine::EngineRenderer,
    meta: HeadlessMeta,
    /// The current render target, rebuilt only when a render asks for a
    /// different size.
    target: Option<frust_gpu::HeadlessTarget>,
}

#[cfg(feature = "engine-tier")]
impl HeadlessRenderer {
    /// Resolves an adapter, verifies it against the expectations, probes the
    /// render tier, and creates the device and engine renderer every later
    /// [`render`](Self::render) reuses.
    ///
    /// # Errors
    ///
    /// Fails when no adapter is available, when the resolved adapter does not
    /// meet an `expect_adapter`/`expect_backend` expectation (**before** any
    /// rendering), when the tier probe refuses the adapter or resolves
    /// anything but the engine tier, or when device/renderer creation fails.
    pub async fn new(options: HeadlessOptions) -> Result<Self> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        descriptor.backends = resolve_backends(
            wgpu::Backends::from_env(),
            options.backend_hint.as_deref(),
            descriptor.backends,
        );
        let instance = wgpu::Instance::new(descriptor);

        // The environment-aware initializer, exactly as `RenderContext`'s
        // device creation calls it: a bare `request_adapter` ignores
        // `WGPU_ADAPTER_NAME`, which on a dual-GPU host silently decides which
        // GPU a baseline was captured on.
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .map_err(|e| anyhow!("frust-render headless: no compatible GPU adapter: {e}"))?;
        let meta = HeadlessMeta::from_info(&adapter.get_info());

        let expect_adapter = options
            .expect_adapter
            .or_else(|| golden_env(GOLDEN_EXPECT_ADAPTER_ENV_VAR));
        let expect_backend = options
            .expect_backend
            .or_else(|| golden_env(GOLDEN_EXPECT_BACKEND_ENV_VAR));
        check_expectations(&meta, expect_adapter.as_deref(), expect_backend.as_deref())?;

        // The same tier probe device creation runs, so a headless render can
        // never pass on an adapter the app itself would refuse — and so an
        // override asking for another renderer refuses the run rather than
        // producing pixels under the wrong label.
        let caps = crate::tier::TierCaps {
            downlevel_flags: adapter.get_downlevel_capabilities().flags,
            adapter_name: meta.adapter.clone(),
        };
        let selection =
            crate::tier::select_render_tier(&caps, crate::tier::render_tier_override_from_env());
        match selection.outcome {
            crate::tier::TierOutcome::Available(crate::tier::RenderTier::Engine) => {}
            crate::tier::TierOutcome::Available(other) => {
                return Err(anyhow!(
                    "frust-render headless: this harness renders on the engine tier, but tier \
                     selection resolved `{other:?}` ({}) — unset the render-tier override for a \
                     headless run",
                    selection.diagnosis
                ));
            }
            crate::tier::TierOutcome::Unavailable { .. } => {
                return Err(anyhow!(selection.diagnosis));
            }
        }

        let required_features = adapter.features() & crate::context::optional_device_features();
        let required_limits =
            crate::context::effective_limits(adapter.limits(), crate::context::is_ios_simulator());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-render headless device"),
                required_features,
                required_limits,
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow!("frust-render headless: failed to create GPU device: {e}"))?;

        let tier_caps = frust_gpu::TierCaps::probe(&adapter);
        let mut engine =
            frust_engine::EngineRenderer::new(&device, &tier_caps, TARGET_FORMAT, None).map_err(
                |e| anyhow!("frust-render headless: the engine refused this device: {e}"),
            )?;
        // Warm-up is forced to completion here rather than left to a background
        // worker holding its own handle on the device: a caller that drops this
        // renderer mid-compile tears the device out from under that worker,
        // a driver-level hazard that surfaces at process exit long after every
        // assertion has passed.
        engine.finish_warm_up(&device);

        log::info!("frust-render headless: {meta}");
        Ok(Self {
            device,
            queue,
            engine,
            meta,
            target: None,
        })
    }

    /// The GPU this renderer resolved — the provenance to record with any
    /// image it produces.
    #[must_use]
    pub fn meta(&self) -> &HeadlessMeta {
        &self.meta
    }

    /// Renders `scene` offscreen and reads the pixels back, unpadded.
    ///
    /// # Errors
    ///
    /// Fails on a zero-sized spec, on a refused frame, and on **any** wgpu
    /// validation error captured during the render — a harness that renders
    /// through a validation error is producing pixels nobody should trust.
    pub async fn render(
        &mut self,
        scene: &frust_scene::Scene,
        spec: &HeadlessSpec,
    ) -> Result<HeadlessImage> {
        if spec.width == 0 || spec.height == 0 {
            return Err(anyhow!(
                "frust-render headless: a render target must have a non-zero size, got {}x{}",
                spec.width,
                spec.height
            ));
        }
        self.ensure_target(spec.width, spec.height);
        let target = self
            .target
            .as_ref()
            .expect("ensure_target always leaves a target in place");

        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-render headless frame"),
            });
        let encoded = self.engine.encode(
            &self.device,
            &self.queue,
            &mut encoder,
            scene,
            frust_engine::EngineTarget {
                view: target.view(),
                format: TARGET_FORMAT,
                width: spec.width,
                height: spec.height,
                // The engine owns its own depth attachment here: this harness
                // composites no pass of its own, so there is no surface-owned
                // buffer for the frame to test against.
                depth: None,
                output: frust_engine::OutputAlpha::Premultiplied,
            },
            spec.base_color,
            spec.root,
        );
        // Submitted whether or not the frame encoded: a refused frame leaves
        // the encoder exactly as it was found, and finishing it keeps the
        // device's own bookkeeping in step before the error scope is drained.
        self.queue.submit([encoder.finish()]);
        self.engine.end_frame(&self.queue);

        let validation = scope.pop().await;
        let encoded =
            encoded.map_err(|e| anyhow!("frust-render headless: the frame was refused: {e}"));
        finish_scoped(encoded, validation, "render")?;

        let target = self
            .target
            .as_ref()
            .expect("ensure_target always leaves a target in place");
        Ok(HeadlessImage {
            width: spec.width,
            height: spec.height,
            rgba8: target.read_back(&self.device, &self.queue),
            meta: self.meta.clone(),
        })
    }

    /// Ensures [`Self::target`] is a `width` x `height` render target,
    /// rebuilding it only on a size change and telling the renderer about the
    /// resize so nothing sized against the old extent survives into the next
    /// frame.
    fn ensure_target(&mut self, width: u32, height: u32) {
        if matches!(&self.target, Some(t) if t.width() == width && t.height() == height) {
            return;
        }
        if self.target.is_some() {
            self.engine.resize(&self.device, width, height);
        }
        self.target = Some(frust_gpu::HeadlessTarget::new(
            &self.device,
            width,
            height,
            TARGET_FORMAT,
        ));
    }
}

/// Combines a scoped operation's own result with whatever its `Validation`
/// error scope captured.
///
/// A captured validation error fails the operation even when it otherwise
/// "succeeded" — the case this exists for is a render that produced plausible
/// pixels through an error wgpu's default handler would only have logged.
fn finish_scoped<T>(result: Result<T>, validation: Option<wgpu::Error>, what: &str) -> Result<T> {
    match (result, validation) {
        (Ok(value), None) => Ok(value),
        (Ok(_), Some(error)) => Err(anyhow!(
            "frust-render headless: wgpu validation error during {what}: {error}"
        )),
        (Err(error), None) => Err(error),
        (Err(error), Some(validation)) => {
            Err(error.context(format!("wgpu validation error during {what}: {validation}")))
        }
    }
}

/// The backends adapter enumeration should be restricted to.
///
/// `WGPU_BACKEND` (already parsed into `env_backends`) wins over the caller's
/// `hint`, so an operator can pin a run's backend without editing the caller;
/// with neither set, `fallback` (the instance descriptor's own env-derived
/// default) applies.
fn resolve_backends(
    env_backends: Option<wgpu::Backends>,
    hint: Option<&str>,
    fallback: wgpu::Backends,
) -> wgpu::Backends {
    env_backends
        .or_else(|| hint.map(wgpu::Backends::from_comma_list))
        .unwrap_or(fallback)
}

/// The value of a `FRUST_GOLDEN_EXPECT_*` variable, treating an empty value as
/// unset (`context::env_str`'s shared precedence, runtime half only — these
/// are operator knobs for a host test run, never baked into a binary).
fn golden_env(name: &str) -> Option<String> {
    crate::context::env_str(None, std::env::var(name).ok())
}

/// Refuses a run whose resolved GPU is not the expected one, **before** it can
/// render anything.
///
/// `expect_adapter` matches case-insensitively as a substring, the same way
/// `WGPU_ADAPTER_NAME` selects (so the selector and the check cannot disagree
/// about what `T400` means); `expect_backend` matches the whole backend name,
/// case-insensitively.
fn check_expectations(
    meta: &HeadlessMeta,
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
            "frust-render headless: expected adapter matching `{expected}`, but the run resolved \
             `{}` ({meta}); set WGPU_ADAPTER_NAME (or isolate the driver ICD) so the intended GPU \
             is selected",
            meta.adapter
        ));
    }
    if let Some(expected) = expect_backend.map(str::trim).filter(|e| !e.is_empty())
        && !meta.backend.eq_ignore_ascii_case(expected)
    {
        return Err(anyhow!(
            "frust-render headless: expected backend `{expected}`, but the run resolved `{}` \
             ({meta}); set WGPU_BACKEND to pin it",
            meta.backend
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(backend: &str, adapter: &str) -> HeadlessMeta {
        HeadlessMeta {
            backend: backend.to_string(),
            adapter: adapter.to_string(),
            driver: "test driver".to_string(),
        }
    }

    #[test]
    fn no_expectation_accepts_any_adapter() {
        assert!(check_expectations(&meta("vulkan", "Intel UHD 770"), None, None).is_ok());
        assert!(check_expectations(&meta("vulkan", "Intel UHD 770"), Some(""), Some("  ")).is_ok());
    }

    #[test]
    fn an_expected_adapter_matches_case_insensitively_as_a_substring() {
        let resolved = meta("vulkan", "NVIDIA T400 4GB");
        assert!(check_expectations(&resolved, Some("T400"), None).is_ok());
        assert!(check_expectations(&resolved, Some("t400"), None).is_ok());
        assert!(check_expectations(&resolved, Some(" NVIDIA T400 "), None).is_ok());
    }

    #[test]
    fn the_wrong_adapter_is_refused_naming_both_names() {
        // The dual-GPU host this exists for: the run asked for the T400 and
        // enumeration handed back the integrated GPU.
        let error = check_expectations(
            &meta("vulkan", "Intel UHD Graphics 770"),
            Some("T400"),
            None,
        )
        .expect_err("a mismatched adapter must be refused");
        let message = error.to_string();
        assert!(message.contains("T400"), "{message}");
        assert!(message.contains("Intel UHD Graphics 770"), "{message}");
        assert!(message.contains("WGPU_ADAPTER_NAME"), "{message}");
    }

    #[test]
    fn the_wrong_backend_is_refused() {
        let resolved = meta("gl", "NVIDIA T400 4GB");
        assert!(check_expectations(&resolved, None, Some("vulkan")).is_err());
        assert!(check_expectations(&resolved, None, Some("GL")).is_ok());
        assert!(check_expectations(&resolved, Some("T400"), Some("vulkan")).is_err());
    }

    #[test]
    fn the_backend_env_knob_wins_over_the_caller_hint() {
        assert_eq!(
            resolve_backends(
                Some(wgpu::Backends::VULKAN),
                Some("metal"),
                wgpu::Backends::all()
            ),
            wgpu::Backends::VULKAN
        );
        assert_eq!(
            resolve_backends(None, Some("vulkan"), wgpu::Backends::all()),
            wgpu::Backends::VULKAN
        );
        assert_eq!(
            resolve_backends(None, None, wgpu::Backends::PRIMARY),
            wgpu::Backends::PRIMARY
        );
    }

    #[test]
    fn meta_renders_the_provenance_a_baseline_must_record() {
        let info = meta("vulkan", "NVIDIA T400 4GB");
        let line = info.to_string();
        assert!(line.contains("backend=vulkan"), "{line}");
        assert!(line.contains("NVIDIA T400 4GB"), "{line}");
        assert!(line.contains("test driver"), "{line}");
    }

    /// A stand-in for what a scope pops, since a real one needs a device.
    fn validation_error(description: &str) -> wgpu::Error {
        wgpu::Error::Validation {
            source: Box::new(std::io::Error::other(description.to_string())),
            description: description.to_string(),
        }
    }

    #[test]
    fn a_clean_scope_passes_the_result_through() {
        assert_eq!(finish_scoped(Ok(7_u8), None, "render").unwrap(), 7);
        let failed: Result<u8> = Err(anyhow!("the frame was refused"));
        let error = finish_scoped(failed, None, "render").expect_err("the failure must survive");
        assert!(error.to_string().contains("the frame was refused"));
    }

    #[test]
    fn a_captured_validation_error_fails_an_otherwise_successful_operation() {
        // The case this exists for: plausible pixels produced through an error
        // wgpu's default handler would only have logged.
        let error = finish_scoped(Ok(7_u8), Some(validation_error("bad bind group")), "render")
            .expect_err("a validation error must fail the operation");
        let message = error.to_string();
        assert!(
            message.contains("validation error during render"),
            "{message}"
        );
        assert!(message.contains("bad bind group"), "{message}");
    }

    #[test]
    fn a_captured_validation_error_annotates_a_failed_operation() {
        let failed: Result<u8> = Err(anyhow!("the frame was refused"));
        let error = finish_scoped(failed, Some(validation_error("bad bind group")), "render")
            .expect_err("the failure must survive");
        let chain = format!("{error:#}");
        assert!(chain.contains("the frame was refused"), "{chain}");
        assert!(chain.contains("bad bind group"), "{chain}");
    }
}
