//! Integration coverage for the `widget-snapshots` binary's own orchestration
//! (`src/bin/widget-snapshots.rs`): CLI argument handling, the write/check
//! round-trip, and the `--filter` manifest-merge rule
//! (`frust_testing::snapshot::merge_manifest`).
//!
//! These tests drive the compiled binary via
//! `env!("CARGO_BIN_EXE_widget-snapshots")` (the same shape
//! `crates/frust-cli/tests/*_e2e.rs` uses for `frust`), each against a unique
//! scratch directory under [`std::env::temp_dir`]. `tempfile` is not in this
//! workspace's lockfile and stays out per the Version-Pin Policy.
//!
//! Deliberately pixel-blind: `examples/gallery`'s case colours can change
//! independently of this crate (a concurrent worker owns that), so nothing
//! here asserts on specific bytes — only on the tool's own behaviour (a
//! render round-trips through `--check`, tampering is detected, a merge keeps
//! what it should and replaces what it should).
//!
//! Every case renders through the GPU-free CPU oracle (the binary's default;
//! none of these pass `--gpu`), so this needs no adapter and stays fast: two
//! known slugs for the write/merge case, one case each for the tamper/delete
//! cases, and three exhaustive (whole-registry) tests — stray reporting, the
//! `--check` success path plus the light/dark divergence guard, and pruning —
//! each of which renders the registry twice; the whole file still runs in a
//! few seconds.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

/// A fresh, never-before-used scratch directory under the system temp dir.
fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "frust-testing-widget-snapshots-bin-{tag}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Runs the compiled `widget-snapshots` binary with `args`, capturing output.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_widget-snapshots"))
        .args(args)
        .output()
        .expect("failed to spawn the widget-snapshots binary")
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn manifest_slugs(out_dir: &Path) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(out_dir.join("manifest.json"))
        .expect("manifest.json should be readable");
    let json: serde_json::Value = serde_json::from_str(&text).expect("manifest.json is valid JSON");
    json["snapshots"]
        .as_array()
        .expect("manifest.json has a snapshots array")
        .iter()
        .map(|row| {
            (
                row["slug"].as_str().expect("row has a slug").to_string(),
                row["variant"]
                    .as_str()
                    .expect("row has a variant")
                    .to_string(),
            )
        })
        .collect()
}

