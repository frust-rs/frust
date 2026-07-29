//! Architecture conformance test: enforces huddle's per-feature
//! clean-architecture layering, hardened across several rounds of fixes
//! against bypasses found in the scan itself — see the "What this is NOT"
//! section below.
//!
//! A plain `std::fs` source scan over `src/`, run as an ordinary `cargo test`
//! (this repo has no lint-plugin/static-analysis tooling — see
//! `docs/CODE_STANDARDS.md`), enforcing the per-feature layering rules:
//!
//! (a) a `features/*/domain/**` file never mentions `frust::`, or a
//!     `data`/`presentation` path segment banned below (see
//!     [`has_exact_segment`]) — domain is the framework- and layer-free core;
//! (b) a `features/*/presentation/**` file never mentions a `data` path
//!     segment banned below — a presentation file reaches the shared
//!     dataset through an injected repository trait, never the store or a
//!     sibling feature's `data/` module, directly;
//! (c) only a `*/data/**` file (or `src/data/` itself) mentions
//!     `crate::data::store` — the raw shared dataset is the data layer's to
//!     read;
//! (d) no file anywhere mentions `crate::mock` or `crate::screens` — both
//!     modules are deleted by this task, so any surviving reference is stale;
//! (e) no file outside `*/data/**`, `src/lib.rs` (the composition root), or
//!     the messages `resolve_repo` allowlist entry names a concrete
//!     `Store*Repository`/`InMemory*Repository` type — closes both direct
//!     construction AND a facade re-export (`pub use` of a concrete repo
//!     type from a feature `mod.rs`) outside the sanctioned sites;
//! (f) `features::search`/`features::profile`'s `domain`+`data` never mention
//!     `HuddleFailure`, `ControllerCore`, or `async fn` — a ratchet
//!     keeping both features' sync-infallible shape from
//!     regressing toward the `ControllerCore`/`HuddleFailure` spine the other
//!     four features use.
//!
//! # Scope: production code only
//!
//! Every check scans **production code only** — a file's `#[cfg(test)]`
//! trailing test module (huddle convention: the test module is always
//! last-in-file, verified per file below) and every doc/line-comment-only
//! line are stripped before matching (see [`production_lines`]). This is
//! deliberate, not a loophole: controller unit tests legitimately construct
//! the real `Store<Feature>Repository` under `#[cfg(test)]` (an established
//! mini-composition-root pattern) so
//! their assertions can target the real dataset shape without a test-logic
//! rewrite (the behavior-preserving bar: import-path edits only, never a
//! test-logic change). Doc
//! comments are stripped for the same reason, seen repeatedly:
//! several domain/data files carry historical or intra-doc-link prose
//! mentioning `crate::mock`/`crate::data::store`/`frust::` (e.g.
//! `profile/domain/entities.rs`'s inherited doc link) that names, not
//! imports, the thing it's talking about. Checks (a)/(b)/(e) additionally
//! run over `production_lines`' *use-joined* output (see
//! [`join_use_statements`]) so a `use` tree can't be split across lines to
//! dodge detection; checks (c)/(d)/(f) don't need this (see each check's own
//! comment).
//!
//! # What this is NOT
//!
//! This is a **substring/token scan, not a parser** — it has no `syn`/AST
//! understanding of Rust `use` trees, paths, or types. An initial adversarial
//! review demonstrated three concrete ways a substring scan can be defeated;
//! a first round of fixes closed those three, but a follow-up
//! review reproduced two more gaps *in that fix itself* (both
//! closed here, by a second round of fixes). A further review then
//! found one more, LATENT (fails-safe, never a bypass) gap, closed here by
//! a third round of fixes (below):
//!
//! 1. **Alias/use-form bypass — CLOSED (first round).** A single
//!    `"::data::"`/
//!    `"::presentation::"` needle missed `use ...::data as msgdata;` (no
//!    trailing `::`). Superseded by the second round's segment tokenizer (gap
//!    B below),
//!    which makes every use-form terminator (`;`, ` as `, `}`, `,`, a
//!    brace-list member, …) irrelevant — see [`has_exact_segment`].
//! 2. **Concrete-repo-name bypass — CLOSED (first round).** A
//!    facade re-export
//!    (`pub use data::repositories::StoreChannelRepository;` from a feature
//!    `mod.rs`) never matched the old `::data::`/`::presentation::` needles
//!    at all — the banned type crosses the layer boundary by name, not by
//!    path. [`contains_concrete_repo_name`] bans the
//!    `Store[A-Za-z]*Repository`/`InMemory[A-Za-z]*Repository` name pattern
//!    itself anywhere outside the data layer/composition root/allowlist (see
//!    check (e)), covering both direct construction and a re-export.
//! 3. **Split-use bypass — CLOSED (first round).** A `use` tree
//!    spanning multiple
//!    physical lines could put a banned segment on a different physical line
//!    than the rest of the path, defeating a needle scanned line-by-line.
//!    [`join_use_statements`] concatenates a `use ...;` statement's physical
//!    lines into one logical line before any check runs.
//! 4. **Gap A — `pub use` join trigger — CLOSED (second round).**
//!    The first round's
//!    [`join_use_statements`] only recognized a joinable statement by the
//!    literal `"use "` prefix, so a split `pub use`/`pub(crate) use`/
//!    `pub(super) use` re-export (the exact facade-re-export shape check (e)
//!    exists to police) was never joined at all, reopening the split-use
//!    bypass for the join-trigger's own blind spot. [`is_use_trigger_line`]
//!    now recognizes bare `use `, and any `pub`/`pub(...)`-prefixed `use `,
//!    as a join trigger — see its doc comment for the exact accepted forms.
//! 5. **Gap B — brace-list membership forms — CLOSED (second round).**
//!    Every needle from the first round
//!    required a `::` immediately before the banned segment; a
//!    rustfmt-produced grouped import puts a segment straight after `{`, `, `,
//!    or inside a nested `{self, ...}` group with no `::` prefix at all
//!    (`use x::{domain::Foo, data::Bar};` — the `data` member sits right
//!    after `, `, not `::`) — none of those needles matched. [`has_exact_segment`]
//!    replaces the whole needle-list approach: a joined `use` line is split on
//!    every non-identifier delimiter (`::`, `{`, `}`, `,`, `;`, whitespace,
//!    `(`, `)` — see [`tokenize_segments`]) and an *exact* `data`/`presentation`
//!    segment token is banned regardless of which punctuation surrounds it, so
//!    no enumeration of use-form terminators is needed at all — the
//!    recommended fix shape from that review.
//! 6. **Trailing-comment false positive — CLOSED (third round).**
//!    This review's one
//!    confirmed high-severity finding: [`has_exact_segment`]'s segment tokenizer ran over
//!    un-comment-stripped `use`-line text, so a trailing `// … data …`
//!    comment on an otherwise-clean `use` line could trip an exact-token
//!    false positive on the comment's own prose — LATENT (fails-safe: a loud
//!    spurious failure naming file+line, never a silent bypass; the opposite
//!    direction from the bypass class the earlier rounds hunted). [`join_use_statements`]
//!    now strips a trailing `//`-to-end-of-line suffix from each physical
//!    line on the use-statement path only, via [`strip_trailing_comment`],
//!    before any joining/tokenizing happens — see that function's doc
//!    comment for why the strip is safe there and only there.
//!
//! **Residual, accepted limitations** (not closed by any pass — a real
//! `syn`-based checker would be the fix, judged not worth the dependency for
//! a single-example conformance ratchet):
//!
//! - **Keyword-adjacent `use\n<path>` split**: a theoretical `use` statement
//!   split immediately after the `use` keyword itself, e.g. `use\n    crate::…;`
//!   (as opposed to every split this scan's fixtures/injections exercise,
//!   which break at a punctuation/path boundary further in) — a pre-existing,
//!   optional-severity gap. `rustfmt` never emits this shape (it
//!   only breaks a `use` item at `::`/`{`/`,` boundaries), so it is
//!   documented here rather than fixed.
//! - **Renames**: `type StoreChannelRepositoryAlias = StoreChannelRepository;`
//!   (or a `use ... as` rename of the *type itself*, not the module) still
//!   reads as the banned name pattern at the `type`/`use` site, but a
//!   *subsequent* reference through the new short name (e.g.
//!   `StoreChannelRepositoryAlias::new()` elsewhere) would not match — the
//!   scan has no cross-file symbol-alias resolution.
//! - **Macro-generated imports/types**: a `use`/type name produced by macro
//!   expansion rather than appearing literally in source text is invisible
//!   to a source-text scan by construction.
//! - **Mid-file `#[cfg(test)]` over-stripping**: [`production_lines`]
//!   truncates a file at its *first* `#[cfg(test)]`/`mod tests` marker,
//!   assuming huddle's test-module-last-in-file convention (verified true
//!   for every file in this crate with one); a future file breaking that
//!   convention would have everything
//!   after an early marker silently excluded from scanning, not just the
//!   real test module (a self-flagged, accepted limitation — no action taken).
//!
//! # Sanctioned exemptions (explicit, file-scoped, comment-documented)
//!
//! Beyond the `#[cfg(test)]`/doc-comment stripping above, documented
//! exceptions exist, each an explicit `(file, needle)` pair below rather than
//! a blanket loosening of the ban it exempts from — see each test function's
//! own doc comment for the full rationale:
//!
//! 1. `features/messages/presentation/controllers.rs`'s `resolve_repo()`
//!    fallback — see
//!    [`presentation_never_imports_data_directly`] and (the same fallback,
//!    now also matching the concrete-name needle)
//!    [`no_concrete_repo_type_name_outside_data_layer_or_composition_root`].
//! 2. `features/settings/domain/{models.rs, accent.rs,
//!    use_cases/set_theme.rs}`'s `frust::` value-type/side-effect imports
//!    — see [`domain_never_imports_frust_presentation_or_data`].
//!
//! Every exemption below is also asserted **used** (fired at least once) —
//! an exemption nobody's code needs any more is exactly as stale as a
//! violation, and should be deleted along with whatever code prompted it.
//! `src/lib.rs` (the composition root) is instead a **structural carve-out**
//! in check (e) below, not an allowlist entry — it's the one sanctioned
//! construction site every concrete repository type is built at, not a
//! narrow documented exception to a general ban.

