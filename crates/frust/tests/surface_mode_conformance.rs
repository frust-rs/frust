//! Source-scan conformance test for review M3's fix (review-fix-2 t01):
//! ONLY the two shell FFI-glue callers may declare host translucency, and
//! the `frust` facade must never re-export the declaration fn.
//!
//! Precedent: `frust-drive/tests/print_free_cores.rs` (a plain `std::fs`
//! source scan run as an ordinary `cargo test` — this repo has no
//! lint-plugin tooling, see `docs/CODE_STANDARDS.md`). Chosen over a doc
//! statement + unit test alone because the whole point of review M3's fix is
//! a workspace-wide invariant ("nothing outside these two files may set the
//! latch") that a single crate's unit test can't see across crate
//! boundaries — a cheap scan here catches a future caller added anywhere in
//! the workspace, not just a regression local to `frust-shell-common`.
//!
//! # What this checks
//!
//! 1. No `crates/*/src/**/*.rs` file calls `declare_host_translucent_surface(`
//!    as a real (non-comment) call, except the two sanctioned host-glue
//!    files (`frust-shell-android/src/jni_glue.rs`,
//!    `frust-shell-ios/src/ffi_glue.rs`) and `frust-shell-common`'s own
//!    defining/test module (`surface_mode.rs`).
//! 2. `crates/frust/src/lib.rs` (the facade) contains no `pub use` line
//!    naming `declare_host_translucent_surface` or the old
//!    `request_translucent_surface` — i.e. the facade doesn't re-export the
//!    latch setter.
//!
//! # What this is NOT
//!
//! A substring/line scan, not a parser — comment-only lines are stripped
//! (mirroring `print_free_cores.rs`), but this is not a full Rust tokenizer;
//! correct for this codebase because every real call in scope sits on its
//! own statement line.

use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root, resolved from this crate's manifest dir
/// (`crates/frust`) so the scan is working-directory-independent.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/frust has a grandparent (the workspace root)")
        .to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("dir entry under {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Every `crates/*/src` tree's `.rs` files, sorted for a stable failure order.
fn all_crate_source_files() -> Vec<PathBuf> {
    let root = workspace_root();
    let crates_dir = root.join("crates");
    let mut out = Vec::new();
    let entries = fs::read_dir(&crates_dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", crates_dir.display()));
    for entry in entries {
        let crate_dir = entry.unwrap_or_else(|e| panic!("dir entry: {e}")).path();
        let src = crate_dir.join("src");
        if src.is_dir() {
            rust_files(&src, &mut out);
        }
    }
    out.sort();
    out
}

/// `path` relative to the workspace root, forward-slashed, for stable
/// failure messages and allowlist keys independent of host path separators.
fn rel(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Files allowed to call `declare_host_translucent_surface(` — the two
/// sanctioned FFI-glue callers, plus `surface_mode.rs` itself (the
/// definition and its own unit tests).
const ALLOWLIST: &[&str] = &[
    "crates/frust-shell-common/src/surface_mode.rs",
    "crates/frust-shell-android/src/jni_glue.rs",
    "crates/frust-shell-ios/src/ffi_glue.rs",
];

/// True if `line`, trimmed, is a comment-only line (mirrors
/// `print_free_cores.rs`'s comment-stripping — doc comments naming the fn in
/// prose, e.g. `[\`declare_host_translucent_surface\`]`, must not count as a
/// call site).
fn is_comment_only(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

#[test]
fn only_shell_glue_may_declare_host_translucent_surface() {
    let mut failures = Vec::new();

    for path in all_crate_source_files() {
        let relp = rel(&path);
        if ALLOWLIST.contains(&relp.as_str()) {
            continue;
        }
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for (i, line) in contents.lines().enumerate() {
            if is_comment_only(line) {
                continue;
            }
            if line.contains("declare_host_translucent_surface(") {
                failures.push(format!(
                    "{relp}:{}: calls `declare_host_translucent_surface()` outside the \
                     sanctioned shell-glue allowlist — only the generated host's JNI/C-ABI \
                     entry points may declare host translucency (review M3). Line: {}",
                    i + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "host-declared-translucency ban violated ({} hit(s)) — see \
         crates/frust-shell-common/src/surface_mode.rs's module docs:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

#[test]
fn facade_does_not_reexport_the_translucency_declaration() {
    let facade = workspace_root().join("crates/frust/src/lib.rs");
    let contents =
        fs::read_to_string(&facade).unwrap_or_else(|e| panic!("reading {}: {e}", facade.display()));

    for (i, line) in contents.lines().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("pub use") {
            continue;
        }
        assert!(
            !trimmed.contains("declare_host_translucent_surface")
                && !trimmed.contains("request_translucent_surface"),
            "crates/frust/src/lib.rs:{}: facade re-exports the translucency-declaration fn — \
             app Rust must never be able to set this latch (review M3). Line: {}",
            i + 1,
            line.trim()
        );
    }
}
