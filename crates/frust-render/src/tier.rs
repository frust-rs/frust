//! Render-tier selection seam.
//!
//! [`RenderTier::Engine`] is the frust-owned `frust-engine` strip pipeline —
//! the sole render tier, behind the default `engine-tier` feature
//! (`frust-render/Cargo.toml`): [`select_render_tier`] probes for it
//! automatically, since `frust-engine` needs no downlevel flag at all (see
//! [`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`]).
//!
//! The vello-classic `Gpu` tier is GONE: the renderer, its `vello` dependency
//! and the `FRUST_RENDER_TIER=gpu` escape hatch that reached it were all
//! deleted once the engine tier became the default. The experimental
//! `vello_cpu`-backed `Cpu` tier is GONE too, retired alongside its
//! `cpu-tier` feature once the engine tier proved it needs no downlevel
//! capability the legacy fallback existed to cover. `"gpu"` and `"cpu"` are
//! therefore no longer values this seam parses — an unknown override string,
//! either included, is refused/logged and falls back to no override (see
//! [`parse_render_tier_override`]).
//!
//! [`select_render_tier`] is pure decision logic over [`TierCaps`] (a plain
//! struct a caller builds from a real `wgpu::Adapter`'s downlevel flags +
//! name), so it is unit-testable with fake caps and needs no GPU — mirroring
//! `context.rs`'s split of pure decision vs. platform lookup (e.g.
//! `effective_instance_flags`/`is_android_emulator`). [`RenderContext`](crate::RenderContext)
//! is the one production call site, consulting it at device-init time and
//! reusing [`TierSelection::diagnosis`] as its single fail-fast message.

/// Which renderer the surface pipeline drives.
///
/// The frust-owned render engine (`frust-engine`: a `frust_scene::Scene`
/// compiled into sparse strips and drawn by ordinary render passes over
/// `frust-gpu`) is the only variant — selectable when the `engine-tier`
/// feature is compiled in, the default feature
/// (`frust-render/Cargo.toml`'s `default = ["engine-tier"]`). The
/// vello-classic `Gpu` tier and the experimental `vello_cpu`-backed `Cpu`
/// tier were both retired; this enum keeps a single variant (rather than
/// collapsing to a unit struct) because [`select_render_tier`] still needs a
/// value to name in [`TierOutcome::Unavailable`] when no tier is compiled in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RenderTier {
    /// **The only tier [`select_render_tier`] can ever select**, whenever the
    /// `engine-tier` feature is compiled in: [`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`]
    /// is deliberately empty, so an adapter that reaches the probe always
    /// satisfies it.
    #[default]
    Engine,
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

/// The [`wgpu::DownlevelFlags`] the engine tier (`frust-engine`) cannot run
/// without: **none of them**.
///
/// Deliberately empty. `frust-engine` is built to the
/// GLES-3.0/WebGL2 downlevel ceiling by design rule — no compute pass, no
/// storage buffer, no indirect draw (`frust-gpu`'s `lint` module enforces that
/// against the engine's own shaders) — so it runs on adapters the deleted
/// vello-classic tier could not: it needed `COMPUTE_SHADERS` and
/// `INDIRECT_EXECUTION`, precisely the pair the iOS Simulator's `Apple2` GPU
/// family cannot supply. No adapter can raise a capability objection here.
///
/// Named as a constant rather than left implicit so a flag the engine ever
/// does need is honoured by [`select_render_tier`] without a second edit.
pub const ENGINE_REQUIRED_DOWNLEVEL_FLAGS: wgpu::DownlevelFlags = wgpu::DownlevelFlags::empty();

/// What [`select_render_tier`] decided is actually usable in this build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TierOutcome {
    /// `tier` is usable — either it probed capable, or an explicit override
    /// selected it.
    Available(RenderTier),
    /// No tier is usable in this build. `would_be` names the tier that
    /// *would* have been selected had it been compiled in — always `Engine`,
    /// the only tier this crate contains, in a build without the
    /// `engine-tier` feature.
    Unavailable { would_be: RenderTier },
}

/// The result of [`select_render_tier`]: the decided [`TierOutcome`] plus a
/// human-readable diagnosis — the one message a refused probe surfaces
/// through (see [`RenderContext::ensure_device`](crate::RenderContext), the
/// production call site).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierSelection {
    pub outcome: TierOutcome,
    pub diagnosis: String,
}

