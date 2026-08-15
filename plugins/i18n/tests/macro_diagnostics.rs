//! Compile-time behavior of the `locales!` macro: what must compile, and
//! what must fail to compile with which message.
//!
//! `tests/locales_macro.rs` covers what the expansion *does* at runtime;
//! this suite covers the half a runtime test cannot reach — a malformed
//! `.ftl`, a locale directory whose name is not BCP-47, a tree with no
//! fallback locale, and a typo'd key, each of which has to stop the build.
//!
//! # Why the fixtures are staged into the generated project
//!
//! `locales!` resolves its path argument against the invoking crate's
//! `CARGO_MANIFEST_DIR`, and `trybuild` compiles each case inside a project
//! it generates under the workspace target directory — so a case's
//! `CARGO_MANIFEST_DIR` is *that* project, not `plugins/i18n`. Rather than
//! spelling a brittle `../../../..` climb in every case file, the harness
//! copies `tests/fixtures` into the generated project first, so each case
//! reads `fixtures/<tree>` and every diagnostic path in the expected stderr
//! stays machine-independent.
//!
//! # Regenerating the expected output
//!
//! `TRYBUILD=overwrite cargo test -p frust-i18n --no-default-features \
//! --test macro_diagnostics -- --ignored` rewrites the `.stderr` files;
//! review the diff, since these are the diagnostics the macro promises.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

#[test]
#[ignore = "compiles a generated project against this crate's dependency graph in its own \
            target directory; run explicitly with `cargo test -p frust-i18n \
            --no-default-features --test macro_diagnostics -- --ignored`"]
fn locales_macro_diagnostics() {
    stage_fixtures(&project_dir());

    let cases = trybuild::TestCases::new();
    cases.pass("tests/macro/valid.rs");
    cases.compile_fail("tests/macro/malformed_ftl.rs");
    cases.compile_fail("tests/macro/bad_locale_name.rs");
    cases.compile_fail("tests/macro/missing_fallback.rs");
    cases.compile_fail("tests/macro/typo_key.rs");
}

/// The directory `trybuild` generates its project into — mirroring its own
/// `<target_directory>/tests/trybuild/<crate name>` rule, with the target
/// directory read from `cargo metadata` exactly as `trybuild` reads it (so
/// a `CARGO_TARGET_DIR` override moves both together).
fn project_dir() -> PathBuf {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version=1"])
        .output()
        .expect("`cargo metadata` runs");
    assert!(
        output.status.success(),
        "`cargo metadata` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let metadata = String::from_utf8(output.stdout).expect("`cargo metadata` emits UTF-8");
    let target = field(&metadata, "target_directory").expect("metadata names a target directory");
    Path::new(&target)
        .join("tests")
        .join("trybuild")
        .join(env!("CARGO_PKG_NAME"))
}

/// Reads one top-level string field out of `cargo metadata`'s JSON.
///
/// A hand-rolled read rather than a `serde_json` dev-dependency: one flat
/// string field, whose only JSON escapes on any path this repo builds on are
/// `\\` and `\"`.
fn field(json: &str, name: &str) -> Option<String> {
    let start = json.find(&format!("\"{name}\":\""))? + name.len() + 4;
    let mut value = String::new();
    let mut chars = json[start..].chars();
    while let Some(character) = chars.next() {
        match character {
            '"' => return Some(value),
            '\\' => value.push(chars.next()?),
            _ => value.push(character),
        }
    }
    None
}

/// Copies `tests/fixtures` into the generated project, where each case's
/// `locales!("fixtures/…")` resolves it.
///
/// The destination is cleared first, so a renamed or deleted fixture cannot
/// linger from an earlier run and quietly change what a case sees — a
/// locale directory is a directory *name*, so a stale one is exactly the
/// kind of leftover that would flip a diagnostic. The path is asserted
/// before removal because it is derived from `cargo metadata` rather than
/// written out here.
fn stage_fixtures(project: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let staged = project.join("fixtures");
    assert!(
        staged.ends_with("tests/trybuild/frust-i18n/fixtures"),
        "refusing to clear an unexpected staging path: {}",
        staged.display()
    );

    match fs::remove_dir_all(&staged) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("clearing {}: {error}", staged.display()),
    }
    copy_tree(&source, &staged);
}

/// Recursively copies `from` onto `to`, creating directories as needed.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap_or_else(|error| panic!("creating {}: {error}", to.display()));

    let entries =
        fs::read_dir(from).unwrap_or_else(|error| panic!("reading {}: {error}", from.display()));
    for entry in entries {
        let entry = entry.expect("directory entry");
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target)
                .unwrap_or_else(|error| panic!("copying to {}: {error}", target.display()));
        }
    }
}
