//! Source-scan conformance test enforcing one invariant:
//! ONLY the two shell FFI-glue callers may declare host translucency, and
//! the `frust` facade must never re-export the declaration fn.
//!
//! Also pins the RESOLVED
//! slot's writer (`publish_resolved_surface_mode`, whose sanctioned callers
//! are the two shells' `app.rs` UI-thread sync points), plus the app-facing
//! half's positive checks: the facade DOES re-export the *reader*
//! (`resolved_surface_mode`), and on a host with no shell running it answers
//! `Unknown`.
//!
//! Precedent: `frust-drive/tests/print_free_cores.rs` (a plain `std::fs`
//! source scan run as an ordinary `cargo test` — this repo has no
//! lint-plugin tooling, see `docs/CODE_STANDARDS.md`). Chosen over a doc
//! statement + unit test alone because the whole point of this invariant is
//! a workspace-wide guarantee ("nothing outside these two files may set the
//! latch") that a single crate's unit test can't see across crate
//! boundaries — a cheap scan here catches a future caller added anywhere in
//! the workspace, not just a regression local to `frust-shell-common`.
//!
//! # What this checks
//!
//! 1. No `crates/*/src/**/*.rs` file calls `declare_host_translucent_surface(`
//!    as a real (non-comment) call, except the two sanctioned host-glue
//!    files (`frust-shell-android/src/jni_glue.rs`,
//!    `frust-shell-ios/src/ffi_glue.rs`) and `frust-shell-common`'s own
//!    defining/test module (`surface_mode.rs`).
//! 2. Likewise for `publish_resolved_surface_mode(` — the RESOLVED slot's
//!    writer — whose allowlist is the two shells' `app.rs` (their single
//!    UI-thread resolved-translucency sync point) plus `surface_mode.rs`.
//! 3. `crates/frust/src/lib.rs` (the facade) contains no `pub use` line
//!    naming `declare_host_translucent_surface`, the old
//!    `request_translucent_surface`, or `publish_resolved_surface_mode` —
//!    i.e. the facade re-exports neither writer.
//!
//! # What this is NOT
//!
//! A substring/line scan, not a parser — comment-only lines are stripped
//! (mirroring `print_free_cores.rs`), but this is not a full Rust tokenizer;
//! correct for this codebase because every real call in scope sits on its
//! own statement line.

use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root, resolved from this crate's manifest dir
/// (`crates/frust`) so the scan is working-directory-independent.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/frust has a grandparent (the workspace root)")
        .to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
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

