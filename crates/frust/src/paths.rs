//! Where an app keeps its files: the platform data and cache directories, and an atomic write.
//!
//! These are the CORE `frust-paths` leaf, re-exported so an app that depends on `frust` alone can
//! place caches, sidecars and export staging.
//!
//! - [`data_dir`] is the platform's data root and [`cache_dir`] its cache root, shared by every
//!   app on desktop: join your own subdirectory onto the returned base. Neither creates the
//!   directory ([`atomic_write`] creates the parents of the file it writes).
//! - Both return `None` when the platform cannot resolve one, never a guess that could write into
//!   the current directory, and never a relative path. That is: on iOS and macOS, `HOME` unset,
//!   empty or not absolute; on Linux and other Unix, the matching `XDG_DATA_HOME` /
//!   `XDG_CACHE_HOME` unset or relative *and* `HOME` unset, empty or not absolute; on Windows,
//!   `APPDATA` (for `data_dir`) or `LOCALAPPDATA` (for `cache_dir`) unset, empty or not absolute;
//!   on Android, before the shell installs the directories; on every other target, always.
//!
//! ```
//! let thumbs = frust::paths::cache_dir().map(|d| d.join("my-app").join("thumbs"));
//! # let _ = thumbs;
//! ```
//!
//! # Where each platform puts them
//!
//! | Platform | `data_dir()` | `cache_dir()` |
//! |----------|--------------|---------------|
//! | iOS | `$HOME/Library/Application Support` | `$HOME/Library/Caches` (both inside the app sandbox) |
//! | macOS | `$HOME/Library/Application Support` | `$HOME/Library/Caches` |
//! | Linux and other Unix | `$XDG_DATA_HOME`, else `~/.local/share` | `$XDG_CACHE_HOME`, else `~/.cache` |
//! | Windows | `%APPDATA%` | `%LOCALAPPDATA%` |
//! | Android | `Context.getFilesDir()` | `Context.getCacheDir()` |
//! | any other target (the web included) | `None` | `None`; `atomic_write` returns `Unsupported` on the web |
//!
//! On macOS the base can instead be the older XDG location (`$XDG_DATA_HOME` or `~/.local/share`,
//! and `$XDG_CACHE_HOME` or `~/.cache`): `frust-paths` returns it when a directory named after the
//! running executable's file stem exists there and not under `~/Library`, so installs that predate
//! the macOS location keep their files. An app that joins its own subdirectory name gets the
//! `~/Library` base unless such a directory exists. An `XDG_*` value that is not an absolute path
//! is ignored, as the XDG Base Directory specification requires.
//!
//! The Android shell installs both directories when the first surface is created
//! (`nativeInitPlatform`), so both functions return `None` before that. Code that runs earlier,
//! such as `Database::open` in `frust-database`, has the same rule.
//!
//! # What gets backed up
//!
//! Put regenerable data in [`cache_dir`] and user-visible, irreplaceable data in [`data_dir`].
//! The platforms treat the two differently, per their own documentation:
//!
//! - **iOS** (iOS Data Storage Guidelines, File System Programming Guide): `Library/Application
//!   Support` is included in iCloud and iTunes/Finder backups; `Library/Caches` is not, and the
//!   system may delete its contents when storage is low.
//! - **Android** (Auto Backup): with `allowBackup="true"`, which the `frust new` scaffold sets, the
//!   app's files directory is eligible for backup under the default rules, and the cache
//!   directory is excluded; the system may also clear the cache directory when storage is low.
//!   Auto Backup is size-limited, so a large file in the files directory may not be backed up.
//! - **macOS**: there is no per-app backup contract. `~/Library/Caches` is the conventional
//!   place for disposable data, and Time Machine's default exclusions skip it; do not rely on
//!   that for anything you cannot regenerate.
//! - **Linux** (XDG Base Directory specification): `$XDG_CACHE_HOME` holds user-specific
//!   non-essential (cached) data; `$XDG_DATA_HOME` holds user-specific data. What a backup tool
//!   includes is the user's choice.
//! - **Windows** (Known Folders): `%APPDATA%` is the Roaming folder, which follows the user between
//!   machines under a roaming profile; `%LOCALAPPDATA%` stays on the machine.
//!
//! Data that is neither backed up everywhere nor regenerable needs the app's own export path.
//!
//! # Atomic writes
//!
//! [`atomic_write`] writes to a temporary file next to the target and renames it into place, so a
//! reader never sees a partial file. Parent directories are created.
//!
//! - The temporary name is unique per call, so no caller-side locking is needed; concurrent
//!   writers of one path are last-writer-wins, never interleaved. On Windows a concurrent replace
//!   of the same path can fail with `PermissionDenied`.
//! - A process crash mid-write leaves the previous contents intact; a crash between creating
//!   and renaming the temp file can leave an orphaned `<name>.tmp.<pid>.<n>` sibling that
//!   nothing removes.
//! - It is NOT power-loss durable: no fsync is done, so after an OS crash or power loss the file
//!   may be empty or truncated on some filesystems.
//! - On the web (`wasm32-unknown-unknown`) there is no filesystem and it returns an
//!   `Unsupported` error.
//!
//! # Not re-exported
//!
//! `legacy_data_dir`, `legacy_cache_dir` and `app_stem` are migration-era internals, and
//! `install_android_dirs` / `android_dirs` belong to the platform shell. They stay in
//! `frust-paths`.

pub use frust_paths::{atomic_write, cache_dir, data_dir};
