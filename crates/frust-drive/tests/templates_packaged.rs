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
//! never `git add`ed to that outer repo. That skip only fires when this
//! crate's own `Cargo.toml` is *also* untracked there (checked separately,
//! next to the crate's real repo shape where `templates/` briefly losing
//! every tracked file while `Cargo.toml` stays tracked would be real drift,
//! not vendoring) — that combination is the vendored shape, not drift, so
//! it is also an `eprintln!` skip rather than a failure.
//!
//! # Ignored strays never reach a scaffold
//!
//! The on-disk walk below filters out anything `git check-ignore` reports —
//! a `.DS_Store`, an editor backup, a `cargo vendor` marker — before
//! diffing against the tracked set, but only when the path is *also*
//! untracked and not listed in either template's `template_manifest.json`:
//! dropped = ignored ∧ untracked ∧ not manifest-listed. A tracked-and-ignored
//! file can therefore never disappear from `on_disk` and present as
//! "tracked but NOT on disk", and a manifest-listed file (one
//! `scaffold::generate`/`generate_design_system` will actually copy into a
//! scaffolded project) can never be silently excused even if it happens to
//! match a `.gitignore` pattern — that combination would mean the file is
//! rendered locally yet missing from the published `.crate`, exactly the
//! drift this test exists to catch. Every path the filter does drop is
//! printed via `eprintln!` so a `.keystore` or similar ignored stray is at
//! least visible in `--nocapture` output. An untracked, non-ignored,
//! non-manifest-listed file (a forgotten `git add`) is left in the diff —
//! that is exactly the failure this test exists to catch.
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

