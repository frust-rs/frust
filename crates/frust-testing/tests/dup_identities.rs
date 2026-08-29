//! Guard G6: the render stack's DUPLICATE crate identities are exactly the
//! ones we chose, and no others.
//!
//! `Cargo.lock` currently resolves two `vello_common`s and two `glifo`s at
//! once, and that is deliberate:
//!
//! - `vello_common 0.0.9` / `glifo 0.1.1` arrive under `frust-render`'s
//!   feature-gated legacy CPU fallback (`vello_cpu = "=0.0.9"`);
//! - `vello_common 0.2.0` / `glifo 0.3.0` are the ENGINE's own core — reached
//!   through `vello_hybrid 0.2.0` and through this crate's `vello_cpu_oracle`
//!   (`vello_cpu = "=0.2.0"`) oracle pin, which is why the oracle rasterizes
//!   with the same code the engine does (see `src/oracle_cpu.rs`).
//!
//! `docs/DEVELOPMENT.md`'s Version-Pin Policy asks for a `cargo tree -d` check
//! after any manifest change; this test is that check, frozen into the gate so
//! a third identity (or a silent collapse to one) fails CI rather than waiting
//! for someone to run the command by hand. A plain lockfile scan, run as an
//! ordinary `cargo test` — the same shape, and the same rationale, as this
//! crate's `tests/deps.rs` guard.
//!
//! Note when reproducing this by hand: the 0.0.9 side is reached only through
//! `frust-render`'s non-default `cpu-tier` feature, so a bare `cargo tree -d`
//! (default features) shows just the 0.2.0/0.3.0 identities —
//! `cargo tree -d --all-features` is the command whose output matches what
//! this test asserts. The lockfile records every optional dependency
//! regardless, which is why the scan below reads it directly.
//!
//! **When the swap phase retires the `=0.0.9` pin**, this guard does not get
//! deleted: flip each expectation to the single remaining version, so the file
//! keeps asserting that exactly one identity is left.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Every duplicated package this workspace sanctions, with the exact set of
/// versions allowed for it. A version present here but absent from the lock is
/// as much a failure as an unexpected one: both mean the graph moved.
const SANCTIONED_DUPLICATES: &[(&str, &[&str])] = &[
    ("vello_common", &["0.0.9", "0.2.0"]),
    ("glifo", &["0.1.1", "0.3.0"]),
];

/// The repository root, resolved from this crate's manifest dir
/// (`crates/frust-testing`) so the scan is working-directory-independent —
/// the same resolution `tests/deps.rs` uses.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/frust-testing has two ancestors: crates/, then the repo root")
        .to_path_buf()
}

/// Every `[[package]]` entry in the root `Cargo.lock`, as `(name, version)`.
///
/// The lockfile is read rather than `cargo metadata` shelled out to: the lock
/// is the artifact the `--locked` build gate actually consumes, and reading it
/// keeps this test hermetic (no network, no target dir, no cargo subprocess).
fn locked_packages(root: &Path) -> Vec<(String, String)> {
    let lock_path = root.join("Cargo.lock");
    let contents = fs::read_to_string(&lock_path)
        .unwrap_or_else(|err| panic!("reading {}: {err}", lock_path.display()));
    let lock: toml::Table = contents
        .parse()
        .unwrap_or_else(|err| panic!("parsing {}: {err}", lock_path.display()));

    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("{} has no [[package]] entries", lock_path.display()));

    packages
        .iter()
        .filter_map(|package| {
            let name = package.get("name")?.as_str()?.to_string();
            let version = package.get("version")?.as_str()?.to_string();
            Some((name, version))
        })
        .collect()
}

/// The versions of `name` present in the lock, deduplicated and sorted.
fn versions_of(packages: &[(String, String)], name: &str) -> BTreeSet<String> {
    packages
        .iter()
        .filter(|(package, _)| package == name)
        .map(|(_, version)| version.clone())
        .collect()
}

#[test]
fn sanctioned_duplicate_identities_are_exactly_as_declared() {
    let packages = locked_packages(&repo_root());
    assert!(
        !packages.is_empty(),
        "Cargo.lock parsed to zero packages — the scan is not reading what it thinks it is"
    );

    for (name, expected) in SANCTIONED_DUPLICATES {
        let found = versions_of(&packages, name);
        let expected: BTreeSet<String> = expected.iter().map(|v| (*v).to_string()).collect();
        assert_eq!(
            found, expected,
            "`{name}` identities in Cargo.lock drifted: expected {expected:?}, found {found:?}. \
             Two identities of the render core are sanctioned ONLY as the legacy-cpu-tier / \
             engine split this file documents — if this change is intentional, update \
             SANCTIONED_DUPLICATES and say why in the commit message; if it is not, a pin was \
             bumped or a route added that unifies (or forks) the graph."
        );
    }
}

#[test]
fn the_oracle_pin_is_the_engine_side_identity() {
    // The oracle exists to rasterize with the ENGINE's core, so its own
    // `vello_cpu` must be the 0.2.0 one — the whole point of the second
    // identity above. A `vello_cpu` that ever collapsed to only 0.0.9 would
    // leave the oracle silently comparing against the legacy rasterizer.
    let packages = locked_packages(&repo_root());
    let vello_cpu = versions_of(&packages, "vello_cpu");
    assert!(
        vello_cpu.contains("0.2.0"),
        "the CPU oracle's `vello_cpu =0.2.0` pin is missing from Cargo.lock, found {vello_cpu:?}"
    );
    assert!(
        vello_cpu.contains("0.0.9"),
        "`frust-render`'s legacy cpu-tier `vello_cpu =0.0.9` pin is missing from Cargo.lock, \
         found {vello_cpu:?}"
    );
    assert_eq!(
        vello_cpu.len(),
        2,
        "exactly two `vello_cpu` identities are sanctioned, found {vello_cpu:?}"
    );
}
