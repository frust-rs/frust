use std::sync::OnceLock;

/// Parse an environment flag as enabled/disabled, checking compile-time or runtime value.
///
/// Mirrors the pattern in `frust-render::context`, supporting both compile-time
/// (`option_env!`) and runtime (`std::env::var`) checks. Runtime overrides
/// compile-time.
fn env_flag_enabled(compile_time: Option<&str>, runtime: Option<String>) -> bool {
    if let Some(val) = runtime {
        val.eq_ignore_ascii_case("1") || val.eq_ignore_ascii_case("true")
    } else if let Some(val) = compile_time {
        val.eq_ignore_ascii_case("1") || val.eq_ignore_ascii_case("true")
    } else {
        false
    }
}

/// The precedence-resolved value of a string-or-number-valued `FRUST_ENGINE_*`
/// env knob, mirroring `frust-render::context`'s `env_str` combinator: a
/// non-empty runtime (`std::env::var`) value wins even when a compile-time
/// (`option_env!`) value is also set; an empty (`""`) runtime value is
/// treated as unset and falls through to the compile-time half; `None` when
/// neither half carries a non-empty value. This is the same
/// compile-time-or-runtime shape `env_flag_enabled` uses for booleans, kept
/// as its own combinator because `max_texture_size`/`atlas_size` need the
/// raw string (to parse) rather than a bool.
fn env_str(compile_time: Option<&'static str>, runtime: Option<String>) -> Option<String> {
    fn non_empty(value: Option<String>) -> Option<String> {
        value.filter(|v| !v.is_empty())
    }
    non_empty(runtime).or_else(|| non_empty(compile_time.map(str::to_string)))
}

/// Parses a raw `FRUST_ENGINE_MAX_TEX` value (already resolved by
/// [`env_str`]) into the maximum texture edge length. An unset, empty, or
/// unparsable value falls back to `0` (no limit).
fn parse_max_texture_size(raw: Option<String>) -> u32 {
    raw.and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Parses a raw `FRUST_ENGINE_ATLAS_SIZE` value (already resolved by
/// [`env_str`]), e.g. `"2048x2048"`, into `(width, height)`. Returns `None`
/// if unset or not exactly two `x`-separated integers.
fn parse_atlas_size(raw: Option<String>) -> Option<(u32, u32)> {
    raw.and_then(|s| {
        let parts: Vec<&str> = s.split('x').collect();
        if parts.len() == 2 {
            let w = parts[0].parse::<u32>().ok()?;
            let h = parts[1].parse::<u32>().ok()?;
            Some((w, h))
        } else {
            None
        }
    })
}

/// Whether depth textures are disabled via `FRUST_ENGINE_NO_DEPTH`.
/// Cached: read once per process.
pub fn depth_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_ENGINE_NO_DEPTH"),
            std::env::var("FRUST_ENGINE_NO_DEPTH").ok(),
        )
    })
}

/// Whether every strip run is forced through the whole (Uber) strip program
/// via `FRUST_ENGINE_UBER_PROGRAM`. Cached: read once per process.
///
/// The kill switch over paint-class specialisation
/// ([`crate::gpu::strips::PaintClass`]): when set, no pass segment is split
/// into class runs, no class program is warmed up or built, and every strip
/// pass draws exactly as the engine did before the classes existed — one
/// pipeline per pass, the shipped `strip.wgsl` unchanged. The escape hatch for
/// a driver that miscompiles a specialisation, and the off-arm of every
/// before/after measurement of it. `EngineRenderer::set_uber_program` is its
/// programmatic counterpart.
pub fn uber_program_forced() -> bool {
    static FORCED: OnceLock<bool> = OnceLock::new();
    *FORCED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_ENGINE_UBER_PROGRAM"),
            std::env::var("FRUST_ENGINE_UBER_PROGRAM").ok(),
        )
    })
}

/// Whether the frame's surface opening stays three passes of its own via
/// `FRUST_ENGINE_NO_PASS_MERGE`. Cached: read once per process.
///
/// The kill switch over pass merging: by default a frame clears its surface,
/// draws its opaque strips and draws its first surface round's strips in ONE
/// render pass (see `crate::renderer`'s *The frame's passes*), which on a
/// tile-based GPU saves a whole-framebuffer load and store per pass folded
/// away. When set, the clear and the opaque strips are recorded as passes of
/// their own ahead of every round, and every surface round in a pass of its
/// own — the off-arm of every before/after measurement of it, and the escape hatch for
/// a driver that mishandles a pipeline switch inside a cleared pass.
/// `EngineRenderer::set_pass_merge` is its programmatic counterpart.
pub fn pass_merge_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_ENGINE_NO_PASS_MERGE"),
            std::env::var("FRUST_ENGINE_NO_PASS_MERGE").ok(),
        )
    })
}