/// Runs `git -C <manifest_dir> check-ignore --no-index -z --stdin`, feeding
/// `paths` NUL-terminated, and returns the subset of `paths` git reports as
/// ignored. `--no-index` matches purely against `.gitignore` patterns
/// (never the index), which is the right question here regardless of
/// whether a path happens to be tracked. `-z` NUL-terminates both the input
/// this function writes and the output it reads back — plain newline
/// framing would silently mis-split (or refuse) any path containing a
/// literal `\n`, which a NUL byte cannot appear in a valid path anyway.
/// Exit code 0 (some lines matched) and 1 (none matched — no output, not an
/// error) are both success; `Ok` wraps the ignored subset either way. Any
/// other exit code, or a failure to spawn `git` at all, is `Err`, and the
/// caller falls back to an `eprintln!` skip rather than treating it as
/// "nothing is ignored".
///
/// The stdin write happens on a spawned thread rather than inline before
/// `wait_with_output`: both the stdin and stdout pipes have bounded OS
/// buffer capacity, and `git check-ignore` can start emitting matches on
/// stdout before it has finished reading all of stdin, so a parent that
/// writes the whole payload synchronously and only then calls
/// `wait_with_output` can deadlock against a child blocked writing to a
/// stdout pipe nobody is draining yet. Draining stdout via
/// `wait_with_output` on this thread while a second thread writes stdin
/// avoids that.
fn ignored_paths(paths: &BTreeSet<String>) -> Result<BTreeSet<String>, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(manifest_dir())
        .arg("check-ignore")
        .arg("--no-index")
        .arg("-z")
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to spawn `git check-ignore`: {err}"))?;

    let mut stdin = child.stdin.take().expect("stdin was requested as piped");
    let payload: Vec<u8> = paths
        .iter()
        .flat_map(|path| path.as_bytes().iter().copied().chain(std::iter::once(0u8)))
        .collect();
    let writer = std::thread::spawn(move || {
        let result = stdin.write_all(&payload);
        // Explicit drop closes the write end so `git` sees EOF once every
        // path has been written, even though the field would drop here
        // anyway at closure exit — spelled out because that EOF is what lets
        // `wait_with_output` on the main thread ever return.
        drop(stdin);
        result
    });

    let output = child
        .wait_with_output()
        .map_err(|err| format!("failed to wait on `git check-ignore`: {err}"))?;
    match writer.join() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            return Err(format!(
                "failed to write to `git check-ignore` stdin: {err}"
            ));
        }
        Err(_) => {
            return Err("`git check-ignore` stdin-writer thread panicked".to_string());
        }
    }

    match output.status.code() {
        Some(0) | Some(1) => {
            let text = String::from_utf8(output.stdout)
                .map_err(|err| format!("`git check-ignore` produced non-UTF-8 output: {err}"))?;
            Ok(text
                .split('\0')
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect())
        }
        other => Err(format!(
            "`git check-ignore` exited unexpectedly ({other:?}); stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )),
    }
}

/// Templates whose `template_manifest.json` [`manifest_listed_paths`] reads
/// to protect a manifest-listed file from the ignore filter below — mirrors
/// the two embedded template roots `scaffold::embedded_template_paths`
/// walks (`EMBEDDED_APP_TEMPLATE`, `EMBEDDED_DESIGN_SYSTEM_TEMPLATE`).
const TEMPLATE_ROOTS: &[&str] = &["app", "design-system"];

/// The manifest filename each template root's `template_manifest.json`
/// lives at, relative to that template's own root — mirrors
/// `scaffold::MANIFEST_FILE`'s value (a private `frust_drive` constant this
/// external test binary cannot reference directly).
const MANIFEST_FILE: &str = "template_manifest.json";

/// Every path listed in each [`TEMPLATE_ROOTS`] template's
/// `template_manifest.json` — a flat JSON array of paths relative to that
/// template's own root (e.g. `.cargo/config.toml`, `src/lib.rs.tmpl`) — each
/// prefixed `templates/<root>/` to match `on_disk`'s path shape. A manifest
/// that is missing or fails to parse as `Vec<String>` simply contributes no
/// entries; that shape is already caught by the on-disk-vs-tracked
/// comparison below, not this helper's job.
fn manifest_listed_paths() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for root in TEMPLATE_ROOTS {
        let manifest_path = manifest_dir()
            .join("templates")
            .join(root)
            .join(MANIFEST_FILE);
        let Ok(contents) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(entries) = serde_json::from_str::<Vec<String>>(&contents) else {
            continue;
        };
        for entry in entries {
            out.insert(format!("templates/{root}/{entry}"));
        }
    }
    out
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

    // Foreign-checkout skip: an empty tracked `templates/` set alone is
    // ambiguous — it is either a `cargo vendor` copy inside a consumer's own
    // work tree (see the module doc's off-repo section) or genuine drift in
    // this repo (every template file lost its git tracking). Distinguish by
    // also checking whether this crate's own `Cargo.toml` is tracked: a
    // vendored copy never `git add`ed its `templates/` tree also never
    // `git add`ed its own manifest, while this repo's real checkout always
    // tracks `Cargo.toml`. Only skip when both are empty; when `Cargo.toml`
    // IS tracked but `templates/` has no tracked files, that is drift — fall
    // through to the assertions below, which will fail loudly.
    if tracked.is_empty() {
        let cargo_toml_tracked = git_lines(&["ls-files", "Cargo.toml"]).unwrap_or_default();
        if cargo_toml_tracked.is_empty() {
            eprintln!(
                "skipping templates_packaged: `git ls-files templates` reported no \
                 tracked files and this crate's own `Cargo.toml` is untracked too \
                 — this looks like a vendored copy inside a foreign work tree, not \
                 this repo"
            );
            return;
        }
    }

    // Always-on guard: what is literally on disk right now, filtered
    // through `git check-ignore` so an ignored stray (`.DS_Store`, an
    // editor backup, …) never fails the test — see the module doc's
    // "Ignored strays never reach a scaffold" section. A path is only
    // dropped from `on_disk` when it is ignored AND untracked AND not
    // listed in either template's `template_manifest.json`: symmetric (a
    // tracked-and-ignored file can never vanish and read as "tracked but
    // NOT on disk") and manifest-aware (a manifest-listed file is exactly
    // what would be rendered into a scaffolded project, so it must never be
    // excused even when a `.gitignore` pattern happens to also match it).
    // Every dropped path is printed so a stray `.keystore` or similar is at
    // least visible under `--nocapture`. An untracked, non-ignored,
    // non-manifest-listed file stays in `on_disk` and is exactly the
    // failure this test exists to catch.
    let manifest_listed = manifest_listed_paths();
    let on_disk: BTreeSet<String> = match ignored_paths(&on_disk_raw) {
        Ok(ignored) => on_disk_raw
            .iter()
            .filter(|path| {
                let drop = ignored.contains(*path)
                    && !tracked.contains(*path)
                    && !manifest_listed.contains(*path);
                if drop {
                    eprintln!("ignored stray under templates/, not considered: {path}");
                }
                !drop
            })
            .cloned()
            .collect(),
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
