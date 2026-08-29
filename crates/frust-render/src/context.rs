//! [`RenderContext`]: owns the wgpu instance and the logical device surfaces
//! render on.
//!
//! Historically this was a thin wrapper over `vello::util::RenderContext`.
//! That type creates its per-surface device internally with a hardcoded
//! `wgpu::Limits::default()` (its `new_device` is private and takes no
//! `required_limits` hook), which the **iOS Simulator's** macOS-Metal-backed
//! device cannot satisfy — `request_device` fails and vello surfaces it as
//! `NoCompatibleDevice` ("Couldn't find suitable device"). We therefore own
//! adapter/device creation ourselves (requesting the adapter's *actual* limits
//! plus the [wgpu #7057](https://github.com/gfx-rs/wgpu/issues/7057)
//! simulator alignment mitigation) and reuse vello only for the surface
//! plumbing (`vello::util::RenderSurface`, the blitter, and the `Renderer`),
//! which all accept a plain `wgpu::Device`.
//!
//! Surface creation lives on the lifecycle state machine in
//! [`crate::SurfaceRenderer`]: the shell mints an empty
//! `SurfaceRenderer` and drives it with `on_surface_created`/`on_surface_changed`/
//! `on_surface_destroyed`, each of which reaches back into this context for the
//! owning device.

use anyhow::{Result, anyhow};
use kurbo::Affine;
use std::sync::OnceLock;
use wgpu::util::TextureBlitter;

use crate::renderer::SurfaceAlphaRequest;

/// The features vello's renderer opportunistically uses when the adapter
/// exposes them — mirrors `vello::util::RenderContext::new_device` so our
/// hand-rolled device request stays behaviourally identical to vello's, apart
/// from the `required_limits` we control.
pub(crate) fn vello_optional_features() -> wgpu::Features {
    wgpu::Features::CLEAR_TEXTURE | wgpu::Features::PIPELINE_CACHE
}

/// Whether a `FRUST_*` boolean env flag is set to a non-zero value, checking
/// both the compile-time (`option_env!`) and runtime (`std::env::var`) halves —
/// the compile-time-or-runtime parsing every shipping flag (`FRUST_TRACE`,
/// `FRUST_NO_FRAME_GATE`, `FRUST_NO_RENDER_THREAD`, …) uses, so an Android app
/// process (which has no runtime env) still honours a baked-in value.
fn env_flag_enabled(name_compile_time: Option<&str>, name_runtime: Option<String>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if v != "0")
    }
    is_set_non_zero(name_compile_time) || is_set_non_zero(name_runtime.as_deref())
}

/// The precedence-resolved value of a `FRUST_*` string-valued env knob (e.g.
/// `FRUST_AA_MODE`), checking both the compile-time (`option_env!`) and
/// runtime (`std::env::var`) halves like [`env_flag_enabled`], but returning
/// the value itself rather than a bool.
///
/// **Runtime wins**: a non-empty runtime value is returned even when a
/// compile-time value is also set; an empty (`""`) runtime value is treated
/// as unset and falls through to the compile-time half; `None` when neither
/// half carries a non-empty value. Same spirit as `env_flag_enabled`'s
/// compile-time-or-runtime parsing, so an Android app process (no runtime
/// env) still honours a baked-in value.
pub(crate) fn env_str(
    compile_time: Option<&'static str>,
    runtime: Option<String>,
) -> Option<String> {
    fn non_empty(value: Option<String>) -> Option<String> {
        value.filter(|v| !v.is_empty())
    }
    non_empty(runtime).or_else(|| non_empty(compile_time.map(str::to_string)))
}

/// vello's per-frame anti-aliasing method, selectable via the `FRUST_AA_MODE`
/// knob (`docs/RENDER_DEVELOPMENT.md` § Instrumentation (render path)) so the
/// Area-vs-MSAA cost of the fine stage can be A/B'd on device. Crate-private:
/// the `vello` type it wraps ([`vello::AaConfig`]/[`vello::AaSupport`]) never
/// crosses this crate's boundary (`docs/CODE_STANDARDS.md`'s wgpu-leak
/// anti-pattern).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AaMode {
    /// vello's default: area anti-aliasing. Byte-identical to pre-knob
    /// behaviour, and the fallback for an unset/empty/unknown knob value.
    Area,
    /// 8x multisampling.
    Msaa8,
    /// 16x multisampling.
    Msaa16,
}

impl std::fmt::Display for AaMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AaMode::Area => "area",
            AaMode::Msaa8 => "msaa8",
            AaMode::Msaa16 => "msaa16",
        })
    }
}

impl AaMode {
    /// The [`vello::AaConfig`] this mode requests from `RenderParams`.
    pub(crate) fn to_vello(self) -> vello::AaConfig {
        match self {
            AaMode::Area => vello::AaConfig::Area,
            AaMode::Msaa8 => vello::AaConfig::Msaa8,
            AaMode::Msaa16 => vello::AaConfig::Msaa16,
        }
    }

    /// The [`vello::AaSupport`] a `vello::Renderer` should be built with to
    /// compile pipelines for exactly this mode — never more: `AaSupport::all()`
    /// compiles shader permutations for every `AaConfig`, ~3x unnecessary
    /// pipeline compiles at init (the same rationale `install_surface`'s
    /// `AaSupport::area_only()` documents), so only the selected mode's bit is
    /// ever set.
    pub(crate) fn support(self) -> vello::AaSupport {
        match self {
            AaMode::Area => vello::AaSupport::area_only(),
            AaMode::Msaa8 => vello::AaSupport {
                area: false,
                msaa8: true,
                msaa16: false,
            },
            AaMode::Msaa16 => vello::AaSupport {
                area: false,
                msaa8: false,
                msaa16: true,
            },
        }
    }
}

/// Parses a raw `FRUST_AA_MODE` value (already resolved by [`env_str`]) into
/// an [`AaMode`]. Case-insensitive on `area`/`msaa8`/`msaa16`, with
/// surrounding whitespace trimmed first. An unset or empty value falls back
/// to [`AaMode::Area`] silently (that is simply "the knob wasn't touched");
/// an unrecognised value also falls back to `Area`, but logs one
/// `log::warn!` naming the offending value, since that case is more likely a
/// typo than a deliberate default.
pub(crate) fn parse_aa_mode(raw: Option<String>) -> AaMode {
    let Some(trimmed) = raw.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
        return AaMode::Area;
    };
    match trimmed.to_ascii_lowercase().as_str() {
        "area" => AaMode::Area,
        "msaa8" => AaMode::Msaa8,
        "msaa16" => AaMode::Msaa16,
        _ => {
            log::warn!(
                "frust-render: unrecognised FRUST_AA_MODE value {trimmed:?}, falling back to area"
            );
            AaMode::Area
        }
    }
}

/// The process-wide anti-aliasing mode, resolved once from `FRUST_AA_MODE`
/// (compile-time-or-runtime, see [`env_str`]) and cached — a `FRUST_AA_MODE`
/// change requires a fresh process, matching every other `FRUST_*` knob.
pub(crate) fn aa_mode() -> AaMode {
    static MODE: OnceLock<AaMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        parse_aa_mode(env_str(
            option_env!("FRUST_AA_MODE"),
            std::env::var("FRUST_AA_MODE").ok(),
        ))
    })
}

/// The narrowest fraction of the surface `FRUST_RENDER_SCALE` will render at.
/// Below a quarter of each axis the frame is too coarse for the upscaled
/// result to be judged against the real one at all — a measurement instrument
/// still has to show what it measures.
const MIN_RENDER_SCALE: f64 = 0.25;

/// The widest value `FRUST_RENDER_SCALE` accepts: the surface's own
/// resolution, which is the default and the byte-identical-to-untouched case.
/// Rendering ABOVE the surface size is a different feature (supersampling) with
/// its own cost profile, deliberately out of this knob's range.
const MAX_RENDER_SCALE: f64 = 1.0;

/// Parses a raw `FRUST_RENDER_SCALE` value (already resolved by [`env_str`])
/// into the fraction of the surface's pixel size each frame renders at.
///
/// An unset or empty value is [`MAX_RENDER_SCALE`] (1.0) silently — that is
/// simply "the knob wasn't touched". An unparsable or non-finite value
/// (`abc`, `NaN`, `inf`) also falls back to 1.0 but logs one `log::warn!`
/// naming it, since that is a typo rather than a deliberate default. A value
/// outside `MIN_RENDER_SCALE..=MAX_RENDER_SCALE` is clamped into it, likewise
/// with one warn: `2` means "the knob was misunderstood", and silently
/// honouring it would present a supersampled frame nobody asked to pay for.
pub(crate) fn parse_render_scale(raw: Option<String>) -> f64 {
    let Some(trimmed) = raw.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
        return MAX_RENDER_SCALE;
    };
    let Ok(parsed) = trimmed.parse::<f64>() else {
        log::warn!(
            "frust-render: unparsable FRUST_RENDER_SCALE value {trimmed:?}, rendering at full \
             surface resolution"
        );
        return MAX_RENDER_SCALE;
    };
    // NaN fails every comparison, so it must be rejected before the clamp
    // rather than by it (`f64::clamp` panics on a NaN bound and returns NaN
    // for a NaN value); infinities are equally meaningless as a fraction.
    if !parsed.is_finite() {
        log::warn!(
            "frust-render: non-finite FRUST_RENDER_SCALE value {trimmed:?}, rendering at full \
             surface resolution"
        );
        return MAX_RENDER_SCALE;
    }
    let clamped = parsed.clamp(MIN_RENDER_SCALE, MAX_RENDER_SCALE);
    if clamped != parsed {
        log::warn!(
            "frust-render: FRUST_RENDER_SCALE {parsed} out of range \
             {MIN_RENDER_SCALE}..={MAX_RENDER_SCALE}, clamped to {clamped}"
        );
    }
    clamped
}

/// The process-wide render scale, resolved once from `FRUST_RENDER_SCALE`
/// (compile-time-or-runtime, see [`env_str`]) and cached — a change requires a
/// fresh process, matching every other `FRUST_*` knob.
///
/// The knob renders the whole frame into an intermediate a fraction of the
/// surface's size and upscales it in the blit pass, so the cost per pixel of
/// vello's fine stage (which is proportional to the pixels it sweeps) can be
/// measured on device. It is a measurement instrument, not a shipping mode:
/// while it is active the surface is pinned onto the blit arm and the
/// snapshot-layer cache is refused outright (see
/// [`crate::renderer::snapshot_cache_enabled`]).
pub(crate) fn render_scale() -> f64 {
    static SCALE: OnceLock<f64> = OnceLock::new();
    *SCALE.get_or_init(|| {
        parse_render_scale(env_str(
            option_env!("FRUST_RENDER_SCALE"),
            std::env::var("FRUST_RENDER_SCALE").ok(),
        ))
    })
}

/// Whether this process was ASKED to render below the surface's own
/// resolution — the raw knob value, read only where a surface is being
/// configured ([`RenderContext::create_render_surface`]'s forced-blit
/// decision, which has no surface to ask yet).
///
/// Once a surface exists, "is this frame scaled?" is answered by the surface
/// itself ([`ConfiguredSurface::render_scaled`]) and the root it carries
/// ([`blit_root`], stored on [`RenderPath::Blit`]): both are derived from the
/// intermediate's REAL size, so they stay true for a CPU-tier surface (pinned
/// to full resolution whatever the knob says) and for the ceil'd target the
/// knob actually produces. Deriving them a second time from this process
/// global is how two answers about one frame start to disagree.
pub(crate) fn render_scaled() -> bool {
    render_scale() < MAX_RENDER_SCALE
}

/// The intermediate-target size for a `width` x `height` surface rendered at
/// `scale`: each axis rounded UP (a truncated axis would leave the frame's
/// last row/column unsampled by the upscaling blit) and floored at one texel
/// (wgpu rejects a zero-sized texture).
///
/// Identity at `scale == 1.0` — every `u32` is exactly representable as `f64`,
/// so `ceil(w * 1.0) == w` with no rounding step — which is what keeps the
/// default path byte-identical to a build without the knob.
pub(crate) fn scaled_size(width: u32, height: u32, scale: f64) -> (u32, u32) {
    // `f64 as u32` saturates at both ends in Rust, so neither a huge product
    // nor a negative one can wrap; `scale` reaches here already clamped to
    // `MIN_RENDER_SCALE..=MAX_RENDER_SCALE` anyway.
    let axis = |value: u32| ((f64::from(value) * scale).ceil() as u32).max(1);
    (axis(width), axis(height))
}

/// The root transform every vello pass targeting a blit arm's intermediate
/// encodes under: the exact mapping of the `surface`'s own coordinate space
/// onto the `target` the frame is rendered into.
///
/// [`Affine::IDENTITY`] whenever the intermediate is the surface's own size —
/// the unscaled default, which is what keeps that path byte-identical to a
/// build the render-scale knob never touched — and otherwise the per-axis
/// ratio `target / surface`.
///
/// Per-axis, and derived from the target's size rather than from the scale
/// that produced it, because [`scaled_size`] rounds each axis UP: a 1179-wide
/// surface at 0.75 takes an 885-wide intermediate (885/1179 ≈ 0.7506), so a
/// uniform `scale(0.75)` root would stop short of the intermediate's last
/// column and the upscaling blit would stretch that unpainted strip of base
/// colour back over the frame's edge. Scaling by the ratio the target actually
/// has maps the frame exactly onto it, whichever axis rounded.
///
/// `surface` is a live swapchain size, never zero on either axis (zero
/// dimensions are rejected before a surface is configured or resized).
pub(crate) fn blit_root(surface: (u32, u32), target: (u32, u32)) -> Affine {
    if surface == target {
        return Affine::IDENTITY;
    }
    Affine::scale_non_uniform(
        f64::from(target.0) / f64::from(surface.0),
        f64::from(target.1) / f64::from(surface.1),
    )
}

/// Whether a surface of `surface` size renders BELOW its own resolution, given
/// the intermediate it renders into (`None` on the direct arms, which have no
/// intermediate and are therefore never scaled — a scale < 1 forces the blit
/// arm, see [`RenderContext::create_render_surface`]).
///
/// The pure core of [`ConfiguredSurface::render_scaled`], and the same
/// comparison [`blit_root`] turns into a transform: a blit intermediate is the
/// surface's own size unless [`scaled_size`] shrank it, so "a different size"
/// IS "scaled".
fn path_render_scaled(surface: (u32, u32), blit_target: Option<(u32, u32)>) -> bool {
    matches!(blit_target, Some(target) if target != surface)
}

/// The sampler filter the blit pass upscales with: linear while the
/// intermediate is smaller than the surface, nearest otherwise.
///
/// `TextureBlitter::copy` is a sampled full-screen triangle draw over the
/// whole destination (verified in wgpu 29.0.4's `util/texture_blitter.rs` +
/// `blit.wgsl`: normalized tex coords, `textureSample`), so source and
/// destination sizes may differ and the sampler's `mag_filter` is what
/// resolves the difference — nearest would show the reduced frame as visible
/// blocks rather than as the softened image an upscale is meant to be.
/// `TextureBlitter::new` builds with `FilterMode::Nearest`, so the unscaled
/// answer here is byte-identical to the blitter the default path had before
/// this knob existed.
pub(crate) fn blit_filter(scaled: bool) -> wgpu::FilterMode {
    if scaled {
        wgpu::FilterMode::Linear
    } else {
        wgpu::FilterMode::Nearest
    }
}

/// Whether perf tracing (frust-perf logging) is enabled via the process-wide
/// `FRUST_TRACE` flag — mirroring the check in `frust-shell-common::perf`.
/// Cached to avoid repeated environment lookups.
///
/// Only compiled under the `perf-trace` feature — the sole callers,
/// [`probe_direct_to_surface_capability`]/
/// [`log_render_path`], are themselves feature-gated with an inert
/// `#[cfg(not(feature = "perf-trace"))]` counterpart that skips this check
/// entirely, so a build without the feature contains neither this env read
/// nor the probe bodies it guards.
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

/// Whether the direct-to-surface render path is force-disabled via the
/// process-wide `FRUST_NO_DIRECT_SURFACE` flag — the fallback-proof safety valve
/// that pins a capable device onto the blit arm so the two arms
/// can be A/B'd on the same hardware. Same compile-time-or-runtime parsing as
/// `FRUST_TRACE`/`FRUST_NO_RENDER_THREAD` (see `docs/RENDER_DEVELOPMENT.md`
/// § Instrumentation (render path)). Cached: read once per process.
fn direct_surface_force_blit() -> bool {
    static FORCED: OnceLock<bool> = OnceLock::new();
    *FORCED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_NO_DIRECT_SURFACE"),
            std::env::var("FRUST_NO_DIRECT_SURFACE").ok(),
        )
    })
}

/// Which per-frame render path a configured surface uses (see [`RenderPath`]).
///
/// Pure decision output, kept separate from the `wgpu` resources so the
/// selection logic ([`choose_render_path`]) is unit-testable without a GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderPathKind {
    /// vello renders straight into the acquired swapchain texture
    /// (`Rgba8Unorm` + `STORAGE_BINDING`); no intermediate texture, no blit.
    Direct,
    /// vello renders into an intermediate `Rgba8Unorm` target that is blitted to
    /// the swapchain each frame — the fallback for probe-refused/`Bgra8`-only
    /// surfaces and the `cpu-tier` path.
    Blit,
    /// `vello_hybrid` renders into the acquired swapchain texture, which needs
    /// only `RENDER_ATTACHMENT` in whatever format the surface reports — no
    /// `Rgba8Unorm` requirement, no `STORAGE_BINDING`, no intermediate and no
    /// blit. Chosen by [`choose_hybrid_render_path`] for the
    /// [`Hybrid`](crate::RenderTier::Hybrid) tier alone, which is why neither
    /// the direct-to-surface probe nor `force_blit` participates.
    #[cfg(feature = "hybrid-tier")]
    HybridDirect,
}

