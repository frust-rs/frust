//! `frust create --frust-path <path>`: a relative path is the invoking
//! directory's, and is written into the generated manifest relative to the
//! new project, so both the `frust` and the plugin path dependencies name
//! the checkout from there (with no unfolded `<dir>/..` pair) and keep
//! naming it after the tree moves; an absolute path is written absolute.
//!
//! Drives the compiled `frust` binary (`CARGO_BIN_EXE_frust`) with
//! `--no-sync` against a minimal stand-in checkout (a `frust-ui` manifest
//! and a `plugins/material` directory): scaffolding alone, pure filesystem
//! work.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use frust_drive::host_path::canonicalize_simplified;

/// A fresh, canonical scratch root holding a stand-in checkout at
/// `frust/` (facade crate `frust/crates/frust`, `frust/plugins/material`)
/// and an empty `invoker/` directory. Canonical, so a path computed
/// lexically against it resolves the same way through the filesystem
/// (`/tmp` and `/var` are symlinks on macOS).
fn scratch_root() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("frust-cli-frust-path-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let facade = root.join("frust").join("crates").join("frust");
    std::fs::create_dir_all(&facade).expect("creating the stand-in facade crate");
    std::fs::write(
        facade.join("Cargo.toml"),
        "[package]\nname = \"frust-ui\"\nversion = \"0.0.0\"\n",
    )
    .expect("writing the stand-in facade manifest");
    std::fs::create_dir_all(root.join("frust").join("plugins").join("material"))
        .expect("creating the stand-in material plugin");
    std::fs::create_dir_all(root.join("invoker")).expect("creating the invoking directory");
    canonicalize_simplified(&root).expect("the scratch root exists")
}

/// Runs `frust create <dest> --frust-path <frust_path> --no-sync` from
/// `cwd`, asserting it succeeds.
fn create(cwd: &Path, dest: &str, frust_path: &str) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_frust"))
        .current_dir(cwd)
        .args([
            "create",
            dest,
            "--project-name",
            "frust_path_app",
            "--frust-path",
            frust_path,
            "--no-sync",
        ])
        .output()
        .expect("failed to spawn `frust create`");
    assert!(
        output.status.success(),
        "`frust create` failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// The `frust` and `frust-material` dependency paths of the project's
/// `Cargo.toml`, with the manifest text for failure messages.
fn manifest_paths(project: &Path) -> (String, String, String) {
    let text = std::fs::read_to_string(project.join("Cargo.toml")).expect("reading Cargo.toml");
    let manifest: toml::Value = toml::from_str(&text).expect("Cargo.toml parses");
    let dep_path = |name: &str| {
        manifest["dependencies"][name]["path"]
            .as_str()
            .unwrap_or_else(|| panic!("`{name}` has no path dependency:\n{text}"))
            .to_string()
    };
    (dep_path("frust"), dep_path("frust-material"), text)
}

/// `path` as cargo resolves a manifest's `path` dependency: joined onto
/// the project directory when relative.
fn resolve(project: &Path, path: &str) -> PathBuf {
    canonicalize_simplified(&project.join(path))
        .unwrap_or_else(|err| panic!("`{path}` does not resolve from the project: {err}"))
}

/// Asserts both dependency paths are relative, folded, and name the
/// stand-in checkout from `project`; returns them.
fn assert_relative_to_project(root: &Path, project: &Path) -> (String, String) {
    let (frust, material, text) = manifest_paths(project);
    for path in [&frust, &material] {
        assert!(
            Path::new(path).is_relative(),
            "`{path}` is absolute:\n{text}"
        );
    }
    assert_eq!(
        resolve(project, &frust),
        root.join("frust/crates/frust"),
        "{text}"
    );
    assert!(
        !material
            .split('/')
            .skip_while(|segment| *segment == "..")
            .any(|segment| segment == ".."),
        "unfolded plugin path `{material}`"
    );
    assert_eq!(
        resolve(project, &material),
        root.join("frust/plugins/material"),
        "{text}"
    );
    (frust, material)
}

/// (a) Invoked from one directory, creating the project in another: the
/// relative path is re-expressed from the project directory.
#[test]
fn a_relative_frust_path_is_written_relative_to_a_project_elsewhere() {
    let root = scratch_root();
    let invoker = root.join("invoker");

    let output = create(&invoker, "../apps/app", "../frust");
    let project = root.join("apps/app");
    let (frust, material) = assert_relative_to_project(&root, &project);
    assert_eq!(frust, "../../frust/crates/frust");
    assert_eq!(material, "../../frust/plugins/material");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("`../../frust/crates/frust`"),
        "the note names the written path:\n{stdout}"
    );

    // An absolute destination gives the same relative path.
    let absolute_dest = root.join("apps/other");
    create(
        &invoker,
        absolute_dest.to_str().expect("dest path is valid UTF-8"),
        "../frust",
    );
    let (frust, _) = assert_relative_to_project(&root, &absolute_dest);
    assert_eq!(frust, "../../frust/crates/frust");

    let _ = std::fs::remove_dir_all(&root);
}

/// (b) `frust create .` in place: the path the user typed is the one the
/// manifest keeps, plus the nested facade crate.
#[test]
fn a_relative_frust_path_stays_relative_for_an_in_place_project() {
    let root = scratch_root();
    let project = root.join("invoker");

    create(&project, ".", "../frust");
    let (frust, material) = assert_relative_to_project(&root, &project);
    assert_eq!(frust, "../frust/crates/frust");
    assert_eq!(material, "../frust/plugins/material");

    let _ = std::fs::remove_dir_all(&root);
}

/// (c) An absolute path is written as given (its nested facade crate).
#[test]
fn an_absolute_frust_path_stays_absolute() {
    let root = scratch_root();
    let checkout = root.join("frust");

    create(
        &root.join("invoker"),
        "../apps/app",
        checkout.to_str().expect("checkout path is valid UTF-8"),
    );
    let project = root.join("apps/app");
    let (frust, material, text) = manifest_paths(&project);
    assert_eq!(
        Path::new(&frust),
        checkout.join("crates").join("frust"),
        "{text}"
    );
    assert_eq!(
        Path::new(&material),
        checkout.join("plugins").join("material"),
        "{text}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
