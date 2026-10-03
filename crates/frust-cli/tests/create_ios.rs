//! End-to-end test: `frust create`'s iOS output is a well-formed Xcode
//! project. This does *not* build the app — it only
//! confirms the generated `project.pbxproj` parses (`xcodebuild -list`) and
//! the rendered `Info.plist` lints (`plutil -lint`). A full simulator
//! build is exercised separately, via `frust build ios`/`frust run`
//! (`docs/DEVELOPMENT.md`).
//!
//! Like `create_e2e`, this drives the compiled `frust` binary via
//! `CARGO_BIN_EXE_frust` rather than calling `scaffold::generate`
//! in-process (frust-cli is binary-only, no `lib` target).
//!
//! Ignored by default: it shells out to `xcodebuild`/`plutil`, which only
//! exist on macOS with the Xcode command-line tools installed. Run
//! explicitly on such a machine:
//! `cargo test -p frust-cli --test create_ios -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

/// Absolute path to this workspace's `crates/frust` (the facade crate the
/// generated project path-depends on via `--frust-path`, keeping these tests
/// hermetic: no crates.io access needed).
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
    std::env::temp_dir().join(format!("frust-cli-ios-e2e-{}-{n}", std::process::id()))
}

#[test]
#[ignore = "shells out to `xcodebuild`/`plutil` (macOS + Xcode command-line tools only); run explicitly with `--ignored`"]
fn create_ios_project_is_parseable() {
    let dest = unique_dest();
    let _ = std::fs::remove_dir_all(&dest);

    let frust_exe = env!("CARGO_BIN_EXE_frust");
    let frust_path = workspace_frust_path();
    let create_status = Command::new(frust_exe)
        .args([
            "create",
            dest.to_str().expect("dest path is valid UTF-8"),
            "--project-name",
            "fk_ios_app",
            "--org",
            "dev.frust",
            "--frust-path",
            frust_path.to_str().expect("frust_path is valid UTF-8"),
        ])
        .status()
        .expect("failed to spawn `frust create`");
    assert!(create_status.success(), "`frust create` exited non-zero");

    let xcodeproj = dest.join("ios/Runner.xcodeproj");
    assert!(
        xcodeproj.join("project.pbxproj").exists(),
        "expected {}",
        xcodeproj.join("project.pbxproj").display()
    );

    // The local package reference and the symlink it resolves through.
    let pbxproj = std::fs::read_to_string(xcodeproj.join("project.pbxproj")).unwrap();
    assert!(
        pbxproj.contains("relativePath = \"FrustEmbedding\";"),
        "pbxproj lacks the FrustEmbedding local package reference"
    );
    assert!(
        dest.join("ios/FrustEmbedding").is_symlink(),
        "expected ios/FrustEmbedding to be a symlink"
    );

    // `xcodebuild -list` parses the pbxproj and enumerates targets/schemes
    // without building anything — the cheap "is this a valid project?" check.
    let list = Command::new("xcodebuild")
        .args(["-list", "-project"])
        .arg(&xcodeproj)
        .output()
        .expect("failed to spawn `xcodebuild`");
    assert!(
        list.status.success(),
        "`xcodebuild -list` failed:\n{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let listing = String::from_utf8_lossy(&list.stdout);
    assert!(
        listing.contains("Runner"),
        "`xcodebuild -list` did not report the Runner target/scheme:\n{listing}"
    );
    // Debug/Release/Profile must all be present in the project's configuration
    // matrix: the run-script phase and version build settings both
    // depend on `Profile` being a real XCBuildConfiguration, not just scheme
    // boilerplate.
    for configuration in ["Debug", "Release", "Profile"] {
        assert!(
            listing.contains(configuration),
            "`xcodebuild -list` did not report the `{configuration}` build configuration:\n{listing}"
        );
    }

    // The shared scheme must be present for headless `-scheme` builds.
    assert!(
        xcodeproj
            .join("xcshareddata/xcschemes/Runner.xcscheme")
            .exists()
    );

    // `plutil -lint` validates the rendered Info.plist is well-formed.
    let plist = dest.join("ios/Runner/Info.plist");
    let lint = Command::new("plutil")
        .arg("-lint")
        .arg(&plist)
        .output()
        .expect("failed to spawn `plutil`");
    assert!(
        lint.status.success(),
        "`plutil -lint` failed on {}:\n{}",
        plist.display(),
        String::from_utf8_lossy(&lint.stderr)
    );

    // Versions are sourced from build settings, not hardcoded into
    // the plist.
    let plist_src = std::fs::read_to_string(&plist).expect("reading rendered Info.plist");
    assert!(
        plist_src.contains("$(MARKETING_VERSION)"),
        "expected CFBundleShortVersionString to reference $(MARKETING_VERSION):\n{plist_src}"
    );
    assert!(
        plist_src.contains("$(CURRENT_PROJECT_VERSION)"),
        "expected CFBundleVersion to reference $(CURRENT_PROJECT_VERSION):\n{plist_src}"
    );

    let _ = std::fs::remove_dir_all(&dest);
}

/// A `--deeplink-scheme` project's rendered Info.plist (with its
/// `CFBundleURLTypes` entry) still lints as a well-formed plist.
#[test]
#[ignore = "shells out to `plutil` (macOS + Xcode command-line tools only); run explicitly with `--ignored`"]
fn create_ios_project_with_deeplink_scheme_has_valid_plist() {
    let dest = unique_dest();
    let _ = std::fs::remove_dir_all(&dest);

    let frust_exe = env!("CARGO_BIN_EXE_frust");
    let frust_path = workspace_frust_path();
    let create_status = Command::new(frust_exe)
        .args([
            "create",
            dest.to_str().expect("dest path is valid UTF-8"),
            "--project-name",
            "fk_ios_deeplink_app",
            "--org",
            "dev.frust",
            "--deeplink-scheme",
            "fkdeeplink",
            "--frust-path",
            frust_path.to_str().expect("frust_path is valid UTF-8"),
        ])
        .status()
        .expect("failed to spawn `frust create`");
    assert!(create_status.success(), "`frust create` exited non-zero");

    let plist = dest.join("ios/Runner/Info.plist");
    let plist_src = std::fs::read_to_string(&plist).expect("reading rendered Info.plist");
    assert!(
        plist_src.contains("CFBundleURLTypes"),
        "expected CFBundleURLTypes in {}:\n{plist_src}",
        plist.display()
    );
    assert!(
        plist_src.contains("<string>fkdeeplink</string>"),
        "{plist_src}"
    );

    let lint = Command::new("plutil")
        .arg("-lint")
        .arg(&plist)
        .output()
        .expect("failed to spawn `plutil`");
    assert!(
        lint.status.success(),
        "`plutil -lint` failed on {}:\n{}",
        plist.display(),
        String::from_utf8_lossy(&lint.stderr)
    );

    let _ = std::fs::remove_dir_all(&dest);
}
