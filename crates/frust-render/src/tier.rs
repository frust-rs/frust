//! The engine's adapter-capability gate.
//!
//! There is no render *tier* to select any more: `frust-engine`'s strip
//! pipeline is the only renderer this crate contains, it is a plain dependency
//! (`frust-render/Cargo.toml` declares no feature for it), and nothing chooses
//! between renderers at build or run time. The tier enum, the env var that
//! overrode it, the CLI flag that set that env var and the whole
//! override/fallback vocabulary were retired with the choice they described:
//! the vello-classic `Gpu` tier and the experimental `vello_cpu`-backed `Cpu`
//! tier are both gone, so an override could only ever have named the renderer
//! that was already running.
//!
//! What survives is the one question an adapter can still answer "no" to:
//! [`engine_support`] checks a real adapter's downlevel flags against
//! [`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`] and refuses one that cannot run the
//! engine, with the diagnosis its caller fails fast on. It is pure decision
//! logic over [`TierCaps`] (a plain struct a caller builds from a real
//! `wgpu::Adapter`'s downlevel flags + name), so it is unit-testable with fake
//! caps and needs no GPU — mirroring `context.rs`'s split of pure decision vs.
//! platform lookup (e.g. `effective_instance_flags`/`is_android_emulator`).
//! `context::create_engine_surface` is the production call site (once per
//! surface creation), with [`HeadlessRenderer::new`](crate::HeadlessRenderer)
//! asking the identical question of the adapter it resolved.

/// Plain-data capability inputs [`engine_support`] probes. Built from a real
/// `wgpu::Adapter` at the surface-creation call site
/// (`context::create_engine_surface`), but constructible by hand in tests with
/// no GPU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierCaps {
    /// The adapter's `wgpu::DownlevelCapabilities::flags`.
    pub downlevel_flags: wgpu::DownlevelFlags,
    /// The adapter's `wgpu::AdapterInfo::name`, used only for diagnostics.
    pub adapter_name: String,
}

/// The [`wgpu::DownlevelFlags`] the engine (`frust-engine`) cannot run
/// without: **none of them**.
///
/// Deliberately empty. `frust-engine` is built to the
/// GLES-3.0/WebGL2 downlevel ceiling by design rule — no compute pass, no
/// storage buffer, no indirect draw (`frust-gpu`'s `lint` module enforces that
/// against the engine's own shaders) — so it runs on adapters the deleted
/// vello-classic renderer could not: it needed `COMPUTE_SHADERS` and
/// `INDIRECT_EXECUTION`, precisely the pair the iOS Simulator's `Apple2` GPU
/// family cannot supply. No adapter can raise a capability objection here.
///
/// Named as a constant rather than left implicit so a flag the engine ever
/// does need is honoured by [`engine_support`] without a second edit.
pub const ENGINE_REQUIRED_DOWNLEVEL_FLAGS: wgpu::DownlevelFlags = wgpu::DownlevelFlags::empty();

/// An adapter that cannot run the engine: the refusal [`engine_support`]
/// returns, naming the adapter and the flags it is missing.
///
/// Carries its own diagnosis through [`std::fmt::Display`] — the one message a
/// refused adapter surfaces through, shared by the surface path
/// (`context::create_engine_surface`) and the headless harness
/// ([`crate::HeadlessRenderer`]), so both refuse in the same words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineUnsupported {
    /// The adapter that was refused, for the diagnosis.
    pub adapter_name: String,
    /// The subset of [`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`] this adapter does not
    /// report. Never empty in a value that exists: an adapter missing nothing
    /// is [`Ok`].
    pub missing_flags: wgpu::DownlevelFlags,
}

impl std::fmt::Display for EngineUnsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "frust-render: adapter `{}` cannot drive the frust-engine renderer — it lacks the \
             required downlevel flags ({:?}; the engine requires \
             {ENGINE_REQUIRED_DOWNLEVEL_FLAGS:?}), and this build contains no other renderer to \
             fall back to.",
            self.adapter_name, self.missing_flags,
        )
    }
}

