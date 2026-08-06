//! Comment-residue conformance ratchet: a source-scan test locking
//! `docs/CODE_STANDARDS.md`'s Comment Conventions § shut so the comment-cleanup
//! pass that swept `plan/phase`, `plan-task`, `FINDINGS #`, and review-round
//! references out of production comments can't quietly regress. Precedent
//! (match structure/error-message style): `crates/frust-drive/tests/
//! print_free_cores.rs` (source-scan mechanics, failure reporting) and
//! `crates/frust/tests/authoring_seam_conformance.rs` (a `frust`-crate test
//! scanning OTHER packages, including `benchmarks/`/`examples/`, by relative
//! path from `CARGO_MANIFEST_DIR`).
//!
//! # What this checks
//!
//! Every `.rs` file under `crates/`, `plugins/`, `benchmarks/`, and
//! `examples/` (resolved relative to the workspace root, [`SCAN_ROOTS`]),
//! skipping any `target/` directory (build output, not source) and any
//! `workflow/` directory (the private, gitignored nested repo — never
//! reachable from these four roots today, but excluded on principle since a
//! future nested checkout must never leak into this scan). Unlike
//! `authoring_seam_conformance.rs`'s consumer scan, this one does NOT skip
//! `tests/`/`benches/` subdirectories or `#[cfg(test)]` blocks — a leftover
//! ledger number in a test comment is exactly as much residue as one in
//! `src/` (confirmed necessary: `examples/huddle/tests/shell.rs` and
//! `plugins/native-widgets/tests/limitation_conformance.rs` both carried real
//! hits during this test's own development).
//!
//! For each production line, the substring from the first `//` (covering
//! `//`, `///`, `//!`) is checked against the banned patterns below; a hit
//! not covered by the allowlist is a failure.
//!
//! # This test's own file is excluded from its own scan
//!
//! Unlike `authoring_seam_conformance.rs` (which never walks `crates/**` at
//! all, so it can't self-match), this scan's roots include `crates/`, and
//! this file lives at `crates/frust/tests/comment_residue_conformance.rs` —
//! squarely inside them. Documenting the banned patterns below necessarily
//! means quoting them (`FINDINGS #43`, `(Phase 9.B step 1)`, ...) in this
//! module's own doc comments and fixtures, which would otherwise trip this
//! scan against itself. [`self_path`] resolves this exact file (the same
//! `CARGO_MANIFEST_DIR`-relative way every other path here is resolved) and
//! [`scan_source_files`] skips it explicitly.
//!
//! # Comment-only extraction (block comments are a stated non-goal)
//!
//! A plain substring scan, not a parser, matching the precedents' own
//! posture. For each line: take the substring from the first `//`; if that
//! `//` sits after an odd number of `"` earlier on the line (a
//! string-literal heuristic — the `//` is presumed to be inside a string,
//! e.g. a URL), the WHOLE line is skipped rather than searching for a later
//! `//`, matching the codebase's real shapes (no line here needs the more
//! precise behavior). `/* */` block comments are never recognized as
//! comment text by this heuristic — a genuine non-goal, not an oversight:
//! this codebase's convention (see `docs/CODE_STANDARDS.md`) is exclusively
//! `//`/`///`/`//!`, and no block comment exists in the scanned trees today.
//!
//! # Banned patterns
//!
//! Mirrors `docs/CODE_STANDARDS.md`'s Comment Conventions § list, at the
//! precision documented per-pattern below (no broader, no narrower than that
//! prose actually supports against the real tree):
//!
//! - **Plan-phase references, dotted or parenthesized** — a letter-suffixed
//!   dotted phase (`Phase 9.B`, `phase 10.E`) is always a plan-phase id in
//!   this codebase's convention (its digit.LETTER shape never denotes a
//!   float) and is banned regardless of case or parens. A pure-float dotted
//!   phase (`phase 0.0`, `phase 5.5`) is banned ONLY when parenthesized —
//!   `docs/CODE_STANDARDS.md`'s own banned example is exactly this shape,
//!   `` `(phase 5.5)` `` — and otherwise kept: an un-parenthesized float is
//!   the animation/oscillator idiom (`` `phase 0.0` ``, dot-rotation code),
//!   sanctioned domain vocabulary per that doc's own carve-out. A bare
//!   integer phase with no dot at all (`Phase 1 of the frame`) is never
//!   banned when un-parenthesized (the renderer-span idiom); parenthesized,
//!   it is banned ONLY when `Phase` is capitalized — narrowed from the
//!   literal `\([Pp]hase[- ][0-9]` case-insensitive form because the real
//!   tree has a genuine, unambiguous domain-vocabulary counterexample:
//!   `crates/frust-widgets/src/cupertino/activity_indicator.rs`'s `` "At
//!   rest (phase 0) spoke 0 is the leading..." `` is an animation rest-state
//!   citation (lowercase `phase`), while the real parenthesized-bare-int
//!   plan-phase citation this scan does flag — `crates/frust-plugin/src/
//!   android.rs`'s `` "(Phase 1 acceptance: ...)" `` — capitalizes `Phase`,
//!   matching the same capitalization convention as the equally real (but
//!   string-literal, not comment, so never scanned either way) `` "(Phase 4
//!   wiring listener attachment, say)" `` in `plugins/native-widgets/tests/
//!   limitation_conformance.rs`. Case is a real, load-bearing signal here,
//!   not an arbitrary cut.
//! - **Plan-task references**: `task-NN`/`task NN` (exactly two digits,
//!   `task[- ][0-9]{2}\b`) and `task gN` (a single digit, `task g[0-9]\b`,
//!   the wave-numbering shorthand this repo's own workflow uses) — both
//!   lowercase `task` only, matching `docs/CODE_STANDARDS.md`'s own
//!   lowercase example (`` `task-12` ``).
//! - **Workflow findings ledger numbers**: `FINDINGS #N` (the ledger's own
//!   all-caps plural spelling) and singular `[Ff]inding #N`.
//! - **Fix/review-round references**: `cfix-N`, `re-review`, `review round
//!   N`, `device-parity-round`.
//! - **Plan requirement numbers**: `req N`, but ONLY when the same line also
//!   contains the substring `phase` (case-insensitive). A bare `req N` is
//!   collision-prone on its own (`req` is a common abbreviation with no
//!   reliable process-reference shape by itself); every real occurrence this
//!   scan exists to catch pairs a requirement number with a phase citation
//!   on the same line (`docs/CODE_STANDARDS.md`'s own example, `` `req 4`
//!   ``, appears exactly this way: `` "(phase 10.B, req 4)" ``). Narrowed
//!   per this test's own charter, not a broadening of the doc's rule.
//!
//! **Deliberately out of scope**: `docs/CODE_STANDARDS.md` also bans
//! "internal PR numbers", but this scan does not attempt to detect a bare
//! `#N` — the real tree carries legitimate external issue citations in that
//! exact shape (`crates/frust-render/src/context.rs`'s `` `#7057` ``,
//! `crates/frust-shell-desktop/src/logger.rs`'s `` `vello#1031` ``) that a
//! precise-enough PR-number pattern has no cheap way to distinguish from an
//! internal one without a maintained allowlist of this repo's own PR range.
//! Left for human review rather than shipping an imprecise pattern.
//!
//! # Allowlist (never flagged, checked before any banned pattern above)
//!
//! - A line containing a backticked `docs/LIMITATIONS.md` stable id (read at
//!   test time via [`limitations_ids`]) — the register's documented purpose.
//! - A line citing a named semantic rule (`R23`, `R44-back`, `R47`, ...) —
//!   naturally never collides with any pattern above (no banned pattern's
//!   shape overlaps `R` + digits), confirmed by this tree's own `R1`/`R8`/
//!   `R18`/`R23`/`R44-back`/`R47` citations, but still checked explicitly
//!   via [`cites_named_rule`] as a defensive allowlist predicate.
//! - A line citing an external-repo SHA in a `rev`/pin context (`` `rev
//!   `910f626`` ``) — same defensive-predicate reasoning.
//! - "Phase N of the frame" (bare, un-parenthesized) and "phase followed by
//!   a float" (un-parenthesized) are allowlisted by construction inside
//!   [`phase_violation`]'s own state machine rather than a separate
//!   predicate — the phase-form parsing (dotted vs. bare, parenthesized vs.
//!   not) has to happen once regardless, and duplicating it as a standalone
//!   predicate would only be able to disagree with itself.
//!
//! # What this is NOT
//!
//! A substring/state-machine scan, not a parser — matching every other
//! source-scan conformance test in this repo. It has no understanding of
//! Rust syntax beyond "does this line have a `//`, and is that `//` inside a
//! string literal by the odd-quote heuristic above" — correct for this
//! codebase because every comment in the scanned trees fits that shape
//! today.

