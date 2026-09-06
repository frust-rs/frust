//! The renderer-side half of surface setup: which per-frame render path a
//! configured swapchain runs on, and the engine resources that path owns.
//!
//! The device/surface foundation itself — the `wgpu` instance, the lazily
//! created logical device, the surface factory, the cross-thread
//! [`DetachedSurface`](crate::DetachedSurface) hand-off, the alpha-mode and
//! format resolution, the swapchain configuration — lives one layer down in
//! `frust-gpu` ([`frust_gpu::context::RenderContext`],
//! [`frust_gpu::surface`]), which this crate re-exports under its own names so
//! no shell file has to know where any of it moved to.
//!
//! What stays here is what only a *renderer* can answer: given a swapchain
//! whose alpha mode and backend are now known, does the engine draw straight
//! into it ([`RenderPathKind::EngineDirect`]) or into an intermediate one
//! fragment pass un-premultiplies from
//! ([`RenderPathKind::EngineDirectUnpremultiply`])? [`choose_engine_render_path`]
//! decides, [`RenderPath`] owns the resources the chosen arm needs, and
//! [`EngineSurface`] pairs that with `frust-gpu`'s renderer-agnostic
//! [`frust_gpu::ConfiguredSurface`].
//!
//! Surface *lifecycle* is driven from [`crate::SurfaceRenderer`]: the shell
//! mints an empty `SurfaceRenderer` and drives it with
//! `on_surface_created`/`on_surface_changed`/`on_surface_destroyed`, each of
//! which reaches back into the context for the owning device.

use anyhow::{Result, anyhow};
#[cfg(feature = "perf-trace")]
use std::sync::OnceLock;

use frust_gpu::{ConfiguredSurface, DeviceHandle, RenderContext, SurfaceAlphaRequest};

// The device-request policy lives in `frust-gpu` now — one copy, shared with
// every other consumer of that crate. Re-exported under this module's own path
// so the call sites here and in `crate::headless` keep naming
// `crate::context::…`, and so a reader of this module still finds the policy
// beside the surface setup that applies it.
pub(crate) use frust_gpu::context::{effective_limits, is_ios_simulator, optional_device_features};
// Two platform facts about a resolved alpha mode, likewise `frust-gpu`'s: what
// the mode says the swapchain stores, and the one backend/mode pair where that
// claim is untrue. The *routing* they feed ([`choose_engine_render_path`]) is
// this crate's.
use frust_gpu::surface::{alpha_mode_is_straight_translucent, compositor_expects_premultiplied};

/// Whether a `FRUST_*` boolean env flag is set to a non-zero value, checking
/// both the compile-time (`option_env!`) and runtime (`std::env::var`) halves —
/// the compile-time-or-runtime parsing every shipping flag (`FRUST_TRACE`,
/// `FRUST_NO_FRAME_GATE`, `FRUST_NO_RENDER_THREAD`, …) uses, so an Android app
/// process (which has no runtime env) still honours a baked-in value.
///
/// `FRUST_TRACE` (via [`perf_tracing_enabled`]) is the only such flag this
/// crate still reads, and only under the `perf-trace` feature — hence the
/// `allow` rather than a `cfg`, which would take the unit tests below with it.
#[cfg_attr(not(feature = "perf-trace"), allow(dead_code))]
fn env_flag_enabled(name_compile_time: Option<&str>, name_runtime: Option<String>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if v != "0")
    }
    is_set_non_zero(name_compile_time) || is_set_non_zero(name_runtime.as_deref())
}

/// `FRUST_TRACE` flag — mirroring the check in `frust-shell-common::perf`.
/// Cached to avoid repeated environment lookups.
///
/// Only compiled under the `perf-trace` feature — the sole callers,
/// [`log_render_path`]/[`log_surface_alpha_caps`], are themselves
/// feature-gated with an inert `#[cfg(not(feature = "perf-trace"))]`
/// counterpart that skips this check entirely, so a build without the feature
/// contains neither this env read nor the probe bodies it guards.
#[cfg(feature = "perf-trace")]
fn perf_tracing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_TRACE"),
            std::env::var("FRUST_TRACE").ok(),
        )
    })
}

