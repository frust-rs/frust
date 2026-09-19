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
//! inside a git work tree. A third off-repo shape is a `cargo vendor` copy
//! landed *inside* a consumer's own git work tree: `is_inside_git_work_tree`
//! reports true (the consumer's repo) and `git ls-files templates` runs
//! cleanly but reports nothing, since the vendored `templates/` tree was
//! never `git add`ed to that outer repo. An empty tracked set alongside a
//! non-empty on-disk one is that shape, not drift, so it is also an
//! `eprintln!` skip rather than a failure.
//!
//! # Ignored strays never reach a scaffold
//!
//! The on-disk walk below filters out anything `git check-ignore` reports —
//! a `.DS_Store`, an editor backup, a `cargo vendor` marker — before
//! diffing against the tracked set. `scaffold::generate`/
//! `generate_design_system` only ever copy the files
//! `template_manifest.json` whitelists, so a stray ignored file sitting
//! next to the template tree can never reach a scaffolded project even
//! though it is physically present on disk; asserting against it here
//! would just make the tripwire noisy for local editor/OS litter it can't
//! actually cause harm from. An untracked file that is *not* ignored (a
//! forgotten `git add`) is left in the diff — that is exactly the failure
//! this test exists to catch.
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
//! walk performed at test time, every run, filtered through `git
//! check-ignore` as above) against `tracked` *first*: that comparison needs
//! no recompile, so it is the always-on guard — it catches an untracked
//! (and non-ignored) file on a bare re-run, which is exactly what the
//! compiled-embed comparison alone can miss. The `embedded == tracked`
//! comparison remains too, as the fresh-build guard that catches embed
//! drift whenever the crate is actually rebuilt (a clean build, `cargo
//! package --verify`, or CI).

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// Runs `git -C <manifest_dir> check-ignore --no-index --stdin`, feeding
/// `paths` one per line, and returns the subset of `paths` git reports as
/// ignored. `--no-index` matches purely against `.gitignore` patterns
/// (never the index), which is the right question here regardless of
/// whether a path happens to be tracked. Exit code 0 (some lines matched)
/// and 1 (none matched — no output, not an error) are both success; `Ok`
/// wraps the ignored subset either way. Any other exit code, or a failure
/// to spawn `git` at all, is `Err`, and the caller falls back to an
/// `eprintln!` skip rather than treating it as "nothing is ignored".
fn ignored_paths(paths: &BTreeSet<String>) -> Result<BTreeSet<String>, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(manifest_dir())
        .arg("check-ignore")
        .arg("--no-index")
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to spawn `git check-ignore`: {err}"))?;

    {
        let stdin = child.stdin.as_mut().expect("stdin was requested as piped");
        for path in paths {
            writeln!(stdin, "{path}")
                .map_err(|err| format!("failed to write to `git check-ignore` stdin: {err}"))?;
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("failed to wait on `git check-ignore`: {err}"))?;

    match output.status.code() {
        Some(0) | Some(1) => {
            let text = String::from_utf8(output.stdout)
                .map_err(|err| format!("`git check-ignore` produced non-UTF-8 output: {err}"))?;
            Ok(text.lines().map(str::to_owned).collect())
        }
        other => Err(format!(
            "`git check-ignore` exited unexpectedly ({other:?}); stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )),
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

    // Fresh walk at test time — no recompile needed.
    let mut on_disk_raw = BTreeSet::new();
    walk_on_disk_paths(
        &manifest_dir(),
        &manifest_dir().join("templates"),
        &mut on_disk_raw,
    );

    // Foreign-checkout skip: an empty tracked set alongside a non-empty
    // on-disk one is a `cargo vendor` copy inside a consumer's own work
    // tree (see the module doc's off-repo section), not drift in this repo.
    if tracked.is_empty() && !on_disk_raw.is_empty() {
        eprintln!(
            "skipping templates_packaged: `git ls-files templates` reported no \
             tracked files while {} on-disk file(s) exist under `templates/` — \
             this looks like a vendored copy inside a foreign work tree, not \
             this repo",
            on_disk_raw.len()
        );
        return;
    }

    // Always-on guard: what is literally on disk right now, filtered
    // through `git check-ignore` so an ignored stray (`.DS_Store`, an
    // editor backup, …) never fails the test — see the module doc's
    // "Ignored strays never reach a scaffold" section. An untracked file
    // that is *not* ignored stays in `on_disk` and is exactly the failure
    // this test exists to catch.
    let on_disk: BTreeSet<String> = match ignored_paths(&on_disk_raw) {
        Ok(ignored) => on_disk_raw.difference(&ignored).cloned().collect(),
        Err(reason) => {
            eprintln!("skipping the git-ignore filter on the on-disk set: {reason}");
            on_disk_raw.clone()
        }
    };

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
}
