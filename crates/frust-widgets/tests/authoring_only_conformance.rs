//! Source-scan conformance test enforcing the "authoring-only" contract for
//! the three built-in widget catalogs (`glyph`, `material`, `cupertino`): each
//! may reach the crate's container/callback plumbing ONLY through the public
//! `crate::authoring::` path.
//!
//! # Background
//!
//! `frust_widgets::authoring` promotes 13 items (`build_child`/
//! `rebuild_child`/`teardown_child`/`rebuild_children`, `route_event`/
//! `route_event_single`, `erase_callback`/`erase_callback_arg`,
//! `ErasedCallback`/`ErasedArgCallback`/`TypedArgCallback`, `ThemeTextColor`,
//! `PRESSED_OPACITY`) into a real, documented public API — the seam a design
//! system authored *outside* this crate is proven sufficient against, by the
//! fact that the three built-in catalogs themselves consume exactly that
//! surface (`docs/ARCHITECTURE.md`'s `frust-widgets` row). There is no longer
//! a crate-root re-export of these names (see `crates/frust-widgets/src/
//! lib.rs`'s module docs — the old `pub(crate) use authoring::*;` compatibility
//! shim is gone), so a catalog file spelling one as a bare `crate::<item>`
//! fails to *compile*. This scan exists for the bypass a plain compile can't
//! catch: a reach through a private module's own internal path that never
//! depended on that shim in the first place (`crate::text::ThemeTextColor`
//! instead of `crate::authoring::ThemeTextColor`), and a reach into
//! `authoring`'s one `pub(crate)` item (`cancel_pod`) that resolves fine
//! in-crate but was never meant for the catalogs.
//!
//! Precedent: `crates/frust/tests/surface_mode_conformance.rs` — a plain
//! `std::fs` source scan run as an ordinary `cargo test` (this repo has no
//! lint-plugin tooling, `docs/CODE_STANDARDS.md`).
//!
//! # What this checks
//!
//! 1. No file under `crates/frust-widgets/src/{glyph,material,cupertino}/`
//!    spells any of the 13 promoted items as a bare `crate::<item>` — only
//!    `crate::authoring::<item>` is allowed.
//! 2. No such file spells `ThemeTextColor` via the private-`text`-module
//!    bypass path `crate::text::ThemeTextColor` — `crate::authoring::
//!    ThemeTextColor` is the one sanctioned spelling.
//! 3. No such file reaches `authoring`'s crate-private `cancel_pod` (used by
//!    `motion`/`nav`, never the catalogs — see `authoring.rs`'s module docs).
//!
//! # What this is NOT
//!
//! A substring/line scan, not a parser — comment-only lines are stripped
//! (mirroring `print_free_cores.rs`/`surface_mode_conformance.rs`); correct
//! for this codebase because every real reference in scope sits on its own
//! statement line.

use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root, resolved from this crate's manifest dir
/// (`crates/frust-widgets`) so the scan is working-directory-independent.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/frust-widgets has a grandparent (the workspace root)")
        .to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("dir entry under {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Every `.rs` file under one of the three built-in catalog directories,
/// sorted for a stable failure order. Panics loudly — rather than silently
/// scanning zero files — if a catalog directory has moved or been renamed, or
/// if the total looks implausibly small for three widget catalogs.
fn catalog_source_files() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut out = Vec::new();
    for catalog in ["glyph", "material", "cupertino"] {
        let dir = root.join("crates/frust-widgets/src").join(catalog);
        assert!(
            dir.is_dir(),
            "expected catalog directory {} to exist — has it moved or been renamed? This test \
             must fail loudly here, not silently pass over zero files.",
            dir.display()
        );
        rust_files(&dir, &mut out);
    }
    out.sort();
    assert!(
        out.len() > 30,
        "expected well over 30 catalog source files across glyph+material+cupertino, found {} \
         — the scan is probably looking in the wrong place",
        out.len()
    );
    out
}

