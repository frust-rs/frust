//! Single source of truth for where a Frust app's build output lives, and
//! the directories [`clean`](crate::clean) removes.
//!
//! ## Layout
//!
//! Every generated/compiled artifact for a Frust app lands under one
//! project-relative root, [`BUILD_ROOT`] (`build/`), rather than scattered
//! across `android/app/build`, `android/build`, `dist/`, and a bare
//! `windows/icon.ico` next to the project sources. [`BuildLayout`] names the
//! target every producer either resolves its output through directly
//! (Android's `android_build`/`android_run`, desktop's `desktop_build`) or is
//! pinned to by an equality test in this module's own `#[cfg(test)]` module:
//! `web_build` (`manifest::DEFAULT_WEB_OUT_DIR`, read through
//! [`crate::manifest::WebSection::out_dir_or_default`]) and `ios_build`
//! (`ios_build::xcodebuild::ARCHIVE_PATH` and its siblings) deliberately keep
//! their own `build/web`/`build/ios` literals rather than calling into
//! [`BuildLayout`] — a future drift between a kept-literal and this module
//! trips a test here rather than silently diverging. Every accessor returns
//! a path relative to the project root — join it onto the project directory
//! at the call site.
//!
//! A scaffolded project's own `crates/frust-drive/templates/app/.gitignore` must stay
//! hand-synced with [`CLEAN_DIRS`]/[`LEGACY_CLEAN_DIRS`] — a rendered
//! template this crate has no build-time hook to check automatically.
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

    /// The per-module Gradle `buildDirectory` redirect for a plugin's Android
    /// library module (`:frust-<plugin>`, e.g. `frust-camera`) — the same
    /// `<app>/build/android/<module>` root [`Self::android_embedding`] uses
    /// for `:frust-embedding`, named generically so `plugin::apply`'s
    /// `settings_include_block` can derive the literal from here instead of
    /// duplicating it.
    pub fn android_module(module: &str) -> PathBuf {
        Self::android_root().join(module)
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

/// The project-root icon path a *pre-`build/`-layout* `windows/build.rs`
/// still embeds. Frust writes here only as a one-release mirror of
/// [`BuildLayout::windows_icon`] (`desktop_build::windows::mirror_legacy_icon`),
/// always together with [`LEGACY_WINDOWS_ICON_MARKER`]; the migration recipe
/// moves the project off it.
pub const LEGACY_WINDOWS_ICON: &str = "windows/icon.ico";

/// Sidecar `desktop_build::windows` writes next to a *mirrored*
/// [`LEGACY_WINDOWS_ICON`], recording that Frust generated that icon. It is
/// the single source of truth `clean::run` consults before removing the
/// root-level icon: marker present → both files are Frust output and are
/// removed; marker absent → the icon is user-owned and left alone.
pub const LEGACY_WINDOWS_ICON_MARKER: &str = "windows/icon.ico.frust-mirrored";

/// Build-output paths from before the [`BUILD_ROOT`] migration.
///
/// Removed for a pre-migration app whose generated Gradle/Xcode config still
/// writes to these paths; kept for at least one release so such an app keeps
/// getting cleaned even before it regenerates that config onto the new
/// layout.
///
/// [`LEGACY_WINDOWS_ICON`] (`windows/icon.ico`) is deliberately **not**
/// listed here: a file at that root-level path is only Frust output when
/// `desktop_build::windows` *mirrored* it there for a pre-`build/`-layout
/// `windows/build.rs` — and then it sits next to the
/// [`LEGACY_WINDOWS_ICON_MARKER`] sidecar. `clean` removes the icon only when
/// that marker is present (see `clean::run`); without the marker the file is
/// the project owner's hand-placed input and is never touched.
pub const LEGACY_CLEAN_DIRS: &[&str] = &[
    "android/app/build",
    "android/build",
    "android/.gradle",
    "android/app/src/main/jniLibs",
    "dist",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ios_build::xcodebuild::ARCHIVE_PATH;
    use crate::manifest::WebSection;

    #[test]
    fn build_layout_accessors_resolve_the_expected_relative_paths() {
        assert_eq!(BuildLayout::rust_dir(), Path::new("build/rust"));
        assert_eq!(BuildLayout::android_root(), Path::new("build/android"));
        assert_eq!(BuildLayout::android_app(), Path::new("build/android/app"));
        assert_eq!(
            BuildLayout::android_embedding(),
            Path::new("build/android/frust-embedding")
        );
        assert_eq!(
            BuildLayout::android_jni_libs(),
            Path::new("build/android/jniLibs")
        );
        assert_eq!(
            BuildLayout::android_module("frust-camera"),
            Path::new("build/android/frust-camera")
        );
        assert_eq!(
            BuildLayout::android_gradle_cache(),
            Path::new("build/android/.gradle")
        );
        assert_eq!(BuildLayout::ios(), Path::new("build/ios"));
        assert_eq!(BuildLayout::web(), Path::new("build/web"));
        assert_eq!(
            BuildLayout::desktop(DesktopBundleTarget::Macos),
            Path::new("build/desktop/macos")
        );
        assert_eq!(
            BuildLayout::desktop(DesktopBundleTarget::Windows),
            Path::new("build/desktop/windows")
        );
        assert_eq!(
            BuildLayout::desktop(DesktopBundleTarget::Linux),
            Path::new("build/desktop/linux")
        );
        assert_eq!(
            BuildLayout::windows_icon(),
            Path::new("build/desktop/windows/icon.ico")
        );
    }

    /// Pin: `BuildLayout::web()` must keep naming the same directory
    /// `web_build`/`manifest::WebSection`'s own kept-literal default
    /// resolves to (`manifest::DEFAULT_WEB_OUT_DIR`, private to that module —
    /// read back through the public [`WebSection::out_dir_or_default`]
    /// accessor instead of duplicating the literal here).
    #[test]
    fn web_dir_matches_the_manifest_default_web_out_dir() {
        assert_eq!(
            BuildLayout::web(),
            Path::new(WebSection::default().out_dir_or_default())
        );
    }

    /// Pin: `BuildLayout::ios()` must stay the directory
    /// `ios_build::xcodebuild::ARCHIVE_PATH` (and its `export`-module
    /// siblings, not reachable from here) actually write under.
    #[test]
    fn ios_dir_is_the_shared_prefix_of_the_ios_build_path_constants() {
        // `ARCHIVE_PATH` is a kept forward-slash literal (see the module
        // doc); `BuildLayout::ios()`'s own `PathBuf` renders with the host's
        // native separator (backslash on Windows), so the comparison must go
        // through the same portable rendering `crate::host_path` uses for
        // every path this crate ever embeds in generated text, rather than a
        // raw `.to_str()`.
        let ios_dir = crate::host_path::to_portable_string(&BuildLayout::ios());
        assert!(
            ARCHIVE_PATH.starts_with(&format!("{ios_dir}/")),
            "`ios_build::xcodebuild::ARCHIVE_PATH` (`{ARCHIVE_PATH}`) must live \
             under `BuildLayout::ios()` (`{ios_dir}`)"
        );
    }

    #[test]
    fn legacy_clean_dirs_no_longer_lists_the_root_windows_icon() {
        assert!(!LEGACY_CLEAN_DIRS.contains(&LEGACY_WINDOWS_ICON));
        assert!(!LEGACY_CLEAN_DIRS.contains(&LEGACY_WINDOWS_ICON_MARKER));
        // The marker sits next to the icon it describes.
        assert_eq!(
            Path::new(LEGACY_WINDOWS_ICON_MARKER).parent(),
            Path::new(LEGACY_WINDOWS_ICON).parent()
        );
    }
}
