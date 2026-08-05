//! Source-scan conformance test pinning the commit-after-skip ordering of the
//! mobile shells' `last_focus_ime_gen` cache (fix `dee29bb`).
//!
//! # The contract
//!
//! Each mobile shell's `app/frame.rs` PEEKS `AppTree::focus_ime_generation()`
//! while gathering that tick's `FrameInputs` (a non-mutating read used to
//! derive `focus_or_ime_changed`), then COMMITS it into
//! `self.last_focus_ime_gen` only after `frame_gate.decide_paced(..).is_skip()`
//! has returned `false` — i.e. only on a frame that actually runs. A commit
//! sited *before* that early return drains the focus/IME edge on a skipped
//! tick: the next tick then compares against the live generation with nothing
//! left to detect, and the edge — which exists specifically to survive a
//! pacing-driven Skip until the next produced frame
//! (`docs/LIMITATIONS.md`'s `focus-ime-edge-paced-deferral`) — is lost
//! outright. That is the round-0 bug `dee29bb` fixed; this test keeps it
//! fixed.
//!
//! # Why a source scan
//!
//! Precedent: `crates/frust/tests/surface_mode_conformance.rs` and
//! `crates/frust/tests/theme_ladder_conformance.rs` (plain `std::fs` source
//! scans run as ordinary `cargo test`s — this repo has no lint-plugin
//! tooling, see `docs/CODE_STANDARDS.md`). The invariant here spans two
//! per-platform `#[cfg(target_os = ..)]` files neither mobile shell's own
//! `cargo test --workspace` run compiles (see `docs/DEVELOPMENT.md`'s Test
//! section), so a regression in the ordering — or a maintainer "restoring"
//! the old drain-at-gather-time shape by following a stale field doc — would
//! otherwise ship through a fully green gate undetected.
//!
//! # What this checks
//!
//! For each of `frust-shell-android/src/app/frame.rs` and
//! `frust-shell-ios/src/app/frame.rs`:
//!
//! 1. Exactly one `self.last_focus_ime_gen = ` assignment exists in the file
//!    (its commit site — the field's *read* sites, e.g.
//!    `!= self.last_focus_ime_gen`, don't match this needle since they have
//!    no trailing `=`).
//! 2. That assignment appears textually AFTER the file's `is_skip()` call
//!    (the `decide_paced(..).is_skip()` early-return gate) — a relative
//!    ordering check, not a pin to either line's absolute number, so
//!    surrounding whitespace/comment churn can't spuriously fail it.
//! 3. The file contains no `mem::replace` touching `last_focus_ime_gen` — the
//!    old drain-on-gather shape this fix replaced.
//!
//! # What this is NOT
//!
//! A substring/line scan, not a parser — comment-only lines are stripped
//! (mirroring `surface_mode_conformance.rs`/`theme_ladder_conformance.rs`),
//! which is correct for this codebase because every real call/assignment in
//! scope sits on its own statement line.

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

/// The two mobile shells' per-tick frame bodies, workspace-relative.
const FRAME_FILES: &[&str] = &[
    "crates/frust-shell-android/src/app/frame.rs",
    "crates/frust-shell-ios/src/app/frame.rs",
];

fn read_frame_file(rel: &str) -> String {
    let path = workspace_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// True if `line`, trimmed, is a comment-only line (mirrors
/// `surface_mode_conformance.rs`'s `is_comment_only`).
fn is_comment_only(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// Real (non-comment) `(1-based line number, trimmed text)` occurrences of
/// `needle` in `src`.
fn hits(src: &str, needle: &str) -> Vec<(usize, String)> {
    src.lines()
        .enumerate()
        .filter(|(_, line)| !is_comment_only(line) && line.contains(needle))
        .map(|(i, line)| (i + 1, line.trim().to_string()))
        .collect()
}

/// The commit site: exactly one real `self.last_focus_ime_gen = ` assignment
/// per file. The trailing space after `=` is deliberate — it excludes the
/// field's *read* sites (`focus_ime_gen != self.last_focus_ime_gen`, which end
/// in `;`/`,`, never `= `) without needing a real parser.
#[test]
fn each_shell_commits_last_focus_ime_gen_exactly_once() {
    for rel in FRAME_FILES {
        let contents = read_frame_file(rel);
        let found = hits(&contents, "last_focus_ime_gen = ");
        assert_eq!(
            found.len(),
            1,
            "{rel} must contain exactly one `self.last_focus_ime_gen = ` commit site — \
             its single generation-cache commit, past the `decide_paced(..).is_skip()` \
             early return. A second commit site (or zero) means this scan can no longer \
             tell the peek from the commit; found {}:\n{}",
            found.len(),
            found
                .iter()
                .map(|(n, l)| format!("  line {n}: {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
}

/// The ordering contract itself: the commit must sit textually after the
/// `is_skip()` early-return check. A commit sited before it drains the
/// focus/IME edge on a tick the gate skips, re-opening the deferred-repaint
/// bug fixed by `dee29bb` (see the module docs above).
#[test]
fn each_shell_commits_last_focus_ime_gen_only_past_the_skip_return() {
    for rel in FRAME_FILES {
        let contents = read_frame_file(rel);

        let skip_hits = hits(&contents, "is_skip()");
        assert_eq!(
            skip_hits.len(),
            1,
            "{rel} must contain exactly one `is_skip()` early-return check — the \
             `decide_paced(..).is_skip()` gate this test orders the commit against; \
             found {}, expected the scan to see exactly one call site",
            skip_hits.len(),
        );
        let (skip_line, skip_text) = &skip_hits[0];

        let commit_hits = hits(&contents, "last_focus_ime_gen = ");
        let (commit_line, commit_text) = commit_hits.first().unwrap_or_else(|| {
            panic!(
                "{rel} has no `self.last_focus_ime_gen = ` commit site to order against \
                 `is_skip()` — see `each_shell_commits_last_focus_ime_gen_exactly_once`"
            )
        });

        assert!(
            commit_line > skip_line,
            "{rel}: the `last_focus_ime_gen` commit (line {commit_line}: `{commit_text}`) \
             must appear AFTER the `is_skip()` early return (line {skip_line}: \
             `{skip_text}`), not before it. A commit sited before the skip return drains \
             the focus/IME edge on a skipped tick and re-opens the deferred-repaint bug \
             `dee29bb` fixed — the edge must survive a Skip so the next tick that actually \
             runs still observes it.",
        );
    }
}

/// The old drain shape this fix replaced: a `mem::replace` (or
/// `mem::take`-style drain) touching `last_focus_ime_gen` at gather time,
/// rather than a plain assignment past the skip return.
#[test]
fn no_shell_drains_last_focus_ime_gen_via_mem_replace() {
    for rel in FRAME_FILES {
        let contents = read_frame_file(rel);
        let found: Vec<(usize, String)> = contents
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                !is_comment_only(line)
                    && line.contains("mem::replace")
                    && line.contains("last_focus_ime_gen")
            })
            .map(|(i, line)| (i + 1, line.trim().to_string()))
            .collect();

        assert!(
            found.is_empty(),
            "{rel} drains `last_focus_ime_gen` via `mem::replace` ({} site(s)) — the old \
             drain-at-gather-time shape `dee29bb` replaced with a peek (a plain read) at \
             gather time and a plain assignment committed only past the `is_skip()` early \
             return. A `mem::replace` here re-opens the same deferred-repaint bug:\n{}",
            found.len(),
            found
                .iter()
                .map(|(n, l)| format!("  line {n}: {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
}
