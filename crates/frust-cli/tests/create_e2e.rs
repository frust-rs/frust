//! End-to-end test: `frust create`'s output actually builds against the
//! real `frust` facade crate (spec Phase 1 exit criterion, task 09).
//!
//! `frust-cli` ships as a binary-only crate (no `lib` target), so this
//! integration test drives the compiled `frust` binary via
//! `CARGO_BIN_EXE_frust` (Cargo builds and points at it automatically
//! for integration tests) rather than calling `scaffold::generate`
//! in-process — that keeps the test entirely within this crate's declared
//! module boundaries (no `src/lib.rs` split) while still exercising the
//! exact code path a real user hits. The `frust create` step itself does
//! no GUI/network work (pure filesystem), so this is sandbox-friendly
//! either way.
//!
//! Ignored by default: the inner `cargo build` compiles the generated
//! project's full dependency graph (winit/vello/wgpu/parley via the
//! `frust` facade) from a cold target dir, which takes several minutes.
//! Run explicitly:
//! `cargo test -p frust-cli --test create_e2e -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

/// Absolute path to this workspace's `crates/frust` (the facade crate
/// the generated project path-depends on via `--frust-path`).
fn workspace_frust_path() -> PathBuf {
    let raw = Path::new(env!("CARGO_MANIFEST_DIR")).join("../frust");
    raw.canonicalize()
        .expect("crates/frust must exist in this workspace checkout")
}

/// A fresh, never-before-used destination directory under the system temp
/// dir (pid + atomic counter avoids collisions across parallel test runs).
fn unique_dest() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("frust-cli-e2e-{}-{n}", std::process::id()))
}

#[test]
#[ignore = "compiles the generated project's full dependency graph (winit/vello/wgpu); run explicitly with `--ignored`"]
fn scaffolded_project_builds_against_the_real_facade() {
    let dest = unique_dest();
    let _ = std::fs::remove_dir_all(&dest);

    let frust_exe = env!("CARGO_BIN_EXE_frust");
    let frust_path = workspace_frust_path();

    let create_status = Command::new(frust_exe)
        .args([
            "create",
            dest.to_str().expect("dest path is valid UTF-8"),
            "--project-name",
            "fk_e2e_app",
            "--frust-path",
            frust_path.to_str().expect("frust_path is valid UTF-8"),
        ])
        .status()
        .expect("failed to spawn `frust create`");
    assert!(create_status.success(), "`frust create` exited non-zero");

    // Sanity: the manifest-listed files landed with substitutions applied.
    assert!(dest.join("Cargo.toml").exists());
    assert!(dest.join("src/lib.rs").exists());
    assert!(dest.join("src/main.rs").exists());
    let cargo_toml = std::fs::read_to_string(dest.join("Cargo.toml")).unwrap();
    assert!(cargo_toml.contains("fk_e2e_app"), "{cargo_toml}");

    let build_status = Command::new("cargo")
        .arg("build")
        .current_dir(&dest)
        .status()
        .expect("failed to spawn `cargo build` in the generated project");
    assert!(
        build_status.success(),
        "generated project at {} failed to `cargo build`",
        dest.display()
    );

    let _ = std::fs::remove_dir_all(&dest);
}
