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
//! - **Data directory** ([`data_dir`]): iOS resolves
//!   `$HOME/Library/Application Support` (`$HOME` is the sandbox container;
//!   the XDG shape would hit the read-only container root — see
//!   [`data_dir`]'s doc). **macOS also diverges from generic Unix**, for a
//!   different reason than iOS: it resolves the platform-conventional
//!   `$HOME/Library/Application Support` as its base, but with a
//!   **read-through legacy-XDG fallback** — if `<legacy base>/<app_stem>`
//!   exists and `<new base>/<app_stem>` does not, `data_dir()` returns the
//!   *legacy* base instead (legacy base: `$XDG_DATA_HOME` if set and
//!   absolute, else `$HOME/.local/share` — what this crate resolved on
//!   macOS before it grew a dedicated arm), so existing installs keep
//!   finding their data. This is per-app (`app_stem()`-granular), never
//!   migrates/copies/deletes anything, and XDG vars only feed the fallback
//!   probe — they do **not** override the new base on macOS. Other Unix
//!   (excluding iOS and macOS) resolves `$XDG_DATA_HOME` if set *and
//!   absolute*, else `$HOME/.local/share`. Windows resolves `%APPDATA%`.
//!   Any other target, or an unresolvable environment (no `HOME`/`APPDATA`),
//!   returns `None` — this crate never guesses a fallback that could
//!   silently write into the process's current directory.
//! - **Cache directory** ([`cache_dir`]): iOS resolves `$HOME/Library/Caches`;
//!   macOS resolves `$HOME/Library/Caches` too, with the same
//!   read-through legacy-XDG fallback as `data_dir` (legacy base:
//!   `$XDG_CACHE_HOME` if absolute, else `$HOME/.cache`). Other Unix
//!   (excluding iOS and macOS) resolves `$XDG_CACHE_HOME` if set and
//!   absolute, else `$HOME/.cache`. Windows resolves `%LOCALAPPDATA%`. Same
//!   `None`-on-unresolvable contract as `data_dir`.
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