use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root, resolved from this crate's manifest dir
/// (`crates/frust`) so the scan is working-directory-independent — mirrors
/// `authoring_seam_conformance.rs`'s `workspace_root`.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/frust has a grandparent (the workspace root)")
        .to_path_buf()
}

/// This test's own file, resolved the same `CARGO_MANIFEST_DIR`-relative way
/// as every other path in this scan — see the module doc's "This test's own
/// file is excluded from its own scan".
fn self_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/comment_residue_conformance.rs")
}

/// The four workspace-root directories this scan walks, matching the task
/// charter exactly (not, e.g., `templates/`, `docs/`, or the workspace
/// root's own loose files).
const SCAN_ROOTS: &[&str] = &["crates", "plugins", "benchmarks", "examples"];

/// True if `name` (a single path component) is a directory this scan must
/// never walk into: build output, or the private nested `workflow/` repo
/// (see module doc).
fn is_excluded_dir_component(name: &str) -> bool {
    name == "target" || name == "workflow"
}

/// Recursively collects every `.rs` file under `dir` into `out`, skipping any
/// `target`/`workflow` subdirectory at any depth.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("dir entry under {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            let excluded = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_excluded_dir_component);
            if excluded {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Every `.rs` file across the four scan roots, minus this test's own file
/// (see module doc), sorted for a stable failure order. Panics loudly rather
/// than silently scanning zero files if a root is missing, matching
/// `authoring_seam_conformance.rs`'s posture.
fn scan_source_files() -> Vec<PathBuf> {
    let root = workspace_root();
    let me = self_path();
    let mut out = Vec::new();
    for scan_root in SCAN_ROOTS {
        let dir = root.join(scan_root);
        assert!(
            dir.is_dir(),
            "expected scan root {} to exist — has it moved or been renamed without updating \
             SCAN_ROOTS?",
            dir.display()
        );
        let before = out.len();
        rust_files(&dir, &mut out);
        assert!(
            out.len() > before,
            "expected at least one `.rs` file under {} — found none, which is exactly the \
             false-confidence failure mode this assertion exists to catch",
            dir.display()
        );
    }
    out.retain(|p| p != &me);
    out.sort();
    assert!(
        out.len() > 400,
        "expected well over 400 `.rs` files across crates/plugins/benchmarks/examples, found {} \
         — the scan is probably looking in the wrong place",
        out.len()
    );
    out
}

/// `path` relative to the workspace root, forward-slashed, for stable
/// failure messages independent of host path separators.
fn rel(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The comment-text substring of `line` (from its first `//` onward), or
/// `None` if the line has no `//`, or its first `//` sits inside a string
/// literal by the odd-quote heuristic (see module doc).
fn comment_text(line: &str) -> Option<&str> {
    let idx = line.find("//")?;
    let quotes_before = line[..idx].matches('"').count();
    if quotes_before % 2 == 1 {
        return None;
    }
    Some(&line[idx..])
}

/// Every starting byte offset of `needle` in `haystack` (non-overlapping,
/// left to right).
fn find_all(haystack: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel_idx) = haystack.get(from..).and_then(|s| s.find(needle)) {
        let idx = from + rel_idx;
        out.push(idx);
        from = idx + needle.len().max(1);
    }
    out
}

/// The count of leading ASCII-digit chars in `s`.
fn leading_digit_run(s: &str) -> usize {
    s.chars().take_while(char::is_ascii_digit).count()
}

/// True if byte offset `idx` in `s` is preceded by a word boundary (start of
/// string, or a non-alphanumeric, non-`_` byte).
fn word_boundary_before(s: &str, idx: usize) -> bool {
    idx == 0 || {
        let b = s.as_bytes()[idx - 1];
        !(b.is_ascii_alphanumeric() || b == b'_')
    }
}

/// True if `s` (the text immediately following a match) starts at a word
/// boundary — empty, or its first char is non-alphanumeric/non-`_`.
fn word_boundary_after(s: &str) -> bool {
    s.chars()
        .next()
        .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
}

/// True if `comment` contains a banned plan-phase reference (dotted or
/// parenthesized-bare-int form) — see module doc's per-pattern breakdown.
/// Also the sole place the "Phase N of the frame" / un-parenthesized-float
/// allowlist classes are enforced (by simply never matching them).
fn phase_violation(comment: &str) -> bool {
    let lower = comment.to_ascii_lowercase();
    let mut search_from = 0usize;
    while let Some(rel_idx) = lower.get(search_from..).and_then(|s| s.find("phase")) {
        let idx = search_from + rel_idx;
        search_from = idx + 5;

        if !word_boundary_before(comment, idx) {
            continue;
        }
        let capitalized = comment.as_bytes()[idx] == b'P';
        let parenthesized = idx > 0 && comment.as_bytes()[idx - 1] == b'(';

        let after = &comment[idx + 5..];
        let Some(sep) = after.chars().next() else {
            continue;
        };
        if sep != '-' && sep != ' ' {
            continue;
        }
        let rest = &after[sep.len_utf8()..];
        let digit_len = leading_digit_run(rest);
        if digit_len == 0 {
            continue;
        }
        let after_digits = &rest[digit_len..];

        if let Some(suffix) = after_digits.strip_prefix('.') {
            let suffix_len = suffix
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .count();
            if suffix_len == 0 {
                continue;
            }
            let suffix_str = &suffix[..suffix_len];
            let has_letter = suffix_str.chars().any(|c| c.is_ascii_alphabetic());
            if has_letter {
                // Rule A: digit.LETTER dotted phase — always a plan-phase id.
                return true;
            }
            if parenthesized {
                // Rule A2: parenthesized pure-float dotted phase — banned
                // (`docs/CODE_STANDARDS.md`'s own `(phase 5.5)` example).
                return true;
            }
            // Rule A3: un-parenthesized pure float — animation/oscillator
            // idiom, allowlisted. Keep scanning the rest of the line.
        } else if parenthesized && capitalized {
            // Rule B: parenthesized, capitalized, bare-int phase — a
            // plan-phase citation (see module doc's real-tree evidence).
            return true;
        }
        // Rule B2/B3 (un-parenthesized, or parenthesized-lowercase): the
        // renderer-span / animation-rest-state idioms — allowlisted.
    }
    false
}

/// True if `comment` contains `task-NN`/`task NN` (exactly two digits) or
/// `task gN` (exactly one digit) — both lowercase `task` only.
fn task_violation(comment: &str) -> bool {
    let two_digit = find_all(comment, "task").into_iter().any(|idx| {
        if !word_boundary_before(comment, idx) {
            return false;
        }
        let after = &comment[idx + 4..];
        let Some(sep) = after.chars().next() else {
            return false;
        };
        if sep != '-' && sep != ' ' {
            return false;
        }
        let rest = &after[sep.len_utf8()..];
        leading_digit_run(rest) == 2 && word_boundary_after(&rest[2..])
    });
    let wave_shorthand = find_all(comment, "task g").into_iter().any(|idx| {
        if !word_boundary_before(comment, idx) {
            return false;
        }
        let after = &comment[idx + 6..];
        leading_digit_run(after) == 1 && word_boundary_after(&after[1..])
    });
    two_digit || wave_shorthand
}

/// True if `haystack` contains `needle` immediately followed by one or more
/// ASCII digits.
fn contains_then_digit(haystack: &str, needle: &str) -> bool {
    find_all(haystack, needle)
        .into_iter()
        .any(|idx| leading_digit_run(&haystack[idx + needle.len()..]) > 0)
}

/// True if `comment` contains `FINDINGS #N` (all-caps plural) or
/// `[Ff]inding #N` (singular, case-insensitive leading letter).
fn findings_violation(comment: &str) -> bool {
    contains_then_digit(comment, "FINDINGS #") || {
        find_all(comment, "inding #").into_iter().any(|idx| {
            idx > 0
                && matches!(comment.as_bytes()[idx - 1], b'F' | b'f')
                && leading_digit_run(&comment[idx + "inding #".len()..]) > 0
        })
    }
}

/// True if `comment` contains `req N` (word-boundary-bounded) AND also
/// contains `phase` (case-insensitive) anywhere on the same line — the
/// narrowed `req` rule (see module doc).
fn req_with_phase_violation(comment: &str) -> bool {
    let has_req = find_all(comment, "req ").into_iter().any(|idx| {
        word_boundary_before(comment, idx) && leading_digit_run(&comment[idx + 4..]) > 0
    });
    has_req && comment.to_ascii_lowercase().contains("phase")
}

/// The single entry point combining every banned-pattern check above; `Some`
/// carries a short human-readable label for the failure message.
fn banned_reason(comment: &str) -> Option<&'static str> {
    if phase_violation(comment) {
        return Some(
            "plan-phase reference (dotted or parenthesized — CODE_STANDARDS's `Phase 9.B step \
             1`/`(phase 5.5)` examples)",
        );
    }
    if task_violation(comment) {
        return Some("plan-task reference (`task-12`/`task g3`)");
    }
    if findings_violation(comment) {
        return Some("workflow findings ledger number (`FINDINGS #43`)");
    }
    if contains_then_digit(comment, "cfix-") {
        return Some("fix-round reference (`cfix-2`)");
    }
    if comment.contains("re-review") {
        return Some("review-round reference (`re-review`)");
    }
    if contains_then_digit(comment, "review round ") {
        return Some("review-round reference (`review round N`)");
    }
    if comment.contains("device-parity-round") {
        return Some("device-parity-round reference");
    }
    if req_with_phase_violation(comment) {
        return Some("plan requirement number (`req N`) alongside a phase reference");
    }
    None
}

