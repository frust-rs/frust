//! `frust plugin add` against the real binary: help text, the failure paths
//! (which never reach cargo) and an end-to-end add on a registry-mode fixture
//! project whose plugin has no native module, so no package lookup runs and
//! no network is needed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn frust(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_frust"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("CLICOLOR")
        .output()
        .expect("failed to spawn `frust`")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("frust-plugin-add-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture(tag: &str) -> PathBuf {
    let dir = scratch(tag);
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nfrust = { package = \"frust-ui\", version = \"0.6\" }\n",
    )
    .unwrap();
    dir
}

#[test]
fn help_shows_the_subcommand() {
    let dir = scratch("help");
    let top = frust(&dir, &["--help"]);
    assert!(text(&top.stdout).contains("plugin"));
    let sub = frust(&dir, &["plugin", "add", "--help"]);
    assert!(sub.status.success());
    let help = text(&sub.stdout);
    assert!(help.contains("--feature"), "{help}");
    assert!(help.contains("<ID>"), "{help}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn adds_a_plugin_and_a_rerun_is_idempotent() {
    let dir = fixture("ok");
    let first = frust(&dir, &["plugin", "add", "device-info"]);
    assert_eq!(first.status.code(), Some(0), "{}", text(&first.stderr));
    assert!(text(&first.stdout).contains("[applied]"));
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("frust-device-info"));

    let second = frust(&dir, &["plugin", "add", "device-info"]);
    assert_eq!(second.status.code(), Some(0));
    assert!(!text(&second.stdout).contains("[applied]"));
    assert_eq!(
        std::fs::read_to_string(dir.join("Cargo.toml")).unwrap(),
        manifest
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_id_exits_1_and_lists_valid_ids() {
    let dir = fixture("badid");
    let out = frust(&dir, &["plugin", "add", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("unknown plugin `nope`"), "{err}");
    assert!(err.contains("device-info"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_feature_exits_1_and_lists_valid_features() {
    let dir = fixture("badfeat");
    let out = frust(
        &dir,
        &["plugin", "add", "network-image", "--feature", "nope"],
    );
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(
        err.contains("Valid features for `network-image`: webp"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn outside_a_project_exits_1() {
    let dir = scratch("outside");
    let out = frust(&dir, &["plugin", "add", "device-info"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("Cargo.toml"));
    let _ = std::fs::remove_dir_all(&dir);
}