/// Pure direct-to-surface path policy: choose [`Direct`] only
/// when the surface advertises **both** `Rgba8Unorm` and `STORAGE_BINDING` (the
/// exact requirements of vello 0.9's `render_to_texture` target) **and** the
/// blit arm is not force-selected; otherwise [`Blit`].
///
/// `force_blit` folds together the `FRUST_NO_DIRECT_SURFACE` safety valve and
/// the `cpu-tier`-selected case (the CPU tier uploads its pixmap into the
/// intermediate target, so it is blit-only by construction) — both resolved at
/// the call site. Host-testable: no `wgpu` state is touched here.
///
/// [`Direct`]: RenderPathKind::Direct
/// [`Blit`]: RenderPathKind::Blit
pub(crate) fn choose_render_path(
    has_rgba8unorm: bool,
    has_storage_binding: bool,
    force_blit: bool,
) -> RenderPathKind {
    if !force_blit && has_rgba8unorm && has_storage_binding {
        RenderPathKind::Direct
    } else {
        RenderPathKind::Blit
    }
}

/// The hybrid tier's render path: always [`RenderPathKind::HybridDirect`],
/// never [`RenderPathKind::Blit`].
///
/// A sibling of [`choose_render_path`] rather than a branch inside it because
/// it shares none of its inputs: `vello_hybrid` targets an ordinary render
/// attachment in the surface's own format, so the `Rgba8Unorm`/
/// `STORAGE_BINDING` probe that decides vello's arm has nothing to say here,
/// and the `force_blit` valve has no blit arm to force it onto — this tier
/// owns no intermediate to blit from. `FRUST_NO_DIRECT_SURFACE` and
/// `FRUST_RENDER_SCALE` are consequently inert on it (both are instruments of
/// vello's own two arms), which is the reason this function takes no
/// arguments at all: there is nothing that could change its answer.
#[cfg(feature = "hybrid-tier")]
pub(crate) fn choose_hybrid_render_path() -> RenderPathKind {
    RenderPathKind::HybridDirect
}

/// Whether `tier` can only ever present through the blit arm — one exhaustive
/// answer per tier, feeding [`choose_render_path`]'s `force_blit`.
///
/// Exhaustive on purpose. The expression this replaced read
/// `selected_tier() != RenderTier::Gpu`, which silently forced the blit arm
/// (and a `TextureBlitter`, and an intermediate) onto every tier added after
/// it — a trap a new tier springs by existing, with no compile error. Written
/// as a match, a new variant cannot compile until it has stated its own
/// answer here.
pub(crate) fn tier_forces_blit(tier: crate::tier::RenderTier) -> bool {
    match tier {
        // Probed per surface: direct where the swapchain supports vello's
        // target, blit otherwise.
        crate::tier::RenderTier::Gpu => false,
        // Uploads its rasterized pixmap into the intermediate target the blit
        // reads from, so it is blit-only by construction.
        crate::tier::RenderTier::Cpu => true,
        // Renders into the acquired swapchain view itself
        // ([`choose_hybrid_render_path`]); an intermediate plus a blit would
        // be a pass it never asked for.
        crate::tier::RenderTier::Hybrid => false,
    }
}

/// Whether THIS build actually contains `tier`'s renderer, i.e. whether the
/// non-default cargo feature that compiles it in is on.
///
/// The guard [`RenderContext::create_device`] fails fast with, so a
/// [`crate::SurfaceRenderer`] never receives a tier it has no encode path for.
/// Exhaustive for [`tier_forces_blit`]'s reason: the `!= RenderTier::Gpu`
/// expression this replaced refused every future tier by default, which would
/// have made a correctly compiled-in tier unreachable for no stated reason.
pub(crate) fn tier_compiled_in(tier: crate::tier::RenderTier) -> bool {
    match tier {
        crate::tier::RenderTier::Gpu => true,
        crate::tier::RenderTier::Cpu => cfg!(feature = "cpu-tier"),
        crate::tier::RenderTier::Hybrid => cfg!(feature = "hybrid-tier"),
    }
}

/// Whether the shader-showcase fragment-shader pre-pass
/// (`crate::renderer::run_shader_prepass`) is force-disabled via the
/// process-wide `FRUST_NO_SHADER_EFFECTS` flag — the same
/// unproven-render-path safety valve shape as
/// [`direct_surface_force_blit`]: the pre-pass drives real GPU work
/// (pipeline compiles, offscreen targets, a vello image-override
/// registration) that is unproven on device, so this flag is the fallback-proof escape hatch that
/// reverts every `Command::ShaderQuad` to the existing placeholder-fill
/// lowering (`convert::encode_range_with_overrides`'s miss path) with zero
/// pre-pass GPU work. Same compile-time-or-runtime parsing as
/// `FRUST_TRACE`/`FRUST_NO_DIRECT_SURFACE` (see `docs/RENDER_DEVELOPMENT.md`
/// § Instrumentation (render path)). Cached: read once per process.
pub(crate) fn shader_effects_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_NO_SHADER_EFFECTS"),
            std::env::var("FRUST_NO_SHADER_EFFECTS").ok(),
        )
    })
}

/// Whether the snapshot-layer cache (`crate::snapshot::SnapshotCache`) is
/// force-disabled via the process-wide `FRUST_NO_SNAPSHOT_LAYERS` flag — the
/// same unproven-render-path safety valve shape as
/// [`shader_effects_disabled`]: the snapshot pre-pass drives real GPU work (an
/// offscreen texture per cached subtree, an extra `render_to_texture` whenever
/// a body changes, and — through [`crate::compositor`] — a quad pass and a
/// possible second vello pass per composited frame), so this flag is the
/// fallback-proof escape hatch that reverts every `Command::PushSnapshot` to
/// the inline emulation the encode walk performed before the cache existed
/// (`convert::encode_range_with_overrides`'s miss path), with zero pre-pass
/// GPU work: a disabled cache yields an empty frame plan, so the compositor
/// pass never runs and each arm is byte-identical to its pre-cache self. Same
/// compile-time-or-runtime parsing as
/// `FRUST_TRACE`/`FRUST_NO_DIRECT_SURFACE` (see `docs/RENDER_DEVELOPMENT.md`
/// § Instrumentation (render path)). Cached: read once per process.
pub(crate) fn snapshot_layers_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_NO_SNAPSHOT_LAYERS"),
            std::env::var("FRUST_NO_SNAPSHOT_LAYERS").ok(),
        )
    })
}

/// The two direct-to-surface capability bits, read once from a surface's
/// [`wgpu::SurfaceCapabilities`]. Shared by the capability probe log and the
/// [`choose_render_path`] decision so the query is not duplicated.
fn direct_surface_caps(capabilities: &wgpu::SurfaceCapabilities) -> (bool, bool) {
    let has_rgba8unorm = capabilities
        .formats
        .contains(&wgpu::TextureFormat::Rgba8Unorm);
    let has_storage_binding = capabilities
        .usages
        .contains(wgpu::TextureUsages::STORAGE_BINDING);
    (has_rgba8unorm, has_storage_binding)
}

