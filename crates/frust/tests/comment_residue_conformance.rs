//! Comment-residue conformance ratchet: a source-scan test locking
//! `docs/CODE_STANDARDS.md`'s Comment Conventions § shut so the comment-cleanup
//! pass that swept `plan/phase`, `plan-task`, plan-document, `FINDINGS #`,
//! review-finding, and review-round references out of production comments
//! can't quietly regress. Precedent (match structure/error-message style):
//! `crates/frust-drive/tests/print_free_cores.rs` (source-scan mechanics,
//! failure reporting) and `crates/frust/tests/authoring_seam_conformance.rs`
//! (a `frust`-crate test scanning OTHER packages, including `benchmarks/`/
//! `examples/`, by relative path from `CARGO_MANIFEST_DIR`).
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
//! `//`, `///`, `//!`) is checked against the banned patterns below; every
//! banned match is a failure unless that *match's own byte span* sits inside
//! a sanctioned citation (see Allowlist).
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
//! precise behavior). String literals are therefore invisible to this scan by
//! construction: a banned-looking phrase inside quoted UI copy (e.g.
//! `examples/glyph-catalog/src/pages/camera.rs`'s on-screen caption) is text
//! the program *displays*, not a comment, and is never rewritten by this
//! ratchet. `/* */` block comments are likewise never recognized as comment
//! text — a genuine non-goal, not an oversight: this codebase's convention
//! (see `docs/CODE_STANDARDS.md`) is exclusively `//`/`///`/`//!`, and no
//! block comment exists in the scanned trees today.
//!
//! # Banned patterns
//!
//! Mirrors `docs/CODE_STANDARDS.md`'s Comment Conventions § list, at the
//! precision documented per-pattern below (no broader, no narrower than that
//! prose actually supports against the real tree):
//!
//! - **Plan-phase references, dotted, hyphenated, or parenthesized** — four
//!   sub-rules, because `phase` is also live domain vocabulary here:
//!   - A letter-suffixed dotted phase (`Phase 9.B`, `phase 10.E`) is always a
//!     plan-phase id in this codebase's convention (its digit.LETTER shape
//!     never denotes a float) and is banned regardless of case or parens.
//!   - A pure-float dotted phase (`phase 0.0`, `phase 5.5`) is banned ONLY
//!     when parenthesized — `docs/CODE_STANDARDS.md`'s own banned example is
//!     exactly this shape, `` `(phase 5.5)` `` — and otherwise kept: an
//!     un-parenthesized float is the animation/oscillator idiom (`` `phase
//!     0.0` ``, dot-rotation code), sanctioned domain vocabulary per that
//!     doc's own carve-out.
//!   - A **hyphenated** bare-int phase (`Phase-4`, `phase-7`) is banned in
//!     any case and any position. The hyphen is this repo's plan-label
//!     spelling and nothing else: every hyphenated occurrence in the scanned
//!     trees was a plan citation (`examples/glyph-catalog`'s "the Phase-4
//!     gate numbers", `crates/frust-shell-common/src/frame_gate.rs`'s "the
//!     phase-7 conservative default"), while every legitimate domain use
//!     spells it with a space.
//!   - A **space-separated** bare-int phase (`Phase 1`, `phase 0`) is banned
//!     only with a second signal: parenthesized AND capitalized (`(Phase 1
//!     acceptance: ...)`, `crates/frust-plugin/src/android.rs`), or
//!     capitalized and preceded by the definite article ("the Phase 4 gate"
//!     — a *label applied to* something, never an enumeration). Bare
//!     un-parenthesized `Phase N` is otherwise KEPT, because the real tree
//!     proves it domain vocabulary in three independent places:
//!     `crates/frust-render/src/renderer.rs`'s "Phase 1 of the frame"
//!     encode/present spans, `crates/frust-shell-common/tests/
//!     pacing_integration.rs`'s "Phase 1 (250ms)" scenario stages (a doc
//!     comment numbering its own steps — explicitly sanctioned by
//!     `docs/CODE_STANDARDS.md`), and
//!     `crates/frust-widgets/src/material/loading_indicator.rs`'s "Phase 0 →
//!     t exactly 0" animation phase. Case is likewise load-bearing:
//!     `crates/frust-widgets/src/cupertino/activity_indicator.rs`'s `` "At
//!     rest (phase 0) spoke 0 is the leading..." `` is an animation
//!     rest-state citation (lowercase `phase`) that the parenthesized rule
//!     must not flag.
//! - **Plan-task references**: `task-NN`/`task NN` (exactly two digits,
//!   `task[- ][0-9]{2}\b`) and `task gN` (a single digit, `task g[0-9]\b`,
//!   the wave-numbering shorthand this repo's own workflow uses) — both
//!   lowercase `task` only, matching `docs/CODE_STANDARDS.md`'s own
//!   lowercase example (`` `task-12` ``).
//! - **Plan-document references**: `PLAN <tag>`/`Plan <tag>` where `<tag>` is
//!   a plan-document tag — an uppercase letter, one or more digits, optional
//!   lowercase suffix (`D6a`, `D5`, `D4`), word-bounded — including the
//!   `Plan D6a §Requirements` section form. The tag shape is what makes this
//!   precise: `Plan 9`, "we plan to", and a bare `Plan B` never match. Plus
//!   any literal `workflow/plans/` path, which is a dangling reference for
//!   every reader without the private workflow repo.
//! - **Workflow findings ledger numbers**: `FINDINGS #N` (the ledger's own
//!   all-caps plural spelling) and singular `[Ff]inding #N`.
//! - **Review-finding citations**: `review finding` (any case) when it either
//!   sits directly after a `(` — the parenthesized citation form, whose id
//!   often wraps onto the next line (`crates/frust-shell-android/src/app/
//!   executor.rs`) — or is directly followed by a finding id (an uppercase
//!   letter plus digits: `review finding F5`, `Review finding M1`). Prose
//!   *about* review findings with neither marker ("this becomes a compile
//!   error rather than a review finding", `crates/frust-widgets/src/nav/
//!   route.rs`) is not a citation and is kept.
//! - **Fix/review-round references**: `cfix-N`, `re-review`, `review round
//!   N`, `device-parity-round`, and the `round-N` label — the last one
//!   narrowed to a hyphen plus digits at both word boundaries (so
//!   `round-trip`, `rounds to zero`, and `rounding` can never match) AND a
//!   process signal: a determiner immediately before ("the round-1 bug",
//!   "The other round-1 case") or review vocabulary within the next
//!   [`ROUND_WINDOW`] bytes ("round-1 busy-spin Critical"). Both halves are
//!   drawn from the real in-tree exemplars this scan was widened to catch.
//! - **Plan requirement numbers**: `req N`, but ONLY when the same line also
//!   contains the substring `phase` (case-insensitive). A bare `req N` is
//!   collision-prone on its own (`req` is a common abbreviation with no
//!   reliable process-reference shape by itself); every real occurrence this
//!   scan exists to catch pairs a requirement number with a phase citation
//!   on the same line (`docs/CODE_STANDARDS.md`'s own example, `` `req 4`
//!   ``, appears exactly this way: `` "(phase 10.B, req 4)" ``). Narrowed
//!   per this test's own charter, not a broadening of the doc's rule.
//!
//! **Deliberately out of scope** — two residue shapes stay human-reviewed
//! rather than mechanically caught, both because the only patterns that would
//! catch them also catch real domain vocabulary:
//!
//! - **Internal PR numbers** (`docs/CODE_STANDARDS.md` bans them; a bare `#N`
//!   is not detected here). The real tree carries legitimate external issue
//!   citations in that exact shape (`crates/frust-render/src/context.rs`'s
//!   `` `#7057` ``, `crates/frust-shell-desktop/src/logger.rs`'s ``
//!   `vello#1031` ``) that a precise-enough PR-number pattern has no cheap
//!   way to distinguish from an internal one without a maintained allowlist
//!   of this repo's own PR range.
//! - **Bare plan/workbook tags with no `PLAN`/`Plan` lead-in** — `T04`, `D4`,
//!   `D6b`, `workbook B2` still appear in `crates/frust-tui/src/ui/`'s
//!   comments. It is the lead-in word that makes the plan-document pattern
//!   above safe; a bare uppercase-letter-plus-digits token collides head-on
//!   with this codebase's own design vocabulary (`M3` Material 3, `R8`
//!   texture formats, `B2` … ), so catching them needs a curated tag list, a
//!   human, or a rename of the tags themselves.
//!
//! # Allowlist (a sanctioned citation exempts its own span, not the line)
//!
//! Banned-pattern detection runs FIRST and yields every match's byte span
//! ([`Hit`]). A match is exempt only when its span sits **inside** a
//! sanctioned citation's span — so a line may cite a named rule and still
//! fail for a ledger number elsewhere on it (a whole-line exemption used to
//! let exactly that through). The sanctioned spans:
//!
//! - A backticked `docs/LIMITATIONS.md` stable id (read at test time via
//!   [`limitations_ids`]) — the register's documented purpose.
//! - A named semantic rule (`R23`, `R44-back`, `R47`, ...) — the rule token
//!   itself.
//! - An external-repo SHA in a `rev`/pin context (`` rev `910f626` ``).
//!
//! "Phase N of the frame" (bare, un-parenthesized) and "phase followed by a
//! float" (un-parenthesized) are allowlisted by construction inside
//! [`phase_hits`]'s own state machine rather than as spans — the phase-form
//! parsing (dotted vs. bare, hyphenated vs. spaced, parenthesized vs. not)
//! has to happen once regardless, and duplicating it as a standalone
//! predicate would only be able to disagree with itself.
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
use std::ops::Range;
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