/// Which per-frame render path a configured surface uses (see [`RenderPath`]).
///
/// Pure decision output, kept separate from the `wgpu` resources so the
/// selection logic ([`choose_engine_render_path`]) is unit-testable without a
/// GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderPathKind {
    /// `frust-engine` renders into the acquired swapchain texture, which needs
    /// only `RENDER_ATTACHMENT` in whatever format the surface reports — no
    /// `Rgba8Unorm` requirement, no `STORAGE_BINDING`, no intermediate and no
    /// blit (ordinary render passes, no compute storage write), plus a
    /// depth attachment the surface carries alongside it
    /// ([`RenderPath::EngineDirect`]). Chosen by
    /// [`choose_engine_render_path`], which routes every surface onto one of
    /// this enum's two arms.
    EngineDirect,
    /// `frust-engine` renders into an intermediate of the surface's own and a
    /// full-screen fragment pass un-premultiplies it into the acquired
    /// swapchain texture ([`RenderPath::EngineDirectUnpremultiply`]) — the
    /// engine arm for a swapchain that stores STRAIGHT alpha and whose
    /// compositor genuinely reads it that way, which the engine's
    /// premultiplied output cannot be handed directly. `PostMultiplied` on
    /// `wgpu::Backend::Metal` (iOS's translucent mode included) is the one
    /// exception: [`compositor_expects_premultiplied`] answers true for it —
    /// an upstream wgpu-hal truth bug — so it takes [`EngineDirect`] instead,
    /// skipping this conversion entirely.
    ///
    /// [`EngineDirect`]: RenderPathKind::EngineDirect
    ///
    /// Still the engine family and still `RENDER_ATTACHMENT`-only on both
    /// textures: the conversion is an ordinary render pass into the swapchain
    /// (never a compute/storage write), so nothing about this arm needs a
    /// capability the engine refuses.
    EngineDirectUnpremultiply,
}

/// The engine tier's render path for a surface on `backend` whose resolved
/// alpha mode is `alpha_mode` — one of the two engine arms.
///
/// The engine targets an ordinary render attachment in the surface's own
/// format, so no format/usage probe participates; what splits the two arms
/// apart is the engine's output convention against the swapchain's:
///
/// - `Opaque`/`Auto` ignore alpha entirely — [`RenderPathKind::EngineDirect`].
/// - `Inherit` (Android's translucent mode) and `PreMultiplied` expect
///   PREMULTIPLIED alpha, which is what the engine's strip pipelines already
///   write (the premultiplied convention every frust GPU path uses). Served
///   as-is on the same arm: no conversion, which would darken every
///   partial-alpha pixel by a second factor of `a`.
/// - `PostMultiplied` expects STRAIGHT alpha and would read `(C·a, a)` as
///   `(C, a)` — every partial-alpha pixel too dark. It takes
///   [`RenderPathKind::EngineDirectUnpremultiply`], where the frame lands in
///   an intermediate first and one fragment pass converts it on the way to
///   the swapchain — **except** on `wgpu::Backend::Metal`, where
///   [`compositor_expects_premultiplied`] answers true: the backend's own
///   `PostMultiplied` swapchain composites premultiplied regardless of its
///   name (an upstream wgpu-hal truth bug, documented on that predicate), so
///   handing it the straight-alpha conversion would double-correct. That
///   combination alone takes [`RenderPathKind::EngineDirect`] too — the same
///   arm `Inherit`/`PreMultiplied` take, and for the identical reason: the
///   engine's premultiplied output already matches what the compositor reads.
///   This also answers iOS, whose sole translucent mode is a Metal
///   `PostMultiplied` surface, removing the conversion pass's per-frame cost
///   there entirely.
///
/// Every resolved (backend, alpha mode) pair therefore has an engine arm:
/// there is no unserveable answer here and no translucency for this tier to
/// refuse.
///
/// Pure and (backend, mode)-only, so it is host-testable without a surface.
/// The two predicates it reads are platform facts owned by `frust-gpu`
/// ([`frust_gpu::surface`]); the *routing* they feed is this crate's.
pub(crate) fn choose_engine_render_path(
    backend: wgpu::Backend,
    alpha_mode: wgpu::CompositeAlphaMode,
) -> RenderPathKind {
    if alpha_mode_is_straight_translucent(alpha_mode)
        && !compositor_expects_premultiplied(backend, alpha_mode)
    {
        RenderPathKind::EngineDirectUnpremultiply
    } else {
        RenderPathKind::EngineDirect
    }
}

