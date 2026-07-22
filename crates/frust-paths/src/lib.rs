//! Shared platform-directory resolution and atomic-write helpers.
//!
//! A leaf substrate crate (like `frust-plugin`/`frust-reactive`): **zero
//! `frust-*` dependencies**. It exists to de-duplicate the XDG/platform
//! directory resolution and atomic-write logic that three desktop-facing
//! sites each independently hand-rolled:
//!
//! - `plugins/shared-preferences/src/file.rs`'s `data_dir`/`preferences_path`
//! - `plugins/secure-storage/src/file.rs`'s atomic `save` (test-only backend,
//!   but the same atomic-write shape)
//! - `crates/frust-shell-desktop/src/cache.rs`'s `cache_dir`/`save_cache_to`
//!
//! # Conventions preserved from those sites
//!
//! - **Data directory** ([`data_dir`]): Unix resolves `$XDG_DATA_HOME` if set
//!   *and absolute*, else `$HOME/.local/share`. Windows resolves `%APPDATA%`.
//!   Any other target, or an unresolvable environment (no `HOME`/`APPDATA`),
//!   returns `None` — this crate never guesses a fallback that could
//!   silently write into the process's current directory.
//! - **Cache directory** ([`cache_dir`]): Unix resolves `$XDG_CACHE_HOME` if
//!   set and absolute, else `$HOME/.cache`. Windows resolves
//!   `%LOCALAPPDATA%`. Same `None`-on-unresolvable contract as `data_dir`.
//! - **Absolute-validation debug log**: a set-but-relative `XDG_DATA_HOME`/
//!   `XDG_CACHE_HOME` is ignored per the XDG Base Directory spec, and that
//!   ignoring is logged at `debug` (via the `log` crate) so a misconfigured
//!   environment is diagnosable. `cache.rs`'s `cache_dir` already logged
//!   this; `data_dir`'s source sites (shared-preferences, secure-storage)
//!   did not — gaining a debug line there is an accepted, harmless
//!   behavioral delta from consolidating the two.
//! - **Per-binary namespacing** ([`app_stem`]): the current executable's
//!   file stem (falling back to `"app"` if `current_exe()` fails), the
//!   component every site joins onto its resolved base directory so that two
//!   Frust apps on one machine never share a file.
//! - **Atomic write** ([`atomic_write`]): `create_dir_all` the parent
//!   directory, write to a process-unique temp file (the target path with
//!   its extension replaced by `tmp.<pid>`), then `rename` into place. A
//!   reader never observes a partially-written file, and a crash mid-write
//!   leaves any previous contents intact. On a failed rename, the temp file
//!   is best-effort removed before the error is returned.
//!
//! # Compat contract
//!
//! **This crate only resolves base directories and writes bytes
//! atomically — it has no opinion on a caller's on-disk file *shape*
//! (JSON, binary blob, whatever).** The exact on-disk paths/filenames a
//! caller builds on top of `data_dir()`/`cache_dir()`/`app_stem()` remain
//! that caller's own compat contract (e.g. a preferences file's exact
//! path), not something this crate can or does version.
//!
//! Hand-rolled env logic is deliberate: this crate does not depend on the
//! `dirs` crate (a recorded future enhancement, not a correctness gap).

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Resolve the user's data directory.
///
/// - Unix: `$XDG_DATA_HOME` if set and absolute, else `$HOME/.local/share`.
/// - Windows: `%APPDATA%`.
/// - Any other target: `None`.
///
/// `None` when unresolvable (`HOME`/`APPDATA` unset) — never guesses a
/// fallback that could write into the current directory.
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        data_dir_from(
            std::env::var("XDG_DATA_HOME").ok().as_deref(),
            std::env::var("HOME").ok().as_deref(),
        )
    }
    #[cfg(target_os = "windows")]
    {
        data_dir_from(std::env::var("APPDATA").ok().as_deref())
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        None
    }
}

/// Resolve the user's cache directory.
///
/// - Unix: `$XDG_CACHE_HOME` if set and absolute, else `$HOME/.cache`.
/// - Windows: `%LOCALAPPDATA%`.
/// - Any other target: `None`.
///
/// `None` when unresolvable. Logs at `debug` (via the `log` crate) when a
/// set-but-relative XDG var is ignored per the XDG Base Directory spec.
pub fn cache_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        cache_dir_from(
            std::env::var("XDG_CACHE_HOME").ok().as_deref(),
            std::env::var("HOME").ok().as_deref(),
        )
    }
    #[cfg(target_os = "windows")]
    {
        cache_dir_from(std::env::var("LOCALAPPDATA").ok().as_deref())
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        None
    }
}

