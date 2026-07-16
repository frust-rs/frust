//! iOS release build pipeline (spec §12.6) — **frozen stub**. This module's
//! [`build`] signature and the [`IosArtifact`]/[`BuiltArtifacts`] types are
//! load-bearing for `commands::build`; task 65 implements the
//! `xcodebuild`-driving body (scheme/configuration resolution, signing vs.
//! `--no-codesign`, `-exportArchive` for `Ipa`) without changing this
//! signature.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::build_info::BuildInfo;
use crate::process::ProcessRunner;

/// The artifact `forgekit build ios`/`ipa` requests (spec §12.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IosArtifact {
    /// A `.app` build for a device or the Simulator.
    App { simulator: bool, codesign: bool },
    /// An archived + exported `.ipa` for the given export method
    /// (`app-store-connect`/`release-testing`/`debugging`/`enterprise`).
    Ipa { export_method: String },
}

/// Paths to the artifact(s) a successful build produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuiltArtifacts {
    pub paths: Vec<PathBuf>,
}

/// Drives the iOS release pipeline (`xcodebuild build`/`archive` +
/// `-exportArchive`) for `target` in the ForgeKit project rooted at
/// `project_dir`.
///
/// **Frozen signature** — task 65 implements the body; do not change this
/// signature without updating both `commands::build` and this doc comment.
pub fn build(
    _runner: &dyn ProcessRunner,
    _project_dir: &Path,
    _info: &BuildInfo,
    _target: &IosArtifact,
) -> Result<BuiltArtifacts> {
    bail!("`forgekit build` for iOS is implemented in task 65")
}
