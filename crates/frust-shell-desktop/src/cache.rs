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
//!
//! It also owns its own **file-level** macOS legacy-base read-through on
//! load: the app stem namespaces this module's *filename*, not a directory,
//! so `frust_paths::cache_dir()`'s built-in `<base>/<app_stem>` probe never
//! matches it (see `frust_paths::legacy_cache_dir`).

use std::fs;
use std::path::{Path, PathBuf};

/// `<base>/frust/pipeline_cache_desktop_<app_stem>.bin` — this module's
/// on-disk filename shape, over any base directory so the legacy-base
/// read-through below (and tests) can build the same shape elsewhere.
fn cache_file_path(base: &Path) -> PathBuf {
    let mut file = std::ffi::OsString::from("pipeline_cache_desktop_");
    file.push(frust_paths::app_stem());
    file.push(".bin");
    base.join("frust").join(file)
}

/// The path the desktop pipeline cache blob is **written** to, or `None`
/// when caching is disabled (no resolvable cache directory — see
/// `frust_paths::cache_dir`).
///
/// The path is namespaced per binary (the current executable's file stem,
/// via `frust_paths::app_stem`): every frust desktop app on a machine would
/// otherwise share one file and concurrent apps would clobber each other's
/// freshly-written cache. Note the stem namespaces the *filename*, not a
/// directory, so `frust_paths::cache_dir()`'s own built-in macOS
/// `<base>/<app_stem>` directory probe can't see this file — the load path
/// does its own file-level read-through instead (see [`load_cache`]).
pub fn cache_path() -> Option<PathBuf> {
    Some(cache_file_path(&frust_paths::cache_dir()?))
}

/// Load the pipeline cache blob from disk (best-effort).
///
/// Returns `None` on any failure (file not found, read error, etc.),
/// logging at debug level. The cache is optional; startup continues either way.
///
/// On macOS this reads through to the legacy cache base
/// (`frust_paths::legacy_cache_dir`) when the current-location file is
/// absent, so a cache written before `frust-paths` grew its dedicated macOS
/// arm is still found. Nothing is migrated, copied, or deleted — saves keep
/// writing [`cache_path`], and the stale legacy blob simply ages out.
pub fn load_cache() -> Option<Vec<u8>> {
    let path = load_path(&cache_path()?, frust_paths::legacy_cache_dir().as_deref());
    load_cache_from(&path)
}

