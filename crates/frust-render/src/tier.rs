//! Render-tier selection seam.
//!
//! [`RenderTier::Gpu`] is the vello 0.9 GPU compute path (the only tier a
//! device is actually created for today); [`RenderTier::Cpu`] is the
//! experimental `vello_cpu` fallback, selectable only when the `cpu-tier`
//! feature is compiled in (not default — see `frust-render/Cargo.toml`)
//! and not yet wired into device creation (that lands alongside the
//! `cpu-tier` encode path). [`RenderTier::Hybrid`] is the `vello_hybrid`
//! sparse-strip path behind the equally non-default `hybrid-tier` feature: a
//! measurement spike that is never probed for — only an explicit override
//! selects it, and only in a build that compiled the feature in.
//!
//! [`select_render_tier`] is pure decision logic over [`TierCaps`] (a plain
//! struct a caller builds from a real `wgpu::Adapter`'s downlevel flags +
//! name), so it is unit-testable with fake caps and needs no GPU — mirroring
//! `context.rs`'s split of pure decision vs. platform lookup (e.g.
//! `effective_instance_flags`/`is_android_emulator`). [`RenderContext`](crate::RenderContext)
//! is the one production call site, consulting it at device-init time and
//! reusing [`TierSelection::diagnosis`] as its single fail-fast message
//! (previously a bespoke check duplicated here and in `context.rs`).

/// Which Vello backend the renderer drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RenderTier {
    /// GPU compute path (`vello` 0.9). Requires
    /// [`GPU_REQUIRED_DOWNLEVEL_FLAGS`].
    #[default]
    Gpu,
    /// Experimental CPU fallback (`vello_cpu` 0.0.9), only selectable when
    /// the `cpu-tier` feature is compiled in. Not yet wired
    /// into device creation.
    Cpu,
    /// Experimental `vello_hybrid` 0.2 path (paths rasterized into sparse
    /// strips on the CPU, composited on the GPU), only selectable when the
    /// `hybrid-tier` feature is compiled in.
    ///
    /// Never fallen back to: [`select_render_tier`] reaches this variant only
    /// through an explicit override, because the tier exists to be *measured*
    /// against [`Self::Gpu`] on the same hardware. A tier that could also be
    /// selected by a probe would make "which tier did this capture run?" a
    /// question about the adapter rather than about the command line.
    Hybrid,
}

/// Plain-data capability inputs [`select_render_tier`] probes. Built from a
/// real `wgpu::Adapter` at [`RenderContext::ensure_device`](crate::RenderContext)'s
/// call site, but constructible by hand in tests with no GPU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierCaps {
    /// The adapter's `wgpu::DownlevelCapabilities::flags`.
    pub downlevel_flags: wgpu::DownlevelFlags,
    /// The adapter's `wgpu::AdapterInfo::name`, used only for diagnostics.
    pub adapter_name: String,
}

/// The [`wgpu::DownlevelFlags`] the GPU tier (vello 0.9) cannot run without:
///
/// - `COMPUTE_SHADERS`: vello's rendering pipeline is a compute shader.
/// - `INDIRECT_EXECUTION`: vello unconditionally allocates its working
///   buffers with `BufferUsages::INDIRECT` (see `wgpu_engine.rs` — "TODO:
///   only some buffers will need indirect"), and wgpu rejects creating any
///   INDIRECT-usage buffer on a device whose adapter lacks this flag.
///
/// The **iOS Simulator** is the known offender for the latter: wgpu-hal 29
/// gates `INDIRECT_EXECUTION` on the `iOS_GPUFamily3_v1`/`macOS_GPUFamily1_v1`
/// Metal feature sets, but the simulator only exposes the `Apple2` GPU
/// family, so the flag is never set — a wgpu-29/vello-0.9-level limitation
/// (a sibling of [wgpu #7057](https://github.com/gfx-rs/wgpu/issues/7057))
/// that cannot be patched under the workspace's version pin.
pub const GPU_REQUIRED_DOWNLEVEL_FLAGS: wgpu::DownlevelFlags =
    wgpu::DownlevelFlags::COMPUTE_SHADERS.union(wgpu::DownlevelFlags::INDIRECT_EXECUTION);