/// `--list` prints the matching cases and renders nothing (no `--out` given
/// at all, so a render would have nowhere to go).
#[test]
fn list_prints_known_slugs_and_renders_nothing() {
    let out = run(&["--list"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stdout = stdout_of(&out);
    assert!(
        stdout.lines().any(|line| line.starts_with("button\t")),
        "expected a `button` row in --list output:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line.starts_with("container\t")),
        "expected a `container` row in --list output:\n{stdout}"
    );
}

/// A missing `--out` (and no `--list`) is a clean, non-zero, immediate
/// refusal — never a panic or an attempt to render into nothing.
#[test]
fn missing_out_exits_non_zero_with_a_clear_message() {
    let out = run(&["--filter", "button"]);
    assert!(
        !out.status.success(),
        "a run with no --out must not succeed"
    );
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("--out"),
        "the refusal should name the missing flag:\n{stderr}"
    );
}

/// The write/merge rule at the heart of review finding (A): a fresh `--out`
/// under `--filter` writes a manifest describing only what it rendered, and a
/// SECOND filtered run for a different slug merges into it rather than
/// clobbering it — the first run's rows survive untouched, and the tree on
/// disk always matches the manifest.
#[test]
fn a_second_filtered_run_merges_into_the_first_filtered_manifest() {
    let dir = unique_dir("merge");

    // 1) Fresh --out, filtered to exactly one known slug ("container" is not
    //    a substring of anything else in the registry).
    let first = run(&["--out", dir.to_str().unwrap(), "--filter", "container"]);
    assert!(first.status.success(), "{}", stderr_of(&first));
    assert!(
        dir.join("container.light.png").is_file(),
        "the first run should have written container.light.png"
    );
    assert!(
        dir.join("container.dark.png").is_file(),
        "the first run should have written container.dark.png"
    );
    assert!(
        !dir.join("button.light.png").exists(),
        "the first run must not render anything outside its own filter"
    );
    // No manifest existed yet: the filtered manifest describes only this run.
    let mut after_first = manifest_slugs(&dir);
    after_first.sort();
    assert_eq!(
        after_first,
        vec![
            ("container".to_string(), "dark".to_string()),
            ("container".to_string(), "light".to_string()),
        ],
        "a filtered run with no prior manifest must describe only its own rows"
    );
    assert!(
        stderr_of(&first).contains("no existing manifest.json"),
        "a filtered run writing a from-scratch manifest must say so on stderr:\n{}",
        stderr_of(&first)
    );

    // 2) A second filtered run, for two DIFFERENT known slugs sharing the
    //    substring "button" ("button" and "icon-button").
    let second = run(&["--out", dir.to_str().unwrap(), "--filter", "button"]);
    assert!(second.status.success(), "{}", stderr_of(&second));
    assert!(dir.join("button.light.png").is_file());
    assert!(dir.join("icon-button.dark.png").is_file());
    // The FIRST run's files are still there — a filtered write never deletes.
    assert!(
        dir.join("container.light.png").is_file(),
        "the second filtered run must not remove the first run's PNGs"
    );

    // 3) The merge rule: the manifest now covers BOTH runs' slugs, not just
    //    the second run's.
    let mut after_second = manifest_slugs(&dir);
    after_second.sort();
    let mut expected = vec![
        ("button".to_string(), "dark".to_string()),
        ("button".to_string(), "light".to_string()),
        ("container".to_string(), "dark".to_string()),
        ("container".to_string(), "light".to_string()),
        ("icon-button".to_string(), "dark".to_string()),
        ("icon-button".to_string(), "light".to_string()),
    ];
    expected.sort();
    assert_eq!(
        after_second, expected,
        "the second filtered run's manifest must merge onto the first's, not replace it"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `--check` against a run's own fresh output must pass — the write/check
/// round-trip this whole tool exists to guarantee.
#[test]
fn check_passes_against_its_own_fresh_output() {
    let dir = unique_dir("check-ok");
    let write = run(&["--out", dir.to_str().unwrap(), "--filter", "container"]);
    assert!(write.status.success(), "{}", stderr_of(&write));

    let check = run(&[
        "--out",
        dir.to_str().unwrap(),
        "--filter",
        "container",
        "--check",
    ]);
    assert!(
        check.status.success(),
        "--check must pass against its own output:\n{}",
        stderr_of(&check)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Tampering with one byte of a written PNG must fail `--check` and name the
/// exact file that differs.
#[test]
fn check_fails_and_names_the_file_after_a_png_is_tampered_with() {
    let dir = unique_dir("check-tamper");
    let write = run(&["--out", dir.to_str().unwrap(), "--filter", "container"]);
    assert!(write.status.success(), "{}", stderr_of(&write));

    let png_path = dir.join("container.light.png");
    let mut bytes = std::fs::read(&png_path).expect("reading the written PNG");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&png_path, &bytes).expect("tampering with the written PNG");

    let check = run(&[
        "--out",
        dir.to_str().unwrap(),
        "--filter",
        "container",
        "--check",
    ]);
    assert!(
        !check.status.success(),
        "--check must fail once a PNG's bytes have changed on disk"
    );
    let stderr = stderr_of(&check);
    assert!(
        stderr.contains("container.light.png") && stderr.contains("differs"),
        "the failure must name the tampered file:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Deleting a produced PNG must fail `--check` and name it as missing.
#[test]
fn check_fails_and_names_the_file_after_a_png_is_deleted() {
    let dir = unique_dir("check-delete");
    let write = run(&["--out", dir.to_str().unwrap(), "--filter", "container"]);
    assert!(write.status.success(), "{}", stderr_of(&write));

    std::fs::remove_file(dir.join("container.dark.png")).expect("deleting the written PNG");

    let check = run(&[
        "--out",
        dir.to_str().unwrap(),
        "--filter",
        "container",
        "--check",
    ]);
    assert!(
        !check.status.success(),
        "--check must fail once a produced PNG has been deleted"
    );
    let stderr = stderr_of(&check);
    assert!(
        stderr.contains("container.dark.png") && stderr.contains("missing"),
        "the failure must name the missing file:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A stray `.png` under `--out` that no case produces is only ever detected
/// by an EXHAUSTIVE `--check` (no `--filter`) — a filtered run has no way to
/// tell a stray apart from a case it deliberately skipped.
#[test]
fn an_exhaustive_check_reports_a_stray_png() {
    let dir = unique_dir("check-stray");
    let write = run(&["--out", dir.to_str().unwrap()]);
    assert!(write.status.success(), "{}", stderr_of(&write));

    let stray_path = dir.join("not-a-real-case.light.png");
    std::fs::write(&stray_path, b"not a real png, just bytes")
        .expect("writing a stray .png under --out");

    let check = run(&["--out", dir.to_str().unwrap(), "--check"]);
    assert!(
        !check.status.success(),
        "an exhaustive --check must fail once a stray .png is present"
    );
    let stderr = stderr_of(&check);
    assert!(
        stderr.contains("not-a-real-case.light.png") && stderr.contains("not produced by any case"),
        "the failure must name the stray file:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Reads `(slug, variant, sha256)` triples back out of the manifest at `out_dir`.
fn manifest_hashes(out_dir: &Path) -> Vec<(String, String, String)> {
    let text = std::fs::read_to_string(out_dir.join("manifest.json")).expect("manifest.json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("manifest parses");
    json["snapshots"]
        .as_array()
        .expect("snapshots array")
        .iter()
        .map(|row| {
            (
                row["slug"].as_str().unwrap().to_owned(),
                row["variant"].as_str().unwrap().to_owned(),
                row["sha256"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// An exhaustive write followed by an exhaustive `--check` must pass, and no
/// case may render byte-identically in light and dark: a fixed full-frame
/// backdrop painted over the recorder's per-variant surface clear is exactly
/// the defect that once made 22 of 35 previews publish one image for both
/// themes. Colour-agnostic on purpose -- it asserts only that the two variant
/// renders of a slug differ, never what they contain.
#[test]
fn exhaustive_check_passes_and_every_case_differs_between_light_and_dark() {
    let dir = unique_dir("exhaustive-ok");
    let write = run(&["--out", dir.to_str().unwrap()]);
    assert!(write.status.success(), "{}", stderr_of(&write));

    let check = run(&["--out", dir.to_str().unwrap(), "--check"]);
    assert!(
        check.status.success(),
        "exhaustive --check must pass against its own output:\n{}",
        stderr_of(&check)
    );

    let mut by_slug: std::collections::BTreeMap<String, Vec<(String, String)>> =
        std::collections::BTreeMap::new();
    for (slug, variant, sha) in manifest_hashes(&dir) {
        by_slug.entry(slug).or_default().push((variant, sha));
    }
    assert!(!by_slug.is_empty(), "the registry rendered nothing");
    // Establish the shape first: exactly one light and one dark row per slug,
    // so a missing or renamed variant fails loudly instead of exempting the
    // slug from the divergence check below.
    for (slug, rows) in &by_slug {
        let mut tags: Vec<&str> = rows.iter().map(|(v, _)| v.as_str()).collect();
        tags.sort_unstable();
        assert_eq!(
            tags,
            ["dark", "light"],
            "{slug}: expected exactly one light and one dark row"
        );
    }
    let identical: Vec<&String> = by_slug
        .iter()
        .filter(|(_, rows)| {
            let light = rows.iter().find(|(v, _)| v == "light").map(|(_, s)| s);
            let dark = rows.iter().find(|(v, _)| v == "dark").map(|(_, s)| s);
            light == dark
        })
        .map(|(slug, _)| slug)
        .collect();
    assert!(
        identical.is_empty(),
        "these cases render byte-identically in light and dark (a fixed full-frame backdrop is \
         hiding the variant surface): {identical:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An exhaustive write prunes a PNG that the previous manifest listed but the
/// registry no longer produces (a renamed or deleted case) — and ONLY such
/// files: a `.png` the manifest never attributed to this tool is left alone
/// whatever `--out` names, and a filtered write never prunes at all.
#[test]
fn an_exhaustive_write_prunes_only_manifest_listed_stale_pngs() {
    let dir = unique_dir("prune");
    let first = run(&["--out", dir.to_str().unwrap()]);
    assert!(first.status.success(), "{}", stderr_of(&first));

    // Simulate a case that was renamed away after the first run: its PNG and
    // its manifest row exist, but no registry case produces it any more.
    let stale = dir.join("renamed-away.light.png");
    std::fs::copy(dir.join("button.light.png"), &stale).expect("copy a real PNG");
    let manifest_path = dir.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    let mut row = manifest["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["slug"] == "button" && r["variant"] == "light")
        .cloned()
        .expect("button.light row");
    row["slug"] = serde_json::Value::String("renamed-away".into());
    manifest["snapshots"].as_array_mut().unwrap().push(row);
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // A PNG this tool never wrote: must survive every run below.
    let foreign = dir.join("not-ours.png");
    std::fs::write(&foreign, b"not a real png").expect("write foreign file");

    let filtered = run(&["--out", dir.to_str().unwrap(), "--filter", "container"]);
    assert!(filtered.status.success(), "{}", stderr_of(&filtered));
    assert!(stale.exists(), "a filtered write must not prune");
    assert!(
        foreign.exists(),
        "a filtered write must not touch foreign files"
    );

    let exhaustive = run(&["--out", dir.to_str().unwrap()]);
    assert!(exhaustive.status.success(), "{}", stderr_of(&exhaustive));
    assert!(
        !stale.exists(),
        "an exhaustive write must prune the manifest-listed stale PNG"
    );
    assert!(
        foreign.exists(),
        "an exhaustive write must never delete a PNG the manifest did not list"
    );
    let err = stderr_of(&exhaustive);
    assert!(
        err.contains("renamed-away.light.png") && err.contains("pruned 1 stale PNG"),
        "each pruned path and the count must be reported on stderr:\n{err}"
    );

    // The foreign file is still reported as a stray by an exhaustive --check —
    // reported, never deleted.
    let check = run(&["--out", dir.to_str().unwrap(), "--check"]);
    assert!(
        !check.status.success(),
        "a foreign stray must fail an exhaustive --check"
    );
    assert!(
        stderr_of(&check).contains("not-ours.png"),
        "{}",
        stderr_of(&check)
    );
    assert!(foreign.exists(), "--check must not delete");

    let _ = std::fs::remove_dir_all(&dir);
}
