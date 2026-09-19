//! The hand-synced profile blocks are duplicated across manifests that Cargo
//! cannot share; DEVELOPMENT.md says they "must be kept in sync by hand" and
//! these tests are the automated backstop (there is no CI — the verify gate
//! is the only enforcement):
//!
//! - `[profile.dev.package.<shader-crate>]` overrides live in FIVE
//!   manifests (root, template, huddle, glyph-catalog, material3-demo) —
//!   drift silently reopens the iOS launch-watchdog crash.
//! - the `[profile.dev.package."*"]` wildcard lives in the SAME five
//!   manifests (root, template, huddle, glyph-catalog, material3-demo — an
//!   earlier examples-convergence deleted `catalog` and `inbox`, folding
//!   their hand-sync role into `huddle`) — drift silently reopens the
//!   measured size/perf regression.
//! - `[profile.release]` hardening lives in NINE: those five plus four
//!   standalone workspaces that hand-copied the block with no tripwire over
//!   them — `benchmarks/frust_bench` (its numbers only mean anything if it
//!   builds under the settings a shipped app does), the two further `frust
//!   create` scaffolds `examples/playground` and `examples/shadertoy`, and
//!   the wasm-only `examples/web-gallery`.
//! - the cold-set `[profile.release.package.<crate>]` block (`opt-level =
//!   "z"` for crates that do no per-frame work) lives in the EIGHT of those
//!   nine that build an Android artifact — i.e. all but `web-gallery`, which
//!   is wasm-only and deliberately has no package overrides. Drift here
//!   silently drops a measured footprint win for one app, or demotes a
//!   per-frame crate to `"z"` in one app alone and makes two apps' frame
//!   numbers incomparable.
//!
//! `examples/glyph-catalog/Cargo.toml` and `examples/material3-demo/Cargo.toml`
//! are two more full `frust create` scaffolds carrying their own copies of
//! these blocks (same reason as `huddle`: each is a standalone workspace
//! whose `[profile.*]` blocks only take effect in a workspace root) — they
//! were hand-authored independently of the root/template/huddle trio, with
//! no automated backstop until this test grew to cover them too.
//!
//! The template manifest contains minijinja placeholders elsewhere in the
//! file, so the blocks are extracted line-wise rather than TOML-parsed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Extracts `package name -> opt-level` from every
/// `[profile.<profile>.package.<name>]` table in the manifest text.
fn package_overrides(manifest: &str, profile: &str) -> BTreeMap<String, String> {
    let prefix = format!("[profile.{profile}.package.");
    let mut out = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in manifest.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(prefix.as_str()) {
            current = rest.strip_suffix(']').map(|name| name.to_string());
        } else if line.starts_with('[') {
            current = None;
        } else if let (Some(name), Some(value)) = (&current, line.strip_prefix("opt-level")) {
            let value = value.trim_start_matches([' ', '=']).trim();
            out.insert(name.clone(), value.to_string());
        }
    }
    out
}

/// `[profile.dev.package.<name>]` overrides — the shader-stack debug fix.
fn dev_package_overrides(manifest: &str) -> BTreeMap<String, String> {
    package_overrides(manifest, "dev")
}

/// `[profile.release.package.<name>]` overrides — the cold-set `opt-level =
/// "z"` footprint block.
fn release_package_overrides(manifest: &str) -> BTreeMap<String, String> {
    package_overrides(manifest, "release")
}

