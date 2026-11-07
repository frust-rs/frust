//! Where an app keeps its files: the platform data and cache directories, and an atomic write.
//!
//! These are the CORE `frust-paths` leaf, re-exported so an app that depends on `frust` alone can
//! place caches, sidecars and export staging.
//!
//! - [`data_dir`] is the platform's data root and [`cache_dir`] its cache root, shared by every
//!   app on desktop: join your own subdirectory onto the returned base. Neither creates the
//!   directory ([`atomic_write`] creates the parents of the file it writes).
//! - Both return `None` when the platform cannot resolve one (`HOME` or `APPDATA` unset), never a
//!   guess that could write into the current directory.
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
//! | any other target | `None` | `None` |
//!
//! On macOS, `XDG_*` variables do not move the base; they only feed a read-through fallback for
//! installs that predate the macOS-specific location. An `XDG_*` value that is not an absolute
//! path is ignored, as the XDG Base Directory specification requires.
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
//! reader never sees a partial file and a crash mid-write leaves the previous contents intact.
//! Parent directories are created. The temporary name is keyed on the process id only, so two
//! threads of one process writing the same path share a temp file: serialize them with a `Mutex`
//! or a single writer thread.
//!
//! # Not re-exported
//!
//! `legacy_data_dir`, `legacy_cache_dir` and `app_stem` are migration-era internals, and
//! `install_android_dirs` / `android_dirs` belong to the platform shell. They stay in
//! `frust-paths`.

pub use frust_paths::{atomic_write, cache_dir, data_dir};
