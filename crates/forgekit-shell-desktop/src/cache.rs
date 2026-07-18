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
/// Never panics: a set-but-relative/malformed `XDG_CACHE_HOME` (the user's
/// environment, not ours) is treated the same as an absent one — per the
/// module contract, startup must never fail on the cache path.
pub fn cache_dir() -> PathBuf {
    #[cfg(unix)]
    {
        // Unix: XDG Base Directory spec. The spec itself says a relative
        // XDG_CACHE_HOME is invalid and should be ignored.
        if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
            let path = PathBuf::from(&xdg);
            if path.is_absolute() {
                return path;
            }
            log::debug!("forgekit-shell-desktop: ignoring non-absolute XDG_CACHE_HOME ({xdg})");
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

/// The path to the persisted desktop pipeline cache blob, or `None` when
/// caching is disabled (no resolvable cache directory).
///
/// The path is namespaced per binary (the current executable's file stem):
/// every forgekit desktop app on a machine would otherwise share one file
/// and concurrent apps would clobber each other's freshly-written cache.
pub fn cache_path() -> Option<PathBuf> {
    let dir = cache_dir();
    if dir.as_os_str().is_empty() {
        // Checked on the DIR, not the joined path — a joined relative path
        // is never empty, which would silently write into the cwd.
        return None;
    }
    let app = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_os_string()))
        .unwrap_or_else(|| "app".into());
    let mut file = std::ffi::OsString::from("pipeline_cache_desktop_");
    file.push(app);
    file.push(".bin");
    Some(dir.join("forgekit").join(file))
}

/// Load the pipeline cache blob from disk (best-effort).
///
/// Returns `None` on any failure (file not found, read error, etc.),
/// logging at debug level. The cache is optional; startup continues either way.
pub fn load_cache() -> Option<Vec<u8>> {
    load_cache_from(&cache_path()?)
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
/// Writes atomically (process-unique temp file + rename) and logs at debug
/// level. Skips the write when the on-disk blob is already byte-identical
/// (mirrors the Android shell's differs-check — no pointless rewrite every
/// launch). All failures are logged-and-ignored (cache is best-effort; we
/// don't spam the log with errors on every frame). Should be called on a
/// background thread (never blocking the UI thread).
pub fn save_cache(data: &[u8]) {
    let Some(path) = cache_path() else {
        log::debug!("forgekit-shell-desktop: cache disabled (no cache dir)");
        return;
    };
    if load_cache_from(&path).as_deref() == Some(data) {
        log::debug!(
            "forgekit-shell-desktop: cache unchanged, skipping write to {}",
            path.display()
        );
        return;
    }
    save_cache_to(&path, data);
}

/// Path-parameterized body of [`save_cache`] — lets tests exercise the
/// atomic-write logic against a scratch path without ever touching the
/// user's real cache directory.
fn save_cache_to(path: &std::path::Path, data: &[u8]) {
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

    // Write atomically: process-unique temp file + rename, so two forgekit
    // processes saving concurrently never interleave writes on one temp path
    // (mirrors the Android shell's `std::process::id()` suffix).
    let temp_path = path.with_extension(format!("tmp.{}", std::process::id()));
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

    /// cache_path() is Some on a normal system, lives under a `forgekit/`
    /// dir, and is namespaced by the current executable's file stem.
    #[test]
    fn test_cache_path_is_per_binary() {
        let path = cache_path().expect("cache dir resolvable on test hosts");
        assert!(path.parent().unwrap().ends_with("forgekit"));
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(file.starts_with("pipeline_cache_desktop_"), "{file}");
        assert!(file.ends_with(".bin"), "{file}");
        let stem = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap();
        assert!(file.contains(&stem), "{file} should embed {stem}");
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
            !path
                .with_extension(format!("tmp.{}", std::process::id()))
                .exists(),
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