/// Refuses an adapter that cannot drive the engine, carrying
/// [`crate::EngineUnsupported`]'s diagnosis as the error.
///
/// Consults the shared capability gate ([`crate::engine_support`]) rather than
/// a bespoke downlevel check, so a refused adapter surfaces through the one
/// diagnostic path that gate owns — shared with its own unit tests and with
/// the headless harness, which asks the identical question of the adapter it
/// resolved. The engine requires no downlevel flag at all
/// ([`crate::ENGINE_REQUIRED_DOWNLEVEL_FLAGS`] is empty), so every adapter
/// that reaches here passes today; the refusal arm exists so a requirement
/// ever added to that constant refuses the surface with a diagnosis instead of
/// failing somewhere inside the engine.
///
/// Asked per surface creation rather than per device creation: the device is
/// `frust-gpu`'s to build and that crate must not depend on this one's gate.
/// Both happen once per surface episode on every shipping platform, and the
/// answer is a pure function of the adapter either way.
fn check_engine_support(adapter: &wgpu::Adapter) -> Result<()> {
    let caps = crate::tier::TierCaps {
        downlevel_flags: adapter.get_downlevel_capabilities().flags,
        adapter_name: adapter.get_info().name,
    };
    crate::tier::engine_support(&caps).map_err(|refusal| anyhow!(refusal.to_string()))
}

/// Emits the one-per-process startup line naming the chosen render path and the
/// reason, mirroring the surface-alpha line's style and its `FRUST_TRACE`
/// gating. Logged once regardless of surface recreation.
///
/// Only compiled under the `perf-trace` feature; see the inert
/// `#[cfg(not(feature = "perf-trace"))]` counterpart
/// below.
#[cfg(feature = "perf-trace")]
fn log_render_path(path: RenderPathKind) {
    static LOGGED: OnceLock<()> = OnceLock::new();

    if !perf_tracing_enabled() {
        return;
    }

    LOGGED.get_or_init(|| {
        let (name, reason) = match path {
            RenderPathKind::EngineDirect => (
                "engine-direct",
                "frust-engine into the acquired swapchain view (RENDER_ATTACHMENT, \
                 surface-reported format, surface-owned depth)",
            ),
            RenderPathKind::EngineDirectUnpremultiply => (
                "engine-direct-unpremultiply",
                "frust-engine into a surface-owned intermediate, un-premultiplied into the \
                 swapchain by one fragment pass (straight-alpha translucent surface)",
            ),
        };
        log::info!("frust-perf render-path {name} ({reason})");
    });
}

/// Without the `perf-trace` feature, the render-path startup line is a
/// complete no-op — no logging, no format-string bodies compiled in.
#[cfg(not(feature = "perf-trace"))]
#[inline]
fn log_render_path(_path: RenderPathKind) {}

/// Permanent surface-caps + chosen-alpha-mode line, logged once
/// per surface configure (not once per process — a resize/recreate that picks
/// a different mode is worth a fresh line, unlike the render-path line above).
/// Gated exactly like [`log_render_path`] so a release build (no `perf-trace`
/// feature) stays string-free.
///
/// The surface's reported `alpha_modes` are re-read from the live surface here
/// rather than threaded out of `frust-gpu`'s resolution: the read costs a wgpu
/// round-trip and happens **only** in a `perf-trace` build with `FRUST_TRACE`
/// actually set, so the production path pays nothing for it.
#[cfg(feature = "perf-trace")]
fn log_surface_alpha_caps(surface: &ConfiguredSurface, handle: &DeviceHandle) {
    if !perf_tracing_enabled() {
        return;
    }
    let capabilities = surface.surface().get_capabilities(&handle.adapter);
    log::info!(
        "frust-render surface-caps: alpha_modes={:?} chosen={:?}",
        capabilities.alpha_modes,
        surface.config().alpha_mode
    );
}

/// Without the `perf-trace` feature, the surface-caps/alpha line is a
/// complete no-op — no logging, no format-string bodies compiled in, and no
/// capabilities read.
#[cfg(not(feature = "perf-trace"))]
#[inline]
fn log_surface_alpha_caps(_surface: &ConfiguredSurface, _handle: &DeviceHandle) {}