/// The shortest class run worth a pipeline switch of its own, via
/// `FRUST_ENGINE_RUN_SPLIT_MIN=<n>`; [`crate::gpu::strips::RUN_SPLIT_MIN`]
/// when unset or unparsable. Cached: read once per process.
///
/// A diagnostic knob for re-measuring the threshold on a device (a build-time
/// `--define` reaches it through `option_env!`); see
/// [`crate::gpu::strips::split_class_runs`] for what it decides.
pub fn run_split_min() -> u32 {
    static MIN: OnceLock<u32> = OnceLock::new();
    *MIN.get_or_init(|| {
        parse_run_split_min(env_str(
            option_env!("FRUST_ENGINE_RUN_SPLIT_MIN"),
            std::env::var("FRUST_ENGINE_RUN_SPLIT_MIN").ok(),
        ))
    })
}

/// Parses a raw `FRUST_ENGINE_RUN_SPLIT_MIN` value (already resolved by
/// [`env_str`]); an unset or unparsable value is the measured default.
fn parse_run_split_min(raw: Option<String>) -> u32 {
    raw.and_then(|s| s.trim().parse().ok())
        .unwrap_or(crate::gpu::strips::RUN_SPLIT_MIN)
}

/// Whether atlas allocation is disabled via `FRUST_ENGINE_NO_ATLAS`.
/// Cached: read once per process.
///
/// Consulted by [`crate::cache::images::ImageResidency`]: when set, every
/// image resolution answers a logged skip instead of touching the atlas.
pub fn atlas_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_ENGINE_NO_ATLAS"),
            std::env::var("FRUST_ENGINE_NO_ATLAS").ok(),
        )
    })
}

/// Whether offscreen fragment-shader effects are disabled via
/// `FRUST_ENGINE_NO_SHADER_EFFECTS`. Cached: read once per process.
///
/// Consulted by [`crate::effects::shader_quad`] and by the compiler's
/// [`frust_scene::Command::ShaderQuad`] lowering: when set, no user fragment
/// program is compiled and no shader quad is drawn — the frame renders
/// everything else and reports the skip once at warning level. The escape
/// hatch for a driver that miscompiles a user shader, where the alternative
/// is losing the whole application rather than one effect.
pub fn shader_effects_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_ENGINE_NO_SHADER_EFFECTS"),
            std::env::var("FRUST_ENGINE_NO_SHADER_EFFECTS").ok(),
        )
    })
}

/// Whether layer caching is disabled via `FRUST_ENGINE_NO_LAYERS`.
/// Cached: read once per process.
///
/// Reserved: parsed and cached, consulted by nothing yet; wired when its
/// subsystem lands.
pub fn layers_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_ENGINE_NO_LAYERS"),
            std::env::var("FRUST_ENGINE_NO_LAYERS").ok(),
        )
    })
}

/// Whether resource pooling is disabled via `FRUST_ENGINE_NO_POOL`.
/// Cached: read once per process.
///
/// Reserved: parsed and cached, consulted by nothing yet; wired when its
/// subsystem lands.
pub fn pool_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_ENGINE_NO_POOL"),
            std::env::var("FRUST_ENGINE_NO_POOL").ok(),
        )
    })
}

/// Maximum texture resolution (each axis) via `FRUST_ENGINE_MAX_TEX=<n>`.
/// Parsed as an integer; defaults to 0 (no limit). Cached: read once per process.
///
/// Reserved: parsed and cached, consulted by nothing yet; wired when its
/// subsystem lands.
pub fn max_texture_size() -> u32 {
    static MAX_SIZE: OnceLock<u32> = OnceLock::new();
    *MAX_SIZE.get_or_init(|| {
        parse_max_texture_size(env_str(
            option_env!("FRUST_ENGINE_MAX_TEX"),
            std::env::var("FRUST_ENGINE_MAX_TEX").ok(),
        ))
    })
}

/// Atlas size via `FRUST_ENGINE_ATLAS_SIZE=<WxH>` (e.g., "2048x2048").
/// Parsed as `(width, height)` or returns `None` if not set or invalid.
/// Cached: read once per process.
///
/// Consulted by [`crate::cache::images::AtlasBudget`]: when set, it overrides
/// the capability-chosen per-layer atlas extent (runtime wins).
pub fn atlas_size() -> Option<(u32, u32)> {
    static ATLAS_SIZE: OnceLock<Option<(u32, u32)>> = OnceLock::new();
    *ATLAS_SIZE.get_or_init(|| {
        parse_atlas_size(env_str(
            option_env!("FRUST_ENGINE_ATLAS_SIZE"),
            std::env::var("FRUST_ENGINE_ATLAS_SIZE").ok(),
        ))
    })
}