/// The lowercased word immediately before byte offset `idx` in `s` (trailing
/// whitespace skipped, then the run of word chars, apostrophes and hyphens
/// included), or `""` when nothing word-shaped precedes it.
fn preceding_word(s: &str, idx: usize) -> String {
    let before = s[..idx].trim_end();
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '\'' || *c == '-')
        .last()
        .map_or(before.len(), |(i, _)| i);
    before[start..].to_ascii_lowercase()
}

/// Up to `max_len` bytes of `s` starting at `start`, truncated down to a char
/// boundary — these comments are full of multi-byte em dashes, so a naive
/// byte slice would panic.
fn window_after(s: &str, start: usize, max_len: usize) -> &str {
    let mut end = start.saturating_add(max_len).min(s.len());
    while end > start && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[start..end]
}

/// True if `window` contains any of `words` as a whole token — tokens are
/// split on every char that is not ASCII-alphanumeric or `-`, so `busy-spin`
/// is one token and `prefix` never matches the word `fix`.
fn window_has_word(window: &str, words: &[&str]) -> bool {
    window
        .to_ascii_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .any(|token| words.contains(&token.trim_matches('-')))
}

/// One banned-pattern match: the byte range it occupies inside the comment
/// text (the unit the allowlist exempts — see the module doc's Allowlist §)
/// plus the label reported when it isn't exempt.
struct Hit {
    range: Range<usize>,
    reason: &'static str,
}

