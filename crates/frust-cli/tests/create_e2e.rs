//! End-to-end test: `frust create`'s output actually builds against the
//! real `frust` facade crate.
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
//! project's full dependency graph (winit/wgpu/frust-engine/parley via the
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

/// Asserts the path-mode wiring `frust create --frust-path` wrote: the
/// machine-local `android/local.properties` `frust.embedding.dir` and the
/// `ios/FrustEmbedding` symlink both point into this checkout's shell crates,
/// while the tracked `gradle.properties` and `settings.gradle.kts` name no
/// path at all.
fn assert_path_mode_wiring(dest: &Path) {
    let checkout = workspace_frust_path()
        .join("../..")
        .canonicalize()
        .expect("the checkout root must exist");
    let android = checkout.join("crates/frust-shell-android/platform/android/frust-embedding");
    let ios = checkout.join("crates/frust-shell-ios/platform/ios/FrustEmbedding");

    let properties = std::fs::read_to_string(dest.join("android/local.properties"))
        .expect("reading android/local.properties");
    let value = properties
        .lines()
        .find_map(|line| line.strip_prefix("frust.embedding.dir="))
        .unwrap_or_else(|| panic!("no frust.embedding.dir in:\n{properties}"));
    for tracked in ["android/gradle.properties", "android/settings.gradle.kts"] {
        let text = std::fs::read_to_string(dest.join(tracked)).expect(tracked);
        assert!(
            !text.contains(&checkout.display().to_string()),
            "{tracked} names the checkout:\n{text}"
        );
        assert!(
            !text.contains("frust.embedding.dir=") && !text.contains("gradleProperty"),
            "{tracked} still carries the embedding key:\n{text}"
        );
    }
    assert_eq!(
        Path::new(value)
            .canonicalize()
            .expect("embedding dir exists"),
        android.canonicalize().expect("android embedding exists"),
        "{properties}"
    );

    let link = dest.join("ios/FrustEmbedding");
    let target = std::fs::read_link(&link).expect("ios/FrustEmbedding must be a symlink");
    assert_eq!(
        target.canonicalize().expect("ios embedding target exists"),
        ios.canonicalize().expect("ios embedding exists"),
        "symlink target {}",
        target.display()
    );
}

#[test]
#[ignore = "compiles the generated project's full dependency graph (winit/wgpu/frust-engine); run explicitly with `--ignored`"]
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
    assert_path_mode_wiring(&dest);

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

    // The generated tree must already be `rustfmt`-clean — the manifest
    // guarantees no downstream project has to reformat its own untouched
    // scaffold before its first commit passes CI (docs/DEVELOPMENT.md's
    // Formatting section pins `cargo fmt --check` into the standard verify
    // gate; scaffolded projects inherit that expectation from turn one).
    let fmt_status = Command::new("cargo")
        .args(["fmt", "--check"])
        .current_dir(&dest)
        .status()
        .expect("failed to spawn `cargo fmt --check` in the generated project");
    assert!(
        fmt_status.success(),
        "generated project at {} is not `cargo fmt --check`-clean",
        dest.display()
    );

    let _ = std::fs::remove_dir_all(&dest);
}

