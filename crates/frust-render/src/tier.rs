//! Render-tier selection seam.
//!
//! [`RenderTier::Gpu`] is the vello 0.9 GPU compute path (the only tier a
//! device is actually created for today); [`RenderTier::Cpu`] is the
//! experimental `vello_cpu` fallback, selectable only when the `cpu-tier`
//! feature is compiled in (not default — see `frust-render/Cargo.toml`)
//! and not yet wired into device creation (that lands alongside the
//! `cpu-tier` encode path). The `Hybrid` (`vello_hybrid`) tier is deferred
//! entirely — its own scene type and pre-1.0 instability make it out of
//! scope for now.
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
///   the same fail-fast behavior a failed probe produces (PLAN.md D4's override
///   plumbing: `FRUST_RENDER_TIER` / `frust run --render-tier`).
/// - Without an override: `caps` supporting [`GPU_REQUIRED_DOWNLEVEL_FLAGS`]
///   selects [`RenderTier::Gpu`]. Otherwise, if the `cpu-tier` feature is
///   compiled in, falls back to [`RenderTier::Cpu`] (still experimental, and
///   not yet wired into device creation — see this module's docs); without
///   the feature, the result is [`TierOutcome::Unavailable`] naming `Cpu` as
///   the tier that would apply, and the caller keeps today's fail-fast
///   behavior using [`TierSelection::diagnosis`].
pub fn select_render_tier(caps: &TierCaps, override_tier: Option<RenderTier>) -> TierSelection {
    let gpu_capable = caps.downlevel_flags.contains(GPU_REQUIRED_DOWNLEVEL_FLAGS);

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
                 feature for a CPU fallback."
            ),
        }
    }
}

/// The env var [`render_tier_override_from_env`] reads (PLAN.md D4's
/// override plumbing) — set directly, or by `frust run --render-tier
/// gpu|cpu` for the spawned desktop process (mobile: not plumbed in v1, see
/// `frust-cli`'s `--render-tier` flag help text).
pub const RENDER_TIER_ENV_VAR: &str = "FRUST_RENDER_TIER";

/// Pure parse of a raw override string (`"gpu"`/`"cpu"`, case-insensitive)
/// into a [`RenderTier`], or `None` (with a logged warning) for anything
/// else. Split out from the env lookup ([`render_tier_override_from_env`])
/// so it is unit-testable without mutating process-wide env state, mirroring
/// `context.rs`'s pure-decision/platform-lookup split.
pub fn parse_render_tier_override(raw: &str) -> Option<RenderTier> {
    match raw.to_ascii_lowercase().as_str() {
        "gpu" => Some(RenderTier::Gpu),
        "cpu" => Some(RenderTier::Cpu),
        other => {
            log::warn!(
                "frust-render: ignoring invalid {RENDER_TIER_ENV_VAR}={other:?} (expected \
                 \"gpu\" or \"cpu\")"
            );
            None
        }
    }
}

/// Reads and parses [`RENDER_TIER_ENV_VAR`] from the process environment. An
/// unset var is `None`; a set-but-invalid value is also `None` (a warning is
/// logged by [`parse_render_tier_override`]) — an override is a convenience,
/// never a hard config error.
pub fn render_tier_override_from_env() -> Option<RenderTier> {
    std::env::var(RENDER_TIER_ENV_VAR)
        .ok()
        .and_then(|raw| parse_render_tier_override(&raw))
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
    fn parses_gpu_and_cpu_case_insensitively() {
        assert_eq!(parse_render_tier_override("gpu"), Some(RenderTier::Gpu));
        assert_eq!(parse_render_tier_override("GPU"), Some(RenderTier::Gpu));
        assert_eq!(parse_render_tier_override("cpu"), Some(RenderTier::Cpu));
        assert_eq!(parse_render_tier_override("Cpu"), Some(RenderTier::Cpu));
    }

    #[test]
    fn invalid_override_value_is_ignored() {
        assert_eq!(parse_render_tier_override("hybrid"), None);
        assert_eq!(parse_render_tier_override(""), None);
    }
}