/// Probes the surface's direct-to-surface capability and logs the result
/// (if perf tracing is enabled). Logs once per process.
///
/// Direct-to-surface rendering requires:
/// - Rgba8Unorm format in the surface's supported formats
/// - STORAGE_BINDING usage in the surface's supported usages
///
/// Only compiled under the `perf-trace` feature; see the inert
/// `#[cfg(not(feature = "perf-trace"))]` counterpart
/// below.
#[cfg(feature = "perf-trace")]
fn probe_direct_to_surface_capability(capabilities: &wgpu::SurfaceCapabilities) {
    static LOGGED: OnceLock<()> = OnceLock::new();

    if !perf_tracing_enabled() {
        return;
    }

    LOGGED.get_or_init(|| {
        // Format the supported formats as a comma-separated list
        let formats_str = if capabilities.formats.is_empty() {
            "[]".to_string()
        } else {
            format!(
                "[{}]",
                capabilities
                    .formats
                    .iter()
                    .map(|f| format!("{:?}", f))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };

        // Reuse the shared capability read so the probe verdict and the live
        // path decision (`choose_render_path`) can never disagree.
        let (has_rgba8unorm, has_storage_binding) = direct_surface_caps(capabilities);

        // Determine the verdict
        let direct_to_surface_supported = has_rgba8unorm && has_storage_binding;
        let verdict = if direct_to_surface_supported {
            "YES"
        } else {
            "NO"
        };

        // Provide a reason for the verdict
        let reason = if direct_to_surface_supported {
            "Rgba8Unorm+STORAGE_BINDING".to_string()
        } else {
            let mut missing = Vec::new();
            if !has_rgba8unorm {
                missing.push("no Rgba8Unorm");
            }
            if !has_storage_binding {
                missing.push("no STORAGE_BINDING");
            }
            missing.join(", ")
        };

        // Log the surface capabilities probe
        log::info!(
            "frust-perf surface-caps formats={} usages={:?} direct_to_surface={} ({})",
            formats_str,
            capabilities.usages,
            verdict,
            reason
        );
    });
}

/// Without the `perf-trace` feature, the surface-caps probe is a complete
/// no-op — no logging, no format-string bodies compiled in.
#[cfg(not(feature = "perf-trace"))]
#[inline]
fn probe_direct_to_surface_capability(_capabilities: &wgpu::SurfaceCapabilities) {}

/// Emits the one-per-process startup line naming the chosen render path and the
/// reason, mirroring the surface-caps probe line style and its
/// `FRUST_TRACE` gating. Logged once regardless of surface recreation.
///
/// Only compiled under the `perf-trace` feature; see the inert
/// `#[cfg(not(feature = "perf-trace"))]` counterpart
/// below.
#[cfg(feature = "perf-trace")]
fn log_render_path(
    path: RenderPathKind,
    has_rgba8unorm: bool,
    has_storage_binding: bool,
    force_blit: bool,
) {
    static LOGGED: OnceLock<()> = OnceLock::new();

    if !perf_tracing_enabled() {
        return;
    }

    LOGGED.get_or_init(|| {
        let (name, reason) = match path {
            RenderPathKind::Direct => ("direct", "Rgba8Unorm+STORAGE_BINDING".to_string()),
            RenderPathKind::Blit => {
                let reason = if force_blit {
                    // The safety valve wins even on a capable surface — say so, so
                    // an A/B run's log confirms the arm it is actually exercising.
                    // `FRUST_RENDER_SCALE < 1` is the third cause: the scaled
                    // intermediate IS the blit arm's target, so the knob pins
                    // the surface here exactly like the kill switch does.
                    "forced (FRUST_NO_DIRECT_SURFACE, FRUST_RENDER_SCALE < 1, or cpu-tier)"
                        .to_string()
                } else {
                    let mut missing = Vec::new();
                    if !has_rgba8unorm {
                        missing.push("no Rgba8Unorm");
                    }
                    if !has_storage_binding {
                        missing.push("no STORAGE_BINDING");
                    }
                    missing.join(", ")
                };
                ("blit", reason)
            }
            #[cfg(feature = "hybrid-tier")]
            RenderPathKind::HybridDirect => (
                "hybrid-direct",
                "vello_hybrid into the acquired swapchain view (RENDER_ATTACHMENT, \
                 surface-reported format)"
                    .to_string(),
            ),
        };
        log::info!("frust-perf render-path {name} ({reason})");
    });
}

/// Without the `perf-trace` feature, the render-path startup line is a
/// complete no-op — no logging, no format-string bodies compiled in.
#[cfg(not(feature = "perf-trace"))]
#[inline]
fn log_render_path(
    _path: RenderPathKind,
    _has_rgba8unorm: bool,
    _has_storage_binding: bool,
    _force_blit: bool,
) {
}

/// Emits the one-per-process line naming the effective render scale and the
/// intermediate size it implies for a `width` x `height` surface, so a device
/// capture proves which resolution the frame it is timing was rendered at.
///
/// Unlike the two probe lines above this is NOT behind the `perf-trace`
/// feature, matching the `frust-render aa-mode=` line the AA knob logs: one
/// formatted line per process, available in any build that can be handed the
/// knob. `scale` is the context's effective render scale (pinned to
/// [`MAX_RENDER_SCALE`] on the CPU tier — see [`RenderContext::effective_render_scale`]),
/// so the size reported is the one the blit arm's intermediate actually
/// takes; on an unscaled surface that is the surface's own size, whichever
/// arm it ends up on (scale < 1 always forces the blit arm).
///
/// A scale below full resolution ALSO emits one `log::warn!`: the knob rewires
/// the render path (forced blit, snapshot cache off) and a capture taken
/// hours later should not have to infer that from an `info` line — a default
/// build logs nothing extra.
fn log_render_scale(width: u32, height: u32, scale: f64) {
    static LOGGED: OnceLock<()> = OnceLock::new();
    LOGGED.get_or_init(|| {
        let (target_width, target_height) = scaled_size(width, height, scale);
        log::info!("frust-render render-scale={scale} blit-target={target_width}x{target_height}");
        if scale < MAX_RENDER_SCALE {
            log::warn!(
                "frust-render measurement knob in effect: render-scale={scale} \
                 (blit forced, snapshot cache off)"
            );
        }
    });
}

/// Resolves a caller's [`SurfaceAlphaRequest`] against the live surface's
/// reported `alpha_modes`, choosing the actual `wgpu::CompositeAlphaMode` to
/// configure with. Kept crate-private and
/// `wgpu`-typed: `SurfaceAlphaRequest` is the public, `wgpu`-free seam; the
/// resolved mode itself never crosses `frust-render`'s boundary (the
/// `DetachedSurface` opacity precedent — `docs/CODE_STANDARDS.md`'s
/// wgpu-leak anti-pattern).
///
/// `Opaque` reproduces today's behavior bit-for-bit (`Auto`, unchanged for
/// every existing caller). `TranslucentPreferred` tries, in order, `Inherit`
/// (Android's only reported translucent mode), `PostMultiplied`
/// (iOS's translucent mode), then `PreMultiplied` — falling back to `Auto`
/// with a `log::warn!` when none of the three is in `capabilities.alpha_modes`
/// (translucency silently unavailable on that surface).
fn resolve_alpha_mode(
    request: SurfaceAlphaRequest,
    capabilities: &wgpu::SurfaceCapabilities,
) -> wgpu::CompositeAlphaMode {
    match request {
        SurfaceAlphaRequest::Opaque => wgpu::CompositeAlphaMode::Auto,
        SurfaceAlphaRequest::TranslucentPreferred => {
            const PREFERRED: [wgpu::CompositeAlphaMode; 3] = [
                wgpu::CompositeAlphaMode::Inherit,
                wgpu::CompositeAlphaMode::PostMultiplied,
                wgpu::CompositeAlphaMode::PreMultiplied,
            ];
            PREFERRED
                .into_iter()
                .find(|mode| capabilities.alpha_modes.contains(mode))
                .unwrap_or_else(|| {
                    log::warn!(
                        "frust-render: translucency requested but unavailable \
                         (alpha_modes={:?}) — falling back to Auto (opaque)",
                        capabilities.alpha_modes
                    );
                    wgpu::CompositeAlphaMode::Auto
                })
        }
    }
}

/// Whether a **resolved** `wgpu::CompositeAlphaMode` actually composites the
/// surface's alpha against what is behind it — i.e. whether the surface really
/// came up translucent ("Mode B"), as opposed to what the caller *requested*.
///
/// This is the wgpu-free truth behind [`ConfiguredSurface::resolved_translucent`]
/// and, through it,
/// [`SurfaceRenderer::surface_resolved_translucent`](crate::SurfaceRenderer::surface_resolved_translucent):
/// [`resolve_alpha_mode`] can silently degrade a
/// [`SurfaceAlphaRequest::TranslucentPreferred`] to `Auto` when the platform
/// advertises no translucent mode, and a shell that kept keying its paint
/// contract off the *request* would then clear to `TRANSPARENT` and punch its
/// platform-view slots via `DestOut` against an OPAQUE swapchain — presenting
/// black rectangles. Keying off this instead degrades to
/// the Mode A contract (opaque base, no punch).
///
/// The three translucent modes are exactly [`resolve_alpha_mode`]'s preference
/// list — `Inherit` (Android), `PostMultiplied` (iOS), `PreMultiplied`;
/// `Opaque`/`Auto` ignore the surface's alpha entirely and are therefore *not*
/// translucent (`Auto` is what every opaque caller and every fallback resolves
/// to).
fn alpha_mode_is_translucent(mode: wgpu::CompositeAlphaMode) -> bool {
    matches!(
        mode,
        wgpu::CompositeAlphaMode::Inherit
            | wgpu::CompositeAlphaMode::PostMultiplied
            | wgpu::CompositeAlphaMode::PreMultiplied
    )
}

/// Whether pixels presented to a swapchain configured with this composite
/// alpha mode must be **premultiplied** before present.
///
/// vello 0.9's fine stage accumulates in premultiplied space but
/// un-premultiplies at its final write — `rgba_sep = vec4(fg.rgb * (1/fg.a),
/// fg.a)` in `vello_shaders-0.9.0/shader/fine.wgsl` (the `textureStore` at
/// line ~1394) — so `render_to_texture`'s output carries **straight**
/// (non-premultiplied) alpha. A compositor that expects premultiplied alpha
/// then over-brightens every partial-alpha pixel by `1/a`. The two
/// premultiplied-expecting modes are `PreMultiplied` and — the shipped Android
/// translucent case — `Inherit`: on Android the only reported translucent
/// mode is `Inherit`, under which SurfaceFlinger blends a `TRANSLUCENT`
/// SurfaceView premultiplied — measured on device: a 50%-alpha
/// `#FFF176` reached SurfaceFlinger stored straight `#FFF176@128` instead of
/// premultiplied `#807B3B@128`, compositing over-bright over a Mode B hole.
///
/// `PostMultiplied` (iOS's translucent mode) expects straight alpha — vello's
/// output is already correct there — and `Opaque`/`Auto` ignore alpha
/// entirely, so all three take vello's output unchanged. See
/// [`PremultiplyPass`], the direct-path fix this gates.
fn alpha_mode_needs_premultiply(mode: wgpu::CompositeAlphaMode) -> bool {
    matches!(
        mode,
        wgpu::CompositeAlphaMode::PreMultiplied | wgpu::CompositeAlphaMode::Inherit
    )
}

/// Whether a **resolved** alpha mode means "translucent, and the swapchain
/// stores STRAIGHT alpha" — the one combination the snapshot compositor has no
/// exact arithmetic for.
///
/// True for `PostMultiplied` alone: it is translucent
/// ([`alpha_mode_is_translucent`]) and expects straight alpha
/// ([`alpha_mode_needs_premultiply`] is false), which is iOS's translucent
/// mode. `Inherit`/`PreMultiplied` are translucent but premultiplied, so they
/// take [`RenderPath::DirectPremultiplied`] and the compositor's premultiplied
/// arm; `Opaque`/`Auto` ignore alpha entirely, which is the destination the
/// straight arm's blend is exact for.
///
/// Pure and mode-only, so the decision is host-testable without a surface; the
/// live per-surface answer is
/// [`ConfiguredSurface::straight_alpha_translucent`], which additionally
/// respects [`blit_translucency_refused`].
fn alpha_mode_is_straight_translucent(mode: wgpu::CompositeAlphaMode) -> bool {
    alpha_mode_is_translucent(mode) && !alpha_mode_needs_premultiply(mode)
}

/// Whether a configured surface must **refuse** translucency because its
/// chosen render path cannot deliver the premultiplied output the resolved
/// alpha mode expects (following on the resolved-translucency seam above).
///
/// [`RenderPathKind::Blit`]'s `TextureBlitter::copy` is a sampled full-screen
/// draw — a fixed pass-through fragment shader that samples the source and
/// writes it out unchanged (wgpu's `util/blit.wgsl`), so it can rescale a
/// source of a different size ([`scaled_size`]/[`blit_filter`]) but offers
/// **no stage frust can premultiply in**, unlike the
/// [`RenderPathKind::Direct`] arm's [`RenderPath::DirectPremultiplied`]
/// compute pass. So a **GPU-tier** surface forced onto the blit arm (no
/// `Rgba8Unorm`+`STORAGE_BINDING`, `FRUST_NO_DIRECT_SURFACE`, or
/// `FRUST_RENDER_SCALE < 1`) that
/// resolves a premultiplied-expecting alpha mode (`Inherit`/`PreMultiplied`,
/// [`alpha_mode_needs_premultiply`]) would feed vello's straight-alpha blit
/// output straight to a premultiplied-expecting compositor — the same
/// over-bright fringing defect fixed on the direct arm above. Rather than
/// build an unverifiable
/// premultiplying blitter (blit targets lack `STORAGE_BINDING`, so it would
/// need new machinery), the adopted fix is refusal: such a surface resolves
/// NOT translucent, degrading the app to Mode A (opaque base, no
/// platform-view punch) instead of silently fringing.
///
/// **`cpu-tier` is expressly exempt** — it also forces the blit arm
/// (`force_blit` above), but its `vello_cpu` output is already
/// premultiplied (verified: `cpu_tier.rs:108-113`'s `PremulRgba8` sample
/// type), so cpu-tier Blit+`Inherit` is correct *today* and must keep
/// resolving translucent; forcing it opaque would be a self-inflicted
/// regression. The `tier` parameter is what gates this exemption.
///
/// Pure decision, kept separate from `create_render_surface`'s wgpu
/// resources so it is host-testable without a GPU, mirroring
/// [`choose_render_path`]'s split.
fn blit_translucency_refused(
    path_kind: RenderPathKind,
    alpha_mode: wgpu::CompositeAlphaMode,
    tier: crate::tier::RenderTier,
) -> bool {
    path_kind == RenderPathKind::Blit
        && alpha_mode_needs_premultiply(alpha_mode)
        && tier == crate::tier::RenderTier::Gpu
}

/// Whether a configured surface must **refuse** translucency because the
/// hybrid tier's output convention is the opposite of the one its swapchain
/// stores — the mirror image of [`blit_translucency_refused`], and the same
/// remedy.
///
/// `vello_hybrid` presents **premultiplied** alpha: both of its user-surface
/// strip pipelines are built with `BlendState::PREMULTIPLIED_ALPHA_BLENDING`
/// (`vello_hybrid-0.2.0/src/render/wgpu/mod.rs:1248,1255`), so its fragments
/// reach the swapchain already multiplied through by their own alpha — the
/// exact opposite of vello's `render_to_texture`, which un-premultiplies at
/// its final write ([`alpha_mode_needs_premultiply`]). So the predicate this
/// arm needs is the exact opposite too:
///
/// - `Inherit`/`PreMultiplied` (Android's translucent mode, and the explicit
///   one) expect premultiplied and receive premultiplied — **correct as-is**,
///   no premultiply pass wanted and nothing to warn about. That is also why
///   this arm is deliberately not routed through
///   [`RenderPath::DirectPremultiplied`]: premultiplying again would darken
///   every partial-alpha pixel by a second factor of `a`.
/// - `PostMultiplied` (iOS's translucent mode,
///   [`alpha_mode_is_straight_translucent`]) expects STRAIGHT alpha and would
///   read `(C·a, a)` as `(C, a)` — every partial-alpha pixel too dark, over a
///   Mode B punch-through hole. This tier owns no stage frust could
///   un-premultiply in (`vello_hybrid` draws into the acquired view itself, so
///   there is no intermediate to post-process), so the answer is the policy
///   [`blit_translucency_refused`] already sets: refuse, degrading the app to
///   Mode A (opaque base, no platform-view punch), rather than silently
///   compositing wrong.
/// - `Opaque`/`Auto` ignore alpha entirely and are untouched.
///
/// Takes `path_kind` rather than assuming it, so the predicate cannot be
/// misapplied to a vello arm — those keep their own answers above. No `tier`
/// parameter is needed: [`RenderPathKind::HybridDirect`] is reachable from the
/// [`Hybrid`](crate::RenderTier::Hybrid) tier alone
/// ([`choose_hybrid_render_path`]).
///
/// Pure and host-testable without a GPU, like its blit sibling.
#[cfg(feature = "hybrid-tier")]
fn hybrid_translucency_refused(
    path_kind: RenderPathKind,
    alpha_mode: wgpu::CompositeAlphaMode,
) -> bool {
    path_kind == RenderPathKind::HybridDirect && alpha_mode_is_straight_translucent(alpha_mode)
}

/// Permanent surface-caps + chosen-alpha-mode line, logged once
/// per surface configure (not once per process — a resize/recreate that picks
/// a different mode is worth a fresh line, unlike the direct-to-surface probe
/// above). Gated exactly like
/// [`log_render_path`] so a release build (no `perf-trace` feature) stays
/// string-free.
#[cfg(feature = "perf-trace")]
fn log_surface_alpha_caps(
    capabilities: &wgpu::SurfaceCapabilities,
    chosen: wgpu::CompositeAlphaMode,
) {
    if !perf_tracing_enabled() {
        return;
    }
    log::info!(
        "frust-render surface-caps: alpha_modes={:?} chosen={:?}",
        capabilities.alpha_modes,
        chosen
    );
}

/// Without the `perf-trace` feature, the surface-caps/alpha line is a
/// complete no-op — no logging, no format-string bodies compiled in.
#[cfg(not(feature = "perf-trace"))]
#[inline]
fn log_surface_alpha_caps(
    _capabilities: &wgpu::SurfaceCapabilities,
    _chosen: wgpu::CompositeAlphaMode,
) {
}

/// A logical device plus the adapter it came from and the queue that executes
/// its command buffers — the frust-owned equivalent of
/// `vello::util::DeviceHandle` (whose `adapter` field is private, which is why
/// we cannot reuse vello's device pool).
pub(crate) struct DeviceHandle {
    pub(crate) adapter: wgpu::Adapter,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
}

/// Owns the wgpu `Instance` and the single logical device vello renders with.
///
/// A single `RenderContext` is shared across every surface a shell creates
/// (frust is single-window); the device is created lazily on the first
/// surface and reused across surface loss/recreation (rotation, backgrounding)
/// since a logical device is display-independent. It is passed into the
/// [`SurfaceRenderer`](crate::SurfaceRenderer) lifecycle and per-frame methods
/// so those operations reach the owning device.
pub struct RenderContext {
    pub(crate) instance: wgpu::Instance,
    /// Lazily created on the first surface; `None` until then.
    pub(crate) device: Option<DeviceHandle>,
    /// The tier [`ensure_device`](Self::ensure_device) selected for the live
    /// device. Defaults to [`RenderTier::Gpu`] and
    /// is only ever [`RenderTier::Cpu`] in a `cpu-tier`-feature build whose
    /// probe (or override) chose the CPU fallback — the
    /// [`SurfaceRenderer`](crate::SurfaceRenderer) reads it to pick the encode
    /// path. In a default (GPU-only) build a failed GPU probe errors out of
    /// `ensure_device` before this is ever set to anything but `Gpu`.
    pub(crate) selected_tier: crate::tier::RenderTier,
}

impl Default for RenderContext {
    fn default() -> Self {
        Self::new()
    }
}

/// A cheap, cloneable handle to a [`RenderContext`]'s wgpu `Instance`, used to
/// create a surface on a *different* thread than the one that owns the context
/// (the render-thread split).
///
/// # Why this exists
///
/// wgpu `Surface` creation reads the platform window handle, which several
/// windowing backends (notably winit on macOS/AppKit) only make available on
/// the main/UI thread. The render-thread split therefore cannot create the
/// surface where the renderer lives; instead the UI thread creates a
/// [`DetachedSurface`] via this factory and hands it across to the render
/// thread, which installs it with
/// [`SurfaceRenderer::on_surface_installed`](crate::SurfaceRenderer::on_surface_installed).
/// A `wgpu::Instance` is `Send + Sync + Clone` (Arc-backed) and a `Surface` it
/// produces stays compatible with any adapter/device the cloned instance
/// requests, so the two threads share one instance with no `unsafe`.
///
/// The mobile shells are unaffected: they receive a platform-created surface
/// pointer (`ANativeWindow`/`CAMetalLayer`) and keep using
/// [`SurfaceRenderer::on_surface_created_from_android_window`](crate::SurfaceRenderer::on_surface_created_from_android_window)
/// / `on_surface_created_from_metal_layer`.
#[derive(Clone)]
pub struct SurfaceFactory {
    instance: wgpu::Instance,
}

impl SurfaceFactory {
    /// Create a [`DetachedSurface`] from a window handle **on the calling
    /// thread** — call this on the thread the windowing backend requires
    /// (the main/UI thread for winit). The returned surface is `Send` and may
    /// then be moved to the render thread for
    /// [`install`](crate::SurfaceRenderer::on_surface_installed).
    ///
    /// This performs *only* the window-handle-dependent step (surface creation);
    /// the device, swapchain configuration, and blitter are all built later, on
    /// the installing thread, so nothing here touches the GPU device.
    pub fn create_detached_surface(
        &self,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
    ) -> Result<DetachedSurface> {
        let surface = self
            .instance
            .create_surface(target)
            .map_err(|e| anyhow!("frust-render: failed to create surface: {e}"))?;
        Ok(DetachedSurface { surface })
    }
}

/// A created-but-not-yet-installed wgpu `Surface`, produced by
/// [`SurfaceFactory::create_detached_surface`] on the windowing thread and
/// installed on the render thread via
/// [`SurfaceRenderer::on_surface_installed`](crate::SurfaceRenderer::on_surface_installed).
/// Opaque so the `wgpu` type stays confined to this crate; a
/// shell only moves it across a thread boundary. `Send` (a `wgpu::Surface` is
/// `Send + Sync`), which is the whole point.
pub struct DetachedSurface {
    surface: wgpu::Surface<'static>,
}

impl DetachedSurface {
    /// Consume the wrapper, yielding the raw surface for installation. Crate-
    /// private so the `wgpu` type never escapes `frust-render`.
    pub(crate) fn into_surface(self) -> wgpu::Surface<'static> {
        self.surface
    }
}

/// Straight-alpha → premultiplied-alpha conversion compute pass for the direct
/// render path's translucent, premultiplied-expecting arm
/// ([`RenderPath::DirectPremultiplied`]).
///
/// vello 0.9 outputs **straight** alpha (see [`alpha_mode_needs_premultiply`]);
/// a `PreMultiplied`/`Inherit` swapchain needs it premultiplied. This pass
/// reads the straight vello output as a sampled texture and writes
/// `(rgb*a, a)` into the swapchain (a write-only `rgba8unorm` storage texture —
/// the same `STORAGE_BINDING` the direct path already configures the swapchain
/// with). Portable: sampled read + write-only storage, no in-place read-write
/// storage (which `rgba8unorm` does not support).
pub(crate) struct PremultiplyPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

/// WGSL for [`PremultiplyPass`]: premultiply every texel of a straight-alpha
/// source into a premultiplied destination. One invocation per pixel; the
/// bounds guard covers a surface whose dimensions are not a multiple of the
/// workgroup size.
const PREMULTIPLY_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8)
fn premultiply(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = textureDimensions(dst);
    if gid.x >= dims.x || gid.y >= dims.y {
        return;
    }
    let coord = vec2<i32>(i32(gid.x), i32(gid.y));
    let c = textureLoad(src, coord, 0);
    textureStore(dst, coord, vec4<f32>(c.rgb * c.a, c.a));
}
"#;

impl PremultiplyPass {
    /// Compute workgroup edge (matches `@workgroup_size(8, 8)` in
    /// [`PREMULTIPLY_WGSL`]); dispatch counts round up against it.
    const WORKGROUP: u32 = 8;