/// Extracts the `key = value` pairs of one `[section]` (exact header match),
/// stopping at the next `[` header. Comment lines are skipped.
fn section_pairs(manifest: &str, header: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut in_section = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line == header {
            in_section = true;
        } else if line.starts_with('[') {
            in_section = false;
        } else if in_section
            && !line.is_empty()
            && !line.starts_with('#')
            && let Some((k, v)) = line.split_once('=')
        {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

/// Every manifest carrying the hand-synced profile blocks: root, template,
/// huddle (an earlier examples-convergence deleted the `catalog` and
/// `inbox` examples, the other two manifests this used to compare), plus
/// the two standalone example scaffolds that independently carry their own
/// copies of the same blocks (`glyph-catalog`, `material3-demo`).
fn synced_manifests() -> Vec<std::path::PathBuf> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    vec![
        repo_root.join("Cargo.toml"),
        repo_root.join("crates/frust-drive/templates/app/Cargo.toml.tmpl"),
        repo_root.join("examples/huddle/Cargo.toml"),
        repo_root.join("examples/glyph-catalog/Cargo.toml"),
        repo_root.join("examples/material3-demo/Cargo.toml"),
    ]
}

/// Every manifest carrying a `[profile.release]` mirror. A superset of
/// `synced_manifests` above: four more standalone workspaces hand-copied the
/// release block without ever being covered by a tripwire — the two further
/// `frust create` scaffolds (`playground`, `shadertoy`), the benchmark app
/// (`benchmarks/frust_bench`, whose numbers are only meaningful if it builds
/// under the same release settings a shipped app does) and the wasm-only
/// `examples/web-gallery`. They carry no `[profile.dev.package.*]` blocks
/// comparable to the five above (each scaffold's dev override set is
/// narrower), so they join the release-block comparison only.
fn release_profile_manifests() -> Vec<std::path::PathBuf> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut manifests = synced_manifests();
    manifests.extend([
        repo_root.join("benchmarks/frust_bench/Cargo.toml"),
        repo_root.join("examples/playground/Cargo.toml"),
        repo_root.join("examples/shadertoy/Cargo.toml"),
        repo_root.join("examples/web-gallery/Cargo.toml"),
    ]);
    manifests
}

/// Every manifest that builds an ANDROID artifact, and therefore carries the
/// hand-synced cold-set `[profile.release.package.*]` block (`opt-level =
/// "z"` for crates that do no per-frame work). This is
/// `release_profile_manifests` MINUS `examples/web-gallery`: that one is
/// wasm-only and deliberately carries the release block without the package
/// overrides, since the cold set was chosen against measured ARM64 device
/// numbers and wasm has no equivalent measurement (see its manifest comment
/// and `web_gallery_carries_no_release_package_overrides` below).
fn android_package_override_manifests() -> Vec<std::path::PathBuf> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let web_gallery = repo_root.join("examples/web-gallery/Cargo.toml");
    release_profile_manifests()
        .into_iter()
        .filter(|path| path != &web_gallery)
        .collect()
}

fn assert_section_identical(
    manifests: Vec<std::path::PathBuf>,
    header: &str,
    expect_keys: &[&str],
) {
    let blocks: Vec<BTreeMap<String, String>> = manifests
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            section_pairs(&text, header)
        })
        .collect();
    for key in expect_keys {
        assert!(
            blocks[0].contains_key(*key),
            "root Cargo.toml's {header} is missing `{key}` — has the \
             profile hardening been removed?"
        );
    }
    for (path, other) in manifests.iter().zip(&blocks).skip(1) {
        assert_eq!(
            &blocks[0],
            other,
            "{header} in {} has drifted from the root Cargo.toml — every \
             hand-synced manifest must stay identical (see DEVELOPMENT.md's \
             profile-sync rule)",
            path.display()
        );
    }
}