/// Whether a `width` x `height` target fits the device's own
/// `max_texture_dimension_2d`.
///
/// The engine arms are the callers: the swapchain is a plain
/// `RENDER_ATTACHMENT` texture paired with a depth attachment of the identical
/// extent ([`RenderPath::EngineDirect`]) and, on the un-premultiplying arm, an
/// intermediate of that extent as well — so an over-ceiling surface would be
/// two or three texture creations wgpu refuses, a validation error
/// mid-configure rather than a diagnosis. Asked once, up front, against the
/// limits the device was actually requested with ([`effective_limits`], which
/// is what `wgpu::Device::limits` reports back), so the surface is refused with
/// a message naming the ceiling instead.
///
/// Pure and host-testable.
pub(crate) fn extent_within_limits(width: u32, height: u32, max_dimension_2d: u32) -> bool {
    width <= max_dimension_2d && height <= max_dimension_2d
}

/// The per-frame render path an [`EngineSurface`] carries — the resources
/// specific to the chosen arm (see [`RenderPathKind`]).
///
/// The engine arms render into the acquired swapchain view, plus, on the
/// un-premultiplying one, a surface-owned intermediate.
pub(crate) enum RenderPath {
    /// The engine tier's arm ([`RenderPathKind::EngineDirect`]): `frust-engine`
    /// records its passes straight into the acquired swapchain view, so there
    /// is no intermediate and no blitter — but the frame needs a depth
    /// attachment matching the target extent, and this is what owns it.
    ///
    /// Owned HERE rather than left to the engine's own lazily allocated
    /// attachment because the surface is what knows when the extent changed:
    /// [`EngineSurface::resize`] recreates it in the same step that
    /// reconfigures the swapchain, keeping the reallocation off the frame
    /// path, and the pairing can never disagree with the colour attachment it
    /// is attached beside.
    EngineDirect {
        /// The `Depth24Plus` attachment this surface's frames test against,
        /// sized to the swapchain and recreated with it. Handed to the engine
        /// as `EngineTarget::depth`; the engine clears it each frame (it is
        /// not pre-cleared — nothing else writes it).
        depth: frust_engine::DepthTexture,
    },
    /// The engine tier's arm for a **straight-alpha translucent** swapchain
    /// (`PostMultiplied`, iOS's translucent mode —
    /// [`alpha_mode_is_straight_translucent`]): the engine renders the frame
    /// into `intermediate_view` and `present` un-premultiplies it into the
    /// acquired swapchain texture, `(rgb / max(a, ε), a)` in one full-screen
    /// fragment pass recorded into the frame's own encoder.
    ///
    /// The conversion is a *render* pass, not a compute dispatch: this tier
    /// runs where compute does not, and its swapchain carries no
    /// `STORAGE_BINDING` to write through.
    ///
    /// The intermediate is `Rgba8Unorm` (`frust_engine`'s own off-screen
    /// format, which is what the renderer's pipelines are warmed for on this
    /// arm) with `RENDER_ATTACHMENT | TEXTURE_BINDING`: a colour attachment for
    /// the frame, a sampled source for the pass. Only its view is stored — a
    /// `TextureView` refcounts its texture alive and nothing here needs the
    /// texture handle.
    EngineDirectUnpremultiply {
        /// The depth attachment, exactly as [`Self::EngineDirect`] carries it —
        /// sized to the swapchain and recreated with it, since the intermediate
        /// the frame targets has the swapchain's own extent.
        depth: frust_engine::DepthTexture,
        /// The premultiplied intermediate the engine renders the frame into,
        /// recreated on resize alongside the depth attachment above.
        intermediate_view: wgpu::TextureView,
        /// The conversion, built once per surface configure for the
        /// swapchain's own format.
        present: frust_engine::gpu::present::UnpremultiplyPass,
    },
}

