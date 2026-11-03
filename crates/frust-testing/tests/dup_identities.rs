//! Guard G6: the render stack resolves exactly ONE identity of `vello_common`
//! and `glifo` — no others.
//!
//! Before p8-06, `Cargo.lock` deliberately resolved two `vello_common`s and
//! two `glifo`s at once:
//!
//! - `vello_common 0.0.9` / `glifo 0.1.1` arrived under `frust-render`'s
//!   feature-gated legacy CPU fallback (`vello_cpu = "=0.0.9"`);
//! - `vello_common 0.2.0` / `glifo 0.3.0` were the ENGINE's own core — reached
//!   through `frust-engine`'s own unconditional dependency on them and
//!   through this crate's `vello_cpu_oracle` (`vello_cpu = "=0.2.0"`) oracle
//!   pin, which is why the oracle rasterizes with the same code the engine
//!   does (see `src/oracle_cpu.rs`).
//!
//! `frust-render`'s `cpu-tier` feature (and its `vello_cpu =0.0.9` pin) has
//! since been retired, so only the 0.2.0/0.3.0 identity remains — this guard
//! is flipped to assert exactly that, per the note this file used to carry:
//! "When the swap phase retires the `=0.0.9` pin, this guard does not get
//! deleted: flip each expectation to the single remaining version, so the
//! file keeps asserting that exactly one identity is left."
//!
//! `docs/DEVELOPMENT.md`'s Version-Pin Policy asks for a `cargo tree -d` check
//! after any manifest change; this test is that check, frozen into the gate so
//! a second identity (or a silent collapse to a *different* version) fails CI
//! rather than waiting for someone to run the command by hand. A plain
//! lockfile scan, run as an ordinary `cargo test` — the same shape, and the
//! same rationale, as this crate's `tests/deps.rs` guard.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The one version sanctioned for each of these packages. A version present
/// here but absent from the lock is as much a failure as an unexpected one:
/// both mean the graph moved.
const SANCTIONED_IDENTITIES: &[(&str, &str)] = &[("vello_common", "0.2.0"), ("glifo", "0.3.0")];

/// The `glifo` version the hvgl atlas route's restated raster maths was last
/// checked against.
///
/// This is an attested acknowledgement, not a measurement: it proves the
/// bumper saw the instruction in the `glifo` failure message, not that the
/// Metal parity test ran. The parity re-run is `cargo test -p frust-engine
/// --test hvgl_atlas_parity -- --ignored --test-threads=1` on macOS Metal, and
/// it is the real evidence. Update this constant only after that run passes.
const HVGL_PARITY_VERIFIED_GLIFO: &str = "0.3.0";

/// Where the hvgl route's restatements of `glifo` live, and what to re-check.
const HVGL_RESTATEMENT_NOTE: &str = "crates/frust-engine/src/text/hvgl_atlas.rs restates glifo's \
     private raster maths (renderer.rs:591 calculate_raster_metrics, :342 render_outline_to_atlas, \
     :521 render_outline_glyph_from_atlas, :311 render_from_atlas). Re-diff those against the new \
     glifo, run `cargo test -p frust-engine --test hvgl_atlas_parity -- --ignored --test-threads=1` \
     on macOS Metal, then update HVGL_PARITY_VERIFIED_GLIFO and the docs/RENDER_DEVELOPMENT.md pin row.";

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
fn sanctioned_identities_are_exactly_as_declared() {
    let packages = locked_packages(&repo_root());
    assert!(
        !packages.is_empty(),
        "Cargo.lock parsed to zero packages — the scan is not reading what it thinks it is"
    );

    for (name, expected) in SANCTIONED_IDENTITIES {
        let found = versions_of(&packages, name);
        let expected: BTreeSet<String> = BTreeSet::from([(*expected).to_string()]);
        let glifo_note = if *name == "glifo" {
            " A glifo move also needs the hvgl parity re-run: see \
             `hvgl_restatement_is_attested_for_the_locked_glifo` for the steps."
        } else {
            ""
        };
        assert_eq!(
            found, expected,
            "`{name}` identities in Cargo.lock drifted: expected {expected:?}, found {found:?}. \
             Exactly one identity of the render core is sanctioned now that the legacy cpu-tier \
             fallback is retired — if this change is intentional, update SANCTIONED_IDENTITIES \
             and say why in the commit message; if it is not, a pin was bumped or a route was \
             added that forks the graph.{glifo_note}"
        );
    }
}

#[test]
fn the_oracle_pin_is_the_engine_side_identity() {
    // The oracle exists to rasterize with the ENGINE's core, so its own
    // `vello_cpu` must be the 0.2.0 one. A `vello_cpu` that ever drifted off
    // 0.2.0 — or resolved a second identity again — would leave the oracle
    // silently comparing against the wrong rasterizer.
    let packages = locked_packages(&repo_root());
    let vello_cpu = versions_of(&packages, "vello_cpu");
    assert_eq!(
        vello_cpu,
        BTreeSet::from(["0.2.0".to_string()]),
        "exactly one `vello_cpu` identity (0.2.0, the CPU oracle's pin) is sanctioned now that \
         the legacy cpu-tier fallback's `vello_cpu =0.0.9` pin is retired, found {vello_cpu:?}"
    );
}

#[test]
fn hvgl_restatement_is_attested_for_the_locked_glifo() {
    // The hvgl route restates glifo's private raster maths, which is only
    // correct for the glifo it was checked against. A glifo move must come with
    // a re-diff and the Metal parity run (see HVGL_RESTATEMENT_NOTE), and then
    // this constant is bumped to acknowledge it.
    let packages = locked_packages(&repo_root());
    let found = versions_of(&packages, "glifo");
    let attested: BTreeSet<String> = BTreeSet::from([HVGL_PARITY_VERIFIED_GLIFO.to_string()]);
    assert_eq!(
        found, attested,
        "glifo moved from {attested:?} to {found:?} in Cargo.lock. {HVGL_RESTATEMENT_NOTE}"
    );
}
