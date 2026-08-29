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

/// Whether atlas allocation is disabled via `FRUST_ENGINE_NO_ATLAS`.
/// Cached: read once per process.
pub fn atlas_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_ENGINE_NO_ATLAS"),
            std::env::var("FRUST_ENGINE_NO_ATLAS").ok(),
        )
    })
}

/// Whether layer caching is disabled via `FRUST_ENGINE_NO_LAYERS`.
/// Cached: read once per process.
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
pub fn max_texture_size() -> u32 {
    static MAX_SIZE: OnceLock<u32> = OnceLock::new();
    *MAX_SIZE.get_or_init(|| {
        if let Some(val) = option_env!("FRUST_ENGINE_MAX_TEX") {
            val.parse().ok().unwrap_or(0)
        } else if let Ok(val) = std::env::var("FRUST_ENGINE_MAX_TEX") {
            val.parse().ok().unwrap_or(0)
        } else {
            0
        }
    })
}

/// Atlas size via `FRUST_ENGINE_ATLAS_SIZE=<WxH>` (e.g., "2048x2048").
/// Parsed as `(width, height)` or returns `None` if not set or invalid.
/// Cached: read once per process.
pub fn atlas_size() -> Option<(u32, u32)> {
    static ATLAS_SIZE: OnceLock<Option<(u32, u32)>> = OnceLock::new();
    *ATLAS_SIZE.get_or_init(|| {
        let val = if let Some(v) = option_env!("FRUST_ENGINE_ATLAS_SIZE") {
            Some(v.to_string())
        } else {
            std::env::var("FRUST_ENGINE_ATLAS_SIZE").ok()
        };
        val.and_then(|s| {
            let parts: Vec<&str> = s.split('x').collect();
            if parts.len() == 2 {
                let w = parts[0].parse::<u32>().ok()?;
                let h = parts[1].parse::<u32>().ok()?;
                Some((w, h))
            } else {
                None
            }
        })
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
    fn test_max_texture_size_parsing() {
        assert_eq!(parse_max_size_str("1024"), Some(1024));
        assert_eq!(parse_max_size_str("0"), Some(0));
        assert_eq!(parse_max_size_str("invalid"), None);
    }

    #[test]
    fn test_atlas_size_parsing() {
        assert_eq!(parse_atlas_size_str("2048x2048"), Some((2048, 2048)));
        assert_eq!(parse_atlas_size_str("1024x512"), Some((1024, 512)));
        assert_eq!(parse_atlas_size_str("2048"), None);
        assert_eq!(parse_atlas_size_str("2048x2048x2048"), None);
    }

    fn parse_max_size_str(s: &str) -> Option<u32> {
        s.parse().ok()
    }

    fn parse_atlas_size_str(s: &str) -> Option<(u32, u32)> {
        let parts: Vec<&str> = s.split('x').collect();
        if parts.len() == 2 {
            let w = parts[0].parse::<u32>().ok()?;
            let h = parts[1].parse::<u32>().ok()?;
            Some((w, h))
        } else {
            None
        }
    }
}