/// A configured swapchain plus the engine resources of the render path it was
/// configured for.
///
/// `frust-gpu`'s [`ConfiguredSurface`] is deliberately renderer-agnostic — the
/// surface, the configuration it came up with, and the resolved alpha facts —
/// so the arm-specific attachments a *frust-engine* frame needs are paired with
/// it here rather than pushed down into that crate. A thin wrapper rather than
/// a field on [`crate::SurfaceRenderer`]: the two halves are created together,
/// resized together and dropped together, and keeping them in one value is what
/// makes "the depth attachment always matches the swapchain extent" true by
/// construction.
///
/// Crate-private; the wrapped `wgpu` types never escape `frust-render`.
pub(crate) struct EngineSurface {
    /// The renderer-agnostic swapchain half.
    pub(crate) gpu: ConfiguredSurface,
    /// The engine resources of this surface's arm.
    pub(crate) path: RenderPath,
}

impl EngineSurface {
    /// The configuration this surface is currently up with.
    pub(crate) fn config(&self) -> &wgpu::SurfaceConfiguration {
        self.gpu.config()
    }

    /// Whether this surface **actually** came up translucent — the wgpu-free
    /// projection of its resolved alpha mode, computed once at configure time.
    /// No arm refuses translucency any more: every resolved alpha mode has an
    /// engine arm ([`choose_engine_render_path`]), straight-alpha translucency
    /// included.
    ///
    /// The value a shell observes through
    /// [`SurfaceRenderer::surface_resolved_translucent`](crate::SurfaceRenderer::surface_resolved_translucent)
    /// crosses this crate's boundary with no `wgpu` type in the signature
    /// (`docs/CODE_STANDARDS.md`'s wgpu-leak anti-pattern).
    pub(crate) fn resolved_translucent(&self) -> bool {
        self.gpu.resolved_translucent()
    }

    /// (Re)applies the current configuration to the swapchain — the recovery
    /// step an `Outdated` (or retried `Invalid`) acquire asks for, where the
    /// surface is stale but its geometry has not changed, so no arm resource
    /// needs rebuilding.
    pub(crate) fn reconfigure(&self, device: &wgpu::Device) {
        self.gpu.reconfigure(device);
    }

    /// Resizes the swapchain in place and rebuilds the arm resources sized
    /// against it. Each engine arm recreates the depth attachment (plus, on the
    /// un-premultiplying arm, the intermediate) that has to match the
    /// swapchain's extent. Zero dimensions are rejected upstream.
    pub(crate) fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        match &mut self.path {
            // A depth attachment must match its colour attachment's extent
            // exactly, so the resize recreates it here — in the same step that
            // reconfigures the swapchain, keeping the allocation off the frame
            // path. Skipped when the extent is unchanged
            // (`DepthTexture::matches`), which is what a reconfigure with the
            // same size costs on the other arm too.
            //
            // An over-ceiling extent keeps the existing attachment rather than
            // creating one wgpu would refuse: the swapchain reconfigure below
            // is what fails on such a size, and a frame encoded against a
            // mismatched depth attachment is refused by the engine rather than
            // submitted ([`extent_within_limits`], the same ceiling
            // `create_engine_surface` refuses on).
            RenderPath::EngineDirect { depth } => {
                let max_dimension_2d = device.limits().max_texture_dimension_2d;
                if !extent_within_limits(width, height, max_dimension_2d) {
                    log::warn!(
                        "frust-render: engine tier cannot resize to {width}x{height} — the \
                         device's max_texture_dimension_2d is {max_dimension_2d}; keeping the \
                         previous depth attachment"
                    );
                } else if !depth.matches(width, height) {
                    *depth = frust_engine::DepthTexture::new(device, width, height);
                }
            }
            // The un-premultiplying arm recreates its intermediate alongside
            // that same depth attachment, both under the same ceiling: the
            // frame renders into the intermediate and the conversion writes the
            // swapchain, so all three have to agree on one extent. The pass
            // itself survives — it holds a pipeline built for the surface's
            // format, which a resize never changes, and its bind group names
            // the source view per frame rather than at build time.
            RenderPath::EngineDirectUnpremultiply {
                depth,
                intermediate_view,
                ..
            } => {
                let max_dimension_2d = device.limits().max_texture_dimension_2d;
                if !extent_within_limits(width, height, max_dimension_2d) {
                    log::warn!(
                        "frust-render: engine tier cannot resize to {width}x{height} — the \
                         device's max_texture_dimension_2d is {max_dimension_2d}; keeping the \
                         previous depth attachment and intermediate"
                    );
                } else {
                    if !depth.matches(width, height) {
                        *depth = frust_engine::DepthTexture::new(device, width, height);
                    }
                    *intermediate_view = create_engine_intermediate(width, height, device);
                }
            }
        }
        self.gpu.resize(device, width, height);
    }
}

