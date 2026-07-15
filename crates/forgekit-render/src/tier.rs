//! Render-tier selection seam (spec §8).
//!
//! Only [`RenderTier::Gpu`] exists today. The `Hybrid` (`vello_hybrid`) and
//! `Cpu` (`vello_cpu`) tiers land in spec Phase 6 once tier probing and the
//! sparse-strips backends are wired in; the enum and [`select_render_tier`]
//! stub exist now so the surrounding init contract (spec §10.3) and CLI
//! `--render-tier` flag have a stable type to target.

/// Which Vello backend the renderer drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RenderTier {
    /// GPU compute path (`vello` 0.9). The only tier implemented in v0.
    #[default]
    Gpu,
}

/// Selects the render tier, honouring an explicit override when supplied.
///
/// Today this always resolves to [`RenderTier::Gpu`]; capability probing
/// (compute-shader support, GPU heuristics) is added alongside the Hybrid/Cpu
/// tiers in spec Phase 6.
pub fn select_render_tier(override_tier: Option<RenderTier>) -> RenderTier {
    override_tier.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_gpu() {
        assert_eq!(select_render_tier(None), RenderTier::Gpu);
    }

    #[test]
    fn honours_override() {
        assert_eq!(select_render_tier(Some(RenderTier::Gpu)), RenderTier::Gpu);
    }
}