/// `frust create --arch clean-signals` renders a project whose source
/// actually compiles and is `cargo fmt --check`-clean.
///
/// Ignored for the same reason as the default-template e2e test above: a full
/// dependency-graph compile from a cold target dir. It needs no sibling
/// `../clean-signals-rs` checkout and never skips itself: the generated
/// `Cargo.toml` requires `clean-signals` from crates.io, so this runs on any
/// host with network access and always executes the fmt assertions below.
/// Run explicitly:
/// `cargo test -p frust-cli --test create_e2e -- --ignored --nocapture`
#[test]
#[ignore = "compiles the generated project's full dependency graph (winit/wgpu/frust-engine/clean-signals); run explicitly with `--ignored`"]
fn scaffolded_clean_signals_project_builds_against_the_real_facade_and_plugin() {
    let dest = unique_dest();
    let _ = std::fs::remove_dir_all(&dest);

    let frust_exe = env!("CARGO_BIN_EXE_frust");
    let frust_path = workspace_frust_path();

    let create_status = Command::new(frust_exe)
        .args([
            "create",
            dest.to_str().expect("dest path is valid UTF-8"),
            "--project-name",
            "fk_e2e_clean_signals_app",
            "--frust-path",
            frust_path.to_str().expect("frust_path is valid UTF-8"),
            "--arch",
            "clean-signals",
        ])
        .status()
        .expect("failed to spawn `frust create --arch clean-signals`");
    assert!(
        create_status.success(),
        "`frust create --arch clean-signals` exited non-zero"
    );

    assert!(dest.join("Cargo.toml").exists());
    assert!(dest.join("src/lib.rs").exists());
    let cargo_toml = std::fs::read_to_string(dest.join("Cargo.toml")).unwrap();
    assert!(cargo_toml.contains("clean-signals-frust"), "{cargo_toml}");
    let lib_rs = std::fs::read_to_string(dest.join("src/lib.rs")).unwrap();
    assert!(lib_rs.contains("GreetingController"), "{lib_rs}");
    assert_path_mode_wiring(&dest);

    let build_status = Command::new("cargo")
        .arg("build")
        .current_dir(&dest)
        .status()
        .expect("failed to spawn `cargo build` in the generated clean-signals project");
    assert!(
        build_status.success(),
        "generated clean-signals project at {} failed to `cargo build`",
        dest.display()
    );

    // Same `rustfmt`-clean guarantee as the default-arch test above.
    let fmt_status = Command::new("cargo")
        .args(["fmt", "--check"])
        .current_dir(&dest)
        .status()
        .expect("failed to spawn `cargo fmt --check` in the generated clean-signals project");
    assert!(
        fmt_status.success(),
        "generated clean-signals project at {} is not `cargo fmt --check`-clean",
        dest.display()
    );

    let _ = std::fs::remove_dir_all(&dest);
}

/// Registry mode: `frust create` without `--frust-path` scaffolds the
/// crates.io release, and the result builds and passes `frust doctor`.
///
/// Needs network and the released crates (`frust-ui`, `frust-material`, ...) on
/// crates.io at this workspace's version. The go-public plan's r5-06 is where
/// it first runs for real; until 0.5.0 is published it is expected to fail.
/// Run explicitly:
/// `cargo test -p frust-cli --test create_e2e -- --ignored scaffolded_project_builds_from_the_registry`
#[test]
#[ignore = "needs network and the released crates on crates.io (first runs for real in go-public plan r5-06)"]
fn scaffolded_project_builds_from_the_registry() {
    let dest = unique_dest();
    let _ = std::fs::remove_dir_all(&dest);

    let frust_exe = env!("CARGO_BIN_EXE_frust");
    let version = env!("CARGO_PKG_VERSION");

    let create_status = Command::new(frust_exe)
        .args([
            "create",
            dest.to_str().expect("dest path is valid UTF-8"),
            "--project-name",
            "fk_e2e_registry_app",
        ])
        .status()
        .expect("failed to spawn `frust create`");
    assert!(create_status.success(), "`frust create` exited non-zero");

    let cargo_toml = std::fs::read_to_string(dest.join("Cargo.toml")).unwrap();
    assert!(
        cargo_toml.contains(&format!(
            "frust = {{ package = \"frust-ui\", version = \"{version}\" }}"
        )),
        "{cargo_toml}"
    );
    assert!(
        cargo_toml.contains(&format!("frust-material = \"{version}\"")),
        "{cargo_toml}"
    );
    assert!(!cargo_toml.contains("frust-glyph"), "{cargo_toml}");

    let build_status = Command::new("cargo")
        .arg("build")
        .current_dir(&dest)
        .status()
        .expect("failed to spawn `cargo build` in the generated project");
    assert!(
        build_status.success(),
        "registry-mode project at {} failed to `cargo build`",
        dest.display()
    );

    let doctor_status = Command::new(frust_exe)
        .arg("doctor")
        .current_dir(&dest)
        .status()
        .expect("failed to spawn `frust doctor` in the generated project");
    assert!(
        doctor_status.success(),
        "`frust doctor` failed in {}",
        dest.display()
    );

    let _ = std::fs::remove_dir_all(&dest);
}