/// Every `docs/LIMITATIONS.md` stable id (the text inside a `` ### `id` ``
/// heading's first backtick-delimited span), read at test time so a comment
/// citing one is recognized without hand-syncing a list here.
fn limitations_ids() -> Vec<String> {
    let path = workspace_root().join("docs/LIMITATIONS.md");
    let contents =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    contents
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("### ")?.trim_start();
            let rest = rest.strip_prefix('`')?;
            let end = rest.find('`')?;
            Some(rest[..end].to_string())
        })
        .collect()
}

/// True if `comment` contains a backticked citation of one of `ids`
/// (`docs/LIMITATIONS.md`'s stable ids — the register's documented purpose).
fn cites_limitations_id(comment: &str, ids: &[String]) -> bool {
    ids.iter().any(|id| comment.contains(&format!("`{id}`")))
}

/// True if `comment` cites a named semantic rule (`R23`, `R44-back`, `R47`,
/// ...): a word-boundary `R` immediately followed by a digit.
fn cites_named_rule(comment: &str) -> bool {
    let bytes = comment.as_bytes();
    (0..bytes.len()).any(|i| {
        bytes[i] == b'R'
            && word_boundary_before(comment, i)
            && comment[i + 1..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
    })
}

/// True if `comment` cites an external-repo SHA in a `rev`/pin context: `rev
/// ` (optionally through a quote/backtick) followed by a 6–40 char hex run.
fn cites_external_rev(comment: &str) -> bool {
    let lower = comment.to_ascii_lowercase();
    find_all(&lower, "rev ").into_iter().any(|idx| {
        let after = comment[idx + 4..].trim_start_matches(['`', '"', '\'']);
        let hex_len = after.chars().take_while(char::is_ascii_hexdigit).count();
        (6..=40).contains(&hex_len)
    })
}

/// True if `comment` matches one of the "Sanctioned citations"/domain-
/// vocabulary KEEP classes this scan must never flag — checked before
/// [`banned_reason`] runs (see module doc's Allowlist section; the
/// phase-specific allowlist classes are handled inside [`phase_violation`]
/// itself instead of here).
fn is_allowlisted(comment: &str, limitation_ids: &[String]) -> bool {
    cites_limitations_id(comment, limitation_ids)
        || cites_named_rule(comment)
        || cites_external_rev(comment)
}

/// The full per-comment decision this scan makes: `None` if `comment` is
/// clean (either no banned pattern matched, or it matched but is
/// allowlisted), `Some(reason)` if it is banned residue.
fn scan_comment(comment: &str, limitation_ids: &[String]) -> Option<&'static str> {
    if is_allowlisted(comment, limitation_ids) {
        return None;
    }
    banned_reason(comment)
}

