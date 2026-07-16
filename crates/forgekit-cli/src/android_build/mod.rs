//! Android release build pipeline (spec §12.5) — **frozen stub**. This
//! module's [`build`] signature and the [`AndroidArtifact`]/[`BuiltArtifacts`]
//! types are load-bearing for `commands::build`; task 64 implements the
//! Gradle-driving body (`local.properties` write, `assemble<Flavor><Mode>`/
//! `bundle<Flavor><Mode>` task selection, artifact path resolution) without
//! changing this signature.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::build_info::BuildInfo;
use crate::process::ProcessRunner;

/// The artifact `forgekit build apk`/`appbundle` requests (spec §12.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AndroidArtifact {
    /// A `--split-per-abi` (one APK per ABI) or fat APK build, for the
    /// already-mapped Gradle ABI names (`arm64-v8a`, `armeabi-v7a`,
    /// `x86_64`) requested via `--target-platform`.
    Apk {
        split_per_abi: bool,
        abis: Vec<String>,
    },
    /// An Android App Bundle (`.aab`) for Play Store distribution.
    Appbundle,
}

/// Paths to the artifact(s) a successful build produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuiltArtifacts {
    pub paths: Vec<PathBuf>,
}

/// Drives the Android release pipeline (Gradle `assemble<Flavor><Mode>` /
/// `bundle<Flavor><Mode>`, `cargo ndk` under the hood) for `target` in the
/// ForgeKit project rooted at `project_dir`.
///
/// **Frozen signature** — task 64 implements the body; do not change this
/// signature without updating both `commands::build` and this doc comment.
pub fn build(
    _runner: &dyn ProcessRunner,
    _project_dir: &Path,
    _info: &BuildInfo,
    _target: &AndroidArtifact,
) -> Result<BuiltArtifacts> {
    bail!("`forgekit build` for Android is implemented in task 64")
}