/// Downlevel mode via `FRUST_ENGINE_DOWNLEVEL=1`.
/// Cached: read once per process.
pub fn downlevel_mode() -> bool {
    static DOWNLEVEL: OnceLock<bool> = OnceLock::new();
    *DOWNLEVEL.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_ENGINE_DOWNLEVEL"),
            std::env::var("FRUST_ENGINE_DOWNLEVEL").ok(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_flag_enabled() {
        assert!(env_flag_enabled(Some("1"), None));
        assert!(env_flag_enabled(Some("true"), None));
        assert!(env_flag_enabled(Some("TRUE"), None));
        assert!(!env_flag_enabled(Some("0"), None));
        assert!(!env_flag_enabled(Some("false"), None));
        assert!(!env_flag_enabled(None, None));
    }

    #[test]
    fn test_runtime_overrides_compile_time() {
        assert!(env_flag_enabled(Some("0"), Some("1".to_string())));
        assert!(!env_flag_enabled(Some("1"), Some("0".to_string())));
    }

    #[test]
    fn test_env_str_unset_is_none() {
        assert_eq!(env_str(None, None), None);
    }

    #[test]
    fn test_env_str_runtime_wins_over_compile_time() {
        assert_eq!(
            env_str(Some("1024"), Some("2048".to_string())),
            Some("2048".to_string())
        );
    }

    #[test]
    fn test_env_str_empty_runtime_falls_through_to_compile_time() {
        assert_eq!(
            env_str(Some("1024"), Some(String::new())),
            Some("1024".to_string())
        );
    }

    #[test]
    fn test_env_str_compile_time_only() {
        assert_eq!(env_str(Some("1024"), None), Some("1024".to_string()));
    }

    #[test]
    fn test_env_str_runtime_only() {
        assert_eq!(
            env_str(None, Some("2048".to_string())),
            Some("2048".to_string())
        );
    }

    /// The production reader itself, on the value the process was started
    /// with: unset in the gate, so the switch is off and shader effects run.
    /// Reading it here also pins that the reader parses and caches without
    /// panicking, which is what the frame path depends on.
    #[test]
    fn shader_effects_are_enabled_unless_the_switch_is_set() {
        assert!(!shader_effects_disabled());
    }

    /// The switch's own parsing rules, exercised through the combinator every
    /// `FRUST_ENGINE_*` boolean shares — including the runtime-wins
    /// precedence, which is what lets a compile-time-disabled build be
    /// re-enabled at launch.
    #[test]
    fn shader_effects_switch_parses_like_every_other_engine_flag() {
        assert!(env_flag_enabled(None, Some("1".to_string())));
        assert!(env_flag_enabled(None, Some("TRUE".to_string())));
        assert!(!env_flag_enabled(None, Some("0".to_string())));
        assert!(!env_flag_enabled(None, Some(String::new())));
        // Runtime wins in both directions.
        assert!(env_flag_enabled(Some("0"), Some("1".to_string())));
        assert!(!env_flag_enabled(Some("1"), Some("0".to_string())));
    }

    /// Unset in the gate: the classes are on unless the switch says
    /// otherwise.
    #[test]
    fn the_uber_program_is_not_forced_unless_the_switch_is_set() {
        assert!(!uber_program_forced());
    }

    /// Unset in the gate: the surface opening is merged unless the switch
    /// says otherwise.
    #[test]
    fn pass_merging_is_on_unless_the_switch_is_set() {
        assert!(!pass_merge_disabled());
    }

    #[test]
    fn the_run_split_threshold_parses_or_falls_back_to_the_measured_default() {
        use crate::gpu::strips::RUN_SPLIT_MIN;
        assert_eq!(parse_run_split_min(Some("64".to_string())), 64);
        assert_eq!(parse_run_split_min(Some(" 256 ".to_string())), 256);
        assert_eq!(parse_run_split_min(Some("0".to_string())), 0);
        assert_eq!(parse_run_split_min(Some("many".to_string())), RUN_SPLIT_MIN);
        assert_eq!(parse_run_split_min(None), RUN_SPLIT_MIN);
        assert_eq!(run_split_min(), RUN_SPLIT_MIN, "unset in the gate");
    }

    #[test]
    fn test_max_texture_size_parsing() {
        assert_eq!(parse_max_texture_size(Some("1024".to_string())), 1024);
        assert_eq!(parse_max_texture_size(Some("0".to_string())), 0);
        assert_eq!(parse_max_texture_size(Some("invalid".to_string())), 0);
        assert_eq!(parse_max_texture_size(None), 0);
    }

    #[test]
    fn test_atlas_size_parsing() {
        assert_eq!(
            parse_atlas_size(Some("2048x2048".to_string())),
            Some((2048, 2048))
        );
        assert_eq!(
            parse_atlas_size(Some("1024x512".to_string())),
            Some((1024, 512))
        );
        assert_eq!(parse_atlas_size(Some("2048".to_string())), None);
        assert_eq!(parse_atlas_size(Some("2048x2048x2048".to_string())), None);
        assert_eq!(parse_atlas_size(None), None);
    }
}