/// The Android link-flags file at the repo root is inherited by every nested
/// workspace built in-tree (Cargo's config discovery walks ancestor
/// directories and merges what it finds), so there is exactly one canonical
/// copy plus the app template's own copy (which a scaffolded project takes
/// with it once it leaves this checkout and needs its own file). The
/// `[target.*]` rustflags tables in those two must stay byte-identical the
/// same way the hand-synced profile blocks do.
///
/// The two files are no longer byte-identical as a *whole*: the template
/// additionally carries a `[build] target-dir = "build/rust"` table (see
/// `crates/frust-drive/src/build_dirs.rs`) that the repo root's config
/// deliberately does NOT have — this checkout's own `target/` is referenced
/// directly by docs and CI and must not move. This test narrows to what
/// must actually agree: the `[target.*]` tables structurally, plus a check
/// that the template's `[build]` table contains exactly `target-dir` and
/// nothing else, and that the root file has gained no `[build]` table at
/// all.
#[test]
fn android_cargo_config_identical_between_root_and_template() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root_config = std::fs::read_to_string(repo_root.join(".cargo/config.toml"))
        .expect("failed to read root .cargo/config.toml");
    let template_config = std::fs::read_to_string(
        repo_root.join("crates/frust-drive/templates/app/.cargo/config.toml"),
    )
    .expect("failed to read crates/frust-drive/templates/app/.cargo/config.toml");

    let root_toml: toml::Table =
        toml::from_str(&root_config).expect("root .cargo/config.toml must be valid TOML");
    let template_toml: toml::Table = toml::from_str(&template_config)
        .expect("crates/frust-drive/templates/app/.cargo/config.toml must be valid TOML");

    // Compare the FULL top-level key sets first, not just the one key
    // ("target") the equality checks below happen to read — a new
    // top-level table ([source]/[registries]/[env]/[net]/...) appended to
    // either file would otherwise pass both checks unnoticed.
    let root_keys: BTreeSet<&str> = root_toml.keys().map(String::as_str).collect();
    assert_eq!(
        root_keys,
        BTreeSet::from(["target"]),
        "the repo root .cargo/config.toml's top-level table set must be \
         exactly {{\"target\"}} — a new top-level table \
         ([source]/[registries]/[env]/[net]/...) must not silently appear"
    );
    let template_keys: BTreeSet<&str> = template_toml.keys().map(String::as_str).collect();
    assert_eq!(
        template_keys,
        BTreeSet::from(["build", "target"]),
        "crates/frust-drive/templates/app/.cargo/config.toml's top-level table set must be \
         exactly {{\"build\", \"target\"}} — a new top-level table \
         ([source]/[registries]/[env]/[net]/...) must not silently appear"
    );
    // `root_toml.get("target")` must actually resolve to something —
    // otherwise the equality check right below it would pass vacuously
    // (`None == None`) the moment the root file's `[target.*]` tables went
    // missing, rather than catching the loss.
    assert!(
        root_toml.get("target").is_some(),
        "the repo root .cargo/config.toml has no [target.*] table to \
         compare the template config against"
    );
    assert_eq!(
        root_toml.get("target"),
        template_toml.get("target"),
        "crates/frust-drive/templates/app/.cargo/config.toml's [target.*] rustflags tables have \
         structurally drifted from the repo root's — the two must stay \
         identical"
    );
    assert!(
        root_toml.get("build").is_none(),
        "the repo root .cargo/config.toml must not gain a [build] table — \
         its target/ is referenced directly by docs and CI"
    );
    let mut expected_build = toml::Table::new();
    expected_build.insert(
        "target-dir".to_string(),
        toml::Value::String("build/rust".to_string()),
    );
    assert_eq!(
        template_toml.get("build"),
        Some(&toml::Value::Table(expected_build)),
        "crates/frust-drive/templates/app/.cargo/config.toml's [build] table must contain \
         exactly `target-dir = \"build/rust\"` and nothing else"
    );
}

#[test]
fn release_profile_identical_across_synced_manifests() {
    assert_section_identical(
        release_profile_manifests(),
        "[profile.release]",
        &["lto", "codegen-units", "strip", "panic"],
    );
}

#[test]
fn dev_wildcard_override_identical_across_synced_manifests() {
    assert_section_identical(
        synced_manifests(),
        "[profile.dev.package.\"*\"]",
        &["opt-level"],
    );
}

#[test]
fn shader_stack_dev_overrides_identical_across_synced_manifests() {
    let manifests = synced_manifests();

    let overrides: Vec<BTreeMap<String, String>> = manifests
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            dev_package_overrides(&text)
        })
        .collect();

    assert!(
        !overrides[0].is_empty(),
        "root Cargo.toml has no [profile.dev.package.*] overrides — the \
         shader-stack dev-profile fix has been removed?"
    );
    for (path, other) in manifests.iter().zip(&overrides).skip(1) {
        assert_eq!(
            &overrides[0],
            other,
            "[profile.dev.package.*] overrides in {} have drifted from the \
             root Cargo.toml — every hand-synced manifest must stay identical \
             (see DEVELOPMENT.md's profile-sync rule)",
            path.display()
        );
    }
}

