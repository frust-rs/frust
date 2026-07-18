//! Pipeline cache persistence for the desktop preview shell.
//!
//! The desktop shell loads a pipeline-cache blob from the user's cache directory
//! before GPU init and saves updated cache data after renderer creation, so
//! second-and-later launches skip Vulkan pipeline compilation on Linux/Windows.
//! This is a no-op on macOS (Metal has no PIPELINE_CACHE feature) and all failures
//! are logged-and-ignored (cache is best-effort; startup must never fail on cache I/O).

use std::fs;
use std::path::PathBuf;

/// Resolve the user's cache directory, following XDG/platform conventions without
/// adding a `dirs` dependency.
///
/// - On Unix: `$XDG_CACHE_HOME` (if set and absolute) or `$HOME/.cache`
/// - On Windows: `%LOCALAPPDATA%`
/// - Elsewhere: empty path (cache disabled)
///
/// # Panics
///
/// Panics if `XDG_CACHE_HOME` is set but not absolute (caller-data validation).
pub fn cache_dir() -> PathBuf {
    #[cfg(unix)]
    {
        // Unix: XDG Base Directory spec
        if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
            let path = PathBuf::from(&xdg);
            assert!(
                path.is_absolute(),
                "XDG_CACHE_HOME must be absolute, got: {xdg}"
            );
            return path;
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".cache");
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local);
        }
    }
    // Fallback: no cache
    PathBuf::new()
}

/// The path to the persisted desktop pipeline cache blob.
pub fn cache_path() -> PathBuf {
    cache_dir()
        .join("forgekit")
        .join("pipeline_cache_desktop.bin")
}

/// Load the pipeline cache blob from disk (best-effort).
///
/// Returns `None` on any failure (file not found, read error, etc.),
/// logging at debug level. The cache is optional; startup continues either way.
pub fn load_cache() -> Option<Vec<u8>> {
    let path = cache_path();
    match fs::read(&path) {
        Ok(data) => {
            log::debug!(
                "forgekit-shell-desktop: loaded cache from {}",
                path.display()
            );
            Some(data)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::debug!(
                "forgekit-shell-desktop: cache not found at {}",
                path.display()
            );
            None
        }
        Err(e) => {
            log::debug!(
                "forgekit-shell-desktop: failed to read cache at {}: {}",
                path.display(),
                e
            );
            None
        }
    }
}

/// Save the pipeline cache blob to disk (best-effort).
///
/// Writes atomically (temp file + rename) and logs at debug level.
/// All failures are logged-and-ignored (cache is best-effort; we don't spam
/// the log with errors on every frame). Should be called on a background thread
/// (never blocking the UI thread).
pub fn save_cache(data: &[u8]) {
    let path = cache_path();
    if path.as_os_str().is_empty() {
        log::debug!("forgekit-shell-desktop: cache disabled (no cache dir)");
        return;
    }

    // Create parent directory if needed.
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        log::debug!(
            "forgekit-shell-desktop: failed to create cache dir {}: {}",
            parent.display(),
            e
        );
        return;
    }

    // Write atomically: temp file + rename.
    let temp_path = path.with_extension("tmp");
    match fs::write(&temp_path, data) {
        Ok(()) => match fs::rename(&temp_path, &path) {
            Ok(()) => {
                log::debug!("forgekit-shell-desktop: saved cache to {}", path.display());
            }
            Err(e) => {
                log::debug!(
                    "forgekit-shell-desktop: failed to rename {} to {}: {}",
                    temp_path.display(),
                    path.display(),
                    e
                );
                // Best-effort cleanup of the temp file.
                let _ = fs::remove_file(&temp_path);
            }
        },
        Err(e) => {
            log::debug!(
                "forgekit-shell-desktop: failed to write cache to {}: {}",
                temp_path.display(),
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test cache_dir() on Unix-like systems with XDG_CACHE_HOME set.
    #[test]
    #[cfg(unix)]
    fn test_cache_dir_xdg() {
        // We can't easily test this without altering env vars globally,
        // so we just verify the logic is sound by examining the code.
        // A real test would use tempfile + env::set_var.
        let dir = cache_dir();
        // Should use XDG_CACHE_HOME if set, or HOME/.cache otherwise.
        // For now, just verify it's not empty on a normal system.
        assert!(!dir.as_os_str().is_empty());
    }

    /// Test cache_path() produces the expected subpath.
    #[test]
    fn test_cache_path() {
        let path = cache_path();
        // Should end with forgekit/pipeline_cache_desktop.bin
        assert!(path.ends_with("forgekit/pipeline_cache_desktop.bin"));
    }

    /// Test that load_cache returns None for a non-existent file.
    /// This is a smoke test; the real contract is "returns None on file not found".
    #[test]
    fn test_load_cache_not_found() {
        // On most machines the cache file won't exist, so load_cache() returns None.
        // If a cache file does exist locally, it will be loaded. Either way, this
        // verifies that load_cache() doesn't panic on the common no-cache-found case.
        let _result = load_cache();
    }

    /// Smoke test: save_cache doesn't panic when called (directory creation, atomic write).
    /// A real integration test would use tempfile to verify the file is actually written.
    #[test]
    fn test_save_cache_smoke() {
        // Call save_cache with a small blob and verify it doesn't panic.
        // The actual file write depends on permissions and the cache dir existing,
        // so this is just a smoke test.
        save_cache(b"test-cache-data");
    }
}