/// The [`wgpu::DownlevelFlags`] the hybrid tier (`vello_hybrid` 0.2) cannot
/// run without: **none of them**.
///
/// The emptiness is the point, not an oversight. `vello_hybrid` rasterizes
/// paths into sparse strips on the CPU and composites them with ordinary
/// render passes, so it needs neither `COMPUTE_SHADERS` nor
/// `INDIRECT_EXECUTION` — precisely the two flags
/// [`GPU_REQUIRED_DOWNLEVEL_FLAGS`] demands and the iOS Simulator's `Apple2`
/// GPU family cannot supply. An adapter that fails the GPU probe therefore
/// raises no capability objection to this tier; the only thing that can refuse
/// it is a build without the `hybrid-tier` feature.
pub const HYBRID_REQUIRED_DOWNLEVEL_FLAGS: wgpu::DownlevelFlags = wgpu::DownlevelFlags::empty();

/// What [`select_render_tier`] decided is actually usable in this build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TierOutcome {
    /// `tier` is usable — either it probed capable, or an explicit override
    /// selected it. An override wins **among available tiers**: a `Cpu` override
    /// always applies, but a `Gpu` override still requires the adapter to support
    /// [`GPU_REQUIRED_DOWNLEVEL_FLAGS`] — an incapable `Gpu` override is
    /// [`TierOutcome::Unavailable`], not `Available`.
    Available(RenderTier),
    /// No tier is usable in this build. `would_be` names the tier that
    /// *would* have been selected had it been compiled in — a diagnosed
    /// failure naming the tier that would apply, e.g. `Cpu` when the adapter
    /// lacks the GPU tier's flags but the `cpu-tier` feature is off.
    Unavailable { would_be: RenderTier },
}

/// The result of [`select_render_tier`]: the decided [`TierOutcome`] plus a
/// human-readable diagnosis — the one message a failed GPU probe surfaces
/// through (see [`RenderContext::ensure_device`](crate::RenderContext), the
/// production call site).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierSelection {
    pub outcome: TierOutcome,
    pub diagnosis: String,
}