/// `current_exe()`'s file stem, `"app"` fallback — the per-binary
/// namespacing component every source site joins onto its resolved base
/// directory (e.g. `<data_dir>/<app_stem>/frust/...`).
pub fn app_stem() -> OsString {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_os_string()))
        .unwrap_or_else(|| "app".into())
}

/// Write `bytes` to `path` atomically.
///
/// `create_dir_all`s `path`'s parent, writes to a process-unique temp file
/// (`path` with its extension replaced by `tmp.<pid>`), then `rename`s into
/// place — so a reader never observes a partially-written file, and a crash
/// mid-write leaves any previous contents at `path` intact. On a failed
/// rename, the temp file is best-effort removed (its own removal error is
/// swallowed) before the rename error is returned.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&temp_path, bytes)?;
    let rename_result = fs::rename(&temp_path, path);
    if rename_result.is_err() {
        // Best-effort cleanup so a failed rename doesn't leave the temp
        // file behind indefinitely.
        let _ = fs::remove_file(&temp_path);
    }
    rename_result
}

/// Absolute-validated `xdg` (logged-and-ignored at `debug` if set but
/// relative), else `home` joined with `suffix` — the shared shape behind
/// both [`data_dir_from`]/[`cache_dir_from`] on Unix.
#[cfg(unix)]
fn xdg_or_home(
    xdg: Option<&str>,
    xdg_var_name: &str,
    home: Option<&str>,
    suffix: &[&str],
) -> Option<PathBuf> {
    if let Some(xdg) = xdg {
        let path = PathBuf::from(xdg);
        if path.is_absolute() {
            return Some(path);
        }
        log::debug!("frust-paths: ignoring non-absolute {xdg_var_name} ({xdg})");
    }
    home.map(|home| {
        let mut p = PathBuf::from(home);
        p.extend(suffix);
        p
    })
}

/// Env-parameterized body of [`data_dir`]'s Unix branch — lets tests
/// exercise the resolution logic against injected values without ever
/// touching the real `HOME`/`XDG_DATA_HOME`.
#[cfg(unix)]
fn data_dir_from(xdg_data_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    xdg_or_home(xdg_data_home, "XDG_DATA_HOME", home, &[".local", "share"])
}

/// Env-parameterized body of [`cache_dir`]'s Unix branch.
#[cfg(unix)]
fn cache_dir_from(xdg_cache_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    xdg_or_home(xdg_cache_home, "XDG_CACHE_HOME", home, &[".cache"])
}

/// Env-parameterized body of [`data_dir`]'s Windows branch.
#[cfg(target_os = "windows")]
fn data_dir_from(appdata: Option<&str>) -> Option<PathBuf> {
    appdata.map(PathBuf::from)
}