const PHASE_REASON: &str = "plan-phase reference (CODE_STANDARDS's `Phase 9.B step 1`/`(phase 5.5)` examples, or a \
     hyphenated `phase-7`)";
const TASK_REASON: &str = "plan-task reference (`task-12`/`task g3`)";
const PLAN_DOC_REASON: &str = "plan-document reference (`PLAN D6a`/`Plan D5 §Requirements`)";
const PLAN_PATH_REASON: &str = "workflow plan path (`workflow/plans/...`)";
const FINDINGS_REASON: &str = "workflow findings ledger number (`FINDINGS #43`)";
const REVIEW_FINDING_REASON: &str = "review-finding citation (`(review finding F5)`)";
const ROUND_REASON: &str = "review-round reference (`the round-1 busy-spin Critical`)";
const CFIX_REASON: &str = "fix-round reference (`cfix-2`)";
const RE_REVIEW_REASON: &str = "review-round reference (`re-review`)";
const REVIEW_ROUND_REASON: &str = "review-round reference (`review round N`)";
const DEVICE_PARITY_REASON: &str = "device-parity-round reference";
const REQ_REASON: &str = "plan requirement number (`req N`) alongside a phase reference";

/// Pushes every banned plan-phase reference in `comment` (dotted, hyphenated,
/// or parenthesized/article-labelled bare int) — see the module doc's
/// per-pattern breakdown. Also the sole place the "Phase N of the frame" /
/// un-parenthesized-float / lowercase-`(phase 0)` KEEP classes are enforced
/// (by simply never matching them).
fn phase_hits(comment: &str, out: &mut Vec<Hit>) {
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
        let hyphenated = sep == '-';
        let rest = &after[sep.len_utf8()..];
        let digit_len = leading_digit_run(rest);
        if digit_len == 0 {
            continue;
        }
        let bare_end = idx + 5 + sep.len_utf8() + digit_len;
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
            let dotted_end = bare_end + 1 + suffix_len;
            let has_letter = suffix_str.chars().any(|c| c.is_ascii_alphabetic());
            if has_letter {
                // Rule A: digit.LETTER dotted phase — always a plan-phase id.
                out.push(Hit {
                    range: idx..dotted_end,
                    reason: PHASE_REASON,
                });
            } else if parenthesized {
                // Rule A2: parenthesized pure-float dotted phase — banned
                // (`docs/CODE_STANDARDS.md`'s own `(phase 5.5)` example).
                out.push(Hit {
                    range: idx..dotted_end,
                    reason: PHASE_REASON,
                });
            }
            // Rule A3: un-parenthesized pure float — animation/oscillator
            // idiom, allowlisted. Keep scanning the rest of the line.
        } else if hyphenated {
            // Rule C: hyphenated bare int — this repo's plan-label spelling,
            // in any case and any position.
            out.push(Hit {
                range: idx..bare_end,
                reason: PHASE_REASON,
            });
        } else if parenthesized && capitalized {
            // Rule B: parenthesized, capitalized, space-separated bare int —
            // a plan-phase citation (see module doc's real-tree evidence).
            out.push(Hit {
                range: idx..bare_end,
                reason: PHASE_REASON,
            });
        } else if capitalized && preceding_word(comment, idx) == "the" {
            // Rule D: "the Phase 4 …" — a definite article makes it a label
            // applied to something, never one of the enumeration idioms.
            out.push(Hit {
                range: idx..bare_end,
                reason: PHASE_REASON,
            });
        }
        // Otherwise (bare, un-parenthesized, article-less, or a lowercase
        // parenthesized int): the renderer-span / scenario-stage /
        // animation-rest-state idioms — allowlisted.
    }
}