/// Selects the render tier.
///
/// - An explicit `override_tier` wins **among available tiers**: a `Cpu`
///   override always applies (the CPU tier has no adapter prerequisite), but a
///   `Gpu` override applies only when `caps` actually support
///   [`GPU_REQUIRED_DOWNLEVEL_FLAGS`]. A `Gpu` override onto an incapable
///   adapter is **refused** — [`TierOutcome::Unavailable`] naming `Gpu` — since
///   an override cannot conjure a missing GPU capability; the caller then keeps
///   the same fail-fast behavior a failed probe produces (the override
///   plumbing: `FRUST_RENDER_TIER` / `frust run --render-tier`).
/// - Without an override: `caps` supporting [`GPU_REQUIRED_DOWNLEVEL_FLAGS`]
///   selects [`RenderTier::Gpu`]. Otherwise, if the `cpu-tier` feature is
///   compiled in, falls back to [`RenderTier::Cpu`] (still experimental, and
///   not yet wired into device creation — see this module's docs); without
///   the feature, the result is [`TierOutcome::Unavailable`] naming `Cpu` as
///   the tier that would apply, and the caller keeps today's fail-fast
///   behavior using [`TierSelection::diagnosis`].
/// - [`RenderTier::Hybrid`] is **override-only**: no probe ever selects it, so
///   the fallback above still chooses between `Gpu` and `Cpu` exactly as
///   before. A `Hybrid` override applies only in a build that compiled the
///   `hybrid-tier` feature in; without it the override is refused
///   ([`TierOutcome::Unavailable`] naming `Hybrid`) rather than ignored, so a
///   capture asked for on the hybrid tier can never be a vello frame wearing
///   the wrong label.
pub fn select_render_tier(caps: &TierCaps, override_tier: Option<RenderTier>) -> TierSelection {
    let gpu_capable = caps.downlevel_flags.contains(GPU_REQUIRED_DOWNLEVEL_FLAGS);
    // Always true today: [`HYBRID_REQUIRED_DOWNLEVEL_FLAGS`] is empty by
    // design. Asked of `caps` in the same shape `gpu_capable` is anyway, so
    // the requirement lives in one named constant rather than in a comment —
    // and a flag ever added to it is honoured here without a second edit.
    let hybrid_capable = caps
        .downlevel_flags
        .contains(HYBRID_REQUIRED_DOWNLEVEL_FLAGS);

    if let Some(tier) = override_tier {
        // A `Gpu` override cannot conjure a capability the adapter lacks: an
        // override selects among *available* tiers, so a `Gpu` override onto an
        // incapable adapter is REFUSED (the same fail-fast the probe path takes),
        // letting `ensure_device`'s `Unavailable` arm fire rather than handing
        // vello a device that panics every frame. A `Cpu` override (no adapter
        // prerequisite) and a `Gpu` override on a capable adapter are unchanged.
        if tier == RenderTier::Gpu && !gpu_capable {
            let missing = GPU_REQUIRED_DOWNLEVEL_FLAGS - caps.downlevel_flags;
            return TierSelection {
                outcome: TierOutcome::Unavailable {
                    would_be: RenderTier::Gpu,
                },
                diagnosis: format!(
                    "frust-render: GPU render tier explicitly requested via override, but \
                     REFUSED — adapter `{}` lacks the downlevel flags the vello renderer requires \
                     ({missing:?}); an override selects among available tiers and cannot supply a \
                     missing GPU capability. Run on a physical device, or build with the \
                     experimental `cpu-tier` feature for a CPU fallback.",
                    caps.adapter_name
                ),
            };
        }
        // The hybrid tier is override-only AND build-gated, so this is the
        // one thing that can refuse it: without the feature there is no
        // renderer to select. REFUSED rather than quietly ignored, for the
        // reason the whole tier exists — a run asked for on `hybrid` that
        // rendered through vello instead would put a mislabelled number into
        // the comparison the spike is being run to produce.
        if tier == RenderTier::Hybrid && !(cfg!(feature = "hybrid-tier") && hybrid_capable) {
            return TierSelection {
                outcome: TierOutcome::Unavailable {
                    would_be: RenderTier::Hybrid,
                },
                diagnosis: format!(
                    "frust-render: hybrid render tier explicitly requested via override, but \
                     REFUSED — the experimental `hybrid-tier` feature is {} and adapter `{}` {} \
                     the hybrid tier's required downlevel flags \
                     ({HYBRID_REQUIRED_DOWNLEVEL_FLAGS:?}). Rebuild with `--features \
                     frust-render/hybrid-tier`.",
                    if cfg!(feature = "hybrid-tier") {
                        "compiled in"
                    } else {
                        "NOT compiled in"
                    },
                    caps.adapter_name,
                    if hybrid_capable { "supports" } else { "lacks" },
                ),
            };
        }
        return TierSelection {
            outcome: TierOutcome::Available(tier),
            diagnosis: format!(
                "frust-render: render tier explicitly overridden to {tier:?} (adapter `{}` {} \
                 the GPU tier's required downlevel flags)",
                caps.adapter_name,
                if gpu_capable { "supports" } else { "lacks" },
            ),
        };
    }

    if gpu_capable {
        return TierSelection {
            outcome: TierOutcome::Available(RenderTier::Gpu),
            diagnosis: format!(
                "frust-render: GPU adapter `{}` supports the vello render pipeline",
                caps.adapter_name
            ),
        };
    }

    let missing = GPU_REQUIRED_DOWNLEVEL_FLAGS - caps.downlevel_flags;
    let reason = format!(
        "frust-render: GPU adapter `{}` lacks downlevel flags required by the vello renderer \
         ({missing:?}); this is the known wgpu-29/vello-0.9 iOS Simulator limitation \
         (INDIRECT_EXECUTION is unavailable on the simulator's Apple2 GPU family) if that is the \
         only missing flag, or a genuinely under-capable adapter otherwise.",
        caps.adapter_name
    );

    if cfg!(feature = "cpu-tier") {
        TierSelection {
            outcome: TierOutcome::Available(RenderTier::Cpu),
            diagnosis: format!(
                "{reason} Falling back to the experimental CPU tier (vello_cpu, `cpu-tier` \
                 feature)."
            ),
        }
    } else {
        TierSelection {
            outcome: TierOutcome::Unavailable {
                would_be: RenderTier::Cpu,
            },
            diagnosis: format!(
                "{reason} Run on a physical device, or build with the experimental `cpu-tier` \
                 feature for a CPU fallback — or with `hybrid-tier` plus \
                 `{RENDER_TIER_ENV_VAR}=hybrid`, which requires none of these flags."
            ),
        }
    }
}

