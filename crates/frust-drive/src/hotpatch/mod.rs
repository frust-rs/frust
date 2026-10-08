//! The desktop hot-patch builder: capture a fat build's rustc and linker
//! invocations, replay the changed workspace crates, link a thin patch
//! against the running image, and decide whether the patch may be applied.
//!
//! The pipeline is a port of dioxus-cli 0.7.10's builder with its rustc-arg
//! keying fixed (records are keyed per target kind over *every*
//! `--crate-type`, so a lib declared `["cdylib", "staticlib", "rlib"]` is
//! found as a lib). Like the rest of `frust-drive`, it depends on no
//! framework crate and shells out only through
//! [`ProcessRunner`](crate::process::ProcessRunner). Every parse surprise in
//! a rustc or linker argument format fails closed as
//! [`HotpatchError::BuilderUnsupported`] — a guessed patch is never built.
//! [`android`] runs the same builder for an Android arm64 app: the fat build
//! through cargo-ndk outside Gradle, the session through `adb forward`.
//! See `docs/CLI_ARCHITECTURE.md`.

use std::path::{Path, PathBuf};

pub mod android;
pub mod capture;
pub mod fat_link;
pub mod graph;
pub mod jump_table;
pub mod layout;
pub mod link_intercept;
pub mod replay;
pub mod seams;
pub mod session;
pub mod stub;
pub mod symbols;
pub mod thin_link;

/// The directory under a cargo target dir that holds every hot-patch
/// artifact: captured invocations, fat archives, patches.
pub const HOTPATCH_DIR: &str = "frust-hotpatch";

/// `<target_dir>/frust-hotpatch`.
pub fn hotpatch_root(target_dir: &Path) -> PathBuf {
    target_dir.join(HOTPATCH_DIR)
}

/// A hot-patch builder failure.
#[derive(Debug, thiserror::Error)]
pub enum HotpatchError {
    /// A rustc or linker argument, capture record or toolchain output had a
    /// shape the builder does not understand. The arg formats it reads are
    /// stable in practice, not by contract, so a surprise refuses the patch
    /// (and the caller falls back to a restart) instead of guessing.
    #[error("hot-patch builder unsupported: {detail}")]
    BuilderUnsupported { detail: String },
    /// A filesystem operation the builder needed failed.
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    /// An external tool could not be spawned at all (a non-zero exit is not
    /// this error; it is reported as the tool's own failure).
    #[error("{detail}")]
    Process { detail: String },
}

impl HotpatchError {
    /// [`HotpatchError::BuilderUnsupported`] with `detail`.
    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self::BuilderUnsupported {
            detail: detail.into(),
        }
    }

    /// [`HotpatchError::Io`] naming what was being attempted.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}