/// Pushes every `task-NN`/`task NN` (exactly two digits) and `task gN`
/// (exactly one digit) match — both lowercase `task` only.
fn task_hits(comment: &str, out: &mut Vec<Hit>) {
    for idx in find_all(comment, "task") {
        if !word_boundary_before(comment, idx) {
            continue;
        }
        let after = &comment[idx + 4..];
        let Some(sep) = after.chars().next() else {
            continue;
        };
        if sep != '-' && sep != ' ' {
            continue;
        }
        let rest = &after[sep.len_utf8()..];
        if leading_digit_run(rest) == 2 && word_boundary_after(&rest[2..]) {
            out.push(Hit {
                range: idx..idx + 4 + sep.len_utf8() + 2,
                reason: TASK_REASON,
            });
            continue;
        }
        if sep == ' '
            && rest.starts_with('g')
            && leading_digit_run(&rest[1..]) == 1
            && word_boundary_after(&rest[2..])
        {
            out.push(Hit {
                range: idx..idx + 4 + 1 + 2,
                reason: TASK_REASON,
            });
        }
    }
}

/// `Some(len)` if `s` starts with a plan-document tag — an uppercase letter,
/// one or more digits, an optional lowercase suffix, then a word boundary
/// (`D6a`, `D5`, `D4`) — else `None`.
fn plan_tag_len(s: &str) -> Option<usize> {
    if !s.chars().next()?.is_ascii_uppercase() {
        return None;
    }
    let digits = leading_digit_run(&s[1..]);
    if digits == 0 {
        return None;
    }
    let tail = &s[1 + digits..];
    let letters = tail.chars().take_while(char::is_ascii_lowercase).count();
    if !word_boundary_after(&tail[letters..]) {
        return None;
    }
    Some(1 + digits + letters)
}

/// Pushes every plan-document reference: `PLAN <tag>`/`Plan <tag>` (see
/// [`plan_tag_len`]) and any literal `workflow/plans` path.
fn plan_doc_hits(comment: &str, out: &mut Vec<Hit>) {
    let lower = comment.to_ascii_lowercase();
    for idx in find_all(&lower, "plan ") {
        if !word_boundary_before(comment, idx) {
            continue;
        }
        let Some(tag_len) = plan_tag_len(&comment[idx + 5..]) else {
            continue;
        };
        out.push(Hit {
            range: idx..idx + 5 + tag_len,
            reason: PLAN_DOC_REASON,
        });
    }
    for idx in find_all(&lower, "workflow/plans") {
        out.push(Hit {
            range: idx..idx + "workflow/plans".len(),
            reason: PLAN_PATH_REASON,
        });
    }
}

