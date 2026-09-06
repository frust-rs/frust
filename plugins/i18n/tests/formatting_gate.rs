//! Source-scan conformance: the ICU4X stack stays behind the `formatting`
//! feature, and only `src/fmt` names it.
//!
//! A `formatting`-off build must resolve to the platform-plugin charter line
//! with no `icu_*`/`tinystr` crate anywhere in its tree
//! (`cargo tree -p frust-i18n --no-default-features -e normal`). That
//! property has two halves, and this file checks both cheaply enough to run
//! in the default `cargo test` pass:
//!
//! 1. every ICU4X-stack dependency is declared `optional` and pulled in by
//!    the `formatting` feature alone — the manifest half;
//! 2. no module outside `src/fmt` names one — the source half, which is what
//!    would otherwise force the gate open through a stray `use`.
//!
//! Same shape as `frust-drive`'s `print_free_cores` scan
//! (`docs/CODE_STANDARDS.md`'s Anti-patterns).

use std::fs;
use std::path::{Path, PathBuf};

/// Every crate `src/fmt` reaches ICU4X through — the whole set the
/// `formatting` feature must gate. `icu_provider` is on the list even though
/// no source file names it: it is declared for its `sync` feature alone, and
/// it has to stay just as gated as the rest.
const GATED_CRATES: [&str; 7] = [
    "icu_decimal",
    "icu_datetime",
    "icu_plurals",
    "icu_experimental",
    "icu_locale_core",
    "icu_provider",
    "tinystr",
];

/// The one directory allowed to name a crate from [`GATED_CRATES`].
const GATED_DIR: &str = "fmt";

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn every_icu_dependency_is_optional_and_gated() {
    let manifest = fs::read_to_string(crate_root().join("Cargo.toml")).expect("reads Cargo.toml");
    let declarations: Vec<&str> = manifest
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .collect();

    for name in GATED_CRATES {
        let declaration = declarations
            .iter()
            .find(|line| line.starts_with(&format!("{name} = ")))
            .unwrap_or_else(|| panic!("`{name}` is not declared"));

        assert!(
            declaration.contains("optional = true"),
            "`{name}` must be optional: {declaration}"
        );
        assert!(
            manifest.contains(&format!("\"dep:{name}\"")),
            "`{name}` must be pulled in by the `formatting` feature"
        );
    }
}

#[test]
fn no_module_outside_fmt_names_the_icu_stack() {
    let mut offenders = Vec::new();

    for file in rust_sources(&crate_root().join("src")) {
        if file
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|dir| dir == GATED_DIR)
        {
            continue;
        }

        let source = fs::read_to_string(&file).expect("reads a source file");
        for name in GATED_CRATES {
            if source.contains(&format!("{name}::")) {
                offenders.push(format!("{}: {name}", file.display()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "only `src/{GATED_DIR}` may name the ICU4X stack: {offenders:?}"
    );
}

/// Every `.rs` file under `root`, recursively.
fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let entries = fs::read_dir(root).expect("reads a source directory");

    for entry in entries {
        let path = entry.expect("reads a directory entry").path();
        if path.is_dir() {
            found.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }

    found
}
