//! Single source of truth for where a Frust app's build output lives, and
//! the directories [`clean`](crate::clean) removes.
//!
//! ## Layout
//!
//! Every generated/compiled artifact for a Frust app lands under one
//! project-relative root, [`BUILD_ROOT`] (`build/`), rather than scattered
//! across `android/app/build`, `android/build`, `dist/`, and a bare
//! `windows/icon.ico` next to the project sources. [`BuildLayout`] names the
//! per-platform subdirectory under that root; every accessor returns a path
//! relative to the project root — join it onto the project directory at the
//! call site.
//!
//! `target/` (cargo's own compiled-artifact cache) is deliberately **not**
//! part of this layout and is never removed by path anywhere in this crate —
//! see the `CARGO_TARGET_DIR` precedence below and [`CLEAN_DIRS`].
//!
//! ## `CARGO_TARGET_DIR` precedence
//!
//! [`BuildLayout::rust_dir`] names only the *default* location cargo's own
//! artifacts land under, `build/rust`. The directory cargo actually writes
//! to for a given invocation is a resolved value, in this order (highest
//! priority first):
//!
//! 1. the `CARGO_TARGET_DIR` environment variable, when set;
//! 2. `cargo metadata`'s reported `target_directory` (which itself honors a
//!    workspace's `build.target-dir` key in any `.cargo/config.toml` cargo
//!    discovers);
//! 3. this module's own default, [`BuildLayout::rust_dir`].
//!
//! Resolving that precedence for a real invocation is a caller's job (the
//! pipelines that shell out to `cargo build`/`cargo run`), not this module's
//! — this module only names the default `cargo clean` (run with no
//! `CARGO_TARGET_DIR` override) would use, and which `cargo clean` itself —
//! not this module — is responsible for actually clearing.

use std::path::{Path, PathBuf};

use crate::desktop_build::DesktopBundleTarget;

/// The project-relative root every Frust build artifact lands under.
pub const BUILD_ROOT: &str = "build";

/// Project-relative accessors for each platform's build-output subdirectory
/// under [`BUILD_ROOT`]. A namespace, not a value — every accessor is an
/// associated function (`BuildLayout::rust_dir()`, not an instance method).
pub struct BuildLayout;

impl BuildLayout {
    /// The default `CARGO_TARGET_DIR` — cargo's own compiled-artifact cache.
    /// See the module doc's `CARGO_TARGET_DIR` precedence: a real build must
    /// resolve the environment/`cargo metadata` overrides before falling
    /// back to this default.
    pub fn rust_dir() -> PathBuf {
        Path::new(BUILD_ROOT).join("rust")
    }

    /// The Android build root Gradle writes into.
    pub fn android_root() -> PathBuf {
        Path::new(BUILD_ROOT).join("android")
    }

    /// The generated app module's Gradle build output.
    pub fn android_app() -> PathBuf {
        Self::android_root().join("app")
    }

    /// The `:frust-embedding` module's redirected Gradle output, keeping the
    /// shared frust checkout (or, for an out-of-tree app, its `frust`
    /// dependency checkout) pristine.
    pub fn android_embedding() -> PathBuf {
        Self::android_root().join("frust-embedding")
    }

    /// Native libraries `cargo ndk` stages for Gradle to package.
    pub fn android_jni_libs() -> PathBuf {
        Self::android_root().join("jniLibs")
    }

    /// The project-local Gradle cache.
    pub fn android_gradle_cache() -> PathBuf {
        Self::android_root().join(".gradle")
    }

    /// `xcodebuild`'s derived-data/archive output.
    pub fn ios() -> PathBuf {
        Path::new(BUILD_ROOT).join("ios")
    }

    /// The default browser build artifact directory. `[web] out-dir` in the
    /// project's manifest still wins when set — this is only the fallback a
    /// project without one resolves to.
    pub fn web() -> PathBuf {
        Path::new(BUILD_ROOT).join("web")
    }

    /// The per-OS desktop bundle root: `build/desktop/<macos|windows|linux>`.
    pub fn desktop(target: DesktopBundleTarget) -> PathBuf {
        Path::new(BUILD_ROOT).join("desktop").join(target.as_str())
    }

    /// The generated Windows `.ico` the packaging tools (`cargo-packager`,
    /// `winresource`) read.
    pub fn windows_icon() -> PathBuf {
        Self::desktop(DesktopBundleTarget::Windows).join("icon.ico")
    }
}

/// Directories `clean` removes beyond `cargo clean`'s own `target/` — which
/// is never removed by path here, since `cargo clean` owns it wherever
/// `CARGO_TARGET_DIR`/`build.target-dir` actually put it (see the module
/// doc's `CARGO_TARGET_DIR` precedence).
pub const CLEAN_DIRS: &[&str] = &[BUILD_ROOT];

/// Build-output paths from before the [`BUILD_ROOT`] migration.
///
/// Removed for a pre-migration app whose generated Gradle/Xcode config still
/// writes to these paths; kept for at least one release so such an app keeps
/// getting cleaned even before it regenerates that config onto the new
/// layout. Entries are a mix of directories and one file
/// (`windows/icon.ico`) — `clean` removes each with the operation its kind
/// needs.
pub const LEGACY_CLEAN_DIRS: &[&str] = &[
    "android/app/build",
    "android/build",
    "android/.gradle",
    "android/app/src/main/jniLibs",
    "dist",
    "windows/icon.ico",
];
