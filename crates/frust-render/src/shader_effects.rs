//! The **policy** half of the offscreen WGSL fragment-shader effects
//! (shader-showcase feature): how big a target a shader quad may ask for.
//!
//! Every GPU resource the feature needs — lazily compiled per-program
//! pipelines, the per-`(program, size)` target pool, the fullscreen-triangle
//! pass, the age-based reap and the churn detector — lives one layer down in
//! [`frust_gpu::effects`], which speaks `(id, wgsl, size, time)` primitives and
//! nothing about scenes. What is left here is the part that reads a
//! `Command::ShaderQuad` and decides the `(id, size)` pair to ask that module
//! for.
//!
//! Currently UNWIRED, like the module it was split out of: the only caller of
//! either half was the vello-classic tier's encode-time pre-pass, which
//! registered each rendered quad with `vello::Renderer` as an image override —
//! a seam that died with vello, and one the engine tier has no counterpart for
//! until the GPU-seam phase builds one. The module is retained (rather than
//! deleted) because that phase needs exactly this policy back; the crate-level
//! `allow(dead_code)` is what keeps it intact without an unused-code failure
//! meanwhile.

/// The conservative upper bound on a shader-effect target's own dimensions,
/// applied on top of the adapter's `max_texture_dimension_2d` by
/// [`clamp_size`].
///
/// 8192 is inherited from the vello-classic tier's image atlas, whose hard
/// 8192×8192 ceiling silently failed to render anything larger — an over-cap
/// quad simply vanished rather than erroring, so clamping proactively was the
/// only guard. That renderer is gone and the engine tier has no atlas of its
/// own to overflow, but the cap is kept as-is pending the GPU-seam phase's own
/// policy: a shader quad this large is a footgun on any adapter, and lifting a
/// bound is a decision to take deliberately rather than by deletion.
const MAX_TEXTURE_DIM: u32 = 8192;

/// Clamp a requested target size to the renderable range: at most
/// [`MAX_TEXTURE_DIM`] and the adapter's own `max_texture_dimension_2d`, and at
/// least 1 per axis (a zero-sized texture is invalid). Pure — no GPU state
/// touched, so the policy is unit-testable.
///
/// The caller warns once when a clamp actually changes the size, and hands the
/// clamped pair to
/// [`ShaderEffects::ensure_target`](frust_gpu::effects::ShaderEffects::ensure_target).
/// The layering is deliberate: this function is the *policy* cap (the 8192
/// footgun bound ∧ the adapter's own ceiling), while `ensure_target` now
/// enforces an independent device-validity floor of its own rather than
/// trusting its caller — belt and suspenders, so both layers may warn on the
/// same oversized request once this module is wired up.
pub(crate) fn clamp_size(requested: (u32, u32), adapter_max: u32) -> (u32, u32) {
    let cap = MAX_TEXTURE_DIM.min(adapter_max);
    let clamp = |v: u32| v.clamp(1, cap.max(1));
    (clamp(requested.0), clamp(requested.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_size_caps_at_8192() {
        assert_eq!(clamp_size((10_000, 10_000), u32::MAX), (8192, 8192));
    }

    #[test]
    fn clamp_size_respects_adapter_max_below_cap() {
        // A 4096-max adapter clamps below the 8192 cap.
        assert_eq!(clamp_size((6000, 6000), 4096), (4096, 4096));
    }

    #[test]
    fn clamp_size_floors_zero_to_one() {
        assert_eq!(clamp_size((0, 0), 8192), (1, 1));
        assert_eq!(clamp_size((0, 512), 8192), (1, 512));
    }

    #[test]
    fn clamp_size_passes_through_in_range() {
        assert_eq!(clamp_size((1290, 2796), 16384), (1290, 2796));
    }

    #[test]
    fn clamp_size_survives_zero_adapter_max() {
        // A degenerate adapter_max of 0 must never produce a 0 dimension.
        assert_eq!(clamp_size((100, 100), 0), (1, 1));
    }
}