use std::fs;
use std::path::{Path, PathBuf};

/// `examples/huddle/src`, resolved from the crate's own manifest dir so this
/// test works regardless of the invoking `cargo test`'s working directory.
fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Recursively collect every `.rs` file under `dir`, sorted for stable,
/// diffable failure output.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("architecture scan: reading {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("architecture scan: dir entry: {e}"));
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// `path` relative to `src/`, forward-slashed, for stable failure messages
/// and exemption-table keys independent of host path separators.
fn rel(path: &Path) -> String {
    path.strip_prefix(src_dir())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// One production source line: 1-based line number (matching an editor's/
/// `rustc`'s own numbering) plus its raw text. A [`join_use_statements`]-
/// joined logical line keeps the *first* physical line's number, so a
/// failure still points a reader at the `use` statement's start.
struct Line {
    number: usize,
    text: String,
}

/// A file's production-code lines: every doc-comment (`//!`/`///`) or plain
/// `//` line-comment-only line dropped, everything from the file's
/// `#[cfg(test)]`/`mod tests` marker onward truncated (huddle convention
/// keeps the test module last-in-file — verified true for every file in this
/// crate that currently has one),
/// and finally a multi-line `use` statement's physical lines joined into one
/// logical [`Line`] (see [`join_use_statements`] — closes the split-use
/// bypass an early adversarial review demonstrated).
///
/// The marker check is anchored on the line's own (non-comment) start, so a
/// *prose* mention of `#[cfg(test)]` inside a doc comment (several files
/// have one, describing this very pattern) never truncates the scan early —
/// only a real attribute/item at that position does.
fn production_lines(contents: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for (i, raw) in contents.lines().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("mod tests") {
            break;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        out.push(Line {
            number: i + 1,
            text: raw.to_string(),
        });
    }
    join_use_statements(out)
}

/// True if `trimmed` (a line's start-trimmed text) opens a joinable `use`
/// statement — the trigger [`join_use_statements`] looks for before it starts
/// accumulating physical lines into one logical line. Recognizes bare
/// `"use "`, and any visibility-qualified form — `"pub use "`,
/// `"pub(crate) use "`, `"pub(super) use "`, `"pub(self) use "`, or
/// `"pub(in <path>) use "` — i.e. `pub` optionally followed by one
/// parenthesized visibility qualifier, then `"use "`. A prefix check, not a
/// full attribute/visibility parser (deliberately: this crate's `use`/
/// `pub use` style never carries an attribute or doc comment on the same
/// physical line as the keyword itself), but it closes a follow-up
/// review's **gap A**: the first round's join trigger matched only the
/// literal `"use "` prefix, so a
/// split `pub use`/`pub(crate) use` facade re-export — exactly the shape
/// check (e) exists to police — was never joined at all, reopening the
/// split-use bypass for that one form (see the module header's "What this is
/// NOT" section).
fn is_use_trigger_line(trimmed: &str) -> bool {
    if trimmed.starts_with("use ") {
        return true;
    }
    let Some(after_pub) = trimmed.strip_prefix("pub") else {
        return false;
    };
    let after_pub = after_pub.trim_start();
    let after_vis = match after_pub.strip_prefix('(') {
        Some(rest) => match rest.find(')') {
            Some(close) => rest[close + 1..].trim_start(),
            None => return false, // unterminated `pub(...)` — not a use trigger
        },
        None => after_pub,
    };
    after_vis.starts_with("use ")
}

/// Join a `use ...;` statement's physical lines into one logical [`Line`] so
/// a banned segment can't straddle a line break (an early adversarial
/// review's third
/// demonstrated bypass — a `use` tree split across lines, with the banned
/// segment on a different physical line than where a line-by-line scan would
/// expect it). Deliberately simple and deterministic, not a real `use`-tree
/// parser: any non-comment line (comments are already stripped by the time
/// this runs) whose trimmed text opens a `use` statement (see
/// [`is_use_trigger_line`] — broadened to `pub`-prefixed forms by the second
/// round's gap
/// A closure) has subsequent lines appended (each trimmed) until the
/// accumulated text contains a `;` — the one character every `use` statement
/// in this codebase's style (one item per statement, `rustfmt`-formatted)
/// ends on. A `use` statement already complete on one line (the common case)
/// round-trips through this unchanged. A malformed/truncated accumulation
/// (statement never closes before the file — or the pre-truncated
/// `#[cfg(test)]` region — ends) just stops accumulating rather than
/// panicking; the resulting text is scanned as-is like anything else.
///
/// **Trailing `//` comments are stripped on this path only (the third
/// round of fixes, closing a further-review finding).** Every physical line that's part of a `use` statement — the trigger
/// line and every line appended to it — is run through
/// [`strip_trailing_comment`] before it's checked for `;` or joined, so a
/// trailing `// … data …` comment can no longer feed a banned segment token
/// into the scan. See [`strip_trailing_comment`]'s doc comment for why this
/// is safe only on this path.
///
/// **No blind space-insertion at the join point** — deliberately, not an
/// oversight. A rustfmt-produced (or hand-written) `use` tree only ever
/// breaks at a punctuation boundary that already supplies its own
/// separation (`::`, `{`, `,`, `}`) or at a whitespace-delimited keyword
/// (`as`) that needs one. [`needs_space_at_join`] tells the two cases apart:
/// a banned segment like `data` must reconstruct with **no** space when a
/// split lands right after the last `::` (`activity::` / `data::...` must
/// rejoin as `activity::data::...`, not `activity:: data::...`), while
/// `"::data as msgdata;"` split right before `as` must rejoin **with** one
/// (`data` / `as msgdata;` must become `data as msgdata;`, not `dataas
/// msgdata;` — which would fuse into one non-matching token under
/// [`tokenize_segments`]' delimiter-based split).
fn join_use_statements(lines: Vec<Line>) -> Vec<Line> {
    let mut out = Vec::new();
    let mut iter = lines.into_iter().peekable();
    while let Some(line) = iter.next() {
        let trimmed = line.text.trim_start();
        if !is_use_trigger_line(trimmed) {
            out.push(line);
            continue;
        }
        // Use-statement path only: strip this physical line's trailing `//`
        // comment (if any) before it's checked for `;` or accumulated — see
        // strip_trailing_comment's doc comment for why this is confined to
        // the use-statement path and must never run on a general expression
        // line.
        let stripped = strip_trailing_comment(&line.text).to_string();
        if stripped.contains(';') {
            out.push(Line {
                number: line.number,
                text: stripped,
            });
            continue;
        }
        let number = line.number;
        let mut text = stripped;
        while !text.contains(';') {
            match iter.next() {
                Some(next) => {
                    let next_trimmed = strip_trailing_comment(next.text.trim());
                    if needs_space_at_join(&text, next_trimmed) {
                        text.push(' ');
                    }
                    text.push_str(next_trimmed);
                }
                None => break,
            }
        }
        out.push(Line { number, text });
    }
    out
}

/// Strip a trailing `//`-to-end-of-line comment from one physical line of a
/// `use` statement, returning everything before the first `//` (right-
/// trimmed of the whitespace that preceded it), or the line unchanged if it
/// has no `//`.
///
/// **Safe ONLY on the use-statement path — [`join_use_statements`] is this
/// function's one caller, and must stay so.** A `use` item can never contain
/// a string literal (there is no such thing as `use "foo";`), so a naive
/// first-`//` split can't misfire the way it could on a general expression
/// line, where a `"https://…"` string literal's `//` would be truncated
/// mid-string and corrupt the line. This is exactly why this fix is
/// scoped to the use-statement path rather than folded into
/// [`production_lines`]'s general per-line stripping (which only ever drops
/// a line whose *entire* trimmed text is a comment, never a trailing one on
/// real code) — see the module header's "What this is NOT" section, item 6.
fn strip_trailing_comment(text: &str) -> &str {
    match text.find("//") {
        Some(idx) => text[..idx].trim_end(),
        None => text,
    }
}

/// True only when both the accumulated text's last character and the next
/// line's first character are identifier characters (ASCII alphanumeric or
/// `_`) — the one case where concatenating with no separator would fuse two
/// distinct tokens into one (e.g. `data` + `as` -> `dataas`). Any join where
/// either side is punctuation (`::`, `{`, `,`, `}`) needs no inserted space:
/// the punctuation already provides the separation the reconstructed text
/// needs (e.g. `activity::` + `data::...` -> `activity::data::...`).
fn needs_space_at_join(accumulated: &str, next_trimmed: &str) -> bool {
    let is_ident_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    accumulated.chars().next_back().is_some_and(is_ident_char)
        && next_trimmed.chars().next().is_some_and(is_ident_char)
}

fn domain_files() -> Vec<PathBuf> {
    rust_files(&src_dir())
        .into_iter()
        .filter(|p| rel(p).contains("/domain/"))
        .collect()
}

fn presentation_files() -> Vec<PathBuf> {
    rust_files(&src_dir())
        .into_iter()
        .filter(|p| rel(p).contains("/presentation/"))
        .collect()
}

/// True for a file inside a feature's `data/` layer or the shared
/// `src/data/` module itself — the only files sanctioned to read
/// `crate::data::store` directly (check c) or name a concrete
/// `Store*Repository`/`InMemory*Repository` type (check e).
fn is_data_layer_file(relp: &str) -> bool {
    relp.starts_with("data/") || relp.contains("/data/")
}

/// Split `text` on every non-identifier delimiter — `::`, `{`, `}`, `,`,
/// `;`, `(`, `)`, and whitespace — into bare identifier-ish segments, empty
/// segments dropped. `::` splits cleanly under a single-char-class split
/// (`:` is one of the delimiter chars, so `"a::b"` yields `["a", "", "b"]`
/// before empty segments are filtered).
///
/// This is the second round's answer to that review's **gap
/// B**: the first round's needle list
/// required a `::` immediately before a banned segment, which a brace-list
/// member (`use x::{domain::Foo, data::Bar};` — `data` sits right after
/// `", "`, not `"::"`) or a nested `{self, ...}` group never supplies. Once
/// split into segments, *where* `data` sits relative to punctuation stops
/// mattering — every use-form terminator (`;`, ` as `, `}`, `,`, a brace-list
/// position, self-form nesting, …) reduces to the same exact-token
/// comparison. See [`has_exact_segment`].
fn tokenize_segments(text: &str) -> Vec<&str> {
    text.split(|c: char| {
        c == ':'
            || c == '{'
            || c == '}'
            || c == ','
            || c == ';'
            || c == '('
            || c == ')'
            || c.is_whitespace()
    })
    .filter(|s| !s.is_empty())
    .collect()
}

/// True if any [`tokenize_segments`] token of `text` is EXACTLY `segment`.
/// Applied only to a [`join_use_statements`]-joined `use`-statement line (see
/// checks (a)/(b) below) — restricting the segment check to `use` lines is
/// the false-positive guard: a plain local variable or field named `data`
/// (e.g. `let data = ...;`, `struct Foo { data: Bar }`) is never a `use`
/// line, so it never reaches this function and never trips the scan, even
/// though it contains the bare identifier `data` too.
fn has_exact_segment(text: &str, segment: &str) -> bool {
    tokenize_segments(text).into_iter().any(|s| s == segment)
}

/// Path-expression needle for a **non**-`use` line — an inline fully
/// qualified expression like
/// `crate::features::messages::data::repositories::Foo::new()` that isn't
/// part of a `use` tree at all, so has no `use`-statement structure for
/// [`has_exact_segment`] to apply to. Reduced from the first pass's
/// five-entry
/// use-form needle list (`DATA_NEEDLES`/`PRESENTATION_NEEDLES`, deleted by
/// the second pass): every `use`-line termination form that list enumerated is now
/// covered by [`has_exact_segment`] instead, so only the plain
/// fully-qualified-path form — a leading and trailing `::` around the banned
/// segment — remains here. This is also part of the false-positive guard:
/// requiring both surrounding `::`s means a bare `data`/`presentation`
/// identifier used as a local name in an expression never matches.
const DATA_EXPR_NEEDLE: &str = "::data::";
/// Same treatment as [`DATA_EXPR_NEEDLE`], for the `presentation` segment
/// domain files ban (check a only — presentation files obviously mention
/// `presentation` themselves, e.g. their own module path, so this is never
/// applied to presentation files).
const PRESENTATION_EXPR_NEEDLE: &str = "::presentation::";

/// True if `text` contains the concrete-repository-type-name pattern
/// `Store[A-Za-z]*Repository` or `InMemory[A-Za-z]*Repository` — a concrete
/// repository *struct* name (`StoreChannelRepository`,
/// `StoreMessageRepository`, a future `InMemoryFooRepository`, ...) as
/// opposed to the `*Repository` *trait* names each feature's own
/// `domain/repositories.rs` declares, none of which carry a `Store`/
/// `InMemory` prefix. Hand-rolled substring/char scan rather than a real
/// regex engine — this crate carries no `regex` dependency, and adding one
/// solely for this test would be disproportionate (see the module header's
/// "What this is NOT" section); the scan below is exactly the naive-regex
/// semantics `Store[A-Za-z]*Repository` describes (`Store`, then zero or
/// more ASCII letters, then `Repository`, with no gap-character other than
/// ASCII letters allowed in between).
fn contains_concrete_repo_name(text: &str) -> bool {
    ["Store", "InMemory"]
        .iter()
        .any(|prefix| has_repo_name_with_prefix(text, prefix))
}

/// Scans `text` for `prefix`, then — from right after each occurrence —
/// walks forward one ASCII-letter at a time, checking at every position
/// (including zero letters consumed) whether the remainder starts with
/// `"Repository"`. A naive greedy `take_while(is_ascii_alphabetic)` would
/// overshoot: since `"Repository"` is itself all letters, it would be
/// consumed as part of the "middle" run, leaving nothing left to match
/// against. Walking one letter at a time and checking at each step avoids
/// that trap.
fn has_repo_name_with_prefix(text: &str, prefix: &str) -> bool {
    let mut search_from = 0;
    while let Some(rel_idx) = text[search_from..].find(prefix) {
        let start = search_from + rel_idx;
        let mut pos = start + prefix.len();
        loop {
            if text[pos..].starts_with("Repository") {
                return true;
            }
            match text[pos..].chars().next() {
                Some(c) if c.is_ascii_alphabetic() => pos += c.len_utf8(),
                _ => break,
            }
        }
        search_from = start + prefix.len();
    }
    false
}

/// One documented, explicitly-listed exemption: a specific file where a
/// specific banned needle is sanctioned, plus why. Matched by
/// `line.text.contains(needle)` — the needle is enough of a fingerprint that
/// removing the exempted code (or the fallback it names) makes the exemption
/// stop firing, which the `assert_all_used` check below turns into a hard
/// failure rather than a silently-stale allowlist entry.
struct Exemption {
    file: &'static str,
    needle: &'static str,
    reason: &'static str,
}

/// Fail loudly, file+line, if any exemption in `exemptions` never matched —
/// an unused exemption is exactly as much drift as an unlisted violation.
fn assert_all_used(check: &str, exemptions: &[Exemption], used: &[bool]) {
    let stale: Vec<_> = exemptions
        .iter()
        .zip(used)
        .filter(|(_, used)| !**used)
        .map(|(e, _)| format!("  {} (needle `{}`): {}", e.file, e.needle, e.reason))
        .collect();
    assert!(
        stale.is_empty(),
        "{check}: {} allowlisted exemption(s) never matched any file — remove the stale \
         entry(ies) from tests/architecture.rs:\n{}",
        stale.len(),
        stale.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// (a) domain never mentions frust::, ::presentation (any use-form), or
//     ::data (any use-form)
// ---------------------------------------------------------------------------

/// A documented exception category: settings is the one
/// feature whose domain use cases ARE theming, so three of its files import
/// `frust::` value types (`Theme`/`Color`/`ColorScheme`/`Brightness`/etc,
/// composed by `compose()`) plus `use_cases/set_theme.rs`'s
/// `set_app_theme`/`clear_app_theme` calls — the app's single theming
/// side-effect site (pre-existing behavior, preserved verbatim per the
/// behavior-preserving bar). This exemption covers ONLY the `frust::` needle for
/// exactly these three files — every other domain ban (`presentation`,
/// `data`, in any form the segment tokenizer or expression needle covers)
/// still applies to settings' domain like every other feature's; only the
/// `frust::` needle consults this table.
#[test]
fn domain_never_imports_frust_presentation_or_data() {
    let frust_exemptions = [
        Exemption {
            file: "features/settings/domain/models.rs",
            needle: "frust::",
            reason: "compose()'s Theme/Color/ColorScheme/Brightness/DesignLanguage/TypeScale \
                  value-type imports — theming IS this domain's subject matter",
        },
        Exemption {
            file: "features/settings/domain/accent.rs",
            needle: "frust::",
            reason: "Brightness/Color/ColorScheme/Theme value-type imports, same rationale as \
                  models.rs above",
        },
        Exemption {
            file: "features/settings/domain/use_cases/set_theme.rs",
            needle: "frust::",
            reason: "set_app_theme/clear_app_theme — the app's single theming side-effect \
                  call site, the effect this use case exists to apply",
        },
    ];
    let mut used = vec![false; frust_exemptions.len()];

    let mut failures = Vec::new();
    for path in domain_files() {
        let relp = rel(&path);
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for line in production_lines(&contents) {
            let is_use = is_use_trigger_line(line.text.trim_start());

            // `frust::` — a plain substring needle, unaffected by the
            // segment tokenizer: it legitimately appears in non-use
            // expression contexts too (e.g. `frust::run(...)`), not just
            // `use` lines, so there is no use/non-use split to make here.
            if line.text.contains("frust::") {
                if let Some(idx) = frust_exemptions
                    .iter()
                    .position(|e| e.file == relp && line.text.contains(e.needle))
                {
                    used[idx] = true;
                } else {
                    failures.push(format!(
                        "{relp}:{}: domain file mentions banned `frust::` — {}",
                        line.number,
                        line.text.trim()
                    ));
                }
            }

            // `presentation` / `data` path segments — gap-B segment
            // tokenizer on a joined `use` line (any brace-list/terminator
            // form), the reduced expression needle otherwise (a
            // fully-qualified non-use path expression) — see
            // has_exact_segment's and DATA_EXPR_NEEDLE's doc comments.
            for (segment, expr_needle) in [
                ("presentation", PRESENTATION_EXPR_NEEDLE),
                ("data", DATA_EXPR_NEEDLE),
            ] {
                let hit = if is_use {
                    has_exact_segment(&line.text, segment)
                } else {
                    line.text.contains(expr_needle)
                };
                if hit {
                    failures.push(format!(
                        "{relp}:{}: domain file mentions banned `{segment}` segment — {}",
                        line.number,
                        line.text.trim()
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "domain layering ban violated ({} file:line hit(s)) — a domain file must not mention \
         `frust::` (except the settings allowlist above), or a `presentation`/`data` path \
         segment in a use tree or fully-qualified expression:\n{}",
        failures.len(),
        failures.join("\n"),
    );
    assert_all_used(
        "domain_never_imports_frust_presentation_or_data",
        &frust_exemptions,
        &used,
    );
}

// ---------------------------------------------------------------------------
// (b) presentation never mentions ::data (any use-form)
// ---------------------------------------------------------------------------

/// A second sanctioned exemption category: `MessagesController::resolve_repo()`'s
/// `StoreMessageRepository` fallback
/// is a documented composition-root reference in production code — the ONE
/// presentation->data edge in the whole crate that isn't `#[cfg(test)]`-gated
/// (unlike the channels feature's precedent). It exists because
/// `new`/`with_latency`/`for_channel` are public constructors external
/// integration-test crates call with a channel id only, so a repo parameter
/// would force non-import-line test edits (forbidden by the
/// behavior-preserving bar);
/// the injected and fallback repos are the identical `StoreMessageRepository`
/// type, so behavior is unchanged either way.
#[test]
fn presentation_never_imports_data_directly() {
    let exemptions = [Exemption {
        file: "features/messages/presentation/controllers.rs",
        needle: "::data::",
        reason: "resolve_repo()'s StoreMessageRepository fallback — documented \
                  composition-root reference kept because new/with_latency/for_channel \
                  must stay signature-stable for external integration-test crates",
    }];
    let mut used = vec![false; exemptions.len()];

    let mut failures = Vec::new();
    for path in presentation_files() {
        let relp = rel(&path);
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for line in production_lines(&contents) {
            // Same use/non-use split as check (a): the gap-B segment
            // tokenizer on a joined `use` line, the reduced expression
            // needle otherwise.
            let hit = if is_use_trigger_line(line.text.trim_start()) {
                has_exact_segment(&line.text, "data")
            } else {
                line.text.contains(DATA_EXPR_NEEDLE)
            };
            if !hit {
                continue;
            }
            if let Some(idx) = exemptions
                .iter()
                .position(|e| e.file == relp && line.text.contains(e.needle))
            {
                used[idx] = true;
                continue;
            }
            failures.push(format!(
                "{relp}:{}: presentation file reaches into a `data` module — {}",
                line.number,
                line.text.trim()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "presentation layering ban violated ({} file:line hit(s)) — a presentation file must \
         not mention a `data` path segment (in a use tree or fully-qualified expression) \
         outside the resolve_repo allowlist above:\n{}",
        failures.len(),
        failures.join("\n"),
    );
    assert_all_used(
        "presentation_never_imports_data_directly",
        &exemptions,
        &used,
    );
}

// ---------------------------------------------------------------------------
// (c) only */data/** (and src/data/ itself) reads crate::data::store
// ---------------------------------------------------------------------------

/// Unlike (a)/(b), this needle (`"crate::data::store"`) is already a
/// complete path ending in a real segment (`store`), not a bare module name
/// an alias can strand mid-path — `use crate::data::store as s;` still
/// contains the full `"crate::data::store"` substring before ` as s;`, so
/// this check doesn't need [`has_exact_segment`]'s segment-tokenizer
/// treatment the way checks (a)/(b) do.
#[test]
fn only_data_layer_reads_the_shared_store_directly() {
    let mut failures = Vec::new();
    for path in rust_files(&src_dir()) {
        let relp = rel(&path);
        if is_data_layer_file(&relp) {
            continue;
        }
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for line in production_lines(&contents) {
            if line.text.contains("crate::data::store") {
                failures.push(format!(
                    "{relp}:{}: reads `crate::data::store` outside a `data/` layer — {}",
                    line.number,
                    line.text.trim()
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "shared-store-access ban violated ({} file:line hit(s)) — only a `*/data/**` file (or \
         src/data/ itself) may mention `crate::data::store`:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// (d) no file mentions crate::mock or crate::screens (both deleted)
// ---------------------------------------------------------------------------

/// Unlike (a)-(c)/(e), this scans **every line, including comments and test
/// regions** (raw `contents.lines()`, not [`production_lines`]) — `src/mock/`
/// and `src/screens/` no longer exist anywhere in this crate,
/// so there is no legitimate reason for even a doc-comment prose mention of
/// either path to survive; any hit means a stale reference the migration that
/// deleted them should have caught.
#[test]
fn no_file_mentions_deleted_mock_or_screens_modules() {
    let mut failures = Vec::new();
    for path in rust_files(&src_dir()) {
        let relp = rel(&path);
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for (i, raw) in contents.lines().enumerate() {
            for needle in ["crate::mock", "crate::screens"] {
                if raw.contains(needle) {
                    failures.push(format!(
                        "{relp}:{}: mentions deleted module `{needle}` — {}",
                        i + 1,
                        raw.trim()
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "stale-module ban violated ({} file:line hit(s)) — `src/mock/` and `src/screens/` are \
         both deleted; no file may mention `crate::mock` or `crate::screens`:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// (e) no file outside */data/**, src/lib.rs, or the messages resolve_repo
//     allowlist entry names a concrete Store*Repository/InMemory*Repository
//     type — closes a concrete-repo-name bypass (both direct
//     construction and a facade re-export)
// ---------------------------------------------------------------------------

/// An early adversarial review's second demonstrated bypass: a facade re-export
/// (`pub use data::repositories::StoreChannelRepository;` from a feature's
/// `mod.rs`) crosses the layer boundary by *type name*, not by path, so
/// neither the `::data`/`::presentation` needles above nor the `crate::
/// data::store` needle (check c) would ever see it. This check instead bans
/// the concrete-repository-name *pattern itself*
/// ([`contains_concrete_repo_name`]) everywhere except:
///
/// - a `*/data/**` file (or `src/data/` itself, [`is_data_layer_file`]) — a
///   feature's own `data/repositories.rs` legitimately declares and names
///   its own `Store*Repository` struct;
/// - `src/lib.rs` — the composition root, a **structural** carve-out (not an
///   allowlist entry: it's the one sanctioned site every concrete repository
///   type is constructed at and wired into `provide_context`, not a narrow
///   documented exception to a general ban — see the module header);
/// - `features/messages/presentation/controllers.rs`'s `resolve_repo()`
///   fallback — the same documented presentation->data edge
///   [`presentation_never_imports_data_directly`] already allowlists, now
///   also covering this check's concrete-name needle (its `use` import line
///   and its `StoreMessageRepository::new()` construction both match one
///   `"StoreMessageRepository"` needle).
#[test]
fn no_concrete_repo_type_name_outside_data_layer_or_composition_root() {
    let exemptions = [Exemption {
        file: "features/messages/presentation/controllers.rs",
        needle: "StoreMessageRepository",
        reason: "resolve_repo()'s composition-root-equivalent fallback (import + \
                  construction) — the same presentation->data edge \
                  presentation_never_imports_data_directly already allowlists",
    }];
    let mut used = vec![false; exemptions.len()];

    let mut failures = Vec::new();
    for path in rust_files(&src_dir()) {
        let relp = rel(&path);
        if relp == "lib.rs" || is_data_layer_file(&relp) {
            continue; // structural carve-outs: the composition root and the data layer itself
        }
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for line in production_lines(&contents) {
            if !contains_concrete_repo_name(&line.text) {
                continue;
            }
            if let Some(idx) = exemptions
                .iter()
                .position(|e| e.file == relp && line.text.contains(e.needle))
            {
                used[idx] = true;
                continue;
            }
            failures.push(format!(
                "{relp}:{}: names a concrete Store*Repository/InMemory*Repository type outside \
                 the data layer/composition root/allowlist — {}",
                line.number,
                line.text.trim()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "concrete-repo-name ban violated ({} file:line hit(s)) — only a `*/data/**` file, \
         `src/lib.rs` (composition root), or the messages resolve_repo allowlist above may \
         name a concrete Store*Repository/InMemory*Repository type:\n{}",
        failures.len(),
        failures.join("\n"),
    );
    assert_all_used(
        "no_concrete_repo_type_name_outside_data_layer_or_composition_root",
        &exemptions,
        &used,
    );
}

// ---------------------------------------------------------------------------
// (f) search/profile domain+data never mention HuddleFailure, ControllerCore,
//     or async fn (a sync-infallible-shape ratchet)
// ---------------------------------------------------------------------------

/// A documented design decision: `search` and `profile` are the two
/// sync-infallible features (`filter`/`load` return a plain value, never a
/// `Result<_, HuddleFailure>` behind a `ControllerCore`/`async fn` spine like
/// channels/messages/activity/settings). This is a ratchet against future
/// drift back toward that spine — a genuinely fallible future data source
/// for either feature is a considered redesign, not a one-line addition (see
/// `features/search/domain/repositories.rs`'s and
/// `features/profile/domain/repositories.rs`'s own module docs).
#[test]
fn search_and_profile_domain_and_data_stay_sync_infallible() {
    let roots = [
        src_dir().join("features/search/domain"),
        src_dir().join("features/search/data"),
        src_dir().join("features/profile/domain"),
        src_dir().join("features/profile/data"),
    ];

    let mut failures = Vec::new();
    for root in roots {
        for path in rust_files(&root) {
            let relp = rel(&path);
            let contents = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
            for line in production_lines(&contents) {
                for needle in ["HuddleFailure", "ControllerCore", "async fn"] {
                    if line.text.contains(needle) {
                        failures.push(format!(
                            "{relp}:{}: search/profile domain or data mentions banned `{needle}` \
                             (Design Decision 8 ratchet) — {}",
                            line.number,
                            line.text.trim()
                        ));
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "Design Decision 8 ratchet violated ({} file:line hit(s)) — search/profile domain+data \
         must stay sync-infallible:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// (g) trailing `//` comments on a use line don't trip the segment scan
//     (one confirmed high-severity finding, LATENT/fails-safe; closed by
//     the third round of fixes)
// ---------------------------------------------------------------------------

/// A further-review finding: before the third round of fixes, [`has_exact_segment`] ran over
/// un-comment-stripped `use`-line text, so a trailing `// … data …` comment
/// on an otherwise-clean `use` line could trip an exact-token false positive
/// on the comment's own prose, not on anything actually imported. This test
/// exercises [`production_lines`]/[`join_use_statements`]/[`has_exact_segment`]
/// directly on literal strings (no file I/O — this file is itself a test
/// file, so a unit test in its own module is in scope) to prove:
///
/// 1. a trailing comment mentioning `data` on an already-complete
///    single-line `use` statement does NOT trip the scan;
/// 2. a trailing comment mentioning `data` on the FIRST physical line of a
///    split `use` statement (i.e. present before the join loop even starts
///    accumulating) does NOT trip the scan either;
/// 3. a REAL `data` segment inside the `use` tree itself (no comment
///    involved) still DOES trip the scan — proving the strip didn't weaken
///    detection, only blinded it to comment prose.
#[test]
fn trailing_comment_on_use_line_does_not_trip_the_segment_scan() {
    // Case 1: single-line, comment trails the terminating `;`.
    let single_line = "use crate::features::messages::domain::FeedMessage; // now covers data too";
    let joined = production_lines(single_line);
    assert_eq!(joined.len(), 1, "expected exactly one production line");
    assert!(
        !has_exact_segment(&joined[0].text, "data"),
        "a trailing comment on a single-line use statement must not trip the `data` segment \
         scan, got: {:?}",
        joined[0].text,
    );

    // Case 2: split use statement, comment trails the FIRST physical line
    // (before the statement's `;` — and before the join loop has
    // accumulated anything from the remaining lines).
    let split_with_comment = "use crate::features::messages:: // data is mentioned here, but only in a comment\n    domain::FeedMessage;";
    let joined = production_lines(split_with_comment);
    assert_eq!(
        joined.len(),
        1,
        "expected the split use statement to join into one logical line"
    );
    assert!(
        !has_exact_segment(&joined[0].text, "data"),
        "a trailing comment on a split use statement's first physical line must not trip the \
         `data` segment scan, got: {:?}",
        joined[0].text,
    );

    // Negative control: a real `data` segment inside a brace-list use tree
    // (gap B's shape) — no comment anywhere — must still trip the scan.
    let real_violation = "use crate::features::messages::{domain::FeedMessage, \
                           data::repositories::StoreMessageRepository};";
    let joined = production_lines(real_violation);
    assert_eq!(joined.len(), 1);
    assert!(
        has_exact_segment(&joined[0].text, "data"),
        "a real `data` segment in a use tree (no comment involved) must still trip the segment \
         scan, got: {:?}",
        joined[0].text,
    );
}