impl std::error::Error for EngineUnsupported {}

/// Whether `caps` reports every flag in `required`.
///
/// Factored out of [`engine_support`] so the refusal arm — otherwise
/// unreachable in production, since [`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`] is
/// deliberately empty — stays exercised by a real call rather than a
/// hand-built [`EngineUnsupported`] value: tests drive it with a non-empty
/// injected `required` set instead.
fn engine_support_with(
    required: wgpu::DownlevelFlags,
    caps: &TierCaps,
) -> Result<(), EngineUnsupported> {
    let missing_flags = required - caps.downlevel_flags;
    if missing_flags.is_empty() {
        return Ok(());
    }
    Err(EngineUnsupported {
        adapter_name: caps.adapter_name.clone(),
        missing_flags,
    })
}

/// Whether `caps` can run the engine.
///
/// `Ok(())` when the adapter reports every flag in
/// [`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`] — which, that constant being
/// deliberately empty, is every adapter that reaches the probe. The refusal
/// arm is kept (rather than assumed away) so a flag ever added to the constant
/// is honoured here without a second edit, and so the diagnosis for an adapter
/// that genuinely cannot run the engine lives in one place.
pub fn engine_support(caps: &TierCaps) -> Result<(), EngineUnsupported> {
    engine_support_with(ENGINE_REQUIRED_DOWNLEVEL_FLAGS, caps)
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
    fn full_caps_run_the_engine() {
        assert_eq!(engine_support(&caps(wgpu::DownlevelFlags::all())), Ok(()));
    }

    #[test]
    fn a_downlevel_adapter_still_runs_the_engine() {
        // The engine's reason for existing on the adapters the deleted
        // vello-classic renderer refused: `ENGINE_REQUIRED_DOWNLEVEL_FLAGS` is
        // empty, so no missing flag disqualifies it — including the
        // `COMPUTE_SHADERS`/`INDIRECT_EXECUTION` pair the iOS Simulator lacks.
        for flags in [
            wgpu::DownlevelFlags::empty(),
            wgpu::DownlevelFlags::all() - wgpu::DownlevelFlags::COMPUTE_SHADERS,
            wgpu::DownlevelFlags::all() - wgpu::DownlevelFlags::INDIRECT_EXECUTION,
        ] {
            assert_eq!(
                engine_support(&caps(flags)),
                Ok(()),
                "the engine was refused on {flags:?}, which requires no flag at all"
            );
        }
    }

    #[test]
    fn engine_requires_no_downlevel_flags() {
        let none = wgpu::DownlevelFlags::empty();
        assert!(none.contains(ENGINE_REQUIRED_DOWNLEVEL_FLAGS));
    }

    /// The refusal arm, exercised against a requirement the constant does not
    /// carry today: a flag added to [`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`] later
    /// must refuse the adapter that lacks it rather than be assumed away.
    /// Drives the refusal through [`engine_support_with`] itself (with a
    /// non-empty injected `required` set — [`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`]
    /// never exercises this arm in production) rather than hand-building an
    /// [`EngineUnsupported`] value, and asserts the diagnosis names the
    /// missing flag.
    #[test]
    fn a_missing_required_flag_is_refused_by_name() {
        let required = wgpu::DownlevelFlags::COMPUTE_SHADERS;
        let adapter = caps(wgpu::DownlevelFlags::all() - required);
        let refusal = engine_support_with(required, &adapter)
            .expect_err("an adapter missing a required flag must be refused");
        assert_eq!(refusal.missing_flags, required);
        let diagnosis = refusal.to_string();
        assert!(diagnosis.contains("fake adapter"), "{diagnosis}");
        assert!(diagnosis.contains("COMPUTE_SHADERS"), "{diagnosis}");
        assert!(diagnosis.contains("frust-engine"), "{diagnosis}");
    }
}