/// STRENGTHENS `shader_stack_dev_overrides_identical_across_synced_manifests`
/// above: that test only compares the *values* of the override names it
/// already knows about, so a crate added to one manifest's
/// `[profile.dev.package.*]` block but forgotten in another escapes it
/// entirely (both blocks agree on every key the shorter one has, and neither
/// test looks at the KEY SET). This one compares the full set of override
/// names across all five manifests instead.
#[test]
fn shader_stack_dev_override_name_set_is_identical_across_synced_manifests() {
    let manifests = synced_manifests();

    let name_sets: Vec<BTreeSet<String>> = manifests
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            dev_package_overrides(&text).into_keys().collect()
        })
        .collect();

    assert!(
        !name_sets[0].is_empty(),
        "root Cargo.toml has no [profile.dev.package.*] overrides — the \
         shader-stack dev-profile fix has been removed?"
    );
    for (path, other) in manifests.iter().zip(&name_sets).skip(1) {
        assert_eq!(
            &name_sets[0],
            other,
            "[profile.dev.package.*] override NAMES in {} differ from the root \
             Cargo.toml — a crate added to one manifest's shader-stack overrides but not \
             the others would otherwise escape the value-only comparison above (see \
             DEVELOPMENT.md's profile-sync rule)",
            path.display()
        );
    }
}

/// The release-profile counterpart of the two dev-override tests above, for
/// the cold-set `[profile.release.package.<crate>] opt-level = "z"` block.
///
/// It compares the full override map — NAMES and VALUES together — so both
/// failure modes the dev pair needs two tests for are caught here at once: a
/// crate added to one manifest's cold set but forgotten in another, and a
/// crate whose opt-level was changed in one place only. Drift either way
/// silently un-does a measured footprint win for whichever app was missed, or
/// (worse) quietly demotes a per-frame crate to `"z"` in one app and not the
/// others, making two apps' frame numbers incomparable.
///
/// Scope is `android_package_override_manifests()` — the eight manifests that
/// build an Android artifact. `examples/web-gallery` is deliberately excluded;
/// the next test pins that exclusion so it stays a decision rather than an
/// oversight.
#[test]
fn release_package_overrides_identical_across_android_manifests() {
    let manifests = android_package_override_manifests();

    let overrides: Vec<BTreeMap<String, String>> = manifests
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            release_package_overrides(&text)
        })
        .collect();

    assert!(
        !overrides[0].is_empty(),
        "root Cargo.toml has no [profile.release.package.*] overrides — the \
         measured cold-set opt-level block has been removed?"
    );
    for (path, other) in manifests.iter().zip(&overrides).skip(1) {
        assert_eq!(
            &overrides[0],
            other,
            "[profile.release.package.*] overrides in {} have drifted from the \
             root Cargo.toml — every Android-building manifest must carry the \
             identical cold-set block, names AND opt-levels (see \
             DEVELOPMENT.md's profile-sync rule)",
            path.display()
        );
    }
}

/// `examples/web-gallery` is wasm-only: it mirrors `[profile.release]` (the
/// test above covers that) but deliberately carries NO package overrides,
/// because the cold set was chosen against measured ARM64 device numbers and
/// the wasm target has no equivalent measurement.
///
/// Without this test the exclusion is invisible — a later reader syncing "all
/// the mirrors" would copy the block in, and nothing would complain. Copying
/// it in is allowed; doing so silently is not. Measure, then update this test.
#[test]
fn web_gallery_carries_no_release_package_overrides() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = repo_root.join("examples/web-gallery/Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let overrides = release_package_overrides(&text);
    assert!(
        overrides.is_empty(),
        "examples/web-gallery/Cargo.toml has grown \
         [profile.release.package.*] overrides ({overrides:?}) — the Android \
         cold set is measured on ARM64 devices and does not transfer to wasm \
         unmeasured. If a wasm measurement now justifies it, update this test \
         and that manifest's comment together."
    );
}