#[cfg(any(target_os = "macos", test))]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Resolve the user's data directory.
///
/// - iOS: `$HOME/Library/Application Support` — `$HOME` is the app's own
///   sandbox container, and `Library/` is one of its three writable roots;
///   the XDG shape below would land on the container *root*, which the
///   sandbox rejects with `EPERM` on the first `create_dir_all`. XDG vars
///   are ignored here — they have no meaning inside an iOS sandbox.
/// - macOS: `$HOME/Library/Application Support`, the platform-conventional
///   location — a *different* reason to diverge from generic Unix than
///   iOS's sandbox constraint. XDG vars do **not** override this base; they
///   only feed a **read-through legacy fallback**: if
///   `<legacy base>/<app_stem>` exists and
///   `<$HOME/Library/Application Support>/<app_stem>` does not, this
///   function returns the *legacy* base instead (never migrates, copies, or
///   deletes anything), so installs that predate this crate's dedicated
///   macOS arm keep finding their data. Legacy base: `$XDG_DATA_HOME` if
///   set and absolute, else `$HOME/.local/share` — the same resolution
///   generic Unix uses below.
/// - Other Unix (excluding iOS and macOS): `$XDG_DATA_HOME` if set and
///   absolute, else `$HOME/.local/share`.
/// - Windows: `%APPDATA%`.
/// - Any other target: `None`.
///
/// `None` when unresolvable (`HOME`/`APPDATA` unset) — never guesses a
/// fallback that could write into the current directory.
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "ios")]
    {
        ios_data_dir_from(std::env::var("HOME").ok().as_deref())
    }
    #[cfg(target_os = "macos")]
    {
        macos_dir_from(
            std::env::var("XDG_DATA_HOME").ok().as_deref(),
            "XDG_DATA_HOME",
            std::env::var("HOME").ok().as_deref(),
            &["Library", "Application Support"],
            &[".local", "share"],
            &app_stem(),
            &|p: &Path| p.exists(),
        )
    }
    #[cfg(all(unix, not(target_os = "ios"), not(target_os = "macos")))]
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
/// - iOS: `$HOME/Library/Caches` — same sandbox-container reasoning as
///   [`data_dir`]'s iOS arm; XDG vars are ignored.
/// - macOS: `$HOME/Library/Caches`, with the same read-through legacy
///   fallback as [`data_dir`]'s macOS arm (legacy base: `$XDG_CACHE_HOME` if
///   set and absolute, else `$HOME/.cache`). XDG vars do not override this
///   base, only the fallback probe.
/// - Other Unix (excluding iOS and macOS): `$XDG_CACHE_HOME` if set and
///   absolute, else `$HOME/.cache`.
/// - Windows: `%LOCALAPPDATA%`.
/// - Any other target: `None`.
///
/// `None` when unresolvable. Logs at `debug` (via the `log` crate) when a
/// set-but-relative XDG var is ignored per the XDG Base Directory spec.
pub fn cache_dir() -> Option<PathBuf> {
    #[cfg(target_os = "ios")]
    {
        ios_cache_dir_from(std::env::var("HOME").ok().as_deref())
    }
    #[cfg(target_os = "macos")]
    {
        macos_dir_from(
            std::env::var("XDG_CACHE_HOME").ok().as_deref(),
            "XDG_CACHE_HOME",
            std::env::var("HOME").ok().as_deref(),
            &["Library", "Caches"],
            &[".cache"],
            &app_stem(),
            &|p: &Path| p.exists(),
        )
    }
    #[cfg(all(unix, not(target_os = "ios"), not(target_os = "macos")))]
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
///
/// # Concurrency contract
///
/// The temp path is keyed on **PID only**, not on the target `path` or a
/// per-call nonce — two concurrent calls from the *same process* writing to
/// the *same* `path` race on one shared temp file. Every current caller
/// already serializes same-process writes to one target path (shared-
/// preferences' `Mutex`, the desktop shell's cache writing from its single
/// render thread, Android's single writer), so this has never been
/// observed in practice; a caller writing to one `path` from multiple
/// threads without its own external serialization must add one (a
/// `Mutex`/single-writer-thread, per the existing call sites) before
/// calling this function concurrently.
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

/// Env-parameterized body of [`data_dir`]'s iOS branch: `home` joined with
/// `Library/Application Support`, `None` without a `HOME` — same
/// no-guessing contract as every other arm.
#[cfg(any(target_os = "ios", test))]
fn ios_data_dir_from(home: Option<&str>) -> Option<PathBuf> {
    home.map(|home| {
        let mut p = PathBuf::from(home);
        p.extend(["Library", "Application Support"]);
        p
    })
}

/// Env-parameterized body of [`cache_dir`]'s iOS branch: `home` joined
/// with `Library/Caches`.
#[cfg(any(target_os = "ios", test))]
fn ios_cache_dir_from(home: Option<&str>) -> Option<PathBuf> {
    home.map(|home| {
        let mut p = PathBuf::from(home);
        p.extend(["Library", "Caches"]);
        p
    })
}

/// Absolute-validated `xdg` (logged-and-ignored at `debug` if set but
/// relative), else `home` joined with `suffix` — the shared shape behind
/// both [`data_dir_from`]/[`cache_dir_from`] on non-iOS, non-macOS Unix
/// (iOS resolves inside its sandbox container instead, and macOS resolves
/// its own new base with this same shape only feeding a legacy-fallback
/// probe — see [`data_dir`] and [`legacy_xdg_or_home`]).
#[cfg(all(unix, not(target_os = "ios"), not(target_os = "macos")))]
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
#[cfg(all(unix, not(target_os = "ios"), not(target_os = "macos")))]
fn data_dir_from(xdg_data_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    xdg_or_home(xdg_data_home, "XDG_DATA_HOME", home, &[".local", "share"])
}

/// Env-parameterized body of [`cache_dir`]'s Unix branch.
#[cfg(all(unix, not(target_os = "ios"), not(target_os = "macos")))]
fn cache_dir_from(xdg_cache_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    xdg_or_home(xdg_cache_home, "XDG_CACHE_HOME", home, &[".cache"])
}

/// Absolute-validated `xdg` (logged-and-ignored at `debug` if set but
/// relative), else `home` joined with `suffix` — computes the **legacy**
/// base consulted only by [`macos_dir_from`]'s read-through-fallback probe.
/// This duplicates rather than reuses [`xdg_or_home`]'s shape because that
/// helper is compiled only for non-iOS, non-macOS Unix now that macOS has
/// its own dedicated arm (see [`data_dir`]).
#[cfg(any(target_os = "macos", test))]
fn legacy_xdg_or_home(
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

/// Env+probe-parameterized body of [`data_dir`]/[`cache_dir`]'s macOS arm.
///
/// Resolves `home` joined with `new_suffix` (`Library/Application Support`
/// or `Library/Caches`) as the base — `None` without `home`, same
/// no-guessing contract as every other arm — then applies the
/// **read-through legacy fallback**: if `<legacy base>/<app_stem>` exists
/// and `<new base>/<app_stem>` does not, returns the legacy base instead
/// (never migrates, copies, or deletes anything; logs one `debug` line
/// naming both paths when it fires). The legacy base is computed by
/// [`legacy_xdg_or_home`] from `xdg`/`home`/`legacy_suffix` — it is what
/// this crate resolved on macOS before growing this dedicated arm.
///
/// `exists` is an injectable filesystem-existence probe (rather than a
/// direct `Path::exists` call) so unit tests can drive every fallback
/// branch on a Linux host without a real `~/Library`; [`data_dir`]/
/// [`cache_dir`]'s macOS bodies wire in real `std::path::Path::exists`.
#[cfg(any(target_os = "macos", test))]
#[allow(clippy::too_many_arguments)]
fn macos_dir_from(
    xdg: Option<&str>,
    xdg_var_name: &str,
    home: Option<&str>,
    new_suffix: &[&str],
    legacy_suffix: &[&str],
    app_stem: &OsStr,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let home = home?;
    let mut new_base = PathBuf::from(home);
    new_base.extend(new_suffix);

    if let Some(legacy_base) = legacy_xdg_or_home(xdg, xdg_var_name, Some(home), legacy_suffix) {
        let new_app_dir = new_base.join(app_stem);
        let legacy_app_dir = legacy_base.join(app_stem);
        if !exists(&new_app_dir) && exists(&legacy_app_dir) {
            log::debug!(
                "frust-paths: {} not found, falling back to legacy {}",
                new_app_dir.display(),
                legacy_app_dir.display()
            );
            return Some(legacy_base);
        }
    }

    Some(new_base)
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

    // --- ios_data_dir_from / ios_cache_dir_from --------------------------

    #[test]
    fn ios_data_dir_is_application_support_inside_the_container() {
        assert_eq!(
            ios_data_dir_from(Some("/private/var/mobile/Containers/Data/Application/ABC")),
            Some(PathBuf::from(
                "/private/var/mobile/Containers/Data/Application/ABC/Library/Application Support"
            ))
        );
    }

    #[test]
    fn ios_cache_dir_is_library_caches_inside_the_container() {
        assert_eq!(
            ios_cache_dir_from(Some("/private/var/mobile/Containers/Data/Application/ABC")),
            Some(PathBuf::from(
                "/private/var/mobile/Containers/Data/Application/ABC/Library/Caches"
            ))
        );
    }

    #[test]
    fn ios_dirs_are_none_without_a_home() {
        assert_eq!(ios_data_dir_from(None), None);
        assert_eq!(ios_cache_dir_from(None), None);
    }

    // --- data_dir_from / cache_dir_from (Unix) ---------------------------

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn data_dir_from_absolute_xdg_wins() {
        assert_eq!(
            data_dir_from(Some("/custom/data"), Some("/home/user")),
            Some(PathBuf::from("/custom/data"))
        );
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn data_dir_from_relative_xdg_is_ignored_falls_back_to_home() {
        assert_eq!(
            data_dir_from(Some("relative/data"), Some("/home/user")),
            Some(PathBuf::from("/home/user/.local/share"))
        );
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn data_dir_from_no_xdg_falls_back_to_home() {
        assert_eq!(
            data_dir_from(None, Some("/home/user")),
            Some(PathBuf::from("/home/user/.local/share"))
        );
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn data_dir_from_unresolvable_is_none() {
        assert_eq!(data_dir_from(None, None), None);
        // A relative XDG var with no HOME to fall back to is also None.
        assert_eq!(data_dir_from(Some("relative"), None), None);
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn cache_dir_from_absolute_xdg_wins() {
        assert_eq!(
            cache_dir_from(Some("/custom/cache"), Some("/home/user")),
            Some(PathBuf::from("/custom/cache"))
        );
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn cache_dir_from_relative_xdg_is_ignored_falls_back_to_home() {
        assert_eq!(
            cache_dir_from(Some("relative/cache"), Some("/home/user")),
            Some(PathBuf::from("/home/user/.cache"))
        );
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn cache_dir_from_no_xdg_falls_back_to_home() {
        assert_eq!(
            cache_dir_from(None, Some("/home/user")),
            Some(PathBuf::from("/home/user/.cache"))
        );
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
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

    // --- macos_dir_from (new base + read-through legacy fallback) --------

    /// Test-only convenience so most cases don't need every `macos_dir_from`
    /// argument spelled out — `new_suffix`/`legacy_suffix` default to the
    /// data-dir shape (`Library/Application Support` / `.local/share`); the
    /// cache-dir shape is exercised explicitly by
    /// `macos_dir_from_cache_new_base_resolves_under_home`.
    fn macos_data_dir_from(
        xdg: Option<&str>,
        home: Option<&str>,
        app_stem: &str,
        exists: &dyn Fn(&Path) -> bool,
    ) -> Option<PathBuf> {
        macos_dir_from(
            xdg,
            "XDG_DATA_HOME",
            home,
            &["Library", "Application Support"],
            &[".local", "share"],
            OsStr::new(app_stem),
            exists,
        )
    }

    const NEVER: &dyn Fn(&Path) -> bool = &|_: &Path| false;

    #[test]
    fn macos_dir_from_new_base_no_home_is_none() {
        assert_eq!(macos_data_dir_from(None, None, "myapp", NEVER), None);
    }

    #[test]
    fn macos_dir_from_new_base_resolves_under_home() {
        assert_eq!(
            macos_data_dir_from(None, Some("/Users/alice"), "myapp", NEVER),
            Some(PathBuf::from("/Users/alice/Library/Application Support"))
        );
    }

    #[test]
    fn macos_dir_from_cache_new_base_resolves_under_home() {
        assert_eq!(
            macos_dir_from(
                None,
                "XDG_CACHE_HOME",
                Some("/Users/alice"),
                &["Library", "Caches"],
                &[".cache"],
                OsStr::new("myapp"),
                NEVER,
            ),
            Some(PathBuf::from("/Users/alice/Library/Caches"))
        );
    }

    #[test]
    fn macos_dir_from_fallback_fires_when_legacy_exists_and_new_absent() {
        let legacy_app = PathBuf::from("/Users/alice/.local/share/myapp");
        let exists = move |p: &Path| p == legacy_app;
        assert_eq!(
            macos_data_dir_from(None, Some("/Users/alice"), "myapp", &exists),
            Some(PathBuf::from("/Users/alice/.local/share")),
        );
    }

    #[test]
    fn macos_dir_from_fallback_does_not_fire_when_new_exists_regardless_of_legacy() {
        let new_app = PathBuf::from("/Users/alice/Library/Application Support/myapp");
        let legacy_app = PathBuf::from("/Users/alice/.local/share/myapp");
        let exists = move |p: &Path| p == new_app || p == legacy_app;
        assert_eq!(
            macos_data_dir_from(None, Some("/Users/alice"), "myapp", &exists),
            Some(PathBuf::from("/Users/alice/Library/Application Support")),
        );
    }

    #[test]
    fn macos_dir_from_fallback_does_not_fire_when_legacy_absent() {
        assert_eq!(
            macos_data_dir_from(None, Some("/Users/alice"), "myapp", NEVER),
            Some(PathBuf::from("/Users/alice/Library/Application Support")),
        );
    }

    #[test]
    fn macos_dir_from_fallback_does_not_fire_when_both_exist_new_wins() {
        let new_app = PathBuf::from("/Users/alice/Library/Application Support/myapp");
        let legacy_app = PathBuf::from("/Users/alice/.local/share/myapp");
        let exists = move |p: &Path| p == new_app || p == legacy_app;
        assert_eq!(
            macos_data_dir_from(None, Some("/Users/alice"), "myapp", &exists),
            Some(PathBuf::from("/Users/alice/Library/Application Support")),
        );
    }

    #[test]
    fn macos_dir_from_legacy_base_honors_absolute_xdg() {
        let legacy_app = PathBuf::from("/custom/data/myapp");
        let exists = move |p: &Path| p == legacy_app;
        assert_eq!(
            macos_data_dir_from(Some("/custom/data"), Some("/Users/alice"), "myapp", &exists,),
            Some(PathBuf::from("/custom/data")),
        );
    }

    #[test]
    fn macos_dir_from_legacy_base_ignores_relative_xdg() {
        // A relative XDG_DATA_HOME is ignored for the legacy probe too, so
        // the legacy base still falls back to `~/.local/share`.
        let legacy_app = PathBuf::from("/Users/alice/.local/share/myapp");
        let exists = move |p: &Path| p == legacy_app;
        assert_eq!(
            macos_data_dir_from(
                Some("relative/data"),
                Some("/Users/alice"),
                "myapp",
                &exists,
            ),
            Some(PathBuf::from("/Users/alice/.local/share")),
        );
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

    /// `create_dir_all(parent)` fails when a path component of the parent is
    /// already a plain file, so `atomic_write` returns via its first `?`
    /// before ever writing a temp file. This covers only that first
    /// short-circuit — it says nothing about the rename-failure cleanup
    /// branch (see `atomic_write_returns_error_and_cleans_up_temp_on_rename_failure`
    /// below for that).
    #[test]
    fn atomic_write_returns_error_when_parent_dir_cannot_be_created() {
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

    /// A failing rename with the temp file *already written* (target `path`
    /// is an existing non-empty directory, so `create_dir_all`/`fs::write`
    /// both succeed and only the final `fs::rename` fails —
    /// EISDIR/ENOTEMPTY-family on Unix/macOS) still returns the underlying
    /// error, cleans up the now-orphaned temp file, and leaves the
    /// directory's existing contents untouched. This is the rename-failure
    /// cleanup branch itself, not just the earlier `create_dir_all` `?`
    /// covered above.
    #[test]
    fn atomic_write_returns_error_and_cleans_up_temp_on_rename_failure() {
        let path = scratch_path("rename-fail");
        // `create_dir_all` on `path` itself creates every ancestor
        // (including `path`'s own parent) *and* `path` as a directory, so
        // the later `atomic_write`'s own `create_dir_all(parent)` call is a
        // no-op success, not the failure point.
        fs::create_dir_all(&path).unwrap();
        let occupant = path.join("occupant");
        fs::write(&occupant, b"do not touch").unwrap();

        let result = atomic_write(&path, b"data");
        assert!(
            result.is_err(),
            "renaming a file onto an existing non-empty directory must fail"
        );

        let temp_path = path.with_extension(format!("tmp.{}", std::process::id()));
        assert!(
            !temp_path.exists(),
            "temp file left behind after failed rename"
        );
        assert_eq!(
            fs::read(&occupant).unwrap(),
            b"do not touch",
            "occupant must be untouched by the failed write"
        );

        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }
}
