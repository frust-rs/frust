//! Manifest comment-density budget: a manifest may not drift back into a
//! document that happens to parse as TOML.
//!
//! # Metric
//!
//! For each manifest, `P` counts prose comment lines — full-line `#` comments
//! that are not commented-out TOML (a line matching `# [table]` or
//! `# key = ...` is a disabled setting, not prose) — and `N` counts
//! non-comment, non-blank lines. A manifest fails when `P > PROSE_FLOOR` and
//! `P / N > CEILING`. The floor lets small files carry a short rationale for a
//! constraint however sparse their settings are; the ceiling bounds how much
//! prose a larger file may wrap around its settings. There is no per-file
//! allowlist: a failing file is trimmed, not exempted.
//!
//! `CEILING` is the maximum `P / N` measured over the swept manifests that
//! exceed the floor, rounded up to the next tenth.
//!
//! # File set
//!
//! The same manifests the comment-residue scan reads: `Cargo.toml`,
//! `frust.toml`, `config.toml` directly inside a `.cargo/` directory, and
//! `*.tmpl` files whose name contains `.toml`, under `crates/`, `plugins/`,
//! `benchmarks/`, and `examples/` (skipping `target/` and `workflow/`), plus
//! the repo-root `Cargo.toml` and `.cargo/config.toml`.

use std::fs;
use std::path::{Path, PathBuf};

/// A manifest with this many prose comment lines or fewer is never flagged.
const PROSE_FLOOR: usize = 12;

/// Maximum allowed prose-to-non-comment line ratio above the floor.
const CEILING: f64 = 3.3;

const SCAN_ROOTS: &[&str] = &["crates", "plugins", "benchmarks", "examples"];
const ROOT_MANIFESTS: &[&str] = &["Cargo.toml", ".cargo/config.toml"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/frust has a grandparent (the workspace root)")
        .to_path_buf()
}

fn is_manifest(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let in_cargo_dir = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|dir| dir == ".cargo");
    name == "Cargo.toml"
        || name == "frust.toml"
        || (name == "config.toml" && in_cargo_dir)
        || (name.ends_with(".tmpl") && name.contains(".toml"))
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("dir entry under {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            let skip = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == "target" || n == "workflow");
            if !skip {
                collect(&path, out);
            }
        } else if is_manifest(&path) {
            out.push(path);
        }
    }
}

fn manifests() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut out = Vec::new();
    for scan_root in SCAN_ROOTS {
        let dir = root.join(scan_root);
        assert!(
            dir.is_dir(),
            "expected scan root {} to exist",
            dir.display()
        );
        let before = out.len();
        collect(&dir, &mut out);
        assert!(
            out.len() > before,
            "expected at least one manifest under {}",
            dir.display()
        );
    }
    for name in ROOT_MANIFESTS {
        let path = root.join(name);
        assert!(path.is_file(), "expected {} to exist", path.display());
        out.push(path);
    }
    out.sort();
    out
}

/// True for a commented-out TOML table header or `key = value` setting.
fn is_commented_out_toml(line: &str) -> bool {
    let Some(body) = line.trim_start().strip_prefix('#') else {
        return false;
    };
    let body = body.trim_start();
    if body.starts_with('[') {
        return true;
    }
    let key_len = body
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '"' | '-'))
        .count();
    key_len > 0 && body[key_len..].trim_start().starts_with('=')
}

/// `(P, N)` for one manifest's text — see the module doc's Metric §.
fn measure(contents: &str) -> (usize, usize) {
    let mut prose = 0;
    let mut code = 0;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            if !is_commented_out_toml(trimmed) {
                prose += 1;
            }
        } else {
            code += 1;
        }
    }
    (prose, code)
}

/// The failure message for a manifest over budget, or `None` when it passes.
fn over_budget(label: &str, contents: &str, ceiling: f64) -> Option<String> {
    let (prose, code) = measure(contents);
    if prose <= PROSE_FLOOR {
        return None;
    }
    let ratio = prose as f64 / code.max(1) as f64;
    (ratio > ceiling).then(|| {
        format!(
            "{label}: {prose} prose comment lines over {code} non-comment lines (ratio {ratio:.2} \
             > ceiling {ceiling:.1}, floor {PROSE_FLOOR}) — trim the comments to the constraints \
             a reader cannot recover from the setting itself"
        )
    })
}

#[test]
fn manifests_stay_within_the_comment_budget() {
    let root = workspace_root();
    let files = manifests();
    assert!(
        files.len() > 50,
        "expected well over 50 manifests, found {} — the scan is probably looking in the wrong \
         place",
        files.len()
    );
    assert!(
        files.contains(&root.join("Cargo.toml")),
        "the repo-root Cargo.toml must be among the checked manifests"
    );
    let mut failures = Vec::new();
    for path in &files {
        let contents =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let label = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        failures.extend(over_budget(&label, &contents, CEILING));
    }
    assert!(
        failures.is_empty(),
        "manifest comment budget exceeded ({} file(s)):\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

/// `lines` prose comment lines followed by `settings` key lines.
fn synthetic(lines: usize, settings: usize) -> String {
    let mut out = String::new();
    for i in 0..lines {
        out.push_str(&format!("# explanatory prose line {i}\n"));
    }
    for i in 0..settings {
        out.push_str(&format!("key{i} = {i}\n"));
    }
    out
}

#[test]
fn a_prose_heavy_manifest_fails_the_check() {
    let message = over_budget("fixture/Cargo.toml", &synthetic(40, 4), CEILING)
        .expect("ratio 10 is far above the ceiling");
    assert!(message.contains("fixture/Cargo.toml"), "{message}");
    assert!(message.contains("40 prose"), "{message}");
    assert!(message.contains("4 non-comment"), "{message}");
}

#[test]
fn a_manifest_just_under_the_ceiling_passes() {
    // 33 / 10 = 3.3 sits exactly on the ceiling and is allowed; one more
    // setting only lowers the ratio.
    assert_eq!(
        over_budget("ok/Cargo.toml", &synthetic(33, 10), CEILING),
        None
    );
    assert_eq!(
        over_budget("ok/Cargo.toml", &synthetic(33, 11), CEILING),
        None
    );
    assert!(over_budget("bad/Cargo.toml", &synthetic(34, 10), CEILING).is_some());
}

#[test]
fn the_floor_exempts_small_prose_blocks() {
    assert_eq!(
        over_budget("small/Cargo.toml", &synthetic(PROSE_FLOOR, 1), CEILING),
        None,
        "exactly at the floor is never flagged however sparse the settings"
    );
    assert!(over_budget("big/Cargo.toml", &synthetic(PROSE_FLOOR + 1, 1), CEILING).is_some());
}

#[test]
fn commented_out_toml_is_not_prose() {
    let contents =
        "# [dependencies]\n# serde = \"1\"\n#   foo.bar-baz = true\n# some prose\nname = \"x\"\n\n";
    assert_eq!(measure(contents), (1, 1));
    assert!(is_commented_out_toml("# \"quoted-key\" = 1"));
    assert!(!is_commented_out_toml(
        "# Pinned to avoid the resolver bug (a = b)"
    ));
}
