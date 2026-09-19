//! Packaging tripwire: the template files `frust_drive::scaffold` embeds via
//! `include_dir!`, the files actually on disk under
//! `crates/frust-drive/templates/`, and the files git tracks there must all
//! three be exactly the same set — no more, no less.
//!
//! `cargo package`/`cargo publish` ships only tracked (non-ignored) files.
//! `include_dir!` instead embeds whatever is literally on disk at compile
//! time. Those two views silently diverging is the hazard this test exists
//! to catch: a template file added to the working tree but never `git add`ed
//! (or one that later becomes git-ignored) still gets embedded into a dev
//! build — `frust create` looks fine locally — but is missing from the
//! published crate's source tarball, so a downstream build fails at runtime
//! with `template file `..` missing from embedded template (manifest
//! drift)` (`scaffold::Source::read`) instead of at this test.
//!
//! This is an ordinary T7 (tooling/packaging) unit-tier test — see
//! `docs/TESTING.md`'s Tooling, Plugins, and Packaging section — not a
//! gated/ignored end-to-end build.
//!
//! # Off-repo behavior
//!
//! `frust-drive` itself has no dependency on being inside a git checkout
//! (`Source::Embedded` reads straight from the binary), so this test must
//! never fail a build from a git-less context (a vendored copy, or the
//! published crate itself, which ships without a `.git` directory at all).
//! Both `git` subprocess calls below return early with an `eprintln!` note
//! instead of failing when `git` is absent or `crates/frust-drive` isn't
//! inside a git work tree.
//!
//! # Two guards, two failure windows
//!
//! `embedded_template_paths()` reflects the directory listing
//! `include_dir!` saw the last time `scaffold/mod.rs` was actually
//! recompiled — without the crate's nightly-only `track_path` feature, a
//! file added to `templates/` after that does not itself invalidate Cargo's
//! build fingerprint (no `include_bytes!` call references it yet), so a
//! bare re-run of this test with no other source change can miss drift in
//! the *embedded* set alone. See
//! `frust_drive::scaffold::embedded_template_paths`'s doc comment.
//!
//! That is why this test compares `on_disk` (a fresh `std::fs::read_dir`
//! walk performed at test time, every run) against `tracked` *first*: that
//! comparison needs no recompile, so it is the always-on guard — it catches
//! an untracked or newly ignored file on a bare re-run, which is exactly
//! what the compiled-embed comparison alone can miss. The `embedded ==
//! tracked` comparison remains too, as the fresh-build guard that catches
//! embed drift whenever the crate is actually rebuilt (a clean build,
//! `cargo package --verify`, or CI).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `crates/frust-drive`, resolved from this test binary's own manifest dir
/// so the git subprocess calls below are working-directory-independent.
fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Runs `git -C <manifest_dir> <args>`, returning trimmed stdout lines on a
/// clean exit, or `None` on any failure (missing `git` binary, non-zero
/// exit, non-UTF-8 output) — the single off-repo early-return path every
/// caller below relies on.
fn git_lines(args: &[&str]) -> Option<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(manifest_dir())
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    Some(
        text.lines()
            .map(str::to_owned)
            .filter(|line| !line.is_empty())
            .collect(),
    )
}

fn is_inside_git_work_tree() -> bool {
    git_lines(&["rev-parse", "--is-inside-work-tree"])
        .is_some_and(|lines| lines.first().map(String::as_str) == Some("true"))
}

/// Recursively walks `dir`, collecting every regular file's path (dotfiles
/// included — nothing is skipped) relative to `root`, forward-slash
/// normalized, and prefixed `templates/` to match `git ls-files templates`'s
/// cwd-relative output. Symlinks are not followed specially; `DirEntry`'s
/// own `file_type()` decides file-vs-directory the same way `read_dir`
/// always has.
fn walk_on_disk_paths(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk_on_disk_paths(root, &path, out);
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("walked path is under root")
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            out.insert(relative);
        }
    }
}