/// Pushes every `FINDINGS #N` (all-caps plural) and `[Ff]inding #N`
/// (singular) ledger citation.
fn findings_hits(comment: &str, out: &mut Vec<Hit>) {
    for idx in find_all(comment, "FINDINGS #") {
        let digits = leading_digit_run(&comment[idx + "FINDINGS #".len()..]);
        if digits > 0 {
            out.push(Hit {
                range: idx..idx + "FINDINGS #".len() + digits,
                reason: FINDINGS_REASON,
            });
        }
    }
    for idx in find_all(comment, "inding #") {
        if idx == 0 || !matches!(comment.as_bytes()[idx - 1], b'F' | b'f') {
            continue;
        }
        let digits = leading_digit_run(&comment[idx + "inding #".len()..]);
        if digits > 0 {
            out.push(Hit {
                range: idx - 1..idx + "inding #".len() + digits,
                reason: FINDINGS_REASON,
            });
        }
    }
}

/// `Some(len)` if `s` starts with ` ` plus a review-finding id (an uppercase
/// letter, one or more digits, then a word boundary: ` F5`, ` M1`).
fn review_finding_id_len(s: &str) -> Option<usize> {
    let rest = s.strip_prefix(' ')?;
    if !rest.chars().next()?.is_ascii_uppercase() {
        return None;
    }
    let digits = leading_digit_run(&rest[1..]);
    if digits == 0 || !word_boundary_after(&rest[1 + digits..]) {
        return None;
    }
    Some(1 + 1 + digits)
}

/// Pushes every review-finding citation: `review finding` (any case) either
/// directly after a `(` or directly followed by a finding id. Prose about
/// review findings with neither marker is kept (see module doc).
fn review_finding_hits(comment: &str, out: &mut Vec<Hit>) {
    let lower = comment.to_ascii_lowercase();
    const NEEDLE: &str = "review finding";
    for idx in find_all(&lower, NEEDLE) {
        if !word_boundary_before(comment, idx) {
            continue;
        }
        let end = idx + NEEDLE.len();
        let parenthesized = idx > 0 && comment.as_bytes()[idx - 1] == b'(';
        let id_len = review_finding_id_len(&comment[end..]).unwrap_or(0);
        if parenthesized || id_len > 0 {
            out.push(Hit {
                range: idx..end + id_len,
                reason: REVIEW_FINDING_REASON,
            });
        }
    }
}

/// How far past a `round-N` label [`round_hits`] looks for review vocabulary.
const ROUND_WINDOW: usize = 48;

/// Determiners that, immediately before a `round-N`, mark it as a label for a
/// specific past review round rather than a count.
const ROUND_DETERMINERS: &[&str] = &[
    "the", "other", "another", "this", "that", "its", "first", "second", "third",
];

/// Review vocabulary that, within [`ROUND_WINDOW`] bytes after a `round-N`,
/// marks it as a review-round label.
const ROUND_REVIEW_WORDS: &[&str] = &[
    "bug",
    "bugs",
    "critical",
    "criticals",
    "review",
    "reviews",
    "fix",
    "fixes",
    "regression",
    "regressions",
    "busy-spin",
    "case",
    "cases",
    "finding",
    "findings",
    "major",
    "majors",
    "minor",
    "minors",
];

/// Pushes every `round-N` review-round label: a word-bounded `round`, a
/// hyphen, digits, a word boundary, plus a determiner before or review
/// vocabulary after (see module doc). `round-trip`, `rounds to zero` and
/// `rounding` can never match.
fn round_hits(comment: &str, out: &mut Vec<Hit>) {
    let lower = comment.to_ascii_lowercase();
    for idx in find_all(&lower, "round-") {
        if !word_boundary_before(comment, idx) {
            continue;
        }
        let rest = &comment[idx + "round-".len()..];
        let digits = leading_digit_run(rest);
        if digits == 0 || !word_boundary_after(&rest[digits..]) {
            continue;
        }
        let end = idx + "round-".len() + digits;
        let determiner = ROUND_DETERMINERS.contains(&preceding_word(comment, idx).as_str());
        let vocabulary =
            window_has_word(window_after(comment, end, ROUND_WINDOW), ROUND_REVIEW_WORDS);
        if determiner || vocabulary {
            out.push(Hit {
                range: idx..end,
                reason: ROUND_REASON,
            });
        }
    }
}

/// Pushes every match of `needle` immediately followed by one or more ASCII
/// digits.
fn digit_suffixed_hits(comment: &str, needle: &str, reason: &'static str, out: &mut Vec<Hit>) {
    for idx in find_all(comment, needle) {
        let digits = leading_digit_run(&comment[idx + needle.len()..]);
        if digits > 0 {
            out.push(Hit {
                range: idx..idx + needle.len() + digits,
                reason,
            });
        }
    }
}

