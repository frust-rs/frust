//! Test-only temp-directory helper shared across `ide_config`'s per-generator
//! test modules.
//!
//! No `tempfile`/tempdir crate is pinned in this workspace (version pins are
//! LAW — `docs/DEVELOPMENT.md`'s Version-Pin Policy — and none of this
//! crate's existing pins cover one), so this mirrors
//! `frust-drive::scaffold`'s own `unique_temp_dir` shape: a uniquely-named,
//! pre-created directory under the OS temp dir, keyed by a counter plus the
//! process id so parallel `cargo test` runs never collide.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// A fresh, existing, uniquely-named directory under the OS temp dir.
///
/// `tag` names the caller for a readable directory name. Any stale directory
/// left over from an earlier interrupted run is removed first; the directory
/// itself is never cleaned up on success — relies on OS temp-dir reclamation,
/// same as `frust-drive::scaffold`'s tests.
pub(crate) fn unique_temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("frust-dap-test-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("creating {}: {e}", dir.display()));
    dir
}