/// All violations found in `contents` (one file's full text), formatted
/// `path:line: <reason> — see docs/CODE_STANDARDS.md § Comment Conventions.
/// Line: <text>` — matching `print_free_cores.rs`'s reporting style.
fn violations_in(path: &Path, contents: &str, limitation_ids: &[String]) -> Vec<String> {
    let mut failures = Vec::new();
    for (i, line) in contents.lines().enumerate() {
        let Some(comment) = comment_text(line) else {
            continue;
        };
        let Some(reason) = scan_comment(comment, limitation_ids) else {
            continue;
        };
        failures.push(format!(
            "{}:{}: {reason} — see docs/CODE_STANDARDS.md § Comment Conventions. Line: {}",
            rel(path),
            i + 1,
            line.trim(),
        ));
    }
    failures
}

#[test]
fn production_and_test_sources_are_comment_residue_free() {
    let limitation_ids = limitations_ids();
    let mut failures = Vec::new();
    for path in scan_source_files() {
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        failures.extend(violations_in(&path, &contents, &limitation_ids));
    }
    assert!(
        failures.is_empty(),
        "comment-residue ban violated ({} hit(s)) — see docs/CODE_STANDARDS.md's Comment \
         Conventions §:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

/// Guards [`limitations_ids`] against a silently-empty parse (the
/// false-confidence failure mode every other scan in this repo also guards
/// against).
#[test]
fn limitations_id_extraction_finds_the_known_ids() {
    let ids = limitations_ids();
    assert!(
        ids.len() > 5,
        "expected several docs/LIMITATIONS.md ids, found {}",
        ids.len()
    );
    assert!(
        ids.iter().any(|id| id == "cam-blit-opaque"),
        "expected `cam-blit-opaque` among the parsed ids, got {ids:?}"
    );
}

/// The exact positive/negative exemplar strings this test's task charter
/// specifies, run straight through [`scan_comment`] (as if each were already
/// the extracted comment-text substring — this is what a real `//`-prefixed
/// line reduces to before any banned-pattern check runs).
#[test]
fn scanner_flags_and_allows_the_documented_exemplars() {
    let limitation_ids = limitations_ids();

    let keep = [
        "Phase 1 of the frame — the **encode** span",
        "At clock phase 0.0, dot 0 …",
        "on the OP9",
        "`cam-blit-opaque`",
        "R44-back",
        "clean-signals rev `910f626`",
    ];
    for example in keep {
        assert_eq!(
            scan_comment(example, &limitation_ids),
            None,
            "expected KEEP exemplar {example:?} to pass"
        );
    }

    let strip = [
        "(Phase 9.B step 1)",
        "the task-12 constants",
        "FINDINGS #43",
        "re-review round 1",
        "(phase 10.B, req 4)",
    ];
    for example in strip {
        assert!(
            scan_comment(example, &limitation_ids).is_some(),
            "expected STRIP exemplar {example:?} to fail"
        );
    }
}

/// Fixture-driven unit tests for the scanner's individual pattern decisions —
/// the tricky dotted-vs-bare / parenthesized-vs-not / case distinctions this
/// module's doc comment documents as deliberate narrowing.
#[cfg(test)]
mod scan_behavior {
    use super::*;

    #[test]
    fn dotted_letter_suffix_phase_is_always_banned() {
        assert!(phase_violation("phase 9.B"), "unparenthesized, no caps");
        assert!(
            phase_violation("(Phase 9.B step 1)"),
            "parenthesized, capitalized"
        );
        assert!(
            phase_violation("see phase 10.E for context"),
            "mid-sentence, lowercase"
        );
    }

    #[test]
    fn dotted_pure_float_phase_is_banned_only_when_parenthesized() {
        assert!(
            !phase_violation("at clock phase 0.0, dot 0 is at local phase 0.0"),
            "un-parenthesized float is the animation/oscillator idiom"
        );
        assert!(
            !phase_violation("at phase 1.0 it has traveled fully past the right edge"),
            "un-parenthesized float, second real-tree shape"
        );
        assert!(
            phase_violation("(phase 5.5)"),
            "parenthesized float is CODE_STANDARDS's own banned example"
        );
    }

    #[test]
    fn bare_int_phase_is_banned_only_when_parenthesized_and_capitalized() {
        assert!(
            !phase_violation("Phase 1 of the frame — the encode span"),
            "un-parenthesized bare int is the renderer-span idiom"
        );
        assert!(
            !phase_violation("At rest (phase 0) spoke 0 is the leading one"),
            "parenthesized but lowercase is the real animation-rest-state idiom \
             (activity_indicator.rs)"
        );
        assert!(
            phase_violation("(Phase 1 acceptance: pre-init is a typed error, never a panic)"),
            "parenthesized AND capitalized is a real plan-phase citation (frust-plugin/android.rs)"
        );
        assert!(
            phase_violation(
                "mid-way through making a claim FALSE (Phase 4 wiring listener \
                              attachment, say)"
            ),
            "same shape, second real-tree site (limitation_conformance.rs)"
        );
    }

    #[test]
    fn task_nn_requires_exactly_two_digits_and_a_boundary() {
        assert!(task_violation("the task-12 constants"));
        assert!(task_violation("(task 06) the semantics rule"));
        assert!(!task_violation("task-123 has three digits, not two"));
        assert!(!task_violation("task-1 has one digit, not two"));
        assert!(
            !task_violation("tasked with something"),
            "must be a word boundary before `task`"
        );
    }

    #[test]
    fn task_g_requires_exactly_one_digit() {
        assert!(task_violation("wave task g3"));
        assert!(
            !task_violation("wave task g10"),
            "two digits doesn't match `task g[0-9]\\b`"
        );
    }

    #[test]
    fn findings_matches_plural_allcaps_and_singular_any_case() {
        assert!(findings_violation("FINDINGS #43"));
        assert!(findings_violation("Finding #44"));
        assert!(findings_violation("finding #45"));
        assert!(!findings_violation("no ledger reference here"));
    }

    #[test]
    fn req_only_fires_alongside_a_phase_reference() {
        assert!(req_with_phase_violation("(phase 10.B, req 4)"));
        assert!(!req_with_phase_violation(
            "req 4 alone on its own line, nothing else"
        ));
    }

    #[test]
    fn cfix_re_review_and_review_round_and_device_parity_round() {
        assert!(banned_reason("cfix-2").is_some());
        assert!(banned_reason("re-review round 1").is_some());
        assert!(banned_reason("review round 1").is_some());
        assert!(banned_reason("device-parity-round").is_some());
        assert!(banned_reason("nothing banned here").is_none());
    }

    #[test]
    fn allowlist_predicates() {
        let ids = vec!["cam-blit-opaque".to_string()];
        assert!(cites_limitations_id(
            "see `cam-blit-opaque` for the known gap",
            &ids
        ));
        assert!(!cites_limitations_id("no citation here", &ids));

        assert!(cites_named_rule("R44-back"));
        assert!(cites_named_rule("named rule R23"));
        assert!(!cites_named_rule("no rule cited"));

        assert!(cites_external_rev("clean-signals rev `910f626`"));
        assert!(cites_external_rev("pinned to rev 910f626abc"));
        assert!(!cites_external_rev("no rev cited"));
    }

    #[test]
    fn comment_text_extraction_and_string_literal_heuristic() {
        assert_eq!(comment_text("let x = 5;"), None, "no `//` at all");
        assert_eq!(comment_text("// a real comment"), Some("// a real comment"));
        assert_eq!(
            comment_text("    /// a doc comment"),
            Some("/// a doc comment")
        );
        assert_eq!(
            comment_text("    //! a module doc comment"),
            Some("//! a module doc comment")
        );
        assert_eq!(
            comment_text("let url = \"https://example.com\";"),
            None,
            "the first `//` sits inside a string literal (odd quote count before it)"
        );
        assert_eq!(
            comment_text("let x = 5; // trailing comment"),
            Some("// trailing comment")
        );
    }
}