/// Pushes every plain-literal match of `needle`.
fn literal_hits(comment: &str, needle: &str, reason: &'static str, out: &mut Vec<Hit>) {
    for idx in find_all(comment, needle) {
        out.push(Hit {
            range: idx..idx + needle.len(),
            reason,
        });
    }
}

/// Pushes every `req N` match, but only when the same comment also contains
/// `phase` (case-insensitive) — the narrowed `req` rule (see module doc).
fn req_with_phase_hits(comment: &str, out: &mut Vec<Hit>) {
    if !comment.to_ascii_lowercase().contains("phase") {
        return;
    }
    for idx in find_all(comment, "req ") {
        if !word_boundary_before(comment, idx) {
            continue;
        }
        let digits = leading_digit_run(&comment[idx + 4..]);
        if digits > 0 {
            out.push(Hit {
                range: idx..idx + 4 + digits,
                reason: REQ_REASON,
            });
        }
    }
}

/// Every banned-pattern match in `comment`, sorted by position — the input to
/// the span-scoped allowlist in [`scan_comment`].
fn banned_hits(comment: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    phase_hits(comment, &mut hits);
    task_hits(comment, &mut hits);
    plan_doc_hits(comment, &mut hits);
    findings_hits(comment, &mut hits);
    review_finding_hits(comment, &mut hits);
    round_hits(comment, &mut hits);
    digit_suffixed_hits(comment, "cfix-", CFIX_REASON, &mut hits);
    literal_hits(comment, "re-review", RE_REVIEW_REASON, &mut hits);
    digit_suffixed_hits(comment, "review round ", REVIEW_ROUND_REASON, &mut hits);
    literal_hits(
        comment,
        "device-parity-round",
        DEVICE_PARITY_REASON,
        &mut hits,
    );
    req_with_phase_hits(comment, &mut hits);
    hits.sort_by_key(|hit| (hit.range.start, hit.range.end));
    hits
}

/// The first banned-pattern label in `comment`, ignoring the allowlist — the
/// fixture-facing view of [`banned_hits`].
fn banned_reason(comment: &str) -> Option<&'static str> {
    banned_hits(comment).first().map(|hit| hit.reason)
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

/// The span of every backticked citation of one of `ids`
/// (`docs/LIMITATIONS.md`'s stable ids — the register's documented purpose),
/// backticks included.
fn limitations_id_spans(comment: &str, ids: &[String]) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    for id in ids {
        let quoted = format!("`{id}`");
        for idx in find_all(comment, &quoted) {
            spans.push(idx..idx + quoted.len());
        }
    }
    spans
}

/// The span of every named-semantic-rule citation (`R23`, `R44-back`,
/// `R47`, ...): a word-boundary `R`, digits, and any word-ish tail.
fn named_rule_spans(comment: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    for (i, byte) in comment.as_bytes().iter().enumerate() {
        if *byte != b'R' || !word_boundary_before(comment, i) {
            continue;
        }
        let rest = &comment[i + 1..];
        if leading_digit_run(rest) == 0 {
            continue;
        }
        let tail = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .map(char::len_utf8)
            .sum::<usize>();
        spans.push(i..i + 1 + tail);
    }
    spans
}