/// Creates the intermediate the engine renders a frame into on
/// [`RenderPath::EngineDirectUnpremultiply`], returning its view alone (the
/// view refcounts the texture alive, and nothing on this arm needs the texture
/// handle).
///
/// The usage is exactly what the two consumers need — `RENDER_ATTACHMENT` for
/// the frame's own passes, `TEXTURE_BINDING` for the un-premultiplying pass to
/// sample it — so the arm asks a downlevel adapter for nothing it lacks (this
/// tier has neither a compute stage nor a storage texture anywhere in it; its
/// downlevel design rules refuse both).
///
/// The format is the engine's own off-screen format rather than the surface's:
/// the renderer on this arm is warmed for it (`renderer::engine_target_format`),
/// and only the final pass has to speak the swapchain's format.
fn create_engine_intermediate(width: u32, height: u32, device: &wgpu::Device) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust-render engine intermediate"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        format: frust_engine::gpu::pipelines::INTERMEDIATE_FORMAT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Configures `surface` as an [`EngineSurface`]: resolve the tier, refuse an
/// over-ceiling extent, hand the swapchain to `frust-gpu` to configure, then
/// build the engine resources of whichever arm the resolved alpha mode and the
/// adapter's backend select.
///
/// The device is created (or reused) first, since both the extent ceiling and
/// the arm's own attachments are sized against it.
pub(crate) async fn create_engine_surface(
    ctx: &mut RenderContext,
    surface: wgpu::Surface<'static>,
    width: u32,
    height: u32,
    present_mode: wgpu::PresentMode,
    alpha: SurfaceAlphaRequest,
) -> Result<EngineSurface> {
    ctx.ensure_device(&surface).await?;
    // The capability gate's diagnosis is this crate's single fail-fast message
    // for an adapter the engine cannot drive.
    check_engine_support(&ctx.device_handle().adapter)?;

    // The extent is checked once, up front, for every texture sized against
    // it below (the engine's depth attachment, and its intermediate on the
    // un-premultiplying arm): an over-ceiling surface is refused with a
    // diagnosis naming the limit, rather than handed to wgpu as texture
    // creations it rejects ([`extent_within_limits`]). Asked before the
    // swapchain is configured, so no `wgpu` validation error is raised on the
    // way to the refusal.
    let max_dimension_2d = ctx.device_handle().device.limits().max_texture_dimension_2d;
    if !extent_within_limits(width, height, max_dimension_2d) {
        return Err(anyhow!(
            "frust-render: the engine refuses a {width}x{height} surface — the device's \
             max_texture_dimension_2d is {max_dimension_2d}, and both the swapchain and \
             the depth attachment paired with it are sized from this extent"
        ));
    }

    // `frust-gpu` resolves the alpha mode against the surface's reported caps,
    // picks the format the surface itself prefers, and brings the swapchain up
    // — everything that is true of a swapchain regardless of who renders into
    // it.
    let configured = ctx
        .create_render_surface(surface, width, height, present_mode, alpha)
        .await?;

    let handle = ctx.device_handle();
    // The engine reads the adapter's backend here rather than inside
    // `choose_engine_render_path` itself, so that function stays a pure
    // value decision, host-testable with no live adapter
    // ([`compositor_expects_premultiplied`]).
    let alpha_mode = configured.config().alpha_mode;
    let path_kind = choose_engine_render_path(handle.adapter.get_info().backend, alpha_mode);
    log_render_path(path_kind);
    log_surface_alpha_caps(&configured, handle);

    let format = configured.config().format;
    let path = match path_kind {
        // The engine's direct arm carries exactly one per-surface resource:
        // the depth attachment its opaque pass establishes and its alpha
        // pass tests against, sized to the swapchain that was just
        // configured (the extent was checked against the device ceiling
        // above).
        //
        // `Inherit`/`PreMultiplied` land here correct and untouched, since
        // `frust-engine` already writes premultiplied. A Metal
        // `PostMultiplied` swapchain (iOS included) lands here too, for the
        // identical reason: `compositor_expects_premultiplied` answers true
        // for it, since Metal's own compositor reads that mode's swapchain
        // premultiplied regardless of its name (upstream wgpu-hal truth bug
        // — see that predicate's doc).
        RenderPathKind::EngineDirect => RenderPath::EngineDirect {
            depth: frust_engine::DepthTexture::new(&handle.device, width, height),
        },
        // The opposite combination — a swapchain that stores STRAIGHT
        // alpha AND whose compositor genuinely reads it that way (every
        // `PostMultiplied` backend except Metal, `choose_engine_render_path`'s
        // one exception) — takes the same depth attachment plus the two
        // resources the conversion needs: the intermediate the frame is
        // rendered into, and the pass that un-premultiplies it into the
        // swapchain, built for the swapchain's own format.
        //
        // No persisted driver cache is threaded into that build: a
        // `wgpu::PipelineCache` exists on Vulkan alone, and this arm never
        // serves a Metal surface (Metal's `PostMultiplied` takes the arm
        // above instead) — the one pipeline it compiles is paid once per
        // surface configure either way.
        RenderPathKind::EngineDirectUnpremultiply => RenderPath::EngineDirectUnpremultiply {
            depth: frust_engine::DepthTexture::new(&handle.device, width, height),
            intermediate_view: create_engine_intermediate(width, height, &handle.device),
            present: frust_engine::gpu::present::UnpremultiplyPass::new(
                &handle.device,
                format,
                None,
            ),
        },
    };

    Ok(EngineSurface {
        gpu: configured,
        path,
    })
}

