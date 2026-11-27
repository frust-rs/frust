//! `frust create --frust-path <relative>`: the relative path is the invoking
//! directory's, so the generated manifest's `frust` and plugin path
//! dependencies must still name this checkout from the new project, with
//! no unfolded `<dir>/..` pair left in them.
//!
//! Drives the compiled `frust` binary (`CARGO_BIN_EXE_frust`) with
//! `--no-sync`: scaffolding alone, pure filesystem work.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

/// A fresh destination directory under the system temp dir.
fn unique_dest() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("frust-cli-frust-path-{}-{n}", std::process::id()))
}

/// The `path` of dependency `name` in the project's `Cargo.toml`.
fn dep_path(manifest: &toml::Value, name: &str) -> String {
    manifest["dependencies"][name]["path"]
        .as_str()
        .unwrap_or_else(|| panic!("`{name}` has no path dependency:\n{manifest}"))
        .to_string()
}

#[test]
fn a_relative_frust_path_resolves_against_the_invoking_directory() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("the crates directory exists");
    let dest = unique_dest();
    let _ = std::fs::remove_dir_all(&dest);

    // Invoked from `crates/`, naming the facade relative to it.
    let output = Command::new(env!("CARGO_BIN_EXE_frust"))
        .current_dir(&crates)
        .args([
            "create",
            dest.to_str().expect("dest path is valid UTF-8"),
            "--project-name",
            "frust_path_app",
            "--frust-path",
            "frust",
            "--no-sync",
        ])
        .output()
        .expect("failed to spawn `frust create`");
    assert!(
        output.status.success(),
        "`frust create` failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let text = std::fs::read_to_string(dest.join("Cargo.toml")).expect("reading Cargo.toml");
    let manifest: toml::Value = toml::from_str(&text).expect("Cargo.toml parses");
    let resolve = |path: String| {
        let path = Path::new(&path);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            dest.join(path)
        };
        path.canonicalize()
            .unwrap_or_else(|err| panic!("{} does not resolve: {err}\n{text}", path.display()))
    };

    let frust = dep_path(&manifest, "frust");
    assert_eq!(resolve(frust), crates.join("frust"), "{text}");
    let material = dep_path(&manifest, "frust-material");
    assert!(
        !material.split('/').any(|segment| segment == ".."),
        "unfolded plugin path `{material}`"
    );
    assert_eq!(
        resolve(material),
        crates.join("../plugins/material").canonicalize().unwrap(),
        "{text}"
    );

    let _ = std::fs::remove_dir_all(&dest);
}