/// Every `crates/*/src` tree's `.rs` files, sorted for a stable failure order.
fn all_crate_source_files() -> Vec<PathBuf> {
    let root = workspace_root();
    let crates_dir = root.join("crates");
    let mut out = Vec::new();
    let entries = fs::read_dir(&crates_dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", crates_dir.display()));
    for entry in entries {
        let crate_dir = entry.unwrap_or_else(|e| panic!("dir entry: {e}")).path();
        let src = crate_dir.join("src");
        if src.is_dir() {
            rust_files(&src, &mut out);
        }
    }
    out.sort();
    out
}

/// `path` relative to the workspace root, forward-slashed, for stable
/// failure messages and allowlist keys independent of host path separators.
fn rel(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Files allowed to call `declare_host_translucent_surface(` — the two
/// sanctioned FFI-glue callers, plus `surface_mode.rs` itself (the
/// definition and its own unit tests).
const ALLOWLIST: &[&str] = &[
    "crates/frust-shell-common/src/surface_mode.rs",
    "crates/frust-shell-android/src/jni_glue.rs",
    "crates/frust-shell-ios/src/ffi_glue.rs",
];

/// Files allowed to call `publish_resolved_surface_mode(` — each
/// mobile shell's `app.rs`, which owns the one UI-thread beat that reads the
/// live surface's resolved translucency and pushes it into the render root,
/// plus `surface_mode.rs` itself (the definition and its own unit tests).
///
/// Deliberately NOT the `jni_glue.rs`/`ffi_glue.rs` install sites: those run
/// render-side in the default split, while app code polls the slot during a
/// UI-thread rebuild — publishing from the install would race the very
/// consumer this slot exists for.
const RESOLVED_PUBLISH_ALLOWLIST: &[&str] = &[
    "crates/frust-shell-common/src/surface_mode.rs",
    "crates/frust-shell-android/src/app.rs",
    "crates/frust-shell-ios/src/app.rs",
];

/// True if `line`, trimmed, is a comment-only line (mirrors
/// `print_free_cores.rs`'s comment-stripping — doc comments naming the fn in
/// prose, e.g. `[\`declare_host_translucent_surface\`]`, must not count as a
/// call site).
fn is_comment_only(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// Every real (non-comment) `<needle>` call site under `crates/*/src` outside
/// `allowlist`, formatted as `path:line: <line>` for the failure message.
fn call_sites_outside(needle: &str, allowlist: &[&str]) -> Vec<String> {
    let mut hits = Vec::new();
    for path in all_crate_source_files() {
        let relp = rel(&path);
        if allowlist.contains(&relp.as_str()) {
            continue;
        }
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for (i, line) in contents.lines().enumerate() {
            if is_comment_only(line) {
                continue;
            }
            if line.contains(needle) {
                hits.push(format!("{relp}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    hits
}

#[test]
fn only_shell_glue_may_declare_host_translucent_surface() {
    let failures = call_sites_outside("declare_host_translucent_surface(", ALLOWLIST);

    assert!(
        failures.is_empty(),
        "host-declared-translucency ban violated ({} hit(s)) — only the generated host's \
         JNI/C-ABI entry points may declare host translucency (review M3); see \
         crates/frust-shell-common/src/surface_mode.rs's module docs:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

/// The RESOLVED slot's writer pin: the RESOLVED slot is shell-published, exactly like
/// the declaration latch is shell-declared. App code reads it and nothing
/// else — a stray publish would let an app fake a platform verdict it never
/// got, the same class of defect as an app declaring host translucency
/// directly.
#[test]
fn only_shell_app_loops_may_publish_the_resolved_surface_mode() {
    let failures = call_sites_outside("publish_resolved_surface_mode(", RESOLVED_PUBLISH_ALLOWLIST);

    assert!(
        failures.is_empty(),
        "resolved-surface-mode publish ban violated ({} hit(s)) — only each mobile shell's \
         own per-frame resolved-translucency sync may publish this slot (native-widgets \
         p1-01); see crates/frust-shell-common/src/surface_mode.rs's module docs:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

#[test]
fn facade_does_not_reexport_the_translucency_declaration() {
    let facade = workspace_root().join("crates/frust/src/lib.rs");
    let contents =
        fs::read_to_string(&facade).unwrap_or_else(|e| panic!("reading {}: {e}", facade.display()));

    // A `pub use` may span several lines as a brace group:
    //
    //     pub use frust_shell_common::{
    //         declare_host_translucent_surface,
    //     };
    //
    // so scanning only lines that *start* with `pub use` would never visit the
    // banned name on a continuation line. Track brace depth from the opening
    // `pub use` until its terminating `;` and scan the whole statement.
    let mut in_pub_use = false;
    let mut depth: i32 = 0;
    for (i, line) in contents.lines().enumerate() {
        let trimmed = line.trim_start();
        if !in_pub_use && !trimmed.starts_with("pub use") {
            continue;
        }
        in_pub_use = true;
        assert!(
            !line.contains("declare_host_translucent_surface")
                && !line.contains("request_translucent_surface")
                && !line.contains("publish_resolved_surface_mode"),
            "crates/frust/src/lib.rs:{}: facade re-exports a surface-mode WRITER — app Rust \
             must never be able to set the declaration latch (review M3) or fake a resolved \
             verdict (p1-01). Line: {}",
            i + 1,
            line.trim()
        );
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if depth <= 0 && line.contains(';') {
            in_pub_use = false;
            depth = 0;
        }
    }
}

/// The multi-line-group blind spot this scan must catch: a grouped
/// `pub use` must be caught on its continuation lines, not just its first.
#[test]
fn facade_reexport_scan_sees_multi_line_groups() {
    let planted = "pub use frust_shell_common::{\n    declare_host_translucent_surface,\n};\n";
    let mut in_pub_use = false;
    let mut depth: i32 = 0;
    let mut visited_banned = false;
    for line in planted.lines() {
        let trimmed = line.trim_start();
        if !in_pub_use && !trimmed.starts_with("pub use") {
            continue;
        }
        in_pub_use = true;
        if line.contains("declare_host_translucent_surface") {
            visited_banned = true;
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if depth <= 0 && line.contains(';') {
            in_pub_use = false;
            depth = 0;
        }
    }
    assert!(
        visited_banned,
        "the scan walk must visit continuation lines of a grouped `pub use`"
    );
}

/// The reader half is app-facing by design: this test failing to
/// *compile* is the real assertion — `frust::resolved_surface_mode` and
/// `frust::ResolvedSurfaceMode` must both be reachable from the facade alone,
/// since app code never depends on `frust-shell-common` directly.
///
/// Acceptance: on a host with no shell running (this test binary), the slot
/// reads `Unknown` — not `Opaque`, which would be a claim about a surface
/// nobody ever created.
#[test]
fn facade_exposes_the_resolved_reader_and_it_starts_unknown() {
    let mode: frust::ResolvedSurfaceMode = frust::resolved_surface_mode();

    assert_eq!(
        mode,
        frust::ResolvedSurfaceMode::Unknown,
        "no shell published in this process, so the resolved slot must still read Unknown"
    );
    assert!(!mode.is_translucent());
    assert!(
        !mode.translucency_refused(),
        "`Unknown` must never be mistaken for a refusal — a fallback branch keyed off it \
         would fire on every desktop-preview run"
    );
}