/// [`load_cache`]'s legacy-base read-through, parameterized over both bases
/// so tests drive it against temp dirs: `new_path` unless that file is
/// absent and the same filename shape under `legacy_base` exists.
///
/// `legacy_base` is `None` on every non-macOS target (see
/// `frust_paths::legacy_cache_dir`), making this an identity function
/// there — the `if let Some(legacy_base) = legacy_base` operand order below
/// is load-bearing for that: it short-circuits before the `try_exists`
/// stat ever runs, so a non-macOS launch is genuinely syscall-free here,
/// not just discarding the stat's result.
///
/// Uses `try_exists` (not `exists`) so a momentarily unstat-able `new_path`
/// (EACCES/ELOOP/dangling symlink) is treated as PRESENT — never silently
/// diverted to a stale legacy cache; the subsequent read surfaces the real
/// error on the canonical path. `Err(_)` on the legacy probe means treat it
/// as absent — never divert onto a broken legacy path either.
fn load_path(new_path: &Path, legacy_base: Option<&Path>) -> PathBuf {
    if let Some(legacy_base) = legacy_base
        && !new_path.try_exists().unwrap_or(true)
    {
        let legacy_path = cache_file_path(legacy_base);
        if legacy_path.try_exists().unwrap_or(false) {
            log::debug!(
                "frust-shell-desktop: cache not found at {}, reading legacy {}",
                new_path.display(),
                legacy_path.display()
            );
            return legacy_path;
        }
    }
    new_path.to_path_buf()
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
    ///
    /// The unique leaf directory (`frust-cache-test-<pid>-<tag>`) is
    /// created eagerly with `fs::create_dir` (fails loudly if the name is
    /// already occupied, e.g. by a planted symlink) rather than left for a
    /// later `create_dir_all` to walk through silently.
    fn scratch_path(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("frust-cache-test-{}-{}", std::process::id(), tag));
        fs::create_dir(&root).expect("scratch root must not already exist (planted path?)");
        root.join("frust").join("pipeline_cache_desktop.bin")
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

    // --- Legacy-base read-through on load (macOS shape) ------------------

    /// A unique scratch *base* directory (the `cache_dir()` analog) under
    /// the OS temp dir — the read-through tests probe the real filesystem,
    /// so they must never touch the user's real `~/.cache`.
    ///
    /// Created eagerly with `fs::create_dir` (fails loudly if the name is
    /// already occupied, e.g. by a planted symlink) rather than left for a
    /// later `create_dir_all` to walk through silently.
    fn scratch_base(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frust-cache-test-{}-base-{}",
            std::process::id(),
            tag
        ));
        fs::create_dir(&dir).expect("scratch base must not already exist (planted path?)");
        dir
    }

    /// Write a cache blob at this module's real filename shape under
    /// `base`, returning its path.
    fn seed_cache_file(base: &Path, data: &[u8]) -> PathBuf {
        let path = cache_file_path(base);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, data).unwrap();
        path
    }

    /// **Negative control**: the legacy base holds only
    /// `frust/pipeline_cache_desktop_<stem>.bin` and deliberately no
    /// `<app_stem>/` directory — the shape `frust_paths::cache_dir()`'s own
    /// built-in probe looks for. The load path must still find it, and must
    /// return the legacy bytes.
    #[test]
    fn test_load_path_reads_through_to_legacy_file_with_no_app_stem_dir() {
        let new_base = scratch_base("rt-new");
        let legacy_base = scratch_base("rt-legacy");
        let legacy_path = seed_cache_file(&legacy_base, b"legacy-blob");

        let stem_dir = legacy_base.join(frust_paths::app_stem());
        assert!(!stem_dir.exists(), "test fixture must have no app-stem dir");

        let resolved = load_path(&cache_file_path(&new_base), Some(&legacy_base));
        assert_eq!(resolved, legacy_path);
        assert_eq!(
            load_cache_from(&resolved).as_deref(),
            Some(&b"legacy-blob"[..])
        );

        let _ = fs::remove_dir_all(&new_base);
        let _ = fs::remove_dir_all(&legacy_base);
    }

    /// A cache at the current location wins even when a legacy one exists,
    /// and the legacy file is left untouched (read-through never migrates).
    #[test]
    fn test_load_path_prefers_the_new_file_when_both_exist() {
        let new_base = scratch_base("both-new");
        let legacy_base = scratch_base("both-legacy");
        let new_path = seed_cache_file(&new_base, b"new-blob");
        let legacy_path = seed_cache_file(&legacy_base, b"legacy-blob");

        let resolved = load_path(&new_path, Some(&legacy_base));
        assert_eq!(resolved, new_path);
        assert_eq!(
            load_cache_from(&resolved).as_deref(),
            Some(&b"new-blob"[..])
        );
        assert!(legacy_path.exists(), "legacy file must be left untouched");

        let _ = fs::remove_dir_all(&new_base);
        let _ = fs::remove_dir_all(&legacy_base);
    }

    /// `Path::try_exists` (not `Path::exists`) semantics: a new-location
    /// probe that errors (here, `EACCES` from an unreadable ancestor
    /// directory) must never be treated as "absent" and diverted to a
    /// legacy cache — `Err(_)` on the new-path probe means treat it as
    /// PRESENT, so the unresolvable path is returned unchanged and a later
    /// read surfaces the real error. This fails under the old
    /// `Path::exists()` behaviour, which swallows the permission error into
    /// `false` and would wrongly divert to `legacy_path` below.
    #[test]
    #[cfg(unix)]
    fn test_load_path_does_not_divert_when_new_path_is_unstatable() {
        use std::os::unix::fs::PermissionsExt;

        let new_base = scratch_base("unstatable-new");
        let locked_dir = new_base.join("locked");
        fs::create_dir(&locked_dir).unwrap();
        fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o000)).unwrap();

        let legacy_base = scratch_base("unstatable-legacy");
        let legacy_path = seed_cache_file(&legacy_base, b"legacy-blob");

        let new_path = cache_file_path(&locked_dir);
        if new_path.try_exists().is_ok() {
            // Running as root (or under some other permission-bypassing
            // capability): a 0o000 directory doesn't block traversal, so
            // this fixture can't produce the EACCES this test targets.
            // Restore permissions so cleanup below can actually remove the
            // directory, then skip rather than assert something the
            // fixture didn't exercise.
            fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o700)).unwrap();
            let _ = fs::remove_dir_all(&new_base);
            let _ = fs::remove_dir_all(&legacy_base);
            return;
        }

        let resolved = load_path(&new_path, Some(&legacy_base));
        assert_eq!(
            resolved, new_path,
            "an unstatable new path must not divert to the legacy cache"
        );
        assert_ne!(resolved, legacy_path);

        fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let _ = fs::remove_dir_all(&new_base);
        let _ = fs::remove_dir_all(&legacy_base);
    }

    /// Neither file exists, or there is no legacy base at all (every
    /// non-macOS target): the current-location path is returned unchanged,
    /// so a later save still writes where `cache_path()` points.
    #[test]
    fn test_load_path_falls_back_to_the_new_path() {
        let new_base = scratch_base("none-new");
        let legacy_base = scratch_base("none-legacy");
        let new_path = cache_file_path(&new_base);

        assert_eq!(load_path(&new_path, Some(&legacy_base)), new_path);
        assert_eq!(load_path(&new_path, None), new_path);
    }

    /// `cache_path()` is `cache_file_path()` over the resolved cache dir —
    /// the shape the read-through rebuilds under a legacy base.
    #[test]
    fn test_cache_path_is_the_shared_filename_shape() {
        let base = frust_paths::cache_dir().expect("cache dir resolvable on test hosts");
        assert_eq!(cache_path(), Some(cache_file_path(&base)));
    }
}