/// The span of every external-repo SHA citation in a `rev`/pin context (``
/// rev `910f626` ``): `rev `, an optional quote/backtick, and a 6–40 char hex
/// run.
fn external_rev_spans(comment: &str) -> Vec<Range<usize>> {
    let lower = comment.to_ascii_lowercase();
    let mut spans = Vec::new();
    for idx in find_all(&lower, "rev ") {
        let after = &comment[idx + 4..];
        let quotes = after.len() - after.trim_start_matches(['`', '"', '\'']).len();
        let hex = after[quotes..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .count();
        if (6..=40).contains(&hex) {
            let closer = usize::from(after[quotes + hex..].starts_with(['`', '"', '\'']));
            spans.push(idx..idx + 4 + quotes + hex + closer);
        }
    }
    spans
}

/// Every "Sanctioned citations" span in `comment` — the KEEP classes whose
/// own text this scan must never flag (see the module doc's Allowlist §).
fn sanctioned_spans(comment: &str, limitation_ids: &[String]) -> Vec<Range<usize>> {
    let mut spans = limitations_id_spans(comment, limitation_ids);
    spans.extend(named_rule_spans(comment));
    spans.extend(external_rev_spans(comment));
    spans
}

/// The full per-comment decision this scan makes: `None` if `comment` is
/// clean (no banned pattern matched, or every match sits inside a sanctioned
/// citation's span), `Some(reason)` for the first match that does not.
fn scan_comment(comment: &str, limitation_ids: &[String]) -> Option<&'static str> {
    let sanctioned = sanctioned_spans(comment, limitation_ids);
    banned_hits(comment)
        .into_iter()
        .find(|hit| {
            !sanctioned
                .iter()
                .any(|span| span.start <= hit.range.start && hit.range.end <= span.end)
        })
        .map(|hit| hit.reason)
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
        "a round-trip through the encoder",
        "the accumulator rounds to zero",
        "a compile error rather than a review finding",
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
        "the Phase-4 gate numbers",
        "Plan D6a §Requirements",
        "the round-0 bug",
        "(review finding M1)",
        "workflow/plans/features/x/PLAN.md",
    ];
    for example in strip {
        assert!(
            scan_comment(example, &limitation_ids).is_some(),
            "expected STRIP exemplar {example:?} to fail"
        );
    }
}

/// Fixture-driven unit tests for the scanner's individual pattern decisions —
/// the tricky dotted-vs-bare / hyphen-vs-space / parenthesized-vs-not / case
/// distinctions this module's doc comment documents as deliberate narrowing.
#[cfg(test)]
mod scan_behavior {
    use super::*;

    /// Runs one detector over `comment` and reports whether it matched — the
    /// bool view every fixture below wants.
    fn fires(detector: fn(&str, &mut Vec<Hit>), comment: &str) -> bool {
        let mut hits = Vec::new();
        detector(comment, &mut hits);
        !hits.is_empty()
    }

    fn phase_violation(comment: &str) -> bool {
        fires(phase_hits, comment)
    }

    fn task_violation(comment: &str) -> bool {
        fires(task_hits, comment)
    }

    fn findings_violation(comment: &str) -> bool {
        fires(findings_hits, comment)
    }

    fn plan_doc_violation(comment: &str) -> bool {
        fires(plan_doc_hits, comment)
    }

    fn round_violation(comment: &str) -> bool {
        fires(round_hits, comment)
    }

    fn review_finding_violation(comment: &str) -> bool {
        fires(review_finding_hits, comment)
    }

    fn req_with_phase_violation(comment: &str) -> bool {
        fires(req_with_phase_hits, comment)
    }

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
    fn hyphenated_bare_int_phase_is_always_banned() {
        assert!(
            phase_violation("the Phase-4 gate numbers"),
            "capitalized, un-parenthesized — the glyph-catalog exemplar"
        );
        assert!(
            phase_violation("the phase-7 conservative default"),
            "lowercase — the frame_gate.rs exemplar"
        );
        assert!(
            phase_violation("Phase-4 gate session transcribes into MEASUREMENTS.md"),
            "line-initial, no article"
        );
    }