/// `path` relative to the workspace root, forward-slashed, for stable failure
/// messages independent of host path separators.
fn rel(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// True if `line`, trimmed, is a comment-only line (mirrors
/// `surface_mode_conformance.rs`/`print_free_cores.rs`'s comment stripping).
fn is_comment_only(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// The 13 items `frust_widgets::authoring` promoted to a real public API —
/// see the module docs. A catalog file may reference each ONLY as
/// `crate::authoring::<item>`; a bare `crate::<item>` is exactly what the
/// removed `pub(crate) use authoring::*;` crate-root glob used to silently
/// resolve.
const PROMOTED_ITEMS: &[&str] = &[
    "build_child",
    "rebuild_child",
    "teardown_child",
    "rebuild_children",
    "route_event",
    "route_event_single",
    "erase_callback",
    "erase_callback_arg",
    "ErasedCallback",
    "ErasedArgCallback",
    "TypedArgCallback",
    "ThemeTextColor",
    "PRESSED_OPACITY",
];

/// True if `line` contains `crate::<needle>` as a genuine identifier
/// reference — bounded by a non-identifier character (or end of line) right
/// after the match, so a search for `erase_callback` doesn't false-positive
/// on `crate::erase_callback_arg`. (`crate::authoring::<needle>` can never
/// match here: the literal text right after `crate::` in that spelling is
/// `authoring`, not `needle`, so the substring search below simply never
/// finds it — no separate allowlist branch is needed.)
fn contains_bare_crate_reference(line: &str, needle: &str) -> bool {
    let full = format!("crate::{needle}");
    let mut search_from = 0;
    while let Some(found_at) = line[search_from..].find(full.as_str()) {
        let idx = search_from + found_at;
        let after = line[idx + full.len()..].chars().next();
        let boundary_ok = after.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if boundary_ok {
            return true;
        }
        search_from = idx + full.len();
    }
    false
}

/// Every non-comment line across the three catalogs containing `needle` as a
/// plain substring, formatted `path:line: <line>` for a failure message.
fn plain_substring_hits(needle: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for path in catalog_source_files() {
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for (i, line) in contents.lines().enumerate() {
            if is_comment_only(line) {
                continue;
            }
            if line.contains(needle) {
                hits.push(format!("{}:{}: {}", rel(&path), i + 1, line.trim()));
            }
        }
    }
    hits
}

#[test]
fn catalogs_never_bypass_authoring_for_a_promoted_item() {
    let mut failures = Vec::new();
    for path in catalog_source_files() {
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for (i, line) in contents.lines().enumerate() {
            if is_comment_only(line) {
                continue;
            }
            for item in PROMOTED_ITEMS {
                if contains_bare_crate_reference(line, item) {
                    failures.push(format!("{}:{}: {}", rel(&path), i + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "a built-in catalog reached a promoted authoring item without going through \
         `crate::authoring::` ({} hit(s)) — see crates/frust-widgets/src/authoring.rs's module \
         docs and this test's own module docs:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

#[test]
fn catalogs_never_bypass_authoring_via_the_private_text_module() {
    let failures = plain_substring_hits("crate::text::ThemeTextColor");
    assert!(
        failures.is_empty(),
        "a built-in catalog reached `ThemeTextColor` via the private `text` module's own path \
         instead of `crate::authoring::ThemeTextColor` ({} hit(s)) — this path never depended on \
         the removed crate-root glob, so a plain compile can't catch its reintroduction:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

#[test]
fn catalogs_never_reach_authorings_crate_private_cancel_pod() {
    let failures = plain_substring_hits("cancel_pod");
    assert!(
        failures.is_empty(),
        "a built-in catalog referenced `cancel_pod` ({} hit(s)) — it is `pub(crate)` inside \
         `authoring`, reachable in-crate but deliberately NOT part of the public authoring \
         surface a design system outside this crate can build on (only `motion`/`nav` use it, \
         see `authoring.rs`'s module docs):\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

/// The boundary-check blind spot this scan must avoid: a search for
/// `erase_callback` must not false-positive on `crate::erase_callback_arg`.
#[test]
fn boundary_check_does_not_false_positive_on_a_longer_identifier() {
    let line = "    on_toggle: crate::erase_callback_arg(&self.on_toggle),";
    assert!(
        !contains_bare_crate_reference(line, "erase_callback"),
        "a scan for `erase_callback` must not flag `crate::erase_callback_arg` — they are \
         distinct identifiers"
    );
    assert!(
        contains_bare_crate_reference(line, "erase_callback_arg"),
        "the exact identifier `erase_callback_arg` must still be detected"
    );
}

/// The positive-detection case this scan exists for: a bare `crate::<item>`
/// reference (the shape the removed crate-root glob used to silently
/// resolve) must be flagged, while the same item spelled through
/// `crate::authoring::` must not.
#[test]
fn boundary_check_flags_bare_crate_but_not_the_authoring_path() {
    assert!(
        contains_bare_crate_reference("let pod = crate::build_child(&view, ctx);", "build_child"),
        "a bare `crate::build_child` reference must be flagged"
    );
    assert!(
        !contains_bare_crate_reference(
            "let pod = crate::authoring::build_child(&view, ctx);",
            "build_child"
        ),
        "`crate::authoring::build_child` must NOT be flagged — it is the sanctioned path"
    );
}