/// [`create_engine_surface`] from anything convertible into a wgpu
/// `SurfaceTarget` (the desktop `winit` window path): the window-handle step
/// runs through `frust-gpu`'s surface factory, then the configured surface
/// takes the shared path above.
pub(crate) async fn create_engine_surface_from_target(
    ctx: &mut RenderContext,
    target: impl Into<wgpu::SurfaceTarget<'static>>,
    width: u32,
    height: u32,
    present_mode: wgpu::PresentMode,
    alpha: SurfaceAlphaRequest,
) -> Result<EngineSurface> {
    let detached = ctx.surface_factory().create_detached_surface(target)?;
    create_engine_surface(
        ctx,
        detached.into_surface(),
        width,
        height,
        present_mode,
        alpha,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_flag_unset_both_halves_is_disabled() {
        assert!(!env_flag_enabled(None, None));
    }

    #[test]
    fn env_flag_compile_time_non_zero_enables() {
        assert!(env_flag_enabled(Some("1"), None));
    }

    #[test]
    fn env_flag_compile_time_zero_is_disabled() {
        assert!(!env_flag_enabled(Some("0"), None));
    }

    #[test]
    fn env_flag_runtime_non_zero_enables() {
        assert!(env_flag_enabled(None, Some("1".to_string())));
    }

    #[test]
    fn env_flag_runtime_zero_is_disabled() {
        assert!(!env_flag_enabled(None, Some("0".to_string())));
    }

    #[test]
    fn env_flag_either_half_set_non_zero_enables() {
        assert!(env_flag_enabled(Some("0"), Some("1".to_string())));
        assert!(env_flag_enabled(Some("1"), Some("0".to_string())));
    }

    /// Every composite alpha mode a surface can resolve to, so a routing claim
    /// is made across the whole space rather than the modes it was written for.
    const EVERY_ALPHA_MODE: [wgpu::CompositeAlphaMode; 5] = [
        wgpu::CompositeAlphaMode::Auto,
        wgpu::CompositeAlphaMode::Opaque,
        wgpu::CompositeAlphaMode::Inherit,
        wgpu::CompositeAlphaMode::PreMultiplied,
        wgpu::CompositeAlphaMode::PostMultiplied,
    ];

    #[test]
    fn every_alpha_mode_routes_to_an_engine_arm() {
        // The completeness claim: this tier serves the whole space, with no
        // (backend, mode) pair left unserveable and no translucency refused.
        // A mode added to `wgpu` later would land on one of the two arms
        // rather than on a refusal, and this is where a wrong answer for it
        // shows up.
        for backend in wgpu::Backend::ALL {
            for mode in EVERY_ALPHA_MODE {
                let kind = choose_engine_render_path(backend, mode);
                assert!(
                    matches!(
                        kind,
                        RenderPathKind::EngineDirect | RenderPathKind::EngineDirectUnpremultiply
                    ),
                    "{backend:?}/{mode:?} routed to {kind:?}, which is not an engine arm"
                );
            }
        }
    }

    #[test]
    fn engine_serves_opaque_and_premultiplied_modes_directly() {
        // Premultiplied output meets a premultiplied-expecting swapchain, and
        // an opaque one ignores alpha entirely: all four are served straight
        // into the swapchain, with no conversion pass in between — on every
        // backend, since `compositor_expects_premultiplied` only ever fires
        // for `PostMultiplied`.
        for backend in wgpu::Backend::ALL {
            for mode in [
                wgpu::CompositeAlphaMode::Auto,
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::Inherit,
                wgpu::CompositeAlphaMode::PreMultiplied,
            ] {
                assert_eq!(
                    choose_engine_render_path(backend, mode),
                    RenderPathKind::EngineDirect,
                    "{mode:?} on {backend:?} took a conversion it does not need"
                );
            }
        }
    }

    #[test]
    fn straight_alpha_translucency_routes_through_the_unpremultiply_arm() {
        // Off Metal, a straight-alpha translucent mode (`PostMultiplied`
        // alone) disagrees with the engine's output convention exactly as
        // advertised, and is the only mode that does.
        for backend in wgpu::Backend::ALL {
            if backend == wgpu::Backend::Metal {
                continue;
            }
            for mode in EVERY_ALPHA_MODE {
                assert_eq!(
                    choose_engine_render_path(backend, mode)
                        == RenderPathKind::EngineDirectUnpremultiply,
                    alpha_mode_is_straight_translucent(mode),
                    "the conversion arm must be exactly the straight-alpha translucent modes on \
                     {backend:?}, not {mode:?}"
                );
            }
        }
    }

    #[test]
    fn metal_post_multiplied_skips_the_unpremultiply_arm() {
        // The upstream wgpu-hal truth bug this predicate corrects for: Metal's
        // `PostMultiplied` composites premultiplied despite advertising
        // straight alpha (wgpu-hal-30.0.1's metal/adapter.rs advertises the
        // mode, metal/surface.rs implements it as nothing beyond
        // `setOpaque(false)`, and `CAMetalLayer` only ever composites
        // premultiplied), so handing it the straight-alpha conversion would
        // double-correct. It must take the same arm as `Inherit`/
        // `PreMultiplied` instead — this also answers iOS, whose sole
        // translucent mode is Metal `PostMultiplied`.
        assert_eq!(
            choose_engine_render_path(
                wgpu::Backend::Metal,
                wgpu::CompositeAlphaMode::PostMultiplied
            ),
            RenderPathKind::EngineDirect,
        );
        // Every other Metal mode is unaffected by the exception — same answer
        // as any other backend.
        for mode in [
            wgpu::CompositeAlphaMode::Auto,
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::Inherit,
            wgpu::CompositeAlphaMode::PreMultiplied,
        ] {
            assert_eq!(
                choose_engine_render_path(wgpu::Backend::Metal, mode),
                RenderPathKind::EngineDirect,
                "{mode:?} on Metal took a conversion it does not need"
            );
        }
    }

    #[test]
    fn non_metal_post_multiplied_keeps_the_unpremultiply_arm() {
        // Genuinely-straight `PostMultiplied` compositors exist off Metal
        // (e.g. Vulkan's own `POST_MULTIPLIED` composite-alpha flag), so the
        // Metal exception above must not spread to any other backend.
        for backend in wgpu::Backend::ALL {
            if backend == wgpu::Backend::Metal {
                continue;
            }
            assert_eq!(
                choose_engine_render_path(backend, wgpu::CompositeAlphaMode::PostMultiplied),
                RenderPathKind::EngineDirectUnpremultiply,
                "{backend:?} PostMultiplied must keep the unpremultiply conversion"
            );
        }
    }

    #[test]
    fn extent_limits_refuse_only_an_over_ceiling_axis() {
        assert!(extent_within_limits(4096, 4096, 4096));
        assert!(extent_within_limits(1, 1, 4096));
        // Either axis alone is enough to refuse.
        assert!(!extent_within_limits(4097, 4096, 4096));
        assert!(!extent_within_limits(4096, 4097, 4096));
    }
}