/// The set of files actually present on disk under `templates/` right now,
/// `git ls-files templates` and `embedded_template_paths()` must all agree
/// — see the module doc for why the on-disk comparison runs first and needs
/// no recompile to catch drift.
#[test]
fn embedded_template_set_matches_git_tracked_files() {
    if !is_inside_git_work_tree() {
        eprintln!(
            "skipping templates_packaged: `{}` is not inside a git work tree \
             (vendored or published crate) — nothing to diff against",
            manifest_dir().display()
        );
        return;
    }

    let Some(tracked_lines) = git_lines(&["ls-files", "templates"]) else {
        eprintln!(
            "skipping templates_packaged: `git ls-files templates` failed \
             (missing `git`, or an unreadable work tree)"
        );
        return;
    };
    let tracked: BTreeSet<String> = tracked_lines.into_iter().collect();

    // Always-on guard: what is literally on disk right now, walked fresh at
    // test time — no recompile needed, so this alone already catches an
    // untracked file added since the last build.
    let mut on_disk = BTreeSet::new();
    walk_on_disk_paths(
        &manifest_dir(),
        &manifest_dir().join("templates"),
        &mut on_disk,
    );

    let tracked_not_on_disk: Vec<&String> = tracked.difference(&on_disk).collect();
    let on_disk_not_tracked: Vec<&String> = on_disk.difference(&tracked).collect();
    assert!(
        tracked_not_on_disk.is_empty() && on_disk_not_tracked.is_empty(),
        "on-disk template set diverges from `git ls-files templates`:\n\
         \x20\x20tracked but NOT on disk (deleted without `git rm`?): {tracked_not_on_disk:?}\n\
         \x20\x20on disk but NOT tracked (would be dropped by `cargo package`): {on_disk_not_tracked:?}"
    );

    // Fresh-build guard: what `include_dir!` embedded as of the last actual
    // recompile of `scaffold/mod.rs`. Complements the on-disk check above —
    // this one catches embed drift whenever the crate is rebuilt (a clean
    // build, `cargo package --verify`, or CI), even though a bare re-run
    // with no other source change can leave it stale (see the module doc).
    //
    // `git ls-files templates` (run with cwd at this crate's manifest dir)
    // reports paths relative to that cwd, i.e. `templates/app/...` — prefix
    // the embedded set the same way rather than stripping the git output,
    // so every panic message below already reads as a real repo-relative
    // path.
    let embedded: BTreeSet<String> = frust_drive::scaffold::embedded_template_paths()
        .into_iter()
        .map(|path| format!("templates/{path}"))
        .collect();

    let tracked_not_embedded: Vec<&String> = tracked.difference(&embedded).collect();
    let embedded_not_tracked: Vec<&String> = embedded.difference(&tracked).collect();
    assert!(
        tracked_not_embedded.is_empty() && embedded_not_tracked.is_empty(),
        "embedded template set diverges from `git ls-files templates`:\n\
         \x20\x20tracked but NOT embedded (missing from the binary): {tracked_not_embedded:?}\n\
         \x20\x20embedded but NOT tracked (would be dropped by `cargo package`): {embedded_not_tracked:?}"
    );

    // No embedded path may be matched by `git check-ignore`: a file could
    // still `ls-files`-track today while a `.gitignore` pattern added later
    // would silently exclude it from a fresh clone. `check-ignore` exits 0
    // when at least one argument matches an ignore pattern, 1 when none do.
    if embedded.is_empty() {
        return;
    }
    let Ok(check_ignore) = Command::new("git")
        .arg("-C")
        .arg(manifest_dir())
        .arg("check-ignore")
        .args(&embedded)
        .output()
    else {
        eprintln!("skipping the git-ignore check: `git check-ignore` failed to run");
        return;
    };
    match check_ignore.status.code() {
        Some(1) => {} // none of the embedded paths are ignored — good
        Some(0) => {
            let ignored = String::from_utf8_lossy(&check_ignore.stdout);
            panic!(
                "embedded template path(s) are git-ignored (would be dropped by \
                 `cargo package`):\n{ignored}"
            );
        }
        _ => eprintln!(
            "skipping the git-ignore check: `git check-ignore` exited unexpectedly \
             ({:?}); stderr: {}",
            check_ignore.status.code(),
            String::from_utf8_lossy(&check_ignore.stderr)
        ),
    }
}
