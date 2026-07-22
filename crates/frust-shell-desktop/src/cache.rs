//! Pipeline cache persistence for the desktop preview shell.
//!
//! The desktop shell loads a pipeline-cache blob from the user's cache directory
//! before GPU init and saves updated cache data after renderer creation, so
//! second-and-later launches skip Vulkan pipeline compilation on Linux/Windows.
//! This is a no-op on macOS (Metal has no PIPELINE_CACHE feature) and all failures
//! are logged-and-ignored (cache is best-effort; startup must never fail on cache I/O).
//!
//! Cache-dir resolution, per-binary app-stem namespacing, and the atomic
//! temp-file+rename write all delegate to `frust-paths` (this crate no
//! longer hand-rolls `XDG_CACHE_HOME`/`LOCALAPPDATA`/`current_exe`/`rename`
//! logic) — this module keeps only the on-disk filename shape, the
//! skip-unchanged check, and its own debug/info logging of outcome.

use std::fs;
use std::path::PathBuf;

/// The path to the persisted desktop pipeline cache blob, or `None` when
/// caching is disabled (no resolvable cache directory — see
/// `frust_paths::cache_dir`).
///
/// The path is namespaced per binary (the current executable's file stem,
/// via `frust_paths::app_stem`): every frust desktop app on a machine would
/// otherwise share one file and concurrent apps would clobber each other's
/// freshly-written cache.
pub fn cache_path() -> Option<PathBuf> {
    let dir = frust_paths::cache_dir()?;
    let mut file = std::ffi::OsString::from("pipeline_cache_desktop_");
    file.push(frust_paths::app_stem());
    file.push(".bin");
    Some(dir.join("frust").join(file))
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
            log::debug!("frust-shell-desktop: loaded cache from {}", path.display());
            Some(data)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::debug!("frust-shell-desktop: cache not found at {}", path.display());
            None
        }
        Err(e) => {
            log::debug!(
                "frust-shell-desktop: failed to read cache at {}: {}",
                path.display(),
                e
            );
            None
        }
    }
}

/// Save the pipeline cache blob to disk (best-effort).
///
/// Writes atomically (`frust_paths::atomic_write`: process-unique temp file
/// plus rename) and logs at debug level. Skips the write when the on-disk blob
/// is already byte-identical (mirrors the Android shell's differs-check —
/// no pointless rewrite every launch). All failures are logged-and-ignored
/// (cache is best-effort; we don't spam the log with errors on every
/// frame). Should be called on a background thread (never blocking the UI
/// thread).
pub fn save_cache(data: &[u8]) {
    let Some(path) = cache_path() else {
        log::debug!("frust-shell-desktop: cache disabled (no cache dir)");
        return;
    };
    if !cache_differs(load_cache_from(&path).as_deref(), data) {
        log::debug!(
            "frust-shell-desktop: cache unchanged, skipping write to {}",
            path.display()
        );
        return;
    }
    save_cache_to(&path, data);
}

/// Whether `new` differs from the previously-loaded blob — the pure seam
/// behind [`save_cache`]'s skip-when-unchanged check, mirroring the Android
/// shell's `ffi_support::pipeline_cache_differs` (same semantics, unit-tested
/// the same way). `None` (nothing on disk) always differs.
fn cache_differs(loaded: Option<&[u8]>, new: &[u8]) -> bool {
    loaded != Some(new)
}

/// Path-parameterized body of [`save_cache`] — lets tests exercise the
/// atomic-write logic against a scratch path without ever touching the
/// user's real cache directory.
fn save_cache_to(path: &std::path::Path, data: &[u8]) {
    match frust_paths::atomic_write(path, data) {
        Ok(()) => {
            log::debug!("frust-shell-desktop: saved cache to {}", path.display());
        }
        Err(e) => {
            log::debug!(
                "frust-shell-desktop: failed to save cache to {}: {}",
                path.display(),
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cache_path()` is Some on a normal system, lives under a `frust/`
    /// dir, and is namespaced by the current executable's file stem.
    #[test]
    fn test_cache_path_is_per_binary() {
        let path = cache_path().expect("cache dir resolvable on test hosts");
        assert!(path.parent().unwrap().ends_with("frust"));
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
            .join(format!("frust-cache-test-{}-{}", std::process::id(), tag))
            .join("frust")
            .join("pipeline_cache_desktop.bin")
    }

    /// The pure differs-check behind save_cache's skip-when-unchanged path
    /// (mirror of Android's `pipeline_cache_differs` test coverage).
    #[test]
    fn test_cache_differs() {
        assert!(cache_differs(None, b"x"), "nothing on disk always differs");
        assert!(
            cache_differs(None, b""),
            "empty new blob vs no file differs"
        );
        assert!(
            !cache_differs(Some(b"x"), b"x"),
            "identical bytes: no write"
        );
        assert!(cache_differs(Some(b"x"), b"y"), "changed bytes differ");
        assert!(cache_differs(Some(b"x"), b"xy"), "length change differs");
        assert!(!cache_differs(Some(b""), b""), "both empty: no write");
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