/// Selects the render tier.
///
/// - An explicit `override_tier` (always `Engine` — the only variant this
///   enum has) applies only in a build that compiled the `engine-tier`
///   feature in, against [`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`]; without it the
///   override is refused ([`TierOutcome::Unavailable`] naming `Engine`)
///   rather than ignored, so a capture asked for on the engine tier can never
///   be some other renderer wearing the wrong label. With the feature
///   compiled in, an explicit `Engine` override is equivalent to the
///   no-override default below.
/// - Without an override: if the `engine-tier` feature is compiled in, `caps`
///   satisfying [`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`] (deliberately empty, so
///   always true) selects [`RenderTier::Engine`] — the default probed tier
///   (`frust-render/Cargo.toml`'s `default = ["engine-tier"]`). A build
///   compiled WITHOUT the `engine-tier` feature has no renderer at all, so
///   this falls through to [`TierOutcome::Unavailable`] naming `Engine` as
///   the tier that would apply — the caller then fails fast using
///   [`TierSelection::diagnosis`].
pub fn select_render_tier(caps: &TierCaps, override_tier: Option<RenderTier>) -> TierSelection {
    // Always true today: [`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`] is empty by
    // design. Asked of `caps` rather than assumed, so the requirement lives in
    // one named constant — and a flag ever added to it is honoured here
    // without a second edit.
    let engine_capable = caps
        .downlevel_flags
        .contains(ENGINE_REQUIRED_DOWNLEVEL_FLAGS);
    let engine_compiled = cfg!(feature = "engine-tier") && engine_capable;

    if let Some(tier) = override_tier {
        // The engine tier is build-gated, so this is the one thing that can
        // refuse it: without the feature there is no renderer to select.
        // REFUSED rather than quietly ignored — a run asked for on `engine`
        // that rendered through something else would put a mislabelled number
        // into the comparison the override exists to produce. `tier` can only
        // ever be `Engine` (the enum's one variant), so there is nothing else
        // to match on here.
        if !engine_compiled {
            return TierSelection {
                outcome: TierOutcome::Unavailable {
                    would_be: RenderTier::Engine,
                },
                diagnosis: format!(
                    "frust-render: engine render tier explicitly requested via override, but \
                     REFUSED — the `engine-tier` feature is {} and adapter `{}` {} the engine \
                     tier's required downlevel flags \
                     ({ENGINE_REQUIRED_DOWNLEVEL_FLAGS:?}). Rebuild with `--features \
                     frust-render/engine-tier`.",
                    if cfg!(feature = "engine-tier") {
                        "compiled in"
                    } else {
                        "NOT compiled in"
                    },
                    caps.adapter_name,
                    if engine_capable { "supports" } else { "lacks" },
                ),
            };
        }
        return TierSelection {
            outcome: TierOutcome::Available(tier),
            diagnosis: format!(
                "frust-render: render tier explicitly overridden to {tier:?} (adapter `{}`)",
                caps.adapter_name,
            ),
        };
    }

    // The engine tier is the probed default whenever its feature is compiled
    // in, which it is by default: `ENGINE_REQUIRED_DOWNLEVEL_FLAGS` is empty,
    // so `engine_capable` is always true and this arm always wins. A build
    // compiled without the feature has no renderer to fall back to — the
    // vello-classic and experimental CPU tiers this seam once fell back to
    // were both retired.
    if engine_compiled {
        return TierSelection {
            outcome: TierOutcome::Available(RenderTier::Engine),
            diagnosis: format!(
                "frust-render: engine render tier selected by default (adapter `{}`; the \
                 frust-engine tier requires no downlevel flags at all)",
                caps.adapter_name
            ),
        };
    }

    TierSelection {
        outcome: TierOutcome::Unavailable {
            would_be: RenderTier::Engine,
        },
        diagnosis: format!(
            "frust-render: this build did not compile the `engine-tier` feature in, so adapter \
             `{}` has no renderer to drive. Rebuild with the default `engine-tier` feature.",
            caps.adapter_name
        ),
    }
}

/// The env var [`render_tier_override_from_env`] reads (the override
/// plumbing) — a compile-time-or-runtime knob like every other `FRUST_*`
/// knob (see `context::env_str`'s precedence: runtime wins when both are
/// set, compile-time otherwise). Set directly at runtime, by `frust run
/// --render-tier engine` for the spawned desktop process, or baked in
/// at build time via `frust build|run --define FRUST_RENDER_TIER=<tier>` —
/// the only way an override reaches an Android app process, which has no
/// runtime env to set (mobile: `--render-tier` itself is not plumbed in v1,
/// see `frust-cli`'s flag help text; `--define` is).
pub const RENDER_TIER_ENV_VAR: &str = "FRUST_RENDER_TIER";

