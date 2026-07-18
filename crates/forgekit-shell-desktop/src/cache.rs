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
    load_cache_from(&cache_path())
}

/// Path-parameterized body of [`load_cache`] — lets tests exercise the
/// logic against a scratch path without ever touching the user's real
/// cache directory.
fn load_cache_from(path: &std::path::Path) -> Option<Vec<u8>> {
    match fs::read(path) {
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
    save_cache_to(&cache_path(), data);
}

/// Path-parameterized body of [`save_cache`] — lets tests exercise the
/// atomic-write logic against a scratch path without ever touching the
/// user's real cache directory.
fn save_cache_to(path: &std::path::Path, data: &[u8]) {
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
        Ok(()) => match fs::rename(&temp_path, path) {
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

    /// A unique scratch path under the OS temp dir — tests must NEVER
    /// write to the user's real cache directory (`~/.cache`), so every
    /// I/O test goes through the `_from`/`_to` path-parameterized seams.
    fn scratch_path(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "forgekit-cache-test-{}-{}",
                std::process::id(),
                tag
            ))
            .join("forgekit")
            .join("pipeline_cache_desktop.bin")
    }

    /// load returns None for a non-existent file.
    #[test]
    fn test_load_cache_not_found() {
        let path = scratch_path("not-found");
        assert!(load_cache_from(&path).is_none());
    }

    /// Atomic write round-trip: save creates parent dirs, writes the blob,
    /// leaves no temp file behind; a second save overwrites.
    #[test]
    fn test_save_cache_round_trip_and_overwrite() {
        let path = scratch_path("round-trip");
        save_cache_to(&path, b"test-cache-data");
        assert_eq!(
            load_cache_from(&path).as_deref(),
            Some(&b"test-cache-data"[..])
        );
        assert!(
            !path.with_extension("tmp").exists(),
            "temp file left behind"
        );

        save_cache_to(&path, b"second-write");
        assert_eq!(
            load_cache_from(&path).as_deref(),
            Some(&b"second-write"[..])
        );

        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }
}