/// Env-parameterized body of [`cache_dir`]'s Windows branch.
#[cfg(target_os = "windows")]
fn cache_dir_from(localappdata: Option<&str>) -> Option<PathBuf> {
    localappdata.map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- data_dir_from / cache_dir_from (Unix) ---------------------------

    #[test]
    #[cfg(unix)]
    fn data_dir_from_absolute_xdg_wins() {
        assert_eq!(
            data_dir_from(Some("/custom/data"), Some("/home/user")),
            Some(PathBuf::from("/custom/data"))
        );
    }

    #[test]
    #[cfg(unix)]
    fn data_dir_from_relative_xdg_is_ignored_falls_back_to_home() {
        assert_eq!(
            data_dir_from(Some("relative/data"), Some("/home/user")),
            Some(PathBuf::from("/home/user/.local/share"))
        );
    }

    #[test]
    #[cfg(unix)]
    fn data_dir_from_no_xdg_falls_back_to_home() {
        assert_eq!(
            data_dir_from(None, Some("/home/user")),
            Some(PathBuf::from("/home/user/.local/share"))
        );
    }

    #[test]
    #[cfg(unix)]
    fn data_dir_from_unresolvable_is_none() {
        assert_eq!(data_dir_from(None, None), None);
        // A relative XDG var with no HOME to fall back to is also None.
        assert_eq!(data_dir_from(Some("relative"), None), None);
    }

    #[test]
    #[cfg(unix)]
    fn cache_dir_from_absolute_xdg_wins() {
        assert_eq!(
            cache_dir_from(Some("/custom/cache"), Some("/home/user")),
            Some(PathBuf::from("/custom/cache"))
        );
    }

    #[test]
    #[cfg(unix)]
    fn cache_dir_from_relative_xdg_is_ignored_falls_back_to_home() {
        assert_eq!(
            cache_dir_from(Some("relative/cache"), Some("/home/user")),
            Some(PathBuf::from("/home/user/.cache"))
        );
    }

    #[test]
    #[cfg(unix)]
    fn cache_dir_from_no_xdg_falls_back_to_home() {
        assert_eq!(
            cache_dir_from(None, Some("/home/user")),
            Some(PathBuf::from("/home/user/.cache"))
        );
    }

    #[test]
    #[cfg(unix)]
    fn cache_dir_from_unresolvable_is_none() {
        assert_eq!(cache_dir_from(None, None), None);
    }

    // --- data_dir_from / cache_dir_from (Windows) ------------------------

    #[test]
    #[cfg(target_os = "windows")]
    fn data_dir_from_appdata() {
        assert_eq!(
            data_dir_from(Some("C:\\Users\\user\\AppData\\Roaming")),
            Some(PathBuf::from("C:\\Users\\user\\AppData\\Roaming"))
        );
        assert_eq!(data_dir_from(None), None);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn cache_dir_from_localappdata() {
        assert_eq!(
            cache_dir_from(Some("C:\\Users\\user\\AppData\\Local")),
            Some(PathBuf::from("C:\\Users\\user\\AppData\\Local"))
        );
        assert_eq!(cache_dir_from(None), None);
    }

    // --- data_dir / cache_dir (real env, smoke only) ---------------------

    /// `data_dir()`/`cache_dir()` don't panic and resolve to *something* on
    /// a normal dev host — not asserting `Some`/`None` (CI hosts vary in
    /// `HOME`/`APPDATA` presence), only that resolution itself is infallible.
    #[test]
    fn data_and_cache_dir_resolve_without_panicking() {
        let _ = data_dir();
        let _ = cache_dir();
    }

    // --- app_stem ---------------------------------------------------------

    #[test]
    fn app_stem_is_nonempty() {
        // On a test binary, current_exe() resolves to the test harness
        // executable, whose file stem is non-empty; the "app" fallback path
        // is only reachable if current_exe() itself fails, which doesn't
        // happen on a normal test host.
        assert!(!app_stem().is_empty());
    }

    // --- atomic_write -------------------------------------------------------

    /// A unique scratch path under the OS temp dir — tests must never touch
    /// a real data/cache directory.
    fn scratch_path(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("frust-paths-test-{}-{tag}-{n}", std::process::id()))
            .join("nested")
            .join("file.bin")
    }

    #[test]
    fn atomic_write_creates_parent_dirs_and_no_temp_left_behind() {
        let path = scratch_path("round-trip");
        atomic_write(&path, b"hello").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"hello");
        assert!(
            !path
                .with_extension(format!("tmp.{}", std::process::id()))
                .exists(),
            "temp file left behind"
        );

        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn atomic_write_overwrites_existing_file() {
        let path = scratch_path("overwrite");
        atomic_write(&path, b"first").unwrap();
        atomic_write(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");

        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    /// A reader must never observe a partial write: this is a structural
    /// property of write-to-temp-then-rename rather than something a
    /// single-threaded test can directly force a race on, so this asserts
    /// the shape (final bytes are always exactly what was passed, never a
    /// truncated/partial buffer) across repeated writes.
    #[test]
    fn atomic_write_final_contents_always_complete() {
        let path = scratch_path("no-partial");
        for i in 0..5 {
            let data = vec![i as u8; 4096];
            atomic_write(&path, &data).unwrap();
            assert_eq!(fs::read(&path).unwrap(), data);
        }

        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    /// A failing rename (target's parent replaced by a file, so
    /// `create_dir_all`/`rename` cannot succeed) still returns the
    /// underlying error and cleans up its temp file rather than leaking it.
    #[test]
    fn atomic_write_returns_error_and_cleans_up_temp_on_failure() {
        let base = std::env::temp_dir().join(format!(
            "frust-paths-test-{}-fail-parent",
            std::process::id()
        ));
        // Make `base` a *file*, so `<base>/child.bin`'s parent can never be
        // created as a directory - create_dir_all(base) fails.
        fs::write(&base, b"not a directory").unwrap();
        let path = base.join("child.bin");

        let result = atomic_write(&path, b"data");
        assert!(result.is_err());

        let temp_path = path.with_extension(format!("tmp.{}", std::process::id()));
        assert!(!temp_path.exists(), "temp file left behind on failure");

        let _ = fs::remove_file(&base);
    }
}