/// The env var [`render_tier_override_from_env`] reads (the override
/// plumbing) — a compile-time-or-runtime knob like every other `FRUST_*`
/// knob (see `context::env_str`'s precedence: runtime wins when both are
/// set, compile-time otherwise). Set directly at runtime, by `frust run
/// --render-tier gpu|cpu|hybrid` for the spawned desktop process, or baked in
/// at build time via `frust build|run --define FRUST_RENDER_TIER=<tier>` —
/// the only way an override reaches an Android app process, which has no
/// runtime env to set (mobile: `--render-tier` itself is not plumbed in v1,
/// see `frust-cli`'s flag help text; `--define` is).
pub const RENDER_TIER_ENV_VAR: &str = "FRUST_RENDER_TIER";

/// Pure parse of a raw override string (`"gpu"`/`"cpu"`/`"hybrid"`,
/// case-insensitive) into a [`RenderTier`], or `None` (with a logged warning)
/// for anything else. Split out from the env lookup
/// ([`render_tier_override_from_env`]) so it is unit-testable without mutating
/// process-wide env state, mirroring `context.rs`'s pure-decision/
/// platform-lookup split.
pub fn parse_render_tier_override(raw: &str) -> Option<RenderTier> {
    match raw.to_ascii_lowercase().as_str() {
        "gpu" => Some(RenderTier::Gpu),
        "cpu" => Some(RenderTier::Cpu),
        // Ungated, exactly like `"cpu"`: the override vocabulary is the same
        // string set in every build. A build without the `hybrid-tier`
        // feature refuses the parsed override in [`select_render_tier`], with
        // a diagnosis naming the missing feature — a far more useful answer
        // than "invalid value, ignored" followed by a vello frame recorded as
        // a hybrid measurement.
        "hybrid" => Some(RenderTier::Hybrid),
        other => {
            log::warn!(
                "frust-render: ignoring invalid {RENDER_TIER_ENV_VAR}={other:?} (expected \
                 \"gpu\", \"cpu\" or \"hybrid\")"
            );
            None
        }
    }
}

/// Pure precedence resolution over the two `FRUST_RENDER_TIER` sources —
/// `compile_time` (an `option_env!("FRUST_RENDER_TIER")` value baked in at
/// build time) and `runtime` (a `std::env::var` read) — via
/// [`crate::context::env_str`]'s documented precedence (a non-empty runtime
/// value wins even when a compile-time value is also set; compile-time is
/// honoured only when runtime is absent or empty; neither present is
/// `None`), then [`parse_render_tier_override`] on whichever raw string that
/// resolves to. Split out from [`render_tier_override_from_env`]'s thin env
/// wrapper so the precedence itself is unit-testable without mutating
/// process-wide env state — the same pure-decision/platform-lookup split
/// this module already follows for [`select_render_tier`], and the shape
/// `hybrid_tier::atlas_cache_enabled`/`parse_atlas_cache` use for
/// `FRUST_HYBRID_ATLAS_CACHE`.
///
/// An invalid value in whichever source wins precedence is `None` (with the
/// existing [`parse_render_tier_override`] warning) — it is not retried
/// against the other source, matching `env_str`'s "runtime wins outright"
/// precedence rather than a per-field fallback.
fn render_tier_override_from_sources(
    compile_time: Option<&'static str>,
    runtime: Option<String>,
) -> Option<RenderTier> {
    crate::context::env_str(compile_time, runtime).and_then(|raw| parse_render_tier_override(&raw))
}