    /// Builds the compute pipeline + bind-group layout. Cheap enough to build
    /// once per surface configure (a rare event), like the blit arm's
    /// [`TextureBlitter`].
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frust-render premultiply module"),
            source: wgpu::ShaderSource::Wgsl(PREMULTIPLY_WGSL.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frust-render premultiply binds"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("frust-render premultiply layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("frust-render premultiply pipeline"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("premultiply"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            pipeline,
            bind_group_layout,
        }
    }

    /// Records the straight→premultiplied compute pass into `encoder`: reads
    /// `src_view` (vello's straight output) and writes premultiplied pixels
    /// into `dst_view` (the acquired swapchain texture). The caller submits
    /// `encoder`.
    pub(crate) fn record(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        dst_view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frust-render premultiply bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(dst_view),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("frust-render premultiply pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(
            width.div_ceil(Self::WORKGROUP),
            height.div_ceil(Self::WORKGROUP),
            1,
        );
    }
}

/// The per-frame render path a [`ConfiguredSurface`] carries — the resources
/// specific to the chosen arm (see [`RenderPathKind`]).
///
/// The [`Direct`](Self::Direct) arm holds nothing extra: vello renders straight
/// into the acquired swapchain texture, so there is no intermediate target or
/// blitter to keep. The [`Blit`](Self::Blit) arm owns the intermediate
/// `Rgba8Unorm` target vello renders into plus the [`TextureBlitter`] that
/// copies it to the swapchain each frame.
pub(crate) enum RenderPath {
    /// Direct-to-surface: the swapchain is configured `Rgba8Unorm` +
    /// `STORAGE_BINDING` and vello's `render_to_texture` targets its acquired
    /// texture directly. Eliminates the intermediate texture and
    /// the per-frame blit pass.
    ///
    /// Shared by the [`Hybrid`](crate::RenderTier::Hybrid) tier
    /// ([`RenderPathKind::HybridDirect`]), which needs the same thing from a
    /// surface — nothing but the acquired view — while configuring it
    /// `RENDER_ATTACHMENT` in the surface's own format instead. The two are
    /// one variant because the per-frame *shape* is identical: `encode` does
    /// CPU work only, `submit` renders into the texture `acquire` produced.
    /// Which renderer runs there is the backend's business
    /// (`renderer::TierBackend`), not the path's.
    Direct,
    /// Direct-to-surface for a **premultiplied-expecting translucent** swapchain
    /// (`Inherit`/`PreMultiplied` alpha mode — see
    /// [`alpha_mode_needs_premultiply`]). vello's
    /// output is straight-alpha, but such a compositor blends premultiplied, so
    /// the frame cannot be presented straight from `render_to_texture`. vello
    /// instead renders into `intermediate_view` (in `encode`, where it already
    /// exists — unlike the swapchain), then [`Self::DirectPremultiplied`]'s
    /// `premultiply` compute pass writes `(rgb*a, a)` into the acquired
    /// swapchain texture (in `submit`). Opaque and `PostMultiplied` surfaces
    /// never take this arm (they stay [`Self::Direct`], byte-identical). Still
    /// the direct family (`STORAGE_BINDING` swapchain, no `RENDER_ATTACHMENT`
    /// blit) — the extra pass is one compute dispatch over the frame, paid only
    /// by the translucent surfaces that need it.
    ///
    /// Only `intermediate_view` is stored, not the backing `wgpu::Texture`: a
    /// `TextureView` refcounts its texture alive internally, and unlike the blit
    /// arm's `target_texture` (which cpu-tier writes into via `write_texture`)
    /// nothing here needs the texture handle — this arm is GPU-tier only.
    DirectPremultiplied {
        intermediate_view: wgpu::TextureView,
        premultiply: PremultiplyPass,
    },
    /// Blit fallback: vello renders into `target_view`, then `blitter` draws
    /// `target_texture` over the acquired swapchain texture each frame —
    /// upscaling it when `FRUST_RENDER_SCALE` made the intermediate smaller
    /// than the swapchain. Also the `cpu-tier` upload target (`COPY_DST`, see
    /// [`create_targets`]).
    Blit {
        target_texture: wgpu::Texture,
        target_view: wgpu::TextureView,
        blitter: TextureBlitter,
        /// The intermediate's own pixel size, which is the surface's size
        /// unless `FRUST_RENDER_SCALE` shrank it ([`scaled_size`]) — the size
        /// every pass that targets this texture must use (vello's
        /// `RenderParams`, the compositor's target and its trailing scratch),
        /// as opposed to `ConfiguredSurface::config`'s swapchain size, which
        /// stays the surface's. Stored rather than re-derived per frame so
        /// one resolved answer feeds every consumer.
        target_size: (u32, u32),
        /// The root transform every vello pass into this intermediate encodes
        /// under — [`blit_root`] of the swapchain size onto `target_size`,
        /// resolved once where that size is (surface creation and resize).
        /// `Affine::IDENTITY` while unscaled, so the default path encodes
        /// exactly the transform it did before the render-scale knob existed.
        ///
        /// Stored beside the size it is derived from, rather than recomputed
        /// per frame from the process-global scale, so the frame is scaled by
        /// precisely the ratio its target has: the two cannot drift apart, and
        /// the ceil'd axis has no unpainted strip left in it.
        root: Affine,
    },
}

/// A configured swapchain surface plus the render-path resources for the arm it
/// was configured for — frust's replacement for `vello::util::RenderSurface`
/// (whose fields always include an intermediate target + blitter, which the
/// direct arm does not use). Crate-private; the wrapped `wgpu` types never
/// escape `frust-render`.
pub(crate) struct ConfiguredSurface {
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) config: wgpu::SurfaceConfiguration,
    pub(crate) path: RenderPath,
    /// Whether this surface **actually** came up translucent — the wgpu-free
    /// projection of `config.alpha_mode` through [`alpha_mode_is_translucent`],
    /// computed once at configure time (the mode never changes for a live
    /// surface; a resize reconfigures with the same `config`), *except* when
    /// either arm-specific refusal forces it to `false`: a GPU-tier
    /// blit-fallback surface cannot deliver the premultiplied output such an
    /// alpha mode expects ([`blit_translucency_refused`] — `cpu-tier` is
    /// exempt), and the hybrid tier cannot deliver the straight output
    /// `PostMultiplied` expects ([`hybrid_translucency_refused`]).
    ///
    /// Stored as a plain `bool` rather than re-derived from `config.alpha_mode`
    /// at each read so the value a shell observes through
    /// [`SurfaceRenderer::surface_resolved_translucent`](crate::SurfaceRenderer::surface_resolved_translucent)
    /// crosses this crate's boundary with no `wgpu` type in the signature
    /// (`docs/CODE_STANDARDS.md`'s wgpu-leak anti-pattern).
    pub(crate) resolved_translucent: bool,
}

impl ConfiguredSurface {
    /// Whether this surface really came up translucent with a swapchain that
    /// stores STRAIGHT alpha — iOS's `PostMultiplied` translucent mode.
    ///
    /// Reads [`Self::resolved_translucent`] rather than the raw alpha mode, so
    /// a surface whose translucency was REFUSED (by either
    /// [`blit_translucency_refused`] or, for this mode specifically, the hybrid
    /// tier's [`hybrid_translucency_refused`]) answers `false`: it presents
    /// opaque, and an opaque destination is exactly what the straight blend is
    /// exact for.
    ///
    /// [`crate::renderer::SurfaceRenderer`] folds this into the snapshot
    /// cache's enable decision (`renderer::snapshot_cache_enabled`): the
    /// compositor's straight arm computes `dst * (1 - a) + rgb * a`, the
    /// straight-alpha `over` only at destination alpha 1, so such a surface
    /// keeps every bracket on the inline path instead.
    pub(crate) fn straight_alpha_translucent(&self) -> bool {
        self.resolved_translucent && alpha_mode_is_straight_translucent(self.config.alpha_mode)
    }

    /// Whether THIS surface renders below its own resolution — the per-surface
    /// answer every scale-dependent decision downstream of configuration reads
    /// (the snapshot-cache refusal in
    /// [`crate::renderer::snapshot_cache_enabled`]), so none of them can
    /// disagree with the geometry the surface was actually configured with.
    ///
    /// Derived from the sizes the surface holds ([`path_render_scaled`]) rather
    /// than from [`render_scaled`]'s process global: the same truth
    /// [`RenderPath::Blit`]'s `root` is built from, so a scaled frame's encode
    /// and the cache's refusal are answering one question, not two.
    pub(crate) fn render_scaled(&self) -> bool {
        let blit_target = match &self.path {
            RenderPath::Blit { target_size, .. } => Some(*target_size),
            RenderPath::Direct | RenderPath::DirectPremultiplied { .. } => None,
        };
        path_render_scaled((self.config.width, self.config.height), blit_target)
    }
}

/// Every swapchain format [`RenderContext::create_render_surface`] can
/// configure a surface with: the direct arm's mandatory `Rgba8Unorm` (vello's
/// `render_to_texture` target format) and, on the blit arm, whichever of the
/// two the platform reports first.
///
/// Named once so the compositor's own attachment-format predicate can be
/// coupled to it by a test (`compositor::is_supported_target_format`): the
/// composite pass draws onto the swapchain itself on both direct arms, so a
/// format this list gained without the pass gaining its arithmetic would
/// silently disable snapshot layers on that surface.
pub(crate) const SURFACE_FORMATS: [wgpu::TextureFormat; 2] = [
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::Bgra8Unorm,
];

/// Given the build-config-derived instance flags and whether the process is
/// currently running on an Android emulator, decides the flags wgpu's
/// `Instance` should actually be created with.
///
/// Pure decision logic, kept separate from the platform property lookup in
/// [`is_android_emulator`] so it can be unit-tested on any host without an
/// Android target.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn effective_instance_flags(flags: wgpu::InstanceFlags, is_emulator: bool) -> wgpu::InstanceFlags {
    if is_emulator {
        flags - (wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION)
    } else {
        flags
    }
}

/// Given a base `wgpu::Limits` and whether the process is currently running
/// on an iOS Simulator, decides the `Limits` a device request should
/// actually use.
///
/// Mitigates [wgpu #7057](https://github.com/gfx-rs/wgpu/issues/7057): the
/// iOS Simulator is macOS-Metal-backed and requires 256-byte
/// `min_uniform_buffer_offset_alignment`, but wgpu 29's Metal backend reports
/// the (lower) iOS-device value, which trips Metal API validation on the
/// simulator. Physical iOS devices are unaffected and pass `base` through
/// unchanged; a `base` whose alignment is already `>= 256` is left alone
/// (never lowered).
///
/// Pure decision logic, mirroring [`effective_instance_flags`]'s split of
/// pure decision vs. platform lookup. It is fed the *adapter's* real limits (so
/// the device request never over-asks and fails on the simulator) and, on the
/// simulator, has its uniform-buffer alignment forced up to 256 — see
/// [`RenderContext::ensure_device`], the live call site.
pub(crate) fn effective_limits(base: wgpu::Limits, is_ios_simulator: bool) -> wgpu::Limits {
    const IOS_SIMULATOR_MIN_UNIFORM_BUFFER_OFFSET_ALIGNMENT: u32 = 256;
    if is_ios_simulator
        && base.min_uniform_buffer_offset_alignment
            < IOS_SIMULATOR_MIN_UNIFORM_BUFFER_OFFSET_ALIGNMENT
    {
        wgpu::Limits {
            min_uniform_buffer_offset_alignment: IOS_SIMULATOR_MIN_UNIFORM_BUFFER_OFFSET_ALIGNMENT,
            ..base
        }
    } else {
        base
    }
}

/// Whether this binary is running on the iOS Simulator (`aarch64-apple-ios-sim`
/// / `x86_64-apple-ios` under the simulator), which sets `target_abi = "sim"`.
/// Compile-time constant: the simulator mitigation only needs to apply to
/// simulator builds, never physical-device or desktop ones.
pub(crate) const fn is_ios_simulator() -> bool {
    cfg!(all(target_os = "ios", target_abi = "sim"))
}

/// Number of uncaptured `wgpu` errors logged at error level per device before
/// the handler latches into suppression. A single flaky frame under a driver
/// hiccup (e.g. the Android emulator's SwiftShader path — see the
/// `on_uncaptured_error` doc below) is expected to surface a handful of
/// errors; past this the process is either wedged in a genuine per-frame error
/// storm or the driver is fundamentally broken, and re-logging every single
/// one would flood the log without adding information.
const MAX_LOGGED_UNCAPTURED_ERRORS: u32 = 5;

/// How often (in error count) a latched handler bumps a debug-level "still
/// happening" line once past [`MAX_LOGGED_UNCAPTURED_ERRORS`] and the one
/// suppression notice. Debug level (not error) because this is diagnostic
/// noise for someone actively investigating, not an actionable signal.
const UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD: u32 = 100;

/// What the `on_uncaptured_error` handler should do for the `count`-th
/// uncaptured error (1-indexed) it has observed on a given device.
///
/// Pure decision logic, split out of the handler closure in [`RenderContext::ensure_device`]
/// so the latch discipline — log the first few, announce the latch once, then
/// go quiet except an occasional debug bump — is unit-testable without a GPU
/// or a real `wgpu::Error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LogAction {
    /// One of the first [`MAX_LOGGED_UNCAPTURED_ERRORS`]: log the error itself
    /// at error level.
    Log,
    /// The first error past the cap: log one suppression notice (naming the
    /// running total) instead of the error itself.
    SuppressionNotice,
    /// Past the cap and past the suppression notice: stay silent, except a
    /// periodic debug-level count bump when `debug_bump` is set.
    Silent { debug_bump: bool },
}

/// Pure latch policy for the uncaptured-error handler (see [`LogAction`]).
pub(crate) fn decide_log_action(count: u32) -> LogAction {
    if count <= MAX_LOGGED_UNCAPTURED_ERRORS {
        LogAction::Log
    } else if count == MAX_LOGGED_UNCAPTURED_ERRORS + 1 {
        LogAction::SuppressionNotice
    } else {
        LogAction::Silent {
            debug_bump: count.is_multiple_of(UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD),
        }
    }
}

/// Detects whether the current process is running on an Android emulator
/// (goldfish/ranchu), as opposed to a physical device, via the standard
/// `ro.kernel.qemu` system property (`"1"` on emulators, unset/absent on
/// real hardware). A failed property read (`None`) is treated as "not an
/// emulator" so physical devices — and any environment where the property
/// can't be read — default to keeping validation on.
#[cfg(target_os = "android")]
fn is_android_emulator() -> bool {
    android_system_properties::AndroidSystemProperties::new()
        .get("ro.kernel.qemu")
        .as_deref()
        == Some("1")
}

/// Creates the intermediate render target vello draws into (it renders via a
/// compute shader, which cannot bind a swapchain texture directly, so each
/// frame is blitted from this target to the acquired surface texture).
/// Replicates `vello::util::create_targets` (which is private).
fn create_targets(
    width: u32,
    height: u32,
    device: &wgpu::Device,
) -> (wgpu::Texture, wgpu::TextureView) {
    // The GPU tier renders into this via a storage binding, then blits it to
    // the swapchain. `RENDER_ATTACHMENT` is the snapshot compositor's
    // (`crate::compositor`) write path: on the blit and direct-premultiplied
    // arms this intermediate is the colour attachment its quad pass loads and
    // draws onto, between vello and the blit/premultiply tail. `Rgba8Unorm` is
    // renderable on every wgpu backend, so the combined usage is universally
    // available. The CPU tier (`cpu-tier` feature) instead uploads its
    // rasterized pixmap into it with `write_texture`, which needs `COPY_DST` —
    // added only under the feature so default GPU-only builds keep the exact
    // usage set they had before.
    #[allow(unused_mut)]
    let mut usage = wgpu::TextureUsages::STORAGE_BINDING
        | wgpu::TextureUsages::TEXTURE_BINDING
        | wgpu::TextureUsages::RENDER_ATTACHMENT;
    #[cfg(feature = "cpu-tier")]
    {
        usage |= wgpu::TextureUsages::COPY_DST;
    }
    let target_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust-render vello target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        usage,
        format: wgpu::TextureFormat::Rgba8Unorm,
        view_formats: &[],
    });
    let target_view = target_texture.create_view(&wgpu::TextureViewDescriptor::default());
    (target_texture, target_view)
}