/// Pure parse of a raw override string (`"engine"`, case-insensitive) into a
/// [`RenderTier`], or `None` (with a logged warning) for anything else —
/// `"gpu"` and `"cpu"` included, since the vello-classic and experimental
/// CPU tiers those named no longer exist. Split out from the env lookup
/// ([`render_tier_override_from_env`]) so it is unit-testable without
/// mutating process-wide env state, mirroring `context.rs`'s
/// pure-decision/platform-lookup split.
pub fn parse_render_tier_override(raw: &str) -> Option<RenderTier> {
    match raw.to_ascii_lowercase().as_str() {
        // A build without the `engine-tier` feature refuses the parsed
        // override in [`select_render_tier`], with a diagnosis naming the
        // missing feature — a far more useful answer than "invalid value,
        // ignored".
        "engine" => Some(RenderTier::Engine),
        other => {
            log::warn!(
                "frust-render: ignoring invalid {RENDER_TIER_ENV_VAR}={other:?} (expected \
                 \"engine\")"
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
/// this module already follows for [`select_render_tier`].
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
/// sources, via [`render_tier_override_from_sources`]. An Android app process
/// has no runtime env, so the compile-time half (`frust build --define
/// FRUST_RENDER_TIER=<tier>`) is the only way an override reaches it; a
/// desktop process may still set the runtime var directly, and runtime wins
/// when both are present. Neither source present, or a set-but-invalid value
/// in whichever wins, is `None` (a warning is logged by
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
    fn full_caps_select_engine_by_default() {
        let selection = select_render_tier(&caps(wgpu::DownlevelFlags::all()), None);
        if cfg!(feature = "engine-tier") {
            assert_eq!(
                selection.outcome,
                TierOutcome::Available(RenderTier::Engine)
            );
        } else {
            assert_eq!(
                selection.outcome,
                TierOutcome::Unavailable {
                    would_be: RenderTier::Engine
                }
            );
        }
    }

    #[test]
    fn a_downlevel_adapter_still_selects_engine_by_default() {
        // The engine tier's reason for existing on the adapters the deleted
        // vello-classic tier refused: `ENGINE_REQUIRED_DOWNLEVEL_FLAGS` is
        // empty, so no missing flag disqualifies it — including the
        // `COMPUTE_SHADERS`/`INDIRECT_EXECUTION` pair the iOS Simulator lacks.
        for flags in [
            wgpu::DownlevelFlags::empty(),
            wgpu::DownlevelFlags::all() - wgpu::DownlevelFlags::COMPUTE_SHADERS,
            wgpu::DownlevelFlags::all() - wgpu::DownlevelFlags::INDIRECT_EXECUTION,
        ] {
            let selection = select_render_tier(&caps(flags), None);
            if cfg!(feature = "engine-tier") {
                assert_eq!(
                    selection.outcome,
                    TierOutcome::Available(RenderTier::Engine),
                    "probe did not select Engine for {flags:?} with engine-tier compiled in"
                );
            }
        }
    }

    #[test]
    fn engine_requires_no_downlevel_flags() {
        let none = wgpu::DownlevelFlags::empty();
        assert!(none.contains(ENGINE_REQUIRED_DOWNLEVEL_FLAGS));
    }

    #[test]
    fn engine_override_is_build_gated() {
        // Refused without the feature (nothing to select), honoured with it —
        // on an adapter with no downlevel flags at all, since the engine tier
        // requires none.
        let selection = select_render_tier(
            &caps(wgpu::DownlevelFlags::empty()),
            Some(RenderTier::Engine),
        );
        if cfg!(feature = "engine-tier") {
            assert_eq!(
                selection.outcome,
                TierOutcome::Available(RenderTier::Engine)
            );
        } else {
            assert_eq!(
                selection.outcome,
                TierOutcome::Unavailable {
                    would_be: RenderTier::Engine
                }
            );
            assert!(selection.diagnosis.contains("REFUSED"));
            assert!(selection.diagnosis.contains("engine-tier"));
        }
    }

    #[test]
    fn parses_engine_case_insensitively() {
        assert_eq!(
            parse_render_tier_override("engine"),
            Some(RenderTier::Engine)
        );
        assert_eq!(
            parse_render_tier_override("Engine"),
            Some(RenderTier::Engine)
        );
    }

    #[test]
    fn the_retired_gpu_value_is_no_longer_parsed() {
        // The vello-classic escape hatch was deleted with its renderer, so
        // `gpu` is an invalid value like any other typo — refused with the
        // warning rather than silently honoured under another renderer.
        assert_eq!(parse_render_tier_override("gpu"), None);
        assert_eq!(parse_render_tier_override("GPU"), None);
    }

    #[test]
    fn the_retired_cpu_value_is_no_longer_parsed() {
        // The experimental `vello_cpu` fallback was deleted with its
        // `cpu-tier` feature, so `cpu` is an invalid value like any other
        // typo now — same treatment as the retired `gpu` value above.
        assert_eq!(parse_render_tier_override("cpu"), None);
        assert_eq!(parse_render_tier_override("CPU"), None);
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
            render_tier_override_from_sources(Some("engine"), None),
            Some(RenderTier::Engine)
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
            render_tier_override_from_sources(Some("engine"), Some(String::new())),
            Some(RenderTier::Engine)
        );
    }

    #[test]
    fn override_sources_invalid_runtime_is_none_even_with_valid_compile_time() {
        // Runtime wins precedence outright, so an invalid runtime value is
        // NOT retried against a (valid) compile-time fallback — it resolves
        // to `None`, with `parse_render_tier_override`'s existing warning.
        assert_eq!(
            render_tier_override_from_sources(Some("engine"), Some("vello".to_string())),
            None
        );
    }

    #[test]
    fn override_sources_invalid_compile_time_with_no_runtime_is_none() {
        assert_eq!(render_tier_override_from_sources(Some("vello"), None), None);
    }
}