/// Reads and parses [`RENDER_TIER_ENV_VAR`] from both the compile-time
/// (`option_env!`, baked in at build time) and runtime (`std::env::var`)
/// sources — the compile-time-or-runtime shape `context.rs`'s `aa_mode()`/
/// `render_scale()` knobs use, via [`render_tier_override_from_sources`]. An
/// Android app process has no runtime env, so the compile-time half (`frust
/// build --define FRUST_RENDER_TIER=<tier>`) is the only way an override
/// reaches it; a desktop process may still set the runtime var directly, and
/// runtime wins when both are present. Neither source present, or a
/// set-but-invalid value in whichever wins, is `None` (a warning is logged by
/// [`parse_render_tier_override`]) — an override is a convenience, never a
/// hard config error.
pub fn render_tier_override_from_env() -> Option<RenderTier> {
    render_tier_override_from_sources(
        option_env!("FRUST_RENDER_TIER"),
        std::env::var(RENDER_TIER_ENV_VAR).ok(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(flags: wgpu::DownlevelFlags) -> TierCaps {
        TierCaps {
            downlevel_flags: flags,
            adapter_name: "fake adapter".to_string(),
        }
    }

    #[test]
    fn full_caps_select_gpu() {
        let selection = select_render_tier(&caps(wgpu::DownlevelFlags::all()), None);
        assert_eq!(selection.outcome, TierOutcome::Available(RenderTier::Gpu));
    }

    #[test]
    fn missing_compute_shaders_falls_back_or_diagnoses() {
        let missing_compute = wgpu::DownlevelFlags::all() - wgpu::DownlevelFlags::COMPUTE_SHADERS;
        let selection = select_render_tier(&caps(missing_compute), None);
        if cfg!(feature = "cpu-tier") {
            assert_eq!(selection.outcome, TierOutcome::Available(RenderTier::Cpu));
        } else {
            assert_eq!(
                selection.outcome,
                TierOutcome::Unavailable {
                    would_be: RenderTier::Cpu
                }
            );
        }
        assert!(selection.diagnosis.contains("lacks downlevel flags"));
    }

    #[test]
    fn missing_indirect_execution_falls_back_or_diagnoses() {
        let missing_indirect =
            wgpu::DownlevelFlags::all() - wgpu::DownlevelFlags::INDIRECT_EXECUTION;
        let selection = select_render_tier(&caps(missing_indirect), None);
        if cfg!(feature = "cpu-tier") {
            assert_eq!(selection.outcome, TierOutcome::Available(RenderTier::Cpu));
        } else {
            assert_eq!(
                selection.outcome,
                TierOutcome::Unavailable {
                    would_be: RenderTier::Cpu
                }
            );
        }
    }

    #[test]
    fn gpu_override_refused_on_incapable_caps() {
        // A `Gpu` override cannot conjure a missing GPU capability: it is refused
        // (Unavailable naming Gpu), so `ensure_device` fails fast instead of
        // handing vello a device that panics every frame.
        let selection =
            select_render_tier(&caps(wgpu::DownlevelFlags::empty()), Some(RenderTier::Gpu));
        assert_eq!(
            selection.outcome,
            TierOutcome::Unavailable {
                would_be: RenderTier::Gpu
            }
        );
        assert!(selection.diagnosis.contains("REFUSED"));
    }

    #[test]
    fn gpu_override_wins_on_capable_caps() {
        // A `Gpu` override on a capable adapter is honored.
        let selection =
            select_render_tier(&caps(wgpu::DownlevelFlags::all()), Some(RenderTier::Gpu));
        assert_eq!(selection.outcome, TierOutcome::Available(RenderTier::Gpu));
    }

    #[test]
    fn cpu_override_wins_even_on_incapable_caps() {
        // The CPU tier has no adapter prerequisite, so a `Cpu` override always
        // applies — even onto an adapter with no downlevel flags at all.
        let selection =
            select_render_tier(&caps(wgpu::DownlevelFlags::empty()), Some(RenderTier::Cpu));
        assert_eq!(selection.outcome, TierOutcome::Available(RenderTier::Cpu));
    }

    #[test]
    fn cpu_override_wins_over_capable_caps_too() {
        let selection =
            select_render_tier(&caps(wgpu::DownlevelFlags::all()), Some(RenderTier::Cpu));
        assert_eq!(selection.outcome, TierOutcome::Available(RenderTier::Cpu));
    }

    #[test]
    fn hybrid_requires_no_downlevel_flags() {
        // The tier's whole reason for existing on the adapters that refuse the
        // GPU tier: an adapter with NO downlevel flags at all still satisfies
        // the hybrid requirement, while failing the GPU one.
        let none = wgpu::DownlevelFlags::empty();
        assert!(none.contains(HYBRID_REQUIRED_DOWNLEVEL_FLAGS));
        assert!(!none.contains(GPU_REQUIRED_DOWNLEVEL_FLAGS));
    }

    #[test]
    fn hybrid_override_is_build_gated() {
        // Refused without the feature (nothing to select), honoured with it —
        // on an adapter with no downlevel flags at all, since the hybrid tier
        // requires none.
        let selection = select_render_tier(
            &caps(wgpu::DownlevelFlags::empty()),
            Some(RenderTier::Hybrid),
        );
        if cfg!(feature = "hybrid-tier") {
            assert_eq!(
                selection.outcome,
                TierOutcome::Available(RenderTier::Hybrid)
            );
        } else {
            assert_eq!(
                selection.outcome,
                TierOutcome::Unavailable {
                    would_be: RenderTier::Hybrid
                }
            );
            assert!(selection.diagnosis.contains("REFUSED"));
            assert!(selection.diagnosis.contains("hybrid-tier"));
        }
    }

    #[test]
    fn hybrid_is_never_auto_probed() {
        // No probe result may be `Hybrid`, however capable or incapable the
        // adapter is: the tier is reachable by override alone.
        for flags in [
            wgpu::DownlevelFlags::all(),
            wgpu::DownlevelFlags::empty(),
            wgpu::DownlevelFlags::all() - wgpu::DownlevelFlags::COMPUTE_SHADERS,
            wgpu::DownlevelFlags::all() - wgpu::DownlevelFlags::INDIRECT_EXECUTION,
        ] {
            let selection = select_render_tier(&caps(flags), None);
            assert_ne!(
                selection.outcome,
                TierOutcome::Available(RenderTier::Hybrid),
                "probe selected Hybrid for {flags:?}"
            );
        }
    }

    #[test]
    fn parses_gpu_and_cpu_case_insensitively() {
        assert_eq!(parse_render_tier_override("gpu"), Some(RenderTier::Gpu));
        assert_eq!(parse_render_tier_override("GPU"), Some(RenderTier::Gpu));
        assert_eq!(parse_render_tier_override("cpu"), Some(RenderTier::Cpu));
        assert_eq!(parse_render_tier_override("Cpu"), Some(RenderTier::Cpu));
    }

    #[test]
    fn parses_hybrid_in_every_build() {
        // Parsed in EVERY build, `cpu`-style: the string vocabulary does not
        // move with the feature. A build without `hybrid-tier` refuses the
        // override one level up (`hybrid_override_is_build_gated`) instead of
        // dropping it here, so an override that cannot be honoured fails
        // loudly rather than rendering vello under a hybrid label.
        assert_eq!(
            parse_render_tier_override("hybrid"),
            Some(RenderTier::Hybrid)
        );
        assert_eq!(
            parse_render_tier_override("Hybrid"),
            Some(RenderTier::Hybrid)
        );
    }

    #[test]
    fn invalid_override_value_is_ignored() {
        assert_eq!(parse_render_tier_override(""), None);
        assert_eq!(parse_render_tier_override("vello"), None);
    }

    #[test]
    fn override_sources_compile_time_only() {
        // No runtime value: the compile-time half is honoured — the only way
        // an override reaches an Android app process (no runtime env).
        assert_eq!(
            render_tier_override_from_sources(Some("hybrid"), None),
            Some(RenderTier::Hybrid)
        );
    }

    #[test]
    fn override_sources_runtime_wins_over_compile_time() {
        // Both set: runtime wins outright, per `env_str`'s precedence.
        assert_eq!(
            render_tier_override_from_sources(Some("gpu"), Some("cpu".to_string())),
            Some(RenderTier::Cpu)
        );
    }

    #[test]
    fn override_sources_neither_present_is_none() {
        assert_eq!(render_tier_override_from_sources(None, None), None);
    }

    #[test]
    fn override_sources_empty_runtime_falls_back_to_compile_time() {
        // An empty runtime value is treated as unset by `env_str`, so the
        // compile-time half still applies rather than resolving to `None`.
        assert_eq!(
            render_tier_override_from_sources(Some("hybrid"), Some(String::new())),
            Some(RenderTier::Hybrid)
        );
    }

    #[test]
    fn override_sources_invalid_runtime_is_none_even_with_valid_compile_time() {
        // Runtime wins precedence outright, so an invalid runtime value is
        // NOT retried against a (valid) compile-time fallback — it resolves
        // to `None`, with `parse_render_tier_override`'s existing warning.
        assert_eq!(
            render_tier_override_from_sources(Some("gpu"), Some("vello".to_string())),
            None
        );
    }

    #[test]
    fn override_sources_invalid_compile_time_with_no_runtime_is_none() {
        assert_eq!(render_tier_override_from_sources(Some("vello"), None), None);
    }
}