impl RenderContext {
    /// Creates a context with a fresh wgpu `Instance` and no device yet
    /// (the device is created lazily on first surface creation).
    ///
    /// Mirrors `vello::util::RenderContext::new()`'s instance setup, except
    /// when actually running on an Android *emulator* it strips the
    /// `DEBUG`/`VALIDATION` instance flags (which
    /// `InstanceFlags::from_build_config()` turns on in debug builds). The
    /// `DEBUG` flag makes wgpu enable `VK_EXT_debug_utils` and set
    /// object-name labels via `vkSetDebugUtilsObjectNameEXT`, and the
    /// emulator's gfxstream Vulkan HAL (`vulkan.ranchu.so`) segfaults inside
    /// that entry point during adapter enumeration — the same class of
    /// debug-utils fragility a MoltenVK Vulkan backend is also known to have
    /// (observed crash: `#00 vulkan.ranchu.so
    /// vk_common_SetDebugUtilsObjectNameEXT`). Debug object labels are only a
    /// developer convenience, so dropping them on the emulator is a safe way
    /// to keep GPU bring-up alive there while leaving physical devices'
    /// validation safety net — and desktop behavior — untouched.
    pub fn new() -> Self {
        let backends = wgpu::Backends::from_env().unwrap_or_default();
        let build_flags = wgpu::InstanceFlags::from_build_config().with_env();
        #[cfg(target_os = "android")]
        let flags = effective_instance_flags(build_flags, is_android_emulator());
        #[cfg(not(target_os = "android"))]
        let flags = build_flags;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            display: None,
            backends,
            flags,
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::from_env_or_default(),
        });
        Self {
            instance,
            device: None,
            selected_tier: crate::tier::RenderTier::Gpu,
        }
    }

    /// A cloneable [`SurfaceFactory`] sharing this context's wgpu `Instance`,
    /// for creating a [`DetachedSurface`] on the windowing/main thread when the
    /// context itself lives on the render thread (the
    /// render-thread split). The surface a clone produces stays compatible with
    /// the device this context creates, since both share one Arc-backed
    /// instance.
    pub fn surface_factory(&self) -> SurfaceFactory {
        SurfaceFactory {
            instance: self.instance.clone(),
        }
    }

    /// The render tier the live device was created for (see
    /// [`selected_tier`](Self::selected_tier) field). `Gpu` until a surface
    /// (and thus a device) has been created.
    pub(crate) fn selected_tier(&self) -> crate::tier::RenderTier {
        self.selected_tier
    }

    /// The single logical device, panicking if no surface has created it yet.
    ///
    /// Only called from [`crate::SurfaceRenderer`]'s install/resize/render
    /// paths, all of which run strictly after a `create_*_surface`, so the
    /// device is always present.
    pub(crate) fn device_handle(&self) -> &DeviceHandle {
        self.device
            .as_ref()
            .expect("device must be created before it is used (surface creation creates it)")
    }

    /// Whether the live device was created with `wgpu::Features::PIPELINE_CACHE`.
    ///
    /// wgpu 29 only implements the persisted pipeline cache on Vulkan — every
    /// Vulkan adapter advertises it (Android, Linux, Windows-on-Vulkan);
    /// Metal and DX12 adapters never do, so it is absent there
    /// and [`create_pipeline_cache`](Self::create_pipeline_cache) returns `None`
    /// — the renderer then behaves exactly as it did before this path existed.
    /// Panics if no surface (and thus no device) has been created yet.
    pub(crate) fn pipeline_cache_supported(&self) -> bool {
        self.device_handle()
            .device
            .features()
            .contains(wgpu::Features::PIPELINE_CACHE)
    }

    /// The adapter fingerprint a persisted pipeline-cache blob is tagged with
    /// (see [`crate::pipeline_cache`]). Panics if no device has been created yet.
    pub(crate) fn adapter_cache_key(&self) -> String {
        crate::pipeline_cache::adapter_cache_key(&self.device_handle().adapter.get_info())
    }

    /// Creates a `wgpu::PipelineCache` for the live device, seeded from a
    /// previously persisted, framed `blob` when it validates for this adapter.
    ///
    /// Returns `None` when the device lacks `PIPELINE_CACHE` support (Metal/
    /// desktop) — the renderer then runs its original, cache-less path. A `blob`
    /// that fails framing/adapter validation
    /// ([`crate::pipeline_cache::unframe`]) is discarded and the cache starts
    /// empty; a `None` `blob` is a cold start.
    ///
    /// # Safety
    ///
    /// This is the sole sanctioned unsafe site in this module. The wgpu contract
    /// on [`wgpu::Device::create_pipeline_cache`] is that a non-`None` `data`
    /// must have come from a prior `PipelineCache::get_data()` on a
    /// `pipeline_cache_key`-compatible adapter. We uphold it two ways: (1) the
    /// caller-supplied blob is `unframe`d against *this* adapter's fingerprint
    /// **before** the unsafe call, so foreign or post-driver-update data never
    /// reaches it; (2) `fallback: true` makes wgpu fall back to an empty cache
    /// for any data it still rejects internally, rather than misbehaving.
    /// Corrupt/mismatched data is therefore a silently-ignored cache miss, never
    /// undefined behaviour.
    pub(crate) fn create_pipeline_cache(&self, blob: Option<&[u8]>) -> Option<wgpu::PipelineCache> {
        if !self.pipeline_cache_supported() {
            return None;
        }
        let handle = self.device_handle();
        let key = crate::pipeline_cache::adapter_cache_key(&handle.adapter.get_info());
        let data = blob.and_then(|b| crate::pipeline_cache::unframe(b, &key));
        log::debug!(
            "frust-render: creating wgpu PipelineCache (seed: {})",
            if data.is_some() {
                "persisted blob"
            } else {
                "empty"
            }
        );
        // SAFETY: see this method's `# Safety` section — `data` has already been
        // validated against this adapter's fingerprint by `unframe`, and
        // `fallback: true` turns any residual internal mismatch into a
        // fall-back-to-empty cache rather than UB.
        let cache = unsafe {
            handle
                .device
                .create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                    label: Some("frust-render pipeline cache"),
                    data,
                    fallback: true,
                })
        };
        Some(cache)
    }

    /// Lazily create the logical device compatible with `surface`, requesting
    /// the adapter's own limits (never `Limits::default()`, which the iOS
    /// Simulator cannot satisfy) plus the #7057 alignment mitigation. Reuses
    /// an already-created device when it is compatible with `surface`, so
    /// surface loss/recreation (rotation, backgrounding) never rebuilds it — and
    /// so a device the [`ensure_device_headless`](Self::ensure_device_headless)
    /// pre-init created before any surface existed is adopted here rather than
    /// rebuilt.
    async fn ensure_device(&mut self, surface: &wgpu::Surface<'static>) -> Result<()> {
        if let Some(existing) = &self.device
            && existing.adapter.is_surface_supported(surface)
        {
            return Ok(());
        }
        self.create_device(Some(surface)).await
    }

    /// Create the logical device **before any surface exists**, so the wgpu
    /// instance/adapter/device bring-up can run on a
    /// background thread kicked at native-library load (`JNI_OnLoad`) and be
    /// joined by `nativeInit` instead of running serially after `surfaceCreated`.
    /// Idempotent: a no-op when a device already exists.
    ///
    /// # Android singular-adapter assumption
    ///
    /// Requesting the adapter with `compatible_surface: None` picks wgpu's
    /// default adapter rather than one filtered to a specific surface. On Android
    /// the Vulkan backend exposes a single physical device, so the adapter chosen
    /// here is the same one a later surface-filtered request would pick, and
    /// [`ensure_device`](Self::ensure_device)'s `is_surface_supported` reuse check
    /// accepts it — the surface created at `nativeInit` reuses this device with no
    /// rebuild. On a hypothetical multi-adapter device where the pre-init adapter
    /// did *not* support the eventual surface, `ensure_device` simply rebuilds the
    /// device against that surface (still correct, just without the overlap win).
    /// This is the sole caller that passes `None` below; desktop/iOS create their
    /// device through the surface path and never invoke this.
    pub async fn ensure_device_headless(&mut self) -> Result<()> {
        if self.device.is_some() {
            return Ok(());
        }
        self.create_device(None).await
    }

    /// Shared device-creation body for both the surface-bound
    /// ([`ensure_device`](Self::ensure_device)) and pre-surface
    /// ([`ensure_device_headless`](Self::ensure_device_headless)) paths.
    ///
    /// `compatible_surface` filters adapter selection to one that can present to
    /// the given surface; `None` (the pre-init path) selects wgpu's default
    /// adapter (see the singular-adapter note on `ensure_device_headless`). The
    /// tier probe, limits mitigation, uncaptured-error handler, and device
    /// request are identical either way — the surface only ever affected adapter
    /// selection, never the device it yields.
    async fn create_device(
        &mut self,
        compatible_surface: Option<&wgpu::Surface<'static>>,
    ) -> Result<()> {
        let adapter =
            wgpu::util::initialize_adapter_from_env_or_default(&self.instance, compatible_surface)
                .await
                .map_err(|e| anyhow!("frust-render: no compatible GPU adapter: {e}"))?;

        // Consult the tier probe instead of a
        // bespoke downlevel check, so a failed GPU probe surfaces through the
        // one diagnostic path `select_render_tier` owns (shared with its own
        // unit tests). An explicit override (`FRUST_RENDER_TIER`, or
        // `frust run --render-tier` setting it for the spawned process)
        // wins *among available tiers* — a `Cpu` override always applies, but a
        // `Gpu` override onto an adapter that lacks the required downlevel flags
        // is refused (`select_render_tier` returns `Unavailable`), so the
        // `Unavailable` arm below fails fast rather than handing vello a device
        // it panics on. When the `cpu-tier` feature is compiled in, a failed
        // GPU probe now selects the experimental `Cpu` tier (vello_cpu, see the
        // `cpu_tier` module + `SurfaceRenderer`) instead of failing — the
        // device is still created (only vello's compute path is unavailable;
        // the blit the CPU pixmap rides is a basic render pipeline). Without
        // the feature, `select_render_tier` never returns `Available(Cpu)`, so
        // a failed probe still fails fast here exactly like the prior bespoke
        // check did, just through the unified diagnosis.
        let downlevel = adapter.get_downlevel_capabilities();
        let caps = crate::tier::TierCaps {
            downlevel_flags: downlevel.flags,
            adapter_name: adapter.get_info().name,
        };
        let override_tier = crate::tier::render_tier_override_from_env();
        let selection = crate::tier::select_render_tier(&caps, override_tier);
        let tier = match selection.outcome {
            crate::tier::TierOutcome::Available(tier) => tier,
            crate::tier::TierOutcome::Unavailable { .. } => {
                return Err(anyhow!(selection.diagnosis));
            }
        };
        // An experimental tier is only reachable in a build that compiled its
        // renderer in; guard against a stray selection so the SurfaceRenderer
        // never sees a tier it has no encode path for. Asked per tier
        // ([`tier_compiled_in`]) rather than as `!= RenderTier::Gpu`, which
        // refused every tier but the default one whatever its feature said.
        if !tier_compiled_in(tier) {
            return Err(anyhow!(selection.diagnosis));
        }
        self.selected_tier = tier;

        let required_features = adapter.features() & vello_optional_features();
        let required_limits = effective_limits(adapter.limits(), is_ios_simulator());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-render device"),
                required_features,
                required_limits,
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow!("frust-render: failed to create GPU device: {e}"))?;

        // Route wgpu's uncaptured errors to the log instead of its default
        // handler, which aborts the process by panicking ("Handling wgpu errors
        // as fatal by default"). A UI framework must survive a driver's
        // *transient* GPU error and recover on a later frame rather than crash —
        // e.g. the Android emulator's SwiftShader path can raise a one-off
        // swapchain-acquire validation error under load, which used to wedge the
        // app into a per-frame panic loop (the surface stayed `SurfaceReady` and
        // every subsequent `render` re-hit the fatal handler). Pairing this with
        // the `Invalid`-acquire → reconfigure recovery (see [`crate::lifecycle`])
        // lets the swapchain rebuild and rendering resume. Genuine API misuse is
        // still surfaced — loudly, at error level — just without killing the
        // process across the FFI boundary.
        //
        // The handler latches rather than logging unbounded: a device stuck in a
        // genuine per-frame error storm (as opposed to a one-off driver hiccup)
        // would otherwise flood the log forever. `error_count` is per-device
        // (captured fresh each time this closure is installed, i.e. once per
        // logical device), `Arc<AtomicU32>` because `on_uncaptured_error`'s
        // handler must be `Fn`, not `FnMut` — see [`decide_log_action`] for the
        // pure latch policy this defers to.
        let error_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        device.on_uncaptured_error(std::sync::Arc::new(move |error| {
            let count = error_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            match decide_log_action(count) {
                LogAction::Log => {
                    log::error!("frust-render: uncaptured wgpu error: {error}");
                }
                LogAction::SuppressionNotice => {
                    log::error!(
                        "frust-render: further uncaptured wgpu errors suppressed \
                         (total so far: {count})"
                    );
                }
                LogAction::Silent { debug_bump } => {
                    if debug_bump {
                        log::debug!(
                            "frust-render: uncaptured wgpu error count now {count} \
                             (still suppressed)"
                        );
                    }
                }
            }
        }));

        self.device = Some(DeviceHandle {
            adapter,
            device,
            queue,
        });
        Ok(())
    }

    /// The render scale that actually applies to this context's surfaces:
    /// [`render_scale`] on the GPU tier, [`MAX_RENDER_SCALE`] on the CPU tier.
    ///
    /// `cpu-tier` rasterizes into its own pixmap through
    /// [`crate::convert::encode_into`] — an entry with no root transform at
    /// all — and uploads it into the same intermediate at the swapchain's
    /// dimensions, so a shrunken intermediate would be a `write_texture`
    /// larger than its destination rather than a scaled frame. The knob
    /// measures vello's GPU fine stage, so the experimental CPU tier simply
    /// keeps rendering at full size instead.
    fn effective_render_scale(&self) -> f64 {
        if self.selected_tier() == crate::tier::RenderTier::Gpu {
            render_scale()
        } else {
            MAX_RENDER_SCALE
        }
    }

    /// Builds a configured [`RenderSurface`] from a raw wgpu `Surface`,
    /// creating the logical device if needed. Replaces
    /// `vello::util::RenderContext::create_render_surface` so we control the
    /// device's `required_limits`.
    pub(crate) async fn create_render_surface(
        &mut self,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
        present_mode: wgpu::PresentMode,
        alpha: SurfaceAlphaRequest,
    ) -> Result<ConfiguredSurface> {
        self.ensure_device(&surface).await?;
        // The tier probe already ran in `ensure_device`, so
        // `selected_tier()` is authoritative here: the `cpu-tier` path uploads
        // its pixmap into the intermediate target, so it is blit-only and forces
        // the blit arm alongside the `FRUST_NO_DIRECT_SURFACE` safety valve.
        //
        // `FRUST_RENDER_SCALE < 1` forces it too, and for the same structural
        // reason: the scaled frame lives in an intermediate that has to be
        // upscaled into the swapchain, which is precisely the blit arm's
        // per-frame pass. A scaled surface therefore behaves exactly as
        // `FRUST_NO_DIRECT_SURFACE` does — including
        // [`blit_translucency_refused`] for a premultiplied-expecting alpha
        // mode — instead of growing a second scaled path on the direct arms.
        //
        // Asked of the tier through [`tier_forces_blit`], not as
        // `selected_tier() != RenderTier::Gpu`: that expression forced the
        // blit arm onto any tier added later, intermediate and blitter
        // included, however little it wanted one.
        let tier = self.selected_tier();
        let force_blit = direct_surface_force_blit() || render_scaled() || tier_forces_blit(tier);
        let handle = self.device_handle();

        let capabilities = surface.get_capabilities(&handle.adapter);
        probe_direct_to_surface_capability(&capabilities);
        let (has_rgba8unorm, has_storage_binding) = direct_surface_caps(&capabilities);
        // The hybrid tier decides its path on its own terms
        // ([`choose_hybrid_render_path`]) — it shares neither the probe nor
        // the valve `choose_render_path` weighs. Every other tier takes the
        // untouched vello decision.
        #[cfg(feature = "hybrid-tier")]
        let path_kind = if tier == crate::tier::RenderTier::Hybrid {
            choose_hybrid_render_path()
        } else {
            choose_render_path(has_rgba8unorm, has_storage_binding, force_blit)
        };
        #[cfg(not(feature = "hybrid-tier"))]
        let path_kind = choose_render_path(has_rgba8unorm, has_storage_binding, force_blit);
        log_render_path(path_kind, has_rgba8unorm, has_storage_binding, force_blit);
        log_render_scale(width, height, self.effective_render_scale());

        let alpha_mode = resolve_alpha_mode(alpha, &capabilities);
        log_surface_alpha_caps(&capabilities, alpha_mode);

        // Direct arm: the swapchain itself is the vello render target, so it must
        // be `Rgba8Unorm` (vello's `render_to_texture` target format) and carry
        // `STORAGE_BINDING` (vello renders via a compute storage write). Blit arm:
        // any supported `Rgba8/Bgra8` swapchain works — the blitter converts the
        // intermediate `Rgba8Unorm` target into it — and only `RENDER_ATTACHMENT`
        // is needed.
        let (format, usage) = match path_kind {
            RenderPathKind::Direct => (
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::STORAGE_BINDING,
            ),
            RenderPathKind::Blit => {
                let format = capabilities
                    .formats
                    .iter()
                    .copied()
                    .find(|it| SURFACE_FORMATS.contains(it))
                    .ok_or_else(|| {
                        anyhow!("frust-render: no supported surface format (Rgba8/Bgra8)")
                    })?;
                (format, wgpu::TextureUsages::RENDER_ATTACHMENT)
            }
            // Hybrid arm: `vello_hybrid` bakes the target format into its own
            // pipelines and draws through an ordinary render pass, so the
            // swapchain needs `RENDER_ATTACHMENT` and nothing else — whichever
            // supported format the surface reports first, exactly as the blit
            // arm picks one, but for the swapchain the frame lands in directly
            // rather than for an intermediate behind it. Duplicated rather
            // than shared with the arm above because the two lists are free to
            // diverge: this tier's pipelines are not bound by vello's
            // `render_to_texture` target format.
            #[cfg(feature = "hybrid-tier")]
            RenderPathKind::HybridDirect => {
                let format = capabilities
                    .formats
                    .iter()
                    .copied()
                    .find(|it| SURFACE_FORMATS.contains(it))
                    .ok_or_else(|| {
                        anyhow!("frust-render: no supported surface format (Rgba8/Bgra8)")
                    })?;
                (format, wgpu::TextureUsages::RENDER_ATTACHMENT)
            }
        };

        let config = wgpu::SurfaceConfiguration {
            usage,
            format,
            width,
            height,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        let path = match path_kind {
            // A premultiplied-expecting translucent swapchain (`Inherit`/
            // `PreMultiplied`) cannot take vello's straight output directly —
            // it needs a premultiply pass. Such surfaces get an
            // intermediate vello renders into plus the compute pass that
            // premultiplies it into the swapchain. Opaque/`PostMultiplied`
            // surfaces skip all of this and stay byte-identical on `Direct`.
            RenderPathKind::Direct if alpha_mode_needs_premultiply(alpha_mode) => {
                let (_intermediate_texture, intermediate_view) =
                    create_targets(width, height, &handle.device);
                RenderPath::DirectPremultiplied {
                    intermediate_view,
                    premultiply: PremultiplyPass::new(&handle.device),
                }
            }
            RenderPathKind::Direct => RenderPath::Direct,
            RenderPathKind::Blit => {
                // The swapchain keeps the surface's own size (the frame is
                // presented at full resolution however it was rendered); only
                // the intermediate shrinks, and the blit's sampled draw
                // stretches it back over the whole swapchain texture.
                let scale = self.effective_render_scale();
                let (target_width, target_height) = scaled_size(width, height, scale);
                let (target_texture, target_view) =
                    create_targets(target_width, target_height, &handle.device);
                RenderPath::Blit {
                    target_texture,
                    target_view,
                    // Linear only while scaled — the unscaled builder is the
                    // exact configuration `TextureBlitter::new` produces.
                    blitter: wgpu::util::TextureBlitterBuilder::new(&handle.device, format)
                        .sample_type(blit_filter(scale < MAX_RENDER_SCALE))
                        .build(),
                    target_size: (target_width, target_height),
                    // Derived HERE, from the size that was just resolved, so
                    // the encode side never re-derives a scale of its own.
                    root: blit_root((width, height), (target_width, target_height)),
                }
            }
            // The hybrid tier carries no per-surface render resources at all:
            // `vello_hybrid`'s own renderer owns everything the frame needs
            // (its depth texture included) and draws into the acquired
            // swapchain view, so this arm holds exactly what
            // [`RenderPath::Direct`] holds — nothing — and takes the same
            // encode/submit split.
            //
            // Deliberately NOT reachable from the premultiply arm above, and
            // deliberately needing nothing from it: that arm premultiplies
            // vello's straight output for a premultiplied-expecting swapchain,
            // whereas `vello_hybrid` already presents premultiplied, so
            // `Inherit`/`PreMultiplied` land here correct and untouched.
            // The one combination this tier cannot serve is the opposite one —
            // a swapchain storing STRAIGHT alpha — and
            // [`hybrid_translucency_refused`] refuses it below.
            #[cfg(feature = "hybrid-tier")]
            RenderPathKind::HybridDirect => RenderPath::Direct,
        };
        // The hybrid arm's refusal, resolved before the chain so the arm that
        // is compiled out contributes a plain `false` rather than a `#[cfg]`
        // inside the expression (same shape as `path_kind` above).
        #[cfg(feature = "hybrid-tier")]
        let hybrid_refused = hybrid_translucency_refused(path_kind, alpha_mode);
        #[cfg(not(feature = "hybrid-tier"))]
        let hybrid_refused = false;
        let resolved_translucent =
            if blit_translucency_refused(path_kind, alpha_mode, self.selected_tier()) {
                log::warn!(
                    "frust-render: GPU-tier blit-fallback surface cannot deliver premultiplied \
                 output (alpha_mode={alpha_mode:?}) — refusing translucency, app degrades to \
                 Mode A"
                );
                false
            } else if hybrid_refused {
                log::warn!(
                    "frust-render: hybrid tier presents premultiplied alpha, which a \
                 straight-alpha translucent surface (alpha_mode={alpha_mode:?}) would read as \
                 straight — refusing translucency, app degrades to Mode A"
                );
                false
            } else {
                // The RESOLVED translucency, not the request: a
                // `TranslucentPreferred` that fell back to `Auto` above lands here
                // as `false`, which is what the shells' paint contract keys off
                // (see `alpha_mode_is_translucent`).
                alpha_mode_is_translucent(alpha_mode)
            };
        let configured = ConfiguredSurface {
            surface,
            config,
            path,
            resolved_translucent,
        };
        self.configure_surface(&configured);
        Ok(configured)
    }

    /// Builds a [`RenderSurface`] from anything convertible into a wgpu
    /// `SurfaceTarget` (the desktop `winit` window path). Replaces
    /// `vello::util::RenderContext::create_surface`.
    pub(crate) async fn create_surface(
        &mut self,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
        present_mode: wgpu::PresentMode,
        alpha: SurfaceAlphaRequest,
    ) -> Result<ConfiguredSurface> {
        let surface = self
            .instance
            .create_surface(target)
            .map_err(|e| anyhow!("frust-render: failed to create surface: {e}"))?;
        self.create_render_surface(surface, width, height, present_mode, alpha)
            .await
    }

    /// (Re)configures the swapchain for `surface`'s current config.
    pub(crate) fn configure_surface(&self, surface: &ConfiguredSurface) {
        surface
            .surface
            .configure(&self.device_handle().device, &surface.config);
    }

    /// Resizes `surface` in place and reconfigures the swapchain. The blit arm
    /// and the direct-premultiplied arm each recreate their intermediate target
    /// texture at the new size; the plain direct arm has no intermediate, so
    /// only the swapchain config changes. Zero dimensions are rejected upstream.
    ///
    /// The blit arm's intermediate is recreated at the SCALED size and its
    /// recorded `target_size` and `root` updated with it, so a resize under
    /// `FRUST_RENDER_SCALE` keeps rendering at the same fraction of the new
    /// surface rather than silently returning to full resolution, and keeps
    /// mapping the frame exactly onto the new target. The
    /// direct-premultiplied arm is never scaled — scale < 1 forces the blit
    /// arm, so that arm only ever exists at scale 1.0.
    pub(crate) fn resize_surface(&self, surface: &mut ConfiguredSurface, width: u32, height: u32) {
        surface.config.width = width;
        surface.config.height = height;
        match &mut surface.path {
            RenderPath::Blit {
                target_texture,
                target_view,
                target_size,
                root,
                ..
            } => {
                let (target_width, target_height) =
                    scaled_size(width, height, self.effective_render_scale());
                let (new_texture, new_view) =
                    create_targets(target_width, target_height, &self.device_handle().device);
                *target_texture = new_texture;
                *target_view = new_view;
                *target_size = (target_width, target_height);
                *root = blit_root((width, height), (target_width, target_height));
            }
            RenderPath::DirectPremultiplied {
                intermediate_view, ..
            } => {
                let (_new_texture, new_view) =
                    create_targets(width, height, &self.device_handle().device);
                *intermediate_view = new_view;
            }
            RenderPath::Direct => {}
        }
        self.configure_surface(surface);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_flag_unset_both_halves_is_disabled() {
        // Neither the compile-time (`option_env!`) nor runtime (`std::env::var`)
        // half is set — every `FRUST_NO_*`/`FRUST_TRACE` flag's default state.
        assert!(!env_flag_enabled(None, None));
    }

    #[test]
    fn env_flag_compile_time_non_zero_enables() {
        assert!(env_flag_enabled(Some("1"), None));
    }

    #[test]
    fn env_flag_compile_time_zero_is_disabled() {
        // "0" is the explicit opt-out spelling (mirrors `FRUST_TRACE`'s doc).
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
        // Compile-time-or-runtime: an Android app process (no runtime env) still
        // honours a baked-in compile-time value, and vice versa.
        assert!(env_flag_enabled(Some("1"), Some("0".to_string())));
        assert!(env_flag_enabled(Some("0"), Some("1".to_string())));
    }

    #[test]
    fn env_str_unset_both_halves_is_none() {
        assert_eq!(env_str(None, None), None);
    }

    #[test]
    fn env_str_runtime_non_empty_beats_compile_time() {
        // Runtime wins even when a compile-time value is also present.
        assert_eq!(
            env_str(Some("msaa8"), Some("msaa16".to_string())),
            Some("msaa16".to_string())
        );
    }

    #[test]
    fn env_str_empty_runtime_falls_through_to_compile_time() {
        // An empty runtime value ("") is treated as unset, not a real override.
        assert_eq!(
            env_str(Some("msaa8"), Some(String::new())),
            Some("msaa8".to_string())
        );
    }

    #[test]
    fn env_str_compile_time_only() {
        assert_eq!(env_str(Some("msaa16"), None), Some("msaa16".to_string()));
    }

    #[test]
    fn env_str_runtime_only() {
        assert_eq!(
            env_str(None, Some("area".to_string())),
            Some("area".to_string())
        );
    }

    #[test]
    fn parse_aa_mode_none_is_area() {
        assert_eq!(parse_aa_mode(None), AaMode::Area);
    }

    #[test]
    fn parse_aa_mode_empty_is_area() {
        assert_eq!(parse_aa_mode(Some(String::new())), AaMode::Area);
    }

    #[test]
    fn parse_aa_mode_case_insensitive_hits() {
        assert_eq!(parse_aa_mode(Some("area".to_string())), AaMode::Area);
        assert_eq!(parse_aa_mode(Some("AREA".to_string())), AaMode::Area);
        assert_eq!(parse_aa_mode(Some("Msaa8".to_string())), AaMode::Msaa8);
        assert_eq!(parse_aa_mode(Some("MSAA8".to_string())), AaMode::Msaa8);
        assert_eq!(parse_aa_mode(Some("msaa16".to_string())), AaMode::Msaa16);
        assert_eq!(parse_aa_mode(Some("MsAa16".to_string())), AaMode::Msaa16);
    }

    #[test]
    fn parse_aa_mode_trims_surrounding_whitespace() {
        assert_eq!(parse_aa_mode(Some("  msaa8  ".to_string())), AaMode::Msaa8);
        assert_eq!(
            parse_aa_mode(Some("\tmsaa16\n".to_string())),
            AaMode::Msaa16
        );
    }

    #[test]
    fn parse_aa_mode_unknown_falls_back_to_area() {
        assert_eq!(parse_aa_mode(Some("nonsense".to_string())), AaMode::Area);
    }

    #[test]
    fn aa_mode_support_enables_exactly_the_selected_mode() {
        let area = AaMode::Area.support();
        assert!(area.area);
        assert!(!area.msaa8);
        assert!(!area.msaa16);

        let msaa8 = AaMode::Msaa8.support();
        assert!(!msaa8.area);
        assert!(msaa8.msaa8);
        assert!(!msaa8.msaa16);

        let msaa16 = AaMode::Msaa16.support();
        assert!(!msaa16.area);
        assert!(!msaa16.msaa8);
        assert!(msaa16.msaa16);
    }

    #[test]
    fn aa_mode_to_vello_matches_selected_mode() {
        assert_eq!(AaMode::Area.to_vello(), vello::AaConfig::Area);
        assert_eq!(AaMode::Msaa8.to_vello(), vello::AaConfig::Msaa8);
        assert_eq!(AaMode::Msaa16.to_vello(), vello::AaConfig::Msaa16);
    }

    #[test]
    fn aa_mode_display_matches_knob_spelling() {
        assert_eq!(AaMode::Area.to_string(), "area");
        assert_eq!(AaMode::Msaa8.to_string(), "msaa8");
        assert_eq!(AaMode::Msaa16.to_string(), "msaa16");
    }

    #[test]
    fn parse_render_scale_unset_or_empty_is_full_resolution() {
        // The knob wasn't touched: full resolution, silently.
        assert_eq!(parse_render_scale(None), 1.0);
        assert_eq!(parse_render_scale(Some(String::new())), 1.0);
        assert_eq!(parse_render_scale(Some("   ".to_string())), 1.0);
    }

    #[test]
    fn parse_render_scale_reads_a_fraction_in_range() {
        assert_eq!(parse_render_scale(Some("0.75".to_string())), 0.75);
        assert_eq!(parse_render_scale(Some("0.5".to_string())), 0.5);
        // Trimmed like every other string knob, and the top of the range is a
        // legal (identity) value rather than a clamp.
        assert_eq!(parse_render_scale(Some("  0.6\n".to_string())), 0.6);
        assert_eq!(parse_render_scale(Some("1".to_string())), 1.0);
    }

    #[test]
    fn parse_render_scale_clamps_out_of_range_values() {
        // Above the surface's own resolution is supersampling, not this knob.
        assert_eq!(parse_render_scale(Some("2".to_string())), 1.0);
        // Below a quarter the frame is too coarse to judge the upscale by.
        assert_eq!(parse_render_scale(Some("0.1".to_string())), 0.25);
        assert_eq!(parse_render_scale(Some("-1".to_string())), 0.25);
    }

    #[test]
    fn parse_render_scale_rejects_unparsable_and_non_finite_values() {
        // A typo must not render at a mystery resolution, and NaN/inf must be
        // caught BEFORE the clamp (`f64::clamp` would hand NaN straight back).
        assert_eq!(parse_render_scale(Some("abc".to_string())), 1.0);
        assert_eq!(parse_render_scale(Some("NaN".to_string())), 1.0);
        assert_eq!(parse_render_scale(Some("inf".to_string())), 1.0);
        assert_eq!(parse_render_scale(Some("-inf".to_string())), 1.0);
    }

    #[test]
    fn scaled_size_rounds_up_and_is_identity_at_full_scale() {
        // A 1080x2400 phone at the two scales the measurement uses.
        assert_eq!(scaled_size(1080, 2400, 0.75), (810, 1800));
        assert_eq!(scaled_size(1080, 2400, 0.5), (540, 1200));
        // Identity at 1.0 is what keeps the default path byte-identical.
        assert_eq!(scaled_size(1080, 2400, 1.0), (1080, 2400));
        // Rounds UP, so no row/column of the surface goes unsampled.
        assert_eq!(scaled_size(1081, 2401, 0.5), (541, 1201));
        // …and never asks wgpu for a zero-sized texture.
        assert_eq!(scaled_size(1, 1, 0.25), (1, 1));
    }

    #[test]
    fn blit_filter_is_linear_only_while_scaled() {
        // Nearest unscaled is exactly what `TextureBlitter::new` builds, so an
        // unscaled surface keeps the blitter it always had.
        assert_eq!(blit_filter(false), wgpu::FilterMode::Nearest);
        assert_eq!(blit_filter(true), wgpu::FilterMode::Linear);
    }

    #[test]
    fn blit_root_is_identity_when_the_intermediate_is_the_surface_size() {
        // The unscaled default, both spellings of it: the size handed straight
        // through, and the one `scaled_size` produces at full scale. Identity
        // here is what keeps the default path byte-identical.
        assert_eq!(
            blit_root((1080, 2400), (1080, 2400)).as_coeffs(),
            Affine::IDENTITY.as_coeffs()
        );
        assert_eq!(
            blit_root((1080, 2400), scaled_size(1080, 2400, 1.0)).as_coeffs(),
            Affine::IDENTITY.as_coeffs()
        );
    }

    #[test]
    fn blit_root_is_the_plain_scale_on_an_evenly_divisible_surface() {
        // 1080x2340 at 0.75 divides evenly on both axes, so nothing was
        // rounded and the root IS the requested fraction.
        let target = scaled_size(1080, 2340, 0.75);
        assert_eq!(target, (810, 1755));
        assert_eq!(
            blit_root((1080, 2340), target).as_coeffs(),
            Affine::scale(0.75).as_coeffs()
        );
    }

    #[test]
    fn blit_root_follows_the_rounded_up_target_not_the_requested_scale() {
        // 1179x2556 at 0.75: the width rounds up (884.25 -> 885) while the
        // height divides evenly, so the frame must be stretched slightly wider
        // than 0.75 to reach the intermediate's last column. A uniform
        // `scale(0.75)` root would stop short of it and the upscaling blit
        // would stretch that unpainted strip of base colour over the frame's
        // right edge.
        let target = scaled_size(1179, 2556, 0.75);
        assert_eq!(target, (885, 1917));
        let root = blit_root((1179, 2556), target);
        assert_eq!(
            root.as_coeffs(),
            Affine::scale_non_uniform(885.0 / 1179.0, 1917.0 / 2556.0).as_coeffs()
        );
        assert_ne!(root.as_coeffs(), Affine::scale(0.75).as_coeffs());
        // Per-axis, and exact at the far edge: the surface's last column/row
        // lands on the intermediate's last column/row, whichever axis rounded.
        let coeffs = root.as_coeffs();
        assert!(coeffs[0] > 0.75, "the rounded-up axis stretches further");
        assert_eq!(coeffs[3], 0.75, "the even axis is untouched");
        assert!((coeffs[0] * 1179.0 - 885.0).abs() < 1e-9);
        assert!((coeffs[3] * 2556.0 - 1917.0).abs() < 1e-9);
    }

    #[test]
    fn only_a_blit_arm_with_a_smaller_intermediate_is_scaled() {
        // The three answers `ConfiguredSurface::render_scaled` delegates here
        // for: a blit arm whose intermediate shrank, one at the surface's own
        // size, and a direct arm, which has no intermediate at all (a scale
        // below 1 forces the blit arm, so a direct surface is never scaled).
        let surface = (1080, 2400);
        assert!(path_render_scaled(
            surface,
            Some(scaled_size(surface.0, surface.1, 0.75))
        ));
        assert!(!path_render_scaled(
            surface,
            Some(scaled_size(surface.0, surface.1, 1.0))
        ));
        assert!(!path_render_scaled(surface, Some(surface)));
        assert!(!path_render_scaled(surface, None));
    }

    #[test]
    fn a_scaled_surface_agrees_with_the_root_it_encodes_under() {
        // The one-derivation contract: "scaled" and "the root" are two reads
        // of the same pair of sizes, so they can never disagree about a frame.
        for (surface, scale) in [
            ((1080, 2400), 0.75),
            ((1179, 2556), 0.75),
            ((720, 1600), 1.0),
        ] {
            let target = scaled_size(surface.0, surface.1, scale);
            let scaled = path_render_scaled(surface, Some(target));
            let root = blit_root(surface, target);
            assert_eq!(scaled, root.as_coeffs() != Affine::IDENTITY.as_coeffs());
        }
    }

    #[test]
    fn direct_path_chosen_only_when_both_caps_present_and_not_forced() {
        // The GO population (OP9/Xiaomi 12): Rgba8Unorm + STORAGE_BINDING, no
        // force → direct-to-surface.
        assert_eq!(
            choose_render_path(true, true, false),
            RenderPathKind::Direct
        );
    }

    #[test]
    fn force_blit_overrides_a_capable_surface() {
        // FRUST_NO_DIRECT_SURFACE, FRUST_RENDER_SCALE < 1, or cpu-tier pins a
        // fully-capable surface onto the blit arm — the fallback-proof safety
        // valve / A-B mechanism. The scaled case rides this same decision:
        // there is no scaled variant of either direct arm.
        assert_eq!(choose_render_path(true, true, true), RenderPathKind::Blit);
    }

    #[test]
    fn missing_either_cap_falls_back_to_blit() {
        // iPhone SE (Bgra8-only): no Rgba8Unorm → blit.
        assert_eq!(choose_render_path(false, true, false), RenderPathKind::Blit);
        // No STORAGE_BINDING → blit.
        assert_eq!(choose_render_path(true, false, false), RenderPathKind::Blit);
        // Neither → blit.
        assert_eq!(
            choose_render_path(false, false, false),
            RenderPathKind::Blit
        );
        // Neither, and forced → still blit (force never resurrects direct).
        assert_eq!(choose_render_path(false, false, true), RenderPathKind::Blit);
    }

    #[test]
    fn only_the_cpu_tier_forces_the_blit_arm() {
        // The R10 trap, pinned: the hybrid tier renders into the acquired
        // swapchain view, so forcing it onto the blit arm (and its
        // intermediate + `TextureBlitter`) would hand it a pass it never
        // asked for. Only the CPU tier, which uploads a pixmap into that
        // intermediate, is blit-only.
        assert!(!tier_forces_blit(crate::tier::RenderTier::Gpu));
        assert!(tier_forces_blit(crate::tier::RenderTier::Cpu));
        assert!(!tier_forces_blit(crate::tier::RenderTier::Hybrid));
    }

    #[test]
    fn a_tier_is_only_selectable_in_a_build_that_compiled_it() {
        // The default tier is always in; each experimental one tracks its own
        // feature — nothing inherits a blanket refusal.
        assert!(tier_compiled_in(crate::tier::RenderTier::Gpu));
        assert_eq!(
            tier_compiled_in(crate::tier::RenderTier::Cpu),
            cfg!(feature = "cpu-tier")
        );
        assert_eq!(
            tier_compiled_in(crate::tier::RenderTier::Hybrid),
            cfg!(feature = "hybrid-tier")
        );
    }

    #[cfg(feature = "hybrid-tier")]
    #[test]
    fn hybrid_render_path_is_never_blit() {
        // The hybrid tier owns no intermediate, so no input — not the
        // direct-surface probe, not `FRUST_NO_DIRECT_SURFACE`, not
        // `FRUST_RENDER_SCALE` — may route it through the blit arm. The
        // function takes no arguments precisely so this cannot be conditional;
        // the assertion is that the arm it names is not `Blit`.
        assert_eq!(choose_hybrid_render_path(), RenderPathKind::HybridDirect);
        assert_ne!(choose_hybrid_render_path(), RenderPathKind::Blit);
    }

    #[test]
    fn emulator_strips_debug_and_validation() {
        let build_flags = wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION;
        let flags = effective_instance_flags(build_flags, true);
        assert!(!flags.contains(wgpu::InstanceFlags::DEBUG));
        assert!(!flags.contains(wgpu::InstanceFlags::VALIDATION));
    }

    #[test]
    fn physical_device_keeps_debug_and_validation() {
        let build_flags = wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION;
        let flags = effective_instance_flags(build_flags, false);
        assert!(flags.contains(wgpu::InstanceFlags::DEBUG));
        assert!(flags.contains(wgpu::InstanceFlags::VALIDATION));
    }

    #[test]
    fn emulator_with_no_debug_flags_stays_empty() {
        let flags = effective_instance_flags(wgpu::InstanceFlags::empty(), true);
        assert!(flags.is_empty());
    }

    #[test]
    fn ios_simulator_bumps_alignment_to_256() {
        let base = wgpu::Limits::default();
        let limits = effective_limits(base.clone(), true);
        assert_eq!(limits.min_uniform_buffer_offset_alignment, 256);
        // Nothing else about the base limits should change.
        assert_eq!(
            wgpu::Limits {
                min_uniform_buffer_offset_alignment: base.min_uniform_buffer_offset_alignment,
                ..limits.clone()
            },
            base
        );
    }

    #[test]
    fn non_simulator_leaves_limits_untouched() {
        let base = wgpu::Limits::default();
        let limits = effective_limits(base.clone(), false);
        assert_eq!(limits, base);
    }

    #[test]
    fn base_already_at_or_above_256_is_not_lowered() {
        let base = wgpu::Limits {
            min_uniform_buffer_offset_alignment: 512,
            ..wgpu::Limits::default()
        };
        let limits = effective_limits(base.clone(), true);
        assert_eq!(limits.min_uniform_buffer_offset_alignment, 512);
        assert_eq!(limits, base);
    }

    #[test]
    fn simulator_alignment_uses_real_adapter_limits_not_defaults() {
        // A low-alignment adapter (the #7057 shape: Metal reports a lower
        // alignment than the simulator driver actually enforces) is bumped to
        // 256 while every other adapter-reported limit is preserved — the
        // whole point of feeding *adapter* limits rather than `Limits::default`.
        let adapter = wgpu::Limits {
            min_uniform_buffer_offset_alignment: 64,
            max_texture_dimension_2d: 4096,
            ..wgpu::Limits::default()
        };
        let limits = effective_limits(adapter.clone(), true);
        assert_eq!(limits.min_uniform_buffer_offset_alignment, 256);
        assert_eq!(limits.max_texture_dimension_2d, 4096);
    }

    #[test]
    fn first_n_uncaptured_errors_log() {
        for count in 1..=MAX_LOGGED_UNCAPTURED_ERRORS {
            assert_eq!(
                decide_log_action(count),
                LogAction::Log,
                "expected Log at count={count}"
            );
        }
    }

    #[test]
    fn nplus1_uncaptured_error_suppresses() {
        assert_eq!(
            decide_log_action(MAX_LOGGED_UNCAPTURED_ERRORS + 1),
            LogAction::SuppressionNotice
        );
    }

    #[test]
    fn further_uncaptured_errors_stay_silent_between_debug_bumps() {
        let past_notice = MAX_LOGGED_UNCAPTURED_ERRORS + 2;
        assert_eq!(
            decide_log_action(past_notice),
            LogAction::Silent { debug_bump: false }
        );
    }

    #[test]
    fn uncaptured_error_debug_bump_is_periodic() {
        assert_eq!(
            decide_log_action(UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD),
            LogAction::Silent { debug_bump: true }
        );
        assert_eq!(
            decide_log_action(UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD * 2),
            LogAction::Silent { debug_bump: true }
        );
        assert_eq!(
            decide_log_action(UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD + 1),
            LogAction::Silent { debug_bump: false }
        );
    }

    /// Builds a synthetic `wgpu::SurfaceCapabilities` reporting only the given
    /// `alpha_modes` — the rest of the struct is irrelevant to
    /// `resolve_alpha_mode`, which reads only that one field.
    fn caps_with_alpha_modes(
        alpha_modes: &[wgpu::CompositeAlphaMode],
    ) -> wgpu::SurfaceCapabilities {
        wgpu::SurfaceCapabilities {
            alpha_modes: alpha_modes.to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn opaque_request_always_resolves_to_auto() {
        // `Opaque` never consults `alpha_modes` — bit-for-bit today's behavior
        // regardless of what the surface reports.
        for modes in [
            [wgpu::CompositeAlphaMode::Inherit].as_slice(),
            &[
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::PostMultiplied,
            ],
            &[wgpu::CompositeAlphaMode::Opaque],
        ] {
            let caps = caps_with_alpha_modes(modes);
            assert_eq!(
                resolve_alpha_mode(SurfaceAlphaRequest::Opaque, &caps),
                wgpu::CompositeAlphaMode::Auto
            );
        }
    }

    #[test]
    fn translucent_preferred_picks_inherit_first() {
        // Android's observed shape: `Inherit` is the only reported mode.
        let caps = caps_with_alpha_modes(&[wgpu::CompositeAlphaMode::Inherit]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::Inherit
        );
    }

    #[test]
    fn translucent_preferred_picks_post_multiplied_when_inherit_absent() {
        // iOS's observed shape: `[Opaque, PostMultiplied]` — no `Inherit`.
        let caps = caps_with_alpha_modes(&[
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::PostMultiplied,
        ]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::PostMultiplied
        );
    }

    #[test]
    fn translucent_preferred_falls_back_to_auto_when_none_available() {
        // A surface reporting only `Opaque` (no `Inherit`/`PostMultiplied`/
        // `PreMultiplied`) can't satisfy translucency — fall back to `Auto`
        // rather than erroring.
        let caps = caps_with_alpha_modes(&[wgpu::CompositeAlphaMode::Opaque]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::Auto
        );
    }

    #[test]
    fn translucent_preferred_falls_back_to_pre_multiplied_last() {
        // Neither `Inherit` nor `PostMultiplied` present, but `PreMultiplied`
        // is — the third preference in the resolution order.
        let caps = caps_with_alpha_modes(&[
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::PreMultiplied,
        ]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::PreMultiplied
        );
    }

    #[test]
    fn resolved_translucency_is_true_only_for_the_three_translucent_modes() {
        // The wgpu-free projection the shells' paint contract keys off:
        // exactly `resolve_alpha_mode`'s preference list.
        for mode in [
            wgpu::CompositeAlphaMode::Inherit,
            wgpu::CompositeAlphaMode::PostMultiplied,
            wgpu::CompositeAlphaMode::PreMultiplied,
        ] {
            assert!(alpha_mode_is_translucent(mode), "{mode:?}");
        }
        // `Auto` is what BOTH an opaque request and a failed translucent
        // resolution land on — neither composites alpha.
        for mode in [
            wgpu::CompositeAlphaMode::Auto,
            wgpu::CompositeAlphaMode::Opaque,
        ] {
            assert!(!alpha_mode_is_translucent(mode), "{mode:?}");
        }
    }

    /// The snapshot compositor's refusal case, over every mode
    /// `resolve_alpha_mode` can produce: `PostMultiplied` alone is translucent
    /// AND straight-alpha, so it is the only one that keeps brackets inline.
    #[test]
    fn only_post_multiplied_is_translucent_with_straight_alpha() {
        assert!(alpha_mode_is_straight_translucent(
            wgpu::CompositeAlphaMode::PostMultiplied
        ));
        // Translucent but premultiplied: `DirectPremultiplied` plus the
        // compositor's premultiplied arm, whose algebra closes for every
        // destination alpha.
        for mode in [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::Inherit,
        ] {
            assert!(
                alpha_mode_needs_premultiply(mode),
                "{mode:?} must take the premultiply pass"
            );
            assert!(!alpha_mode_is_straight_translucent(mode), "{mode:?}");
        }
        // Not translucent at all: the opaque destination the straight blend is
        // exact for.
        for mode in [
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::Auto,
        ] {
            assert!(!alpha_mode_is_straight_translucent(mode), "{mode:?}");
        }
    }

    #[test]
    fn forced_mismatch_translucent_request_resolves_not_translucent() {
        // A surface whose
        // advertised capabilities carry NO translucent mode, asked for
        // translucency. The request is honored as far as it can be (`Auto`),
        // but the RESOLVED translucency — the value
        // `SurfaceRenderer::surface_resolved_translucent` hands the shells — is
        // `false`, so the shells keep the opaque (Mode A) paint contract: an
        // opaque base color and no `ClearRect` punch (the encode-level half of
        // this claim lives in `convert.rs`'s
        // `mode_a_scene_encodes_no_clear_rect_even_with_a_slot_sized_region`).
        for modes in [
            [wgpu::CompositeAlphaMode::Opaque].as_slice(),
            &[wgpu::CompositeAlphaMode::Auto],
            &[],
        ] {
            let caps = caps_with_alpha_modes(modes);
            let resolved = resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps);
            assert_eq!(resolved, wgpu::CompositeAlphaMode::Auto, "{modes:?}");
            assert!(
                !alpha_mode_is_translucent(resolved),
                "a fallback-to-opaque surface must never report translucent ({modes:?})"
            );
        }
    }

    #[test]
    fn happy_path_translucent_request_resolves_translucent() {
        // The shipped configs stay unchanged: Android (`Inherit`-only) and iOS
        // (`[Opaque, PostMultiplied]`) both resolve to a translucent mode, so
        // the punch + transparent base keep running exactly as today.
        for modes in [
            [wgpu::CompositeAlphaMode::Inherit].as_slice(),
            &[
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::PostMultiplied,
            ],
        ] {
            let resolved = resolve_alpha_mode(
                SurfaceAlphaRequest::TranslucentPreferred,
                &caps_with_alpha_modes(modes),
            );
            assert!(alpha_mode_is_translucent(resolved), "{modes:?}");
        }
        // An opaque request never reports translucent, whatever the surface
        // advertises.
        assert!(!alpha_mode_is_translucent(resolve_alpha_mode(
            SurfaceAlphaRequest::Opaque,
            &caps_with_alpha_modes(&[wgpu::CompositeAlphaMode::Inherit]),
        )));
    }

    #[test]
    fn premultiply_gated_on_premultiplied_expecting_modes() {
        // The two premultiplied-expecting modes need the pass:
        // `Inherit` is Android's translucent mode (SurfaceFlinger blends
        // premultiplied); `PreMultiplied` is explicit.
        assert!(alpha_mode_needs_premultiply(
            wgpu::CompositeAlphaMode::Inherit
        ));
        assert!(alpha_mode_needs_premultiply(
            wgpu::CompositeAlphaMode::PreMultiplied
        ));
        // Straight-expecting (iOS `PostMultiplied`) and alpha-ignoring
        // (`Opaque`/`Auto`, every opaque surface) take vello's output unchanged
        // — the pass must never touch them, or the opaque path stops being
        // byte-identical.
        assert!(!alpha_mode_needs_premultiply(
            wgpu::CompositeAlphaMode::PostMultiplied
        ));
        assert!(!alpha_mode_needs_premultiply(
            wgpu::CompositeAlphaMode::Opaque
        ));
        assert!(!alpha_mode_needs_premultiply(
            wgpu::CompositeAlphaMode::Auto
        ));
    }

    #[test]
    fn gpu_tier_blit_refuses_premultiplied_expecting_translucency() {
        // A GPU-tier surface forced onto the blit
        // arm (no Rgba8Unorm+STORAGE_BINDING) that resolves a
        // premultiplied-expecting alpha mode (Android's `Inherit`) must
        // refuse translucency — the blit arm's `TextureBlitter::copy` is a
        // fixed pass-through sampled draw with no premultiply stage frust can
        // reach, so presenting it straight would reproduce the
        // over-bright-fringing defect.
        assert!(blit_translucency_refused(
            RenderPathKind::Blit,
            wgpu::CompositeAlphaMode::Inherit,
            crate::tier::RenderTier::Gpu,
        ));
        assert!(blit_translucency_refused(
            RenderPathKind::Blit,
            wgpu::CompositeAlphaMode::PreMultiplied,
            crate::tier::RenderTier::Gpu,
        ));
    }

    #[test]
    fn cpu_tier_blit_stays_translucent_capable() {
        // The regression guard that matters most: cpu-tier ALSO forces the
        // blit arm (`force_blit`), but `vello_cpu`'s output is already
        // premultiplied (`cpu_tier.rs:108-113`'s `PremulRgba8`), so refusal
        // must NOT fire for it — forcing it opaque would be a
        // self-inflicted regression on a path that is correct today.
        assert!(!blit_translucency_refused(
            RenderPathKind::Blit,
            wgpu::CompositeAlphaMode::Inherit,
            crate::tier::RenderTier::Cpu,
        ));
        assert!(!blit_translucency_refused(
            RenderPathKind::Blit,
            wgpu::CompositeAlphaMode::PreMultiplied,
            crate::tier::RenderTier::Cpu,
        ));
    }

    #[test]
    fn blit_refusal_never_fires_for_non_premultiplied_modes_or_direct_path() {
        // Straight-expecting/alpha-ignoring modes never need refusal, on
        // either path kind.
        for mode in [
            wgpu::CompositeAlphaMode::PostMultiplied,
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::Auto,
        ] {
            assert!(!blit_translucency_refused(
                RenderPathKind::Blit,
                mode,
                crate::tier::RenderTier::Gpu
            ));
        }
        // The Direct arm is unaffected regardless of tier/mode — it already
        // has its own premultiply pass (`RenderPath::DirectPremultiplied`),
        // so byte-identical direct-path behavior is preserved.
        for mode in [
            wgpu::CompositeAlphaMode::Inherit,
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied,
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::Auto,
        ] {
            assert!(!blit_translucency_refused(
                RenderPathKind::Direct,
                mode,
                crate::tier::RenderTier::Gpu
            ));
        }
    }

    #[cfg(feature = "hybrid-tier")]
    #[test]
    fn hybrid_refuses_straight_alpha_translucency() {
        // `vello_hybrid` presents PREMULTIPLIED alpha, so the ONE surface it
        // cannot serve is the straight-alpha translucent one (iOS's
        // `PostMultiplied`), which would read `(C·a, a)` as `(C, a)` — too
        // dark over a Mode B hole. The blit refusal cannot cover this: it
        // gates on `RenderPathKind::Blit`, an arm this tier never takes.
        assert!(hybrid_translucency_refused(
            RenderPathKind::HybridDirect,
            wgpu::CompositeAlphaMode::PostMultiplied,
        ));
        assert!(!blit_translucency_refused(
            RenderPathKind::HybridDirect,
            wgpu::CompositeAlphaMode::PostMultiplied,
            crate::tier::RenderTier::Hybrid,
        ));
        // Refused therefore beats the fall-through: `create_render_surface`
        // stores `resolved_translucent = false`, which is verbatim what
        // `SurfaceRenderer::surface_resolved_translucent` hands the shells, so
        // they keep the opaque Mode A contract on this combination even though
        // the mode itself is translucent.
        assert!(alpha_mode_is_translucent(
            wgpu::CompositeAlphaMode::PostMultiplied
        ));
    }

    #[cfg(feature = "hybrid-tier")]
    #[test]
    fn hybrid_keeps_premultiplied_expecting_surfaces_translucent() {
        // The regression guard that matters most, and the inversion this
        // predicate exists to fix: Android's `Inherit` (and the explicit
        // `PreMultiplied`) expect premultiplied and RECEIVE premultiplied from
        // this tier — correct as-is. Neither refusal may fire, and the
        // fall-through must keep reporting translucent, or a shipped Android
        // translucent surface would silently lose its platform-view punch.
        for mode in [
            wgpu::CompositeAlphaMode::Inherit,
            wgpu::CompositeAlphaMode::PreMultiplied,
        ] {
            assert!(
                !hybrid_translucency_refused(RenderPathKind::HybridDirect, mode),
                "{mode:?} is exactly what this tier already emits"
            );
            assert!(!blit_translucency_refused(
                RenderPathKind::HybridDirect,
                mode,
                crate::tier::RenderTier::Hybrid,
            ));
            assert!(alpha_mode_is_translucent(mode), "{mode:?}");
        }
        // Alpha-ignoring modes are untouched — the opaque surface every
        // measured hybrid number was taken on.
        for mode in [
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::Auto,
        ] {
            assert!(!hybrid_translucency_refused(
                RenderPathKind::HybridDirect,
                mode
            ));
            assert!(!alpha_mode_is_translucent(mode), "{mode:?}");
        }
    }

    #[cfg(feature = "hybrid-tier")]
    #[test]
    fn hybrid_refusal_never_reaches_a_vello_arm() {
        // The `path_kind` parameter is what keeps this predicate off vello's
        // two arms: their output is straight, so `PostMultiplied` is the one
        // mode they need no help with, and a refusal here would take
        // translucency away from a surface that works today.
        for path_kind in [RenderPathKind::Direct, RenderPathKind::Blit] {
            for mode in [
                wgpu::CompositeAlphaMode::PostMultiplied,
                wgpu::CompositeAlphaMode::Inherit,
                wgpu::CompositeAlphaMode::PreMultiplied,
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::Auto,
            ] {
                assert!(
                    !hybrid_translucency_refused(path_kind, mode),
                    "{path_kind:?}/{mode:?}"
                );
            }
        }
    }

    /// The exact premultiply arithmetic the [`PremultiplyPass`] shader performs,
    /// as an always-run reference (no GPU): a 50%-alpha `#FFF176` painted over
    /// a Mode B hole must reach a premultiplied-expecting compositor stored
    /// premultiplied (`~#807B3B@128`), NOT straight (`#FFF176@128` — the
    /// over-bright value the device measured before this fix). Encodes the
    /// predicted-correct pixel a device re-check must confirm.
    #[test]
    fn d3_premultiply_math_matches_verify_predictions() {
        // `premultiply` in PREMULTIPLY_WGSL is `rgb * a` in normalized [0,1];
        // in 8-bit that is `round(c * a / 255)`.
        fn premul8(c: u8, a: u8) -> u8 {
            ((u32::from(c) * u32::from(a) + 127) / 255) as u8
        }

        // 50%-alpha #FFF176 (a translucent ghost-button fill color). a=128 ≈ 0.502.
        let (r, g, b, a) = (0xFF, 0xF1, 0x76, 128);
        let premul = [premul8(r, a), premul8(g, a), premul8(b, a), a];
        // Straight (the WRONG, over-bright value) keeps rgb at full intensity.
        assert_eq!([r, g, b, a], [0xFF, 0xF1, 0x76, 128]);
        // Premultiplied is materially darker per channel — this is the fix.
        assert_eq!(premul, [0x80, 0x79, 0x3B, 128]);
        // Within a couple of LSB of the predicted `~#807B3B@128`.
        assert!((i16::from(premul[1]) - 0x7B).abs() <= 2);

        // A second point on the curve: 25%-alpha opaque-magenta debris fill
        // (an `#E4..FF`-class over-bright color). a=64 ≈ 0.251.
        let a = 64;
        assert_eq!(
            [premul8(0xFF, a), premul8(0x00, a), premul8(0xFF, a), a],
            [0x40, 0x00, 0x40, 64]
        );
        // Fully-opaque pixels are premultiply-invariant — this is why the
        // opaque path stays byte-identical and in-scene blends over opaque
        // content already composite correctly.
        assert_eq!(
            [premul8(0xFF, 255), premul8(0xF1, 255), premul8(0x76, 255)],
            [0xFF, 0xF1, 0x76]
        );
    }

    // ---- GPU-dependent tests ----
    //
    // Each drives real adapter work, so each is `#[ignore]`d and run by hand
    // on a host with a GPU (`cargo test -p frust-render -- --ignored`; Metal
    // locally). A sandbox without an adapter skips them rather than failing:
    //
    // - `premultiply_pass_converts_vello_straight_output_to_premultiplied` —
    //   vello's straight output really is straight, and `PremultiplyPass`
    //   converts it.
    // - `scaled_blit_stretches_the_intermediate_over_the_whole_target` — a
    //   smaller intermediate reaches every pixel of the swapchain-sized
    //   target through the blit, leaving no edge strip behind.
    // - `a_narrowed_aa_support_renders_the_mode_it_was_built_for` — a renderer
    //   whose `AaSupport` was narrowed to one mode renders that mode.
    //
    // The shared opener is `gpu_device`; readback of an arbitrary-width
    // texture goes through `read_rgba`.

    /// Real-GPU end-to-end confirmation of the premultiply fix on this host's
    /// Vulkan adapter: vello's `render_to_texture` emits **straight** alpha, and
    /// [`PremultiplyPass`] converts it to premultiplied — the exact operation
    /// [`RenderPath::DirectPremultiplied`] inserts before presenting a
    /// premultiplied-expecting translucent swapchain (Android `Inherit`). No
    /// on-screen surface exists headlessly, so this drives the two GPU halves
    /// (vello output + the compute pass) directly against owned textures rather
    /// than through `submit`.
    ///
    /// Encodes the predicted `#FFF176@128` behavior: asserts (a) vello stores
    /// the fill STRAIGHT (`r≈255`, proving the diagnosis — the output really is
    /// non-premultiplied), then (b) the pass stores it PREMULTIPLIED
    /// (`r≈128 = round(255 * 128/255)`, proving the fix), each within a couple
    /// of LSB of the value derived from vello's own readback.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn premultiply_pass_converts_vello_straight_output_to_premultiplied() {
        pollster::block_on(run());

        async fn run() {
            const SIZE: u32 = 64; // 64*4 = 256-byte rows: no copy-padding math.

            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("no compatible GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust premultiply test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create device");

            let make_tex = |label: &str, extra: wgpu::TextureUsages| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
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
                        | wgpu::TextureUsages::COPY_SRC
                        | extra,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    view_formats: &[],
                })
            };
            // vello's straight output target (also the premultiply pass's read
            // source), and the pass's write destination (the "swapchain").
            let straight_tex = make_tex("straight (vello)", wgpu::TextureUsages::empty());
            let straight_view = straight_tex.create_view(&wgpu::TextureViewDescriptor::default());
            let premul_tex = make_tex("premultiplied (out)", wgpu::TextureUsages::empty());
            let premul_view = premul_tex.create_view(&wgpu::TextureViewDescriptor::default());

            // A 50%-alpha #FFF176 fill over a fully-transparent base — a
            // translucent ghost button over a Mode B hole (alpha-0 behind).
            let mut fk_scene = frust_scene::Scene::new();
            {
                let mut builder = frust_scene::SceneBuilder::new(&mut fk_scene);
                builder.fill_rect(
                    kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
                    peniko::Brush::Solid(peniko::Color::from_rgba8(0xFF, 0xF1, 0x76, 128)),
                );
            }
            let mut vello_scene = vello::Scene::new();
            crate::encode_scene(&fk_scene, &mut vello_scene);
            let mut renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
                .expect("failed to create vello renderer");
            renderer
                .render_to_texture(
                    &device,
                    &queue,
                    &vello_scene,
                    &straight_view,
                    &vello::RenderParams {
                        base_color: peniko::Color::TRANSPARENT,
                        width: SIZE,
                        height: SIZE,
                        antialiasing_method: vello::AaConfig::Area,
                    },
                )
                .expect("render_to_texture failed");

            // Run the fix: straight → premultiplied into the destination.
            let pass = PremultiplyPass::new(&device);
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            pass.record(
                &device,
                &mut encoder,
                &straight_view,
                &premul_view,
                SIZE,
                SIZE,
            );
            queue.submit([encoder.finish()]);

            let straight = read_centre(&device, &queue, &straight_tex, SIZE).await;
            let premul = read_centre(&device, &queue, &premul_tex, SIZE).await;

            // (a) vello stored STRAIGHT alpha: rgb near full intensity, a≈128.
            // This is the diagnosis — the un-premultiply in fine.wgsl.
            assert!(
                straight[0] >= 250 && (i16::from(straight[3]) - 128).abs() <= 2,
                "expected vello straight output (~#FFF176@128), got {straight:?}"
            );
            // (b) the pass stored PREMULTIPLIED alpha, derived from vello's own
            // straight readback so the assertion is robust to vello's rounding:
            // premul_c ≈ round(straight_c * a/255).
            let a = u32::from(straight[3]);
            for ch in 0..3 {
                let expected = ((u32::from(straight[ch]) * a + 127) / 255) as i16;
                assert!(
                    (i16::from(premul[ch]) - expected).abs() <= 2,
                    "channel {ch}: premultiplied {} not within 2 LSB of expected {expected} \
                     (straight {straight:?} -> premul {premul:?})",
                    premul[ch]
                );
            }
            // Alpha is unchanged by premultiply; red must have visibly darkened
            // (255 -> ~128), proving the pass actually ran.
            assert_eq!(premul[3], straight[3], "premultiply must not change alpha");
            assert!(
                premul[0] < 160,
                "red should be premultiplied down toward 128, got {}",
                premul[0]
            );
        }

        /// Copies the centre texel of `tex` back to the CPU as `[r,g,b,a]`.
        async fn read_centre(
            device: &wgpu::Device,
            queue: &wgpu::Queue,
            tex: &wgpu::Texture,
            size: u32,
        ) -> [u8; 4] {
            let bytes_per_row = size * 4;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("frust premultiply readback"),
                size: u64::from(bytes_per_row * size),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(size),
                    },
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
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
            let centre = ((size / 2) * bytes_per_row + (size / 2) * 4) as usize;
            [
                data[centre],
                data[centre + 1],
                data[centre + 2],
                data[centre + 3],
            ]
        }
    }

    /// The scaled blit end to end on this host's adapter: an intermediate
    /// smaller than the target, filled edge to edge, must reach EVERY pixel of
    /// the target through the blitter — the upscale [`RenderPath::Blit`]
    /// performs once its intermediate has been shrunk. A destination pixel
    /// left at the clear colour would be exactly the strip a target the frame
    /// does not quite cover leaves behind (see [`blit_root`]).
    ///
    /// Runs both roundings [`scaled_size`] can produce: 100 halves evenly to
    /// 50, while 65 rounds up to 33 — the odd case, where the intermediate
    /// covers slightly MORE than the requested fraction.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn scaled_blit_stretches_the_intermediate_over_the_whole_target() {
        pollster::block_on(run());

        async fn run() {
            const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
            let (device, queue) = gpu_device("frust scaled blit test").await;

            for size in [100u32, 65] {
                let target = scaled_size(size, size, 0.5);
                assert!(
                    target.0 < size && target.1 < size,
                    "the intermediate must really be smaller for this to test an upscale"
                );
                let make_tex = |label: &str, (w, h): (u32, u32), extra| {
                    device.create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: w,
                            height: h,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | extra,
                        format: FORMAT,
                        view_formats: &[],
                    })
                };
                // The blit arm's two textures: the intermediate vello renders
                // into (sampled by the blitter) and the swapchain-sized
                // destination it is copied onto.
                let intermediate = make_tex(
                    "frust blit smoke intermediate",
                    target,
                    wgpu::TextureUsages::TEXTURE_BINDING,
                );
                let destination = make_tex(
                    "frust blit smoke destination",
                    (size, size),
                    wgpu::TextureUsages::COPY_SRC,
                );
                let intermediate_view =
                    intermediate.create_view(&wgpu::TextureViewDescriptor::default());
                let destination_view =
                    destination.create_view(&wgpu::TextureViewDescriptor::default());

                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                // Stand in for the frame: the whole intermediate opaque red,
                // so any destination pixel the blit fails to cover shows up as
                // the destination's own (black, transparent) initial content.
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("frust blit smoke fill"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &intermediate_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::RED),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                }
                // The blitter the scaled arm builds: a sampled full-screen
                // draw over the whole destination, `Linear` because the source
                // is smaller ([`blit_filter`]).
                wgpu::util::TextureBlitterBuilder::new(&device, FORMAT)
                    .sample_type(blit_filter(true))
                    .build()
                    .copy(&device, &mut encoder, &intermediate_view, &destination_view);
                queue.submit([encoder.finish()]);

                let pixels = read_rgba(&device, &queue, &destination, size, size).await;
                assert_eq!(pixels.len(), (size * size) as usize);
                for (index, pixel) in pixels.iter().enumerate() {
                    assert!(
                        pixel[0] >= 250 && pixel[1] <= 5 && pixel[2] <= 5 && pixel[3] >= 250,
                        "{size}x{size} target from a {}x{} intermediate: pixel \
                         ({}, {}) is {pixel:?}, not opaque red — the blit left it uncovered",
                        target.0,
                        target.1,
                        index as u32 % size,
                        index as u32 / size,
                    );
                }
            }
        }
    }

    /// A `vello::Renderer` built for ONE anti-aliasing mode renders that mode:
    /// the narrowing [`AaMode::support`] performs (instead of compiling every
    /// `AaConfig`'s pipelines) must still leave the mode the frame asks for
    /// present, or every pass would fail at `render_to_texture`.
    ///
    /// Msaa8 is the interesting case — it is the mode the default `Area` build
    /// does NOT compile, so a renderer that renders it here really was built
    /// for it. Metal is a correct MSAA implementation; the corruption this
    /// mode shows on one Adreno part is device-specific and registered
    /// (`render-measurement-knobs-not-shipping-modes`), so it is no reason to
    /// leave the seam untested here.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn a_narrowed_aa_support_renders_the_mode_it_was_built_for() {
        pollster::block_on(run());

        async fn run() {
            const SIZE: u32 = 64;
            let mode = AaMode::Msaa8;
            let (device, queue) = gpu_device("frust narrowed aa test").await;

            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frust aa smoke target"),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // vello renders by compute storage write; the readback needs
                // the copy source.
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

            // An opaque red rect inset from every edge, so the frame carries
            // both filled and anti-aliased pixels.
            let mut fk_scene = frust_scene::Scene::new();
            {
                let mut builder = frust_scene::SceneBuilder::new(&mut fk_scene);
                builder.fill_rect(
                    kurbo::Rect::new(8.0, 8.0, 56.0, 56.0),
                    peniko::Brush::Solid(peniko::Color::from_rgba8(0xFF, 0x00, 0x00, 0xFF)),
                );
            }
            let mut vello_scene = vello::Scene::new();
            crate::encode_scene(&fk_scene, &mut vello_scene);

            let mut renderer = vello::Renderer::new(
                &device,
                vello::RendererOptions {
                    antialiasing_support: mode.support(),
                    ..Default::default()
                },
            )
            .expect("failed to create a vello renderer with a narrowed AaSupport");
            renderer
                .render_to_texture(
                    &device,
                    &queue,
                    &vello_scene,
                    &view,
                    &vello::RenderParams {
                        base_color: peniko::Color::BLACK,
                        width: SIZE,
                        height: SIZE,
                        antialiasing_method: mode.to_vello(),
                    },
                )
                .unwrap_or_else(|e| panic!("render_to_texture failed for aa-mode={mode}: {e}"));

            let pixels = read_rgba(&device, &queue, &texture, SIZE, SIZE).await;
            let red = pixels
                .iter()
                .filter(|p| p[0] >= 250 && p[1] <= 5 && p[2] <= 5)
                .count();
            assert!(
                red > 0,
                "aa-mode={mode} rendered no red pixels at all — the narrowed \
                 support did not render the mode it was built for"
            );
        }
    }

    /// The adapter + device the GPU tests above open with: this host's default
    /// adapter, and a device at default limits.
    async fn gpu_device(label: &str) -> (wgpu::Device, wgpu::Queue) {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect("no compatible GPU adapter");
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some(label),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("failed to create device")
    }

    /// Copies all of `tex` back to the CPU as one `[r,g,b,a]` per pixel,
    /// un-padding the row alignment `copy_texture_to_buffer` requires: an
    /// arbitrary width does not land on it (100 pixels is 400 bytes, and the
    /// copy demands a multiple of 256), unlike the 64-wide targets whose rows
    /// are exactly one alignment unit.
    async fn read_rgba(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tex: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Vec<[u8; 4]> {
        let row_bytes = width * 4;
        let padded_row_bytes = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frust gpu-test readback"),
            size: u64::from(padded_row_bytes * height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
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
        (0..height)
            .flat_map(|row| {
                let start = (row * padded_row_bytes) as usize;
                data[start..start + row_bytes as usize]
                    .chunks_exact(4)
                    .map(|texel| [texel[0], texel[1], texel[2], texel[3]])
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}