    #[test]
    fn space_separated_bare_int_phase_needs_parens_or_an_article() {
        assert!(
            !phase_violation("Phase 1 of the frame — the encode span"),
            "un-parenthesized bare int is the renderer-span idiom"
        );
        assert!(
            !phase_violation("Phase 1 (250ms): a transition and a paced loop both request"),
            "a test's own numbered scenario stage (pacing_integration.rs)"
        );
        assert!(
            !phase_violation("Phase 0 → t exactly 0."),
            "an animation phase (loading_indicator.rs)"
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
        assert!(
            phase_violation("the Phase 4 gate numbers"),
            "a definite article makes it a label, not an enumeration"
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
    fn plan_document_references_need_a_plan_tag_shape() {
        assert!(plan_doc_violation("Plan D6a §Requirements"));
        assert!(plan_doc_violation("The doctor panel (PLAN D6/D6a)"));
        assert!(plan_doc_violation("Async session supervision (PLAN D2/D3)"));
        assert!(
            !plan_doc_violation("we plan to revisit this once the atlas lands"),
            "`plan` as an ordinary verb"
        );
        assert!(
            !plan_doc_violation("Plan 9 from outer space"),
            "a bare digit is not a plan tag"
        );
        assert!(
            !plan_doc_violation("the fallback plan B keeps the frame"),
            "a letter with no digit is not a plan tag"
        );
    }

    #[test]
    fn workflow_plan_paths_are_banned() {
        assert!(plan_doc_violation(
            "the GO/NO-GO input for `workflow/plans/features/x/PLAN.md`"
        ));
        assert!(!plan_doc_violation(
            "see docs/DEVELOPMENT.md's Benchmarks §"
        ));
    }

    #[test]
    fn findings_matches_plural_allcaps_and_singular_any_case() {
        assert!(findings_violation("FINDINGS #43"));
        assert!(findings_violation("Finding #44"));
        assert!(findings_violation("finding #45"));
        assert!(!findings_violation("no ledger reference here"));
    }

    #[test]
    fn review_finding_citations_need_a_paren_or_an_id() {
        assert!(review_finding_violation("(review finding M1)"));
        assert!(review_finding_violation(
            "Review finding M1: the shells now"
        ));
        assert!(review_finding_violation("the next frame (review finding"));
        assert!(
            !review_finding_violation("this becomes a compile error rather than a review finding."),
            "prose about review findings is not a citation (nav/route.rs)"
        );
        assert!(
            !review_finding_violation("closing a further-review finding)"),
            "trailing paren is not the parenthesized citation form (huddle architecture.rs)"
        );
    }

    #[test]
    fn round_n_needs_a_determiner_or_review_vocabulary() {
        assert!(round_violation("That is the round-0 bug `dee29bb` fixed"));
        assert!(round_violation("the round-1 busy-spin Critical"));
        assert!(round_violation(
            "round-1 busy-spin Critical). The cap/fold arithmetic is"
        ));
        assert!(round_violation(
            "The other round-1 case: a CI rig exporting"
        ));
        assert!(round_violation("The round-1 regression, in miniature"));
        assert!(
            !round_violation("a round-trip through the encoder"),
            "a letter after the hyphen can never match"
        );
        assert!(
            !round_violation("the accumulator rounds to zero"),
            "`rounds` is not the word `round`"
        );
        assert!(
            !round_violation("rounding-1 is not a word boundary match"),
            "`round` must end at the hyphen"
        );
        assert!(
            !round_violation("a round-2 corner radius in device pixels"),
            "no determiner, no review vocabulary"
        );
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
    fn sanctioned_citation_spans() {
        let ids = vec!["cam-blit-opaque".to_string()];
        assert!(!limitations_id_spans("see `cam-blit-opaque` for the known gap", &ids).is_empty());
        assert!(limitations_id_spans("no citation here", &ids).is_empty());

        assert!(!named_rule_spans("R44-back").is_empty());
        assert!(!named_rule_spans("named rule R23").is_empty());
        assert!(named_rule_spans("no rule cited").is_empty());

        assert!(!external_rev_spans("clean-signals rev `910f626`").is_empty());
        assert!(!external_rev_spans("pinned to rev 910f626abc").is_empty());
        assert!(external_rev_spans("no rev cited").is_empty());
    }

    #[test]
    fn an_allowlisted_citation_exempts_only_its_own_span() {
        let ids = limitations_ids();

        assert_eq!(
            scan_comment("the R23 focus rule, still honored", &ids),
            None,
            "a named rule alone is a sanctioned citation"
        );
        assert!(
            scan_comment("the R23 focus rule — re-created FINDINGS #43", &ids).is_some(),
            "a sanctioned citation must not launder a ledger number elsewhere on the line"
        );
        assert!(
            scan_comment("`cam-blit-opaque`, first seen in (Phase 9.B step 1)", &ids).is_some(),
            "same for a LIMITATIONS id"
        );
        assert!(
            scan_comment("clean-signals rev `910f626` — the task-12 constants", &ids).is_some(),
            "same for an external rev pin"
        );
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
        assert_eq!(
            comment_text("    caption(\"… the Phase-4 gate numbers.\"),"),
            None,
            "UI copy in a string literal is never comment text (glyph-catalog camera.rs)"
        );
    }

    #[test]
    fn multi_byte_comment_text_never_panics() {
        // Em dashes are everywhere in this codebase's comments; the round
        // window and every other slice must land on char boundaries.
        let ids = limitations_ids();
        let comment = "// the round-1 fix — an em dash — and a → arrow, plus ✓ and ζ=0.6";
        assert!(scan_comment(comment, &ids).is_some());
        assert_eq!(scan_comment("// — → ✓ ζ", &ids), None);
    }
}
